#!/usr/bin/env bash
set -euo pipefail
scenario_dir="$(cd "$(dirname "$0")" && pwd)"
transport="${REALM_HTTPS:-1}"
# The Publisher rotation fixture edits Registry rows directly. Its dynamic
# document provider is intentionally separate from the HTTPS publication path.
if [ "${MODE:-runtime}" = rotation ]; then transport="${REALM_HTTPS:-0}"; fi
case "$transport" in 0|1) ;; *) echo 'REALM_HTTPS must be 0 or 1' >&2; exit 1 ;; esac
if [ "${MODE:-runtime}" = rotation ] && [ "$transport" = 1 ]; then
    echo 'Publisher rotation uses REALM_HTTPS=0; HTTPS publication is covered by the default matrix' >&2
    exit 1
fi
for realm_mode in provider external; do
    MODE="${MODE:-runtime}" REALM_HTTPS="$transport" REALM_MODE="$realm_mode" bash "$scenario_dir/run.sh" "$@"
done
