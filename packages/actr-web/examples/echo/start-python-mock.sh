#!/usr/bin/env bash
# Web client -> generated Python EchoService e2e using mock-loran.

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ACTR_ROOT="$(cd "$PROJECT_ROOT/../.." && pwd)"

CLIENT_GUEST_DIR="$SCRIPT_DIR/client-guest"
PY_WORKLOAD_DIR="$ACTR_ROOT/examples/actr/python/echo-workload"
RELEASE_DIR="$SCRIPT_DIR/release"
LOG_DIR="$SCRIPT_DIR/logs/python-workload"
RUN_DIR="$LOG_DIR/run"
MOCK_PORT="${1:-18081}"
REALM="python-web-echo@example.com"
PUBLISHER_NAME="acme"
PYTHON_SERVER_READY_ATTEMPTS="${PYTHON_SERVER_READY_ATTEMPTS:-600}"
ACTR_E2E_RUST_LOG="${ACTR_E2E_RUST_LOG:-${RUST_LOG:-info}}"

mkdir -p "$RELEASE_DIR" "$LOG_DIR" "$RUN_DIR"

export PATH="$ACTR_ROOT/target/debug:$ACTR_ROOT/target/release:$HOME/.cargo/bin:$PATH"

MOCK_PID=""
PY_SERVER_PID=""
CLIENT_PID=""

cleanup() {
    set +e
    if [[ -n "$CLIENT_PID" ]]; then kill "$CLIENT_PID" 2>/dev/null || true; fi
    if [[ -n "$PY_SERVER_PID" ]]; then kill "$PY_SERVER_PID" 2>/dev/null || true; fi
    if [[ -n "$MOCK_PID" ]]; then kill "$MOCK_PID" 2>/dev/null || true; fi
    wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

log_step() {
    echo -e "${BLUE}$1${NC}"
}

fail() {
    echo -e "${RED}$1${NC}" >&2
    exit 1
}

wait_for_log() {
    local file="$1"
    local needle="$2"
    local label="$3"
    local attempts="${4:-300}"
    for _ in $(seq 1 "$attempts"); do
        if grep -q "$needle" "$file" 2>/dev/null; then
            return 0
        fi
        sleep 0.2
    done
    echo -e "${RED}${label} did not become ready${NC}" >&2
    cat "$file" >&2 2>/dev/null || true
    return 1
}

dump_process_state() {
    local pid="$1"
    local label="$2"
    if [[ -z "$pid" ]]; then
        return 0
    fi

    echo -e "${YELLOW}${label} process state:${NC}" >&2
    ps -fp "$pid" >&2 2>/dev/null || true
    if [[ -r "/proc/$pid/status" ]]; then
        cat "/proc/$pid/status" >&2 || true
    fi
}

wait_for_port() {
    local port="$1"
    local label="$2"
    for _ in $(seq 1 300); do
        if nc -z 127.0.0.1 "$port" 2>/dev/null; then
            return 0
        fi
        sleep 0.2
    done
    echo -e "${RED}${label} did not bind port ${port}${NC}" >&2
    return 1
}

for port in "$MOCK_PORT" 5173; do
    pids="$(lsof -ti:"$port" 2>/dev/null || true)"
    if [[ -n "$pids" ]]; then
        echo "$pids" | xargs kill -9 2>/dev/null || true
    fi
done

if [[ -n "${ACTR_CMD:-}" ]]; then
    [[ -x "$ACTR_CMD" ]] || fail "ACTR_CMD is not executable: $ACTR_CMD"
else
    if [[ -x "$ACTR_ROOT/target/release/actr" ]]; then
        ACTR_CMD="$ACTR_ROOT/target/release/actr"
    else
        log_step "Building current actr CLI..."
        (cd "$ACTR_ROOT" && cargo build -p actr-cli --bin actr --features wasm-engine)
        ACTR_CMD="$ACTR_ROOT/target/debug/actr"
    fi
fi

MOCK_BIN="$ACTR_ROOT/target/debug/mock-loran"
if [[ ! -x "$MOCK_BIN" ]]; then
    log_step "Building mock-loran..."
    (cd "$ACTR_ROOT" && cargo build -p actr-mock-loran --bin mock-loran)
fi
[[ -x "$MOCK_BIN" ]] || fail "mock-loran not found at $MOCK_BIN"

command -v wasm-pack >/dev/null 2>&1 || fail "wasm-pack not found"

PUBLISHER_KEY_FILE="$RELEASE_DIR/python-web-dev-key.json"
log_step "Generating shared development signing key..."
"$ACTR_CMD" pkg keygen --output "$PUBLISHER_KEY_FILE" --force >/dev/null
PUBLISHER_PUBKEY="$(python3 -c "import json; print(json.load(open('$PUBLISHER_KEY_FILE'))['public_key'])")"

log_step "Building generated Python EchoService package..."
(
    cd "$PY_WORKLOAD_DIR"
    ACTR_SIGNING_KEY="$PUBLISHER_KEY_FILE" PATH="$ACTR_ROOT/target/debug:$PATH" ./build.sh package
)
PY_ACTR_PACKAGE="$PY_WORKLOAD_DIR/dist/acme-EchoService-0.2.0-wasm32-wasip2.actr"
[[ -f "$PY_ACTR_PACKAGE" ]] || fail "missing Python package: $PY_ACTR_PACKAGE"

log_step "Building web client guest package..."
(
    export RUSTFLAGS=""
    export CARGO_ENCODED_RUSTFLAGS=""
    cd "$CLIENT_GUEST_DIR"
    wasm-pack build --target no-modules --release --out-dir pkg \
        -- --no-default-features --features web
)

CLIENT_ACTR_PACKAGE="$RELEASE_DIR/acme-echo-client-app-0.1.0-wasm32-unknown-unknown.actr"
(
    cd "$CLIENT_GUEST_DIR"
    "$ACTR_CMD" build \
        --no-compile \
        --target wasm32-unknown-unknown \
        --key "$PUBLISHER_KEY_FILE" \
        --output "$CLIENT_ACTR_PACKAGE"
)

log_step "Starting mock-loran..."
MOCK_LOG="$LOG_DIR/mock-loran.log"
: > "$MOCK_LOG"
"$MOCK_BIN" --port "$MOCK_PORT" > "$MOCK_LOG" 2>&1 &
MOCK_PID=$!
wait_for_log "$MOCK_LOG" "listening on 127.0.0.1:$MOCK_PORT" "mock-loran"
ENDPOINT="http://127.0.0.1:$MOCK_PORT"

log_step "Registering Realm and Publisher key..."
curl -fsS -X POST "$ENDPOINT/admin/realms" \
    -H 'content-type: application/json' \
    --data "{\"realm\": \"$REALM\"}" >/dev/null
curl -fsS -X POST "$ENDPOINT/admin/publisher" \
    -H 'content-type: application/json' \
    --data "{\"name\": \"$PUBLISHER_NAME\", \"pubkey_b64\": \"$PUBLISHER_PUBKEY\", \"contact\": \"dev@example.com\"}" >/dev/null


PY_SERVER_CONFIG="$RUN_DIR/python-server-actr.toml"
CLIENT_CONFIG="$RUN_DIR/client-actr.toml"

cat > "$PY_SERVER_CONFIG" <<EOF
edition = 1

[package]
path = "$PY_ACTR_PACKAGE"

[signaling]
url = "ws://127.0.0.1:$MOCK_PORT/signaling/ws"

[issuer_endpoint]
url = "http://127.0.0.1:$MOCK_PORT/issuer"

[deployment]
realm = "$REALM"

[discovery]
visible = true

[observability]
filter_level = "info"
tracing_enabled = false

[webrtc]
force_relay = false
stun_urls = ["stun:localhost:3478"]
turn_urls = ["turn:localhost:3478"]

[acl]

[[acl.rules]]
permission = "allow"
type = "acme/echo-client-app@0.1.0"

[[trust]]
kind = "static"
pubkey_b64 = "$PUBLISHER_PUBKEY"
EOF

cat > "$CLIENT_CONFIG" <<EOF
edition = 1

[package]
path = "$CLIENT_ACTR_PACKAGE"

[signaling]
url = "ws://127.0.0.1:$MOCK_PORT/signaling/ws"

[issuer_endpoint]
url = "http://127.0.0.1:$MOCK_PORT/issuer"

[deployment]
realm = "$REALM"

[discovery]
visible = true

[observability]
filter_level = "info"
tracing_enabled = false

[webrtc]
force_relay = false
stun_urls = ["stun:localhost:3478"]
turn_urls = ["turn:localhost:3478"]

[acl]

[[acl.rules]]
permission = "allow"
type = "acme/EchoService@0.2.0"

[web]
port = 5173
host = "127.0.0.1"

[[trust]]
kind = "static"
pubkey_b64 = "$PUBLISHER_PUBKEY"
EOF

log_step "Starting native Python EchoService runtime..."
PY_SERVER_LOG="$LOG_DIR/python-server.log"
: > "$PY_SERVER_LOG"
RUST_LOG="$ACTR_E2E_RUST_LOG" "$ACTR_CMD" run -c "$PY_SERVER_CONFIG" > "$PY_SERVER_LOG" 2>&1 &
PY_SERVER_PID=$!
if ! wait_for_log \
    "$PY_SERVER_LOG" \
    "ActrNode started" \
    "Python server" \
    "$PYTHON_SERVER_READY_ATTEMPTS"; then
    dump_process_state "$PY_SERVER_PID" "Python server"
    exit 1
fi

log_step "Starting actr-web client runtime..."
CLIENT_LOG="$LOG_DIR/client.log"
: > "$CLIENT_LOG"
RUST_LOG="$ACTR_E2E_RUST_LOG" "$ACTR_CMD" run --web -c "$CLIENT_CONFIG" > "$CLIENT_LOG" 2>&1 &
CLIENT_PID=$!
wait_for_port 5173 "actr-web client"

log_step "Running browser assertion..."
if ! node -e "require('puppeteer')" 2>/dev/null; then
    for candidate in \
        "$PROJECT_ROOT/node_modules" \
        "$ACTR_ROOT/node_modules"; do
        if [[ -d "$candidate" ]] && NODE_PATH="$candidate" node -e "require('puppeteer')" 2>/dev/null; then
            export NODE_PATH="$candidate:${NODE_PATH:-}"
            break
        fi
    done
fi

if ! node -e "require('puppeteer')" 2>/dev/null; then
    log_step "Installing local Puppeteer dependency..."
    PUPPETEER_NODE_DIR="$LOG_DIR/node-deps"
    mkdir -p "$PUPPETEER_NODE_DIR"
    PUPPETEER_SKIP_DOWNLOAD=1 npm install --prefix "$PUPPETEER_NODE_DIR" puppeteer@24.39.0
    export NODE_PATH="$PUPPETEER_NODE_DIR/node_modules:${NODE_PATH:-}"
fi

if ! node -e "require('puppeteer').launch({headless:'new'}).then(b=>b.close())" 2>/dev/null; then
    if [[ -x "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" ]]; then
        export PUPPETEER_EXECUTABLE_PATH="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
    fi
fi

CLIENT_URL="http://127.0.0.1:5173" \
PYTHON_ECHO_MESSAGE="${PYTHON_ECHO_MESSAGE:-hello-from-actr-web}" \
node "$SCRIPT_DIR/test-python-workload.js"

echo -e "${GREEN}Python workload web echo PASSED${NC}"
