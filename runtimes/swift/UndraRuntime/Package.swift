// swift-tools-version: 6.0
//
// UndraRuntime: the Swift platform runtime for Undra (docs/SPEC.md sections 6.2, 10 and 11).
//
// Targets
//   UndraFFI           C target: `include/undra.h` (the C ABI of SPEC section 6) and its module map.
//                     `undra_stub.c` provides link-time stand-ins for every `undra_*` symbol and is
//                     compiled only when `UNDRA_STUB_FFI` is defined (see below).
//   UndraRuntime       Swift target: the wire layer, the runtime core (UndraCore, the in-process and
//                     WebSocket transports, mirror, objects, stores, ports) and the default adapters.
//   UndraTestKit       Swift target: the testing kit (PreviewCore, RecordedCore, PortRecorder, Replayer, the deterministic fakes).
//   UndraRuntimeTests  XCTest target. Runs with `swift test` and needs neither the Rust core nor
//                     the XCFramework (a scripted fake core stands in for it).
//
// The UNDRA_STUB_FFI switch
//   The real core (libundra_ffi, shipped later as an XCFramework) is not linked yet. Until it is,
//   this manifest defines `UNDRA_STUB_FFI` through `cSettings`, so `undra_stub.c` supplies the
//   symbols and the package builds and tests on any Mac without the core. The stub reports ABI
//   version 0 and fails every call, so a runtime that reaches it fails loudly at attach time.
//   Set `UNDRA_LINK_CORE=1` in the environment to build WITHOUT the stub (the linker must then be
//   given the real core, otherwise duplicate/missing symbols are the correct failure). The
//   XCFramework wiring task flips this default; see README.md.

import PackageDescription
import Foundation

let linkRealCore = ProcessInfo.processInfo.environment["UNDRA_LINK_CORE"] == "1"

let ffiCSettings: [CSetting] = linkRealCore ? [] : [.define("UNDRA_STUB_FFI")]

let package = Package(
    name: "UndraRuntime",
    platforms: [
        .iOS(.v17),
        .macOS(.v14),
    ],
    products: [
        .library(name: "UndraRuntime", targets: ["UndraRuntime"]),
        .library(name: "UndraTestKit", targets: ["UndraTestKit"]),
    ],
    targets: [
        .target(
            name: "UndraFFI",
            path: "Sources/UndraFFI",
            publicHeadersPath: "include",
            cSettings: ffiCSettings
        ),
        .target(
            name: "UndraRuntime",
            dependencies: ["UndraFFI"],
            path: "Sources/UndraRuntime"
        ),
        // The testing kit (docs/TESTING.md): preview cores, recorded cores, port record/replay. Built on the runtime's `package` seam
        // (UndraTransport and UndraInbound), so it ships with the runtime's package and nothing else can reach that seam.
        .target(
            name: "UndraTestKit",
            dependencies: ["UndraRuntime"],
            path: "Sources/UndraTestKit"
        ),
        .testTarget(
            name: "UndraTestKitTests",
            dependencies: ["UndraTestKit", "UndraRuntime"],
            path: "Tests/UndraTestKitTests"
        ),
        .testTarget(
            name: "UndraRuntimeTests",
            dependencies: ["UndraRuntime"],
            path: "Tests/UndraRuntimeTests",
            resources: [
                .process("Resources"),
            ]
        ),
    ],
    swiftLanguageModes: [.v6]
)
