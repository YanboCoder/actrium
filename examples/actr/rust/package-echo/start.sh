#!/bin/bash
# Test script for package-echo example — echo-actr package loaded from local build
#
# Demonstrates the full package-driven execution flow:
#   1. Build the local echo-actr package and reuse its public key
#   2. Verify the signed .actr archive
#   3. Host server loads the package and picks the workload from package target
#   4. Client discovers the echo service, sends messages, verifies responses
#
# Usage:
#   ./start.sh                        # Use default message "TestMsg"
#   ./start.sh "Hello"                # Send custom message
#   ./start.sh --backend cdylib       # Use native cdylib backend (default: wasm)
#   ./start.sh --backend cdylib "Hi"  # cdylib backend with custom message
#   ./start.sh --client-mode http     # Expose the client Actor on 127.0.0.1:8391
#   ./start.sh --client-mode http --keep-running

set -e
set -o pipefail

# ── Shared ports & defaults (single source of truth for this example) ────
LORAN_HTTP_PORT=${LORAN_HTTP_PORT:-8081}
LORAN_ICE_PORT=${LORAN_ICE_PORT:-13478}
LORAN_HTTP_URL="http://localhost:${LORAN_HTTP_PORT}"
LORAN_WS_URL="ws://localhost:${LORAN_HTTP_PORT}"
LORAN_ADMIN_PASSWORD=${LORAN_ADMIN_PASSWORD:-testpassword123}

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# ── Argument parsing ─────────────────────────────────────────────────────
BACKEND="wasm"
CLIENT_MODE="cli"
HTTP_LISTEN="${ECHO_HTTP_LISTEN:-127.0.0.1:8391}"
KEEP_RUNNING="${ECHO_KEEP_RUNNING:-0}"
TEST_INPUT="TestMsg"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --backend)
            BACKEND="$2"; shift 2 ;;
        --backend=*)
            BACKEND="${1#--backend=}"; shift ;;
        --client-mode)
            CLIENT_MODE="$2"; shift 2 ;;
        --client-mode=*)
            CLIENT_MODE="${1#--client-mode=}"; shift ;;
        --http-listen)
            HTTP_LISTEN="$2"; shift 2 ;;
        --http-listen=*)
            HTTP_LISTEN="${1#--http-listen=}"; shift ;;
        --keep-running)
            KEEP_RUNNING=1; shift ;;
        -*)
            echo "Unknown option: $1" >&2; exit 1 ;;
        *)
            TEST_INPUT="$1"; shift ;;
    esac
done
if [[ "$BACKEND" != "wasm" && "$BACKEND" != "cdylib" ]]; then
    echo "Invalid --backend value: '$BACKEND' (must be 'wasm' or 'cdylib')" >&2
    exit 1
fi
if [[ "$CLIENT_MODE" != "cli" && "$CLIENT_MODE" != "http" ]]; then
    echo "Invalid --client-mode value: '$CLIENT_MODE' (must be 'cli' or 'http')" >&2
    exit 1
fi

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🧪 Testing package-echo (backend: $BACKEND)"
echo "    Using Loran as the Actor infrastructure service"
echo "    Client Host mode: $CLIENT_MODE"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# ── Paths ────────────────────────────────────────────────────────────────

WORKSPACE_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Repo root is 3 levels up from WORKSPACE_ROOT (examples/actr/rust).
ACTR_REPO_DIR="$(cd "$WORKSPACE_ROOT/../../.." && pwd)"

resolve_loran_dir() {
    local repo_dir="$1"
    local candidate=""

    if [ -f "$repo_dir/crates/loran-server/Cargo.toml" ]; then
        printf '%s\n' "$repo_dir"
        return 0
    fi

    if [ -n "${LORAN_DIR:-}" ] && [ -d "${LORAN_DIR}" ]; then
        printf '%s\n' "${LORAN_DIR}"
        return 0
    fi

    candidate="$(cd "$repo_dir/.." && pwd)/loran"
    if [ -d "$candidate" ]; then
        printf '%s\n' "$candidate"
        return 0
    fi

    while IFS= read -r candidate; do
        if [ -n "$candidate" ] && [ "$candidate" != "$repo_dir" ] && [ -d "$(cd "$candidate/.." && pwd)/loran" ]; then
            printf '%s\n' "$(cd "$candidate/.." && pwd)/loran"
            return 0
        fi
    done <<EOF
$(git -C "$repo_dir" worktree list --porcelain 2>/dev/null | awk '/^worktree / { print substr($0, 10) }')
EOF

    return 1
}

LORAN_DIR="$(resolve_loran_dir "$ACTR_REPO_DIR" || true)"
ACTR_CLI_MANIFEST="$ACTR_REPO_DIR/crates/actr-cli/Cargo.toml"
LORAN_CONFIG="$WORKSPACE_ROOT/loran-config.toml"
PACKAGE_ECHO_DIR="$WORKSPACE_ROOT/package-echo"
CLIENT_DIR="$PACKAGE_ECHO_DIR/client"
CLIENT_GUEST_DIR="$PACKAGE_ECHO_DIR/client-guest"
ECHO_ACTR_DIR="${ECHO_ACTR_DIR:-$WORKSPACE_ROOT/echo-actr}"

# Ensure ~/.cargo/bin is in PATH
export PATH="$HOME/.cargo/bin:$PATH"

# ── Check and install jq if needed ───────────────────────────────────────
if ! command -v jq >/dev/null 2>&1; then
    echo ""
    echo -e "${YELLOW}⚠️  jq not found, attempting to install...${NC}"

    if [[ "$OSTYPE" == "darwin"* ]]; then
        # macOS - use Homebrew
        if command -v brew >/dev/null 2>&1; then
            echo "Installing jq via Homebrew..."
            brew install jq
        else
            echo -e "${RED}❌ Homebrew not found. Please install jq manually:${NC}"
            echo "   brew install jq"
            echo "   or visit: https://jqlang.github.io/jq/download/"
            exit 1
        fi
    elif [[ "$OSTYPE" == "linux-gnu"* ]]; then
        # Linux - try common package managers
        if command -v apt-get >/dev/null 2>&1; then
            echo "Installing jq via apt-get..."
            sudo apt-get update && sudo apt-get install -y jq
        elif command -v yum >/dev/null 2>&1; then
            echo "Installing jq via yum..."
            sudo yum install -y jq
        elif command -v dnf >/dev/null 2>&1; then
            echo "Installing jq via dnf..."
            sudo dnf install -y jq
        else
            echo -e "${RED}❌ No supported package manager found. Please install jq manually.${NC}"
            exit 1
        fi
    else
        echo -e "${RED}❌ Unsupported OS: $OSTYPE. Please install jq manually.${NC}"
        exit 1
    fi

    # Verify installation
    if command -v jq >/dev/null 2>&1; then
        echo -e "${GREEN}✅ jq installed successfully: $(jq --version)${NC}"
    else
        echo -e "${RED}❌ jq installation failed${NC}"
        exit 1
    fi
else
    echo -e "${GREEN}✅ jq found: $(jq --version)${NC}"
fi

cd "$WORKSPACE_ROOT"

# Create logs directory
LOG_DIR="$WORKSPACE_ROOT/logs"
mkdir -p "$LOG_DIR"

# Ensure required helper scripts
source "$WORKSPACE_ROOT/scripts/ensure-tools.sh"
source "$WORKSPACE_ROOT/scripts/ensure-config-toml.sh"

ensure_project_keychain_config() {
    local project_dir="$1"
    local signing_key="$2"
    local config_dir="$project_dir/.actr"
    local config_path="$config_dir/config.toml"

    mkdir -p "$config_dir"
    cat > "$config_path" << TOML
[publisher]
keychain = "$signing_key"
TOML
}

resolve_codegen_input_dir() {
    local project_dir="$1"

    if [ -d "$project_dir/proto" ]; then
        printf '%s\n' "proto"
        return 0
    fi

    if [ -d "$project_dir/protos" ]; then
        printf '%s\n' "protos"
        return 0
    fi

    return 1
}

# ── Clean stale database and config files ────────────────────────────────
# Remove DB files from previous runs so loran starts with fresh keys.
# Without this, expired signing keys cause "Invalid credential format" errors.
echo ""
echo "🗑️  Cleaning stale database files..."
rm -rf "$WORKSPACE_ROOT/database"
# Remove runtime config files to ensure they're freshly copied from Actr.example.toml,
# and manifest files from Actr.example.toml
rm -f "$CLIENT_DIR/actr.toml" "$CLIENT_GUEST_DIR/actr.toml"
echo -e "${GREEN}✅ Stale database cleaned${NC}"

# Ensure manifest.toml and actr.toml files exist
echo ""
echo "🔍 Checking config files..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
ensure_actr_toml "$CLIENT_DIR"
ensure_actr_toml "$CLIENT_GUEST_DIR"
# actr.toml = runtime config (from Actr.example.toml)
echo -e "${GREEN}✅ Synchronized actr.toml from Actr.example.toml${NC}"

# Keep the disposable Loran config synchronized with the tracked template.
cp "$WORKSPACE_ROOT/loran-config.example.toml" "$LORAN_CONFIG"
ensure_loran_config "$WORKSPACE_ROOT"

# Keep every disposable runtime config on this demo's ports.  The non-standard
# TURN port deliberately avoids colliding with a machine-wide TURN service.
python3 - "$LORAN_CONFIG" "$CLIENT_DIR/actr.toml" "$CLIENT_GUEST_DIR/actr.toml" \
    "$LORAN_HTTP_PORT" "$LORAN_ICE_PORT" <<'PY'
from pathlib import Path
import sys

config_path, client_path, guest_path, http_port, ice_port = sys.argv[1:]

loran = Path(config_path)
text = loran.read_text(encoding="utf-8")
text = text.replace('port = 8081', f'port = {http_port}', 1)
text = text.replace('advertised_port = 3478', f'advertised_port = {ice_port}', 1)
text = text.replace('port = 3478', f'port = {ice_port}', 1)
loran.write_text(text, encoding="utf-8")

for runtime_path in (client_path, guest_path):
    path = Path(runtime_path)
    text = path.read_text(encoding="utf-8")
    text = text.replace('localhost:8081', f'localhost:{http_port}')
    text = text.replace('localhost:3478', f'localhost:{ice_port}')
    path.write_text(text, encoding="utf-8")
PY

# ── Cleanup ──────────────────────────────────────────────────────────────

LORAN_PID=""
SERVER_PID=""
CLIENT_PID=""

cleanup() {
    echo ""
    echo "🧹 Cleaning up..."

    if [ -n "$CLIENT_PID" ]; then
        echo "Stopping package-echo-client (PID: $CLIENT_PID)"
        kill "$CLIENT_PID" 2>/dev/null || true
    fi

    if [ -n "$SERVER_PID" ]; then
        echo "Stopping package-echo-server (PID: $SERVER_PID)"
        kill "$SERVER_PID" 2>/dev/null || true
    fi

    if [ -n "$LORAN_PID" ]; then
        echo "Stopping Loran (PID: $LORAN_PID)"
        kill "$LORAN_PID" 2>/dev/null || true
    fi

    wait 2>/dev/null || true
    echo "✅ Cleanup complete"
}

trap cleanup EXIT INT TERM

# ── Step 0: Build echo-actr package ─────────────────────────────────────

echo ""
if [ "$BACKEND" = "cdylib" ]; then
    echo -e "${BLUE}📦 Step 0: Building echo-actr (cdylib / native)...${NC}"
else
    echo -e "${BLUE}📦 Step 0: Building echo-actr (wasm)...${NC}"
fi
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

echo_actr_version() {
    awk '
        /^\[package\]/ { in_package = 1; next }
        /^\[/ && in_package { exit }
        in_package && $1 == "version" {
            gsub(/"/, "", $3)
            print $3
            exit
        }
    ' "$ECHO_ACTR_DIR/Cargo.toml"
}

ECHO_ACTR_VERSION="${ECHO_ACTR_VERSION:-$(echo_actr_version)}"
SIGNING_KEY="$ECHO_ACTR_DIR/packaging/keys/dev-signing-key.json"
PUBLIC_KEY_PATH="$ECHO_ACTR_DIR/public-key.json"
ECHO_ACTR_BUILD_MANIFEST="$ECHO_ACTR_DIR/manifest.toml"
if [ "$BACKEND" = "cdylib" ] && [ -f "$ECHO_ACTR_DIR/manifest-cdylib.toml" ]; then
    ECHO_ACTR_BUILD_MANIFEST="$ECHO_ACTR_DIR/manifest-cdylib.toml"
fi

if [ ! -d "$ECHO_ACTR_DIR" ]; then
    echo -e "${RED}❌ echo-actr repository not found: $ECHO_ACTR_DIR${NC}"
    exit 1
fi

if [ ! -f "$SIGNING_KEY" ]; then
    echo -e "${RED}❌ signing key not found: $SIGNING_KEY${NC}"
    exit 1
fi

if [ ! -f "$PUBLIC_KEY_PATH" ]; then
    mkdir -p "$(dirname "$PUBLIC_KEY_PATH")"
    jq '{public_key: .public_key}' "$SIGNING_KEY" > "$PUBLIC_KEY_PATH"
fi

echo ""
echo -e "${BLUE}🧬 Step 0a: Installing manifest lock and generating echo-actr sources...${NC}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
ensure_project_keychain_config "$ECHO_ACTR_DIR" "$SIGNING_KEY"
CODEGEN_INPUT_DIR="$(resolve_codegen_input_dir "$ECHO_ACTR_DIR" || true)"
if [ -z "$CODEGEN_INPUT_DIR" ]; then
    echo -e "${RED}❌ Unable to detect codegen input dir under $ECHO_ACTR_DIR (expected proto/ or protos/)${NC}"
    exit 1
fi

# Build the workspace plugin before codegen. `actr gen` checks the workload,
# which runs its build script; allowing that build script to invoke Cargo for
# the same plugin would wait on Cargo's own target lock.
CARGO_TARGET_DIR="$ACTR_REPO_DIR/target" cargo build \
    --manifest-path "$ACTR_REPO_DIR/tools/actr/protoc-gen/rust/Cargo.toml" \
    --bin protoc-gen-actrframework >/dev/null
export PATH="$ACTR_REPO_DIR/target/debug:$PATH"
(
    cd "$ECHO_ACTR_DIR"
    cargo run --manifest-path "$ACTR_CLI_MANIFEST" --bin actr --features wasm-engine -- deps install >/dev/null
    cargo run --manifest-path "$ACTR_CLI_MANIFEST" --bin actr --features wasm-engine -- gen \
        -l rust \
        --input "$CODEGEN_INPUT_DIR" \
        --output src/generated \
        --clean \
        --no-scaffold >/dev/null
)
echo -e "${GREEN}✅ echo-actr manifest.lock.toml and generated sources ready${NC}"

echo "echo-actr dir: $ECHO_ACTR_DIR"
echo "Version:       $ECHO_ACTR_VERSION"

# Patch client actr.toml ACL to reference the correct EchoService version
perl -0pi -e "s/type = \"actrium:EchoService:[^\"]+\"/type = \"actrium:EchoService:${ECHO_ACTR_VERSION}\"/g" \
    "$CLIENT_DIR/actr.toml" "$CLIENT_GUEST_DIR/actr.toml" 2>/dev/null || true

if [ "$BACKEND" = "cdylib" ]; then
    ECHO_ACTR_TARGET="$(rustc -vV | awk '/host:/ {print $2}')"
    echo "Target:        $ECHO_ACTR_TARGET (cdylib)"
    ACTR_PACKAGE="$ECHO_ACTR_DIR/dist/actrium-EchoService-${ECHO_ACTR_VERSION}-${ECHO_ACTR_TARGET}.actr"

    cargo run --manifest-path "$ACTR_CLI_MANIFEST" --bin actr --features wasm-engine -- build \
        --manifest-path "$ECHO_ACTR_BUILD_MANIFEST" \
        --key "$SIGNING_KEY" \
        --output "$ACTR_PACKAGE"

    if [ ! -f "$ACTR_PACKAGE" ]; then
        echo -e "${RED}❌ actr build failed: $ACTR_PACKAGE not found${NC}"
        exit 1
    fi
    echo -e "${GREEN}✅ cdylib .actr package built: $(du -h "$ACTR_PACKAGE" | cut -f1)${NC}"
else
    ECHO_ACTR_TARGET="wasm32-wasip2"
    echo "Target:        $ECHO_ACTR_TARGET (Component Model)"
    rustup target add "$ECHO_ACTR_TARGET" >/dev/null

    # A clean Rust installation does not include the Component Model linker.
    # Keep the demo self-bootstrapping, while reusing any compatible version
    # that the developer already has installed.
    WASM_COMPONENT_LD_MIN_VERSION="0.5.22"
    WASM_COMPONENT_LD_INSTALL_VERSION="0.5.26"
    WASM_COMPONENT_LD_VERSION="$(wasm-component-ld --version 2>/dev/null | awk '{ print $2 }' || true)"
    if ! python3 - "$WASM_COMPONENT_LD_VERSION" "$WASM_COMPONENT_LD_MIN_VERSION" <<'PY'
import sys

def version_tuple(value: str) -> tuple[int, ...]:
    try:
        return tuple(int(part) for part in value.split("."))
    except ValueError:
        return ()

installed = version_tuple(sys.argv[1])
minimum = version_tuple(sys.argv[2])
raise SystemExit(0 if installed and installed >= minimum else 1)
PY
    then
        echo "Installing wasm-component-ld ${WASM_COMPONENT_LD_INSTALL_VERSION}..."
        cargo install wasm-component-ld \
            --version "$WASM_COMPONENT_LD_INSTALL_VERSION" \
            --locked
    fi

    # ── Step 0.5: Pack .actr with actr build ──────────────────────────

    echo ""
    echo -e "${BLUE}📦 Step 0.5: Building signed Component .actr package via actr build...${NC}"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    ACTR_PACKAGE="$ECHO_ACTR_DIR/dist/actrium-EchoService-${ECHO_ACTR_VERSION}-${ECHO_ACTR_TARGET}.actr"

    cargo run --manifest-path "$ACTR_CLI_MANIFEST" --bin actr --features wasm-engine -- build \
        --manifest-path "$ECHO_ACTR_BUILD_MANIFEST" \
        --key "$SIGNING_KEY" \
        --target "$ECHO_ACTR_TARGET" \
        --output "$ACTR_PACKAGE"

    if [ ! -f "$ACTR_PACKAGE" ]; then
        echo -e "${RED}❌ actr build failed: $ACTR_PACKAGE not found${NC}"
        exit 1
    fi
    echo -e "${GREEN}✅ .actr package built: $(du -h "$ACTR_PACKAGE" | cut -f1)${NC}"
fi

# Verify the package signature
cargo run --manifest-path "$ACTR_CLI_MANIFEST" --bin actr --features wasm-engine -- pkg verify \
    --pubkey "$PUBLIC_KEY_PATH" \
    --package "$ACTR_PACKAGE" >/dev/null
echo -e "${GREEN}✅ Package signature verified${NC}"

# ── Step 0.5: Build client-guest cdylib package ──────────────────────────

echo ""
echo -e "${BLUE}📦 Building client-guest cdylib package...${NC}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

CLIENT_GUEST_PUBLISHER="acme"
CLIENT_GUEST_NAME="package-echo-client-guest"
CLIENT_GUEST_VERSION="0.1.0"
CLIENT_GUEST_TARGET="$(rustc -vV | awk '/host:/ {print $2}')"
CLIENT_GUEST_DIST_DIR="$CLIENT_GUEST_DIR/dist"
mkdir -p "$CLIENT_GUEST_DIST_DIR"

# Package from disposable config copies so the tracked example remains clean
# when ECHO_ACTR_VERSION differs from its default. The v2 lock and manifest
# must carry the same canonical ActrType.
CLIENT_GUEST_BUILD_CONFIG_DIR="$CLIENT_GUEST_DIST_DIR/package-config"
mkdir -p "$CLIENT_GUEST_BUILD_CONFIG_DIR"
cp "$CLIENT_GUEST_DIR/manifest.toml" "$CLIENT_GUEST_BUILD_CONFIG_DIR/manifest.toml"
cp "$CLIENT_GUEST_DIR/manifest.lock.toml" "$CLIENT_GUEST_BUILD_CONFIG_DIR/manifest.lock.toml"
perl -0pi -e "s/actr_type = \"actrium:EchoService:[^\"]+\"/actr_type = \"actrium:EchoService:${ECHO_ACTR_VERSION}\"/g" \
    "$CLIENT_GUEST_BUILD_CONFIG_DIR/manifest.toml" \
    "$CLIENT_GUEST_BUILD_CONFIG_DIR/manifest.lock.toml"

# Resolve Cargo's effective target directory instead of duplicating the
# relative `.cargo/config.toml` calculation here.
WORKSPACE_TARGET_DIR="$(cargo metadata \
    --manifest-path "$CLIENT_GUEST_DIR/Cargo.toml" \
    --no-deps \
    --format-version 1 \
    | jq -r '.target_directory')"
if ! cargo build --manifest-path "$CLIENT_GUEST_DIR/Cargo.toml" 2>&1; then
    echo -e "${RED}❌ Failed to build client-guest cdylib${NC}"
    exit 1
fi

# Locate the built dylib/so (workspace target dir)
if [ "$(uname)" = "Darwin" ]; then
    CLIENT_GUEST_BINARY="$WORKSPACE_TARGET_DIR/debug/libpackage_echo_client_guest.dylib"
elif [ "$(uname)" = "Linux" ]; then
    CLIENT_GUEST_BINARY="$WORKSPACE_TARGET_DIR/debug/libpackage_echo_client_guest.so"
else
    CLIENT_GUEST_BINARY="$WORKSPACE_TARGET_DIR/debug/package_echo_client_guest.dll"
fi

if [ ! -f "$CLIENT_GUEST_BINARY" ]; then
    echo -e "${RED}❌ client-guest binary not found: $CLIENT_GUEST_BINARY${NC}"
    exit 1
fi

cat >> "$CLIENT_GUEST_BUILD_CONFIG_DIR/manifest.toml" << TOML

[binary]
path = "$CLIENT_GUEST_BINARY"
target = "$CLIENT_GUEST_TARGET"
TOML

CLIENT_GUEST_DEV_KEY="$PACKAGE_ECHO_DIR/dev-key.json"
CLIENT_GUEST_PACKAGE_NAME="${CLIENT_GUEST_PUBLISHER}-${CLIENT_GUEST_NAME}-${CLIENT_GUEST_VERSION}-cdylib.actr"
CLIENT_GUEST_PACKAGE="$CLIENT_GUEST_DIST_DIR/$CLIENT_GUEST_PACKAGE_NAME"
CLIENT_GUEST_PUBLIC_KEY="$CLIENT_GUEST_DIST_DIR/public-key.json"

# Package the already-built client workload through the unified build command.
cargo run --manifest-path "$ACTR_CLI_MANIFEST" --bin actr --features wasm-engine -- build \
    --manifest-path "$CLIENT_GUEST_BUILD_CONFIG_DIR/manifest.toml" \
    --key "$CLIENT_GUEST_DEV_KEY" \
    --target "$CLIENT_GUEST_TARGET" \
    --no-compile \
    --output "$CLIENT_GUEST_PACKAGE"

if [ ! -f "$CLIENT_GUEST_PACKAGE" ]; then
    echo -e "${RED}❌ client-guest package build failed: $CLIENT_GUEST_PACKAGE not found${NC}"
    exit 1
fi

# Extract public key from dev-key.json for client-guest
jq '{public_key: .public_key}' "$CLIENT_GUEST_DEV_KEY" > "$CLIENT_GUEST_PUBLIC_KEY"

echo -e "${GREEN}✅ client-guest package ready: $(du -h "$CLIENT_GUEST_PACKAGE" | cut -f1)${NC}"

# Verify client-guest package
cargo run --manifest-path "$ACTR_CLI_MANIFEST" --bin actr --features wasm-engine -- pkg verify \
    --pubkey "$CLIENT_GUEST_PUBLIC_KEY" \
    --package "$CLIENT_GUEST_PACKAGE" >/dev/null

echo -e "${GREEN}✅ client-guest package verified${NC}"

# ── Step 1: Ensure loran is available ──────────────────────────────────

echo ""
echo "📦 Checking Loran availability..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

LORAN_CMD=""
# Always build from source when LORAN_DIR is available to ensure latest migrations
if [ -n "$LORAN_DIR" ] && [ -d "$LORAN_DIR" ]; then
    echo -e "${BLUE}🔧 Building Loran from source ($LORAN_DIR)...${NC}"
    CARGO_TARGET_DIR="$LORAN_DIR/target" cargo build --manifest-path "$LORAN_DIR/Cargo.toml" --bin loran 2>&1 | tail -5
    if [ -x "$LORAN_DIR/target/debug/loran" ]; then
        LORAN_CMD="$LORAN_DIR/target/debug/loran"
        echo -e "${GREEN}✅ Loran built from source: $LORAN_CMD${NC}"
    else
        echo -e "${RED}❌ Failed to build Loran from source${NC}"
        exit 1
    fi
elif command -v loran > /dev/null 2>&1; then
    LORAN_CMD="loran"
    echo -e "${GREEN}✅ Using Loran from PATH: $(which loran)${NC}"
else
    echo -e "${RED}❌ Loran not available. Set LORAN_DIR to this monorepo or install its server binary${NC}"
    exit 1
fi

# ── Step 2: Start loran ────────────────────────────────────────────────

echo ""
echo "🚀 Starting Loran (Actor infrastructure server)..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Never stop an unrelated service. Use LORAN_HTTP_PORT/LORAN_ICE_PORT to
# select another pair when either requested port is occupied.
if lsof -tiTCP:${LORAN_HTTP_PORT} -sTCP:LISTEN >/dev/null 2>&1; then
    echo -e "${RED}❌ ${LORAN_HTTP_PORT}/tcp is already in use; set LORAN_HTTP_PORT to a free port${NC}"
    exit 1
fi
if lsof -tiUDP:${LORAN_ICE_PORT} >/dev/null 2>&1; then
    echo -e "${RED}❌ ${LORAN_ICE_PORT}/udp is already in use; set LORAN_ICE_PORT to a free port${NC}"
    exit 1
fi

$LORAN_CMD --config "$LORAN_CONFIG" > "$LOG_DIR/loran.log" 2>&1 &
LORAN_PID=$!

echo "Loran started (PID: $LORAN_PID)"
echo "Waiting for Loran to be ready..."

loran_http_ready() {
    lsof -nP -iTCP:${LORAN_HTTP_PORT} -sTCP:LISTEN > /dev/null 2>&1 \
        || nc -z localhost ${LORAN_HTTP_PORT} 2>/dev/null
}

loran_ice_ready() {
    lsof -nP -iUDP:${LORAN_ICE_PORT} > /dev/null 2>&1
}

# Wait for HTTP first (hard requirement)
MAX_WAIT=20
COUNTER=0
while [ $COUNTER -lt $MAX_WAIT ]; do
    if ! kill -0 $LORAN_PID 2>/dev/null; then
        echo -e "${RED}❌ Loran failed to start${NC}"
        cat "$LOG_DIR/loran.log"
        exit 1
    fi

    if loran_http_ready; then
        echo -e "${GREEN}✅ Loran HTTP is ready on port ${LORAN_HTTP_PORT}/tcp${NC}"
        break
    fi

    sleep 1
    COUNTER=$((COUNTER + 1))
done

if [ $COUNTER -eq $MAX_WAIT ]; then
    echo -e "${RED}❌ Loran not listening on ${LORAN_HTTP_PORT}/tcp after ${MAX_WAIT} seconds${NC}"
    cat "$LOG_DIR/loran.log"
    exit 1
fi

# Wait for ICE/TURN (soft check — give it up to 5 more seconds)
ICE_COUNTER=0
while [ $ICE_COUNTER -lt 5 ]; do
    if loran_ice_ready 2>/dev/null; then
        echo -e "${GREEN}✅ Loran ICE/TURN ready on ${LORAN_ICE_PORT}/udp${NC}"
        break
    fi
    sleep 1 || true
    ICE_COUNTER=$((ICE_COUNTER + 1))
done
if [ $ICE_COUNTER -eq 5 ]; then
    echo -e "${YELLOW}⚠️  ${LORAN_ICE_PORT}/udp not detected (may be using host ICE); continuing...${NC}" || true
fi

# ── Step 2.5: Setup realms ──────────────────────────────────────────────

echo ""
echo "🔑 Setting up realms in loran..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Poll until the Issuer reaches the Signer, which signs every credential with
# the Realm's issuer key. The signer gRPC service may take a few seconds to
# initialize after loran starts.
MAX_SIGNER_WAIT=60
SIGNER_COUNTER=0
echo "Waiting for the loran Signer (up to ${MAX_SIGNER_WAIT}s)..."
while [ $SIGNER_COUNTER -lt $MAX_SIGNER_WAIT ]; do
    HEALTH_JSON=$(curl -sf "${LORAN_HTTP_URL}/issuer/health" 2>/dev/null || true)
    if [ "$(echo "$HEALTH_JSON" | jq -r '.signer_service // "missing"' 2>/dev/null)" = "ok" ]; then
        echo -e "${GREEN}✅ Loran Signer ready${NC}"
        break
    fi
    sleep 1
    SIGNER_COUNTER=$((SIGNER_COUNTER + 1))
done
if [ $SIGNER_COUNTER -eq $MAX_SIGNER_WAIT ]; then
    echo -e "${RED}❌ Signer not ready after ${MAX_SIGNER_WAIT}s${NC}"
    grep -aEn "Signer|Authentication failed" "$LOG_DIR/loran.log" || true
    exit 1
fi

# Extract Realm names from actr.toml (runtime config) files
SERVER_REALM=package-echo@example.com
CLIENT_REALM=$(grep -E '^\s*realm\s*=' "$CLIENT_DIR/actr.toml" | head -1 | sed 's/.*=\s*//' | tr -d ' "')

# Insert realms directly into SQLite (same approach as loran fullstack tests)
LORAN_DB="$WORKSPACE_ROOT/database/loran.db"

if [ ! -f "$LORAN_DB" ]; then
    echo -e "${RED}❌ Loran database not found at $LORAN_DB${NC}"
    echo "Loran may not have started properly."
    exit 1
fi

for REALM in $SERVER_REALM $CLIENT_REALM; do
    echo "  Creating realm $REALM..."
    sqlite3 "$LORAN_DB" \
        "INSERT OR IGNORE INTO realm (name, status, enabled, created_at) VALUES ('$REALM', 'active', 1, strftime('%s','now'));"
done

echo -e "${GREEN}✅ Realms setup completed (realm IDs: $SERVER_REALM, $CLIENT_REALM)${NC}"

# Join the Realm: the server host as the owner, the client hosts as editors.
# Each actr.toml names its key file (`deployment.realm_client_key`).
ACTR_CLI_BIN="$WORKSPACE_TARGET_DIR/debug/actr"
if [ ! -x "$ACTR_CLI_BIN" ]; then
    echo "Building actr CLI..."
    cargo build --manifest-path "$ACTR_CLI_MANIFEST" --bin actr --features wasm-engine 2>&1 | tail -3
fi
SERVER_REALM_CLIENT_KEY="$LOG_DIR/server-realm-client.json"
ACTR_CMD="$ACTR_CLI_BIN" "$WORKSPACE_ROOT/scripts/join-realm.sh" \
    "$LORAN_DB" "$SERVER_REALM" "${LORAN_HTTP_URL}/issuer" "$SERVER_REALM_CLIENT_KEY" \
    "$CLIENT_DIR/realm-client.json" "$CLIENT_GUEST_DIR/realm-client.json"
echo -e "${GREEN}✅ Joined Realm $SERVER_REALM${NC}"

# ── Step 2.6: Register publisher identity ────────────────────────

echo ""
echo "🏷️  Registering publisher 'actrium' with public key..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

NOW=$(date +%s)
EXPIRES_AT=$((NOW + 86400 * 365))
PUBLISHER_PUBKEY=$(jq -r '.public_key' "$PUBLIC_KEY_PATH")

if [ -z "$PUBLISHER_PUBKEY" ]; then
    echo -e "${RED}❌ Failed to extract public_key from $PUBLIC_KEY_PATH${NC}"
    exit 1
fi

PUBLISHER_KEY_ID=$(printf '%s' "$PUBLISHER_PUBKEY" | base64 -d | openssl dgst -sha256 -r | awk '{print "publisher-" substr($1, 1, 16)}')

if [ -z "$PUBLISHER_KEY_ID" ]; then
    echo -e "${RED}❌ Failed to compute key_id from $PUBLIC_KEY_PATH${NC}"
    exit 1
fi

sqlite3 "$LORAN_DB" << SQL
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

echo -e "${GREEN}✅ publisher 'actrium' registered (key_id: $PUBLISHER_KEY_ID, pubkey: ${PUBLISHER_PUBKEY:0:20}...)${NC}"

# ── Step 2.7: Seed client-guest Publisher key ────────────────────

echo ""
echo "🏷️  Seeding client-guest Publisher key..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

python3 - \
    "$LORAN_DB" \
    "$CLIENT_GUEST_PUBLIC_KEY" \
    "$CLIENT_GUEST_PUBLISHER" \
    <<'PY'
import base64
import hashlib
import json
import sqlite3
import sys
import time

db_path, public_key_path, publisher = sys.argv[1:]
now = int(time.time())
key_expires_at = now + 365 * 24 * 3600

with open(public_key_path, "r", encoding="utf-8") as fh:
    public_key = json.load(fh)["public_key"]
key_id = "publisher-" + hashlib.sha256(base64.b64decode(public_key)).hexdigest()[:16]

conn = sqlite3.connect(db_path)
try:
    cur = conn.cursor()
    cur.execute(
        """
        INSERT OR IGNORE INTO publisher
            (name, public_key, key_id, contact, status, created_at, updated_at, verified_at, key_expires_at)
        VALUES (?, ?, ?, ?, 'active', ?, ?, ?, ?)
        """,
        (publisher, public_key, key_id, "examples@actrium.local", now, now, now, key_expires_at),
    )
    cur.execute(
        """
        UPDATE publisher
           SET public_key = ?,
               key_id = ?,
               status = 'active',
               updated_at = ?,
               verified_at = ?,
               key_expires_at = ?,
               suspended_at = NULL,
               revoked_at = NULL
         WHERE name = ?
        """,
        (public_key, key_id, now, now, key_expires_at, publisher),
    )
    conn.commit()
finally:
    conn.close()

print(f"Seeded Publisher key for {publisher}")
PY

echo -e "${GREEN}✅ Publisher key seeded for client-guest package${NC}"

# ── Step 3: Build host binaries ──────────────────────────────────────────

echo ""
echo "🔨 Building host binaries..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

if ! cargo build --bin package-echo-client 2>&1; then
    echo -e "${RED}❌ Failed to build binaries${NC}"
    exit 1
fi

echo -e "${GREEN}✅ Binaries built successfully${NC}"

# ── Step 4: Start package-backed echo server via actr run ─────────────────

echo ""
echo "🚀 Starting package-echo-server via actr run..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Generate server actr.toml with the actual package path and trust mode
SERVER_ACTR_CONFIG="${SERVER_ACTR_CONFIG:-$LOG_DIR/server-actr.toml}"
cat > "$SERVER_ACTR_CONFIG" << TOML
edition = 1

[package]
path = "$ACTR_PACKAGE"

[signaling]
url = "${LORAN_WS_URL}/signaling/ws"

[issuer_endpoint]
url = "${LORAN_HTTP_URL}/issuer"

[deployment]
realm = "package-echo@example.com"
realm_client_key = "$SERVER_REALM_CLIENT_KEY"

[[trust]]
kind = "registry"
endpoint = "${LORAN_HTTP_URL}/issuer"

[discovery]
visible = true

[observability]
filter_level = "info"
tracing_enabled = false
tracing_endpoint = "http://localhost:4317"
tracing_service_name = "package-echo-server"

[webrtc]
force_relay = false
stun_urls = ["stun:localhost:${LORAN_ICE_PORT}"]
turn_urls = ["turn:localhost:${LORAN_ICE_PORT}"]

[acl]

[[acl.rules]]
permission = "allow"
type = "acme/package-echo-client-guest@0.1.0"
TOML

ACTR_CLI_BIN="$WORKSPACE_TARGET_DIR/debug/actr"
if [ ! -x "$ACTR_CLI_BIN" ]; then
    echo "Building actr CLI..."
    cargo build --manifest-path "$ACTR_CLI_MANIFEST" --bin actr --features wasm-engine 2>&1 | tail -3
fi

(
    # `actr run` resolves `.actr/config.toml` from its working directory.
    # Run from the package project so a stale user-global keychain cannot
    # override this demo's publisher signing key.
    cd "$ECHO_ACTR_DIR"
    RUST_LOG="${RUST_LOG:-info}" \
        "$ACTR_CLI_BIN" run -c "$SERVER_ACTR_CONFIG"
) > "$LOG_DIR/package-echo-server.log" 2>&1 &
SERVER_PID=$!

echo "Server started (PID: $SERVER_PID)"
echo "Waiting for package-backed server to register..."

MAX_WAIT=30
COUNTER=0
while [ $COUNTER -lt $MAX_WAIT ]; do
    if ! kill -0 $SERVER_PID 2>/dev/null; then
        echo -e "${RED}❌ Package-backed server failed to start${NC}"
        cat "$LOG_DIR/package-echo-server.log"
        exit 1
    fi

    if grep -q "Echo Host fully started\|ActrNode started" "$LOG_DIR/package-echo-server.log" 2>/dev/null; then
        echo -e "${GREEN}✅ Package-backed server is running and registered${NC}"
        break
    fi

    sleep 1
    COUNTER=$((COUNTER + 1))
done

if [ $COUNTER -eq $MAX_WAIT ]; then
    echo -e "${YELLOW}⚠️  Server may not have fully registered, but continuing...${NC}"
fi

sleep 2

# ── Step 5: Run client with test input ───────────────────────────────────

echo ""
echo "🚀 Running package-echo-client..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

echo "Sending test message: \"$TEST_INPUT\""

if [ "$CLIENT_MODE" = "http" ]; then
    cargo build --bin package-echo-client 2>&1 | tail -3
    CLIENT_BIN="$WORKSPACE_TARGET_DIR/debug/package-echo-client"
    ECHO_HTTP_LISTEN="$HTTP_LISTEN" \
        ECHO_ACTR_VERSION="$ECHO_ACTR_VERSION" \
        CLIENT_GUEST_PACKAGE_PATH="$CLIENT_GUEST_PACKAGE" \
        CLIENT_GUEST_PUBLIC_KEY_PATH="$CLIENT_GUEST_PUBLIC_KEY" \
        RUST_LOG="${RUST_LOG:-info}" \
        "$CLIENT_BIN" > "$LOG_DIR/package-echo-client.log" 2>&1 &
    CLIENT_PID=$!

    GATEWAY_URL="http://${HTTP_LISTEN}"
    GATEWAY_TIMEOUT_SECONDS="${GATEWAY_TIMEOUT_SECONDS:-60}"
    COUNTER=0
    until curl --silent --fail "$GATEWAY_URL/health" > "$LOG_DIR/package-echo-health.json"; do
        if ! kill -0 "$CLIENT_PID" 2>/dev/null; then
            echo -e "${RED}❌ Actrium Actor gateway failed to start${NC}"
            cat "$LOG_DIR/package-echo-client.log"
            exit 1
        fi
        if [ "$COUNTER" -ge "$GATEWAY_TIMEOUT_SECONDS" ]; then
            echo -e "${RED}❌ Timed out waiting for Actrium Actor gateway${NC}"
            cat "$LOG_DIR/package-echo-client.log"
            exit 1
        fi
        sleep 1
        COUNTER=$((COUNTER + 1))
    done
    echo -e "${GREEN}✅ Actrium Actor gateway is ready at $GATEWAY_URL${NC}"

    REQUEST_JSON="$(python3 - "$TEST_INPUT" <<'PY'
import base64
import json
import sys


def varint(value):
    result = bytearray()
    while True:
        byte = value & 0x7f
        value >>= 7
        result.append(byte | (0x80 if value else 0))
        if not value:
            return result


message = sys.argv[1].encode("utf-8")
payload = bytes([0x0a]) + varint(len(message)) + message
print(json.dumps({
    "routeKey": "echo.EchoService.Echo",
    "payloadBase64": base64.b64encode(payload).decode("ascii"),
    "timeoutMs": 30000,
}))
PY
)"

    # A cold WebRTC host can occasionally time out before its first ICE pair
    # is available. Retry the complete Actor call, while keeping each attempt
    # independently bounded by the request timeout above.
    ECHO_CALL_ATTEMPTS="${ECHO_CALL_ATTEMPTS:-3}"
    HTTP_STATUS="000"
    CALL_SUCCEEDED=0
    for ATTEMPT in $(seq 1 "$ECHO_CALL_ATTEMPTS"); do
        if HTTP_STATUS="$(curl --silent --show-error \
            --output "$LOG_DIR/package-echo-call.json" \
            --write-out '%{http_code}' \
            --header 'Content-Type: application/json' \
            --data "$REQUEST_JSON" \
            "$GATEWAY_URL/v1/call")" \
            && [ "$HTTP_STATUS" = "200" ] \
            && jq -e '.success == true' "$LOG_DIR/package-echo-call.json" >/dev/null; then
            CALL_SUCCEEDED=1
            break
        fi

        if [ "$ATTEMPT" -lt "$ECHO_CALL_ATTEMPTS" ]; then
            echo -e "${YELLOW}⚠️  Actor call attempt $ATTEMPT/$ECHO_CALL_ATTEMPTS failed; retrying...${NC}"
            sleep 2
        fi
    done

    if [ "$CALL_SUCCEEDED" != "1" ]; then
        echo -e "${RED}❌ Actor gateway call failed after $ECHO_CALL_ATTEMPTS attempts (last HTTP $HTTP_STATUS)${NC}"
        cat "$LOG_DIR/package-echo-call.json"
        echo ""
        cat "$LOG_DIR/package-echo-client.log"
        exit 1
    fi

    RESPONSE_PAYLOAD="$(jq -r '.payloadBase64' "$LOG_DIR/package-echo-call.json")"
    RECEIVED_REPLY="$(python3 - "$RESPONSE_PAYLOAD" <<'PY'
import base64
import sys


def varint(data, cursor):
    value = 0
    shift = 0
    while cursor < len(data):
        byte = data[cursor]
        cursor += 1
        value |= (byte & 0x7f) << shift
        if byte & 0x80 == 0:
            return value, cursor
        shift += 7
    raise ValueError("invalid protobuf varint")


data = base64.b64decode(sys.argv[1])
cursor = 0
reply = ""
while cursor < len(data):
    tag, cursor = varint(data, cursor)
    field = tag >> 3
    wire = tag & 7
    if wire == 0:
        _, cursor = varint(data, cursor)
    elif wire == 2:
        length, cursor = varint(data, cursor)
        value = data[cursor:cursor + length]
        cursor += length
        if field == 1:
            reply = value.decode("utf-8")
    else:
        raise ValueError(f"unsupported protobuf wire type: {wire}")
print(reply)
PY
)"
else
    (
        sleep 3
        echo "$TEST_INPUT"
        sleep 2
        echo "quit"
    ) | ECHO_ACTR_VERSION="$ECHO_ACTR_VERSION" \
        CLIENT_GUEST_PACKAGE_PATH="$CLIENT_GUEST_PACKAGE" \
        CLIENT_GUEST_PUBLIC_KEY_PATH="$CLIENT_GUEST_PUBLIC_KEY" \
        RUST_LOG="${RUST_LOG:-info}" \
        cargo run --bin package-echo-client > "$LOG_DIR/package-echo-client.log" 2>&1 &
    CLIENT_PID=$!

    # Wait long enough for the WebRTC connection factory to use its built-in retry path.
    # A single attempt can take 10s, and the factory may back off before retrying.
    CLIENT_TIMEOUT_SECONDS="${CLIENT_TIMEOUT_SECONDS:-40}"
    COUNTER=0
    while kill -0 "$CLIENT_PID" 2>/dev/null && [ "$COUNTER" -lt "$CLIENT_TIMEOUT_SECONDS" ]; do
        sleep 1
        COUNTER=$((COUNTER + 1))
    done

    if kill -0 "$CLIENT_PID" 2>/dev/null; then
        echo -e "${YELLOW}⚠️  Client still running after ${CLIENT_TIMEOUT_SECONDS} seconds, killing...${NC}"
        kill "$CLIENT_PID" 2>/dev/null || true
    fi
fi

# ── Step 6: Verify output ────────────────────────────────────────────────

echo ""
echo "🔍 Verifying output..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

if { [ "$CLIENT_MODE" = "http" ] && { [ "$RECEIVED_REPLY" = "Echo: $TEST_INPUT" ] || [ "$RECEIVED_REPLY" = "$TEST_INPUT" ]; }; } \
    || grep -q "\[Received reply\].*Echo: $TEST_INPUT" "$LOG_DIR/package-echo-client.log" \
    || grep -q "\[Received reply\] $TEST_INPUT" "$LOG_DIR/package-echo-client.log"; then
    echo -e "${GREEN}✅ Test PASSED: package-backed echo server response received${NC}"
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "🎉 Echo package test completed successfully!"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
    echo "✅ Validated:"
    echo "   1. $BACKEND artifact built from $ECHO_ACTR_DIR"
    echo "   2. signed .actr package created (key: $SIGNING_KEY)"
    echo "   3. Hyper (TRUST_MODE=production) fetched pubkey from publisher, verified .actr"
    echo "   4. Issuer verified the package signature and issued a credential"
    echo "   5. Client ↔ server echo confirmed via WebRTC"
    echo ""
    echo "Client output:"
    grep "Received reply" "$LOG_DIR/package-echo-client.log" || true
    echo ""
    echo "📖 View full logs:"
    echo "   cat $LOG_DIR/package-echo-client.log  # Client logs"
    echo "   cat $LOG_DIR/package-echo-server.log  # Server logs"
    echo "   tail -f $LOG_DIR/loran.log           # Loran logs"
    echo ""
    if [ "$CLIENT_MODE" = "http" ]; then
        echo "Gateway response: $RECEIVED_REPLY"
        if [ "$KEEP_RUNNING" = "1" ]; then
            echo ""
            echo -e "${GREEN}✅ Echo stack is ready for Actrium at $GATEWAY_URL${NC}"
            echo "Press Ctrl-C to stop Loran, the echo server, and the client Host."
            while kill -0 "$LORAN_PID" 2>/dev/null \
                && kill -0 "$SERVER_PID" 2>/dev/null \
                && kill -0 "$CLIENT_PID" 2>/dev/null; do
                sleep 1
            done
            echo -e "${RED}❌ An echo stack process exited unexpectedly${NC}"
            exit 1
        fi
    fi
    exit 0
else
    echo -e "${RED}❌ Test FAILED: Expected package-backed echo server response not found${NC}"
    echo -e "${RED}   Looking for either: [Received reply] Echo: $TEST_INPUT${NC}"
    echo -e "${RED}                    or [Received reply] $TEST_INPUT${NC}"
    echo ""
    echo "Client output:"
    cat "$LOG_DIR/package-echo-client.log"
    echo ""
    echo "Server output (last 30 lines):"
    tail -30 "$LOG_DIR/package-echo-server.log"
    echo ""
    echo "Loran output (last 30 lines):"
    tail -30 "$LOG_DIR/loran.log"
    exit 1
fi
