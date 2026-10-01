#!/usr/bin/env bash
# Runs the two-core test app on the iOS simulator (ADR-044): builds both cores for iOS (two prelinked
# static libraries, playground_a and playground_b), builds TwoCores.app with both linked into its one
# binary, launches it and checks its `two-cores ios:` log lines. Exits non-zero unless every check passed.
#
#   examples/two-cores/ios/run.sh                     the booted simulator, or SIMULATOR (default "iPhone 17 Pro")
#   CONFIGURATION=Release examples/two-cores/ios/run.sh   release cores (fat LTO, prelinked with -u) and a Release app
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
UNDRA="${UNDRA:-$REPO/target/debug/undra}"
SIMULATOR="${SIMULATOR:-iPhone 17 Pro}"
DERIVED="$HERE/DerivedData"
CONFIGURATION="${CONFIGURATION:-Debug}"
RELEASE=(); [ "$CONFIGURATION" = Release ] && RELEASE=(--release)
if [ -z "${DEVELOPER_DIR:-}" ] && [ -d "/Applications/Xcode.app/Contents/Developer" ]; then
  export DEVELOPER_DIR="/Applications/Xcode.app/Contents/Developer"
fi
[ -x "$UNDRA" ] || (cd "$REPO" && cargo build -p undra-cli)
for ns in a b; do "$UNDRA" build -C "$HERE/../$ns" --platform ios ${RELEASE[@]+"${RELEASE[@]}"}; done

udid="$(xcrun simctl list devices booted | grep -m1 -oE '\(([0-9A-F-]{36})\) \(Booted\)' | grep -oE '[0-9A-F-]{36}' || true)"
if [ -z "$udid" ]; then
  udid="$(xcrun simctl list devices available | grep -m1 "    $SIMULATOR (" | grep -oE '[0-9A-F-]{36}')"
  xcrun simctl boot "$udid"
fi
xcodebuild -quiet -project "$HERE/TwoCores.xcodeproj" -scheme TwoCores -configuration "$CONFIGURATION" -sdk iphonesimulator \
  -destination "id=$udid" -derivedDataPath "$DERIVED" build
xcrun simctl install "$udid" "$DERIVED/Build/Products/$CONFIGURATION-iphonesimulator/TwoCores.app"
xcrun simctl terminate "$udid" dev.undra.twocores >/dev/null 2>&1 || true
started="$(date '+%Y-%m-%d %H:%M:%S')"
xcrun simctl launch "$udid" dev.undra.twocores >/dev/null
for _ in $(seq 1 30); do
  lines="$(xcrun simctl spawn "$udid" log show --style compact --start "$started" \
    --predicate 'subsystem == "dev.undra.twocores"' 2>/dev/null | grep -o 'two-cores ios: .*' || true)"
  if printf '%s\n' "$lines" | grep -qE 'two-cores ios: (passed|FAILED)$'; then break; fi
  sleep 1
done
printf '%s\n' "$lines"
printf '%s\n' "$lines" | grep -qx 'two-cores ios: passed'
