#!/usr/bin/env bash
# register.sh — Register the echo-actr Publisher public key to a running loran instance.
# Usage: ./register.sh [--db <path>] [--endpoint <url>] [--package <path>]
set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ACTR_REPO_DIR="$(cd "$WORKSPACE_ROOT/../../.." && pwd)"

# ── Defaults ─────────────────────────────────────────────────────────────────
# Auto-detect loran database: ask the running loran process which DB it has open.
# Falls back to checking known candidate paths by mtime if lsof is unavailable.
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

    # Fallback: pick the most recently modified candidate
    local candidates=(
        "$WORKSPACE_ROOT/database/loran.db"
        "$(cd "$WORKSPACE_ROOT/../.." && pwd)/database/loran.db"
        "$(cd "$WORKSPACE_ROOT/../../.." && pwd)/loran/database/loran.db"
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
    echo "${best:-$WORKSPACE_ROOT/database/loran.db}"
}
LORAN_DB="${LORAN_DB:-$(_detect_loran_db)}"
ENDPOINT="${ENDPOINT:-http://localhost:8081}"
ECHO_ACTR_DIR="$WORKSPACE_ROOT/echo-actr"
PUBLIC_KEY_PATH="$ECHO_ACTR_DIR/public-key.json"
SIGNING_KEY="$ECHO_ACTR_DIR/packaging/keys/dev-signing-key.json"
ACTR_CLI_MANIFEST="$ACTR_REPO_DIR/crates/actr-cli/Cargo.toml"

# Auto-detect package version from Cargo.toml
ECHO_ACTR_VERSION=$(awk '
    /^\[package\]/ { in_package = 1; next }
    /^\[/ && in_package { exit }
    in_package && $1 == "version" { gsub(/"/, "", $3); print $3; exit }
' "$ECHO_ACTR_DIR/Cargo.toml")

ACTR_PACKAGE="${ACTR_PACKAGE:-$ECHO_ACTR_DIR/dist/actrium-EchoService-${ECHO_ACTR_VERSION}-wasm32-wasip2.actr}"

# ── Argument parsing ──────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case "$1" in
        --db)       LORAN_DB="$2";  shift 2 ;;
        --endpoint) ENDPOINT="$2";   shift 2 ;;
        --package)  ACTR_PACKAGE="$2"; shift 2 ;;
        *) echo -e "${RED}Unknown argument: $1${NC}"; exit 1 ;;
    esac
done

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  actr register — publisher setup for package-echo"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  DB:       $LORAN_DB"
echo "  Endpoint: $ENDPOINT"
echo "  Package:  $ACTR_PACKAGE"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# ── Preflight checks ──────────────────────────────────────────────────────────
if [ ! -f "$LORAN_DB" ]; then
    echo -e "${RED}❌ Loran database not found: $LORAN_DB${NC}"
    echo "   Make sure loran is running and has created its database."
    exit 1
fi

if [ ! -f "$PUBLIC_KEY_PATH" ]; then
    echo -e "${RED}❌ Public key not found: $PUBLIC_KEY_PATH${NC}"
    exit 1
fi

if [ ! -f "$ACTR_PACKAGE" ]; then
    echo -e "${RED}❌ Package not found: $ACTR_PACKAGE${NC}"
    echo "   Build it first: cd $ECHO_ACTR_DIR && ./packaging/scripts/build-wasm.sh"
    exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
    echo -e "${RED}❌ jq is required but not found${NC}"
    exit 1
fi

# ── Step 1: Create realm ──────────────────────────────────────────────────────
echo ""
echo "🔑 Step 1: Creating realm package-echo@example.com..."

sqlite3 "$LORAN_DB" \
    "INSERT OR IGNORE INTO realm (name, status, enabled, created_at) \
     VALUES ('package-echo@example.com', 'active', 1, strftime('%s','now'));"

echo -e "${GREEN}✅ Realm package-echo@example.com ready${NC}"

# ── Step 2: Register publisher ────────────────────────────────────────
echo ""
echo "🏷️  Step 2: Registering publisher 'actrium'..."

NOW=$(date +%s)
EXPIRES_AT=$((NOW + 86400 * 365))
PUBLISHER_PUBKEY=$(jq -r '.public_key' "$PUBLIC_KEY_PATH")

if [ -z "$PUBLISHER_PUBKEY" ]; then
    echo -e "${RED}❌ Failed to extract public_key from $PUBLIC_KEY_PATH${NC}"
    exit 1
fi

PUBLISHER_KEY_ID=$(printf '%s' "$PUBLISHER_PUBKEY" | base64 -d | openssl dgst -sha256 -r | awk '{print "publisher-" substr($1, 1, 16)}')

if [ -z "$PUBLISHER_KEY_ID" ]; then
    echo -e "${RED}❌ Failed to compute key_id${NC}"
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

echo -e "${GREEN}✅ publisher 'actrium' registered (key_id: $PUBLISHER_KEY_ID)${NC}"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo -e "${GREEN}✅ Registration complete. You can now run:${NC}"
echo "   actr run -c server-actr.toml"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
