// swift-tools-version: 6.0
//
// UndraRuntime: the Swift platform runtime for Undra (docs/SPEC.md sections 6.2, 10 and 11).
//
// Targets
//   UndraFFI           C target: `include/undra.h` (the C ABI of SPEC section 6: the `UndraApi`
//                     table and the callback types, no functions) and its module map. It declares
//                     types only and references no symbol, so the package links without any core.
//   UndraRuntime       Swift target: the wire layer, the runtime core (UndraCore, the in-process and
//                     WebSocket transports, mirror, objects, stores, ports) and the default adapters.
//   UndraRuntimeTests  XCTest target. Runs with `swift test` and needs no Rust core (a scripted fake
//                     core table stands in for it).
//
// The core is not a dependency of this package (ADR-044). Each core exports one function,
// `<namespace>_undra_api()`, returning its table; the bindings generated for it declare that
// function (module `<Namespace>CoreFFI`) and hand it to `UndraCoreEntry`, and the app links the
// core (`lib<namespace>.a` from its XCFramework). Several cores can therefore share a process.

import PackageDescription

let package = Package(
    name: "UndraRuntime",
    platforms: [
        .iOS(.v17),
        .macOS(.v14),
    ],
    products: [
        .library(name: "UndraRuntime", targets: ["UndraRuntime"]),
    ],
    targets: [
        .target(
            name: "UndraFFI",
            path: "Sources/UndraFFI",
            publicHeadersPath: "include"
        ),
        .target(
            name: "UndraRuntime",
            dependencies: ["UndraFFI"],
            path: "Sources/UndraRuntime"
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
