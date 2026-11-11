#!/usr/bin/env bash
#
# Bundle Actrium runtime apps for production (without Metro dependency)
#
# Usage:
#   ./scripts/bundle-actrium-runtime.sh [options]
#
# Options:
#   --target <name>   Bundle target: "system" (default: system)
#   --dev             Build development bundle (with source maps, not minified)
#   --help            Show this help message
#
# Bundles:
#   system  -> Actrium/Resources/system.jsbundle  (for RCTViewService0)
#
# Examples:
#   ./scripts/bundle-actrium-runtime.sh                    # Build system apps (release)
#   ./scripts/bundle-actrium-runtime.sh --target system    # Build system apps only
#   ./scripts/bundle-actrium-runtime.sh --dev              # Build system apps in dev mode
#

set -euo pipefail
unset NO_COLOR

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$PROJECT_ROOT/.." && pwd)"
if [ -n "${ACTRIUM_RUNTIME_PATH:-}" ]; then
    ACTRIUM_RUNTIME_DIR="$(cd "$ACTRIUM_RUNTIME_PATH" && pwd -P)"
else
    ACTRIUM_RUNTIME_DIR="$REPO_ROOT/packages/actrium"
fi
OUTPUT_DIR="$PROJECT_ROOT/Actrium/Resources"
GENERATED_ASSETS_DIR="$PROJECT_ROOT/.generated/actrium-runtime-assets"

# Default options
TARGET="system"
DEV_MODE=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --target)
            TARGET="$2"
            shift 2
            ;;
        --dev)
            DEV_MODE=true
            shift
            ;;
        --help|-h)
            head -20 "$0"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# Validate target
if [[ ! "$TARGET" =~ ^system$ ]]; then
    echo "❌ Invalid target: $TARGET"
    echo "   Valid target: system"
    exit 1
fi

echo "📦 Bundling Actrium runtime apps..."
echo "   Source: $ACTRIUM_RUNTIME_DIR"
echo "   Output: $OUTPUT_DIR"
echo "   Target: $TARGET"
echo "   Dev mode: $DEV_MODE"
echo ""

# Check if Actrium runtime directory exists
if [ ! -d "$ACTRIUM_RUNTIME_DIR" ]; then
    echo "❌ Error: Actrium runtime directory not found at $ACTRIUM_RUNTIME_DIR"
    exit 1
fi

# Check if node_modules exists
if [ ! -d "$ACTRIUM_RUNTIME_DIR/node_modules" ]; then
    echo "⚠️  node_modules not found, installing runtime dependencies..."
    if command -v bun >/dev/null 2>&1; then
        (cd "$ACTRIUM_RUNTIME_DIR" && bun install)
    elif command -v corepack >/dev/null 2>&1; then
        (cd "$ACTRIUM_RUNTIME_DIR" && corepack yarn install)
    else
        (cd "$ACTRIUM_RUNTIME_DIR" && npm install)
    fi
fi

# Create output directory if needed
mkdir -p "$OUTPUT_DIR"
rm -rf "$OUTPUT_DIR/.generated-assets" "$GENERATED_ASSETS_DIR"

cd "$ACTRIUM_RUNTIME_DIR"

# The shared runtime also contains the iOS/Android React Native package, whose
# CLI does not register the out-of-tree macOS platform. Bundle this macOS app
# through react-native-macos so the matching platform and Metro defaults load.
REACT_NATIVE_CLI="node_modules/react-native-macos/cli.js"
if [ ! -f "$REACT_NATIVE_CLI" ]; then
    echo "❌ Error: react-native-macos CLI not found at $ACTRIUM_RUNTIME_DIR/$REACT_NATIVE_CLI"
    exit 1
fi

copy_assets_for_xcode() {
    local assets_dir="$GENERATED_ASSETS_DIR/assets"
    if [ -z "${TARGET_BUILD_DIR:-}" ] || [ -z "${UNLOCALIZED_RESOURCES_FOLDER_PATH:-}" ]; then
        return
    fi
    if [ ! -d "$assets_dir" ]; then
        return
    fi

    local resources_dir="$TARGET_BUILD_DIR/$UNLOCALIZED_RESOURCES_FOLDER_PATH"
    mkdir -p "$resources_dir/assets"
    rsync -a --delete "$assets_dir/" "$resources_dir/assets/"
    echo "   Assets copied: $resources_dir/assets"
}

# Function to bundle a target
bundle_target() {
    local name="$1"
    local entry_file="$2"
    local output_file="$3"

    echo "🔨 Building $name bundle..."
    echo "   Entry: $entry_file"
    echo "   Output: $output_file"

    if [ "$DEV_MODE" = true ]; then
        node "$REACT_NATIVE_CLI" bundle \
            --platform macos \
            --dev true \
            --entry-file "$entry_file" \
            --bundle-output "$output_file" \
            --assets-dest "$GENERATED_ASSETS_DIR" \
            --sourcemap-output "$output_file.map"
        echo "   ✅ Dev bundle created (with source map)"
    else
        node "$REACT_NATIVE_CLI" bundle \
            --platform macos \
            --dev false \
            --entry-file "$entry_file" \
            --bundle-output "$output_file" \
            --assets-dest "$GENERATED_ASSETS_DIR" \
            --minify true
        echo "   ✅ Production bundle created (minified)"
    fi

    copy_assets_for_xcode

    # Show bundle size
    if [ -f "$output_file" ]; then
        local size=$(du -h "$output_file" | cut -f1)
        echo "   Size: $size"
    fi
    echo ""
}

# Bundle the system apps
bundle_target "System Apps" "index-system.tsx" "$OUTPUT_DIR/system.jsbundle"

echo "🎉 Done!"
echo ""
echo "Bundle locations:"
echo "   System Apps: $OUTPUT_DIR/system.jsbundle"
echo ""
echo "Note: Make sure bundle files are added to Xcode project resources."
