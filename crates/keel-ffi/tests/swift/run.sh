#!/usr/bin/env bash
# Runs NativeCoreTests.swift: the Swift runtime over the real C ABI, against the fixture core.
#
#   crates/keel-ffi/tests/swift/run.sh
#
# The Swift package is copied to a scratch directory (nothing in runtimes/swift changes), the test
# is added, and the package is built with KEEL_LINK_CORE=1 so `keel_stub.c` is left out and the
# `keel_*` symbols come from the fixture's static library.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../../.." && pwd)"
SCRATCH="${KEEL_SWIFT_SCRATCH:-$(mktemp -d)}"

if [ -z "${DEVELOPER_DIR:-}" ] && [ -d "/Applications/Xcode.app/Contents/Developer" ]; then
  export DEVELOPER_DIR="/Applications/Xcode.app/Contents/Developer"
fi

cargo build --manifest-path "$HERE/../fixture/Cargo.toml"
LIB="$HERE/../fixture/target/debug/libkeel_core.a"

rm -rf "$SCRATCH/pkg"
mkdir -p "$SCRATCH/pkg"
rsync -a --exclude .build "$REPO/runtimes/swift/KeelRuntime/" "$SCRATCH/pkg/"
cp "$HERE/NativeCoreTests.swift" "$SCRATCH/pkg/Tests/KeelRuntimeTests/"
cd "$SCRATCH/pkg"
# force_load: the core's `#[keel::api]` registrations are static constructors in object files nothing else references; without it the linker drops those archive members and the schema is empty.
KEEL_LINK_CORE=1 swift test --filter NativeCoreTests -Xlinker -force_load -Xlinker "$LIB"
