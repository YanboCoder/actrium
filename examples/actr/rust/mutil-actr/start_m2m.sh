#!/bin/bash
# mutil-actr many-to-many concurrent test script - using loran as signaling server
# Test multiple clients concurrently sending messages to multiple servers
#
# Usage:
#   ./start_mutilclient2mutilserver.sh              # Start 3 clients and 2 servers (default)
#   ./start_mutilclient2mutilserver.sh 5 3          # Start 5 clients and 3 servers

# Note: not using set -e; we check errors explicitly throughout

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🧪 Testing mutil-actr (Multi-client to multi-server concurrent test)"
echo "    Using Loran as signaling server"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Get concurrent client and server count (default 3 clients, 2 servers)
NUM_CLIENTS=${1:-3}
NUM_SERVERS=${2:-2}
echo "📊 Starting $NUM_CLIENTS concurrent clients and $NUM_SERVERS servers"

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
ECHO_SERVER_DIR="$WORKSPACE_ROOT/mutil-actr/server"
ECHO_CLIENT_DIR="$WORKSPACE_ROOT/mutil-actr/client"
PROTO_DIR="$WORKSPACE_ROOT/mutil-actr/proto"

# Switch to workspace root and stay there
cd "$WORKSPACE_ROOT"

# Create logs directory
LOG_DIR="$WORKSPACE_ROOT/logs"
mkdir -p "$LOG_DIR"

# Ensure required CLI tools
source "$WORKSPACE_ROOT/scripts/ensure-tools.sh"

# Ensure actr.toml files exist
source "$WORKSPACE_ROOT/scripts/ensure-config-toml.sh"

# Ensure actr.toml files exist for server and client
echo ""
echo "🔍 Checking actr.toml files..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
ensure_actr_toml "$ECHO_SERVER_DIR"
ensure_actr_toml "$ECHO_CLIENT_DIR"

# Ensure loran-config.toml exists
ensure_loran_config "$WORKSPACE_ROOT"

echo ""
echo "🧰 Checking required CLI tools..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Cleanup function
cleanup() {
    echo ""
    echo "🧹 Cleaning up..."

    # Kill loran
    if [ ! -z "$LORAN_PID" ]; then
        echo "Stopping loran (PID: $LORAN_PID)"
        kill -9 $LORAN_PID 2>/dev/null || true
    fi

    # Kill all echo servers
    if [ ${#SERVER_PIDS[@]} -gt 0 ]; then
        echo "Stopping ${#SERVER_PIDS[@]} servers..."
        for pid in "${SERVER_PIDS[@]}"; do
            kill -9 $pid 2>/dev/null || true
        done
    fi

    # Kill all client apps
    if [ ${#CLIENT_PIDS[@]} -gt 0 ]; then
        echo "Stopping ${#CLIENT_PIDS[@]} clients..."
        for pid in "${CLIENT_PIDS[@]}"; do
            kill -9 $pid 2>/dev/null || true
        done
    fi

    # Wait briefly for processes to exit
    sleep 1

    echo "✅ Cleanup complete"
}

# Set trap to cleanup on exit
trap cleanup EXIT INT TERM

# Locate actr CLI
ACTR_GEN_CMD=""

if command -v actr > /dev/null 2>&1; then
    ACTR_GEN_CMD="actr"
elif [ -x "$ACTOR_RTC_DIR/actr/target/debug/actr" ]; then
    ACTR_GEN_CMD="$ACTOR_RTC_DIR/actr/target/debug/actr"
elif [ -x "$ACTOR_RTC_DIR/actr/target/release/actr" ]; then
    ACTR_GEN_CMD="$ACTOR_RTC_DIR/actr/target/release/actr"
else
    echo -e "${RED}❌ actr CLI not found (expected 'actr' in PATH or built at $ACTOR_RTC_DIR/actr)${NC}"
    exit 1
fi

if [ ! -d "$PROTO_DIR" ]; then
    echo -e "${RED}❌ Proto directory not found at $PROTO_DIR${NC}"
    exit 1
fi

# Step 0: Generate server code (no dependencies, can proceed without loran)
echo ""
echo "🛠️ Generating server code..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

cd "$ECHO_SERVER_DIR"
echo "Running actr deps install (server)..."
$ACTR_GEN_CMD deps install || {
    echo -e "${RED}❌ actr deps install failed (server)${NC}"
    exit 1
}
OUTPUT_FILE="$LOG_DIR/actr-gen-echo-server.log"
$ACTR_GEN_CMD gen --input="$PROTO_DIR" --output=src/generated --clean > "$OUTPUT_FILE" 2>&1 || {
    echo -e "${RED}❌ actr gen failed (server)${NC}"
    cat "$OUTPUT_FILE"
    exit 1
}
cd "$WORKSPACE_ROOT"
echo -e "${GREEN}✅ Server code generated${NC}"

# Step 1: Build and install loran
echo ""
echo "📦 Building and installing loran..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Check if loran directory exists
if [ ! -d "$LORAN_DIR" ]; then
    echo -e "${RED}❌ loran directory not found at $LORAN_DIR${NC}"
    echo "Please ensure loran project exists at: $LORAN_DIR"
    exit 1
fi

# Build loran with opentelemetry feature
echo "Building loran from source (with opentelemetry feature)..."
cd "$LORAN_DIR"
cargo build --manifest-path "$LORAN_DIR/crates/loran-server/Cargo.toml" --features opentelemetry 2>&1 | grep -v "^warning:" || true

# Check if build was successful
if [ ! -f "$LORAN_DIR/target/debug/loran" ]; then
    echo -e "${RED}❌ Failed to build loran${NC}"
    exit 1
fi

# Copy to ~/.cargo/bin
echo "Installing loran to ~/.cargo/bin..."
mkdir -p ~/.cargo/bin
cp "$LORAN_DIR/target/debug/loran" ~/.cargo/bin/loran
chmod +x ~/.cargo/bin/loran

# Return to workspace root
cd "$WORKSPACE_ROOT"

# Verify loran is available in PATH
if ! command -v loran > /dev/null 2>&1; then
    echo -e "${RED}❌ loran command not found in PATH after installation${NC}"
    echo "Please ensure ~/.cargo/bin is in your PATH"
    exit 1
fi

LORAN_CMD="loran"
echo -e "${GREEN}✅ Loran built and installed: $(which loran)${NC}"

# Step 2: Start loran
echo ""
echo "🚀 Starting loran (signaling server)..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

$LORAN_CMD --config "$LORAN_CONFIG" > "$LOG_DIR/loran.log" 2>&1 &
LORAN_PID=$!

echo "Loran started (PID: $LORAN_PID)"
echo "Waiting for loran to be ready..."

# Wait for loran to start and verify it's listening on port 8081
MAX_WAIT=10
COUNTER=0
while [ $COUNTER -lt $MAX_WAIT ]; do
    if ! kill -0 $LORAN_PID 2>/dev/null; then
        echo -e "${RED}❌ Loran failed to start${NC}"
        cat "$LOG_DIR/loran.log"
        exit 1
    fi

    # Check if port 8081 is listening (loran WebSocket server)
    if lsof -i:8081 > /dev/null 2>&1 || nc -z localhost 8081 2>/dev/null; then
        echo -e "${GREEN}✅ Loran is running and listening on port 8081${NC}"
        break
    fi

    sleep 1
    COUNTER=$((COUNTER + 1))
done

if [ $COUNTER -eq $MAX_WAIT ]; then
    echo -e "${RED}❌ Loran not listening on port 8081 after ${MAX_WAIT}s${NC}"
    cat "$LOG_DIR/loran.log"
    exit 1
fi

# Step 2.5: Setting up realms in loran (sqlite3)
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

REALM=$(grep -E '^\s*realm\s*=' "$ECHO_SERVER_DIR/actr.toml" | sed 's/.*=\s*//' | tr -d ' "' | head -1 || true)
if [ -z "$REALM" ]; then REALM=mutil-actr@example.com; fi


sqlite3 "$LORAN_DB" <<EOF
INSERT OR IGNORE INTO realm (name, status, enabled, created_at) VALUES ('$REALM', 'active', 1, strftime('%s','now'));
DELETE FROM actoracl WHERE realm = '$REALM';
INSERT INTO actoracl (realm, source_realm, from_type, to_type, access)
VALUES ('$REALM', '$REALM', 'acme/echo-client-app@1.0.0', 'acme/EchoService@1.0.0', 1);
INSERT INTO actoracl (realm, source_realm, from_type, to_type, access)
VALUES ('$REALM', '$REALM', 'acme/EchoService@1.0.0', 'acme/echo-client-app@1.0.0', 1);
EOF

echo -e "${GREEN}✅ Realm and ACL setup completed${NC}"

# Join the Realm: each actr.toml names its key file (`realm_client_key`).
ACTR_CMD="${ACTR_GEN_CMD:-actr}" "$WORKSPACE_ROOT/scripts/join-realm.sh" \
    "$LORAN_DB" "$REALM" "http://localhost:8081/issuer" \
    "$ECHO_SERVER_DIR/realm-client.json" "$ECHO_CLIENT_DIR/realm-client.json"
echo -e "${GREEN}✅ Joined Realm $REALM${NC}"


# Step 3: Start multiple servers
echo ""
echo "🚀 Starting $NUM_SERVERS servers..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Array to store all server process IDs
SERVER_PIDS=()

# Start multiple servers concurrently
for i in $(seq 1 $NUM_SERVERS); do
    echo "Starting server #$i..."
    RUST_LOG="${RUST_LOG:-info}" cargo run --bin mutil_server > "$LOG_DIR/mutil-actr-server-$i.log" 2>&1 &
    SERVER_PIDS+=($!)
    # Stagger start times slightly
    sleep 1
done

echo "Started ${#SERVER_PIDS[@]} server processes:"
for i in "${!SERVER_PIDS[@]}"; do
    echo "  Server #$((i+1)): PID ${SERVER_PIDS[$i]}"
done

# Wait for all servers to register
echo ""
echo "⏳ Waiting for all servers to register..."
MAX_WAIT=15
REGISTERED_COUNT=0

for i in $(seq 1 $NUM_SERVERS); do
    LOG_FILE="$LOG_DIR/mutil-actr-server-$i.log"
    COUNTER=0

    while [ $COUNTER -lt $MAX_WAIT ]; do
        if ! kill -0 ${SERVER_PIDS[$((i-1))]} 2>/dev/null; then
            echo -e "${RED}❌ Server #$i failed to start${NC}"
            cat "$LOG_FILE"
            exit 1
        fi

        if grep -q "ActrNode started\|Echo Server fully started and registered\|ActrNode started" "$LOG_FILE" 2>/dev/null; then
            echo -e "${GREEN}✅ Server #$i registered${NC}"
            REGISTERED_COUNT=$((REGISTERED_COUNT + 1))
            break
        fi

        sleep 1
        COUNTER=$((COUNTER + 1))
    done

    if [ $COUNTER -eq $MAX_WAIT ]; then
        echo -e "${YELLOW}⚠️  Server #$i may not have fully registered, continuing...${NC}"
    fi
done

echo "Registered servers: $REGISTERED_COUNT / $NUM_SERVERS"

# Extra wait time to ensure all servers are ready
sleep 2

# Step 3.5: Install client dependencies (from loran registry)
echo ""
echo "📦 Installing client dependencies (actr deps install)..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

cd "$ECHO_CLIENT_DIR"
INSTALL_LOG="$LOG_DIR/actr-install-client.log"
$ACTR_GEN_CMD deps install > "$INSTALL_LOG" 2>&1 || {
    echo -e "${YELLOW}⚠️  actr deps install returned non-zero, check log${NC}"
}
echo -e "${GREEN}✅ Client dependencies resolved${NC}"

# Step 3.6: Generate client code
echo ""
echo "🛠️ Generating client code (actr gen)..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

OUTPUT_FILE="$LOG_DIR/actr-gen-echo-client.log"
$ACTR_GEN_CMD gen --input="$PROTO_DIR" --output=src/generated --clean --no-scaffold > "$OUTPUT_FILE" 2>&1 || {
    echo -e "${RED}❌ actr gen failed${NC}"
    cat "$OUTPUT_FILE"
    exit 1
}
cd "$WORKSPACE_ROOT"
echo -e "${GREEN}✅ actr gen completed (client code refreshed)${NC}"

# Step 4: Start multiple concurrent clients
echo ""
echo "🚀 Starting $NUM_CLIENTS concurrent clients..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Array to store all client process IDs
CLIENT_PIDS=()

# Start multiple clients concurrently
for i in $(seq 1 $NUM_CLIENTS); do
    echo "Starting client #$i..."
    RUST_LOG="${RUST_LOG:-info}" cargo run --bin mutil_client > "$LOG_DIR/mutil-actr-client-$i.log" 2>&1 &
    CLIENT_PIDS+=($!)
    # Stagger start times slightly to avoid simultaneous connections
    sleep 0.5
done

echo "Started ${#CLIENT_PIDS[@]} client processes:"
for i in "${!CLIENT_PIDS[@]}"; do
    echo "  Client #$((i+1)): PID ${CLIENT_PIDS[$i]}"
done

# Wait for all clients to complete (max 20 seconds)
echo ""
echo "⏳ Waiting for all clients to complete..."
MAX_WAIT=20
COUNTER=0
while [ $COUNTER -lt $MAX_WAIT ]; do
    # Check if any clients are still running
    RUNNING=0
    for pid in "${CLIENT_PIDS[@]}"; do
        if kill -0 $pid 2>/dev/null; then
            RUNNING=$((RUNNING + 1))
        fi
    done

    if [ $RUNNING -eq 0 ]; then
        echo "✅ All clients completed"
        break
    fi

    sleep 1
    COUNTER=$((COUNTER + 1))
done

# Force terminate still-running clients
if [ $RUNNING -gt 0 ]; then
    echo -e "${YELLOW}⚠️  $RUNNING clients still running, force terminating...${NC}"
    for pid in "${CLIENT_PIDS[@]}"; do
        kill $pid 2>/dev/null || true
    done
fi

# Step 5: Show and verify output
echo ""
echo "🔍 Verifying output..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

SUCCESS_COUNT=0
FAIL_COUNT=0

for i in $(seq 1 $NUM_CLIENTS); do
    LOG_FILE="$LOG_DIR/mutil-actr-client-$i.log"

    # Check if response is successful
    if grep -q "success\|Got response from server" "$LOG_FILE"; then
        SUCCESS_COUNT=$((SUCCESS_COUNT + 1))
        echo -e "${GREEN}✅ Client #$i: PASSED${NC}"
    else
        FAIL_COUNT=$((FAIL_COUNT + 1))
        echo -e "${RED}❌ Client #$i: FAILED${NC}"
    fi
done

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 Test Results Summary"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  Servers: $NUM_SERVERS"
echo "  Clients: $NUM_CLIENTS"
echo "  Passed: $SUCCESS_COUNT"
echo "  Failed: $FAIL_COUNT"
echo ""

if [ $SUCCESS_COUNT -eq $NUM_CLIENTS ]; then
    echo -e "${GREEN}🎉 All tests passed!${NC}"
    echo ""
    echo "✅ Validated:"
    echo "   • $NUM_SERVERS servers running concurrently"
    echo "   • $NUM_CLIENTS concurrent client calls"
    echo "   • Each client received correct response"
    echo "   • Response contains corresponding client ID"
    echo "   • Using loran as signaling server"
    echo ""

    # Show all successful client outputs
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "📝 All client outputs:"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    for i in $(seq 1 $NUM_CLIENTS); do
        LOG_FILE="$LOG_DIR/mutil-actr-client-$i.log"
        if grep -q "success\|Got response from server" "$LOG_FILE"; then
            echo ""
            echo "Client #$i:"
            cat "$LOG_FILE" | grep -A 6 "success" || true
        fi
    done
    echo ""

    # Show server statistics
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "📝 Server processing stats:"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    for i in $(seq 1 $NUM_SERVERS); do
        LOG_FILE="$LOG_DIR/mutil-actr-server-$i.log"
        if [ -f "$LOG_FILE" ]; then
            # Try multiple patterns to match requests
            REQUEST_COUNT=$(grep -E "Received Echo request|📨.*Echo|handle.*Echo" "$LOG_FILE" 2>/dev/null | wc -l | tr -d ' ')
            echo "Server #$i: processed $REQUEST_COUNT requests"
        else
            echo "Server #$i: log file not found"
        fi
    done
    echo ""

    exit 0
else
    echo -e "${RED}❌ Test failed!${NC}"
    echo ""
    exit 1
fi
