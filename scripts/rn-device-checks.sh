#!/usr/bin/env bash
# The React Native playground app's on-device checks on the iOS simulator and an Android emulator or phone: build
# the core and the Release app, install it, launch it, and wait for the app's own verdict line in the device log,
#
#     UNDRA-RN CHECKS 16/16 passed            RN01..RN10, the boundary; RN11..RN16, the default ports through the
#                                             core (examples/playground/rn/src/checks.ts, ADR-038 amendment B)
#
# then drive what only the outside of the app can do, each a CHECK line of its own (ADR-038 amendment B, B10):
#
#     RN17  SecureStore: the app's secret is not in its files as plain text, while a Kv marker written next to it is
#           (the scan works): the simulator's data container, or /data/data/<package> through `su` on Android
#     RN18  Kv survives the process: the app is killed and relaunched, and reads the token the first launch wrote
#     RN19  Lifecycle: the core sees `background` (Home on Android, another app on iOS) and `active` again
#     RN20  Connectivity (Android): the core sees online = false in airplane mode and true after it
#
# and print the total, which is the only thing that counts:
#
#     UNDRA-RN CHECKS 20/20 passed            (19/19 on iOS: the simulator has no airplane mode)
#
# The process exits 0 only when every check passed, and prints the CHECK lines (and the device-log tail when the
# app's verdict is missing or failed). The Http check (RN14) calls a loopback server this script runs on
# 127.0.0.1:8737 (the simulator shares the Mac's loopback; Android reaches it through `adb reverse`). The
# .github/workflows/rn-devices.yml jobs run exactly this.
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
#   --expect N        the number of the app's own checks that must pass (default 16, RN01..RN16)
#   --app-only        stop after the app's own verdict: no restart, scan, lifecycle or airplane-mode phase
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

BUILD=1 RUN=1 TARGET="" ABI="" EXPECT=16 TIMEOUT=240 RELEASE=(--release) PHASES=1
while [ $# -gt 0 ]; do
  case "$1" in
    --build-only) RUN=0; shift ;;
    --no-build) BUILD=0; shift ;;
    --target) TARGET="${2:?--target needs an id}"; shift 2 ;;
    --abi) ABI="${2:?--abi needs an Android ABI}"; shift 2 ;;
    --expect) EXPECT="${2:?}"; shift 2 ;;
    --timeout) TIMEOUT="${2:?}"; shift 2 ;;
    --debug-core) RELEASE=(); shift ;;
    --app-only) PHASES=0; shift ;;
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
CLEANUPS=()
cleanup() {
  local pid step
  for step in ${CLEANUPS[@]+"${CLEANUPS[@]}"}; do
    eval "$step" >/dev/null 2>&1 || true
  done
  for pid in ${BG_PIDS[@]+"${BG_PIDS[@]}"}; do
    kill "$pid" >/dev/null 2>&1 || true
    { wait "$pid"; } >/dev/null 2>&1 || true   # reaped here, so the shell does not print "Terminated"
  done
  rm -rf "$WORK"
}
trap cleanup EXIT

# ---- the phases outside the app (RN17..RN20) -----------------------------------------------------------------

LOOPBACK_PORT=8737
APP_PASSED=0 APP_TOTAL=0 OUT_PASSED=0 OUT_TOTAL=0

# start_loopback: the server of the Http check (RN14): GET /undra-rn-check answers its request's headers as JSON.
start_loopback() {
  if (exec 3<>"/dev/tcp/127.0.0.1/$LOOPBACK_PORT") 2>/dev/null; then
    die "127.0.0.1:$LOOPBACK_PORT is taken: the loopback server of the Http check needs it"
  fi
  node -e '
    const http = require("http");
    http.createServer((req, res) => {
      if (req.url.startsWith("/undra-rn-check")) {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ ok: true, path: req.url, headers: req.headers }));
      } else {
        res.writeHead(404);
        res.end();
      }
    }).listen(Number(process.argv[1]), "127.0.0.1");' "$LOOPBACK_PORT" >"$WORK/loopback.log" 2>&1 &
  BG_PIDS+=("$!")
  local waited=0
  until (exec 3<>"/dev/tcp/127.0.0.1/$LOOPBACK_PORT") 2>/dev/null; do
    sleep 0.2; waited=$((waited + 1))
    [ "$waited" -lt 50 ] || die "the loopback server did not start: $(cat "$WORK/loopback.log")"
  done
  echo "== loopback server on 127.0.0.1:$LOOPBACK_PORT"
}

# lines LOG: the number of lines LOG has now (a mark to search after).
lines() { wc -l <"$1" | tr -d ' '; }

# wait_line LOG FROM REGEX SECONDS: prints the first line after line FROM matching REGEX; 1 if none in time.
wait_line() {
  local log="$1" from="$2" regex="$3" limit="$4" waited=0 found
  while :; do
    found="$(tail -n +"$((from + 1))" "$log" | grep -E "$regex" | head -n 1 || true)"
    if [ -n "$found" ]; then
      echo "$found" | sed -E 's/^.*(UNDRA-RN)/\1/'
      return 0
    fi
    [ "$waited" -lt "$limit" ] || return 1
    sleep 1; waited=$((waited + 1))
  done
}

# outside ID TITLE OK DETAIL: one check made from outside the app.
outside() {
  OUT_TOTAL=$((OUT_TOTAL + 1))
  if [ "$3" = 0 ]; then
    OUT_PASSED=$((OUT_PASSED + 1))
    echo "UNDRA-RN CHECK $1 PASS $2: $4"
  else
    echo "UNDRA-RN CHECK $1 FAIL $2: $4"
  fi
}

# app_verdict LOG: the app's own counts from its verdict line, into APP_PASSED and APP_TOTAL.
app_verdict() {
  local line
  line="$(grep -E 'UNDRA-RN CHECKS [0-9]+/[0-9]+ passed' "$1" | tail -n 1 || true)"
  APP_PASSED="$(echo "$line" | sed -E 's/.*UNDRA-RN CHECKS ([0-9]+)\/([0-9]+) passed.*/\1/')"
  APP_TOTAL="$(echo "$line" | sed -E 's/.*UNDRA-RN CHECKS ([0-9]+)\/([0-9]+) passed.*/\2/')"
}

# scan_verdict NONCE SECRET_HITS KV_HITS: RN17 from the two scans of the app's files.
scan_verdict() {
  local nonce="$1" secret_hits="$2" kv_hits="$3"
  if [ -z "$nonce" ]; then
    outside RN17 "SecureStore is not plain text in the app's files" 1 "the app logged no marker nonce (RN12)"
  elif [ "$kv_hits" -lt 1 ]; then
    outside RN17 "SecureStore is not plain text in the app's files" 1 "the scan did not find the Kv marker either: it cannot see the app's files"
  elif [ "$secret_hits" -ne 0 ]; then
    outside RN17 "SecureStore is not plain text in the app's files" 1 "UNDRA-SECRET-$nonce found in $secret_hits file(s)"
  else
    outside RN17 "SecureStore is not plain text in the app's files" 0 "UNDRA-SECRET-$nonce in no file; the Kv marker next to it in $kv_hits (the scan sees the files)"
  fi
}

# lifecycle_back LOG FROM BACKGROUND_LINE: the first `state=active` report after the background one (a later
# report number), or nothing within 30 s.
lifecycle_back() {
  local log="$1" from="$2" bg="$3" n line waited=0
  [ -n "$bg" ] || return 0
  n="$(echo "$bg" | sed -E 's/.*reports=([0-9]+).*/\1/')"
  while [ "$waited" -lt 30 ]; do
    while IFS= read -r line; do
      if [ "$(echo "$line" | sed -E 's/.*reports=([0-9]+).*/\1/')" -gt "$n" ]; then
        echo "$line" | sed -E 's/^.*(UNDRA-RN)/\1/'
        return 0
      fi
    done < <(tail -n +"$((from + 1))" "$log" | grep -E 'UNDRA-RN LIFECYCLE state=active reports=[0-9]+' || true)
    sleep 1; waited=$((waited + 1))
  done
}

# total: the line that counts, and the exit status.
total() {
  local passed=$((APP_PASSED + OUT_PASSED)) all=$((APP_TOTAL + OUT_TOTAL))
  echo "== UNDRA-RN CHECKS $passed/$all passed"
  [ "$passed" = "$all" ] && [ "$APP_TOTAL" -ge "$EXPECT" ]
}

# build_core: the core for both phones (the iOS half only on macOS) and its pod (PlaygroundCore, ADR-044).
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
  grep -E 'UNDRA-RN (CHECK|loaded|defaults|KV|failed|error|stores|todos)' "$log" | sed -E 's/^.*(UNDRA-RN)/\1/' | sort -u -k1,3 -s || true
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
    # The core is a vendored static library that a script phase copies into the build directory, and a rebuilt
    # core has been seen to leave an up-to-date-looking app that still carries the old one. Dropping the app and
    # its own intermediates (not the Pods') makes the linker run again; the compile of a few app files is all it
    # costs.
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
  start_loopback
  echo "== launching $IOS_BUNDLE"
  xcrun simctl launch "$udid" "$IOS_BUNDLE" >/dev/null
  local rc=0
  wait_for_verdict "$log" || rc=$?
  app_verdict "$log"
  if [ "$rc" = 0 ] && [ "$PHASES" = 1 ]; then
    ios_phases "$udid" "$log"
    total || rc=1
  fi
  xcrun simctl terminate "$udid" "$IOS_BUNDLE" >/dev/null 2>&1 || true
  [ -z "${STARTED_SIM:-}" ] || xcrun simctl shutdown "$STARTED_SIM" >/dev/null 2>&1 || true
  return "$rc"
}

# ios_phases UDID LOG: RN17..RN19 on the simulator (it has no airplane mode: no RN20).
ios_phases() {
  local udid="$1" log="$2" nonce data secret_hits kv_hits wrote previous mark bg fg
  echo "== outside the app"
  # RN17: the simulator's data container is a directory of this Mac. The Keychain is not in it (it is the
  # simulator's own database): a secret written there must not appear in the app's files.
  nonce="$(grep -E 'UNDRA-RN CHECK RN12 PASS' "$log" | tail -n 1 | sed -E 's/.*marker nonce ([a-z0-9]+).*/\1/')"
  data="$(xcrun simctl get_app_container "$udid" "$IOS_BUNDLE" data)"
  # (`|| true` inside: grep finds nothing on purpose for the secret, which pipefail would make fatal.)
  secret_hits="$({ grep -r -l -a -F "UNDRA-SECRET-$nonce" "$data" 2>/dev/null || true; } | wc -l | tr -d ' ')"
  kv_hits="$({ grep -r -l -a -F "UNDRA-KVMARK-$nonce" "$data" 2>/dev/null || true; } | wc -l | tr -d ' ')"
  scan_verdict "$nonce" "$secret_hits" "$kv_hits"

  # RN18: kill the app, launch it again; it reads what the first launch wrote.
  wrote="$(grep -E 'UNDRA-RN KV wrote=' "$log" | tail -n 1 | sed -E 's/.*wrote=([a-z0-9]+).*/\1/')"
  xcrun simctl terminate "$udid" "$IOS_BUNDLE" >/dev/null 2>&1 || true
  sleep 1
  mark="$(lines "$log")"
  xcrun simctl launch "$udid" "$IOS_BUNDLE" >/dev/null
  previous="$(wait_line "$log" "$mark" 'UNDRA-RN KV previous=' 90 | sed -E 's/.*previous=([^ ]+).*/\1/' || true)"
  if [ -n "$wrote" ] && [ "$previous" = "$wrote" ]; then
    outside RN18 "Kv survives the process" 0 "the first launch wrote $wrote; after the kill the second read $previous"
  else
    outside RN18 "Kv survives the process" 1 "the first launch wrote '${wrote}', the second read '${previous}'"
  fi

  # RN19: another app in front (Settings), then this one again. The Device store's signals are no_coalesce, so
  # the background report shows even if the app's JavaScript only drains it once it is back.
  wait_line "$log" "$mark" 'UNDRA-RN LIFECYCLE state=active' 60 >/dev/null || true
  mark="$(lines "$log")"
  xcrun simctl launch "$udid" com.apple.Preferences >/dev/null 2>&1 || true
  sleep 4
  xcrun simctl launch "$udid" "$IOS_BUNDLE" >/dev/null
  bg="$(wait_line "$log" "$mark" 'UNDRA-RN LIFECYCLE state=background' 30 || true)"
  fg="$(lifecycle_back "$log" "$mark" "$bg")"
  if [ -n "$bg" ] && [ -n "$fg" ]; then
    outside RN19 "Lifecycle reaches the core" 0 "Settings in front: '$bg'; back: '$fg'"
  else
    outside RN19 "Lifecycle reaches the core" 1 "background: '${bg}', active again: '${fg}'"
  fi
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
  start_loopback
  # The device's 127.0.0.1:8737 is this machine's (the emulator's own loopback is not the host's).
  "$adb" -s "$serial" reverse "tcp:$LOOPBACK_PORT" "tcp:$LOOPBACK_PORT" >/dev/null
  CLEANUPS+=("'$adb' -s '$serial' reverse --remove tcp:$LOOPBACK_PORT")
  echo "== launching $ANDROID_PACKAGE on $serial"
  "$adb" -s "$serial" shell am start -W -n "$ANDROID_PACKAGE/$ANDROID_ACTIVITY" >/dev/null
  local rc=0
  wait_for_verdict "$log" || rc=$?
  app_verdict "$log"
  if [ "$rc" = 0 ] && [ "$PHASES" = 1 ]; then
    android_phases "$adb" "$serial" "$log"
    total || rc=1
  fi
  "$adb" -s "$serial" shell am force-stop "$ANDROID_PACKAGE" >/dev/null 2>&1 || true
  return "$rc"
}

# android_phases ADB SERIAL LOG: RN17..RN20 on the emulator or phone.
android_phases() {
  local adb="$1" serial="$2" log="$3" nonce secret_hits kv_hits wrote previous mark bg fg off on
  local app_dir="/data/data/$ANDROID_PACKAGE"
  echo "== outside the app"
  # RN17: the app's private directory needs root to read from outside (a userdebug emulator image has `su`; a
  # release build is not debuggable, so `run-as` cannot).
  nonce="$(grep -E 'UNDRA-RN CHECK RN12 PASS' "$log" | tail -n 1 | sed -E 's/.*marker nonce ([a-z0-9]+).*/\1/')"
  if "$adb" -s "$serial" shell "su 0 true" >/dev/null 2>&1; then
    secret_hits="$({ "$adb" -s "$serial" shell "su 0 grep -r -l -F 'UNDRA-SECRET-$nonce' $app_dir" 2>/dev/null || true; } | tr -d '\r' | { grep -c . || true; })"
    kv_hits="$({ "$adb" -s "$serial" shell "su 0 grep -r -l -F 'UNDRA-KVMARK-$nonce' $app_dir" 2>/dev/null || true; } | tr -d '\r' | { grep -c . || true; })"
    scan_verdict "$nonce" "$secret_hits" "$kv_hits"
  else
    outside RN17 "SecureStore is not plain text in the app's files" 1 "no root (su) on $serial: the app's files cannot be read from outside"
  fi

  # RN18: kill the process, start the app again; it reads what the first launch wrote.
  wrote="$(grep -E 'UNDRA-RN KV wrote=' "$log" | tail -n 1 | sed -E 's/.*wrote=([a-z0-9]+).*/\1/')"
  "$adb" -s "$serial" shell am force-stop "$ANDROID_PACKAGE" >/dev/null 2>&1 || true
  mark="$(lines "$log")"
  "$adb" -s "$serial" shell am start -W -n "$ANDROID_PACKAGE/$ANDROID_ACTIVITY" >/dev/null
  previous="$(wait_line "$log" "$mark" 'UNDRA-RN KV previous=' 90 | sed -E 's/.*previous=([^ ]+).*/\1/' || true)"
  if [ -n "$wrote" ] && [ "$previous" = "$wrote" ]; then
    outside RN18 "Kv survives the process" 0 "the first launch wrote $wrote; after force-stop the second read $previous"
  else
    outside RN18 "Kv survives the process" 1 "the first launch wrote '${wrote}', the second read '${previous}'"
  fi

  # RN19: Home, then the app again. React Native pauses the JS timers of a backgrounded Android app, so the
  # app's mirror applies the core's background report when it is back (the Device store's signals are
  # no_coalesce: each report is applied, in order); the core itself received it at Home.
  wait_line "$log" "$mark" 'UNDRA-RN LIFECYCLE state=active' 60 >/dev/null || true
  mark="$(lines "$log")"
  "$adb" -s "$serial" shell input keyevent KEYCODE_HOME
  sleep 4
  "$adb" -s "$serial" shell am start -n "$ANDROID_PACKAGE/$ANDROID_ACTIVITY" >/dev/null
  bg="$(wait_line "$log" "$mark" 'UNDRA-RN LIFECYCLE state=background' 30 || true)"
  fg="$(lifecycle_back "$log" "$mark" "$bg")"
  if [ -n "$bg" ] && [ -n "$fg" ]; then
    outside RN19 "Lifecycle reaches the core" 0 "Home: '$bg'; back: '$fg'"
  else
    outside RN19 "Lifecycle reaches the core" 1 "background: '${bg}', active again: '${fg}'"
  fi

  # RN20: real airplane mode, then out of it (switched off on any exit).
  CLEANUPS+=("'$adb' -s '$serial' shell cmd connectivity airplane-mode disable")
  mark="$(lines "$log")"
  "$adb" -s "$serial" shell cmd connectivity airplane-mode enable
  off="$(wait_line "$log" "$mark" 'UNDRA-RN CONNECTIVITY online=false' 60 || true)"
  mark="$(lines "$log")"
  "$adb" -s "$serial" shell cmd connectivity airplane-mode disable
  on="$(wait_line "$log" "$mark" 'UNDRA-RN CONNECTIVITY online=true' 90 || true)"
  if [ -n "$off" ] && [ -n "$on" ]; then
    outside RN20 "Connectivity follows airplane mode" 0 "on: '$off'; off: '$on'"
  else
    outside RN20 "Connectivity follows airplane mode" 1 "airplane mode on: '${off}', off: '${on}'"
  fi
}

STARTED_SIM=""
"$PLATFORM"
