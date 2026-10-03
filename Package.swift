// swift-tools-version: 6.0
//
// The Undra Swift runtime as a package of the repository (ADR-063): an app adds
//
//     .package(url: "https://github.com/shreypdev/undra", from: "1.0.0")
//
// (Xcode: File > Add Package Dependencies, the same URL) and uses the product `UndraRuntime`
// (`UndraTestKit` for its tests); SwiftPM reads the `v1.0.0` tags of the repository. `undra init` and
// `undra bindgen` write exactly that for a released project.
//
// The sources are the in-repository package's (runtimes/swift/UndraRuntime), which stays what the
// examples and the Swift tests use. This manifest exposes the same library products and targets, without
// the test targets; crates/undra-cli/tests/swift_manifests.rs keeps the two in step.

import PackageDescription

let package = Package(
    name: "undra",
    platforms: [
        .iOS(.v15),
        .macOS(.v12),
    ],
    products: [
        .library(name: "UndraRuntime", targets: ["UndraRuntime"]),
        .library(name: "UndraTestKit", targets: ["UndraTestKit"]),
    ],
    targets: [
        .target(
            name: "UndraFFI",
            path: "runtimes/swift/UndraRuntime/Sources/UndraFFI",
            publicHeadersPath: "include"
        ),
        .target(
            name: "UndraRuntime",
            dependencies: ["UndraFFI"],
            path: "runtimes/swift/UndraRuntime/Sources/UndraRuntime"
        ),
        .target(
            name: "UndraTestKit",
            dependencies: ["UndraRuntime"],
            path: "runtimes/swift/UndraRuntime/Sources/UndraTestKit"
        ),
    ],
    swiftLanguageModes: [.v6]
)
