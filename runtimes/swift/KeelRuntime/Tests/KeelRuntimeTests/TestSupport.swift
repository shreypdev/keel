import Foundation
import XCTest
import KeelRuntime

// MARK: - Hex

/// Decodes a hex string (either case; spaces are ignored) into bytes.
func hexToBytes(_ hex: String) -> [UInt8] {
    var digits: [UInt8] = []
    for character in hex.utf8 where character != 0x20 {
        digits.append(character)
    }
    precondition(digits.count % 2 == 0, "odd-length hex string")
    var out: [UInt8] = []
    out.reserveCapacity(digits.count / 2)
    var index = 0
    while index < digits.count {
        let high = hexNibble(digits[index])
        let low = hexNibble(digits[index + 1])
        out.append((high << 4) | low)
        index += 2
    }
    return out
}

private func hexNibble(_ character: UInt8) -> UInt8 {
    switch character {
    case 0x30 ... 0x39:
        return character - 0x30
    case 0x61 ... 0x66:
        return character - 0x61 + 10
    case 0x41 ... 0x46:
        return character - 0x41 + 10
    default:
        preconditionFailure("not a hex digit: \(character)")
    }
}

/// Lowercase hex of `bytes`.
func bytesToHex(_ bytes: [UInt8]) -> String {
    let digits: [UInt8] = Array("0123456789abcdef".utf8)
    var out: [UInt8] = []
    out.reserveCapacity(bytes.count * 2)
    for byte in bytes {
        out.append(digits[Int(byte >> 4)])
        out.append(digits[Int(byte & 0x0F)])
    }
    return String(decoding: out, as: UTF8.self)
}

/// The UTF-8 bytes of a string. Swift `String` equality uses canonical equivalence, so tests that
/// must prove a string survived byte-for-byte compare these instead.
func utf8Bytes(_ text: String) -> [UInt8] {
    return Array(text.utf8)
}

// MARK: - Deterministic randomness

/// SplitMix64: a tiny deterministic generator, so fuzz failures reproduce from the seed.
struct SplitMix64: RandomNumberGenerator {
    private var state: UInt64

    init(seed: UInt64) {
        state = seed
    }

    mutating func next() -> UInt64 {
        state = state &+ 0x9E37_79B9_7F4A_7C15
        var z = state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }
}

// MARK: - Assertions

/// Asserts that `body` throws exactly `expected` (and nothing else).
func expectWireError(
    _ expected: WireError,
    file: StaticString = #filePath,
    line: UInt = #line,
    _ body: () throws -> Void
) {
    do {
        try body()
        XCTFail("expected \(expected) but nothing was thrown", file: file, line: line)
    } catch let error as WireError {
        XCTAssertEqual(error, expected, file: file, line: line)
    } catch {
        XCTFail("expected \(expected) but got a non-WireError: \(error)", file: file, line: line)
    }
}

/// Asserts that `value` encodes to exactly `hex` and that `hex` decodes back to an equal value.
func assertCodec<T: KeelCodec & Equatable>(
    _ value: T,
    hex: String,
    _ label: String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) {
    let expected = hexToBytes(hex)
    XCTAssertEqual(bytesToHex(value.keelEncoded()), bytesToHex(expected), "\(label) encode", file: file, line: line)
    do {
        let decoded = try T.keelDecoded(from: expected)
        XCTAssertEqual(decoded, value, "\(label) decode", file: file, line: line)
    } catch {
        XCTFail("\(label) decode threw \(error)", file: file, line: line)
    }
}

/// Asserts that `value` survives an encode/decode round trip.
func assertRoundTrip<T: KeelCodec & Equatable>(
    _ value: T,
    file: StaticString = #filePath,
    line: UInt = #line
) {
    let bytes = value.keelEncoded()
    do {
        let decoded = try T.keelDecoded(from: bytes)
        XCTAssertEqual(decoded, value, "round trip of \(value)", file: file, line: line)
    } catch {
        XCTFail("round trip of \(value) threw \(error)", file: file, line: line)
    }
}

// MARK: - JSON value extraction (for wire-vectors.json)

/// The value stored under `key`, or `NSNull` when the key is absent (which every `json*`
/// extractor below rejects).
func jsonField(_ object: [String: Any], _ key: String) -> Any {
    if let value = object[key] {
        return value
    }
    return NSNull()
}

func jsonInt64(_ any: Any) -> Int64? {
    if let text = any as? String {
        return Int64(text)
    }
    if let number = any as? Int {
        return Int64(number)
    }
    if let number = any as? NSNumber {
        return number.int64Value
    }
    return nil
}

func jsonUInt64(_ any: Any) -> UInt64? {
    if let text = any as? String {
        return UInt64(text)
    }
    if let number = any as? Int {
        return UInt64(exactly: number)
    }
    if let number = any as? NSNumber {
        return number.uint64Value
    }
    return nil
}

func jsonDouble(_ any: Any) -> Double? {
    if let number = any as? Double {
        return number
    }
    if let number = any as? NSNumber {
        return number.doubleValue
    }
    return nil
}

func jsonBool(_ any: Any) -> Bool? {
    if let flag = any as? Bool {
        return flag
    }
    return nil
}

func jsonByteArray(_ any: Any) -> [UInt8]? {
    guard let items = any as? [Any] else {
        return nil
    }
    var out: [UInt8] = []
    for item in items {
        guard let value = jsonInt64(item), let byte = UInt8(exactly: value) else {
            return nil
        }
        out.append(byte)
    }
    return out
}

func jsonInt32Array(_ any: Any) -> [Int32]? {
    guard let items = any as? [Any] else {
        return nil
    }
    var out: [Int32] = []
    for item in items {
        guard let value = jsonInt64(item), let narrowed = Int32(exactly: value) else {
            return nil
        }
        out.append(narrowed)
    }
    return out
}

// MARK: - Test value types (stand-ins for generated code)

/// `record Todo{id:uuid,title:string,done:bool}` from the contract vectors.
struct Todo: KeelCodec, Equatable, Sendable {
    var id: KeelUUID
    var title: String
    var done: Bool

    static func keelDecode(_ r: inout KeelReader) throws -> Todo {
        let id = try KeelUUID.keelDecode(&r)
        let title = try String.keelDecode(&r)
        let done = try Bool.keelDecode(&r)
        return Todo(id: id, title: title, done: done)
    }

    func keelEncode(_ w: inout KeelWriter) {
        id.keelEncode(&w)
        title.keelEncode(&w)
        done.keelEncode(&w)
    }
}

/// `enum Filter{All,Active,Done}` from the contract vectors: a unit enum, `u16` variant index.
enum Filter: UInt16, KeelCodec, Equatable, Sendable {
    case all = 0
    case active = 1
    case done = 2

    static func keelDecode(_ r: inout KeelReader) throws -> Filter {
        let at = r.position
        let raw = try r.readU16()
        guard let value = Filter(rawValue: raw) else {
            throw WireError.invalidTag(tag: UInt32(raw), at: at, type: "Filter")
        }
        return value
    }

    func keelEncode(_ w: inout KeelWriter) {
        w.writeU16(rawValue)
    }
}

/// `enum Shape{Circle{radius:f64},Rect{w:f64,h:f64}}` from the contract vectors: a data enum.
enum Shape: KeelCodec, Equatable, Sendable {
    case circle(radius: Double)
    case rect(w: Double, h: Double)

    static func keelDecode(_ r: inout KeelReader) throws -> Shape {
        let at = r.position
        let index = try r.readU16()
        switch index {
        case 0:
            let radius = try r.readF64()
            return .circle(radius: radius)
        case 1:
            let w = try r.readF64()
            let h = try r.readF64()
            return .rect(w: w, h: h)
        default:
            throw WireError.invalidTag(tag: UInt32(index), at: at, type: "Shape")
        }
    }

    func keelEncode(_ w: inout KeelWriter) {
        switch self {
        case .circle(let radius):
            w.writeU16(0)
            w.writeF64(radius)
        case .rect(let width, let height):
            w.writeU16(1)
            w.writeF64(width)
            w.writeF64(height)
        }
    }
}
