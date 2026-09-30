// swift-tools-version: 6.0
//
// KeelRuntime: the Swift platform runtime for Keel (docs/SPEC.md sections 6.2, 10 and 11).
//
// Targets
//   KeelFFI           C target: `include/keel.h` (the C ABI of SPEC section 6) and its module map.
//                     `keel_stub.c` provides link-time stand-ins for every `keel_*` symbol and is
//                     compiled only when `KEEL_STUB_FFI` is defined (see below).
//   KeelRuntime       Swift target: wire codecs, envelope, payloads (and, in later tasks, the
//                     transport, mirror and adapters).
//   KeelRuntimeTests  XCTest target. Runs with `swift test` and needs neither the Rust core nor
//                     the XCFramework.
//
// The KEEL_STUB_FFI switch
//   The real core (libkeel_ffi, shipped later as an XCFramework) is not linked yet. Until it is,
//   this manifest defines `KEEL_STUB_FFI` through `cSettings`, so `keel_stub.c` supplies the
//   symbols and the package builds and tests on any Mac without the core. The stub reports ABI
//   version 0 and fails every call, so a runtime that reaches it fails loudly at attach time.
//   Set `KEEL_LINK_CORE=1` in the environment to build WITHOUT the stub (the linker must then be
//   given the real core, otherwise duplicate/missing symbols are the correct failure). The
//   XCFramework wiring task flips this default; see README.md.

import PackageDescription
import Foundation

let linkRealCore = ProcessInfo.processInfo.environment["KEEL_LINK_CORE"] == "1"

let ffiCSettings: [CSetting] = linkRealCore ? [] : [.define("KEEL_STUB_FFI")]

let package = Package(
    name: "KeelRuntime",
    platforms: [
        .iOS(.v17),
        .macOS(.v14),
    ],
    products: [
        .library(name: "KeelRuntime", targets: ["KeelRuntime"]),
    ],
    targets: [
        .target(
            name: "KeelFFI",
            path: "Sources/KeelFFI",
            publicHeadersPath: "include",
            cSettings: ffiCSettings
        ),
        .target(
            name: "KeelRuntime",
            dependencies: ["KeelFFI"],
            path: "Sources/KeelRuntime"
        ),
        .testTarget(
            name: "KeelRuntimeTests",
            dependencies: ["KeelRuntime"],
            path: "Tests/KeelRuntimeTests",
            resources: [
                .process("Resources"),
            ]
        ),
    ],
    swiftLanguageModes: [.v6]
)
