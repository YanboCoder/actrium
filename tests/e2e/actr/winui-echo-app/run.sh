#!/usr/bin/env bash

# WinUI EchoApp E2E — Windows mirror of tests/e2e/actr/swift-echo-app/run.sh.
#
# Flow: build loran + actr CLI + EchoService (Rust) + the cabi.EchoApp
# workload dll + the WinUI app; start loran; provision realm/publisher keychain;
# start the EchoService; render the app's actr.toml; launch the WinUI app
# with ACTR_ECHOAPP_AUTO_SEND=1; wait for the ACTR_E2E_RESULT marker on
# stderr and verify the echo reply. The SSH service runs in Session 0, where
# WinUI cannot initialize, so the app itself is started through an interactive
# Task Scheduler task in the logged-in desktop session.
#
# Reuses tests/e2e/actr/package-runtime-echo/lib/common.sh for the loran / realm /
# keychain / service-package lifecycle. Windows-specific: port cleanup
# (netstat+taskkill instead of lsof), dotnet build for the app, no simulator.
#
# Run on a Windows runner (windows-latest / windows-arm64-larger) with dotnet
# and the Windows App SDK workload installed.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"

source "$SCRIPT_DIR/../package-runtime-echo/lib/common.sh"
source "$SCRIPT_DIR/../swift-echo-app/lib/readiness.sh"

HTTP_PORT=8081
ICE_PORT=3478
REALM=""
ADMIN_PASSWORD="e2e-test-password"
PUBLISHER="actrium"
LORAN_BIN="${LORAN_BIN:-}"
ACTR_CLI_MANIFEST="$REPO_ROOT/crates/actr-cli/Cargo.toml"
LORAN_CONFIG_TEMPLATE="$SCRIPT_DIR/loran.toml"
E2E_TARGET_ROOT="$REPO_ROOT/target/e2e-cache/winui-echo-app"
ACTR_TARGET_DIR="$E2E_TARGET_ROOT/actr-cli"
TEMP_SERVICE_TARGET_DIR="$E2E_TARGET_ROOT/temp-service"
DEFAULT_MESSAGE="e2e-test-message"
REALM_NAME_PREFIX="winui-echo-app"
TEST_INPUT="$DEFAULT_MESSAGE"
ICE_HOST="${LORAN_ICE_HOST:-}"

# The cabi workload crate is a standalone workspace; build-actr-cabi-dll.sh
# expects the cdylib under ${WORKSPACE_ROOT}/target, so force that target dir
# for the dll build (actr CLI / loran builds below override it inline).
export CARGO_TARGET_DIR="$REPO_ROOT/target"

while [[ $# -gt 0 ]]; do
    case "$1" in
        -*) fail "Unknown option: $1" ;;
        *) TEST_INPUT="$1"; shift ;;
    esac
done

for cmd in cargo curl jq sqlite3 python3 rustc dotnet powershell.exe schtasks.exe; do
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
SERVICE_KEYCHAIN="$TMP_SERVICE_DIR/packaging/keys/publisher.keychain.json"
SERVICE_PUBLIC_KEY="$TMP_SERVICE_DIR/public-key.json"
PROVISIONED_KEYCHAIN="$RUN_DIR/publisher.keychain.json"
PROVISIONED_PUBLIC_KEY="$RUN_DIR/publisher-public-key.json"
LORAN_CONFIG_PATH="$RUN_DIR/loran.toml"
SERVER_RUNTIME_PATH="$RUN_DIR/server-runtime.toml"
ECHOAPP_LORAN_CONFIG="$RUN_DIR/echoapp-actr.toml"
HOST_TARGET="$(rustc -vV | awk '/host:/ {print $2}')"
APP_STDOUT_LOG="$LOG_DIR/app.stdout.log"
APP_STDERR_LOG="$LOG_DIR/app.stderr.log"
APP_RUNTIME_LOG="$LOG_DIR/app.runtime.log"
APP_PROCESS_NAME="WinUiEchoApp"
CABI_DLL=""
APP_EXE=""
APP_TASK_NAME=""
APP_TASK_LAUNCHER=""

mkdir -p "$SQLITE_DIR" "$LOG_DIR" "$DIST_DIR" "$TMP_SERVICE_ROOT" "$E2E_TARGET_ROOT"

LORAN_PID=""
SERVER_PID=""
ACTR_CLI_BIN=""
ADMIN_TOKEN=""
SERVICE_PACKAGE=""
SERVICE_VERSION=""
REALM_OWNER_KEY=""
REALM_CLIENT_KEY=""

# ──── Windows port cleanup (override common.sh's lsof-based kill_listener) ────
kill_listener() {
    local protocol="$1" port="$2"
    [ "$protocol" = "tcp" ] || return 0
    local pids
    pids="$(netstat -ano 2>/dev/null \
            | awk -v p=":$port" '$2 ~ p && $4 == "LISTENING" {print $5}' \
            | sort -u)"
    for pid in $pids; do
        [ -n "$pid" ] && MSYS_NO_PATHCONV=1 taskkill.exe /F /PID "$pid" 2>/dev/null || true
    done
}

# On Windows, a native curl process can remain blocked while the local HTTP
# listener is being created. Bound each probe so the outer readiness timeout
# remains effective.
wait_for_http_ok() {
    local url="$1" timeout="$2"
    local started_at now
    started_at="$(date +%s)"

    while true; do
        if curl -fsS --connect-timeout 2 --max-time 5 "$url" >/dev/null 2>&1; then
            return 0
        fi

        now="$(date +%s)"
        if [ $((now - started_at)) -ge "$timeout" ]; then
            return 1
        fi
        sleep 1
    done
}

run_actr() {
    CARGO_TARGET_DIR="$ACTR_TARGET_DIR" "$ACTR_CLI_BIN" "$@"
}

render_runtime_configs() {
    [ -n "${LORAN_CONFIG_TEMPLATE:-}" ] || fail "LORAN_CONFIG_TEMPLATE is not set"
    if [ -z "$ICE_HOST" ]; then
        ICE_HOST="$(detect_ice_host)"
    fi

    # Loran parses this value as a native Windows path. Keep SQLITE_DIR in
    # POSIX form for shell checks, but render a drive-qualified path in TOML.
    local loran_sqlite_dir
    loran_sqlite_dir="$(cargo_path "$SQLITE_DIR")"
    render_template \
        "$LORAN_CONFIG_TEMPLATE" \
        "$LORAN_CONFIG_PATH" \
        "__SQLITE_DIR__=$loran_sqlite_dir" \
        "__HTTP_PORT__=$HTTP_PORT" \
        "__ICE_PORT__=$ICE_PORT" \
        "__ICE_HOST__=$ICE_HOST"
}

windows_path() {
    if command -v cygpath >/dev/null 2>&1; then
        cygpath -aw "$1"
    elif command -v wslpath >/dev/null 2>&1; then
        wslpath -aw "$1"
    else
        fail "A Windows path converter (cygpath or wslpath) is required"
    fi
}

cargo_path() {
    if command -v cygpath >/dev/null 2>&1; then
        cygpath -am "$1"
    elif command -v wslpath >/dev/null 2>&1; then
        wslpath -am "$1"
    else
        fail "A Windows path converter (cygpath or wslpath) is required"
    fi
}

detect_ice_host() {
    local ice_host
    ice_host="$(powershell.exe -NoProfile -Command '$route = Get-NetRoute -DestinationPrefix "0.0.0.0/0" | Sort-Object RouteMetric, InterfaceMetric | Select-Object -First 1; if ($null -ne $route) { Get-NetIPAddress -InterfaceIndex $route.InterfaceIndex -AddressFamily IPv4 | Where-Object { $_.IPAddress -notlike "169.254.*" } | Select-Object -First 1 -ExpandProperty IPAddress }' | tr -d '\r')"
    [ -n "$ice_host" ] || fail "Unable to detect the Windows default-route IPv4 address; set LORAN_ICE_HOST"
    printf '%s\n' "$ice_host"
}

grep_app_logs() { grep -h "$@" "$APP_STDOUT_LOG" "$APP_STDERR_LOG" 2>/dev/null; }
tail_app_logs() {
    echo "App stdout tail:"
    tail -n "$1" "$APP_STDOUT_LOG" >&2 2>/dev/null || true
    echo "App stderr tail:"
    tail -n "$1" "$APP_STDERR_LOG" >&2 2>/dev/null || true
    echo "App runtime tail:"
    tail -n "$1" "$APP_RUNTIME_LOG" >&2 2>/dev/null || true
}

cleanup() {
    local status=$?
    if [ -n "${SERVER_PID:-}" ]; then
        kill "$SERVER_PID" 2>/dev/null || true
        # Git Bash can leave the native child process alive after SIGTERM.
        # The Bash job PID may not be the native child process, so also find
        # the uniquely scoped `actr run` command by this E2E run ID.
        MSYS_NO_PATHCONV=1 taskkill.exe /F /T /PID "$SERVER_PID" >/dev/null 2>&1 || true
        MSYS_NO_PATHCONV=1 powershell.exe -NoProfile -Command \
            "\$runId = '$RUN_ID'; Get-CimInstance Win32_Process | Where-Object { \
                \$_.Name -eq 'actr.exe' -and \
                \$_.CommandLine -like ('*' + \$runId + '*') \
            } | ForEach-Object { Stop-Process -Id \$_.ProcessId -Force }" \
            >/dev/null 2>&1 || true
    fi
    if [ -n "${LORAN_PID:-}" ] && kill -0 "$LORAN_PID" 2>/dev/null; then
        kill "$LORAN_PID" 2>/dev/null || true
    fi
    if [ -n "${APP_TASK_NAME:-}" ]; then
        MSYS_NO_PATHCONV=1 schtasks.exe /End /TN "$APP_TASK_NAME" >/dev/null 2>&1 || true
        MSYS_NO_PATHCONV=1 schtasks.exe /Delete /TN "$APP_TASK_NAME" /F >/dev/null 2>&1 || true
    fi
    MSYS_NO_PATHCONV=1 taskkill.exe /F /IM "${APP_PROCESS_NAME}.exe" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -f "$ECHOAPP_LORAN_CONFIG"
    if [ $status -eq 0 ] && [ "${KEEP_TMP:-0}" != "1" ]; then
        rm -rf "$RUN_DIR"
    else
        echo "Artifacts preserved at: $RUN_DIR"
    fi
    exit $status
}
trap cleanup EXIT INT TERM

# ──── Rust / loran / actr-CLI build (reused from swift-echo-app) ────

build_local_actr_cli() {
    section "🔧 Building local actr CLI"
    CARGO_TARGET_DIR="$ACTR_TARGET_DIR" cargo build --manifest-path "$ACTR_CLI_MANIFEST" --bin actr >/dev/null
    ACTR_CLI_BIN="$ACTR_TARGET_DIR/debug/actr"
    [ -x "$ACTR_CLI_BIN" ] || fail "actr CLI binary missing at $ACTR_CLI_BIN"
    success "actr CLI ready: $ACTR_CLI_BIN"
}

build_local_loran() {
    if [ -n "${LORAN_BIN:-}" ]; then
        local resolved
        resolved="$(command -v "$LORAN_BIN" 2>/dev/null || true)"
        [ -n "$resolved" ] || [ -x "$LORAN_BIN" ] || fail "LORAN_BIN set but not executable: $LORAN_BIN"
        LORAN_BIN="${resolved:-$LORAN_BIN}"
        export LORAN_BIN
        success "Using caller-provided loran: $LORAN_BIN"
        return 0
    fi
    section "🔨 Building local loran from source"
    CARGO_TARGET_DIR="$ACTR_TARGET_DIR" cargo build \
        --manifest-path "$REPO_ROOT/crates/loran-server/Cargo.toml" \
        --bin loran >/dev/null
    LORAN_BIN="$ACTR_TARGET_DIR/debug/loran"
    [ -x "$LORAN_BIN" ] || fail "loran binary missing at $LORAN_BIN"
    export LORAN_BIN
    success "loran ready: $LORAN_BIN"
}

# ──── cabi.EchoApp workload dll ────

build_workload_dll() {
    section "🔨 Building actr_cabi.dll (cabi.EchoApp workload)"
    # build-actr-cabi-dll.sh builds the workload cdylib and copies it to
    # dist/actr_cabi.dll. CARGO_TARGET_DIR is already exported to
    # $REPO_ROOT/target so the script's lib_dir lookup matches.
    ACTR_CABI_WORKLOAD_CRATE="$SCRIPT_DIR/workload" \
    ACTR_CABI_TARGETS="x86_64-pc-windows-msvc" \
    ACTR_BUILD_PROFILE="release" \
        bash "$REPO_ROOT/packages/actr-cabi/csharp/build-actr-cabi-dll.sh"
    CABI_DLL="$REPO_ROOT/packages/actr-cabi/csharp/dist/actr_cabi.dll"
    [ -f "$CABI_DLL" ] || fail "actr_cabi.dll not built at $CABI_DLL"
    success "actr_cabi.dll ready: $CABI_DLL"
}

# ──── WinUI app build ────

build_winui_app() {
    section "🏗️  Building WinUI EchoApp"
    local csproj="$SCRIPT_DIR/WinUiEchoApp/WinUiEchoApp.csproj"
    local actrsharp_csproj="$REPO_ROOT/packages/actr-cabi/csharp/ActrSharp/ActrSharp.csproj"
    local actr_version actrsharp_framework actrsharp_dll managed_actrsharp_dll
    actr_version="$(awk '
        /^\[workspace\.package\]$/ { in_workspace_package = 1; next }
        /^\[/ { in_workspace_package = 0 }
        in_workspace_package && $1 == "version" { gsub(/"/, "", $3); print $3; exit }
    ' "$REPO_ROOT/Cargo.toml")"
    [ -n "$actr_version" ] || fail "Unable to read workspace package version"
    actrsharp_framework="$(sed -n 's@.*<TargetFramework>\(.*\)</TargetFramework>.*@\1@p' "$actrsharp_csproj" | head -n1)"
    [ -n "$actrsharp_framework" ] || fail "Unable to read ActrSharp target framework"
    actrsharp_dll="$(dirname "$actrsharp_csproj")/bin/Debug/$actrsharp_framework/ActrSharp.dll"
    managed_actrsharp_dll="$RUN_DIR/ActrSharp.dll"
    # Place the dll next to the csproj so CopyToOutputDirectory ships it.
    cp "$CABI_DLL" "$SCRIPT_DIR/WinUiEchoApp/actr_cabi.dll"
    # Build the reference directly and also pass the version through the app's
    # ProjectReference, so both managed build paths match the native C ABI.
    dotnet build "$actrsharp_csproj" -t:Rebuild -c Debug -p:Version="$actr_version" 2>&1 \
        | tee -a "$LOG_DIR/dotnet-build.log"
    [ -f "$actrsharp_dll" ] || fail "ActrSharp.dll not found at $actrsharp_dll"
    cp "$actrsharp_dll" "$managed_actrsharp_dll"
    dotnet build "$csproj" -c Debug -r win-x64 -p:ActrCabiVersion="$actr_version" 2>&1 \
        | tee -a "$LOG_DIR/dotnet-build.log"
    APP_EXE="$SCRIPT_DIR/WinUiEchoApp/bin/Debug/net8.0-windows10.0.19041.0/win-x64/${APP_PROCESS_NAME}.exe"
    [ -f "$APP_EXE" ] || fail "WinUiEchoApp.exe not found at $APP_EXE"
    # The WinUI build can invoke a separate ProjectReference path. Restore the
    # version-locked assembly compiled above into the app's runtime directory.
    cp "$managed_actrsharp_dll" "$(dirname "$APP_EXE")/ActrSharp.dll"
    success "WinUI EchoApp built: $APP_EXE"
}

# ──── Service guest scaffolding (reused from swift-echo-app) ────

append_workspace_patch() {
    local cargo_toml="$1"
    local repo_path
    repo_path="$(cargo_path "$REPO_ROOT")"
    grep -q '^\[workspace\]' "$cargo_toml" || cat >>"$cargo_toml" <<'EOF'

[workspace]
EOF
    grep -q '^\[patch\.crates-io\]' "$cargo_toml" && return 0
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

pin_workspace_actr_dependencies() {
    local cargo_toml="$1"
    local repo_path
    repo_path="$(cargo_path "$REPO_ROOT")"
    ACTR_FRAMEWORK_PATH="$repo_path/crates/actr-framework" \
    ACTR_PROTOCOL_PATH="$repo_path/crates/actr-protocol" \
        perl -i -pe '
            if (/^actr-framework = /) { $_ = qq{actr-framework = { path = "$ENV{ACTR_FRAMEWORK_PATH}" }\n}; }
            elsif (/^actr-protocol = /) { $_ = qq{actr-protocol = { path = "$ENV{ACTR_PROTOCOL_PATH}" }\n}; }
        ' "$cargo_toml"

    grep -Fq "actr-framework = { path = \"$repo_path/crates/actr-framework\" }" "$cargo_toml" ||
        fail "Failed to pin actr-framework to the workspace"
    grep -Fq "actr-protocol = { path = \"$repo_path/crates/actr-protocol\" }" "$cargo_toml" ||
        fail "Failed to pin actr-protocol to the workspace"
}

write_project_keychain_config() {
    local project_dir="$1" keychain_path="$2"
    mkdir -p "$project_dir/.actr"
    cat >"$project_dir/.actr/config.toml" <<EOF
[publisher]
name = "$PUBLISHER"
keychain = "$keychain_path"

[network]
signaling_url = "ws://127.0.0.1:${HTTP_PORT}/signaling/ws"
issuer_endpoint = "http://127.0.0.1:${HTTP_PORT}/issuer"
realm = "$REALM"
realm_client_key = "$REALM_CLIENT_KEY"
EOF
}

provision_publisher_keychain() {
    section "🏷️  Provisioning publisher keychain via Admin API"
    local apply_file="$RUN_DIR/publisher-apply.json"
    local approve_file="$RUN_DIR/publisher-approve.json"
    local now
    now="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

    curl -fsS \
        -X POST \
        "http://127.0.0.1:${HTTP_PORT}/admin/api/publisher-registry/apply" \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H 'Content-Type: application/json' \
        -d "{\"github_login\":\"${PUBLISHER}\",\"contact\":\"e2e@local.actr\"}" \
        >"$apply_file"

    local publisher_id
    publisher_id="$(json_field "$apply_file" '.publisher_id')"

    curl -fsS \
        -X POST \
        "http://127.0.0.1:${HTTP_PORT}/admin/api/publisher-registry/admin/${publisher_id}/approve" \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H 'Content-Type: application/json' \
        -d '{}' \
        >"$approve_file"

    mkdir -p "$(dirname "$PROVISIONED_KEYCHAIN")"
    # A Base64 key can start with `/`. Under Git Bash, prevent MSYS from
    # rewriting that value as a Windows path before jq receives it.
    MSYS_NO_PATHCONV=1 jq -n \
        --arg private_key "$(json_field "$approve_file" '.private_key')" \
        --arg public_key "$(json_field "$approve_file" '.certificate.publisher_pubkey')" \
        --arg created_at "$now" \
        '{
          created_at: $created_at,
          note: "E2E publisher signing key issued by local loran admin API",
          private_key: $private_key,
          public_key: $public_key
        }' \
        >"$PROVISIONED_KEYCHAIN"
    chmod 600 "$PROVISIONED_KEYCHAIN" 2>/dev/null || true

    MSYS_NO_PATHCONV=1 jq -n \
        --arg public_key "$(json_field "$approve_file" '.certificate.publisher_pubkey')" \
        '{ public_key: $public_key }' \
        >"$PROVISIONED_PUBLIC_KEY"

    success "Generated publisher keychain for ${PUBLISHER}"
}

scaffold_service_guest() {
    section "🧱 Scaffolding temporary echo service"
    run_actr init -l rust --template echo --role service \
        --signaling "ws://127.0.0.1:${HTTP_PORT}/signaling/ws" \
        --publisher "$PUBLISHER" --realm "$REALM" "$TMP_SERVICE_DIR"
    pin_workspace_actr_dependencies "$TMP_SERVICE_DIR/Cargo.toml"
    append_workspace_patch "$TMP_SERVICE_DIR/Cargo.toml"
    python3 "$REPO_ROOT/scripts/actr/apply-upstream-patches.py" "$TMP_SERVICE_DIR/Cargo.toml"
    mkdir -p "$(dirname "$SERVICE_KEYCHAIN")"
    cp "$PROVISIONED_KEYCHAIN" "$SERVICE_KEYCHAIN"
    cp "$PROVISIONED_PUBLIC_KEY" "$SERVICE_PUBLIC_KEY"
    write_project_keychain_config "$TMP_SERVICE_DIR" "$SERVICE_KEYCHAIN"
    (
        cd "$TMP_SERVICE_DIR"
        CARGO_TARGET_DIR="$TEMP_SERVICE_TARGET_DIR" run_actr deps install
        CARGO_TARGET_DIR="$TEMP_SERVICE_TARGET_DIR" run_actr gen -l rust
    )
    SERVICE_VERSION="$(awk '
        /^\[package\]/ { in_package = 1; next }
        /^\[/ && in_package { exit }
        in_package && $1 == "version" { gsub(/"/, "", $3); print $3; exit }
    ' "$TMP_SERVICE_DIR/manifest.toml")"
    [ -n "$SERVICE_VERSION" ] || fail "Unable to detect temporary service version"
    success "Temporary echo service ready: version ${SERVICE_VERSION}"
}

build_service_package() {
    section "📦 Building and verifying the server package"
    SERVICE_PACKAGE="$DIST_DIR/${PUBLISHER}-EchoService-${SERVICE_VERSION}-${HOST_TARGET}.actr"
    (
        cd "$TMP_SERVICE_DIR"
        CARGO_TARGET_DIR="$TEMP_SERVICE_TARGET_DIR" run_actr build \
            --manifest-path manifest.toml --key "$SERVICE_KEYCHAIN" --output "$SERVICE_PACKAGE"
    )
    [ -f "$SERVICE_PACKAGE" ] || fail "Server package missing: $SERVICE_PACKAGE"
    run_actr pkg verify --pubkey "$SERVICE_PUBLIC_KEY" --package "$SERVICE_PACKAGE" >/dev/null
    success "Server package built and verified"
}


render_echoapp_config() {
    section "📝 Rendering EchoApp runtime config"
    render_template "$SCRIPT_DIR/actr.toml.tpl" "$ECHOAPP_LORAN_CONFIG" \
        "__HOST__=127.0.0.1" \
        "__ICE_HOST__=$ICE_HOST" \
        "__HTTP_PORT__=$HTTP_PORT" \
        "__ICE_PORT__=$ICE_PORT" \
        "__REALM__=$REALM" \
        "__REALM_CLIENT_KEY__=$REALM_CLIENT_KEY"
    success "EchoApp actr.toml rendered"
}

run_server_host() {
    section "🚀 Starting package-backed server host"
    local service_package_path
    service_package_path="$(cargo_path "$SERVICE_PACKAGE")"
    cat >"$SERVER_RUNTIME_PATH" <<EOF
edition = 1

[package]
path = "${service_package_path}"

[signaling]
url = "ws://127.0.0.1:${HTTP_PORT}/signaling/ws"

[issuer_endpoint]
url = "http://127.0.0.1:${HTTP_PORT}/issuer"

[deployment]
realm = "${REALM}"
realm_client_key = "${REALM_OWNER_KEY}"

[[trust]]
kind = "registry"
endpoint = "http://127.0.0.1:${HTTP_PORT}/issuer"

[discovery]
visible = true

[observability]
filter_level = "info"
tracing_enabled = false
tracing_endpoint = "http://localhost:4317"
tracing_service_name = "winui-echo-app-server"

[webrtc]
# Mirror the WinUI client: use the local TURN relay to avoid cross-session
# Windows host-candidate routing through virtual network adapters.
force_relay = true
stun_urls = ["stun:${ICE_HOST}:${ICE_PORT}"]
turn_urls = ["turn:${ICE_HOST}:${ICE_PORT}"]

[acl]

[[acl.rules]]
permission = "allow"
type = "${PUBLISHER}/EchoApp@0.1.0"
EOF
    RUST_LOG="${RUST_LOG:-info}" run_actr run -c "$SERVER_RUNTIME_PATH" >"$LOG_DIR/server.log" 2>&1 &
    SERVER_PID=$!
    local attempt=0
    while [ $attempt -lt 30 ]; do
        if ! kill -0 "$SERVER_PID" 2>/dev/null; then
            cat "$LOG_DIR/server.log" >&2 || true
            fail "Server host exited early"
        fi
        if grep -q "Echo Host fully started\|ActrNode started" "$LOG_DIR/server.log" 2>/dev/null; then
            success "Server host is running"
            return 0
        fi
        sleep 1
        attempt=$((attempt + 1))
    done
    warn "Server host readiness log not observed, continuing"
}

check_service_ready() {
    section "🔍 Verifying EchoService readiness"
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        cat "$LOG_DIR/server.log" >&2 || true
        fail "EchoService process died before app launch"
    fi
    if ! curl -fsS "http://127.0.0.1:${HTTP_PORT}/signaling/health" >/dev/null 2>&1; then
        fail "Signaling health check failed before app launch"
    fi
    if ! wait_for_service_registration "$SQLITE_DIR/signaling_cache.db" \
            "$REALM" "$PUBLISHER" "EchoService" \
            "${SERVICE_READY_TIMEOUT_SECONDS:-60}"; then
        tail -n 120 "$LOG_DIR/server.log" >&2 2>/dev/null || true
        fail "EchoService did not register with signaling within the timeout"
    fi
    success "EchoService readiness check complete"
}

write_app_task_launcher() {
    APP_TASK_LAUNCHER="$RUN_DIR/winui-e2e-launch.cmd"

    local app_exe_win config_win stdout_win stderr_win runtime_log_win
    app_exe_win="$(windows_path "$APP_EXE")"
    config_win="$(windows_path "$ECHOAPP_LORAN_CONFIG")"
    stdout_win="$(windows_path "$APP_STDOUT_LOG")"
    stderr_win="$(windows_path "$APP_STDERR_LOG")"
    runtime_log_win="$(windows_path "$APP_RUNTIME_LOG")"

    # The test input is written into a cmd file. Reject syntax that would
    # change the command file rather than silently executing a different app.
    case "$TEST_INPUT" in
        *'"'*|*'%'*) fail "TEST_INPUT cannot contain double quotes or percent signs on Windows" ;;
    esac

    printf '%s\r\n' \
        '@echo off' \
        'set "ACTR_ECHOAPP_AUTO_SEND=1"' \
        "set \"ACTR_ECHOAPP_TEST_INPUT=$TEST_INPUT\"" \
        "set \"ACTR_ECHOAPP_ACTR_CONFIG=$config_win\"" \
        "set \"ACTR_ECHOAPP_LOG_PATH=$runtime_log_win\"" \
        "\"$app_exe_win\" >\"$stdout_win\" 2>\"$stderr_win\"" \
        >"$APP_TASK_LAUNCHER"
}

launch_app() {
    section "📲 Launching WinUI EchoApp"
    write_app_task_launcher

    local interactive_user task_launcher_win task_command
    interactive_user="$(powershell.exe -NoProfile -Command '(Get-CimInstance Win32_ComputerSystem).UserName' | tr -d '\r')"
    [ -n "$interactive_user" ] || fail "No interactive Windows user is available for the WinUI app"

    task_launcher_win="$(windows_path "$APP_TASK_LAUNCHER")"
    task_command="C:\\Windows\\System32\\cmd.exe /d /c call \"$task_launcher_win\""
    APP_TASK_NAME="ActrWinUiE2E-${RUN_ID//[^[:alnum:]]/-}"

    MSYS_NO_PATHCONV=1 schtasks.exe /Create \
        /TN "$APP_TASK_NAME" \
        /TR "$task_command" \
        /SC ONCE /ST 23:59 \
        /RU "$interactive_user" /IT /F >/dev/null
    MSYS_NO_PATHCONV=1 schtasks.exe /Run /TN "$APP_TASK_NAME"
    success "App launched in the interactive desktop session, waiting for echo result"
}

wait_for_echo_result() {
    section "⏳ Waiting for echo result"
    local timeout="${CLIENT_TIMEOUT_SECONDS:-120}"
    local elapsed=0
    while [ $elapsed -lt "$timeout" ]; do
        if grep_app_logs -q "ACTR_E2E_RESULT:"; then
            local result
            result="$(grep_app_logs "ACTR_E2E_RESULT:" | tail -1)"
            echo "Echo result: $result"
            if echo "$result" | grep -q "$TEST_INPUT"; then
                success "End-to-end echo succeeded"
                return 0
            fi
            warn "Echo result does not contain expected message: $TEST_INPUT"
            return 1
        fi
        sleep 2
        elapsed=$((elapsed + 2))
    done
    tail_app_logs 80
    fail "Timed out waiting for echo result after ${timeout}s"
}

# ──── Main ────

section "🧪 WinUI EchoApp E2E"
echo "Run directory: $RUN_DIR"
echo "Message:       $TEST_INPUT"
echo "Loran binary: ${LORAN_BIN:-workspace source (built below)}"
echo "Host target:   $HOST_TARGET"

render_runtime_configs
build_local_actr_cli
build_local_loran
build_workload_dll
build_winui_app
start_loran
login_admin
wait_for_signer
ensure_realm
provision_publisher_keychain

scaffold_service_guest
build_service_package

render_echoapp_config
run_server_host
check_service_ready

launch_app
wait_for_echo_result

echo ""
success "WinUI EchoApp e2e completed successfully"
