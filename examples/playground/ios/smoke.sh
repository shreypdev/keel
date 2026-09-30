#!/usr/bin/env bash
# Builds the playground for the iOS simulator and proves it runs: the four screens launched one by
# one (`-tab`), a screenshot of each in ../.proof/ios-<view>.png, the process checked alive with no
# crash in the log, and the XCUITest tour run on top.
#
#   examples/playground/ios/smoke.sh                 everything, output also in ../.proof/ios-smoke.log
#   SIMULATOR="iPhone 17 Pro" examples/playground/ios/smoke.sh
#
# Needs Xcode, the aarch64-apple-ios-sim Rust target and the keel CLI (`cargo build -p keel-cli`).
# The core is linked with `-force_load` (see the project settings) and the runtime package is built
# with KEEL_LINK_CORE=1, which leaves out its link-time stand-in for the core.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT="$(cd "$HERE/.." && pwd)"
REPO="$(cd "$PROJECT/../.." && pwd)"
PROOF="$PROJECT/.proof"
KEEL="${KEEL:-$REPO/target/debug/keel}"
SIMULATOR="${SIMULATOR:-iPhone 17 Pro}"
BUNDLE="dev.keel.playground"
DERIVED="$HERE/DerivedData"

if [ -z "${DEVELOPER_DIR:-}" ] && [ -d "/Applications/Xcode.app/Contents/Developer" ]; then
  export DEVELOPER_DIR="/Applications/Xcode.app/Contents/Developer"
fi
export PATH="$HOME/.cargo/bin:$PATH"
export KEEL_LINK_CORE=1

mkdir -p "$PROOF"
exec > >(tee "$PROOF/ios-smoke.log") 2>&1

step() { printf '\n$ %s\n' "$*"; }
run() { step "$*"; "$@"; }

echo "# playground iOS smoke run, $(date '+%Y-%m-%d %H:%M:%S')"
run xcodebuild -version
run swift --version

# 1. The core, as an XCFramework (static libraries, no header: KeelRuntime's KeelFFI module has it).
[ -x "$KEEL" ] || cargo build --manifest-path "$REPO/Cargo.toml" -p keel-cli
run "$KEEL" build -C "$PROJECT" --platform ios

# 2. The app.
step "xcodebuild -project PlaygroundApp.xcodeproj -scheme PlaygroundApp -destination 'platform=iOS Simulator,name=$SIMULATOR' build"
xcodebuild -quiet -project "$HERE/PlaygroundApp.xcodeproj" -scheme PlaygroundApp -configuration Debug \
  -destination "platform=iOS Simulator,name=$SIMULATOR" -derivedDataPath "$DERIVED" build 2>&1 | grep -E "error:|BUILD|warning: .*PlaygroundApp/" || true
APP="$DERIVED/Build/Products/Debug-iphonesimulator/PlaygroundApp.app"
[ -d "$APP" ] || { echo "no app at $APP"; exit 1; }
echo "built $APP"
du -sh "$APP" | sed 's/^/app bundle (debug): /'
ls -l "$APP/PlaygroundApp" "$APP/PlaygroundApp.debug.dylib" 2>/dev/null | awk '{print "  " $5 " bytes  " $9}'

# 3. The simulator.
UDID="$(xcrun simctl list devices available | grep -F "$SIMULATOR (" | head -1 | sed -E 's/.*\(([0-9A-F-]{36})\).*/\1/')"
[ -n "$UDID" ] || { echo "no simulator called $SIMULATOR"; exit 1; }
echo "simulator $SIMULATOR = $UDID"
xcrun simctl boot "$UDID" 2>/dev/null || true
run xcrun simctl bootstatus "$UDID" -b
run xcrun simctl terminate "$UDID" "$BUNDLE" || true
run xcrun simctl uninstall "$UDID" "$BUNDLE" || true
run xcrun simctl install "$UDID" "$APP"

# 4. Each screen: launch, wait, screenshot, alive, no crash in the log.
failed=0
for view in todos counter biglist remote; do
  step "xcrun simctl launch $UDID $BUNDLE -tab $view"
  xcrun simctl terminate "$UDID" "$BUNDLE" >/dev/null 2>&1 || true
  xcrun simctl launch "$UDID" "$BUNDLE" -tab "$view"
  sleep 4
  run xcrun simctl io "$UDID" screenshot "$PROOF/ios-$view.png"
  step "xcrun simctl spawn $UDID launchctl list | grep $BUNDLE"
  if xcrun simctl spawn "$UDID" launchctl list | grep "$BUNDLE"; then
    echo "alive: $view"
  else
    echo "NOT RUNNING: $view"
    failed=1
  fi
done
# Faults from anywhere in the process, and errors from Keel's own subsystem (dev.keel.*). The
# simulator itself logs harmless errors (accessibility, haptics) that are not the app's.
PREDICATE='process == "PlaygroundApp" AND (messageType == fault OR (messageType == error AND subsystem BEGINSWITH "dev.keel"))'
step "xcrun simctl spawn $UDID log show --last 5m --predicate '$PREDICATE'"
problems="$(xcrun simctl spawn "$UDID" log show --last 5m --style compact --predicate "$PREDICATE" 2>/dev/null | grep -vE "^(Filtering the log data|Timestamp|getpwuid_r)" || true)"
if [ -n "$problems" ]; then
  echo "$problems"
  echo "FAULT OR KEEL ERROR LINES above"
  failed=1
else
  echo "no fault lines and no Keel error lines in the app's log"
fi
step "crash reports for PlaygroundApp since this run started"
if find "$HOME/Library/Logs/DiagnosticReports" -name 'PlaygroundApp*' -newer "$PROOF/ios-smoke.log" 2>/dev/null | grep .; then
  echo "CRASH REPORT above"
  failed=1
else
  echo "none"
fi
ls -l "$PROOF"/ios-*.png | awk '{print $5 "  " $9}'

# 5. The XCUITest tour (taps through all four screens; saves its own screenshots next to the others).
step "xcodebuild test -project PlaygroundApp.xcodeproj -scheme PlaygroundApp -destination 'platform=iOS Simulator,name=$SIMULATOR' -parallel-testing-enabled NO"
TEST_RUNNER_PROOF_DIR="$PROOF" xcodebuild test -project "$HERE/PlaygroundApp.xcodeproj" -scheme PlaygroundApp -configuration Debug \
  -destination "platform=iOS Simulator,name=$SIMULATOR" -derivedDataPath "$DERIVED" -parallel-testing-enabled NO 2>&1 \
  | grep -E "Test (case|Suite)|Executed|TEST (SUCCEEDED|FAILED)|error:" || failed=1

# 6. Leave the simulator as it was found.
run xcrun simctl shutdown "$UDID"
[ "$failed" = 0 ] && echo "ios smoke: PASS" || { echo "ios smoke: FAIL"; exit 1; }
