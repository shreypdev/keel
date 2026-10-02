// swift-tools-version: 6.0
// The Swift lines the cookbook pages show, built against the bindings `undra bindgen` generates from
// examples/cookbook (../check.sh runs `swift build` here). Nothing in this package ships.
import PackageDescription

let package = Package(
    name: "CookbookSnippets",
    platforms: [.iOS(.v17), .macOS(.v14)],
    dependencies: [.package(path: "../../generated/swift")],
    targets: [
        .target(
            name: "Snippets",
            dependencies: [.product(name: "CookbookCore", package: "swift")],
            path: "Sources/Snippets"
        ),
    ],
    swiftLanguageModes: [.v6]
)
