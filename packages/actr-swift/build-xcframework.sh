#!/usr/bin/env bash
set -euo pipefail

# Build the Rust library as an XCFramework and generate UniFFI Swift bindings.
#
# Inputs:
# - ../../crates/actr-ffi (Rust FFI crate with the UniFFI configuration)
#
# Outputs:
# - ./dist/ActrBindings/** (Actr.swift + headers + modulemap)
# - ./dist/ActrFFI.xcframework/**
#
# The macOS slice is a DYNAMIC framework (cdylib): two rustc-built staticlibs
# cannot share one Mach-O — libactr.a linked next to the static libactrium_core.a
# trips hundreds of duplicate std/core symbols, because both archives embed the
# same toolchain runtime. Shipping the macOS slice as a versioned @rpath dylib
# framework keeps the two FFI chains coexisting in one app (be148b6df). An
# xcframework cannot mix framework and library slices, so the iOS slices are
# dynamic frameworks too (SwiftPM embeds them in the consuming app).

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "${ROOT_DIR}"

WORKSPACE_ROOT="$(cd "${ROOT_DIR}/../.." && pwd)"
CRATE_DIR="${WORKSPACE_ROOT}/crates/actr-ffi"
CRATE_LIB_NAME="actr"
FRAMEWORK_NAME="ActrFFI"
TARGET_DIR="${WORKSPACE_ROOT}/target"
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-15.0}"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-12.0}"
BUILD_PROFILE="${ACTR_BUILD_PROFILE:-release}"
XCFRAMEWORK_TARGETS="${ACTR_XCFRAMEWORK_TARGETS:-all}"
declare -a CARGO_PROFILE_ARGS
declare -a HOST_FEATURE_ARGS

case "${BUILD_PROFILE}" in
  debug)
    CARGO_PROFILE_ARGS=()
    CARGO_PROFILE_DIR="debug"
    ;;
  release)
    CARGO_PROFILE_ARGS=(--release)
    CARGO_PROFILE_DIR="release"
    ;;
  *)
    echo "error: unsupported ACTR_BUILD_PROFILE: ${BUILD_PROFILE}" >&2
    echo "hint: use ACTR_BUILD_PROFILE=debug or ACTR_BUILD_PROFILE=release" >&2
    exit 1
    ;;
esac

case "${XCFRAMEWORK_TARGETS}" in
  all | macos)
    ;;
  *)
    echo "error: unsupported ACTR_XCFRAMEWORK_TARGETS: ${XCFRAMEWORK_TARGETS}" >&2
    echo "hint: use ACTR_XCFRAMEWORK_TARGETS=all or ACTR_XCFRAMEWORK_TARGETS=macos" >&2
    exit 1
    ;;
esac

resolve_root_path() {
  local path="$1"
  if [[ "${path}" = /* ]]; then
    printf '%s\n' "${path}"
  else
    printf '%s\n' "${ROOT_DIR}/${path}"
  fi
}

BINDINGS_DIR="$(resolve_root_path "${ACTR_BINDINGS_PATH:-dist/ActrBindings}")"
HEADERS_DIR="${BINDINGS_DIR}/include"
XCFRAMEWORK_DIR="$(resolve_root_path "${ACTR_BINARY_PATH:-dist/${FRAMEWORK_NAME}.xcframework}")"

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: missing required command: $1" >&2
    exit 1
  fi
}

require_file() {
  if [[ ! -f "$1" ]]; then
    echo "error: missing required file: $1" >&2
    exit 1
  fi
}

require_cmd cargo
require_cmd xcodebuild
require_cmd uniffi-bindgen
require_cmd rustc

echo "Checking libactr crate..."
echo "Using libactr crate at: ${CRATE_DIR}"
echo "Using cargo profile: ${BUILD_PROFILE}"
echo "Using XCFramework targets: ${XCFRAMEWORK_TARGETS}"

require_file "${CRATE_DIR}/Cargo.toml"
require_file "${CRATE_DIR}/uniffi.toml"

HOST_TARGET="$(rustc -vV | awk -F': ' '/^host:/{print $2}')"
if [[ -z "${HOST_TARGET}" ]]; then
  echo "error: failed to detect host target triple from rustc" >&2
  exit 1
fi

cargo_build_libactr() {
  local target="$1"
  shift

  (cd "${WORKSPACE_ROOT}" && env -u SDKROOT cargo build -p libactr "${CARGO_PROFILE_ARGS[@]+"${CARGO_PROFILE_ARGS[@]}"}" --target "${target}" "$@")
}

echo "[1/4] Preparing bindings output directory"
mkdir -p "${HEADERS_DIR}"

# Keep the "empty C file trick" for Xcode. UniFFI does not generate this file.
if [[ ! -f "${BINDINGS_DIR}/actrFFI.c" ]]; then
  : > "${BINDINGS_DIR}/actrFFI.c"
fi

# Remove previously generated artifacts to avoid accidentally mixing old/new symbols.
rm -f \
  "${BINDINGS_DIR}/Actr.swift" \
  "${BINDINGS_DIR}/actrFFI.h" \
  "${BINDINGS_DIR}/ActrFFI.h" \
  "${BINDINGS_DIR}/actrFFI.modulemap" \
  "${BINDINGS_DIR}/ActrFFI.modulemap" \
  "${HEADERS_DIR}/actrFFI.h"

echo "[2/4] Generating Swift bindings (host: ${HOST_TARGET})"
REUSE_HOST_MACOS_BUILD=0
if [[ "${HOST_TARGET}" == "aarch64-apple-darwin" ]]; then
  HOST_FEATURE_ARGS=(--features macos-oslog)
  REUSE_HOST_MACOS_BUILD=1
fi

cargo_build_libactr "${HOST_TARGET}" "${HOST_FEATURE_ARGS[@]+"${HOST_FEATURE_ARGS[@]}"}"

DYLIB_PATH="${TARGET_DIR}/${HOST_TARGET}/${CARGO_PROFILE_DIR}/lib${CRATE_LIB_NAME}.dylib"
if [[ ! -f "${DYLIB_PATH}" ]]; then
  echo "error: expected host dylib not found: ${DYLIB_PATH}" >&2
  echo "hint: ensure Cargo.toml has crate-type = [\"cdylib\", \"staticlib\"]" >&2
  exit 1
fi

(cd "${CRATE_DIR}" && uniffi-bindgen generate --library "${DYLIB_PATH}" --language swift --out-dir "${BINDINGS_DIR}")
python3 "${WORKSPACE_ROOT}/scripts/actr/patch-error-messages.py" swift "${BINDINGS_DIR}/Actr.swift"

require_file "${BINDINGS_DIR}/Actr.swift"
MODULEMAP_FILE=$(find "${BINDINGS_DIR}" -maxdepth 1 -iname "actrFFI.modulemap" | head -n 1)
require_file "${MODULEMAP_FILE}"

# UniFFI's generated Swift bindings only attempt to import the binary target module (ActrFFI),
# but the C declarations (RustBuffer, RustCallStatus, etc) live in the SwiftPM C target module
# (`actrFFI`). Patch the generated file so it builds in SwiftPM/Xcode.
if ! grep -q -F '#if canImport(actrFFI)' "${BINDINGS_DIR}/Actr.swift"; then
  perl -0777pi -e 's|(#if canImport\(ActrFFI\)\nimport ActrFFI\n#endif)|#if canImport(actrFFI)\n    import actrFFI\n#endif\n$1|' "${BINDINGS_DIR}/Actr.swift"
fi

# Swift 6 strict concurrency can reject passing non-Sendable closures into Task. Patch the generated
# helper to wrap captured closures in an @unchecked Sendable container.
if ! grep -q "struct UniffiUnsafeSendable" "${BINDINGS_DIR}/Actr.swift"; then
  # Insert the struct definition
  perl -0777pi -e 's|(private func uniffiTraitInterfaceCallAsync<T>\()|private struct UniffiUnsafeSendable<T>: \@unchecked Sendable {\n    let value: T\n\n    init(_ value: T) {\n        self.value = value\n    }\n}\n\n$1|' "${BINDINGS_DIR}/Actr.swift"
fi

if ! grep -q "makeCallSendable = UniffiUnsafeSendable(makeCall)" "${BINDINGS_DIR}/Actr.swift"; then
  # Patch uniffiTraitInterfaceCallAsync
  perl -0777pi -e 's|private func uniffiTraitInterfaceCallAsync<T>\(\n\s*makeCall: \@escaping \(\) async throws -> T,\n\s*handleSuccess: \@escaping \(T\) -> \(\),\n\s*handleError: \@escaping \(Int8, RustBuffer\) -> \(\),\n\s*droppedCallback: UnsafeMutablePointer<UniffiForeignFutureDroppedCallbackStruct>\n\) \{\n\s*let task = Task \{.*?\n\s*\}\n\s*let handle =|private func uniffiTraitInterfaceCallAsync<T>(\n    makeCall: \@escaping () async throws -> T,\n    handleSuccess: \@escaping (T) -> (),\n    handleError: \@escaping (Int8, RustBuffer) -> (),\n    droppedCallback: UnsafeMutablePointer<UniffiForeignFutureDroppedCallbackStruct>\n) {\n    let makeCallSendable = UniffiUnsafeSendable(makeCall)\n    let handleSuccessSendable = UniffiUnsafeSendable(handleSuccess)\n    let handleErrorSendable = UniffiUnsafeSendable(handleError)\n\n    let task = Task {\n        var callResult: T\n        do {\n            callResult = try await makeCallSendable.value()\n        } catch {\n            handleErrorSendable.value(\n                CALL_UNEXPECTED_ERROR,\n                FfiConverterString.lower(String(describing: error))\n            )\n            return\n        }\n        handleSuccessSendable.value(callResult)\n    }\n    let handle =|sg' "${BINDINGS_DIR}/Actr.swift"

  # Patch uniffiTraitInterfaceCallAsyncWithError
	  perl -0777pi -e 's|private func uniffiTraitInterfaceCallAsyncWithError<T, E>\(\n\s*makeCall: \@escaping \(\) async throws -> T,\n\s*handleSuccess: \@escaping \(T\) -> \(\),\n\s*handleError: \@escaping \(Int8, RustBuffer\) -> \(\),\n\s*lowerError: \@escaping \(E\) -> RustBuffer,\n\s*droppedCallback: UnsafeMutablePointer<UniffiForeignFutureDroppedCallbackStruct>\n\) \{\n\s*let task = Task \{.*?\n\s*\}\n\s*let handle =|private func uniffiTraitInterfaceCallAsyncWithError<T, E>(\n    makeCall: \@escaping () async throws -> T,\n    handleSuccess: \@escaping (T) -> (),\n    handleError: \@escaping (Int8, RustBuffer) -> (),\n    lowerError: \@escaping (E) -> RustBuffer,\n    droppedCallback: UnsafeMutablePointer<UniffiForeignFutureDroppedCallbackStruct>\n) {\n    let makeCallSendable = UniffiUnsafeSendable(makeCall)\n    let handleSuccessSendable = UniffiUnsafeSendable(handleSuccess)\n    let handleErrorSendable = UniffiUnsafeSendable(handleError)\n    let lowerErrorSendable = UniffiUnsafeSendable(lowerError)\n\n    let task = Task {\n        var callResult: T\n        do {\n            callResult = try await makeCallSendable.value()\n        } catch let error as E {\n            handleErrorSendable.value(CALL_ERROR, lowerErrorSendable.value(error))\n            return\n        } catch {\n            handleErrorSendable.value(\n                CALL_UNEXPECTED_ERROR,\n                FfiConverterString.lower(String(describing: error))\n            )\n            return\n        }\n        handleSuccessSendable.value(callResult)\n    }\n    let handle =|sg' "${BINDINGS_DIR}/Actr.swift"
fi

# Swift 6 treats the generated callback vtable pointer as unsafe global state.
# It is intentionally leaked for process lifetime because Rust stores the pointer.
perl -0777pi -e 's|(\n\s*)static let vtablePtr:|$1nonisolated(unsafe) static let vtablePtr:|g' "${BINDINGS_DIR}/Actr.swift"

if ! grep -q -F '#if canImport(actrFFI)' "${BINDINGS_DIR}/Actr.swift"; then
  echo "error: expected Actr.swift to include 'import actrFFI' patch" >&2
  exit 1
fi

if ! grep -q "struct UniffiUnsafeSendable" "${BINDINGS_DIR}/Actr.swift"; then
  echo "error: expected Actr.swift to include UniffiUnsafeSendable patch" >&2
  exit 1
fi

if ! grep -q "makeCallSendable = UniffiUnsafeSendable(makeCall)" "${BINDINGS_DIR}/Actr.swift"; then
  echo "error: expected Actr.swift to include Swift 6 concurrency patch" >&2
  exit 1
fi

if grep -q "^[[:space:]]*static let vtablePtr:" "${BINDINGS_DIR}/Actr.swift"; then
  echo "error: expected Actr.swift callback vtable pointers to be marked nonisolated(unsafe)" >&2
  exit 1
fi

# UniFFI currently writes the header next to the modulemap; SwiftPM expects public headers under
# `publicHeadersPath` (we use `ActrBindings/include`).
HEADER_FILE=$(find "${BINDINGS_DIR}" -maxdepth 1 -iname "actrFFI.h" | head -n 1)
if [[ -n "${HEADER_FILE}" && -f "${HEADER_FILE}" ]]; then
  mv -f "${HEADER_FILE}" "${HEADERS_DIR}/actrFFI.h"
fi
require_file "${HEADERS_DIR}/actrFFI.h"

# Ensure the modulemap points at the header location used by SwiftPM.
if [[ -f "${MODULEMAP_FILE}" ]]; then
  if [[ "$OSTYPE" == "darwin"* ]]; then
    sed -i '' 's|header ".*"|header "include/actrFFI.h"|g' "${MODULEMAP_FILE}"
  else
    sed -i 's|header ".*"|header "include/actrFFI.h"|g' "${MODULEMAP_FILE}"
  fi
fi

if [[ "${XCFRAMEWORK_TARGETS}" == "all" ]]; then
  echo "[3/4] Building Rust static libraries (iOS) in parallel"

  # Use separate target directories to allow concurrent cargo invocations.
  IOS_TARGET_DIR="${TARGET_DIR}/xcframework-ios"
  IOS_SIM_TARGET_DIR="${TARGET_DIR}/xcframework-ios-sim"

  echo "  → Starting aarch64-apple-ios build..."
  (cd "${WORKSPACE_ROOT}" && env -u SDKROOT CARGO_TARGET_DIR="${IOS_TARGET_DIR}" \
    cargo build -p libactr "${CARGO_PROFILE_ARGS[@]+"${CARGO_PROFILE_ARGS[@]}"}" --target aarch64-apple-ios) &
  IOS_PID=$!

  echo "  → Starting aarch64-apple-ios-sim build..."
  (cd "${WORKSPACE_ROOT}" && env -u SDKROOT CARGO_TARGET_DIR="${IOS_SIM_TARGET_DIR}" \
    cargo build -p libactr "${CARGO_PROFILE_ARGS[@]+"${CARGO_PROFILE_ARGS[@]}"}" --target aarch64-apple-ios-sim) &
  IOS_SIM_PID=$!

  # wait(1) returns the background process exit code.  Under `set -e` a
  # non-zero exit from `wait` would kill the script before `$?` is captured.
  # The `||` operator suppresses errexit for its left-hand side so the
  # assignment still runs on failure.
  IOS_EXIT=0
  IOS_SIM_EXIT=0
  wait $IOS_PID || IOS_EXIT=$?
  wait $IOS_SIM_PID || IOS_SIM_EXIT=$?

  if [[ $IOS_EXIT -ne 0 ]]; then
    echo "error: aarch64-apple-ios build failed (exit ${IOS_EXIT})" >&2
    exit $IOS_EXIT
  fi
  if [[ $IOS_SIM_EXIT -ne 0 ]]; then
    echo "error: aarch64-apple-ios-sim build failed (exit ${IOS_SIM_EXIT})" >&2
    exit $IOS_SIM_EXIT
  fi

  # Verify artifacts exist before copying.
  IOS_LIB="${IOS_TARGET_DIR}/aarch64-apple-ios/${CARGO_PROFILE_DIR}/lib${CRATE_LIB_NAME}.a"
  IOS_SIM_LIB="${IOS_SIM_TARGET_DIR}/aarch64-apple-ios-sim/${CARGO_PROFILE_DIR}/lib${CRATE_LIB_NAME}.a"
  require_file "${IOS_LIB}"
  require_file "${IOS_SIM_LIB}"

  # Copy artifacts to the standard target layout expected by step [4/4].
  mkdir -p "${TARGET_DIR}/aarch64-apple-ios/${CARGO_PROFILE_DIR}"
  mkdir -p "${TARGET_DIR}/aarch64-apple-ios-sim/${CARGO_PROFILE_DIR}"
  cp "${IOS_LIB}" "${TARGET_DIR}/aarch64-apple-ios/${CARGO_PROFILE_DIR}/lib${CRATE_LIB_NAME}.a"
  cp "${IOS_SIM_LIB}" "${TARGET_DIR}/aarch64-apple-ios-sim/${CARGO_PROFILE_DIR}/lib${CRATE_LIB_NAME}.a"
  cp "${IOS_TARGET_DIR}/aarch64-apple-ios/${CARGO_PROFILE_DIR}/lib${CRATE_LIB_NAME}.dylib" \
    "${TARGET_DIR}/aarch64-apple-ios/${CARGO_PROFILE_DIR}/lib${CRATE_LIB_NAME}.dylib"
  cp "${IOS_SIM_TARGET_DIR}/aarch64-apple-ios-sim/${CARGO_PROFILE_DIR}/lib${CRATE_LIB_NAME}.dylib" \
    "${TARGET_DIR}/aarch64-apple-ios-sim/${CARGO_PROFILE_DIR}/lib${CRATE_LIB_NAME}.dylib"

  # Clean up temporary target directories.
  rm -rf "${IOS_TARGET_DIR}" "${IOS_SIM_TARGET_DIR}"

  echo "  ✅ All targets built successfully"
else
  echo "[3/4] Skipping iOS slices (ACTR_XCFRAMEWORK_TARGETS=macos)"
fi
if [[ "${REUSE_HOST_MACOS_BUILD}" -eq 1 ]]; then
  echo "Reusing host build for aarch64-apple-darwin"
else
  cargo_build_libactr aarch64-apple-darwin --features macos-oslog
fi
# macOS slice is Apple Silicon only: the app targets macOS 26.1 and is built
# arm64-only (see ARCHS in the Xcode project), so no x86_64 build is required.

echo "[4/4] Creating XCFramework (macOS slice: dynamic framework)"
rm -rf "${XCFRAMEWORK_DIR}"

write_framework_info_plist() {
  cat > "$1" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleExecutable</key>
	<string>ActrFFI</string>
	<key>CFBundleIdentifier</key>
	<string>org.actr.ActrFFI</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>ActrFFI</string>
	<key>CFBundlePackageType</key>
	<string>FMWK</string>
	<key>CFBundleShortVersionString</key>
	<string>1.0</string>
	<key>CFBundleVersion</key>
	<string>1</string>
</dict>
</plist>
PLIST
}

write_framework_modulemap() {
  cat > "$1" <<MODULEMAP
framework module ${FRAMEWORK_NAME} {
    header "Headers/actrFFI.h"
    export *
}
MODULEMAP
}

# Assemble a versioned ActrFFI.framework bundle around the arm64 macOS
# dylib: Versions/A/{ActrFFI, Headers, Modules, Resources}, Current +
# top-level symlinks, ad-hoc signature (Xcode re-signs on embed).
create_macos_dylib_framework() {
  local arm64_dylib="${TARGET_DIR}/aarch64-apple-darwin/${CARGO_PROFILE_DIR}/lib${CRATE_LIB_NAME}.dylib"
  require_file "${arm64_dylib}"

  local staging
  staging="$(mktemp -d)"
  local fw="${staging}/${FRAMEWORK_NAME}.framework"
  mkdir -p \
    "${fw}/Versions/A/Headers" \
    "${fw}/Versions/A/Modules" \
    "${fw}/Versions/A/Resources"

  cp "${arm64_dylib}" "${fw}/Versions/A/${FRAMEWORK_NAME}"
  install_name_tool \
    -id "@rpath/${FRAMEWORK_NAME}.framework/Versions/A/${FRAMEWORK_NAME}" \
    "${fw}/Versions/A/${FRAMEWORK_NAME}"

  cp "${HEADERS_DIR}/actrFFI.h" "${fw}/Versions/A/Headers/"
  write_framework_modulemap "${fw}/Versions/A/Modules/module.modulemap"
  write_framework_info_plist "${fw}/Versions/A/Resources/Info.plist"

  (cd "${fw}/Versions" && ln -sfh A Current)
  (cd "${fw}" \
    && ln -sfh "Versions/Current/${FRAMEWORK_NAME}" "${FRAMEWORK_NAME}" \
    && ln -sfh Versions/Current/Headers Headers \
    && ln -sfh Versions/Current/Modules Modules \
    && ln -sfh Versions/Current/Resources Resources)

  codesign --force -s - "${fw}"
  echo "${fw}"
}

# Flat iOS-style ActrFFI.framework around a single-arch dylib.
create_ios_dylib_framework() {
  local dylib="$1"
  require_file "${dylib}"

  local staging
  staging="$(mktemp -d)"
  local fw="${staging}/${FRAMEWORK_NAME}.framework"
  mkdir -p "${fw}/Headers" "${fw}/Modules"

  cp "${dylib}" "${fw}/${FRAMEWORK_NAME}"
  install_name_tool \
    -id "@rpath/${FRAMEWORK_NAME}.framework/${FRAMEWORK_NAME}" \
    "${fw}/${FRAMEWORK_NAME}"
  cp "${HEADERS_DIR}/actrFFI.h" "${fw}/Headers/"
  write_framework_modulemap "${fw}/Modules/module.modulemap"
  write_framework_info_plist "${fw}/Info.plist"

  codesign --force -s - "${fw}"
  echo "${fw}"
}

# An xcframework cannot mix framework and library slices, so every slice is a
# dynamic framework; on iOS the app embeds it through SwiftPM like on macOS.
MACOS_FRAMEWORK_DIR="$(create_macos_dylib_framework)"
IOS_FRAMEWORK_DIR=""
IOS_SIM_FRAMEWORK_DIR=""
if [[ "${XCFRAMEWORK_TARGETS}" == "all" ]]; then
  IOS_FRAMEWORK_DIR="$(create_ios_dylib_framework \
    "${TARGET_DIR}/aarch64-apple-ios/${CARGO_PROFILE_DIR}/lib${CRATE_LIB_NAME}.dylib")"
  IOS_SIM_FRAMEWORK_DIR="$(create_ios_dylib_framework \
    "${TARGET_DIR}/aarch64-apple-ios-sim/${CARGO_PROFILE_DIR}/lib${CRATE_LIB_NAME}.dylib")"
fi

declare -a XCODEBUILD_ARGS
if [[ "${XCFRAMEWORK_TARGETS}" == "all" ]]; then
  XCODEBUILD_ARGS+=(
    -framework "${IOS_FRAMEWORK_DIR}"
    -framework "${IOS_SIM_FRAMEWORK_DIR}"
  )
fi
XCODEBUILD_ARGS+=(
  -framework "${MACOS_FRAMEWORK_DIR}"
  -output "${XCFRAMEWORK_DIR}"
)

xcodebuild -create-xcframework "${XCODEBUILD_ARGS[@]}"

rm -rf "$(dirname "${MACOS_FRAMEWORK_DIR}")"
[[ -n "${IOS_FRAMEWORK_DIR}" ]] && rm -rf "$(dirname "${IOS_FRAMEWORK_DIR}")"
[[ -n "${IOS_SIM_FRAMEWORK_DIR}" ]] && rm -rf "$(dirname "${IOS_SIM_FRAMEWORK_DIR}")"

echo ""
echo "✅ XCFramework build complete!"
echo ""
echo "📦 Output:"
echo "   - Framework: ${XCFRAMEWORK_DIR}"
echo "   - Bindings:  ${BINDINGS_DIR}/Actr.swift"
echo ""
echo "📋 Next steps:"
echo "   1. Package for release: ./scripts/package-binary.sh <tag>"
echo "   2. Publish through the monorepo release train"
echo "   3. Keep this directory's Package.swift local-only"
echo ""
