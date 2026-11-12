#!/usr/bin/env bash
# Real Component loading/RPC followed by real-Issuer identity lifecycle tests
# over the exact Component bytes built by that successful runtime scenario.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(git -C "$script_dir" rev-parse --show-toplevel)"
log_dir="$(mktemp -d -t actr-v2-joint.XXXXXX)"
runtime_dir=""

cleanup() {
    local status=$?
    if [ "$status" -eq 0 ] && [ "${KEEP_TMP:-0}" != 1 ]; then
        # Both directories were created by this run. Never accept an arbitrary
        # path parsed from a failed or unrecognized runtime log for deletion.
        if [ -n "$runtime_dir" ] && [ "$(dirname "$runtime_dir")" = "$script_dir/.tmp" ] \
            && [[ "$(basename "$runtime_dir")" == run-* ]]; then
            rm -rf -- "$runtime_dir"
        fi
        rm -rf -- "$log_dir"
    else
        printf 'Joint acceptance logs: %s\n' "$log_dir"
    fi
}
trap cleanup EXIT

KEEP_TMP=1 bash "$script_dir/run.sh" --backend v2-component "$@" 2>&1 | tee "$log_dir/runtime.log"
runtime_dir="$(sed -n 's/^Artifacts preserved at: //p' "$log_dir/runtime.log" | tail -1)"
if [ -z "$runtime_dir" ] || [ ! -d "$runtime_dir/v2-echo-payload" ]; then
    echo "Successful runtime did not provide its Component artifact directory" >&2
    exit 1
fi
component="$runtime_dir/v2-echo-payload/workload/echo.wasm"
[ -s "$component" ] || { echo "Missing real Component artifact" >&2; exit 1; }

cd "$repo_root"
ACTR_V2_COMPONENT_FIXTURE="$component" cargo test --locked -p loran-issuer \
    --test integration_test v2_package:: -- --test-threads=1 2>&1 | tee "$log_dir/issuer-lifecycle.log"

printf 'Joint v2 acceptance passed: real Host load/register/RPC and Component-backed Issuer renewal/reissue.\n'
