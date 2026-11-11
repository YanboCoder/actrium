#!/usr/bin/env bash
# Test-only invitation transport. The device creates its own key; no private
# key or invitation is included in the built APK/IPA or ordinary config.
# Requires the existing E2E common.sh, RUN_DIR and desktop owner key.
mobile_invitation_file() {
    local invite_file="$RUN_DIR/keys/mobile-invite.json"
    local result="$RUN_DIR/keys/mobile-enrollment.json"
    (umask 077; run_actr realm invite --key "$REALM_OWNER_KEY" --role editor > "$invite_file")
    python3 - "$invite_file" "$REALM_OWNER_KEY" "$result" <<'PY'
import json, os, sys
invitation = json.load(open(sys.argv[1]))
owner = json.load(open(sys.argv[2]))
with os.fdopen(os.open(sys.argv[3], os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), 'w') as f:
    json.dump({'invite': invitation['invite'], 'provider_id': owner['provider_id']}, f)
os.unlink(sys.argv[1])
PY
}

install_ios_mobile_invitation() {
    mobile_invitation_file
    local container
    container="$(xcrun simctl get_app_container "$DEVICE_UDID" "$APP_BUNDLE_ID" data)"
    mkdir -p "$container/Library/Caches"
    install -m 600 "$RUN_DIR/keys/mobile-enrollment.json" "$container/Library/Caches/actr-enrollment.json"
    rm "$RUN_DIR/keys/mobile-enrollment.json"
}
