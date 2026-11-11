#!/usr/bin/env bash
# Verify detached runtime lifecycle flows against the package-echo server config.
#
# Validates:
#   - `actr run -d` prints WID and follow hint
#   - `actr ps` shows WID / ACTR_ID / PID / STATUS / STARTED_AT
#   - `actr stop <wid-prefix>` then `actr start <wid-prefix>` keeps WID and changes PID
#   - `actr restart <wid-prefix>` keeps WID and log file
#   - `actr logs <wid-prefix> -f` keeps streaming across restarts
#   - v1 runtime record schema in run_dir returns an actionable error with the directory path

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ACTR_REPO_DIR="$(cd "$WORKSPACE_ROOT/../../.." && pwd)"
LORAN_DIR="$ACTR_REPO_DIR"
ECHO_ACTR_DIR="$WORKSPACE_ROOT/echo-actr"
LORAN_CONFIG="$WORKSPACE_ROOT/loran-config.toml"
ACTR_CLI_MANIFEST="$ACTR_REPO_DIR/crates/actr-cli/Cargo.toml"
SERVER_TEMPLATE_CONFIG="$SCRIPT_DIR/server-actr.toml"
ARTIFACT_DIR="$SCRIPT_DIR/logs/manual-runtime-lifecycle"
COMMAND_DIR="$ARTIFACT_DIR/commands"
FOLLOW_CAPTURE="$ARTIFACT_DIR/logs-follow.txt"
TEST_CONFIG="$ARTIFACT_DIR/server-actr.lifecycle.toml"
TEST_HYPER_DIR="$ARTIFACT_DIR/hyper"
LORAN_LOG="$ARTIFACT_DIR/loran.log"
HOST_TARGET="$(rustc -vV | awk '/host:/ {print $2}')"

LORAN_CMD=""
LORAN_PID=""
LOGS_FOLLOW_PID=""
RUNTIME_PID=""
ACTR_PACKAGE=""
SIGNING_KEY=""
PUBLIC_KEY_PATH=""
ACTR_CLI_BIN=""
LORAN_DB=""
PACKAGE_BUILD_CONFIG=""

section() {
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "$1"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
}

fail() {
    echo -e "${RED}❌ $1${NC}" >&2
    exit 1
}

assert_contains() {
    local haystack="$1"
    local needle="$2"
    local message="$3"
    [[ -n "$needle" ]] || fail "$message (empty needle)"
    if [[ "$haystack" != *"$needle"* ]]; then
        fail "$message (missing: $needle)"
    fi
}

assert_equals() {
    local actual="$1"
    local expected="$2"
    local message="$3"
    if [[ "$actual" != "$expected" ]]; then
        fail "$message (expected: $expected, actual: $actual)"
    fi
}

assert_not_equals() {
    local left="$1"
    local right="$2"
    local message="$3"
    if [[ "$left" == "$right" ]]; then
        fail "$message (both: $left)"
    fi
}

require_cmd() {
    local cmd="$1"
    command -v "$cmd" >/dev/null 2>&1 || fail "Required command not found: $cmd"
}

detect_loran_db() {
    if [[ -n "$LORAN_PID" ]]; then
        local live_db
        live_db="$(lsof -p "$LORAN_PID" 2>/dev/null | awk '/loran\.db$/ {print $NF; exit}')"
        if [[ -n "$live_db" && -f "$live_db" ]]; then
            printf '%s\n' "$live_db"
            return 0
        fi
    fi

    local candidate
    for candidate in \
        "$WORKSPACE_ROOT/database/loran.db" \
        "$ACTR_REPO_DIR/database/loran.db" \
        "$ACTRIUM_DIR/loran/database/loran.db"; do
        if [[ -f "$candidate" ]]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done

    return 1
}

record_path() {
    python3 - <<'PY' "$TEST_HYPER_DIR"
import pathlib
import sys

run_dir = pathlib.Path(sys.argv[1]) / "run"
records = sorted(run_dir.glob("*.json"))
if len(records) == 1:
    print(records[0])
PY
}

record_field() {
    local field="$1"
    python3 - <<'PY' "$TEST_HYPER_DIR" "$field"
import json
import pathlib
import sys

hyper_dir = pathlib.Path(sys.argv[1])
field = sys.argv[2]
records = sorted((hyper_dir / "run").glob("*.json"))
if len(records) != 1:
    raise SystemExit(f"expected exactly 1 runtime record, found {len(records)}")
data = json.loads(records[0].read_text())
value = data.get(field)
print("" if value is None else value)
PY
}

wait_for_record() {
    local retries="${1:-30}"
    local count=0
    while [[ $count -lt $retries ]]; do
        if [[ -n "$(record_path)" ]]; then
            return 0
        fi
        sleep 1
        count=$((count + 1))
    done
    fail "Timed out waiting for runtime record in $TEST_HYPER_DIR/run"
}

wait_for_pid_change() {
    local previous_pid="$1"
    local retries="${2:-30}"
    local count=0
    while [[ $count -lt $retries ]]; do
        if [[ -n "$(record_path)" ]]; then
            local current_pid
            current_pid="$(record_field pid)"
            if [[ -n "$current_pid" && "$current_pid" != "$previous_pid" ]]; then
                return 0
            fi
        fi
        sleep 1
        count=$((count + 1))
    done
    fail "Timed out waiting for PID to change from $previous_pid"
}

wait_for_status() {
    local expected_status="$1"
    local retries="${2:-30}"
    local count=0
    while [[ $count -lt $retries ]]; do
        local output
        output="$("$ACTR_CLI_BIN" ps "${ACTR_RUNTIME_ARGS[@]}" --all --log 2>&1 || true)"
        if [[ "$output" == *"$expected_status"* ]]; then
            printf '%s\n' "$output" > "$COMMAND_DIR/ps-status-${expected_status}.txt"
            return 0
        fi
        sleep 1
        count=$((count + 1))
    done
    fail "Timed out waiting for runtime status '$expected_status'"
}

stop_logs_follow() {
    if [[ -n "$LOGS_FOLLOW_PID" ]] && kill -0 "$LOGS_FOLLOW_PID" 2>/dev/null; then
        kill "$LOGS_FOLLOW_PID" 2>/dev/null || true
        wait "$LOGS_FOLLOW_PID" 2>/dev/null || true
    fi
    LOGS_FOLLOW_PID=""
}

cleanup() {
    stop_logs_follow

    if [[ -n "$PACKAGE_BUILD_CONFIG" && -f "$PACKAGE_BUILD_CONFIG" ]]; then
        rm -f "$PACKAGE_BUILD_CONFIG"
    fi

    if [[ -n "$RUNTIME_PID" ]] && kill -0 "$RUNTIME_PID" 2>/dev/null; then
        kill "$RUNTIME_PID" 2>/dev/null || true
        wait "$RUNTIME_PID" 2>/dev/null || true
    fi

    if [[ -n "$LORAN_PID" ]] && kill -0 "$LORAN_PID" 2>/dev/null; then
        kill "$LORAN_PID" 2>/dev/null || true
        wait "$LORAN_PID" 2>/dev/null || true
    fi
}

trap cleanup EXIT INT TERM

section "🔎 Preflight"

mkdir -p "$COMMAND_DIR"
: > "$FOLLOW_CAPTURE"

require_cmd python3
require_cmd sqlite3
require_cmd curl
require_cmd lsof
require_cmd openssl
source "$WORKSPACE_ROOT/scripts/ensure-config-toml.sh"
ensure_loran_config "$WORKSPACE_ROOT"

section "📦 Build Package"

PACKAGE_VERSION="$(python3 - <<'PY' "$ECHO_ACTR_DIR/Cargo.toml"
import pathlib
import sys
import tomllib

data = tomllib.loads(pathlib.Path(sys.argv[1]).read_text())
print(data["package"]["version"])
PY
)"

cargo build \
    --manifest-path "$ECHO_ACTR_DIR/Cargo.toml" \
    --lib \
    --release \
    --target "$HOST_TARGET" \
    --features cdylib

case "$(uname)" in
    Darwin)
        ECHO_BINARY_PATH="$WORKSPACE_ROOT/target/$HOST_TARGET/release/libecho_guest.dylib"
        ;;
    Linux)
        ECHO_BINARY_PATH="$WORKSPACE_ROOT/target/$HOST_TARGET/release/libecho_guest.so"
        ;;
    *)
        ECHO_BINARY_PATH="$WORKSPACE_ROOT/target/$HOST_TARGET/release/echo_guest.dll"
        ;;
esac
[[ -f "$ECHO_BINARY_PATH" ]] || fail "Built echo-actr binary not found: $ECHO_BINARY_PATH"

PACKAGE_BUILD_CONFIG="$(mktemp "${TMPDIR:-/tmp}/package-echo-build.XXXXXX.toml")"
cat > "$PACKAGE_BUILD_CONFIG" <<EOF
[package]
publisher = "actrium"
name = "EchoService"
version = "$PACKAGE_VERSION"
description = "Signed Echo guest actor distributed as an ActrPackage"
license = "Apache-2.0"
exports = ["$ECHO_ACTR_DIR/proto/echo.proto"]
EOF

ACTR_PACKAGE="$ECHO_ACTR_DIR/dist/actrium-EchoService-${PACKAGE_VERSION}-${HOST_TARGET}.actr"
SIGNING_KEY="$ECHO_ACTR_DIR/packaging/keys/dev-signing-key.json"
PUBLIC_KEY_PATH="$ECHO_ACTR_DIR/public-key.json"

cargo run --manifest-path "$ACTR_CLI_MANIFEST" --bin actr -- pkg build \
    --binary "$ECHO_BINARY_PATH" \
    --config "$PACKAGE_BUILD_CONFIG" \
    --key "$SIGNING_KEY" \
    --target "$HOST_TARGET" \
    --output "$ACTR_PACKAGE"

[[ -f "$ACTR_PACKAGE" ]] || fail "Package build did not produce $ACTR_PACKAGE"
[[ -f "$SIGNING_KEY" ]] || fail "Signing key not found: $SIGNING_KEY"
[[ -f "$PUBLIC_KEY_PATH" ]] || fail "Public key not found: $PUBLIC_KEY_PATH"

section "🧹 Prepare Isolated State"

rm -rf "$ARTIFACT_DIR" "$WORKSPACE_ROOT/database"
mkdir -p "$COMMAND_DIR" "$TEST_HYPER_DIR"

cp "$SERVER_TEMPLATE_CONFIG" "$TEST_CONFIG"
python3 - <<'PY' "$TEST_CONFIG" "$ACTR_PACKAGE"
import pathlib
import re
import sys

config_path = pathlib.Path(sys.argv[1])
package_path = sys.argv[2]
content = config_path.read_text()
content, replaced = re.subn(
    r'(?m)^path = ".*"$',
    f'path = "{package_path}"',
    content,
    count=1,
)
if replaced != 1:
    raise SystemExit("failed to replace [package].path in server config")
config_path.write_text(content)
PY

ACTR_RUNTIME_ARGS=(--hyper-dir "$TEST_HYPER_DIR" -c "$TEST_CONFIG")

section "🛠️ Resolve Binaries"

ACTR_CLI_TARGET_DIR="$(cargo metadata --manifest-path "$ACTR_CLI_MANIFEST" --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
ACTR_CLI_BIN="$ACTR_CLI_TARGET_DIR/debug/actr"

cargo build --manifest-path "$ACTR_CLI_MANIFEST" --bin actr
[[ -x "$ACTR_CLI_BIN" ]] || fail "actr CLI not found at $ACTR_CLI_BIN"

if [[ -x "$LORAN_DIR/target/debug/loran" ]]; then
    LORAN_CMD="$LORAN_DIR/target/debug/loran"
elif [[ -x "$LORAN_DIR/target/release/loran" ]]; then
    LORAN_CMD="$LORAN_DIR/target/release/loran"
elif command -v loran >/dev/null 2>&1; then
    LORAN_CMD="$(command -v loran)"
else
    cargo build --manifest-path "$LORAN_DIR/Cargo.toml" --bin loran
    LORAN_CMD="$LORAN_DIR/target/debug/loran"
fi
[[ -x "$LORAN_CMD" ]] || fail "loran binary not found"

section "🚀 Start Loran"

if lsof -tiTCP:8081 -sTCP:LISTEN >/dev/null 2>&1; then
    STALE_TCP_PIDS="$(lsof -tiTCP:8081 -sTCP:LISTEN)"
    kill $STALE_TCP_PIDS 2>/dev/null || true
    sleep 1
fi
if lsof -tiUDP:3478 >/dev/null 2>&1; then
    STALE_UDP_PIDS="$(lsof -tiUDP:3478)"
    kill $STALE_UDP_PIDS 2>/dev/null || true
    sleep 1
fi

(cd "$WORKSPACE_ROOT" && "$LORAN_CMD" --config "$LORAN_CONFIG" > "$LORAN_LOG" 2>&1) &
LORAN_PID=$!

LORAN_READY=0
for _ in $(seq 1 30); do
    if ! kill -0 "$LORAN_PID" 2>/dev/null; then
        cat "$LORAN_LOG" >&2 || true
        fail "loran exited before becoming ready"
    fi
    if lsof -nP -iTCP:8081 -sTCP:LISTEN >/dev/null 2>&1; then
        LORAN_READY=1
        break
    fi
    sleep 1
done
[[ "$LORAN_READY" -eq 1 ]] || fail "loran did not listen on 8081 in time"

LORAN_DB="$(detect_loran_db || true)"
[[ -n "$LORAN_DB" && -f "$LORAN_DB" ]] || fail "Loran database not found after startup"

section "🔑 Wait For Signer"

signer_status() {
    python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("signer_service", "missing"))
except Exception:
    print("missing")'
}

for _ in $(seq 1 60); do
    if [[ "$(curl -sf "http://localhost:8081/issuer/health" 2>/dev/null | signer_status)" == "ok" ]]; then
        break
    fi
    sleep 1
done

[[ "$(curl -sf "http://localhost:8081/issuer/health" 2>/dev/null | signer_status)" == "ok" ]] \
    || fail "Signer did not become ready"

section "📡 Register publisher And Publish Package"

PUBLISHER_PUBKEY="$(python3 - <<'PY' "$PUBLIC_KEY_PATH"
import json
import pathlib
import sys

data = json.loads(pathlib.Path(sys.argv[1]).read_text())
print(data["public_key"])
PY
)"

PUBLISHER_KEY_ID="$(python3 - <<'PY' "$PUBLISHER_PUBKEY"
import base64
import hashlib
import sys

pubkey = base64.b64decode(sys.argv[1])
digest = hashlib.sha256(pubkey).hexdigest()
print(f"publisher-{digest[:16]}")
PY
)"

NOW="$(date +%s)"
EXPIRES_AT="$((NOW + 86400 * 365))"

sqlite3 "$LORAN_DB" <<SQL
INSERT OR IGNORE INTO realm
    (name, status, enabled, created_at)
VALUES ('package-echo@example.com', 'active', 1, strftime('%s','now'));

INSERT OR IGNORE INTO publisher
    (name, public_key, key_id, contact, status, created_at, updated_at, verified_at, key_expires_at)
VALUES ('actrium', '$PUBLISHER_PUBKEY', '$PUBLISHER_KEY_ID', 'examples@actrium.local', 'active',
        $NOW, $NOW, $NOW, $EXPIRES_AT);

UPDATE publisher
   SET public_key     = '$PUBLISHER_PUBKEY',
       key_id         = '$PUBLISHER_KEY_ID',
       status         = 'active',
       updated_at     = $NOW,
       verified_at    = $NOW,
       key_expires_at = $EXPIRES_AT,
       suspended_at   = NULL,
       revoked_at     = NULL
 WHERE name = 'actrium';
SQL

# Join the Realm as its owner; the config names the key file next to it.
ACTR_CMD="$ACTR_CLI_BIN" "$WORKSPACE_ROOT/scripts/join-realm.sh" \
    "$LORAN_DB" "package-echo@example.com" "http://localhost:8081/issuer" \
    "$ARTIFACT_DIR/realm-client.json"

section "🧪 actr run -d"

RUN_OUTPUT="$("$ACTR_CLI_BIN" run -d "${ACTR_RUNTIME_ARGS[@]}" 2>&1)"
printf '%s\n' "$RUN_OUTPUT" > "$COMMAND_DIR/run-detached.txt"

assert_contains "$RUN_OUTPUT" "Detached runtime started" "`actr run -d` should report detached startup"

WID_SHORT="$(printf '%s\n' "$RUN_OUTPUT" | awk -F': *' '/WID:/ {print $2; exit}')"
RUNTIME_PID="$(printf '%s\n' "$RUN_OUTPUT" | awk -F': *' '/PID:/ {print $2; exit}')"

[[ -n "$WID_SHORT" ]] || fail "Failed to parse WID from detached run output"
[[ -n "$RUNTIME_PID" ]] || fail "Failed to parse PID from detached run output"

assert_contains "$RUN_OUTPUT" "Follow logs: actr logs $WID_SHORT -f" "`actr run -d` should print follow hint"

wait_for_record
wait_for_status "running"

FULL_WID="$(record_field wid)"
ACTR_ID="$(record_field actr_id)"
RUN_PID="$(record_field pid)"
RUN_LOG_PATH="$(record_field log_path)"
RUN_STARTED_AT="$(record_field started_at)"

assert_contains "$FULL_WID" "$WID_SHORT" "Detached runtime record should keep the same WID prefix"
assert_equals "$RUN_PID" "$RUNTIME_PID" "Runtime record PID should match detached startup output"
[[ -f "$RUN_LOG_PATH" ]] || fail "Runtime log file not found: $RUN_LOG_PATH"

PS_OUTPUT="$("$ACTR_CLI_BIN" ps "${ACTR_RUNTIME_ARGS[@]}" --log 2>&1)"
printf '%s\n' "$PS_OUTPUT" > "$COMMAND_DIR/ps-running.txt"

assert_contains "$PS_OUTPUT" "WID" "`actr ps` should show WID column"
assert_contains "$PS_OUTPUT" "ACTR_ID" "`actr ps` should show ACTR_ID column"
assert_contains "$PS_OUTPUT" "PID" "`actr ps` should show PID column"
assert_contains "$PS_OUTPUT" "STATUS" "`actr ps` should show STATUS column"
assert_contains "$PS_OUTPUT" "STARTED_AT" "`actr ps` should show STARTED_AT column"
assert_contains "$PS_OUTPUT" "$WID_SHORT" "`actr ps` should show the runtime WID"
assert_contains "$PS_OUTPUT" "${ACTR_ID:0:16}" "`actr ps` should show the ACTR_ID"
assert_contains "$PS_OUTPUT" "$RUN_PID" "`actr ps` should show the PID"
assert_contains "$PS_OUTPUT" "running" "`actr ps` should show running status"
assert_contains "$PS_OUTPUT" "${RUN_STARTED_AT%%T*}" "`actr ps` should show started_at"

section "🛑 actr stop → actr start"

STOP_OUTPUT="$("$ACTR_CLI_BIN" stop "${ACTR_RUNTIME_ARGS[@]}" "$WID_SHORT" 2>&1)"
printf '%s\n' "$STOP_OUTPUT" > "$COMMAND_DIR/stop.txt"
assert_contains "$STOP_OUTPUT" "Stopped runtime: $WID_SHORT" "`actr stop` should stop the runtime"

wait_for_status "exited"

START_OUTPUT="$("$ACTR_CLI_BIN" start "${ACTR_RUNTIME_ARGS[@]}" "$WID_SHORT" 2>&1)"
printf '%s\n' "$START_OUTPUT" > "$COMMAND_DIR/start.txt"
assert_contains "$START_OUTPUT" "Detached runtime started" "`actr start` should launch the runtime again"

wait_for_pid_change "$RUN_PID"
wait_for_status "running"

START_WID="$(record_field wid)"
START_PID="$(record_field pid)"

assert_equals "$START_WID" "$FULL_WID" "`actr start` should preserve the same full WID"
assert_not_equals "$START_PID" "$RUN_PID" "`actr start` should create a new PID"

RUNTIME_PID="$START_PID"

section "🔄 actr restart + logs -f"

"$ACTR_CLI_BIN" logs "${ACTR_RUNTIME_ARGS[@]}" -f "$WID_SHORT" > "$FOLLOW_CAPTURE" 2>&1 &
LOGS_FOLLOW_PID=$!
sleep 2

START_LOG_PATH="$(record_field log_path)"
RESTART_OUTPUT="$("$ACTR_CLI_BIN" restart "${ACTR_RUNTIME_ARGS[@]}" "$WID_SHORT" 2>&1)"
printf '%s\n' "$RESTART_OUTPUT" > "$COMMAND_DIR/restart.txt"
assert_contains "$RESTART_OUTPUT" "Stopping runtime: $WID_SHORT" "`actr restart` should stop the runtime first"
assert_contains "$RESTART_OUTPUT" "Starting runtime with config: $TEST_CONFIG" "`actr restart` should reuse the same config"

wait_for_pid_change "$START_PID"
wait_for_status "running"
sleep 3

stop_logs_follow

RESTART_WID="$(record_field wid)"
RESTART_PID="$(record_field pid)"
RESTART_LOG_PATH="$(record_field log_path)"

assert_equals "$RESTART_WID" "$FULL_WID" "`actr restart` should preserve the full WID"
assert_not_equals "$RESTART_PID" "$START_PID" "`actr restart` should create a new PID"
assert_equals "$RESTART_LOG_PATH" "$START_LOG_PATH" "`actr restart` should preserve the log file path"

RUNTIME_PID="$RESTART_PID"

START_COUNT="$(grep -c "ActrNode started" "$FOLLOW_CAPTURE" || true)"
if [[ "$START_COUNT" -lt 3 ]]; then
    fail "`actr logs -f` did not capture continuous startup logs across restart"
fi

section "📁 v1 Runtime Schema Error"

"$ACTR_CLI_BIN" stop "${ACTR_RUNTIME_ARGS[@]}" "$WID_SHORT" > "$COMMAND_DIR/stop-before-schema.txt" 2>&1 || true
RUNTIME_PID=""

RUNTIME_RECORD_PATH="$(record_path)"
[[ -n "$RUNTIME_RECORD_PATH" ]] || fail "Runtime record not found before schema downgrade"

python3 - <<'PY' "$RUNTIME_RECORD_PATH"
import json
import pathlib
import sys

record_path = pathlib.Path(sys.argv[1])
data = json.loads(record_path.read_text())
data["schema_version"] = 1
record_path.write_text(json.dumps(data, indent=2) + "\n")
PY

set +e
SCHEMA_OUTPUT="$("$ACTR_CLI_BIN" ps "${ACTR_RUNTIME_ARGS[@]}" --all 2>&1)"
SCHEMA_STATUS=$?
set -e
printf '%s\n' "$SCHEMA_OUTPUT" > "$COMMAND_DIR/ps-schema-v1.txt"

if [[ "$SCHEMA_STATUS" -eq 0 ]]; then
    fail "Schema downgrade test should fail for v1 runtime records"
fi

RUN_DIR_PATH="$TEST_HYPER_DIR/run"
assert_contains "$SCHEMA_OUTPUT" "Incompatible runtime record schema v1" "v1 runtime record should be rejected"
assert_contains "$SCHEMA_OUTPUT" "$RUN_DIR_PATH" "Schema error should include the run directory path"
assert_contains "$SCHEMA_OUTPUT" 're-run `actr run -d`' "Schema error should explain how to recover"

section "✅ Completed"

echo -e "${GREEN}All detached runtime lifecycle checks passed.${NC}"
echo "Artifacts:"
echo "  Config:   $TEST_CONFIG"
echo "  Hyper:    $TEST_HYPER_DIR"
echo "  Commands: $COMMAND_DIR"
echo "  Follow:   $FOLLOW_CAPTURE"
