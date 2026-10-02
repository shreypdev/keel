// swift-tools-version: 6.0
//
// A few lines that report, at run time, which observation path the runtime takes (ADR-045): `scripts/ios-floor.sh probe` builds
// this for an iOS 15.0 and for an iOS 17.0 deployment target (UNDRA_PROBE_IOS=15|17) and runs both on the booted simulator. On an OS
// that has Observation, `core.connection` must be the one `@Observable` connection whatever the binary's deployment target is: a
// floor build must not fall back to the floor path on a device that can do better.

import PackageDescription

let floor = Context.environment["UNDRA_PROBE_IOS"] == "15"

let package = Package(
    name: "probe",
    platforms: floor ? [.iOS(.v15), .macOS(.v12)] : [.iOS(.v17), .macOS(.v14)],
    dependencies: [.package(path: "../../runtimes/swift/UndraRuntime")],
    targets: [
        .executableTarget(name: "probe", dependencies: [.product(name: "UndraRuntime", package: "UndraRuntime")]),
    ]
)
