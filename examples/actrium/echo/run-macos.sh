#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
APP_DIR="$SCRIPT_DIR/app"
APP_PACKAGE="$APP_DIR/dist/actrium-echo-ui-0.1.0.actr"
PACKAGE_ECHO_DIR="$REPO_ROOT/examples/actr/rust/package-echo"
DERIVED_DATA_DIR="${ACTRIUM_ECHO_DERIVED_DATA_DIR:-$REPO_ROOT/target/xcode-derived-data/actrium-echo}"
STACK_LOG="$REPO_ROOT/examples/actr/rust/logs/actrium-echo-stack.log"
MESSAGE="Hello from Actrium"
OPEN_APP=1
SKIP_INSTALL=0
STACK_PID=""

usage() {
    sed -n '/^# Usage:/,/^# The script/p' "$0" \
        | sed 's/^# \{0,1\}//' \
        | sed '$d'
}

# Run the complete Echo at Actrium demo on macOS.
#
# Usage:
#   ./run-macos.sh [options] [message]
#
# Options:
#   --no-open       Build and verify the complete stack without launching Actrium.
#   --skip-install  Reuse existing pnpm and CocoaPods installations.
#   --help          Show this help.
#
# The script verifies a real echo call before opening Actrium. It then keeps
# Loran and both Actor Hosts alive until interrupted.

while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-open)
            OPEN_APP=0
            shift
            ;;
        --skip-install)
            SKIP_INSTALL=1
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        -*)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
        *)
            MESSAGE="$1"
            shift
            ;;
    esac
done

cleanup() {
    if [ -n "$STACK_PID" ]; then
        kill "$STACK_PID" 2>/dev/null || true
        wait "$STACK_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

for command in bun cargo curl jq pnpm python3 xcodebuild; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "Required command is missing: $command" >&2
        exit 1
    fi
done

cd "$REPO_ROOT"
if [ "$SKIP_INSTALL" = "0" ]; then
    # The demo consumes workspace sources; package lifecycle hooks are not
    # needed here and must not make setup depend on unrelated publish checks.
    pnpm install --frozen-lockfile --ignore-scripts
else
    # `pnpm run` may otherwise repair a stale workspace automatically. The
    # explicit skip flag must reuse the installed tree without starting an
    # implicit install; pnpm still reports any drift before running scripts.
    export pnpm_config_verify_deps_before_run="${pnpm_config_verify_deps_before_run:-warn}"
fi

# Apply the repository's idempotent react-native-macos compatibility fixes.
# The root install intentionally skips all package lifecycle scripts above.
node "$REPO_ROOT/packages/actrium/scripts/patch-react-native-macos.js"

# The Guest bundler is Keel's source CLI. Keel intentionally keeps its own
# Bun lockfile outside the root pnpm workspace, so provision that toolchain on
# a clean checkout even when the root install is being reused.
if [ "$SKIP_INSTALL" = "0" ] || [ ! -d "$REPO_ROOT/packages/keel/node_modules/commander" ]; then
    (
        cd "$REPO_ROOT/packages/keel"
        bun install --frozen-lockfile
    )
fi

pnpm --filter echo-at-actrium typecheck
pnpm --filter echo-at-actrium test
pnpm --filter echo-at-actrium build
cargo run -p actr-cli -- pkg pack \
    --manifest-path "$APP_DIR/manifest.toml" \
    --payload-root "$APP_DIR" \
    --key "$PACKAGE_ECHO_DIR/dev-key.json" \
    --output "$APP_PACKAGE"
cargo run -p actr-cli -- pkg verify \
    --package "$APP_PACKAGE" \
    --pubkey "$PACKAGE_ECHO_DIR/dev-key.json"

mkdir -p "$(dirname "$STACK_LOG")"
ECHO_KEEP_RUNNING=1 \
    "$PACKAGE_ECHO_DIR/start.sh" \
    --client-mode http \
    --http-listen 127.0.0.1:8391 \
    --keep-running \
    "$MESSAGE" > "$STACK_LOG" 2>&1 &
STACK_PID=$!

echo "Starting Loran and the Echo Actors. Log: $STACK_LOG"
STACK_TIMEOUT_SECONDS="${STACK_TIMEOUT_SECONDS:-1800}"
COUNTER=0
until grep -q "Echo stack is ready" "$STACK_LOG" 2>/dev/null; do
    if ! kill -0 "$STACK_PID" 2>/dev/null; then
        echo "Echo stack failed to start." >&2
        tail -100 "$STACK_LOG" >&2
        exit 1
    fi
    if [ "$COUNTER" -ge "$STACK_TIMEOUT_SECONDS" ]; then
        echo "Timed out waiting for the Echo stack." >&2
        tail -100 "$STACK_LOG" >&2
        exit 1
    fi
    sleep 1
    COUNTER=$((COUNTER + 1))
done

curl --silent --fail http://127.0.0.1:8391/health | jq .
echo "The Actor path is verified. Building Actrium for macOS."

cd "$REPO_ROOT/macos"
if [ "$SKIP_INSTALL" = "0" ] || [ ! -d Pods ] || [ ! -d build/generated/ios ]; then
    if ! command -v pod >/dev/null 2>&1; then
        echo "CocoaPods is required to build Actrium." >&2
        exit 1
    fi
    pod install
fi

./scripts/bundle-actrium-runtime.sh --target system --dev

ACTRIUM_CORE_LIBRARY="$REPO_ROOT/crates/actrium-core/ActriumCore.xcframework/macos-arm64_x86_64/libactrium_core.a"
ACTRIUM_VAULT_LIBRARY="$REPO_ROOT/crates/actrium-vault/ActriumVault.xcframework/macos-arm64_x86_64/libactrium_vault.a"
if [ ! -f "$ACTRIUM_CORE_LIBRARY" ] || [ ! -f "$ACTRIUM_VAULT_LIBRARY" ]; then
    echo "Building the Apple Rust bindings required by Actrium."
    "$REPO_ROOT/scripts/actrium/build-actrium-rust-bindings.sh" apple-macos
fi

xcodebuild \
    -workspace Actrium.xcworkspace \
    -scheme Actrium \
    -configuration Debug \
    -destination "platform=macOS,arch=$(uname -m)" \
    -derivedDataPath "$DERIVED_DATA_DIR" \
    CODE_SIGNING_ALLOWED=NO \
    build

APP_PATH="$DERIVED_DATA_DIR/Build/Products/Debug/Actrium.app"
if [ ! -d "$APP_PATH" ]; then
    echo "Actrium build did not produce $APP_PATH" >&2
    exit 1
fi

if [ "$OPEN_APP" = "0" ]; then
    echo "Echo at Actrium verified without launching the UI."
    exit 0
fi

open -n "$APP_PATH" --args --url "actro://$APP_PACKAGE" --legacy-actor-bridge-package "$APP_PACKAGE"
echo "Echo at Actrium is running. Press Ctrl-C to stop the demo stack."
wait "$STACK_PID"
