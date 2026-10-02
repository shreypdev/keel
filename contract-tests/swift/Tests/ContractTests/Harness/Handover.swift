import Foundation

/// What build A hands over to the build-B process (scenarios.md, "Two builds"): the `Kv` contents and
/// the failed POST's `Idempotency-Key` from S14 step 7, the snapshots `P` and `L` and the `Profile`
/// handle from S15 step 11. One JSON file per scenario, in the directory `UNDRA_CONTRACT_HANDOVER` names
/// (`run.sh` sets it to `.build/migration`), else in `.build/migration` of this package.
enum Handover {
    /// S14 step 7.
    struct Queue: Codable, Equatable {
        /// Every key of the `Kv` and its value, in hex.
        var kv: [String: String]
        /// The `Idempotency-Key` of build A's failed `save_note` POST.
        var idempotencyKey: String
    }

    /// S15 step 11.
    struct Snapshots: Codable, Equatable {
        /// Snapshot `P` (a `Profile` named "ada", visited twice), in hex.
        var profile: String
        /// Snapshot `L` (`P` plus a `Legacy` of score 5), in hex.
        var legacy: String
        /// The raw handle of the `Profile`.
        var profileHandle: UInt64
    }

    /// The directory of the files.
    static var directory: URL {
        if let path = ProcessInfo.processInfo.environment["UNDRA_CONTRACT_HANDOVER"], !path.isEmpty {
            return URL(fileURLWithPath: path, isDirectory: true)
        }
        // Tests/ContractTests/Harness/Handover.swift -> the package directory.
        return URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent(".build", isDirectory: true)
            .appendingPathComponent("migration", isDirectory: true)
    }

    private static func file(_ name: String) -> URL {
        return directory.appendingPathComponent(name + ".json", isDirectory: false)
    }

    /// Removes the file `name` (a scenario does this first, so a failure never leaves an older run's
    /// handover in place).
    static func discard(_ name: String) {
        try? FileManager.default.removeItem(at: file(name))
    }

    /// Writes `value` as the file `name`.
    static func write<Value: Encodable>(_ name: String, _ value: Value) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        try encoder.encode(value).write(to: file(name), options: .atomic)
    }

    /// Reads the file `name`, or `nil` if build A did not write it.
    static func read<Value: Decodable>(_ name: String, as type: Value.Type) throws -> Value? {
        let url = file(name)
        guard FileManager.default.fileExists(atPath: url.path) else {
            return nil
        }
        return try JSONDecoder().decode(Value.self, from: Data(contentsOf: url))
    }

    /// Lowercase hex of `bytes`.
    static func hex(_ bytes: [UInt8]) -> String {
        let digits = Array("0123456789abcdef")
        var text = ""
        text.reserveCapacity(bytes.count * 2)
        for byte in bytes {
            text.append(digits[Int(byte >> 4)])
            text.append(digits[Int(byte & 0x0f)])
        }
        return text
    }

    /// The bytes of `text` (lowercase or uppercase hex), or `nil` if it is not hex.
    static func bytes(_ text: String) -> [UInt8]? {
        let characters = Array(text.utf8)
        guard characters.count % 2 == 0 else {
            return nil
        }
        var out: [UInt8] = []
        out.reserveCapacity(characters.count / 2)
        var index = 0
        while index < characters.count {
            guard let high = nibble(characters[index]), let low = nibble(characters[index + 1]) else {
                return nil
            }
            out.append(high << 4 | low)
            index += 2
        }
        return out
    }

    private static func nibble(_ character: UInt8) -> UInt8? {
        switch character {
        case UInt8(ascii: "0") ... UInt8(ascii: "9"): return character - UInt8(ascii: "0")
        case UInt8(ascii: "a") ... UInt8(ascii: "f"): return character - UInt8(ascii: "a") + 10
        case UInt8(ascii: "A") ... UInt8(ascii: "F"): return character - UInt8(ascii: "A") + 10
        default: return nil
        }
    }
}
