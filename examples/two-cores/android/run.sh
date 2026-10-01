#!/usr/bin/env bash
# Runs the two-core test app on an Android emulator or device (ADR-044): builds both cores for Android
# (libplayground_a.so and libplayground_b.so), builds and installs one APK with both, launches it and
# checks its `two-cores android:` logcat lines. Exits non-zero unless every check passed.
#
#   examples/two-cores/android/run.sh              the one attached device, or ANDROID_SERIAL
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
UNDRA="${UNDRA:-$REPO/target/debug/undra}"
ADB="${ADB:-$(command -v adb || echo "${ANDROID_HOME:-}/platform-tools/adb")}"
[ -x "$UNDRA" ] || (cd "$REPO" && cargo build -p undra-cli)
for ns in a b; do "$UNDRA" build -C "$HERE/../$ns" --platform android --release; done
(cd "$HERE" && ./gradlew --quiet :app:assembleDebug)
"$ADB" install -r "$HERE/app/build/outputs/apk/debug/app-debug.apk" >/dev/null
"$ADB" shell am force-stop dev.undra.twocores
"$ADB" logcat -c
"$ADB" shell am start -n dev.undra.twocores/.MainActivity >/dev/null
for _ in $(seq 1 30); do
  lines="$("$ADB" logcat -d -s TwoCores:I | grep -o 'two-cores android: .*' || true)"
  if printf '%s\n' "$lines" | grep -qE 'two-cores android: (passed|FAILED)$'; then break; fi
  sleep 1
done
printf '%s\n' "$lines"
printf '%s\n' "$lines" | grep -qx 'two-cores android: passed'
