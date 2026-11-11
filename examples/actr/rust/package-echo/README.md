# package-echo

End-to-end test for the Actr package-driven execution flow, demonstrating how to build, sign, verify, and run `.actr` packages with WebRTC-based service discovery.

## Overview

This example demonstrates:

1. **Build**: Compile echo-actr package (WASM or native cdylib) and package it as a signed `.actr` archive
2. **Sign**: Create signed `.actr` archive using `actr build` (cdylib) or `actr build` (wasm)
3. **Verify**: Validate package signature with `actr pkg verify`
4. **Run**: Host server loads the package and exposes the echo service
5. **Discover**: Client discovers the service via Loran signaling server
6. **Connect**: Establish WebRTC connection and exchange messages

## Architecture

```
┌─────────────────┐
│   echo-actr     │  WASM package (guest actor)
│   (WASM)        │
└────────┬────────┘
         │ packaged into
         ▼
┌─────────────────┐
│  .actr package  │  Signed archive
│  (signed)       │
└────────┬────────┘
         │ published to
         ▼
┌─────────────────┐       ┌──────────────────┐
│     Loran      │◄─────►│ package-echo     │
│  (signaling +   │       │    server        │
│   publisher + Issuer)    │       │  (loads .actr)   │
└────────┬────────┘       └──────────────────┘
         │                         ▲
         │ WebRTC signaling        │ WebRTC data
         ▼                         │
┌─────────────────┐                │
│ package-echo    │────────────────┘
│    client       │
└─────────────────┘
```

## Platform Support

| Platform | Support | Notes |
|----------|---------|-------|
| **macOS** | ✅ Full | Native support |
| **Linux** | ✅ Full | Native support |
| **Windows** | ⚠️ WSL 2 | Requires WSL 2 (see below) |

### Windows Requirements

Windows users must use **WSL 2** (Windows Subsystem for Linux 2):

- ❌ **WSL 1**: Not supported (incomplete network stack, UDP issues)
- ✅ **WSL 2**: Fully supported (complete Linux kernel)

**Why WSL 2?**
- The test script uses Unix-specific tools (`bash`, `lsof`, `sqlite3`, etc.)
- WebRTC requires proper UDP support (TURN/ICE on port 3478)
- Better file system performance for Rust compilation

## Prerequisites

### All Platforms

1. **Rust toolchain** with `wasm32-wasip2` support
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   rustup target add wasm32-wasip2
   cargo install wasm-component-ld --version 0.5.26 --locked
   ```

2. **jq** (JSON processor)
   - macOS: Auto-installed via Homebrew if missing
   - Linux: Auto-installed via apt/yum/dnf if missing
   - Manual install: https://jqlang.github.io/jq/download/

3. **System tools**
   - `sqlite3` - Database operations
   - `lsof` - Port checking
   - `curl` - HTTP requests
   - `nc` (netcat) - Network testing

### macOS

```bash
# Install Homebrew if not present
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"

# Install dependencies (jq auto-installs if missing)
brew install sqlite3
```

### Linux (Ubuntu/Debian)

```bash
sudo apt-get update
sudo apt-get install -y \
    build-essential \
    pkg-config \
    libssl-dev \
    sqlite3 \
    lsof \
    netcat \
    curl \
    jq
```

### Linux (RHEL/CentOS/Fedora)

```bash
sudo yum install -y \
    gcc \
    openssl-devel \
    sqlite \
    lsof \
    nc \
    curl \
    jq
```

### Windows (WSL 2 Setup)

**Step 1: Install WSL 2**

```powershell
# PowerShell (Administrator)
wsl --install -d Ubuntu
wsl --set-default-version 2

# Verify WSL 2 is active
wsl -l -v
# Should show VERSION 2
```

**Step 2: Enter WSL and Install Dependencies**

```bash
# Enter WSL
wsl

# Install dependencies
sudo apt-get update
sudo apt-get install -y \
    build-essential \
    pkg-config \
    libssl-dev \
    sqlite3 \
    lsof \
    netcat \
    curl \
    jq \
    git

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
rustup target add wasm32-wasip2
cargo install wasm-component-ld --version 0.5.26 --locked
```

**Step 3: Clone Project to WSL Filesystem**

⚠️ **Important**: Clone to WSL filesystem, NOT Windows filesystem (`/mnt/c`)

```bash
# ✅ Good: WSL filesystem (fast)
cd ~
git clone <repo-url> actr

# ❌ Bad: Windows filesystem (3-5x slower compilation)
cd /mnt/c/Users/username/actr
```

**Performance Comparison:**
- WSL filesystem (`~`): Normal compilation speed
- Windows filesystem (`/mnt/c`): 3-5x slower

## Quick Start

### 1. Navigate to Example Directory

```bash
cd examples/actr/rust/package-echo
```

### 2. Run the Test

```bash
# Use default test message "TestMsg" with wasm backend (default)
./start.sh

# Send custom message
./start.sh "Hello World"

# Use native cdylib backend
./start.sh --backend cdylib

# cdylib backend with custom message
./start.sh --backend cdylib "Hello World"

# Generate a temporary echo-actr-xx service project and test it as the server workload
./start_tmp_echo_actr.sh "Hello World"
```

### 3. Expected Output

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
🧪 Testing package-echo (backend: wasm)
    Using Loran as signaling server
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
✅ jq found: jq-1.8.1
📦 Step 0: Building echo-actr (wasm)...
📦 Step 0.5: Building signed Component .actr package via actr build...
✅ .actr package built: 1.8M
✅ Package signature verified
...
✅ Test PASSED: package-backed echo server response received
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
🎉 Echo package test completed successfully!
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## What the Script Does

The `start.sh` script performs a complete end-to-end test:

### Step 0: Build Package

Two backends are supported via the `--backend` flag:

**wasm (default)**
- Builds `echo-actr` as a WASM Component (`wasm32-wasip2`)
- Links with `wasm-component-ld` through `actr build`

**cdylib**
- Compiles `echo-actr` as a native shared library using `manifest-cdylib.toml`
- Packages with `actr build`

### Step 0.5: Sign and Verify Package
- Creates signed `.actr` archive using `actr build`
- Verifies signature with `actr pkg verify`
- Builds client-guest cdylib package

### Step 1-2: Start Loran
- Checks for the Loran server binary (builds it if needed)
- Starts Loran on port 8081
- Waits for HTTP (8081/tcp) and ICE/TURN (13478/udp) to be ready

### Step 2.5-2.7: Setup Infrastructure
- Creates realms in the Loran database
- Registers publisher identity with public key
- Registers the client-guest Publisher public key

### Step 3-4: Start Server
- Builds package-echo-server binary
- Generates `../logs/server-actr.toml` with the current package path
- Runs `actr run -c ../logs/server-actr.toml`
- Loads `.actr` package in production trust mode
- Registers with Issuer and obtains credential

When using `start_tmp_echo_actr.sh`, the same flow runs against its isolated `tmp_server-actr.toml`.

### Step 5: Run Client
- Builds package-echo-client binary
- Discovers echo service via Loran
- Establishes WebRTC connection
- Sends test message and verifies echo response

### Step 6: Verify Results
- Checks client logs for expected echo response
- Reports success or failure

## Logs

All logs are stored in `logs/` directory:

```bash
# View logs
cat logs/loran.log              # Loran signaling server
cat logs/package-echo-server.log # Echo server
cat logs/package-echo-client.log # Echo client

# Follow logs in real-time
tail -f logs/loran.log
```

## Network Ports

| Port | Protocol | Service | Purpose |
|------|----------|---------|---------|
| 8081 | TCP | Loran HTTP | Signaling, Issuer, publisher |
| 3478 | UDP | Loran ICE/TURN | WebRTC connectivity |
| 49152-65535 | UDP | TURN relay | WebRTC data channels |

### WSL 2 Network Notes

- ✅ All ports work within WSL
- ⚠️ To access from Windows host, use port forwarding:

```powershell
# PowerShell (Administrator)
$wslIP = (wsl hostname -I).Trim()
netsh interface portproxy add v4tov4 `
    listenport=8081 `
    listenaddress=0.0.0.0 `
    connectport=8081 `
    connectaddress=$wslIP
```

## Troubleshooting

### "jq not found"

The script auto-installs `jq` on macOS (via Homebrew) and Linux (via package manager). If auto-install fails:

```bash
# macOS
brew install jq

# Linux
sudo apt-get install jq  # Debian/Ubuntu
sudo yum install jq      # RHEL/CentOS
```

### "Failed to extract public_key"

Ensure `jq` is installed and the public key file exists:

```bash
jq --version
ls -la ../echo-actr/public-key.json
```

### "Loran not available"

Build loran manually:

```bash
cd ../../../loran  # Navigate to loran directory
cargo build
```

### "Port 8081 already in use"

Kill existing process:

```bash
# macOS/Linux
lsof -ti:8081 | xargs kill

# WSL
sudo lsof -ti:8081 | xargs sudo kill
```

### WSL 2: "command not found: lsof"

Install missing tools:

```bash
sudo apt-get install lsof netcat sqlite3
```

### WSL 2: Slow compilation

Ensure project is on WSL filesystem, not Windows filesystem:

```bash
pwd
# ✅ Should be: /home/username/...
# ❌ Not: /mnt/c/Users/...
```

### WebRTC connection fails

Check UDP port 3478 is available:

```bash
lsof -iUDP:3478
# Should show loran listening
```

## Using `actr run` Directly

Instead of the full test script, you can run components individually:

```bash
# Start loran manually
cd ../../../loran
cargo run -- --config examples/actr/rust/package-echo/loran-config.toml

# Run the server with actr run after start.sh has generated ../logs/server-actr.toml
cd examples/actr/rust/package-echo
cargo run -p actr-cli -- run \
    --config ../logs/server-actr.toml

# Run client
cargo run --bin package-echo-client
```

Before running the server manually, make sure the runtime config points to a real `.actr` package. The committed `server-actr.toml` is only a template; `start.sh` leaves it untouched and writes the runnable config to `../logs/server-actr.toml` after building and signing `echo-actr`.

## Detached Runtime Lifecycle Check

Use the dedicated lifecycle script when you want to validate the detached runtime management flow against the package-backed server config:

```bash
cd examples/actr/rust/package-echo
bash manual-runtime-lifecycle.sh
```

The script derives an isolated config from `server-actr.toml`, starts loran, runs the signed local package, and verifies:

- `actr run -d` prints the WID plus the `actr logs <wid> -f` follow hint.
- `actr ps` shows `WID`, `ACTR_ID`, `PID`, `STATUS`, and `STARTED_AT`.
- `actr stop <wid-prefix>` followed by `actr start <wid-prefix>` keeps the same WID and assigns a new PID.
- `actr restart <wid-prefix>` keeps the same WID and log file.
- `actr logs <wid-prefix> -f` keeps streaming appended logs across restarts.
- A v1 runtime record in `run_dir` returns an actionable schema error that includes the directory path.

## Project Structure

```
package-echo/
├── README.md              # This file
├── manual-runtime-lifecycle.sh # Detached runtime lifecycle verification
├── start.sh               # End-to-end test script
├── loran-config.toml     # Loran server configuration
├── dev-key.json           # Development signing key
├── server-actr.toml       # Default server runtime config template
├── tmp_server-actr.toml   # Temporary-workload runtime config template (overwritten by start_tmp_echo_actr.sh)
├── client/                # Echo client (native)
│   ├── src/
│   ├── actr.toml
│   └── manifest.toml
├── client-guest/          # Echo client (cdylib package)
│   ├── src/
│   ├── actr.toml
│   └── manifest.toml
└── logs/                  # Generated logs (gitignored)
    ├── loran.log
    ├── package-echo-server.log
    └── package-echo-client.log
```

## Related Examples

- `echo-actr/` - The WASM guest actor package
- `shell-actr-echo/` - Shell-based echo example
- `ws-actr-echo/` - WebSocket-based echo example

## License

Apache-2.0
