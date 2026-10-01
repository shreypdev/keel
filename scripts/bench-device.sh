#!/usr/bin/env bash
# The device benchmark: the blueprint's section 14 operations (docs/blueprint.html), measured the way an app
# experiences them, on an iPhone (or the simulator), an Android phone (or the emulator) or in Chromium, with
# the ADR-031 drain experiment, a cold start and a JSON file for the run. It builds the playground core and app for
# the target, drives the app's bench hooks (XCUITest, an instrumented test, Playwright: the harness each platform's
# smoke test already uses), writes bench/results/device/<date>-<target>.json and renders the numbers into the device
# section of bench/RESULTS.md.
#
#   scripts/bench-device.sh --device ios                      the iPhone 17 Pro simulator (boots it if it is not running)
#   scripts/bench-device.sh --device ios --target <udid>      a simulator by UDID, or an iPhone attached over USB
#   scripts/bench-device.sh --device android                  the one device or emulator adb sees; else boots the `undra` AVD
#   scripts/bench-device.sh --device android --target <serial>
#   scripts/bench-device.sh --device web                      headless Chromium (Playwright)
#
# Options
#   --target ID       the simulator UDID, the iPhone's UDID, the adb serial (default: see above); ignored for web
#   --runs N          measure N times in a row, each its own file (`<date>-<target>.json`, then `...-run2.json`, ...)
#   --cold-runs N     fresh-process cold starts after the full run (default 10)
#   --quick           a fraction of a second of each part: checks the plumbing, writes nothing under bench/ and does not render
#   --tag NAME        put -NAME in the result file's name
#   --out DIR         write the result files here instead of bench/results/device
#   --no-build        use the app and core built by an earlier run
#   --no-render       do not touch bench/RESULTS.md
#   --keep-booted     leave a simulator or emulator this script started running
#
# A PHYSICAL DEVICE. The row is labelled a device only when the target is one, and that is what turns the verdict
# column of RESULTS.md on. iPhone: plug it in, trust the computer, enable Developer Mode, and set UNDRA_IOS_TEAM to your
# development team id (`UNDRA_IOS_TEAM=ABCDE12345 scripts/bench-device.sh --device ios --target <udid>`; the id is in
# `xcrun devicectl list devices`). Android: enable USB debugging and run
# `scripts/bench-device.sh --device android --target <serial>` (`adb devices` lists it). Keep the phone cool, charged
# and out of low-power mode (the file records the thermal state and the mode), screen on and unlocked.
#
# Environment: SIMULATOR (iOS simulator name, default "iPhone 17 Pro"), AVD (default "undra"), UNDRA (the CLI binary),
# UNDRA_IOS_TEAM, UNDRA_BROWSER_CHANNEL=chrome (use an installed Chrome instead of Playwright's Chromium).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
PROJECT="$REPO/examples/playground"
REPORT=(node "$HERE/bench-device-report.mjs")
BUNDLE="dev.undra.playground"

DEVICE="" TARGET="" RUNS=1 COLD_RUNS=10 QUICK=0 TAG="" OUT="" BUILD=1 RENDER=1 KEEP_BOOTED=0
CMDLINE="scripts/bench-device.sh $*"

usage() { sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//'; }
die() { echo "bench-device: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --device) DEVICE="${2:?--device needs ios, android or web}"; shift 2 ;;
    --target) TARGET="${2:?--target needs an id}"; shift 2 ;;
    --runs) RUNS="${2:?}"; shift 2 ;;
    --cold-runs) COLD_RUNS="${2:?}"; shift 2 ;;
    --quick) QUICK=1; shift ;;
    --tag) TAG="${2:?}"; shift 2 ;;
    --out) OUT="${2:?}"; shift 2 ;;
    --no-build) BUILD=0; shift ;;
    --no-render) RENDER=0; shift ;;
    --keep-booted) KEEP_BOOTED=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option $1 (see --help)" ;;
  esac
done
case "$DEVICE" in ios|android|web) ;; *) die "--device ios|android|web is required (see --help)" ;; esac

# ---- toolchain -----------------------------------------------------------------------------------
# shellcheck source=env.sh
. "$HERE/env.sh"
if [ -z "${DEVELOPER_DIR:-}" ] && [ -d "/Applications/Xcode.app/Contents/Developer" ]; then
  export DEVELOPER_DIR="/Applications/Xcode.app/Contents/Developer"
fi
command -v node >/dev/null || die "node 22+ is needed (brew install node)"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/undra-bench-device.XXXXXX")"
STARTED_SIMULATOR="" STARTED_EMULATOR=""
cleanup() {
  if [ "$KEEP_BOOTED" = 0 ]; then
    [ -z "$STARTED_SIMULATOR" ] || xcrun simctl shutdown "$STARTED_SIMULATOR" >/dev/null 2>&1 || true
    [ -z "$STARTED_EMULATOR" ] || "$ADB" -s "$STARTED_EMULATOR" emu kill >/dev/null 2>&1 || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

if [ "$QUICK" = 1 ]; then
  [ -n "$OUT" ] || OUT="$WORK/results"
  RENDER=0
  [ -n "$TAG" ] || TAG="quick"
fi
[ -n "$OUT" ] || OUT="$REPO/bench/results/device"

UNDRA="${UNDRA:-$REPO/target/debug/undra}"
if [ "$BUILD" = 1 ] && [ ! -x "$UNDRA" ]; then
  echo "== building the undra CLI"
  cargo build --manifest-path "$REPO/Cargo.toml" -p undra-cli
fi
[ -x "$UNDRA" ] || die "no undra CLI at $UNDRA (cargo build -p undra-cli)"

COMMIT="$(git -C "$REPO" rev-parse --short HEAD)"
DIRTY_FLAG=()
# What the files record as "uncommitted changes": the tree apart from the files this script writes.
if [ -n "$(git -C "$REPO" status --porcelain --untracked-files=no -- . ':(exclude)bench/results/device' ':(exclude)bench/RESULTS.md')" ]; then
  DIRTY_FLAG=(--dirty)
fi

loadavg() { sysctl -n vm.loadavg | awk '{print $2}'; }
host_cpu() { sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -m; }
host_os() { echo "macOS $(sw_vers -productVersion) ($(sw_vers -buildVersion))"; }
slug() { echo "$1" | tr '[:upper:]' '[:lower:]' | sed -E 's/[^a-z0-9]+/-/g; s/^-+//; s/-+$//'; }
field() { "${REPORT[@]}" get "$1" "$2"; }

# finalize RAW SLUG KIND LABEL TAG BUILD_CORE BUILD_APP LOAD_BEFORE LOAD_AFTER [NOTE]
finalize_run() {
  local raw="$1" target_slug="$2" kind="$3" label="$4" tag="$5" core="$6" app="$7" l1="$8" l2="$9" note="${10:-}"
  local host=()
  if [ "$kind" != device ]; then
    host=(--host-cpu "$(host_cpu)" --host-os "$(host_os)" --host-arch "$(uname -m)" --load-before "$l1" --load-after "$l2")
  fi
  "${REPORT[@]}" finalize --raw "$raw" --platform "$DEVICE" --target "$target_slug" --kind "$kind" --label "$label" \
    --commit "$COMMIT" ${DIRTY_FLAG[@]+"${DIRTY_FLAG[@]}"} --command "$CMDLINE" --core "$core" --app "$app" \
    --out "$OUT" ${tag:+--tag "$tag"} ${note:+--note "$note"} ${host[@]+"${host[@]}"}
}

# run_tag N: the tag of the Nth run, then the user's tag
run_tag() {
  local n="$1" t=""
  [ "$n" = 1 ] || t="run$n"
  if [ -n "$TAG" ]; then t="${t:+$t-}$TAG"; fi
  echo "$t"
}

# ================================================================================================
# iOS
# ================================================================================================
bench_ios() {
  local sim="${SIMULATOR:-iPhone 17 Pro}" udid="$TARGET" kind name dest
  xcrun simctl list devices available -j >"$WORK/simctl.json"
  if [ -z "$udid" ]; then
    udid="$(node -e '
      const d = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).devices, want = process.argv[2];
      const all = Object.values(d).flat().filter((x) => x.name === want);
      const pick = all.find((x) => x.state === "Booted") ?? all[0];
      if (pick) console.log(pick.udid);' "$WORK/simctl.json" "$sim")"
    [ -n "$udid" ] || die "no simulator called \"$sim\" (SIMULATOR=... or --target <udid>)"
  fi
  local info
  info="$(node -e '
    const d = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).devices;
    for (const [runtime, list] of Object.entries(d)) for (const x of list) if (x.udid === process.argv[2]) {
      // The name of the device type, not of the simulator: a simulator cloned for this run is still an "iPhone 17 Pro".
      const type = (x.deviceTypeIdentifier ?? "").replace(/.*SimDeviceType\./, "").replace(/-/g, " ");
      console.log([type || x.name, x.state].join("|"));
    }' "$WORK/simctl.json" "$udid")"
  export UNDRA_LINK_CORE=1
  local derived="$PROJECT/ios/DerivedData/bench" team=()
  if [ -n "$info" ]; then
    kind=simulator
    name="${info%%|*}"; local state="${info##*|}"
    if [ "$state" != Booted ]; then
      echo "== booting $name ($udid)"
      xcrun simctl boot "$udid"
      STARTED_SIMULATOR="$udid"
    fi
    xcrun simctl bootstatus "$udid" -b >/dev/null
    dest="platform=iOS Simulator,id=$udid"
  else
    kind=device
    [ -n "${UNDRA_IOS_TEAM:-}" ] || die "a physical iPhone needs UNDRA_IOS_TEAM=<your development team id> (xcrun devicectl list devices shows the device; the team is in Xcode > Settings > Accounts)"
    xcrun devicectl list devices 2>/dev/null | grep -qi "$udid" || die "$udid is neither a simulator nor a device xcrun devicectl knows (is it unlocked and trusted?)"
    name="iPhone"
    dest="platform=iOS,id=$udid"
    team=(-allowProvisioningUpdates DEVELOPMENT_TEAM="$UNDRA_IOS_TEAM")
  fi

  if [ "$BUILD" = 1 ]; then
    echo "== core: undra build --platform ios --release"
    "$UNDRA" build -C "$PROJECT" --platform ios --release
    echo "== app + UI tests: Release, for $dest"
    xcodebuild build-for-testing -project "$PROJECT/ios/PlaygroundApp.xcodeproj" -scheme PlaygroundApp -configuration Release \
      -destination "$dest" -derivedDataPath "$derived" ${team[@]+"${team[@]}"} >"$WORK/build.log" 2>&1 \
      || { tail -30 "$WORK/build.log"; die "xcodebuild build-for-testing failed (log: $WORK/build.log)"; }
  fi

  local n
  for n in $(seq 1 "$RUNS"); do
    local log="$WORK/test-$n.log" bundle="$WORK/result-$n.xcresult" l1 l2
    rm -rf "$bundle"
    l1="$(loadavg)"
    echo "== run $n of $RUNS: the full benchmark, then $COLD_RUNS cold launches (load average $l1)"
    local test_env=(TEST_RUNNER_UNDRA_BENCH=1 TEST_RUNNER_UNDRA_BENCH_COLD_RUNS="$COLD_RUNS")
    [ "$QUICK" = 1 ] && test_env+=(TEST_RUNNER_UNDRA_BENCH_QUICK=1)
    env "${test_env[@]}" \
      xcodebuild test-without-building -project "$PROJECT/ios/PlaygroundApp.xcodeproj" -scheme PlaygroundApp -configuration Release \
      -destination "$dest" -derivedDataPath "$derived" -only-testing:PlaygroundAppUITests/PlaygroundBenchTests \
      -parallel-testing-enabled NO -resultBundlePath "$bundle" ${team[@]+"${team[@]}"} >"$log" 2>&1 || {
        grep -E "error:|failed|Test Case" "$log" | tail -20; die "the benchmark test failed (log: $log)"; }
    l2="$(loadavg)"
    xcrun xcresulttool export attachments --path "$bundle" --output-path "$WORK/att-$n" >/dev/null 2>&1 || true
    "${REPORT[@]}" assemble --platform ios --log "$log" --attachments "$WORK/att-$n" --out "$WORK/raw-$n.json"
    local model osv label slug_
    model="$(field "$WORK/raw-$n.json" device.model)"; osv="$(field "$WORK/raw-$n.json" device.os)"
    if [ "$kind" = simulator ]; then
      label="$name simulator ($osv, $(field "$WORK/raw-$n.json" device.arch) on $(host_cpu))"
      slug_="ios-simulator-$(slug "$name")"
    else
      label="iPhone $model ($osv)"
      slug_="ios-device-$(slug "$model")"
    fi
    finalize_run "$WORK/raw-$n.json" "$slug_" "$kind" "$label" "$(run_tag "$n")" \
      "release (LTO fat), static XCFramework slice" "Release (-O, whole-module); UI test launches in benchmark mode" "$l1" "$l2"
  done
}

# ================================================================================================
# Android
# ================================================================================================
bench_android() {
  [ -n "${ANDROID_HOME:-}" ] || die "ANDROID_HOME is not set (docs/ONBOARDING.md)"
  ADB="$ANDROID_HOME/platform-tools/adb"
  local serial="$TARGET" avd="${AVD:-undra}"
  if [ -z "$serial" ]; then
    local found
    found="$("$ADB" devices | awk 'NR>1 && $2=="device" {print $1}')"
    local physical emulators
    physical="$(echo "$found" | grep -v '^emulator-' || true)"
    emulators="$(echo "$found" | grep '^emulator-' || true)"
    if [ -n "$physical" ]; then
      [ "$(echo "$physical" | wc -l)" = 1 ] || die "several devices attached ($(echo "$physical" | tr '\n' ' ')): pass --target <serial>"
      serial="$physical"
    elif [ -n "$emulators" ]; then
      [ "$(echo "$emulators" | wc -l)" = 1 ] || die "several emulators running ($(echo "$emulators" | tr '\n' ' ')): pass --target <serial>"
      serial="$emulators"
    else
      echo "== no device attached: booting the \"$avd\" AVD headless"
      local port=5556
      while "$ADB" devices | grep -q "emulator-$port"; do port=$((port + 2)); done
      nohup "$ANDROID_HOME/emulator/emulator" -avd "$avd" -read-only -port "$port" -no-snapshot-load -no-snapshot-save -no-window \
        -no-audio -no-boot-anim -gpu swiftshader_indirect >"$WORK/emulator.log" 2>&1 &
      serial="emulator-$port"
      STARTED_EMULATOR="$serial"
      local waited=0
      until [ "$("$ADB" -s "$serial" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ]; do
        sleep 2; waited=$((waited + 2))
        [ "$waited" -lt 240 ] || { tail -5 "$WORK/emulator.log"; die "the emulator did not boot within 240 s (is the AVD already running? pass --target <serial>)"; }
      done
    fi
  fi
  export ANDROID_SERIAL="$serial"
  local kind=device
  case "$serial" in emulator-*) kind=emulator ;; esac
  "$ADB" -s "$serial" get-state >/dev/null 2>&1 || die "adb cannot reach $serial"
  local abi model
  abi="$("$ADB" -s "$serial" shell getprop ro.product.cpu.abi | tr -d '\r')"
  model="$("$ADB" -s "$serial" shell getprop ro.product.model | tr -d '\r')"

  if [ "$BUILD" = 1 ]; then
    echo "== core: undra build --platform android --release"
    "$UNDRA" build -C "$PROJECT" --platform android --release
    echo "== app + instrumented test: the benchmark build type"
    (cd "$PROJECT/android" && ./gradlew :app:assembleBenchmark :app:assembleBenchmarkAndroidTest --console=plain -q) \
      || die "gradle failed"
  fi
  local app_apk="$PROJECT/android/app/build/outputs/apk/benchmark/app-benchmark.apk"
  local test_apk="$PROJECT/android/app/build/outputs/apk/androidTest/benchmark/app-benchmark-androidTest.apk"
  [ -f "$app_apk" ] && [ -f "$test_apk" ] || die "no benchmark APKs (run without --no-build)"
  "$ADB" -s "$serial" install -r -t "$app_apk" >/dev/null 2>&1 || { "$ADB" -s "$serial" uninstall "$BUNDLE" >/dev/null 2>&1 || true; "$ADB" -s "$serial" install -t "$app_apk" >/dev/null; }
  "$ADB" -s "$serial" install -r -t "$test_apk" >/dev/null
  # What a user's device does after a few runs of an app: the code is compiled ahead of time, not interpreted and jitted.
  "$ADB" -s "$serial" shell cmd package compile -m speed -f "$BUNDLE" >/dev/null
  "$ADB" -s "$serial" shell cmd package compile -m speed -f "$BUNDLE.test" >/dev/null
  if [ "$kind" = device ]; then
    "$ADB" -s "$serial" shell input keyevent KEYCODE_WAKEUP >/dev/null 2>&1 || true
    "$ADB" -s "$serial" shell svc power stayon usb >/dev/null 2>&1 || true
  fi

  instrument() { # MODE OUTFILE
    "$ADB" -s "$serial" shell am force-stop "$BUNDLE" >/dev/null 2>&1 || true
    "$ADB" -s "$serial" shell am instrument -w -r -e undra_bench 1 -e undra_bench_mode "$1" \
      ${QUICK:+-e undra_bench_quick "$QUICK"} "$BUNDLE.test/androidx.test.runner.AndroidJUnitRunner" >"$2" 2>&1 || true
    grep -q "undra_bench_json=" "$2" || { tail -20 "$2"; die "the $1 run produced no result ($2)"; }
  }

  local n
  for n in $(seq 1 "$RUNS"); do
    local l1 l2 cold_files=()
    l1="$(loadavg)"
    echo "== run $n of $RUNS: the full benchmark, then $COLD_RUNS cold launches on $serial (load average $l1)"
    instrument full "$WORK/full-$n.out"
    local c
    for c in $(seq 1 "$COLD_RUNS"); do
      instrument cold "$WORK/cold-$n-$c.out"
      cold_files+=("$WORK/cold-$n-$c.out")
    done
    l2="$(loadavg)"
    "${REPORT[@]}" assemble --platform android --out "$WORK/raw-$n.json" "$WORK/full-$n.out" ${cold_files[@]+"${cold_files[@]}"}
    local osv label slug_ battery=""
    osv="$(field "$WORK/raw-$n.json" device.os)"
    if [ "$kind" = emulator ]; then
      label="Android emulator \"$(field "$WORK/raw-$n.json" device.model)\" ($osv, $abi, hardware-virtualized on $(host_cpu))"
      slug_="android-emulator-$abi"
    else
      label="$(field "$WORK/raw-$n.json" device.model) ($osv, $abi)"
      slug_="android-device-$(slug "$model")"
      battery="$("$ADB" -s "$serial" shell dumpsys battery | tr -d '\r' | grep -E "level|temperature|status" | tr -s ' ' | tr '\n' ';')"
    fi
    finalize_run "$WORK/raw-$n.json" "$slug_" "$kind" "$label" "$(run_tag "$n")" \
      "release (LTO fat), libundra_core.so" "benchmark build type (release, not debuggable); ART compile filter speed; instrumented test" "$l1" "$l2" "${battery:+battery after the run: $battery}"
  done
  [ "$kind" != device ] || "$ADB" -s "$serial" shell svc power stayon false >/dev/null 2>&1 || true
}

# ================================================================================================
# Web
# ================================================================================================
bench_web() {
  if [ "$BUILD" = 1 ]; then
    echo "== core: undra build --platform web"
    "$UNDRA" build -C "$PROJECT" --platform web
    [ -d "$PROJECT/web/node_modules" ] || (cd "$PROJECT/web" && npm ci --no-audit --no-fund)
    [ -d "$REPO/runtimes/ts/@undra/runtime/node_modules" ] || (cd "$REPO/runtimes/ts/@undra/runtime" && npm ci --no-audit --no-fund)
  fi
  local n
  for n in $(seq 1 "$RUNS"); do
    local l1 l2
    l1="$(loadavg)"
    echo "== run $n of $RUNS: the full benchmark in Chromium, then $COLD_RUNS cold starts in fresh contexts (load average $l1)"
    local web_env=(UNDRA_BENCH_RAW_OUT="$WORK/raw-$n.json" UNDRA_BENCH_COLD_RUNS="$COLD_RUNS")
    [ "$QUICK" = 1 ] && web_env+=(UNDRA_BENCH_QUICK=1)
    (cd "$PROJECT/web" && env "${web_env[@]}" npm run bench) \
      >"$WORK/web-$n.log" 2>&1 || { tail -30 "$WORK/web-$n.log"; die "the web benchmark failed (log: $WORK/web-$n.log)"; }
    l2="$(loadavg)"
    local version label
    version="$(field "$WORK/raw-$n.json" device.browser_version)"
    label="Chromium $version headless (Playwright, wasm-main) on $(host_cpu)"
    finalize_run "$WORK/raw-$n.json" "web-chromium-headless" browser "$label" "$(run_tag "$n")" \
      "release-wasm, wasm-opt -Oz" "vite production build, cross-origin isolated (5 µs clock)" "$l1" "$l2"
  done
}

"bench_$DEVICE"
if [ "$RENDER" = 1 ]; then
  "${REPORT[@]}" render
fi
echo "done: results in ${OUT#"$REPO"/}"
