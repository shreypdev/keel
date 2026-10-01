#!/usr/bin/env bash
# The React Native playground app's on-device checks (RN01..RN10, examples/playground/rn/src/checks.ts) on the
# iOS simulator and an Android emulator or phone: build the core and the Release app, install it, launch it, and
# wait for the app's own verdict line in the device log,
#
#     UNDRA-RN CHECKS 10/10 passed
#
# which is the only thing that counts: the process exits 0 only when every check passed, and prints the CHECK
# lines (and the device-log tail when it did not). The .github/workflows/rn-devices.yml jobs run exactly this.
#
#   scripts/rn-device-checks.sh ios                          the iPhone 17 Pro simulator (boots it if needed)
#   scripts/rn-device-checks.sh ios --target <udid>          a simulator by UDID
#   scripts/rn-device-checks.sh android                      the one device or emulator adb sees
#   scripts/rn-device-checks.sh android --target <serial>
#
# Options
#   --build-only      build the core and the app and stop, without installing or booting anything (CI boots its
#                     emulator after the build, so the build does not compete with the boot)
#   --no-build        use the core and app built by an earlier run
#   --target ID       the simulator UDID or the adb serial
#   --abi ABI         Android only: the one ABI of the app's own native library (default: the device's, else every
#                     ABI of gradle.properties), which is what keeps the CI build to one architecture
#   --expect N        the number of checks that must pass (default 10, RN01..RN10)
#   --timeout SEC     how long to wait for the verdict line after the launch (default 240)
#   --debug-core      build the core without --release (much slower; the checks pass too)
#
# Environment: SIMULATOR (default "iPhone 17 Pro"; when there is no such simulator the first available iPhone),
# UNDRA (the CLI binary, default target/debug/undra, built when missing), ANDROID_HOME for the adb and the SDK.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
PROJECT="$REPO/examples/playground"
APP="$PROJECT/rn"
IOS_BUNDLE="dev.undra.playground.rn"
ANDROID_PACKAGE="dev.undra.playground.rn"
ANDROID_ACTIVITY=".MainActivity"

PLATFORM="${1:-}"
case "$PLATFORM" in ios|android) shift ;; -h|--help) PLATFORM=help ;; *) PLATFORM="" ;; esac
usage() { sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//'; }
die() { echo "rn-device-checks: $*" >&2; exit 1; }
[ "$PLATFORM" != help ] || { usage; exit 0; }
[ -n "$PLATFORM" ] || die "ios or android is required (see --help)"

BUILD=1 RUN=1 TARGET="" ABI="" EXPECT=10 TIMEOUT=240 RELEASE=(--release)
while [ $# -gt 0 ]; do
  case "$1" in
    --build-only) RUN=0; shift ;;
    --no-build) BUILD=0; shift ;;
    --target) TARGET="${2:?--target needs an id}"; shift 2 ;;
    --abi) ABI="${2:?--abi needs an Android ABI}"; shift 2 ;;
    --expect) EXPECT="${2:?}"; shift 2 ;;
    --timeout) TIMEOUT="${2:?}"; shift 2 ;;
    --debug-core) RELEASE=(); shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option $1 (see --help)" ;;
  esac
done

# shellcheck source=env.sh
[ -f "$HERE/env.sh" ] && . "$HERE/env.sh" >/dev/null 2>&1 || true
if [ -z "${DEVELOPER_DIR:-}" ] && [ -d "/Applications/Xcode.app/Contents/Developer" ] && [ "$(uname -s)" = Darwin ]; then
  case "$(xcode-select -p 2>/dev/null)" in */CommandLineTools) export DEVELOPER_DIR="/Applications/Xcode.app/Contents/Developer" ;; esac
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/undra-rn-checks.XXXXXX")"
BG_PIDS=()
cleanup() {
  local pid
  for pid in ${BG_PIDS[@]+"${BG_PIDS[@]}"}; do kill "$pid" >/dev/null 2>&1 || true; done
  rm -rf "$WORK"
}
trap cleanup EXIT

# build_core: the core for both phones (the iOS half only on macOS) and the UndraCore pod.
build_core() {
  UNDRA="${UNDRA:-$REPO/target/debug/undra}"
  if [ ! -x "$UNDRA" ]; then
    echo "== building the undra CLI"
    cargo build --manifest-path "$REPO/Cargo.toml" -p undra-cli
  fi
  echo "== core: undra build --platform rn ${RELEASE[*]-}"
  "$UNDRA" build -C "$PROJECT" --platform rn ${RELEASE[@]+"${RELEASE[@]}"}
}

install_js() {
  if [ ! -d "$APP/node_modules" ]; then
    echo "== npm ci in examples/playground/rn"
    (cd "$APP" && npm ci --no-audit --no-fund)
  fi
}

# verdict LOG: reads the app's lines from a device-log file; 0 when every check passed, 1 when one failed,
# 2 when there is no verdict yet.
verdict() {
  local line passed total
  line="$(grep -E 'UNDRA-RN CHECKS [0-9]+/[0-9]+ passed' "$1" | tail -n 1 || true)"
  [ -n "$line" ] || return 2
  passed="$(echo "$line" | sed -E 's/.*UNDRA-RN CHECKS ([0-9]+)\/([0-9]+) passed.*/\1/')"
  total="$(echo "$line" | sed -E 's/.*UNDRA-RN CHECKS ([0-9]+)\/([0-9]+) passed.*/\2/')"
  [ "$passed" = "$total" ] && [ "$total" -ge "$EXPECT" ] || return 1
}

# wait_for_verdict LOG: polls until the verdict line, its failure or the timeout; prints the evidence.
wait_for_verdict() {
  local log="$1" waited=0 rc
  while :; do
    rc=0; verdict "$log" || rc=$?
    [ "$rc" = 2 ] || break
    [ "$waited" -lt "$TIMEOUT" ] || break
    sleep 2; waited=$((waited + 2))
  done
  echo "== the app's lines"
  grep -E 'UNDRA-RN (CHECK|loaded|failed|error|stores|todos)' "$log" | sed -E 's/^.*(UNDRA-RN)/\1/' | sort -u -k1,3 -s || true
  if [ "$rc" = 0 ]; then
    echo "== PASS: $(grep -E 'UNDRA-RN CHECKS [0-9]+/[0-9]+ passed' "$log" | tail -n 1 | sed -E 's/^.*(UNDRA-RN)/\1/') (after ${waited} s)"
    return 0
  fi
  if [ "$rc" = 1 ]; then
    echo "== FAIL: not every check passed (need $EXPECT)" >&2
  else
    echo "== FAIL: no 'UNDRA-RN CHECKS' line within ${TIMEOUT} s of the launch" >&2
  fi
  echo "== last 40 lines of the device log ($log)" >&2
  tail -n 40 "$log" >&2 || true
  return 1
}

# ================================================================================================
# iOS
# ================================================================================================
ios() {
  [ "$(uname -s)" = Darwin ] || die "the iOS simulator needs macOS"
  local sim="${SIMULATOR:-iPhone 17 Pro}" udid="$TARGET"
  xcrun simctl list devices available -j >"$WORK/simctl.json"
  if [ -z "$udid" ]; then
    udid="$(node -e '
      const d = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).devices, want = process.argv[2];
      const all = Object.values(d).flat().filter((x) => x.isAvailable !== false);
      const named = all.filter((x) => x.name === want);
      const phones = all.filter((x) => /^iPhone/.test(x.name));
      const pick = named.find((x) => x.state === "Booted") ?? named[0] ?? phones.find((x) => x.state === "Booted") ?? phones[0];
      if (pick) console.log(pick.udid);' "$WORK/simctl.json" "$sim")"
    [ -n "$udid" ] || die "no available iPhone simulator (SIMULATOR=... or --target <udid>)"
  fi
  local derived="$APP/ios/build/DerivedData"
  local app="$derived/Build/Products/Release-iphonesimulator/UndraPlayground.app"

  if [ "$BUILD" = 1 ]; then
    build_core
    install_js
    echo "== pod install"
    # CocoaPods stops on a non-UTF-8 locale (an agent's or a cron job's shell often has none).
    case "${LC_ALL:-${LANG:-}}" in *UTF-8*|*utf8*) ;; *) export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ;; esac
    (cd "$APP/ios" && pod install)
    # The pod links the core with -force_load, which Xcode does not track as an input of the link step: a rebuilt
    # core would leave an up-to-date-looking app that still carries the old one. Dropping the app and its own
    # intermediates (not the Pods') makes the linker run again; the compile of a few app files is all it costs.
    rm -rf "$app" "$derived/Build/Intermediates.noindex/UndraPlayground.build"
    echo "== app: Release, for the simulator ($udid), active architecture only"
    # ONLY_ACTIVE_ARCH: the core's simulator slice has [ios] simulator_archs only (arm64 by default); a Release
    # configuration would otherwise build x86_64 too and fail to link it (docs/REACT_NATIVE.md, troubleshooting).
    xcodebuild -workspace "$APP/ios/UndraPlayground.xcworkspace" -scheme UndraPlayground -configuration Release \
      -destination "platform=iOS Simulator,id=$udid" -derivedDataPath "$derived" ONLY_ACTIVE_ARCH=YES ARCHS=arm64 \
      build >"$WORK/xcodebuild.log" 2>&1 \
      || { tail -n 40 "$WORK/xcodebuild.log"; die "xcodebuild failed"; }
    echo "== built $app"
  fi
  [ "$RUN" = 1 ] || return 0
  [ -d "$app" ] || die "no app at $app (run without --no-build)"

  local state
  state="$(node -e '
    const d = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).devices;
    for (const list of Object.values(d)) for (const x of list) if (x.udid === process.argv[2]) console.log(x.state);' "$WORK/simctl.json" "$udid")"
  if [ "$state" != Booted ]; then
    echo "== booting the simulator ($udid)"
    xcrun simctl boot "$udid"
    STARTED_SIM="$udid"
  fi
  xcrun simctl bootstatus "$udid" -b >/dev/null
  xcrun simctl terminate "$udid" "$IOS_BUNDLE" >/dev/null 2>&1 || true
  xcrun simctl install "$udid" "$app"

  # The log stream starts before the launch so the app's first lines are not missed.
  local log="$WORK/ios.log"
  : >"$log"
  xcrun simctl spawn "$udid" log stream --level debug --style compact --predicate 'eventMessage CONTAINS "UNDRA-RN"' >"$log" 2>&1 &
  BG_PIDS+=("$!")
  sleep 2
  echo "== launching $IOS_BUNDLE"
  xcrun simctl launch "$udid" "$IOS_BUNDLE" >/dev/null
  local rc=0
  wait_for_verdict "$log" || rc=$?
  xcrun simctl terminate "$udid" "$IOS_BUNDLE" >/dev/null 2>&1 || true
  [ -z "${STARTED_SIM:-}" ] || xcrun simctl shutdown "$STARTED_SIM" >/dev/null 2>&1 || true
  return "$rc"
}

# ================================================================================================
# Android
# ================================================================================================
android() {
  [ -n "${ANDROID_HOME:-}" ] || die "ANDROID_HOME is not set (docs/ONBOARDING.md)"
  local adb="$ANDROID_HOME/platform-tools/adb" serial="$TARGET"
  [ -x "$adb" ] || adb="$(command -v adb || true)"
  [ -n "$adb" ] || die "no adb under ANDROID_HOME ($ANDROID_HOME) or on PATH"

  if [ "$BUILD" = 0 ] || [ "$RUN" = 1 ]; then
    if [ -z "$serial" ]; then
      local found
      found="$("$adb" devices | awk 'NR>1 && $2=="device" {print $1}')"
      if [ "$RUN" = 1 ]; then
        [ -n "$found" ] || die "no device or emulator attached (boot one, or --target <serial>)"
        [ "$(echo "$found" | wc -l | tr -d ' ')" = 1 ] || die "several devices attached ($(echo "$found" | tr '\n' ' ')): pass --target <serial>"
      fi
      serial="$found"
    fi
  fi
  if [ -z "$ABI" ] && [ -n "$serial" ]; then
    ABI="$("$adb" -s "$serial" shell getprop ro.product.cpu.abi 2>/dev/null | tr -d '\r' || true)"
  fi

  local apk="$APP/android/app/build/outputs/apk/release/app-release.apk"
  if [ "$BUILD" = 1 ]; then
    build_core
    install_js
    echo "== app: assembleRelease${ABI:+ for $ABI}"
    # The app's own native library (libappmodules.so, which holds the module) is built for ABI alone: on CI the one
    # the emulator runs. The core's jniLibs carry every ABI of [android] abis either way.
    (cd "$APP/android" && ./gradlew --no-daemon :app:assembleRelease ${ABI:+-PreactNativeArchitectures="$ABI"}) \
      >"$WORK/gradle.log" 2>&1 || { tail -n 60 "$WORK/gradle.log"; die "gradle assembleRelease failed"; }
    echo "== built $apk"
  fi
  [ "$RUN" = 1 ] || return 0
  [ -f "$apk" ] || die "no APK at $apk (run without --no-build)"

  "$adb" -s "$serial" get-state >/dev/null 2>&1 || die "adb cannot reach $serial"
  "$adb" -s "$serial" install -r -d "$apk" >/dev/null
  "$adb" -s "$serial" shell am force-stop "$ANDROID_PACKAGE" >/dev/null 2>&1 || true
  "$adb" -s "$serial" logcat -c
  local log="$WORK/android.log"
  : >"$log"
  "$adb" -s "$serial" logcat -v brief ReactNativeJS:V '*:S' >"$log" 2>&1 &
  BG_PIDS+=("$!")
  echo "== launching $ANDROID_PACKAGE on $serial"
  "$adb" -s "$serial" shell am start -W -n "$ANDROID_PACKAGE/$ANDROID_ACTIVITY" >/dev/null
  local rc=0
  wait_for_verdict "$log" || rc=$?
  "$adb" -s "$serial" shell am force-stop "$ANDROID_PACKAGE" >/dev/null 2>&1 || true
  return "$rc"
}

STARTED_SIM=""
"$PLATFORM"
