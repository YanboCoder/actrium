#!/usr/bin/env bash
# register.sh — Seed the web echo Realm and Publisher public key
#               to a running loran instance.
# Usage: ./register.sh [--db <path>] [--endpoint <url>]
#
# Prerequisites:
#   1. loran running (signaling + Issuer + publisher) on port 8081
#   2. .actr packages already built in release/ (via start.sh Steps 1–2)
#   3. publisher signing key at release/dev-key.json (generated during build)

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ACTR_ROOT="$(cd "$PROJECT_ROOT/../.." && pwd)"

# ── Defaults ─────────────────────────────────────────────────────────────────

_detect_loran_db() {
    # First: ask the live process
    if command -v lsof >/dev/null 2>&1; then
        local live_db
        live_db=$(lsof -c loran 2>/dev/null | awk '/loran\.db$/ {print $NF; exit}')
        if [ -n "$live_db" ] && [ -f "$live_db" ]; then
            echo "$live_db"
            return
        fi
    fi

    # Fallback: check local dev db first, then candidates
    local candidates=(
        "$SCRIPT_DIR/loran-dev-db/loran.db"
        "$(cd "$ACTR_ROOT/.." && pwd)/loran/database/loran.db"
        "$(cd "$ACTR_ROOT/.." && pwd)/loran/loran-dev-db/loran.db"
    )
    local best="" best_mtime=0
    for db in "${candidates[@]}"; do
        if [ -f "$db" ] && sqlite3 "$db" ".tables" 2>/dev/null | grep -q "publisher"; then
            local mtime
            mtime=$(stat -f "%m" "$db" 2>/dev/null || stat -c "%Y" "$db" 2>/dev/null || echo 0)
            if [ "$mtime" -gt "$best_mtime" ]; then
                best_mtime="$mtime"
                best="$db"
            fi
        fi
    done
    echo "${best:-$SCRIPT_DIR/loran-dev-db/loran.db}"
}

LORAN_DB="${LORAN_DB:-$(_detect_loran_db)}"
ENDPOINT="${ENDPOINT:-http://localhost:8081}"
RELEASE_DIR="$SCRIPT_DIR/release"
PUBLISHER_KEY_FILE="$RELEASE_DIR/dev-key.json"
PUBLISHER_NAME="acme"

# Browser packages contain the authenticated wasm-bindgen binary and glue.
SERVER_ACTR_PACKAGE="$RELEASE_DIR/acme-EchoService-0.2.0-wasm32-unknown-unknown.actr"
CLIENT_ACTR_PACKAGE="$RELEASE_DIR/acme-echo-client-app-0.1.0-wasm32-unknown-unknown.actr"

# Config files
SERVER_ACTR_TOML="$SCRIPT_DIR/server-actr.toml"
CLIENT_ACTR_TOML="$SCRIPT_DIR/client-actr.toml"

# Detect actr CLI
ACTR_CMD=""
if [ -x "$ACTR_ROOT/target/debug/actr" ]; then
    ACTR_CMD="$ACTR_ROOT/target/debug/actr"
elif [ -x "$ACTR_ROOT/target/release/actr" ]; then
    ACTR_CMD="$ACTR_ROOT/target/release/actr"
elif command -v actr > /dev/null 2>&1; then
    ACTR_CMD="actr"
else
    echo -e "${RED}❌ actr CLI not found${NC}"
    exit 1
fi

# ── Argument parsing ──────────────────────────────────────────────────────────

while [[ $# -gt 0 ]]; do
    case "$1" in
        --db)       LORAN_DB="$2";  shift 2 ;;
        --endpoint) ENDPOINT="$2";   shift 2 ;;
        *) echo -e "${RED}Unknown argument: $1${NC}"; exit 1 ;;
    esac
done

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  actr register — Web Echo publisher setup"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  DB:       $LORAN_DB"
echo "  Endpoint: $ENDPOINT"
echo "  actr:     $ACTR_CMD"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# ── Preflight checks ──────────────────────────────────────────────────────────

if [ ! -f "$LORAN_DB" ]; then
    echo -e "${RED}❌ Loran database not found: $LORAN_DB${NC}"
    echo "   Make sure loran is running and has created its database."
    exit 1
fi

if [ ! -f "$PUBLISHER_KEY_FILE" ]; then
    echo -e "${RED}❌ publisher key not found: $PUBLISHER_KEY_FILE${NC}"
    echo "   Build packages first (./start.sh builds them)."
    exit 1
fi

if [ ! -f "$SERVER_ACTR_PACKAGE" ]; then
    echo -e "${RED}❌ Server package not found: $SERVER_ACTR_PACKAGE${NC}"
    exit 1
fi

if [ ! -f "$CLIENT_ACTR_PACKAGE" ]; then
    echo -e "${RED}❌ Client package not found: $CLIENT_ACTR_PACKAGE${NC}"
    exit 1
fi

if ! command -v jq >/dev/null 2>&1 && ! command -v python3 >/dev/null 2>&1; then
    echo -e "${RED}❌ jq or python3 is required${NC}"
    exit 1
fi

# Extract publisher public key
if command -v jq >/dev/null 2>&1; then
    PUBLISHER_PUBKEY=$(jq -r '.public_key' "$PUBLISHER_KEY_FILE")
else
    PUBLISHER_PUBKEY=$(python3 -c "import json; print(json.load(open('$PUBLISHER_KEY_FILE'))['public_key'])")
fi

if [ -z "$PUBLISHER_PUBKEY" ]; then
    echo -e "${RED}❌ Failed to extract public_key from $PUBLISHER_KEY_FILE${NC}"
    exit 1
fi

# ── Step 1: Create realm ──────────────────────────────────────────────────────

echo ""
echo "🔑 Step 1: Creating realms..."

SERVER_REALM=$(grep -E '^\s*realm\s*=' "$SERVER_ACTR_TOML" | head -1 | sed 's/.*=\s*//' | tr -d ' "')
CLIENT_REALM=$(grep -E '^\s*realm\s*=' "$CLIENT_ACTR_TOML" | head -1 | sed 's/.*=\s*//' | tr -d ' "')

NOW=$(date +%s)

for REALM in $SERVER_REALM $CLIENT_REALM; do
    sqlite3 "$LORAN_DB" \
        "INSERT OR IGNORE INTO realm (name, status, enabled, created_at) \
         VALUES ('$REALM', 'active', 1, $NOW);"
done

echo -e "${GREEN}✅ Realms ready: $SERVER_REALM, $CLIENT_REALM${NC}"

# ── Step 2: Register publisher ────────────────────────────────────────

echo ""
echo "🏷️  Step 2: Registering publisher '$PUBLISHER_NAME'..."

EXPIRES_AT=$((NOW + 86400 * 365))

sqlite3 "$LORAN_DB" << SQL
INSERT OR IGNORE INTO publisher
    (name, public_key, contact, status, created_at, verified_at, key_expires_at)
VALUES ('$PUBLISHER_NAME', '$PUBLISHER_PUBKEY', 'dev@example.com', 'active',
        $NOW, $NOW, $EXPIRES_AT);
UPDATE publisher
   SET public_key     = '$PUBLISHER_PUBKEY',
       status         = 'active',
       updated_at     = $NOW,
       verified_at    = $NOW,
       key_expires_at = $EXPIRES_AT,
       suspended_at   = NULL,
       revoked_at     = NULL
 WHERE name = '$PUBLISHER_NAME';
SQL

echo -e "${GREEN}✅ publisher '$PUBLISHER_NAME' registered${NC}"

# ── Step 3: Inject publisher pubkey into config files ──────────────────────────────

echo ""
echo "🔐 Step 3: Injecting publisher pubkey into config files..."

sed -i '' "s|__PUBLISHER_PUBKEY_PLACEHOLDER__|${PUBLISHER_PUBKEY}|g" "$SERVER_ACTR_TOML"
sed -i '' "s|__PUBLISHER_PUBKEY_PLACEHOLDER__|${PUBLISHER_PUBKEY}|g" "$CLIENT_ACTR_TOML"

echo -e "${GREEN}✅ publisher pubkey injected${NC}"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo -e "${GREEN}✅ Registration complete. You can now run:${NC}"
echo "   actr run --web -c server-actr.toml"
echo "   actr run --web -c client-actr.toml"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
