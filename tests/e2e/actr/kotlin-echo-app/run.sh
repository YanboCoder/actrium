#!/usr/bin/env bash
#
# kotlin-echo-app/run.sh — Kotlin Android nightly e2e (echo scenario).
#
# Mirrors tests/e2e/actr/swift-echo-app: a Rust EchoService is scaffolded, built, published
# to local loran and run; a Kotlin Android client (linked mode, scaffolded via
# `actr init -l kotlin --template echo`) calls echo.EchoService.Echo on it; the
# EchoIntegrationTest asserts the reply. Verification is `./gradlew
# connectedDebugAndroidTest` on an already-booted Android emulator (CI: booted by
# reactivecircus/android-emulator-runner; locally: boot one beforehand).
#
# Heavy work (native .so + AAR publish) is done up front to avoid an idle emulator.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"

source "$REPO_ROOT/tests/e2e/actr/package-runtime-echo/lib/common.sh"
source "$SCRIPT_DIR/../kotlin-lib/lib/readiness.sh"
source "$SCRIPT_DIR/../kotlin-lib/lib/kotlin-app.sh"

HTTP_PORT=8081
ICE_PORT=3478
ADMIN_PASSWORD="e2e-test-password"
PUBLISHER="e2e.actrium.example"
ACTR_CLI_MANIFEST="$REPO_ROOT/crates/actr-cli/Cargo.toml"
LORAN_CONFIG_TEMPLATE="$REPO_ROOT/tests/e2e/actr/package-runtime-echo/config/loran.toml"
E2E_TARGET_ROOT="$REPO_ROOT/target/e2e-cache/kotlin-echo-app"
ACTR_TARGET_DIR="$E2E_TARGET_ROOT/actr-cli"
TEMP_SERVICE_TARGET_DIR="$E2E_TARGET_ROOT/temp-service"

REALM_NAME_PREFIX="kotlin-echo-app"
SERVICE_ACTOR="EchoService"
APP_ACTOR="KotlinEchoApp"
APP_VERSION="0.1.0"
APPLICATION_ID="io.actrium.kotlinechoapp"
SHORT_BACKGROUND_SECONDS="${SHORT_BACKGROUND_SECONDS:-5}"
LONG_BACKGROUND_SECONDS="${LONG_BACKGROUND_SECONDS:-60}"

for cmd in cargo curl jq sqlite3 python3 perl rustc lsof adb; do
    require_cmd "$cmd"
done

RUN_ID="$(date +%Y%m%d-%H%M%S)-$RANDOM"
RUN_DIR="$SCRIPT_DIR/.tmp/run-$RUN_ID"
STATE_DIR="$RUN_DIR/state"
SQLITE_DIR="$STATE_DIR/sqlite"
LOG_DIR="$RUN_DIR/logs"
DIST_DIR="$RUN_DIR/dist"
TMP_SERVICE_ROOT="$RUN_DIR/workspace"
TMP_SERVICE_DIR="$TMP_SERVICE_ROOT/echo-actr-$RANDOM"
TMP_APP_DIR="$RUN_DIR/app"
LORAN_CONFIG_PATH="$RUN_DIR/loran.toml"
SERVER_RUNTIME_PATH="$RUN_DIR/server-runtime.toml"
SERVICE_KEYCHAIN="$TMP_SERVICE_DIR/packaging/keys/publisher.keychain.json"
SERVICE_PUBLIC_KEY="$TMP_SERVICE_DIR/public-key.json"
PROVISIONED_KEYCHAIN="$RUN_DIR/publisher.keychain.json"
PROVISIONED_PUBLIC_KEY="$RUN_DIR/publisher-public-key.json"

mkdir -p "$SQLITE_DIR" "$LOG_DIR" "$DIST_DIR" "$TMP_SERVICE_ROOT" "$TMP_APP_DIR" "$E2E_TARGET_ROOT"

LORAN_PID=""
SERVER_PID=""
ACTR_CLI_BIN=""
LORAN_BIN=""
ADMIN_TOKEN=""
SERVICE_PACKAGE=""
SERVICE_VERSION=""
REALM=""
REALM_OWNER_KEY=""
REALM_CLIENT_KEY=""

trap kotlin_cleanup EXIT INT TERM

# ──── Rust EchoService scaffold (mirrors swift-echo-app scaffold_service_guest) ────

scaffold_echo_service() {
    section "🧱 Scaffolding temporary Rust EchoService"
    run_actr init \
        -l rust --template echo --role service \
        --signaling "ws://127.0.0.1:${HTTP_PORT}/signaling/ws" \
        --publisher "$PUBLISHER" \
        --realm "$REALM" \
        "$TMP_SERVICE_DIR"

    append_workspace_patch "$TMP_SERVICE_DIR/Cargo.toml"
    python3 "$REPO_ROOT/scripts/actr/apply-upstream-patches.py" "$TMP_SERVICE_DIR/Cargo.toml"
    pin_workspace_actr_dependencies "$TMP_SERVICE_DIR/Cargo.toml"
    mkdir -p "$(dirname "$SERVICE_KEYCHAIN")"
    cp "$PROVISIONED_KEYCHAIN" "$SERVICE_KEYCHAIN"
    cp "$PROVISIONED_PUBLIC_KEY" "$SERVICE_PUBLIC_KEY"
    write_project_keychain_config "$TMP_SERVICE_DIR" "$SERVICE_KEYCHAIN"

    (
        cd "$TMP_SERVICE_DIR"
        CARGO_TARGET_DIR="$TEMP_SERVICE_TARGET_DIR" run_actr deps install
        CARGO_TARGET_DIR="$TEMP_SERVICE_TARGET_DIR" run_actr gen -l rust
    )

    SERVICE_VERSION="$(
        awk '/^\[package\]/ {p=1; next} /^\[/ && p {exit} p && $1 == "version" {gsub(/"/, "", $3); print $3; exit}' \
            "$TMP_SERVICE_DIR/manifest.toml"
    )"
    [ -n "$SERVICE_VERSION" ] || fail "Unable to detect EchoService version"
    success "Rust EchoService ready: version ${SERVICE_VERSION}"
}

# Write the client manifest.toml — per actr-kt-migration §2.1 this file carries
# ONLY [package] (app ActrType) + [dependencies]. Connection config + ACL live in
# actr.toml (render_linked_client_config). [package].name is the app actor name so
# resolveManifestPackageActrType yields ${PUBLISHER}/${APP_ACTOR}@${APP_VERSION}.
write_client_manifest() {
    section "📝 Writing Kotlin client manifest.toml"
    local acl_type="${PUBLISHER}/${SERVICE_ACTOR}@${SERVICE_VERSION}"
    cat >"$TMP_APP_DIR/manifest.toml" <<EOF
edition = 1

[package]
name = "${APP_ACTOR}"
publisher = "${PUBLISHER}"
version = "${APP_VERSION}"
description = "Actrium Kotlin EchoApp e2e client"

[dependencies]
${SERVICE_ACTOR} = { actr_type = "${acl_type}" }
EOF
    success "Client manifest.toml written (dep ${acl_type})"
}

# Place the remote EchoService proto under its manifest dependency alias.
# DispatchSpec reads this alias from the path and linked runtime resolution
# reads the same alias from manifest.toml; no runtime lock is needed.
seed_client_protos() {
    section "📄 Seeding client protos"
    mkdir -p "$TMP_APP_DIR/protos/remote/${SERVICE_ACTOR}"
    cp "$REPO_ROOT/tests/e2e/actr/package-runtime-echo/proto/echo.proto" \
       "$TMP_APP_DIR/protos/remote/${SERVICE_ACTOR}/echo.proto"
    success "Remote echo.proto written for dependency alias ${SERVICE_ACTOR}"
}

# Drop the correct linked-mode instrumentation test into the scaffolded app,
# and replace the fixture's (non-compiling, pre-0.4) MainActivity with a minimal
# launcher stub — the instrumentation test drives the echo round-trip.
write_instrumentation_test() {
    section "✍️  Writing linked-mode instrumentation test + minimal MainActivity"
    local pkg_dir="$TMP_APP_DIR/app/src/androidTest/java/${APPLICATION_ID//.//}"
    local main_dir="$TMP_APP_DIR/app/src/main/java/${APPLICATION_ID//.//}"
    mkdir -p "$pkg_dir" "$main_dir"

    render_template \
        "$REPO_ROOT/tests/e2e/actr/kotlin-lib/EchoIntegrationTest.kt" \
        "$pkg_dir/EchoIntegrationTest.kt" \
        "__PACKAGE__=$APPLICATION_ID"
    rm -f "$pkg_dir/EchoIntegrationTest.kt.bak"

    render_template \
        "$REPO_ROOT/tests/e2e/actr/kotlin-lib/MainActivity.kt.tpl" \
        "$main_dir/MainActivity.kt" \
        "__PACKAGE__=$APPLICATION_ID"
    rm -f "$main_dir/MainActivity.kt.bak"

    success "Instrumentation test + minimal MainActivity written"
}

HOST_TARGET="$(rustc -vV | awk '/host:/ {print $2}')"

# ──── Main ────

section "🧪 Kotlin EchoApp E2E"
echo "Run directory:  $RUN_DIR"
echo "Service actor:  ${PUBLISHER}:${SERVICE_ACTOR}"
echo "App actor:      ${PUBLISHER}/${APP_ACTOR}@${APP_VERSION}"
echo "Loran binary:  $LORAN_BIN"
echo "Host target:    $HOST_TARGET"

# Phase 0: native lib + local AAR (before any emulator dependence).
# Skip with SKIP_AAR_BUILD=1 when mavenLocal already has the AAR (local iteration
# or CI cache hit) to avoid the ~10 min native rebuild.
if [ "${SKIP_AAR_BUILD:-0}" != "1" ]; then
    build_and_publish_aar
else
    success "SKIP_AAR_BUILD=1 — assuming io.actrium:actr:${ACTR_KOTLIN_VERSION} is already in mavenLocal"
fi

# Phase 1: loran infrastructure.
HOST_IP="$(detect_host_lan_ip)"
export HOST_IP
section "🌐 Host LAN IP for emulator connectivity: $HOST_IP"
render_runtime_configs
# The shared template's enable=25 = SIGNALING|Issuer|SIGNER (STUN/TURN bits NOT set),
# so loran logs "ICE service (STUN/TURN) is disabled" and never starts the STUN/TURN server.
# The Android emulator needs STUN+TURN (direct ICE can't cross the QEMU NAT), so
# add ENABLE_STUN(2) + ENABLE_TURN(4) → enable=31.
perl -0pi -e 's/^enable = 25$/enable = 31/m' "$LORAN_CONFIG_PATH"
# Bind ICE (and HTTP) on all interfaces and advertise the host LAN IP so the
# Android emulator can reach STUN/TURN. Patterns are line-anchored (^) so the
# `ip = ` rewrite doesn't also clobber `advertised_ip`.
perl -0pi -e 's/^(ip) = "127\.0\.0\.1"/${1} = "0.0.0.0"/mg' "$LORAN_CONFIG_PATH"
perl -0pi -e "s/^advertised_ip = \"127\\.0\\.0\\.1\"/advertised_ip = \"${HOST_IP}\"/mg" "$LORAN_CONFIG_PATH"
rm -f "${LORAN_CONFIG_PATH}.bak"
build_local_actr_cli
build_local_loran
start_loran
login_admin
wait_for_signer
ensure_realm
provision_publisher_keychain

# Phase 2: scaffold, build, and verify the Rust EchoService.
scaffold_echo_service
build_service_package "$TMP_SERVICE_DIR" "$SERVICE_ACTOR" "$SERVICE_VERSION"

# Phase 3: start the EchoService host and wait for registration.
run_server_host "$SERVICE_PACKAGE" "${PUBLISHER}/${APP_ACTOR}@${APP_VERSION}" "kotlin-echo-server"
check_service_ready "$SERVICE_ACTOR"

# Phase 4: scaffold the Kotlin client, wire local AAR, seed protos, gen.
# No `actr deps install` is needed: codegen and the linked runtime both source
# the alias mapping from manifest.toml.
scaffold_kotlin_app echo "$APP_ACTOR" "$TMP_APP_DIR" \
    "ws://10.0.2.2:${HTTP_PORT}/signaling/ws" "$PUBLISHER"
write_client_manifest
seed_client_protos
substitute_local_aar "$TMP_APP_DIR"
run_kotlin_gen "$TMP_APP_DIR" "$APPLICATION_ID"

# Phase 5: linked-mode client config + instrumentation test.
render_linked_client_config "$TMP_APP_DIR" "$APPLICATION_ID" \
    "${PUBLISHER}/${SERVICE_ACTOR}@${SERVICE_VERSION}"
write_instrumentation_test

# Phase 6: run the instrumentation test on the emulator.
# Set ANDROID_SERIAL when multiple devices are attached (e.g. a phone + emulator).
run_connected_android_test "$TMP_APP_DIR" "$APPLICATION_ID"

echo ""
success "Kotlin EchoApp e2e completed successfully"
