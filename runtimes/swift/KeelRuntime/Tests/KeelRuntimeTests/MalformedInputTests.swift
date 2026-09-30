import Foundation
import XCTest
import KeelRuntime

/// Malformed inputs for every `WireError` case, at the primitive, collection and record level,
/// plus the error type's own behaviour (equality, `Sendable`, descriptions).
final class MalformedInputTests: XCTestCase {
    // MARK: unexpectedEOF

    private struct WidthCase {
        let name: String
        let width: Int
        let decode: ([UInt8]) throws -> Void
    }

    private func widthCase<T: KeelCodec>(_ type: T.Type, _ name: String, _ width: Int) -> WidthCase {
        return WidthCase(name: name, width: width, decode: { bytes in
            _ = try T.keelDecoded(from: bytes)
        })
    }

    func testUnexpectedEOFForEveryFixedWidthType() {
        var cases: [WidthCase] = []
        cases.append(widthCase(UInt8.self, "u8", 1))
        cases.append(widthCase(Int8.self, "i8", 1))
        cases.append(widthCase(UInt16.self, "u16", 2))
        cases.append(widthCase(Int16.self, "i16", 2))
        cases.append(widthCase(UInt32.self, "u32", 4))
        cases.append(widthCase(Int32.self, "i32", 4))
        cases.append(widthCase(Float.self, "f32", 4))
        cases.append(widthCase(UInt64.self, "u64", 8))
        cases.append(widthCase(Int64.self, "i64", 8))
        cases.append(widthCase(Double.self, "f64", 8))
        cases.append(widthCase(Bool.self, "bool", 1))
        cases.append(widthCase(KeelHandle.self, "handle", 8))
        cases.append(widthCase(KeelTimestamp.self, "timestamp", 8))
        cases.append(widthCase(KeelDuration.self, "duration", 8))
        for entry in cases {
            for length in 0 ..< entry.width {
                let bytes = [UInt8](repeating: 0, count: length)
                let decode = entry.decode
                expectWireError(.unexpectedEOF(needed: entry.width, at: 0)) {
                    try decode(bytes)
                }
            }
        }
    }

    func testUnexpectedEOFReportsNeededAndOffset() {
        let bytes: [UInt8] = [1, 2, 3, 4, 5]
        var reader = KeelReader(bytes)
        XCTAssertNoThrow(try reader.readU32())
        expectWireError(.unexpectedEOF(needed: 8, at: 4)) {
            _ = try reader.readU64()
        }
    }

    func testUUIDShortInputFailsInsideTheHalfThatIsCut() {
        let five: [UInt8] = [1, 2, 3, 4, 5]
        expectWireError(.unexpectedEOF(needed: 8, at: 0)) {
            _ = try KeelUUID.keelDecoded(from: five)
        }
        let twelve = [UInt8](repeating: 1, count: 12)
        expectWireError(.unexpectedEOF(needed: 8, at: 8)) {
            _ = try KeelUUID.keelDecoded(from: twelve)
        }
    }

    func testEveryPrefixOfARecordIsAWireError() throws {
        let id = try XCTUnwrap(KeelUUID(uuidString: "123e4567-e89b-12d3-a456-426614174000"))
        let full = Todo(id: id, title: "Milk", done: false).keelEncoded()
        XCTAssertEqual(full.count, 25)
        for length in 0 ..< full.count {
            let cut = Array(full[0 ..< length])
            do {
                _ = try Todo.keelDecoded(from: cut)
                XCTFail("a \(length)-byte prefix cannot be a whole Todo")
            } catch is WireError {
                // expected
            } catch {
                XCTFail("non-WireError \(error)")
            }
        }
    }

    // MARK: invalidUTF8

    func testInvalidUTF8IsRejectedAtTheStartOfTheStringBody() {
        let bad: [(String, String)] = [
            ("lone continuation byte", "0100000080"),
            ("invalid lead byte ff", "01000000ff"),
            ("invalid lead byte c0", "02000000c080"),
            ("overlong slash", "02000000c0af"),
            ("overlong three-byte NUL", "03000000e08080"),
            ("truncated two-byte sequence", "01000000c3"),
            ("truncated three-byte sequence", "02000000e282"),
            ("truncated four-byte sequence, same length as U+FFFD", "03000000f09f98"),
            ("utf-16 surrogate", "03000000eda080"),
            ("above U+10FFFF", "04000000f4908080"),
            ("five-byte form", "05000000f888808080"),
            ("bad byte after good text", "040000006162ff63"),
        ]
        for (label, hex) in bad {
            let bytes = hexToBytes(hex)
            expectWireError(.invalidUTF8(at: 4)) {
                _ = try String.keelDecoded(from: bytes)
            }
            XCTAssertFalse(label.isEmpty)
        }
    }

    func testEveryUTF8LengthBoundaryIsAccepted() throws {
        let good: [(String, String)] = [
            ("U+0000", "0100000000"),
            ("U+007F", "010000007f"),
            ("U+0080", "02000000c280"),
            ("U+07FF", "02000000dfbf"),
            ("U+0800", "03000000e0a080"),
            ("U+D7FF", "03000000ed9fbf"),
            ("U+E000", "03000000ee8080"),
            ("U+FFFF", "03000000efbfbf"),
            ("U+10000", "04000000f0908080"),
            ("U+10FFFF", "04000000f48fbfbf"),
        ]
        for (label, hex) in good {
            let bytes = hexToBytes(hex)
            let decoded = try String.keelDecoded(from: bytes)
            XCTAssertEqual(decoded.keelEncoded(), bytes, label)
        }
    }

    func testInvalidUTF8InsideACollectionReportsTheBodyOffset() {
        // Vec<String> of two: "" then a one-byte string 0xff whose body starts at offset 12.
        let bytes = hexToBytes("020000000000000001000000ff")
        expectWireError(.invalidUTF8(at: 12)) {
            _ = try [String].keelDecoded(from: bytes)
        }
    }

    // MARK: invalidTag

    func testInvalidBoolTag() {
        expectWireError(.invalidTag(tag: 2, at: 0, type: "bool")) {
            _ = try Bool.keelDecoded(from: [2])
        }
        expectWireError(.invalidTag(tag: 255, at: 0, type: "bool")) {
            _ = try Bool.keelDecoded(from: [255])
        }
    }

    func testInvalidOptionTag() {
        expectWireError(.invalidTag(tag: 2, at: 0, type: "Option")) {
            _ = try String?.keelDecoded(from: [2])
        }
        // Inside a Vec, at the tag's own offset.
        expectWireError(.invalidTag(tag: 7, at: 4, type: "Option")) {
            _ = try [Int32?].keelDecoded(from: hexToBytes("0100000007"))
        }
    }

    func testInvalidResultTag() {
        expectWireError(.invalidTag(tag: 2, at: 0, type: "Result")) {
            _ = try KeelResult<Int32, String>.keelDecoded(from: [2, 0, 0, 0, 0])
        }
    }

    func testInvalidEnumVariantIndex() {
        expectWireError(.invalidTag(tag: 3, at: 0, type: "Filter")) {
            _ = try Filter.keelDecoded(from: [3, 0])
        }
        expectWireError(.invalidTag(tag: 2, at: 0, type: "Shape")) {
            _ = try Shape.keelDecoded(from: [2, 0])
        }
        expectWireError(.invalidTag(tag: 0xFFFF, at: 0, type: "Filter")) {
            _ = try Filter.keelDecoded(from: [0xFF, 0xFF])
        }
    }

    // MARK: lengthTooLarge

    func testStringLengthBeyondTheInput() {
        expectWireError(.lengthTooLarge(len: UInt32.max, at: 0)) {
            _ = try String.keelDecoded(from: hexToBytes("ffffffff"))
        }
        expectWireError(.lengthTooLarge(len: 3, at: 0)) {
            _ = try String.keelDecoded(from: hexToBytes("030000006162"))
        }
        // Exactly the remaining length is fine; one more is not.
        XCTAssertEqual(try String.keelDecoded(from: hexToBytes("020000006162")), "ab")
        expectWireError(.lengthTooLarge(len: 3, at: 0)) {
            _ = try String.keelDecoded(from: hexToBytes("030000006162"))
        }
    }

    func testBytesLengthBeyondTheInput() {
        expectWireError(.lengthTooLarge(len: 5, at: 0)) {
            _ = try KeelBytes.keelDecoded(from: hexToBytes("0500000001020304"))
        }
    }

    func testCountBeyondTheInput() {
        // 1e9 Int32s claimed, eight bytes present.
        expectWireError(.lengthTooLarge(len: 1_000_000_000, at: 0)) {
            _ = try [Int32].keelDecoded(from: hexToBytes("00ca9a3b0100000002000000"))
        }
        expectWireError(.lengthTooLarge(len: 1_000_000_000, at: 0)) {
            _ = try [String: Int32].keelDecoded(from: hexToBytes("00ca9a3b"))
        }
        // A nested length reports its own offset: Vec<String> of one with a 100-byte string.
        expectWireError(.lengthTooLarge(len: 100, at: 4)) {
            _ = try [String].keelDecoded(from: hexToBytes("0100000064000000616263"))
        }
    }

    // MARK: trailingBytes

    func testTrailingBytes() {
        expectWireError(.trailingBytes(count: 1)) {
            _ = try UInt8.keelDecoded(from: [1, 2])
        }
        expectWireError(.trailingBytes(count: 1)) {
            _ = try String.keelDecoded(from: hexToBytes("0000000000"))
        }
        expectWireError(.trailingBytes(count: 3)) {
            _ = try KeelUnit.keelDecoded(from: [1, 2, 3])
        }
        expectWireError(.trailingBytes(count: 2)) {
            _ = try [Int32].keelDecoded(from: hexToBytes("000000000102"))
        }
    }

    // MARK: badMagic, unsupportedVersion, schemaMismatch

    func testBadMagic() {
        let bytes = hexToBytes("4b45455801000807060504030201010700000000000000")
        expectWireError(.badMagic) {
            _ = try decodeEnvelope(bytes)
        }
    }

    func testUnsupportedVersion() {
        let bytes = hexToBytes("4b45454c02000807060504030201010700000000000000")
        expectWireError(.unsupportedVersion(2)) {
            _ = try decodeEnvelope(bytes)
        }
    }

    func testSchemaMismatch() {
        expectWireError(.schemaMismatch(expected: 0xAAAA, got: 0xBBBB)) {
            try Envelope.requireSchemaHash(0xBBBB, expected: 0xAAAA)
        }
        XCTAssertNoThrow(try Envelope.requireSchemaHash(0xAAAA, expected: 0xAAAA))
    }

    // MARK: duplicateKey

    func testDuplicateKeyIsRejectedAtTheSecondOccurrence() {
        // { "a": 1, "a": 2 }: the second entry starts at offset 13.
        let strings = hexToBytes("02000000010000006101000000010000006102000000")
        expectWireError(.duplicateKey(at: 13)) {
            _ = try [String: Int32].keelDecoded(from: strings)
        }
        // { 7: true, 7: false }: the second entry starts at offset 6.
        let bytes = hexToBytes("0200000007010700")
        expectWireError(.duplicateKey(at: 6)) {
            _ = try [UInt8: Bool].keelDecoded(from: bytes)
        }
        // A repeat that is not adjacent to the first occurrence: keys 1, 2, 1.
        let spread = hexToBytes("03000000010002000101")
        expectWireError(.duplicateKey(at: 8)) {
            _ = try [UInt8: Bool].keelDecoded(from: spread)
        }
    }

    func testTheDecoderAcceptsUnsortedButUniqueKeys() throws {
        // The encoder sorts for determinism; the decoder does not insist on it.
        let unsorted = hexToBytes("0200000002010100")
        let decoded = try [UInt8: Bool].keelDecoded(from: unsorted)
        XCTAssertEqual(decoded, [1: false, 2: true])
    }

    // MARK: negativeDuration

    func testNegativeDuration() throws {
        expectWireError(.negativeDuration(at: 0)) {
            _ = try KeelDuration.keelDecoded(from: hexToBytes("ffffffffffffffff"))
        }
        expectWireError(.negativeDuration(at: 4)) {
            _ = try [KeelDuration].keelDecoded(from: hexToBytes("01000000ffffffffffffffff"))
        }
        // Zero is fine.
        XCTAssertEqual(try KeelDuration.keelDecoded(from: hexToBytes("0000000000000000")).nanoseconds, 0)
    }

    // MARK: WireError itself

    func testWireErrorIsAnEquatableSendableError() {
        func requireSendable<T: Sendable>(_ value: T) {}
        requireSendable(WireError.badMagic)
        let asError: any Error & Sendable = WireError.trailingBytes(count: 1)
        XCTAssertNotNil(asError as? WireError)

        XCTAssertEqual(WireError.badMagic, WireError.badMagic)
        XCTAssertNotEqual(WireError.unsupportedVersion(1), WireError.unsupportedVersion(2))
        XCTAssertNotEqual(WireError.unexpectedEOF(needed: 4, at: 0), WireError.unexpectedEOF(needed: 4, at: 1))
        XCTAssertNotEqual(
            WireError.invalidTag(tag: 1, at: 0, type: "A"),
            WireError.invalidTag(tag: 1, at: 0, type: "B")
        )
        XCTAssertNotEqual(WireError.duplicateKey(at: 1), WireError.negativeDuration(at: 1))
    }

    func testWireErrorDescriptions() {
        XCTAssertEqual(
            WireError.unexpectedEOF(needed: 4, at: 10).description,
            "unexpected end of input: needed 4 byte(s) at offset 10"
        )
        XCTAssertEqual(WireError.invalidUTF8(at: 3).description, "invalid UTF-8 in string at offset 3")
        XCTAssertEqual(
            WireError.invalidTag(tag: 9, at: 2, type: "Option").description,
            "invalid tag 9 for Option at offset 2"
        )
        XCTAssertEqual(
            WireError.lengthTooLarge(len: 100, at: 4).description,
            "length 100 at offset 4 exceeds the remaining input"
        )
        XCTAssertEqual(WireError.trailingBytes(count: 3).description, "3 trailing byte(s) after the end of the value")
        XCTAssertEqual(WireError.badMagic.description, "bad envelope magic (expected \"KEEL\")")
        XCTAssertEqual(
            WireError.unsupportedVersion(2).description,
            "unsupported envelope version 2 (this runtime speaks version 1)"
        )
        XCTAssertEqual(
            WireError.schemaMismatch(expected: 255, got: 16).description,
            "schema mismatch: expected 0xff, got 0x10"
        )
        XCTAssertEqual(WireError.duplicateKey(at: 7).description, "duplicate map key at offset 7")
        XCTAssertEqual(WireError.negativeDuration(at: 0).description, "negative duration at offset 0")
        XCTAssertEqual(String(describing: WireError.badMagic), WireError.badMagic.description)
        XCTAssertEqual("\(WireError.duplicateKey(at: 7))", "duplicate map key at offset 7")
    }
}
