#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"

source "$SCRIPT_DIR/lib/common.sh"
source "$SCRIPT_DIR/lib/realm-scenarios.sh"

# Keep the provenance matrix independent of the Realm creation matrix.
REALM_MODE="${REALM_MODE:-legacy}"
case "$REALM_MODE" in
    legacy|provider|external) ;;
    *) fail "Unknown REALM_MODE: $REALM_MODE" ;;
esac
REALM_STAGE="${REALM_STAGE:-full}"
case "$REALM_STAGE" in full|profile) ;; *) fail "Unknown REALM_STAGE: $REALM_STAGE" ;; esac
if [ "$REALM_STAGE" = profile ] && [ "$REALM_MODE" = legacy ]; then
    fail "REALM_STAGE=profile requires a provider or external Realm"
fi

HTTP_PORT="${HTTP_PORT:-8081}"
HTTP_BASE="http://127.0.0.1:${HTTP_PORT}"
WS_BASE="ws://127.0.0.1:${HTTP_PORT}"
ICE_PORT="${ICE_PORT:-3478}"
REALM=""
ADMIN_PASSWORD="e2e-test-password"
PUBLISHER="e2e.actrium.example"
# The client guest is a realm consumer, not a verified publisher: it keeps a
# non-dotted publisher so its linked registrations satisfy Issuer-01, which
# reserves DNS-style names for package identities.
CLIENT_PUBLISHER="e2e-actrium-local"
CLIENT_GUEST_VERSION="0.1.0"
LORAN_BIN="${LORAN_BIN:-}"
ACTR_CLI_MANIFEST="$REPO_ROOT/crates/actr-cli/Cargo.toml"
E2E_TARGET_ROOT="$REPO_ROOT/target/e2e-cache/package-runtime-echo"
ACTR_TARGET_DIR="${ACTR_TARGET_DIR:-$E2E_TARGET_ROOT/actr-cli}"
LORAN_TARGET_DIR="${LORAN_TARGET_DIR:-$E2E_TARGET_ROOT/loran-bin}"
WORKSPACE_TARGET_DIR="${WORKSPACE_TARGET_DIR:-$E2E_TARGET_ROOT/workspace}"
TEMP_SERVICE_TARGET_DIR="${TEMP_SERVICE_TARGET_DIR:-$E2E_TARGET_ROOT/temp-service}"
DEFAULT_MESSAGE="TmpFlow"

BACKEND="cdylib"
TEST_INPUT="$DEFAULT_MESSAGE"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --backend)
            [[ $# -lt 2 ]] && fail "Missing value for --backend"
            BACKEND="$2"
            shift 2
            ;;
        --backend=*)
            BACKEND="${1#--backend=}"
            shift
            ;;
        -*)
            fail "Unknown option: $1"
            ;;
        *)
            TEST_INPUT="$1"
            shift
            ;;
    esac
done

if [[ "$BACKEND" != "cdylib" ]]; then
    fail "Only --backend cdylib is supported in this scenario"
fi

# MODE selects the scenario. The service host never holds the Publisher's
# private key: it registers with the signed package and its Realm client key.
#   runtime   (default) signed package runtime lifecycle
#   rotation  Publisher key retired (pass) then revoked (reject)
MODE="${MODE:-runtime}"
case "$MODE" in
    runtime|rotation) ;;
    *) fail "Unknown MODE: $MODE" ;;
esac

for cmd in cargo curl jq sqlite3 python3 perl rustc lsof timeout; do
    require_cmd "$cmd"
done

# Both embedded manifest parsers read TOML via the stdlib. Require Python
# 3.11+ (the tomllib floor) instead of shipping a tomli fallback that no CI
# runner ever exercises.
python3 -c 'import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)' ||
    fail "Python 3.11+ is required for stdlib tomllib (found $(python3 -V 2>&1))"

RUN_ID="$(date +%Y%m%d-%H%M%S)-$RANDOM"
RUN_DIR="$SCRIPT_DIR/.tmp/run-$RUN_ID"
STATE_DIR="$RUN_DIR/state"
SQLITE_DIR="$STATE_DIR/sqlite"
LOG_DIR="$RUN_DIR/logs"
DIST_DIR="$RUN_DIR/dist"
TMP_SERVICE_ROOT="$RUN_DIR/workspace"
TMP_SERVICE_DIR="$TMP_SERVICE_ROOT/echo-actr-$RANDOM"
LORAN_CONFIG_PATH="$RUN_DIR/loran.toml"
SERVER_RUNTIME_PATH="$RUN_DIR/server-runtime.toml"
CLIENT_RUNTIME_PATH="$RUN_DIR/client-runtime.toml"
LORAN_DB="$SQLITE_DIR/loran.db"
PUBLISHER_PRIVATE_DIR="$RUN_DIR/publisher-private"
BUILDER_HOME="$RUN_DIR/home-builder"
BUILDER_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
BUILDER_RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
SERVICE_KEYCHAIN="$PUBLISHER_PRIVATE_DIR/package.keychain.json"
SERVICE_PUBLIC_KEY="$TMP_SERVICE_DIR/public-key.json"
PROVISIONED_KEYCHAIN="$SERVICE_KEYCHAIN"
PROVISIONED_PUBLIC_KEY="$RUN_DIR/publisher-public-key.json"
CLIENT_GUEST_PACKAGE="$DIST_DIR/${CLIENT_PUBLISHER}-pkg-runtime-echo-client-guest-${CLIENT_GUEST_VERSION}-cdylib.actr"
CLIENT_GUEST_PUBLIC_KEY="$DIST_DIR/public-key.json"

umask 077
mkdir -p "$SQLITE_DIR" "$LOG_DIR" "$DIST_DIR" "$TMP_SERVICE_ROOT" "$E2E_TARGET_ROOT" "$PUBLISHER_PRIVATE_DIR" "$BUILDER_HOME"
RUNTIME_LAUNCHER=(python3 "$SCRIPT_DIR/lib/runtime-isolation.py" --private-dir "$PUBLISHER_PRIVATE_DIR" --)

LORAN_PID=""
HTTPS_PID=""
SERVER_PID=""
CLIENT_PID=""
ACTR_CLI_BIN=""
ADMIN_TOKEN=""
SERVICE_PACKAGE=""
SERVICE_VERSION=""
REALM_NAME_PREFIX="package-runtime-echo"
REALM_OWNER_KEY=""
REALM_CLIENT_KEY=""
REALM_CLIENT_MEMBER_ID=""
HOST_TARGET="$(rustc -vV | awk '/host:/ {print $2}')"

cleanup() {
    local status=$?

    if [ -n "$CLIENT_PID" ] && kill -0 "$CLIENT_PID" 2>/dev/null; then
        kill "$CLIENT_PID" 2>/dev/null || true
    fi
    if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
    fi
    if [ -n "$LORAN_PID" ] && kill -0 "$LORAN_PID" 2>/dev/null; then
        kill "$LORAN_PID" 2>/dev/null || true
    fi
    if [ -n "$HTTPS_PID" ] && kill -0 "$HTTPS_PID" 2>/dev/null; then
        kill "$HTTPS_PID" 2>/dev/null || true
    fi
    wait 2>/dev/null || true

    if [ $status -eq 0 ] && [ "${KEEP_TMP:-0}" != "1" ]; then
        rm -rf "$RUN_DIR"
    else
        echo ""
        echo "Artifacts preserved at: $RUN_DIR"
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

run_actr() {
    # pkg keygen writes ~/.actr/config.toml even with an explicit output.
    # Isolate all CLI configuration while keeping the existing toolchain cache.
    HOME="$BUILDER_HOME" CARGO_HOME="$BUILDER_CARGO_HOME" RUSTUP_HOME="$BUILDER_RUSTUP_HOME" \
        CARGO_TARGET_DIR="$ACTR_TARGET_DIR" "$ACTR_CLI_BIN" "$@"
}

build_local_actr_cli() {
    section "🔧 Building local actr CLI"
    CARGO_TARGET_DIR="$ACTR_TARGET_DIR" cargo build --manifest-path "$ACTR_CLI_MANIFEST" --bin actr >/dev/null
    ACTR_CLI_BIN="$ACTR_TARGET_DIR/debug/actr"
    [ -x "$ACTR_CLI_BIN" ] || fail "actr CLI binary missing at $ACTR_CLI_BIN"
    success "actr CLI ready: $ACTR_CLI_BIN"
}

# Build a test-only server whose injected transport serves the disposable
# publisher's signed Profile. The production executable retains public HTTPS
# discovery and has no fixture override, including in all-features builds.
build_local_loran() {
    if [ -n "${LORAN_BIN:-}" ] && [ -x "$LORAN_BIN" ]; then
        "$LORAN_BIN" --list | grep -q '^package_runtime_fixture: test$' ||
            fail "LORAN_BIN must be the loran test binary containing package_runtime_fixture"
        success "Using caller-provided loran test fixture: $LORAN_BIN"
        return 0
    fi
    section "🔧 Building local loran from source"
    CARGO_TARGET_DIR="$LORAN_TARGET_DIR" cargo test --no-run --message-format=json \
        --manifest-path "$REPO_ROOT/crates/loran-server/Cargo.toml" \
        --bin loran >"$RUN_DIR/loran-fixture-build.jsonl"
    LORAN_BIN="$(jq -r 'select(.reason == "compiler-artifact" and .target.name == "loran" and .profile.test == true and .executable != null) | .executable' "$RUN_DIR/loran-fixture-build.jsonl" | tail -1)"
    [ -x "$LORAN_BIN" ] || fail "loran binary missing at $LORAN_BIN"
    export LORAN_BIN
    success "loran ready: $LORAN_BIN"
}

render_runtime_configs() {
    render_template \
        "$SCRIPT_DIR/config/loran.toml" \
        "$LORAN_CONFIG_PATH" \
        "__SQLITE_DIR__=$SQLITE_DIR" \
        "__HTTP_PORT__=$HTTP_PORT" \
        "__ICE_PORT__=$ICE_PORT"
}

start_loran() {
    section "🚀 Starting local loran"
    if lsof -tiTCP:"$HTTP_PORT" -sTCP:LISTEN >/dev/null 2>&1 \
        || lsof -tiUDP:"$ICE_PORT" >/dev/null 2>&1; then
        fail "Realm E2E ports are occupied; set HTTP_PORT and ICE_PORT to unused ports"
    fi

    # LORAN_RUST_LOG lets modes capture the debug provenance marker.
    RUST_LOG="${LORAN_RUST_LOG:-${RUST_LOG:-info}}" \
        ACTR_PACKAGE_RUNTIME_FIXTURE=1 ACTR_PACKAGE_RUNTIME_CONFIG="$LORAN_CONFIG_PATH" \
        "$LORAN_BIN" package_runtime_fixture --exact --ignored --nocapture >"$LOG_DIR/loran.log" 2>&1 &
    LORAN_PID=$!

    if ! wait_for_http_ok "${HTTP_BASE}/signaling/health" 120; then
        cat "$LOG_DIR/loran.log" >&2 || true
        fail "loran did not become healthy on port $HTTP_PORT"
    fi
    success "loran is healthy on ${HTTP_BASE}"
}

login_admin() {
    section "🔐 Logging into Admin API"
    local response_file="$RUN_DIR/admin-login.json"
    curl -fsS \
        -X POST \
        "${HTTP_BASE}/admin/api/auth/login" \
        -H 'Content-Type: application/json' \
        -d "{\"password\":\"${ADMIN_PASSWORD}\"}" \
        >"$response_file"
    ADMIN_TOKEN="$(json_field "$response_file" '.token')"
    success "Admin API login succeeded"
}

wait_for_signer() {
    section "🔑 Waiting for Signer"
    local health_file="$RUN_DIR/issuer-health.json"
    local attempt=0

    while [ $attempt -lt 60 ]; do
        if curl -fsS "${HTTP_BASE}/issuer/health" >"$health_file" 2>/dev/null \
            && [ "$(jq -r '.signer_service // "missing"' "$health_file" 2>/dev/null)" = "ok" ]; then
            success "Signer is ready"
            return 0
        fi
        sleep 1
        attempt=$((attempt + 1))
    done

    fail "Signer did not become ready"
}

append_workspace_patch() {
    local cargo_toml="$1"
    local repo_path="$REPO_ROOT"

    if ! grep -q '^\[workspace\]' "$cargo_toml"; then
        cat >>"$cargo_toml" <<'EOF'

[workspace]
EOF
    fi

    if grep -q '^\[patch\.crates-io\]' "$cargo_toml"; then
        return 0
    fi

    cat >>"$cargo_toml" <<EOF

[patch.crates-io]
actr = { path = "$repo_path/crates/actr" }
actr-config = { path = "$repo_path/crates/actr-config" }
actr-protocol = { path = "$repo_path/crates/actr-protocol" }
actr-framework = { path = "$repo_path/crates/actr-framework" }
actr-hyper = { path = "$repo_path/crates/actr-hyper" }
actr-pack = { path = "$repo_path/crates/actr-pack" }
actr-platform-native = { path = "$repo_path/crates/actr-platform-native" }
actr-platform-traits = { path = "$repo_path/crates/actr-platform-traits" }
actr-runtime = { path = "$repo_path/crates/actr-runtime" }
actr-runtime-mailbox = { path = "$repo_path/crates/actr-runtime-mailbox" }
actr-service-compat = { path = "$repo_path/crates/actr-service-compat" }
EOF
}

write_project_keychain_config() {
    local project_dir="$1"
    local keychain_path="$2"
    mkdir -p "$project_dir/.actr"
    cat >"$project_dir/.actr/config.toml" <<EOF
[publisher]
keychain = "$keychain_path"
EOF
}

provision_publisher_keychain() {
    provision_e2e_publisher
}

scaffold_service_guest() {
    section "🧱 Scaffolding temporary echo service"
    run_actr init \
        -l rust \
        --template echo \
        --role service \
        --signaling "${WS_BASE}/signaling/ws" \
        --publisher "$PUBLISHER" \
        --realm "$REALM" \
        "$TMP_SERVICE_DIR"

    append_workspace_patch "$TMP_SERVICE_DIR/Cargo.toml"
    python3 "$REPO_ROOT/scripts/actr/apply-upstream-patches.py" "$TMP_SERVICE_DIR/Cargo.toml"
    cp "$PROVISIONED_PUBLIC_KEY" "$SERVICE_PUBLIC_KEY"
    write_project_keychain_config "$TMP_SERVICE_DIR" "$SERVICE_KEYCHAIN"

    (
        cd "$TMP_SERVICE_DIR"
        CARGO_TARGET_DIR="$TEMP_SERVICE_TARGET_DIR" run_actr deps install
        CARGO_TARGET_DIR="$TEMP_SERVICE_TARGET_DIR" run_actr gen -l rust
    )

    SERVICE_VERSION="$(
        awk '
            /^\[package\]/ { in_package = 1; next }
            /^\[/ && in_package { exit }
            in_package && $1 == "version" {
                gsub(/"/, "", $3)
                print $3
                exit
            }
        ' "$TMP_SERVICE_DIR/manifest.toml"
    )"

    [ -n "$SERVICE_VERSION" ] || fail "Unable to detect temporary service version"
    success "Temporary echo service ready: version ${SERVICE_VERSION}"
}

build_service_package() {
    section "📦 Building the server package"
    SERVICE_PACKAGE="$DIST_DIR/${PUBLISHER}-EchoService-${SERVICE_VERSION}-${HOST_TARGET}.actr"

    (
        cd "$TMP_SERVICE_DIR"
        CARGO_TARGET_DIR="$TEMP_SERVICE_TARGET_DIR" run_actr build \
            --manifest-path manifest.toml \
            --key "$SERVICE_KEYCHAIN" \
            --output "$SERVICE_PACKAGE"
    )

    [ -f "$SERVICE_PACKAGE" ] || fail "Server package missing: $SERVICE_PACKAGE"
    run_actr pkg verify --pubkey "$SERVICE_PUBLIC_KEY" --package "$SERVICE_PACKAGE" >/dev/null
    success "Server package built: $SERVICE_PACKAGE"
}

client_guest_library_path() {
    case "$(uname)" in
        Darwin)
            printf '%s\n' "$WORKSPACE_TARGET_DIR/debug/libpackage_runtime_echo_client_guest.dylib"
            ;;
        Linux)
            printf '%s\n' "$WORKSPACE_TARGET_DIR/debug/libpackage_runtime_echo_client_guest.so"
            ;;
        *)
            printf '%s\n' "$WORKSPACE_TARGET_DIR/debug/package_runtime_echo_client_guest.dll"
            ;;
    esac
}

build_client_guest_package() {
    section "📦 Building client guest package"

    CARGO_TARGET_DIR="$WORKSPACE_TARGET_DIR" cargo build --manifest-path "$SCRIPT_DIR/client-guest/Cargo.toml" >/dev/null

    local client_guest_binary
    client_guest_binary="$(client_guest_library_path)"
    [ -f "$client_guest_binary" ] || fail "Client guest library missing: $client_guest_binary"

    local client_guest_dir client_guest_manifest
    client_guest_dir="$RUN_DIR/client-guest-package"
    client_guest_manifest="$client_guest_dir/manifest.toml"
    mkdir -p "$client_guest_dir/.actr"
    cp "$SCRIPT_DIR/client-guest/manifest.toml" "$client_guest_manifest"
    # Package builds now require a lock that exactly matches every declared
    # remote dependency. The CLI reads discovery settings from its project-level
    # .actr/config.toml, independently of the runtime actr.toml schema.
    cat >"$client_guest_dir/.actr/config.toml" <<EOF
version = 1

[network]
signaling_url = "${WS_BASE}/signaling/ws"
issuer_endpoint = "${HTTP_BASE}/issuer"
realm = "$REALM"
realm_client_key = "$REALM_CLIENT_KEY"
EOF
    (
        cd "$client_guest_dir"
        run_actr deps install
    )
    cat >>"$client_guest_manifest" <<EOF

[binary]
path = "$client_guest_binary"
target = "$HOST_TARGET"
EOF

    run_actr build \
        --no-compile \
        --manifest-path "$client_guest_manifest" \
        --key "$PROVISIONED_KEYCHAIN" \
        --target "$HOST_TARGET" \
        --output "$CLIENT_GUEST_PACKAGE"

    [ -f "$CLIENT_GUEST_PACKAGE" ] || fail "Client guest package missing: $CLIENT_GUEST_PACKAGE"

    cp "$PROVISIONED_PUBLIC_KEY" "$CLIENT_GUEST_PUBLIC_KEY"
    run_actr pkg verify --pubkey "$CLIENT_GUEST_PUBLIC_KEY" --package "$CLIENT_GUEST_PACKAGE" >/dev/null
    success "Client guest package ready"
}

# The client package is verified by StaticTrust and registers as a development
# identity. Only the service publisher is provisioned in the Registry.
render_client_runtime_config() {
    render_template \
        "$SCRIPT_DIR/config/client-runtime.toml.tpl" \
        "$CLIENT_RUNTIME_PATH" \
        "__HTTP_PORT__=$HTTP_PORT" \
        "__ICE_PORT__=$ICE_PORT" \
        "__REALM__=$REALM" \
        "__ECHO_SERVICE_VERSION__=$SERVICE_VERSION" \
        "__REALM_CLIENT_KEY__=$REALM_CLIENT_KEY"
    apply_verified_realm_config "$CLIENT_RUNTIME_PATH"
}

run_client_and_assert() {
    section "🚀 Running client host"
    render_client_runtime_config
    CARGO_TARGET_DIR="$WORKSPACE_TARGET_DIR" cargo build --manifest-path "$SCRIPT_DIR/client/Cargo.toml" >/dev/null

    # The isolated HOME only keeps the hyper data dir away from the
    # developer's ~/.actr.
    local client_home="$RUN_DIR/home-client"
    mkdir -p "$client_home"

    (
        sleep 3
        echo "$TEST_INPUT"
        sleep 2
        echo "quit"
    ) | \
        HOME="$client_home" \
        ECHO_ACTR_VERSION="$SERVICE_VERSION" \
        ISSUER_ENDPOINT="${HTTP_BASE}/issuer" \
        CLIENT_RUNTIME_CONFIG_PATH="$CLIENT_RUNTIME_PATH" \
        CLIENT_GUEST_PACKAGE_PATH="$CLIENT_GUEST_PACKAGE" \
        CLIENT_GUEST_PUBLIC_KEY_PATH="$CLIENT_GUEST_PUBLIC_KEY" \
        RUST_LOG="${ACTR_E2E_RUST_LOG:-info}" \
        "${RUNTIME_LAUNCHER[@]}" "$WORKSPACE_TARGET_DIR/debug/package-runtime-echo-client" \
        >"$LOG_DIR/client.log" 2>&1 &
    CLIENT_PID=$!

    local timeout="${CLIENT_TIMEOUT_SECONDS:-40}"
    local attempt=0
    while kill -0 "$CLIENT_PID" 2>/dev/null && [ $attempt -lt "$timeout" ]; do
        sleep 1
        attempt=$((attempt + 1))
    done

    if kill -0 "$CLIENT_PID" 2>/dev/null; then
        kill "$CLIENT_PID" 2>/dev/null || true
        fail "Client host timed out after ${timeout}s"
    fi

    if grep -q "\[Received reply\].*Echo: ${TEST_INPUT}" "$LOG_DIR/client.log"; then
        success "End-to-end echo succeeded"
        grep "Received reply" "$LOG_DIR/client.log" || true
        return 0
    fi

    echo ""
    echo "Client log:"
    cat "$LOG_DIR/client.log" || true
    echo ""
    echo "Server log:"
    cat "$LOG_DIR/server.log" || true
    echo ""
    echo "Loran log:"
    cat "$LOG_DIR/loran.log" || true
    fail "Expected echo reply not found"
}

# Removing the client's member revokes its Realm client key: the same client
# host can no longer obtain an Actor credential.
assert_removed_member_refused() {
    section "🚫 Removing the client's member"
    run_actr realm remove-member --key "$REALM_OWNER_KEY" "$REALM_CLIENT_MEMBER_ID" >/dev/null
    run_actr realm members --key "$REALM_OWNER_KEY" >"$RUN_DIR/members.json"
    [ "$(jq -r --arg id "$REALM_CLIENT_MEMBER_ID" \
        '.members[] | select(.member_id == $id) | .status' "$RUN_DIR/members.json")" = "removed" ] ||
        fail "The client's member is not listed as removed"

    local client_home="$RUN_DIR/home-client-removed"
    mkdir -p "$client_home"
    local status=0
    echo "quit" | \
        HOME="$client_home" \
        ECHO_ACTR_VERSION="$SERVICE_VERSION" \
        ISSUER_ENDPOINT="${HTTP_BASE}/issuer" \
        CLIENT_RUNTIME_CONFIG_PATH="$CLIENT_RUNTIME_PATH" \
        CLIENT_GUEST_PACKAGE_PATH="$CLIENT_GUEST_PACKAGE" \
        CLIENT_GUEST_PUBLIC_KEY_PATH="$CLIENT_GUEST_PUBLIC_KEY" \
        RUST_LOG="${ACTR_E2E_RUST_LOG:-info}" \
        "${RUNTIME_LAUNCHER[@]}" timeout "${CLIENT_TIMEOUT_SECONDS:-40}" \
        "$WORKSPACE_TARGET_DIR/debug/package-runtime-echo-client" \
        >"$LOG_DIR/client-removed.log" 2>&1 || status=$?

    if [ "$status" -ne 0 ] && grep -q "client_revoked" "$LOG_DIR/client-removed.log"; then
        success "The removed member's client was refused a credential"
        return 0
    fi
    cat "$LOG_DIR/client-removed.log" || true
    fail "The removed member's client was not refused (exit status $status)"
}

# ============================================================================
# Mode helpers
# ============================================================================

# Per-mode isolated HOME and hyper data dir, so actr run never touches the real
# ~/.actr/config.toml or shares local state across modes/phases.
mk_isolated_env() {
    local label="$1"
    local home_dir="$RUN_DIR/home-$label"
    local hyper_dir="$RUN_DIR/hyper-$label"
    mkdir -p "$home_dir" "$hyper_dir"
    printf '%s\t%s\n' "$home_dir" "$hyper_dir"
}

# Publisher Profile/key caches are independent of the removed publication ledger.
# Require the obsolete tables to be absent and surface database errors.
assert_no_publication_tables() {
    python3 - "$LORAN_DB" <<'PY'
import sqlite3
import sys
with sqlite3.connect(sys.argv[1]) as connection:
    tables = connection.execute(
        "SELECT name FROM sqlite_master WHERE type = 'table' "
        "AND name IN ('publisher_package', 'publisher_publish_nonce')"
    ).fetchall()
    if tables:
        raise SystemExit(f"obsolete publication tables exist: {tables}")
PY
}

# Render the server runtime config (reuses the existing template).
render_server_runtime() {
    render_template \
        "$SCRIPT_DIR/config/server-runtime.toml.tpl" \
        "$SERVER_RUNTIME_PATH" \
        "__HTTP_BASE__=$HTTP_BASE" \
        "__HTTP_PORT__=$HTTP_PORT" \
        "__ICE_PORT__=$ICE_PORT" \
        "__PACKAGE_PATH__=$SERVICE_PACKAGE" \
        "__REALM__=$REALM" \
        "__REALM_CLIENT_KEY__=$REALM_OWNER_KEY"
    apply_verified_realm_config "$SERVER_RUNTIME_PATH"
}

# Launch actr run fully isolated — independent HOME and --hyper-dir.
# $1=home_dir $2=hyper_dir $3=log_name.
# Sets SERVER_PID and writes to $LOG_DIR/<log_name>.
start_server_isolated() {
    local home_dir="$1"
    local hyper_dir="$2"
    local log_name="${3:-server.log}"
    render_server_runtime
    HOME="$home_dir" "${RUNTIME_LAUNCHER[@]}" python3 - "$SERVICE_KEYCHAIN" "$REALM_OWNER_KEY" <<'PY'
import pathlib, sys
try:
    pathlib.Path(sys.argv[1]).read_bytes()
except OSError:
    pass
else:
    raise SystemExit('Runtime can read the Publisher signing key')
assert pathlib.Path(sys.argv[2]).read_bytes(), 'Runtime cannot read its own Realm key'
PY
    HOME="$home_dir" \
        RUST_LOG="${ACTR_E2E_RUST_LOG:-info}" \
        CARGO_TARGET_DIR="$ACTR_TARGET_DIR" \
        "${RUNTIME_LAUNCHER[@]}" "$ACTR_CLI_BIN" run -c "$SERVER_RUNTIME_PATH" --hyper-dir "$hyper_dir" \
        >"$LOG_DIR/$log_name" 2>&1 &
    SERVER_PID=$!
}

# Poll for successful startup ("ActrNode started").
expect_server_started() {
    local log_name="${1:-server.log}"
    local attempt=0
    while [ $attempt -lt 30 ]; do
        if ! kill -0 "$SERVER_PID" 2>/dev/null; then
            cat "$LOG_DIR/$log_name" >&2 || true
            fail "Server host exited before becoming ready (expected success)"
        fi
        if grep -q "ActrNode started" "$LOG_DIR/$log_name" 2>/dev/null; then
            success "Server host started (Issuer registration succeeded)"
            return 0
        fi
        sleep 1
        attempt=$((attempt + 1))
    done
    fail "Server host did not report ready within 30s"
}

# Poll for expected rejection: server must exit non-zero and log a rejection.
# Rejection can happen at two stages: Issuer registration ("Failed to register with
# Issuer"), or earlier at package attach ("Failed to attach package: untrusted
# publisher") when the Publisher Registry key is revoked and cert_cache cannot
# fetch the pubkey.
expect_server_rejected() {
    local log_name="${1:-server.log}"
    local attempt=0
    while [ $attempt -lt 30 ]; do
        if ! kill -0 "$SERVER_PID" 2>/dev/null; then
            if grep -qE "Failed to register with Issuer|Failed to attach package|untrusted publisher" "$LOG_DIR/$log_name" 2>/dev/null; then
                success "Server host rejected as expected"
                return 0
            fi
            cat "$LOG_DIR/$log_name" >&2 || true
            fail "Server host exited but rejection marker not found"
        fi
        sleep 1
        attempt=$((attempt + 1))
    done
    # Still running after 30s — registration did not fail as expected.
    kill "$SERVER_PID" 2>/dev/null || true
    cat "$LOG_DIR/$log_name" >&2 || true
    fail "Server host was not rejected within 30s (expected rejection)"
}

stop_server() {
    if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    SERVER_PID=""
}

# Assert loran.log shows the package verified with its Publisher identity
# (not as a development identity).
assert_publisher_provenance() {
    if ! grep -q "Package provenance verified.*provenance=Publisher" "$LOG_DIR/loran.log" 2>/dev/null; then
        fail "Publisher provenance marker not found in loran.log"
    fi
    success "Confirmed registration kept the Publisher identity"
}

stop_loran() {
    if [ -n "$LORAN_PID" ] && kill -0 "$LORAN_PID" 2>/dev/null; then
        kill "$LORAN_PID" 2>/dev/null || true
        wait "$LORAN_PID" 2>/dev/null || true
    fi
    LORAN_PID=""
}

# ============================================================================
# Mode flows
# ============================================================================

mode_runtime() {
    assert_no_publication_tables
    local env
    env="$(mk_isolated_env "$MODE")"
    local home_dir hyper_dir
    IFS=$'\t' read -r home_dir hyper_dir <<<"$env"
    start_server_isolated "$home_dir" "$hyper_dir" "server.log"
    expect_server_started "server.log"
    assert_publisher_provenance
    # Dependency installation resolves the service over signaling, so the
    # service must also be online before the client guest lockfile is
    # generated. Reuse this server for the final echo round-trip.
    build_client_guest_package
    run_client_and_assert
    if [ "$REALM_MODE" != legacy ]; then
        stop_server
        start_server_isolated "$home_dir" "$hyper_dir" "server-restarted.log"
        expect_server_started "server-restarted.log"
        run_client_and_assert
    fi
    assert_removed_member_refused
    stop_server
}

mode_rotation() {
    section "🧪 MODE=rotation (retired key passes, revoked key rejects)"
    assert_no_publication_tables

    # The manifest's signing_key_id (key A) — needed to find the history_id after rotation.
    local signing_key_id
    signing_key_id="$(python3 - "$SERVICE_PACKAGE" <<'PY'
import sys, tomllib, zipfile
with zipfile.ZipFile(sys.argv[1]) as zf:
    m = tomllib.loads(zf.read("manifest.toml").decode())
print(m["signing_key_id"])
PY
)"
    [ -n "$signing_key_id" ] || fail "Could not read signing_key_id from manifest"

    # Look up the publisher fixture row.
    local publisher_id
    publisher_id="$(sqlite3 "$LORAN_DB" "SELECT id FROM publisher WHERE name='${PUBLISHER}';")"
    [ -n "$publisher_id" ] || fail "Could not resolve publisher_id for ${PUBLISHER}"

    # --- Phase 1: rotate A→B (A archived as retired); the package signed by A still runs. ---
    section "🔄 Phase 1: rotate key (A → retired), expect success"
    local rotation_keychain="$PUBLISHER_PRIVATE_DIR/rotation-b.keychain.json"
    run_actr pkg keygen --output "$rotation_keychain" --force >/dev/null
    python3 - "$LORAN_DB" "$PUBLISHER" "$rotation_keychain" <<'PY'
import base64
import hashlib
import json
import sqlite3
import sys
import time

db_path, publisher, keychain_path = sys.argv[1:]
with open(keychain_path, encoding="utf-8") as stream:
    public_key = json.load(stream)["public_key"]
key_id = "actr-pub-" + hashlib.sha256(base64.b64decode(public_key, validate=True)).hexdigest()[:16]
now = int(time.time())
connection = sqlite3.connect(db_path, timeout=30)
connection.execute("PRAGMA foreign_keys = ON")
with connection:
    row = connection.execute(
        "SELECT id, key_id, public_key, COALESCE(verified_at, created_at) FROM publisher WHERE name = ?",
        (publisher,),
    ).fetchone()
    if row is None:
        raise SystemExit(f"publisher fixture not found: {publisher}")
    publisher_id, old_key_id, old_public_key, old_created_at = row
    connection.execute(
        """
        INSERT INTO publisher_key_history
            (publisher_id, key_id, public_key, status, created_at, retired_at)
        VALUES (?, ?, ?, 'retired', ?, ?)
        """,
        (publisher_id, old_key_id, old_public_key, old_created_at, now),
    )
    connection.execute(
        """
        UPDATE publisher
        SET public_key = ?, key_id = ?, key_expires_at = ?, updated_at = ?
        WHERE id = ?
        """,
        (public_key, key_id, now + 365 * 86400, now, publisher_id),
    )
connection.close()
PY

    local env1
    env1="$(mk_isolated_env "rot1")"
    local home1 hyper1
    IFS=$'\t' read -r home1 hyper1 <<<"$env1"
    start_server_isolated "$home1" "$hyper1" "server-phase1.log"
    expect_server_started "server-phase1.log"
    assert_publisher_provenance
    stop_server

    # --- Phase 2: revoke key A by history_id (matched by signing_key_id, not array order). ---
    section "🚫 Phase 2: revoke retired key A, expect rejection"
    local revoked_rows
    revoked_rows="$(sqlite3 "$LORAN_DB" \
        "UPDATE publisher_key_history SET status='revoked' WHERE publisher_id=${publisher_id} AND key_id='${signing_key_id}'; SELECT changes();")"
    [ "$revoked_rows" = "1" ] || fail "Could not revoke historical key ${signing_key_id}"

    # KeyRevoked is enforced directly from the Publisher key history.
    assert_no_publication_tables

    local env2
    env2="$(mk_isolated_env "rot2")"
    local home2 hyper2
    IFS=$'\t' read -r home2 hyper2 <<<"$env2"
    start_server_isolated "$home2" "$hyper2" "server-phase2.log"
    expect_server_rejected "server-phase2.log"
    stop_server
}

# ============================================================================
# Main
# ============================================================================

section "🧪 Package Runtime Echo E2E (MODE=${MODE}, REALM_MODE=${REALM_MODE})"
echo "Run directory: $RUN_DIR"
echo "Backend:       $BACKEND"
echo "Message:       $TEST_INPUT"
echo "Loran binary: $LORAN_BIN"

python3 "$SCRIPT_DIR/test_runtime_isolation.py"
render_runtime_configs
build_local_actr_cli
build_local_loran
if [ "$REALM_MODE" != legacy ]; then prepare_realm_scenario; fi
# Modes that assert the provenance need the debug-level "Package provenance
# verified" marker in loran.log.
export LORAN_RUST_LOG="info,loran::observability=debug"
start_loran
login_admin
wait_for_signer
if [ "$REALM_MODE" = legacy ]; then ensure_realm; fi
provision_publisher_keychain
scaffold_service_guest
build_service_package
if [ "$REALM_MODE" != legacy ]; then create_profile_realm; fi
if [ "$REALM_STAGE" = profile ]; then
    # An explicit transport diagnostic, not a successful runtime Echo. Keep
    # the default/full CI path unchanged and require retirement verification.
    retire_profile_realm
    assert_no_publication_tables
    success "Realm Profile/package lifecycle (REALM_MODE=${REALM_MODE}, REALM_HTTPS=${REALM_HTTPS:-0}) passed; runtime Echo was not run"
    exit 0
fi

case "$MODE" in
    runtime) mode_runtime ;;
    rotation)    mode_rotation ;;
esac

if [ "$REALM_MODE" != legacy ]; then retire_profile_realm; fi
assert_no_publication_tables

echo ""
success "Package runtime echo E2E (MODE=${MODE}, REALM_MODE=${REALM_MODE}) completed successfully"
