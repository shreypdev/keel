#!/usr/bin/env bash
# The iOS 15 / 16 floor (ADR-045, docs/IOS_15_16.md): everything that proves the compatibility mode builds where it claims to.
#
#   scripts/ios-floor.sh             run every step below (macOS with Xcode)
#   scripts/ios-floor.sh runtime     one step: runtime | golden | sample | apps | contract | probe | simulator
#
#   runtime    the Swift runtime package (UndraRuntime and UndraTestKit) for the iOS 15.0 simulator, and for macOS 12
#              (the same API availability), in Swift 6 language mode
#   golden     the generated Swift of every golden case, in the ObservableObject mode, against the runtime for the iOS 15.0
#              and 16.0 simulators (cargo test -p undra-bindgen --test typecheck_swift)
#   sample     examples/ios15-sample: its bindings are up to date (`undra bindgen --check`), and the app builds for the
#              iOS 15.0 simulator with xcodebuild (the Run Script phase builds the core)
#   apps       the bindings of the playground and Fieldbook, generated for an iOS 15 floor into a scratch directory,
#              compile for the iOS 15.0 simulator (their apps stay on iOS 17: they dogfood Observation)
#   contract   the Swift contract scenarios (the whole grid) against bindings generated for the iOS 15 floor
#              (contract-tests/swift/run.sh --floor)
#   probe      the runtime on the booted simulator (the newest iOS runtime installed): a tiny executable built for an iOS 15.0
#              and for an iOS 17.0 deployment target (scripts/ios-floor-probe) reports that `core.connection` is the one
#              `@Observable` connection on an OS that has Observation, so a floor build never falls back to the floor path on a
#              device that can do better
#   simulator  only when an iOS 15 or 16 simulator runtime is installed (this machine and CI may have iOS 26 only): the
#              sample is installed and launched on it. Without one it says so and passes; Xcode can still *build*
#              for those versions, which is what the steps above prove.
set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"
if [ -z "${DEVELOPER_DIR:-}" ] && [ -d "/Applications/Xcode.app/Contents/Developer" ]; then
  export DEVELOPER_DIR="/Applications/Xcode.app/Contents/Developer"
fi
SCRATCH="${UNDRA_FLOOR_SCRATCH:-$REPO/target/ios-floor}"
mkdir -p "$SCRATCH"
SDK="$(xcrun --sdk iphonesimulator --show-sdk-path)"
UNDRA="${UNDRA:-$REPO/target/debug/undra}"
steps=("$@")
[ "${#steps[@]}" = 0 ] && steps=(runtime golden sample apps contract probe simulator)

need_undra() {
  [ -x "$UNDRA" ] || cargo build -p undra-cli
}

step_runtime() {
  echo "==> the Swift runtime for the iOS 15.0 simulator"
  swift build --package-path runtimes/swift/UndraRuntime --scratch-path "$SCRATCH/runtime-ios15" \
    --sdk "$SDK" --triple arm64-apple-ios15.0-simulator
  echo "==> the Swift runtime for macOS 12"
  swift build --package-path runtimes/swift/UndraRuntime --scratch-path "$SCRATCH/runtime-macos"
}

step_golden() {
  echo "==> the generated Swift of every golden case, ObservableObject mode, iOS 15.0 and 16.0"
  UNDRA_REQUIRE_TOOLCHAINS=1 cargo test -p undra-bindgen --test typecheck_swift -- ios_15 ios_16
}

step_sample() {
  need_undra
  echo "==> examples/ios15-sample: the bindings are up to date"
  "$UNDRA" bindgen -C examples/ios15-sample --check
  echo "==> examples/ios15-sample: xcodebuild for the iOS 15.0 simulator"
  PATH="$(dirname "$UNDRA"):$PATH" xcodebuild -quiet -project examples/ios15-sample/ios/Ios15Sample.xcodeproj \
    -scheme Ios15Sample -configuration Debug -destination 'generic/platform=iOS Simulator' \
    -derivedDataPath "$SCRATCH/sample-dd" build
  local app="$SCRATCH/sample-dd/Build/Products/Debug-iphonesimulator/Ios15Sample.app"
  local minimum
  minimum="$(/usr/libexec/PlistBuddy -c 'Print :MinimumOSVersion' "$app/Info.plist")"
  echo "    MinimumOSVersion $minimum"
  [ "$minimum" = "15.0" ] || { echo "the sample does not declare iOS 15.0" >&2; exit 1; }
}

step_apps() {
  need_undra
  for project in examples/playground examples/fieldbook; do
    local name
    name="$(basename "$project")"
    echo "==> $name: bindings for an iOS 15 floor compile for the iOS 15.0 simulator"
    rm -rf "$SCRATCH/$name-bindings"
    "$UNDRA" bindgen -C "$project" --out "$SCRATCH/$name-bindings" --platforms ios \
      --swift-observation observable-object --ios-deployment-target 15.0
    swift build --package-path "$SCRATCH/$name-bindings/swift" --scratch-path "$SCRATCH/$name-bindings/.build" \
      --sdk "$SDK" --triple arm64-apple-ios15.0-simulator --disable-sandbox
  done
}

step_contract() {
  echo "==> the Swift contract scenarios against bindings generated for iOS 15.0"
  bash contract-tests/swift/run.sh --floor
}

step_probe() {
  local udid
  udid="$(xcrun simctl list devices available | grep -E '^ +iPhone' | head -1 | grep -oE '[0-9A-F]{8}(-[0-9A-F]{4}){3}-[0-9A-F]{12}' || true)"
  if [ -z "$udid" ]; then
    echo "==> no iPhone simulator is available here; the probe needs one to run"
    return 0
  fi
  xcrun simctl boot "$udid" 2>/dev/null || true
  if ! xcrun simctl bootstatus "$udid" -b >/dev/null 2>&1; then
    echo "==> the simulator $udid did not boot; the probe was not run"
    return 0
  fi
  for floor in 15 17; do
    echo "==> the observation path at run time, binary built for iOS $floor.0, on the simulator $udid"
    UNDRA_PROBE_IOS="$floor" swift build --package-path scripts/ios-floor-probe --scratch-path "$SCRATCH/probe-$floor" \
      --sdk "$SDK" --triple "arm64-apple-ios$floor.0-simulator"
    local binary
    binary="$(find "$SCRATCH/probe-$floor" -name probe -type f -perm +111 | head -1)"
    xcrun vtool -show-build "$binary" | grep minos | sed 's/^ */    /'
    xcrun simctl spawn "$udid" "$binary"
  done
}

step_simulator() {
  if xcrun simctl list runtimes | grep -Eq 'iOS (15|16)\.'; then
    local udid
    udid="$(xcrun simctl list devices available | grep -E '^-- iOS (15|16)\.' -A1 | grep -oE '[0-9A-F]{8}(-[0-9A-F]{4}){3}-[0-9A-F]{12}' | head -1)"
    echo "==> launching the sample on an iOS 15/16 simulator ($udid)"
    xcrun simctl boot "$udid" 2>/dev/null || true
    xcrun simctl install "$udid" "$SCRATCH/sample-dd/Build/Products/Debug-iphonesimulator/Ios15Sample.app"
    xcrun simctl launch "$udid" com.example.ios15sample
  else
    echo "==> no iOS 15 or 16 simulator runtime is installed here (Xcode builds for them, which the steps above prove);"
    echo "    to run the sample on one: Xcode > Settings > Components, or xcodebuild -downloadPlatform iOS -buildVersion 16.4"
  fi
}

for step in "${steps[@]}"; do
  "step_$step"
done
