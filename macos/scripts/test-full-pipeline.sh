#!/bin/bash
# ============================================================================
# Actrium Full Pipeline Test Script
#
# 1. Kill old processes
# 2. Rebuild: keel guest-bundle -> system.jsbundle -> Actrium.app
# 3. Launch app, collect logs for N seconds
# 4. Kill, parse & filter logs
# 5. Generate categorised report
# ============================================================================

set -euo pipefail

# ---- Configuration --------------------------------------------------------
WAIT_SECS="${1:-8}"                       # How long to let the app run (default 8s)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ACTRIUM_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$ACTRIUM_DIR/../../.." && pwd)"
KEEL_DIR="${ACTRIUM_KEEL_PATH:-$REPO_ROOT/packages/keel}"
IPC_LOG="/tmp/actrium_ipc_debug.log"
SYSLOG_RAW="/tmp/actrium_syslog_raw.txt"
REPORT="/tmp/actrium_pipeline_report_$(date +%Y%m%d_%H%M%S).txt"
DERIVED_DATA=$(xcodebuild -workspace "$ACTRIUM_DIR/Actrium.xcworkspace" -scheme Actrium -showBuildSettings 2>/dev/null | grep -m1 'BUILT_PRODUCTS_DIR' | awk '{print $3}')
APP_PATH="${ACTRIUM_APP_PATH:-${DERIVED_DATA}/Actrium.app}"

# ANSI colors (only for terminal, not in report file)
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[0;33m'; CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'
info()  { printf "${CYAN}[INFO]${NC}  %s\n" "$*"; }
ok()    { printf "${GREEN}[OK]${NC}    %s\n" "$*"; }
fail()  { printf "${RED}[FAIL]${NC}  %s\n" "$*"; }
warn()  { printf "${YELLOW}[WARN]${NC}  %s\n" "$*"; }
header(){ printf "\n${BOLD}=== %s ===${NC}\n" "$*"; }

# ---- Step 0: Pre-flight ---------------------------------------------------
header "Step 0: Pre-flight"
echo "ACTRIUM_DIR  = $ACTRIUM_DIR"
echo "KEEL_DIR    = $KEEL_DIR"
echo "APP_PATH    = $APP_PATH"
echo "WAIT_SECS   = $WAIT_SECS"
echo "REPORT      = $REPORT"

# ---- Step 1: Kill old processes -------------------------------------------
header "Step 1: Kill old processes"
pkill -9 -f 'Actrium\.app/Contents/MacOS/Actrium' 2>/dev/null && info "Killed Actrium.app" || info "No Actrium running"
pkill -9 -f 'ActriumViewService' 2>/dev/null && info "Killed ActriumViewService" || info "No ActriumViewService running"
sleep 0.5

# Double-check
if pgrep -f 'Actrium\.app/Contents/MacOS/Actrium' >/dev/null 2>&1; then
    warn "Actrium still alive, sending SIGKILL again..."
    pkill -9 -f 'Actrium\.app/Contents/MacOS/Actrium' 2>/dev/null || true
    sleep 1
fi

# Clean stale logs
rm -f "$IPC_LOG" "$SYSLOG_RAW"
info "Old processes killed, stale logs cleaned."

# ---- Step 2: Rebuild everything ------------------------------------------
header "Step 2: Rebuild"

# 2a. keel guest-bundle
info "Building keel guest-bundle..."
GUEST_OUT=$(cd "$KEEL_DIR" && bun scripts/build-guest-bundle.ts 2>&1)
if echo "$GUEST_OUT" | grep -q 'Build complete'; then
    SIZE=$(echo "$GUEST_OUT" | grep -oE '[0-9.]+ KB' | head -1)
    ok "guest-bundle built ($SIZE)"
else
    fail "guest-bundle build failed:"
    echo "$GUEST_OUT" | tail -10
    exit 1
fi

# 2b. The single system jsbundle (Actr Guests share Engine.ts)
info "Building system.jsbundle..."
ACTR_OUT=$(cd "$ACTRIUM_DIR" && ./scripts/bundle-actrium-runtime.sh --target system 2>&1)
if echo "$ACTR_OUT" | grep -q 'Done!'; then
    ACTR_SIZE=$(echo "$ACTR_OUT" | grep -oE 'Size: [0-9A-Za-z.]+' | head -1)
    ok "system.jsbundle built ($ACTR_SIZE)"
else
    fail "system.jsbundle build failed:"
    echo "$ACTR_OUT" | tail -15
    exit 1
fi

# 2c. Actrium.app (xcodebuild)
info "Building Actrium.app (xcodebuild)..."
BUILD_LOG="/tmp/actrium_xcode_build.log"
xcodebuild -workspace "$ACTRIUM_DIR/Actrium.xcworkspace" -scheme Actrium \
    -configuration Debug build 2>&1 > "$BUILD_LOG"
if tail -5 "$BUILD_LOG" | grep -q 'BUILD SUCCEEDED'; then
    ok "Actrium.app built"
else
    fail "Actrium.app build failed:"
    tail -20 "$BUILD_LOG"
    exit 1
fi

# ---- Step 3: Launch app and collect logs ----------------------------------
header "Step 3: Launch & collect logs (${WAIT_SECS}s)"

# Touch IPC log file so tail -f has something to follow
touch "$IPC_LOG"

# Start system log stream (Actrium main + XPC service)
/usr/bin/log stream \
    --predicate 'process CONTAINS "Actrium" OR process CONTAINS "ActriumViewService" OR process CONTAINS "RCTViewService"' \
    --style compact --level debug \
    > "$SYSLOG_RAW" 2>&1 &
SYSLOG_PID=$!

# Launch app
info "Opening Actrium.app..."
open "$APP_PATH"

# Show live IPC log progress
info "Collecting logs for ${WAIT_SECS}s... (Ctrl+C to abort early)"
SECONDS_ELAPSED=0
while [ $SECONDS_ELAPSED -lt "$WAIT_SECS" ]; do
    sleep 1
    SECONDS_ELAPSED=$((SECONDS_ELAPSED + 1))
    LINE_COUNT=0
    [ -f "$IPC_LOG" ] && LINE_COUNT=$(wc -l < "$IPC_LOG" | tr -d ' ')
    printf "  %2d/%ds  IPC lines: %s\r" "$SECONDS_ELAPSED" "$WAIT_SECS" "$LINE_COUNT"
done
echo ""

# ---- Step 4: Kill & collect -----------------------------------------------
header "Step 4: Stop processes"
pkill -f 'Actrium\.app/Contents/MacOS/Actrium' 2>/dev/null || true
pkill -f 'ActriumViewService' 2>/dev/null || true
kill "$SYSLOG_PID" 2>/dev/null || true
sleep 0.5
info "All processes stopped."

# ---- Step 5: Parse, filter, report ----------------------------------------
header "Step 5: Generating report"

{
    echo "============================================================"
    echo "  Actrium Full Pipeline Test Report"
    echo "  Generated: $(date '+%Y-%m-%d %H:%M:%S')"
    echo "  Run duration: ${WAIT_SECS}s"
    echo "============================================================"
    echo ""

    # -----------------------------------------------------------------
    # Section A: JS-side logs (from /tmp/actrium_ipc_debug.log)
    # -----------------------------------------------------------------
    echo "============================================================"
    echo "  A. JS-SIDE LOGS (IPC debug log)"
    echo "============================================================"
    if [ -f "$IPC_LOG" ] && [ -s "$IPC_LOG" ]; then
        TOTAL_JS=$(wc -l < "$IPC_LOG" | tr -d ' ')
        echo "  Total lines: $TOTAL_JS"
        echo ""

        # A1. Engine lifecycle
        echo "--- A1. Engine & Bundle Lifecycle ---"
        grep -iE 'Engine init|loadBundle|loadUnifiedBundle|TIMING|createReceiver|Engine created|Engine destroy' "$IPC_LOG" 2>/dev/null || echo "  (none)"
        echo ""

        # A2. Reconciler / render
        echo "--- A2. Reconciler & Render ---"
        grep -iE 'reconciler|render called|transformed element|handleUpdate|nodeCount|totalBatches|totalOps' "$IPC_LOG" 2>/dev/null || echo "  (none)"
        echo ""

        # A3. Panel extraction
        echo "--- A3. Panel Extraction ---"
        grep -iE 'leftPanel|rightPanel|fullTree|Panel|PanelMarker' "$IPC_LOG" 2>/dev/null || echo "  (none)"
        echo ""

        # A4. Timer / setTimeout / setInterval
        echo "--- A4. Timers (setTimeout / setInterval) ---"
        grep -iE 'setTimeout|setInterval|DIAG.*timer|TimerManager' "$IPC_LOG" 2>/dev/null || echo "  (none)"
        echo ""

        # A5. Function props / callbacks
        echo "--- A5. Function Props & Callbacks ---"
        grep -iE 'serializeProps|deserializeProps|__fnId|callbackRegistry|invokeCallback|__type.*function|onPress|onSubmit' "$IPC_LOG" 2>/dev/null || echo "  (none)"
        echo ""

        # A6. Errors & warnings
        echo "--- A6. JS Errors & Warnings ---"
        grep -iE 'error|ERROR|warn|WARN|exception|crash|fail' "$IPC_LOG" 2>/dev/null | grep -vi 'onUncaughtError\|onCaughtError\|onRecoverableError' || echo "  (none)"
        echo ""

        # A7. Bridge / serialization (JS-side only, exclude [HOST]/[SVC] ViewBridge lines)
        echo "--- A7. Bridge & Serialization ---"
        grep -iE '\[JS\].*Bridge|sendToHost|decodeBatch|encodeBatch|seenPool|circular' "$IPC_LOG" 2>/dev/null \
            | grep -v 'ViewBridge\|ensureBridge\|RCTBridge' || echo "  (none)"
        echo ""

    else
        echo "  (No IPC log or empty)"
    fi

    # -----------------------------------------------------------------
    # Section B: XPC Service (Native) logs from syslog
    # -----------------------------------------------------------------
    echo "============================================================"
    echo "  B. XPC SERVICE NATIVE LOGS (actr-only)"
    echo "============================================================"
    if [ -f "$SYSLOG_RAW" ] && [ -s "$SYSLOG_RAW" ]; then
        # Whitelist: only keep lines with actr/Actrium app-specific keywords
        grep -E 'ActriumViewService|RCTViewService' "$SYSLOG_RAW" 2>/dev/null \
            | grep -iE 'Keel|actr|tabConfig|ViewBridge|RCTRoot|NativeLog|ensureBridge|createRoot|registerPending|applyConfig|bootstrap|pendingTab|ServiceXPC|hostProcess|containerSize|listenerReady|serviceReady|serviceState|serviceDidBecome|handleTab|pendingEntry|ipcLog|\[SVC\]|\[HOST\]|\[JS\]' \
            | head -60 || echo "  (none)"
    else
        echo "  (No syslog captured)"
    fi
    echo ""

    # -----------------------------------------------------------------
    # Section C: Host-side Actrium logs from syslog
    # -----------------------------------------------------------------
    echo "============================================================"
    echo "  C. HOST-SIDE ACTRIUM LOGS (actr-only)"
    echo "============================================================"
    if [ -f "$SYSLOG_RAW" ] && [ -s "$SYSLOG_RAW" ]; then
        # Whitelist: only keep Actrium host process lines with app-specific keywords
        grep -E 'Actrium\[' "$SYSLOG_RAW" 2>/dev/null \
            | grep -iE 'Keel|actr|tabConfig|ViewBridge|RCTRoot|IPC|XPC|Service|ensureBridge|bootstrap|listenerReady|serviceReady|serviceState|UnifiedRN|coordinator|containerSize|hostProcess|allocateService|prewarm|pendingTab|\[HOST\]|\[JS\]' \
            | head -100 || echo "  (none)"
    else
        echo "  (No syslog captured)"
    fi
    echo ""

    # -----------------------------------------------------------------
    # Section D: Mouse / Click events
    # -----------------------------------------------------------------
    echo "============================================================"
    echo "  D. MOUSE / CLICK EVENTS"
    echo "============================================================"
    if [ -f "$IPC_LOG" ]; then
        grep -iE 'mouse|click|onPress|touchStart|touchEnd|gestureRecognizer' "$IPC_LOG" 2>/dev/null | head -30 || echo "  (none)"
    fi
    if [ -f "$SYSLOG_RAW" ]; then
        grep -E 'Actrium\[|ActriumViewService' "$SYSLOG_RAW" 2>/dev/null \
            | grep -iE 'mouseDown|mouseUp|touchHandler|hitTest|onPress|gestureRecognizer' \
            | head -20 || echo "  (none from syslog)"
    fi
    echo ""

    # -----------------------------------------------------------------
    # Section E: Health checks
    # -----------------------------------------------------------------
    echo "============================================================"
    echo "  E. HEALTH CHECKS"
    echo "============================================================"

    ISSUES=0

    # E1. Service received tabConfig?
    if grep -q 'tabConfig\|handleTabConfigNotification' "$IPC_LOG" 2>/dev/null; then
        echo "  [OK]   TabConfig delivered to service"
    else
        echo "  [FAIL] TabConfig NOT delivered"
        ISSUES=$((ISSUES + 1))
    fi

    # E2. Engine created / JS loaded?
    if grep -q 'Engine created\|Engine init\|RuntimeApp registered\|Bundle loaded' "$IPC_LOG" 2>/dev/null; then
        echo "  [OK]   Engine / JS runtime active"
    else
        echo "  [FAIL] Engine / JS runtime NOT active"
        ISSUES=$((ISSUES + 1))
    fi

    # E3. Bundle loaded?
    if grep -q 'loadBundle\|Bundle loaded\|loadUnifiedBundle' "$IPC_LOG" 2>/dev/null; then
        echo "  [OK]   Bundle loaded"
    else
        echo "  [FAIL] Bundle NOT loaded"
        ISSUES=$((ISSUES + 1))
    fi

    # E4. Receiver has nodes?
    if grep -qE 'nodeCount=[1-9]' "$IPC_LOG" 2>/dev/null; then
        NODES=$(grep -oE 'nodeCount=[0-9]+' "$IPC_LOG" | tail -1)
        echo "  [OK]   Receiver has nodes ($NODES)"
    else
        echo "  [WARN] nodeCount not logged or zero"
    fi

    # E5. Panels extracted?
    if grep -q 'leftPanel=true\|leftPanel.*exists' "$IPC_LOG" 2>/dev/null; then
        echo "  [OK]   Left panel extracted"
    else
        echo "  [WARN] Left panel extraction not confirmed"
    fi
    if grep -q 'rightPanel=true\|rightPanel.*exists' "$IPC_LOG" 2>/dev/null; then
        echo "  [OK]   Right panel extracted"
    else
        echo "  [WARN] Right panel extraction not confirmed"
    fi

    # E6. Any JS errors?
    JS_ERRORS=0
    if [ -f "$IPC_LOG" ]; then
        JS_ERRORS=$(grep -ciE '\[JS ERROR\]|render error|callback.*error|Exception' "$IPC_LOG" 2>/dev/null || true)
        JS_ERRORS=$(echo "$JS_ERRORS" | tr -d '[:space:]')
        [ -z "$JS_ERRORS" ] && JS_ERRORS=0
    fi
    if [ "$JS_ERRORS" -gt 0 ] 2>/dev/null; then
        echo "  [FAIL] $JS_ERRORS JS error(s) detected"
        ISSUES=$((ISSUES + 1))
    else
        echo "  [OK]   No JS errors"
    fi

    # E7. Timer functioning? (zero-delay fires promptly)
    if grep -qE 'DIAG.*setTimeout\|TIMING' "$IPC_LOG" 2>/dev/null; then
        echo "  [OK]   Timer diagnostics present"
    else
        echo "  [INFO] No timer diagnostics (normal if no DIAG logging)"
    fi

    # E8. XPC Service process running (syslog evidence)
    if grep -q 'ActriumViewService\|RCTViewService' "$SYSLOG_RAW" 2>/dev/null; then
        echo "  [OK]   XPC Service process was active"
    else
        echo "  [WARN] No XPC Service syslog evidence"
    fi

    echo ""
    echo "============================================================"
    if [ $ISSUES -eq 0 ]; then
        echo "  RESULT: All critical checks PASSED"
    else
        echo "  RESULT: $ISSUES issue(s) found"
    fi
    echo "============================================================"
    echo ""

    # -----------------------------------------------------------------
    # Section F: Full IPC log (chronological, cleaned)
    # -----------------------------------------------------------------
    echo "============================================================"
    echo "  F. FULL IPC LOG (chronological, noise-filtered)"
    echo "============================================================"
    if [ -f "$IPC_LOG" ] && [ -s "$IPC_LOG" ]; then
        # Remove extremely verbose / repetitive lines
        grep -v \
            -e 'Require cycle:' \
            -e 'Running "' \
            -e 'registerCallableModule' \
            -e 'TurboModuleManager' \
            -e 'SurfacePresenter' \
            -e 'Unrecognized font' \
            -e 'UIViewRegistry' \
            "$IPC_LOG" 2>/dev/null || cat "$IPC_LOG"
    else
        echo "  (empty)"
    fi

} > "$REPORT" 2>&1

# ---- Print summary to terminal -------------------------------------------
header "Report saved to: $REPORT"
echo ""

# Print health checks section to terminal with colors
info "Health Check Summary:"
while IFS= read -r line; do
    if echo "$line" | grep -q '\[OK\]'; then
        ok "$(echo "$line" | sed 's/.*\[OK\] *//')"
    elif echo "$line" | grep -q '\[FAIL\]'; then
        fail "$(echo "$line" | sed 's/.*\[FAIL\] *//')"
    elif echo "$line" | grep -q '\[WARN\]'; then
        warn "$(echo "$line" | sed 's/.*\[WARN\] *//')"
    elif echo "$line" | grep -q '\[INFO\]'; then
        info "$(echo "$line" | sed 's/.*\[INFO\] *//')"
    elif echo "$line" | grep -q 'RESULT:'; then
        echo ""
        if echo "$line" | grep -q 'PASSED'; then
            ok "$line"
        else
            fail "$line"
        fi
    fi
done < <(sed -n '/E\. HEALTH CHECKS/,/RESULT:/p' "$REPORT")

echo ""
info "Full report:  $REPORT"
info "IPC log:      $IPC_LOG"
info "Syslog raw:   $SYSLOG_RAW"
echo ""
info "Quick view:   less $REPORT"
info "JS errors:    grep -i error $IPC_LOG"

# Cleanup temp build log
rm -f "$BUILD_LOG"
