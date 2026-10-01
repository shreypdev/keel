#!/usr/bin/env bash
# Runs NativeCoreTests.swift: the Swift runtime over the real C ABI (version 2, a table per core,
# ADR-044), against the fixture core.
#
#   crates/undra-ffi/tests/swift/run.sh
#
# Builds the fixture core (libundra_fixture, namespace `undra_fixture`) and a scratch SwiftPM package
# that depends on runtimes/swift/UndraRuntime by path (nothing in the repository changes), with
#
#   UndraFixtureCoreFFI  the C module bindgen generates for a core: `undra_fixture_undra.h` declaring
#                        `const void *undra_fixture_undra_api(void);`, linked against the fixture
#   NativeCoreTests      this directory's NativeCoreTests.swift
#
# The fixture is linked as its dylib: a cdylib holds every registration of the core, so no
# `-force_load` is needed (an app links the prelinked `lib<namespace>.a` that `undra build` makes,
# which needs none either).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../../.." && pwd)"
SCRATCH="${UNDRA_SWIFT_SCRATCH:-$(mktemp -d)}"

if [ -z "${DEVELOPER_DIR:-}" ] && [ -d "/Applications/Xcode.app/Contents/Developer" ]; then
  export DEVELOPER_DIR="/Applications/Xcode.app/Contents/Developer"
fi

FIXTURE="$HERE/../fixture"
cargo build --manifest-path "$FIXTURE/Cargo.toml"
LIBDIR="${CARGO_TARGET_DIR:-$FIXTURE/target}/debug"
if [ ! -f "$LIBDIR/libundra_fixture.dylib" ]; then
  echo "run.sh: $LIBDIR/libundra_fixture.dylib was not built" >&2
  exit 1
fi

PKG="$SCRATCH/pkg"
rm -rf "$PKG"
mkdir -p "$PKG/Sources/UndraFixtureCoreFFI/include" "$PKG/Tests/NativeCoreTests"

cat > "$PKG/Sources/UndraFixtureCoreFFI/include/undra_fixture_undra.h" <<'H'
/* The entry point of the fixture core `undra_fixture` (C ABI version 2): its `UndraApi` table. */
#ifndef UNDRA_FIXTURE_UNDRA_H
#define UNDRA_FIXTURE_UNDRA_H
const void *undra_fixture_undra_api(void);
#endif
H
cat > "$PKG/Sources/UndraFixtureCoreFFI/include/module.modulemap" <<'M'
module UndraFixtureCoreFFI {
    header "undra_fixture_undra.h"
    export *
}
M
cat > "$PKG/Sources/UndraFixtureCoreFFI/undra_fixture_undra.c" <<'C'
#include "undra_fixture_undra.h"
C
cat > "$PKG/Package.swift" <<P
// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "NativeCore",
    platforms: [.macOS(.v14)],
    dependencies: [.package(path: "$REPO/runtimes/swift/UndraRuntime")],
    targets: [
        .target(
            name: "UndraFixtureCoreFFI",
            path: "Sources/UndraFixtureCoreFFI",
            publicHeadersPath: "include",
            linkerSettings: [.linkedLibrary("undra_fixture")]
        ),
        .testTarget(
            name: "NativeCoreTests",
            dependencies: [.product(name: "UndraRuntime", package: "UndraRuntime"), "UndraFixtureCoreFFI"],
            path: "Tests/NativeCoreTests"
        ),
    ],
    swiftLanguageModes: [.v6]
)
P
cp "$HERE/NativeCoreTests.swift" "$PKG/Tests/NativeCoreTests/"

cd "$PKG"
swift test -Xlinker -L"$LIBDIR" -Xlinker -rpath -Xlinker "$LIBDIR" "$@"
