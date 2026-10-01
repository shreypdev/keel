// swift-tools-version: 6.0
//
// The Swift column of the contract scenarios (contract-tests/scenarios.md): UndraRuntime over the C
// ABI against the real playground core, through the bindings `undra bindgen` generated.
//
// Run it with `contract-tests/swift/run.sh`. It builds the core with the undra CLI, stages
// the library in `.build/core` and runs `swift test` with `UNDRA_LINK_CORE=1`: that variable makes
// the runtime package leave its link-time stand-in (`undra_stub.c`) out, so the `undra_*` symbols come
// from the real core. The linker flags below find the staged library.

import PackageDescription

/// Where run.sh stages the core library (a copy of build/host/libundra_core.dylib whose install name
/// is `@rpath/libundra_core.dylib`).
let coreDirectory = Context.packageDirectory + "/.build/core"

let package = Package(
    name: "UndraContractTests",
    platforms: [.macOS(.v14)],
    dependencies: [
        .package(path: "../../runtimes/swift/UndraRuntime"),
        .package(path: "Packages/PlaygroundCore"),
    ],
    targets: [
        .testTarget(
            name: "ContractTests",
            dependencies: [
                .product(name: "UndraRuntime", package: "UndraRuntime"),
                .product(name: "PlaygroundCore", package: "PlaygroundCore"),
            ],
            path: "Tests/ContractTests",
            linkerSettings: [
                // The staged core, found at run time through the test bundle's rpath.
                .unsafeFlags([
                    "-L", coreDirectory,
                    "-lundra_core",
                    "-Xlinker", "-rpath", "-Xlinker", coreDirectory,
                ]),
            ]
        ),
    ],
    swiftLanguageModes: [.v6]
)
