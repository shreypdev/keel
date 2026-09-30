// swift-tools-version: 6.0
//
// The Swift column of the contract scenarios (contract-tests/scenarios.md): KeelRuntime over the C
// ABI against the real playground core, through the bindings `keel bindgen` generated.
//
// Run it with `contract-tests/swift/run.sh`. It builds the core with the keel CLI, stages
// the library in `.build/core` and runs `swift test` with `KEEL_LINK_CORE=1`: that variable makes
// the runtime package leave its link-time stand-in (`keel_stub.c`) out, so the `keel_*` symbols come
// from the real core. The linker flags below find the staged library.

import PackageDescription

/// Where run.sh stages the core library (a copy of build/host/libkeel_core.dylib whose install name
/// is `@rpath/libkeel_core.dylib`).
let coreDirectory = Context.packageDirectory + "/.build/core"

let package = Package(
    name: "KeelContractTests",
    platforms: [.macOS(.v14)],
    dependencies: [
        .package(path: "../../runtimes/swift/KeelRuntime"),
        .package(path: "Packages/PlaygroundCore"),
    ],
    targets: [
        .testTarget(
            name: "ContractTests",
            dependencies: [
                .product(name: "KeelRuntime", package: "KeelRuntime"),
                .product(name: "PlaygroundCore", package: "PlaygroundCore"),
            ],
            path: "Tests/ContractTests",
            linkerSettings: [
                // The staged core, found at run time through the test bundle's rpath.
                .unsafeFlags([
                    "-L", coreDirectory,
                    "-lkeel_core",
                    "-Xlinker", "-rpath", "-Xlinker", coreDirectory,
                ]),
            ]
        ),
    ],
    swiftLanguageModes: [.v6]
)
