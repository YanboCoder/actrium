#!/usr/bin/env bash
# Note: not using set -euo pipefail; we check errors explicitly throughout

# Color output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Directories
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ACTOR_RTC_DIR="$(cd "$WORKSPACE_ROOT/../../.." && pwd)"
LORAN_DIR="$ACTOR_RTC_DIR"
# Try multiple config locations
LORAN_CONFIG=""
for candidate in "$SCRIPT_DIR/loran-config.toml" "$WORKSPACE_ROOT/loran-config.toml" "$LORAN_DIR/loran/config/loran.example.toml"; do
    if [ -f "$candidate" ]; then
        LORAN_CONFIG="$candidate"
        break
    fi
done
if [ -z "$LORAN_CONFIG" ]; then
    if [ -f "$LORAN_DIR/loran/config/loran.example.toml" ]; then
        LORAN_CONFIG="$WORKSPACE_ROOT/loran-config.toml"
        cp "$LORAN_DIR/loran/config/loran.example.toml" "$LORAN_CONFIG"
    fi
fi
ACL_DEMO_DIR="$WORKSPACE_ROOT/acl"
SERVER_DIR="$ACL_DEMO_DIR/server"
ALLOWED_CLIENT_DIR="$ACL_DEMO_DIR/allowed-client"
BLOCKED_CLIENT_DIR="$ACL_DEMO_DIR/blocked-client"
PROTO_DIR="$ACL_DEMO_DIR/proto"

cd "$WORKSPACE_ROOT"

LOG_DIR="$WORKSPACE_ROOT/logs"
mkdir -p "$LOG_DIR"

source "$WORKSPACE_ROOT/scripts/ensure-tools.sh"
source "$WORKSPACE_ROOT/scripts/ensure-config-toml.sh"

echo ""
echo "🔍 Checking actr.toml files..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
ensure_actr_toml "$SERVER_DIR"
ensure_actr_toml "$ALLOWED_CLIENT_DIR"
ensure_actr_toml "$BLOCKED_CLIENT_DIR"
ensure_loran_config "$WORKSPACE_ROOT"

echo ""
echo "🧰 Checking required CLI tools..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
ensure_cargo_bin "protoc-gen-prost" "protoc-gen-prost" "$LOG_DIR"
ensure_cargo_bin "protoc-gen-tonic" "protoc-gen-tonic" "$LOG_DIR"

# Step 1: Build and start loran
echo ""
echo "🚀 Step 1: Building and starting loran (signaling server)..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Check if loran directory exists
if [ ! -d "$LORAN_DIR" ]; then
    echo -e "${RED}❌ Cannot find loran directory at $LORAN_DIR${NC}"
    exit 1
fi

# Build loran
echo "Building loran from source..."
cd "$LORAN_DIR"
if ! cargo build --manifest-path "$LORAN_DIR/crates/loran-server/Cargo.toml" 2>&1 | tee "$LOG_DIR/loran-build.log" | tail -5; then
    echo -e "${RED}❌ Failed to build loran${NC}"
    exit 1
fi

# Check if build was successful
if [ ! -f "$LORAN_DIR/target/debug/loran" ]; then
    echo -e "${RED}❌ Failed to build loran${NC}"
    exit 1
fi

cd "$WORKSPACE_ROOT"
echo -e "${GREEN}✅ Loran built successfully${NC}"

# Start loran
echo "Starting loran..."
"$LORAN_DIR/target/debug/loran" --config "$LORAN_CONFIG" > "$LOG_DIR/loran.log" 2>&1 &
LORAN_PID=$!
echo "Loran started (PID: $LORAN_PID)"

# Wait for loran to be ready
MAX_WAIT=10
COUNTER=0
while [ $COUNTER -lt $MAX_WAIT ]; do
    if ! kill -0 $LORAN_PID 2>/dev/null; then
        echo -e "${RED}❌ Loran failed to start${NC}"
        cat "$LOG_DIR/loran.log"
        exit 1
    fi
    if curl -s http://127.0.0.1:7000/health > /dev/null 2>&1; then
        echo -e "${GREEN}✅ Loran is running${NC}"
        break
    fi
    sleep 1
    COUNTER=$((COUNTER+1))
done

if [ $COUNTER -eq $MAX_WAIT ]; then
    echo -e "${YELLOW}⚠️  Loran health check timeout, proceeding anyway...${NC}"
fi

function cleanup {
    echo ""
    echo "🧹 Cleaning up..."
    [ -n "${SERVER_PID:-}" ] && kill $SERVER_PID 2>/dev/null || true
    [ -n "${ALLOWED_CLIENT_PID:-}" ] && kill $ALLOWED_CLIENT_PID 2>/dev/null || true
    [ -n "${BLOCKED_CLIENT_PID:-}" ] && kill $BLOCKED_CLIENT_PID 2>/dev/null || true
    [ -n "${LORAN_PID:-}" ] && kill $LORAN_PID 2>/dev/null || true
}
trap cleanup EXIT

# Step 1.5: Setup realms in loran (sqlite3)
echo ""
echo "🔑 Setting up realms in loran (sqlite3)..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

sleep 2

SQLITE_PATH=$(grep -E '^sqlite_path' "$LORAN_CONFIG" | sed 's/.*= *"\(.*\)".*/\1/' | head -1 || true)
if [ -z "$SQLITE_PATH" ]; then SQLITE_PATH="database"; fi
case "$SQLITE_PATH" in
    /*) LORAN_DB="$SQLITE_PATH/loran.db" ;;
    *)  LORAN_DB="$WORKSPACE_ROOT/$SQLITE_PATH/loran.db" ;;
esac

if [ ! -f "$LORAN_DB" ]; then
    echo -e "${RED}❌ Loran database not found at $LORAN_DB${NC}"
    exit 1
fi

REALM=$(grep -E '^\s*realm\s*=' "$SERVER_DIR/actr.toml" | sed 's/.*=\s*//' | tr -d ' "' | head -1 || true)
if [ -z "$REALM" ]; then REALM=acl-demo@example.com; fi


sqlite3 "$LORAN_DB" <<EOF
INSERT OR IGNORE INTO realm (name, status, enabled, created_at) VALUES ('$REALM', 'active', 1, strftime('%s','now'));
DELETE FROM actoracl WHERE realm = '$REALM';
INSERT INTO actoracl (realm, source_realm, from_type, to_type, access)
VALUES ('$REALM', '$REALM', 'acme/allowed-greeter-client@1.0.0', 'acme/greeter.GreeterService@1.0.0', 1);
INSERT INTO actoracl (realm, source_realm, from_type, to_type, access)
VALUES ('$REALM', '$REALM', 'acme/greeter.GreeterService@1.0.0', 'acme/allowed-greeter-client@1.0.0', 1);
EOF

echo -e "${GREEN}✅ Realm and ACL setup completed${NC}"

# Join the Realm: each actr.toml names its key file (`realm_client_key`).
ACTR_CMD="${ACTR_GEN_CMD:-actr}" "$WORKSPACE_ROOT/scripts/join-realm.sh" \
    "$LORAN_DB" "$REALM" "http://localhost:8081/issuer" \
    "$SERVER_DIR/realm-client.json" "$ALLOWED_CLIENT_DIR/realm-client.json" "$BLOCKED_CLIENT_DIR/realm-client.json"
echo -e "${GREEN}✅ Joined Realm $REALM${NC}"


# Step 2: Generate code
echo ""
echo "🛠️  Generating code (actr gen)..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

ACTR_GEN_CMD=""
if command -v actr > /dev/null 2>&1; then
    ACTR_GEN_CMD="actr"
elif [ -x "$ACTOR_RTC_DIR/actr/target/debug/actr" ]; then
    ACTR_GEN_CMD="$ACTOR_RTC_DIR/actr/target/debug/actr"
elif [ -x "$ACTOR_RTC_DIR/actr/target/release/actr" ]; then
    ACTR_GEN_CMD="$ACTOR_RTC_DIR/actr/target/release/actr"
else
    echo -e "${RED}❌ actr generator not found${NC}"
    exit 1
fi

if [ ! -d "$PROTO_DIR" ]; then
    echo -e "${RED}❌ Proto directory not found at $PROTO_DIR${NC}"
    exit 1
fi

# Generate code for the service scaffold only
cd "$SERVER_DIR"
echo "Running actr deps install (server)..."
if ! $ACTR_GEN_CMD deps install > "$LOG_DIR/actr-deps-server.log" 2>&1; then
    echo -e "${RED}❌ actr deps install failed (server)${NC}"
    cat "$LOG_DIR/actr-deps-server.log"
    exit 1
fi
if ! $ACTR_GEN_CMD gen --input="$PROTO_DIR" --output=src/generated --clean > "$LOG_DIR/actr-gen-server.log" 2>&1; then
    echo -e "${RED}❌ Server code generation failed${NC}"
    cat "$LOG_DIR/actr-gen-server.log"
    exit 1
fi
cd "$WORKSPACE_ROOT"
echo -e "${GREEN}✅ Server code generated successfully${NC}"

# Step 3: Build server binary
echo ""
echo "🔨 Building server binary..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
cd "$WORKSPACE_ROOT"
if ! cargo build --bin acl-server > "$LOG_DIR/cargo-build-server.log" 2>&1; then
    echo -e "${RED}❌ Server build failed${NC}"
    tail -20 "$LOG_DIR/cargo-build-server.log"
    exit 1
fi
echo -e "${GREEN}✅ Server binary built successfully${NC}"

# Step 4: Start server
echo ""
echo "🚀 Starting ACL-protected server..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
cd "$SERVER_DIR"
RUST_LOG="${RUST_LOG:-info}" "$WORKSPACE_ROOT/target/debug/acl-server" > "$LOG_DIR/acl-server.log" 2>&1 &
SERVER_PID=$!
echo "Server started (PID: $SERVER_PID)"

MAX_WAIT=10
COUNTER=0
while [ $COUNTER -lt $MAX_WAIT ]; do
    if ! kill -0 $SERVER_PID 2>/dev/null; then
        echo -e "${RED}❌ Server failed to start${NC}"
        cat "$LOG_DIR/acl-server.log"
        exit 1
    fi
    if grep -q "ActrNode started\|Greeter Server is running" "$LOG_DIR/acl-server.log" 2>/dev/null; then
        echo -e "${GREEN}✅ Server is running${NC}"
        break
    fi
    sleep 1
    COUNTER=$((COUNTER + 1))
done

sleep 2

# Step 4.5: Install client dependencies (resolve from loran registry)
echo ""
echo "📦 Installing client dependencies (actr deps install)..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

for client_dir in "$ALLOWED_CLIENT_DIR" "$BLOCKED_CLIENT_DIR"; do
    cd "$client_dir"
    INSTALL_LOG="$LOG_DIR/actr-install-$(basename $client_dir).log"
    $ACTR_GEN_CMD deps install > "$INSTALL_LOG" 2>&1 || {
        echo -e "${YELLOW}⚠️  actr deps install returned non-zero for $(basename $client_dir), check log${NC}"
    }
done
echo -e "${GREEN}✅ Client dependencies resolved${NC}"

# Step 4.6: Generate client code
echo ""
echo "🛠️  Generating client code (actr gen)..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

for client_dir in "$ALLOWED_CLIENT_DIR" "$BLOCKED_CLIENT_DIR"; do
    cd "$client_dir"
    if ! $ACTR_GEN_CMD gen --input="$PROTO_DIR" --output=src/generated --clean --no-scaffold > "$LOG_DIR/actr-gen-$(basename $client_dir).log" 2>&1; then
        echo -e "${RED}❌ $(basename $client_dir) code generation failed${NC}"
        cat "$LOG_DIR/actr-gen-$(basename $client_dir).log"
        exit 1
    fi
done
cd "$WORKSPACE_ROOT"
echo -e "${GREEN}✅ Client code generated successfully${NC}"

# Step 4.7: Build client binaries
echo ""
echo "🔨 Building client binaries..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
cd "$WORKSPACE_ROOT"
if ! cargo build --bin allowed-client --bin blocked-client > "$LOG_DIR/cargo-build-clients.log" 2>&1; then
    echo -e "${RED}❌ Client build failed${NC}"
    tail -20 "$LOG_DIR/cargo-build-clients.log"
    exit 1
fi
echo -e "${GREEN}✅ Client binaries built successfully${NC}"

# Step 5: Test allowed client
echo ""
echo "🧪 Testing ALLOWED client..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
cd "$ALLOWED_CLIENT_DIR"
RUST_LOG="${RUST_LOG:-info}" "$WORKSPACE_ROOT/target/debug/allowed-client" > "$LOG_DIR/allowed-client.log" 2>&1 &
ALLOWED_CLIENT_PID=$!

sleep 3
wait $ALLOWED_CLIENT_PID || true

# Step 6: Test blocked client
echo ""
echo "🧪 Testing BLOCKED client..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
cd "$BLOCKED_CLIENT_DIR"
RUST_LOG="${RUST_LOG:-info}" "$WORKSPACE_ROOT/target/debug/blocked-client" > "$LOG_DIR/blocked-client.log" 2>&1 &
BLOCKED_CLIENT_PID=$!

sleep 3
wait $BLOCKED_CLIENT_PID || true

# Step 7: Verify results
echo ""
echo "📊 Verifying ACL test results..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

ALLOWED_SUCCESS=false
BLOCKED_FAILED=false

if grep -q "ACL test PASSED - Allowed client can access server" "$LOG_DIR/allowed-client.log"; then
    ALLOWED_SUCCESS=true
    echo -e "${GREEN}✅ Allowed client test PASSED${NC}"
else
    echo -e "${RED}❌ Allowed client test FAILED${NC}"
fi

if grep -q "ACL test PASSED - Blocked client was correctly denied" "$LOG_DIR/blocked-client.log"; then
    BLOCKED_FAILED=true
    echo -e "${GREEN}✅ Blocked client test PASSED${NC}"
else
    echo -e "${RED}❌ Blocked client test FAILED${NC}"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

if [ "$ALLOWED_SUCCESS" = true ] && [ "$BLOCKED_FAILED" = true ]; then
    echo -e "${GREEN}🎉 ACL Demo Test PASSED!${NC}"
    echo ""
    echo "✅ Validated:"
    echo "   • ACL rules correctly allow 'allowed-greeter-client'"
    echo "   • ACL rules correctly block 'blocked-greeter-client'"
    echo "   • ACL configuration from actr.toml works"
    echo "   • Real distributed Actor communication with ACL"
    echo ""
else
    echo -e "${RED}❌ ACL Demo Test FAILED${NC}"
    echo ""
fi

echo "📖 View full logs:"
echo "   cat $LOG_DIR/acl-server.log         # Server logs"
echo "   cat $LOG_DIR/allowed-client.log     # Allowed client logs"
echo "   cat $LOG_DIR/blocked-client.log     # Blocked client logs"
echo "   tail -f $LOG_DIR/loran.log         # Loran logs"
echo ""

if [ "$ALLOWED_SUCCESS" = true ] && [ "$BLOCKED_FAILED" = true ]; then
    exit 0
else
    exit 1
fi
