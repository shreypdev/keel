import Foundation

/// The checked-in fixtures every kit reads (`testkit/` of the repository), found from this file's own path.
func fixture(_ name: String) throws -> String {
    var url = URL(fileURLWithPath: #filePath)
    for _ in 0..<6 {
        url.deleteLastPathComponent()
    }
    return try String(contentsOf: url.appendingPathComponent("testkit/\(name)"), encoding: .utf8)
}
