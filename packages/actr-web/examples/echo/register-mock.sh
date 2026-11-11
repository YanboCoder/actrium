#!/usr/bin/env bash
# register-mock.sh — Seed the web echo Realm and Publisher public key
#                    against a running `mock-loran` instance.
#
# Unlike register.sh, this does NOT touch SQLite. It drives the mock's
# `/admin/*` HTTP endpoints instead, which is closer to how a production-grade
# loran would expose management APIs and decouples the e2e flow from loran's
# sqlite schema.
#
# Usage: ./register-mock.sh [--endpoint URL]

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ACTR_ROOT="$(cd "$PROJECT_ROOT/../.." && pwd)"

ENDPOINT="${ENDPOINT:-http://127.0.0.1:8081}"
RELEASE_DIR="$SCRIPT_DIR/release"
PUBLISHER_KEY_FILE="$RELEASE_DIR/dev-key.json"
PUBLISHER_NAME="acme"

SERVER_ACTR_PACKAGE="$RELEASE_DIR/acme-EchoService-0.2.0-wasm32-unknown-unknown.actr"
CLIENT_ACTR_PACKAGE="$RELEASE_DIR/acme-echo-client-app-0.1.0-wasm32-unknown-unknown.actr"

SERVER_ACTR_TOML="$SCRIPT_DIR/server-actr.toml"
CLIENT_ACTR_TOML="$SCRIPT_DIR/client-actr.toml"

ACTR_CMD=""
if [ -x "$ACTR_ROOT/target/debug/actr" ]; then
    ACTR_CMD="$ACTR_ROOT/target/debug/actr"
elif [ -x "$ACTR_ROOT/target/release/actr" ]; then
    ACTR_CMD="$ACTR_ROOT/target/release/actr"
elif command -v actr > /dev/null 2>&1; then
    ACTR_CMD="actr"
else
    echo -e "${RED}actr CLI not found${NC}"
    exit 1
fi

# --- Argument parsing ---

while [[ $# -gt 0 ]]; do
    case "$1" in
        --endpoint) ENDPOINT="$2"; shift 2 ;;
        *) echo -e "${RED}Unknown argument: $1${NC}"; exit 1 ;;
    esac
done

# Portable in-place sed.
sed_inplace() {
    local expr="$1"; shift
    if sed --version >/dev/null 2>&1; then
        sed -i "$expr" "$@"
    else
        sed -i '' "$expr" "$@"
    fi
}

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  actr register-mock — Web Echo via mock-loran"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  Endpoint: $ENDPOINT"
echo "  actr:     $ACTR_CMD"

# --- Preflight ---

[ -f "$PUBLISHER_KEY_FILE" ] || { echo -e "${RED}Missing $PUBLISHER_KEY_FILE${NC}"; exit 1; }
[ -f "$SERVER_ACTR_PACKAGE" ] || { echo -e "${RED}Missing server package${NC}"; exit 1; }
[ -f "$CLIENT_ACTR_PACKAGE" ] || { echo -e "${RED}Missing client package${NC}"; exit 1; }

if command -v jq >/dev/null 2>&1; then
    PUBLISHER_PUBKEY=$(jq -r '.public_key' "$PUBLISHER_KEY_FILE")
else
    PUBLISHER_PUBKEY=$(python3 -c "import json; print(json.load(open('$PUBLISHER_KEY_FILE'))['public_key'])")
fi
[ -n "$PUBLISHER_PUBKEY" ] || { echo -e "${RED}Failed to read publisher pubkey${NC}"; exit 1; }

# --- Step 1: Create realms via /admin/realms ---

echo ""
echo "Step 1: Creating realms..."

SERVER_REALM=$(grep -E '^\s*realm\s*=' "$SERVER_ACTR_TOML" | head -1 | sed 's/.*=\s*//' | tr -d ' "')
CLIENT_REALM=$(grep -E '^\s*realm\s*=' "$CLIENT_ACTR_TOML" | head -1 | sed 's/.*=\s*//' | tr -d ' "')

for REALM in "$SERVER_REALM" "$CLIENT_REALM"; do
    curl -fsS -X POST "$ENDPOINT/admin/realms" \
        -H 'content-type: application/json' \
        --data "{\"realm\": \"$REALM\"}" >/dev/null
done

echo -e "${GREEN}realms ready: $SERVER_REALM, $CLIENT_REALM${NC}"

# --- Step 2: Register publisher via /admin/publisher ---

echo ""
echo "Step 2: Registering publisher '$PUBLISHER_NAME'..."

curl -fsS -X POST "$ENDPOINT/admin/publisher" \
    -H 'content-type: application/json' \
    --data "{\"name\": \"$PUBLISHER_NAME\", \"pubkey_b64\": \"$PUBLISHER_PUBKEY\", \"contact\": \"dev@example.com\"}" >/dev/null

echo -e "${GREEN}publisher '$PUBLISHER_NAME' registered${NC}"

# --- Step 3: Inject publisher pubkey into config files ---

echo ""
echo "Step 3: Injecting publisher pubkey into config files..."

sed_inplace "s|__PUBLISHER_PUBKEY_PLACEHOLDER__|${PUBLISHER_PUBKEY}|g" "$SERVER_ACTR_TOML"
sed_inplace "s|__PUBLISHER_PUBKEY_PLACEHOLDER__|${PUBLISHER_PUBKEY}|g" "$CLIENT_ACTR_TOML"

echo -e "${GREEN}publisher pubkey injected${NC}"

# --- Step 5: Sanity-check state ---

echo ""
echo "Step 5: mock-loran state snapshot:"
curl -fsS "$ENDPOINT/admin/state" | (jq . 2>/dev/null || cat)

echo ""
echo -e "${GREEN}Registration complete.${NC}"
