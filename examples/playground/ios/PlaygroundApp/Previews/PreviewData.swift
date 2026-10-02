#if DEBUG
import Foundation
import UndraTestKit

/// What the previews read: the fixtures of the testing kit (`testkit/fixtures` at the root of the repository), found from this file's own path.
/// A preview runs on the developer's Mac, where the sources are; nothing here ships in a release build.
enum PreviewData {
    private static func text(_ name: String) throws -> String {
        var url = URL(fileURLWithPath: #filePath)
        for _ in 0..<6 {
            url.deleteLastPathComponent()
        }
        return try String(contentsOf: url.appendingPathComponent("testkit/fixtures/\(name)"), encoding: .utf8)
    }

    /// A session recorded from the Todos store: `undra dev --record` writes files of this shape, and a Rust test wrote this one.
    static func recording(_ name: String) throws -> Recording {
        return try Recording(json: try text("\(name).json"))
    }

    /// The starting state of the fakes: a server that answers the inbox list, a few stored values.
    static func seed() throws -> Seed {
        return try Seed(json: try text("seed.json"))
    }
}
#endif
