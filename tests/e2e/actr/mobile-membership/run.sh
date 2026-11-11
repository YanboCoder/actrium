#!/usr/bin/env bash
set -euo pipefail
task_root="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
platform="${1:?usage: run.sh android|ios}"
task_dir="$(mktemp -d "${TMPDIR:-/tmp}/actr-mobile-membership.XXXXXX")"
fixture_pid=""
cleanup() {
    touch "$task_dir/fixture.json.stop"
    if [ -n "$fixture_pid" ]; then wait "$fixture_pid" || true; fi
    rm -f "$task_dir/fixture.json"
}
trap cleanup EXIT
cd "$task_root"
cargo test -p loran-issuer --test integration_test --no-run --message-format=json > "$task_dir/build.json"
fixture_bin="$(python3 - "$task_dir/build.json" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    item = json.loads(line)
    if item.get('target', {}).get('name') == 'integration_test' and item.get('executable'):
        print(item['executable'])
PY
)"
ACTR_MOBILE_FIXTURE_READY="$task_dir/fixture.json" "$fixture_bin" --ignored \
    --exact device_membership::mobile_issuer_http_fixture > "$task_dir/issuer.log" 2>&1 &
fixture_pid=$!
for _ in $(seq 1 100); do
    [ -s "$task_dir/fixture.json" ] && break
    kill -0 "$fixture_pid" || { cat "$task_dir/issuer.log"; exit 1; }
    sleep 0.1
done
test -s "$task_dir/fixture.json"
case "$platform" in
    android)
        : "${ANDROID_SERIAL:?select an emulator explicitly with ANDROID_SERIAL}"
        cd "$task_root/packages/actr-kotlin"
        # Native library and generated bindings must already match this checkout.
        test -f actr-kotlin/src/main/jniLibs/arm64-v8a/libactr.so
        ./gradlew :actr-kotlin:assembleDebugAndroidTest --no-daemon
        apk=actr-kotlin/build/outputs/apk/androidTest/debug/actr-kotlin-debug-androidTest.apk
        python3 "$task_root/scripts/actr/check-mobile-assets.py" "$apk"
        adb -s "$ANDROID_SERIAL" install -r "$apk" >/dev/null
        adb -s "$ANDROID_SERIAL" shell run-as io.actrium.actr.test mkdir -p no_backup
        adb -s "$ANDROID_SERIAL" shell "run-as io.actrium.actr.test sh -c 'umask 077; cat > no_backup/issuer-fixture.json'" < "$task_dir/fixture.json"
        adb -s "$ANDROID_SERIAL" shell am instrument -w -e class io.actrium.actr.RealmIdentityStoreTest \
            io.actrium.actr.test/androidx.test.runner.AndroidJUnitRunner | tee "$task_dir/android.log"
        rg -q 'OK \(2 tests\)' "$task_dir/android.log"
        ;;
    ios)
        : "${IOS_SIMULATOR_UDID:?select a booted iOS simulator explicitly}"
        cd "$task_root/tests/e2e/actr/mobile-membership/ios"
        xcodegen generate
        xcodebuild -scheme MembershipHost -destination "platform=iOS Simulator,id=$IOS_SIMULATOR_UDID" \
            -derivedDataPath "${ACTR_IOS_TEST_DERIVED_DATA:-$task_dir/derived}" \
            "ACTR_MOBILE_FIXTURE=$task_dir/fixture.json" test > "$task_dir/ios.log" 2>&1 || {
                tail -80 "$task_dir/ios.log"; exit 1;
            }
        tail -18 "$task_dir/ios.log"
        ;;
    *) echo 'expected android or ios' >&2; exit 2 ;;
esac
python3 - "$task_dir/fixture.json.observed" <<'PY'
import json, sys
rows = list(json.load(open(sys.argv[1])).values())
assert sum(row.get('renew:200', 0) > 0 for row in rows) >= 2, 'both device keys must renew through the real Issuer'
assert any(row.get('renew:403', 0) > 0 for row in rows), 'revoked device must be refused on renewal'
print('Verified real Issuer renewal on both keys and rejection after revocation.')
PY
echo "Mobile membership evidence: $task_dir"
