// swift-tools-version: 6.0
//
// The Swift column of the contract scenarios (contract-tests/scenarios.md): UndraRuntime over the C
// ABI against the real playground core, through the bindings `undra bindgen` generated, and, for S26,
// the same core under two more namespaces (examples/two-cores/a and b) in the same process.
//
// Run it with `contract-tests/swift/run.sh`. It builds the three cores with the undra CLI and stages
// the libraries in `.build/core`; each exports one symbol, its table entry (`<namespace>_undra_api`,
// ADR-044), which the generated packages reference, so the three link side by side. The linker flags
// below find the staged libraries.

import PackageDescription

/// Where run.sh stages the cores (copies of build/host/lib<namespace>.dylib whose install names are
/// `@rpath/lib<namespace>.dylib`).
let coreDirectory = Context.packageDirectory + "/.build/core"

let package = Package(
    name: "UndraContractTests",
    platforms: [.macOS(.v14)],
    dependencies: [
        .package(path: "../../runtimes/swift/UndraRuntime"),
        .package(path: "Packages/PlaygroundCore"),
        .package(path: "Packages/PlaygroundA"),
        .package(path: "Packages/PlaygroundB"),
    ],
    targets: [
        // The testing kit (docs/TESTING.md) against the same core: PreviewCore with the fakes, RecordedCore under the generated store. Not part of
        // the grid; run.sh runs it as its own `swift test` (one in-process core per process) after the scenarios.
        .testTarget(
            name: "TestKitTests",
            dependencies: [
                .product(name: "UndraRuntime", package: "UndraRuntime"),
                .product(name: "UndraTestKit", package: "UndraRuntime"),
                .product(name: "PlaygroundCore", package: "PlaygroundCore"),
            ],
            path: "Tests/TestKitTests",
            linkerSettings: [
                .unsafeFlags([
                    "-L", coreDirectory,
                    "-lplayground_core",
                    "-Xlinker", "-rpath", "-Xlinker", coreDirectory,
                ]),
            ]
        ),
        .testTarget(
            name: "ContractTests",
            dependencies: [
                .product(name: "UndraRuntime", package: "UndraRuntime"),
                .product(name: "PlaygroundCore", package: "PlaygroundCore"),
                .product(name: "PlaygroundA", package: "PlaygroundA"),
                .product(name: "PlaygroundB", package: "PlaygroundB"),
            ],
            path: "Tests/ContractTests",
            linkerSettings: [
                // The staged cores, found at run time through the test bundle's rpath.
                .unsafeFlags([
                    "-L", coreDirectory,
                    "-lplayground_core",
                    "-lplayground_a",
                    "-lplayground_b",
                    "-Xlinker", "-rpath", "-Xlinker", coreDirectory,
                ]),
            ]
        ),
    ],
    swiftLanguageModes: [.v6]
)
