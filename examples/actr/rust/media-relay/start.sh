#!/bin/bash
# Start media-relay example - Using loran as signaling server
# Auto-starts loran, actr-b, and actr-a

# Note: not using set -e; we check errors explicitly throughout

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🎬 Media Relay Example - Using Loran"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Determine paths and switch to workspace root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ACTOR_RTC_DIR="$(cd "$WORKSPACE_ROOT/../../.." && pwd)"
ACTR_DIR="$ACTOR_RTC_DIR"
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
PROTO_DIR="$WORKSPACE_ROOT/media-relay/proto"

# Switch to workspace root and stay there
cd "$WORKSPACE_ROOT"

# Create logs directory
LOG_DIR="$WORKSPACE_ROOT/logs"
mkdir -p "$LOG_DIR"

# Ensure required CLI tools
source "$WORKSPACE_ROOT/scripts/ensure-tools.sh"

# Ensure actr.toml files exist
source "$WORKSPACE_ROOT/scripts/ensure-config-toml.sh"

# Ensure actr.toml files exist for actr-a and actr-b
echo ""
echo "🔍 Checking actr.toml files..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
ACTR_B_DIR="$WORKSPACE_ROOT/media-relay/actr-b"
ACTR_A_DIR="$WORKSPACE_ROOT/media-relay/actr-a"
ensure_actr_toml "$ACTR_B_DIR"
ensure_actr_toml "$ACTR_A_DIR"

# Ensure loran-config.toml exists
ensure_loran_config "$WORKSPACE_ROOT"

echo ""
echo "🧰 Checking required CLI tools..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
ensure_cargo_bin "protoc-gen-prost" "protoc-gen-prost" "$LOG_DIR"
ensure_cargo_bin "protoc-gen-actrframework" "actr-framework-protoc-codegen" "$LOG_DIR"
ensure_cargo_bin "actr" "actr-cli" "$LOG_DIR"

# Cleanup function
cleanup() {
    echo ""
    echo "🧹 Cleaning up..."

    # Kill loran (only if we started it)
    if [ ! -z "$LORAN_PID" ] && [ "$LORAN_PID" != "external" ]; then
        echo "Stopping loran (PID: $LORAN_PID)"
        kill $LORAN_PID 2>/dev/null || true
    fi

    # Kill actr-b
    if [ ! -z "$ACTR_B_PID" ]; then
        echo "Stopping actr-b (PID: $ACTR_B_PID)"
        kill $ACTR_B_PID 2>/dev/null || true
    fi

    # Kill actr-a
    if [ ! -z "$ACTR_A_PID" ]; then
        echo "Stopping actr-a (PID: $ACTR_A_PID)"
        kill $ACTR_A_PID 2>/dev/null || true
    fi

    wait 2>/dev/null || true

    echo "✅ Cleanup complete"
}

# Set trap to cleanup on exit
trap cleanup EXIT INT TERM

# Step 0: Generate code for actr-a and actr-b
echo ""
echo "🛠️ Generating code (actr gen)..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

ACTR_GEN_CMD=""

if command -v actr > /dev/null 2>&1; then
    ACTR_GEN_CMD="actr"
elif [ -x "$ACTR_DIR/target/debug/actr" ]; then
    ACTR_GEN_CMD="$ACTR_DIR/target/debug/actr"
elif [ -x "$ACTR_DIR/target/release/actr" ]; then
    ACTR_GEN_CMD="$ACTR_DIR/target/release/actr"
else
    echo -e "${RED}❌ actr generator not found (expected 'actr' in PATH or built under $ACTR_DIR)${NC}"
    exit 1
fi

if [ ! -d "$PROTO_DIR" ]; then
    echo -e "${RED}❌ Proto directory not found at $PROTO_DIR${NC}"
    exit 1
fi

# Generate code for actr-b (receiver/server) only
CRATE_DIR="$WORKSPACE_ROOT/media-relay/actr-b"
OUTPUT_FILE="$LOG_DIR/actr-gen-actr-b.log"
echo "🔧 Running actr gen for actr-b (receiver)..."
cd "$CRATE_DIR"
$ACTR_GEN_CMD gen --input="$PROTO_DIR" --output=src/generated --clean > "$OUTPUT_FILE" 2>&1 || {
    echo -e "${RED}❌ actr gen failed for actr-b${NC}"
    cat "$OUTPUT_FILE"
    exit 1
}
cd "$WORKSPACE_ROOT"
echo -e "${GREEN}✅ actr gen completed for actr-b (receiver)${NC}"

# Step 1: Check/Build loran
echo ""
echo "📦 Checking loran..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

LORAN_CMD=""

if command -v loran > /dev/null 2>&1; then
    LORAN_CMD="loran"
    echo "✅ Using installed loran command: $(which loran)"
elif [ -f "$LORAN_DIR/target/debug/loran" ]; then
    LORAN_CMD="$LORAN_DIR/target/debug/loran"
    echo "✅ Using local loran build: $LORAN_CMD"
elif [ -f "$LORAN_DIR/target/release/loran" ]; then
    LORAN_CMD="$LORAN_DIR/target/release/loran"
    echo "✅ Using local loran build: $LORAN_CMD"
elif [ -d "$LORAN_DIR" ]; then
    echo -e "${YELLOW}⚠️  loran not found, building from source...${NC}"
    cd "$LORAN_DIR"
    cargo build --manifest-path "$LORAN_DIR/crates/loran-server/Cargo.toml" 2>&1 | tail -5
    LORAN_CMD="$LORAN_DIR/target/debug/loran"
    echo "✅ Built loran: $LORAN_CMD"
    cd "$WORKSPACE_ROOT"
else
    echo -e "${RED}❌ Cannot find loran directory at $LORAN_DIR${NC}"
    echo "Please ensure loran project exists at: $LORAN_DIR"
    exit 1
fi

# Step 2: Start loran (or detect existing one)
echo ""
echo "🚀 Checking loran (signaling server)..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Check if port 8081 is already in use
if nc -z localhost 8081 2>/dev/null; then
    echo -e "${YELLOW}⚠️  Port 8081 is already in use${NC}"
    echo -e "${GREEN}✅ Using existing loran/signaling-server${NC}"
    LORAN_PID="external"
else
    echo "Starting loran..."
    $LORAN_CMD --config "$LORAN_CONFIG" > "$LOG_DIR/loran.log" 2>&1 &
    LORAN_PID=$!

    echo "Loran started (PID: $LORAN_PID)"
    echo "Waiting for loran to be ready..."
    sleep 3

    if ! kill -0 $LORAN_PID 2>/dev/null; then
        echo -e "${RED}❌ Loran failed to start${NC}"
        cat "$LOG_DIR/loran.log"
        exit 1
    fi

    echo -e "${GREEN}✅ Loran is running on port 8081${NC}"
fi

# Step 2.5a: Setup realms in loran (sqlite3)
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

REALM=$(grep -E '^realm\s*=' "$ACTR_B_DIR/actr.toml" | sed 's/.*= *//' | tr -d ' "' | head -1 || true)
if [ -z "$REALM" ]; then REALM=media-relay@example.com; fi


sqlite3 "$LORAN_DB" <<EOF
INSERT OR IGNORE INTO realm (name, status, enabled, created_at) VALUES ('$REALM', 'active', 1, strftime('%s','now'));
DELETE FROM actoracl WHERE realm = '$REALM';
INSERT INTO actoracl (realm, source_realm, from_type, to_type, access)
VALUES ('$REALM', '$REALM', 'actr-example/RelayClient@1.0.0', 'actr-example/RelayService@1.0.0', 1);
INSERT INTO actoracl (realm, source_realm, from_type, to_type, access)
VALUES ('$REALM', '$REALM', 'actr-example/RelayService@1.0.0', 'actr-example/RelayClient@1.0.0', 1);
EOF

echo -e "${GREEN}✅ Realm and ACL setup completed${NC}"

# Join the Realm: each actr.toml names its key file (`realm_client_key`).
ACTR_CMD="${ACTR_GEN_CMD:-actr}" "$WORKSPACE_ROOT/scripts/join-realm.sh" \
    "$LORAN_DB" "$REALM" "http://localhost:8081/issuer" \
    "$ACTR_B_DIR/realm-client.json" "$ACTR_A_DIR/realm-client.json"
echo -e "${GREEN}✅ Joined Realm $REALM${NC}"


# Step 2.5: Build actr-b (receiver) binary
echo ""
echo "🔨 Building actr-b (receiver) binary..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

if ! cargo build --bin actr-b-receiver 2>&1; then
    echo -e "${RED}❌ Failed to build actr-b-receiver${NC}"
    exit 1
fi

echo -e "${GREEN}✅ actr-b-receiver binary built successfully${NC}"

# Step 3: Start Actr B (receiver)
echo ""
echo "🚀 Starting Actr B (Receiver)..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

RUST_LOG="${RUST_LOG:-info}" cargo run --bin actr-b-receiver > "$LOG_DIR/actr-b.log" 2>&1 &
ACTR_B_PID=$!

echo "Actr B started (PID: $ACTR_B_PID)"
echo "Waiting for Actr B to register..."
MAX_WAIT=25
COUNTER=0
while [ $COUNTER -lt $MAX_WAIT ]; do
    if ! kill -0 $ACTR_B_PID 2>/dev/null; then
        echo -e "${RED}❌ Actr B failed to start${NC}"
        cat "$LOG_DIR/actr-b.log"
        exit 1
    fi

    if grep -q "Actr B fully started and registered\|ActrNode started\|ActrNode started" "$LOG_DIR/actr-b.log" 2>/dev/null; then
        echo -e "${GREEN}✅ Actr B is running${NC}"
        break
    fi

    sleep 1
    COUNTER=$((COUNTER + 1))
done

if [ $COUNTER -eq $MAX_WAIT ]; then
    echo -e "${YELLOW}⚠️  Actr B may still be registering, continuing...${NC}"
fi

sleep 2

# Step 3.5: Install actr-a dependencies (resolve from loran registry after actr-b registered)
echo ""
echo "📦 Installing actr-a dependencies (actr deps install)..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

cd "$WORKSPACE_ROOT/media-relay/actr-a"
INSTALL_LOG="$LOG_DIR/actr-install-actr-a.log"
$ACTR_GEN_CMD deps install > "$INSTALL_LOG" 2>&1 || {
    echo -e "${YELLOW}⚠️  actr deps install returned non-zero, check log${NC}"
}
echo -e "${GREEN}✅ actr-a dependencies resolved${NC}"

# Step 3.6: Generate actr-a code
echo ""
echo "🛠️ Generating actr-a code (actr gen)..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

OUTPUT_FILE="$LOG_DIR/actr-gen-actr-a.log"
$ACTR_GEN_CMD gen --input="$PROTO_DIR" --output=src/generated --clean --no-scaffold > "$OUTPUT_FILE" 2>&1 || {
    echo -e "${RED}❌ actr gen failed for actr-a${NC}"
    cat "$OUTPUT_FILE"
    exit 1
}
cd "$WORKSPACE_ROOT"
echo -e "${GREEN}✅ actr gen completed for actr-a (relay)${NC}"

# Step 3.7: Build actr-a binary
echo ""
echo "🔨 Building actr-a (relay) binary..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

if ! cargo build --bin actr-a-relay 2>&1; then
    echo -e "${RED}❌ Failed to build actr-a-relay${NC}"
    exit 1
fi
echo -e "${GREEN}✅ actr-a-relay binary built successfully${NC}"

# Step 4: Start Actr A (relay/client)
echo ""
echo "🚀 Starting Actr A (Relay/Client)..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

RUST_LOG="${RUST_LOG:-info}" cargo run --bin actr-a-relay > "$LOG_DIR/actr-a.log" 2>&1 &
ACTR_A_PID=$!

echo "Actr A started (PID: $ACTR_A_PID)"
echo "Waiting for Actr A to start and send frames..."
MAX_WAIT=25
COUNTER=0
while [ $COUNTER -lt $MAX_WAIT ]; do
    if ! kill -0 $ACTR_A_PID 2>/dev/null; then
        echo -e "${RED}❌ Actr A failed to start${NC}"
        cat "$LOG_DIR/actr-a.log"
        exit 1
    fi

    if grep -q "ActrNode started" "$LOG_DIR/actr-a.log" 2>/dev/null; then
        echo -e "${GREEN}✅ Actr A is running${NC}"
        break
    fi

    sleep 1
    COUNTER=$((COUNTER + 1))
done

if [ $COUNTER -eq $MAX_WAIT ]; then
    echo -e "${YELLOW}⚠️  Actr A may still be compiling or starting, continuing...${NC}"
fi

echo ""
echo "⏳ Waiting for media frames to be sent and received..."
FRAME_WAIT=30
FRAME_COUNTER=0
while [ $FRAME_COUNTER -lt $FRAME_WAIT ]; do
    if grep -q "Received frame" "$LOG_DIR/actr-b.log" 2>/dev/null; then
        break
    fi

    sleep 1
    FRAME_COUNTER=$((FRAME_COUNTER + 1))
done

# Check if Actr A is still running (it should complete after sending frames)
echo ""
echo "🔍 Checking results..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Show last output from both actors
echo ""
echo "📋 Actr B Output (last 15 lines):"
tail -15 "$LOG_DIR/actr-b.log" | grep -E "(Received frame|started|success|register)" || tail -15 "$LOG_DIR/actr-b.log"

echo ""
echo "📋 Actr A Output (last 20 lines):"
tail -20 "$LOG_DIR/actr-a.log" | grep -E "(generate frame|sent|complete|started|success)" || tail -20 "$LOG_DIR/actr-a.log"

# Verify frames were received
echo ""
if grep -q "Received frame" "$LOG_DIR/actr-b.log"; then
    FRAME_COUNT=$(grep -c "Received frame" "$LOG_DIR/actr-b.log")
    echo -e "${GREEN}✅ Test PASSED: Actr B received $FRAME_COUNT frames${NC}"
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "🎉 Demo completed successfully!"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
    echo "✅ Validated:"
    echo "   • Real ActrNode lifecycle"
    echo "   • Real WebRTC P2P connection establishment"
    echo "   • Real RPC message routing and dispatch"
    echo "   • Real protobuf encode/decode"
    echo "   • Real distributed Actor communication"
    echo "   • Using loran as signaling server"
    echo ""
    echo "📖 View full logs:"
    echo "   tail -f $LOG_DIR/actr-a.log    # Sender logs"
    echo "   tail -f $LOG_DIR/actr-b.log    # Receiver logs"
    if [ "$LORAN_PID" != "external" ]; then
        echo "   tail -f $LOG_DIR/loran.log  # Loran logs"
    fi
    echo ""
    exit 0
else
    echo -e "${RED}❌ Test FAILED: No frames received${NC}"
    echo ""
    echo "Full logs:"
    echo "=== Actr B ==="
    cat "$LOG_DIR/actr-b.log"
    echo ""
    echo "=== Actr A ==="
    cat "$LOG_DIR/actr-a.log"
    if [ "$LORAN_PID" != "external" ]; then
        echo ""
        echo "=== Loran ==="
        tail -50 "$LOG_DIR/loran.log"
    fi
    exit 1
fi

# Wait a bit before cleanup
echo "Press Ctrl+C to stop all processes..."
wait $ACTR_A_PID $ACTR_B_PID 2>/dev/null || true
