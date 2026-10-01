import XCTest
import UndraRuntime

/// The 23-byte transport envelope (docs/SPEC.md section 3.2).
final class EnvelopeTests: XCTestCase {
    private let schema: UInt64 = 0x0102_0304_0506_0708

    /// A well-formed envelope for the malformed-input tests to corrupt one field at a time.
    private func validEnvelope(payload: [UInt8] = [0xAA, 0xBB, 0xCC]) -> [UInt8] {
        return encodeEnvelope(kind: .call, seq: 7, schemaHash: schema, payload: payload)
    }

    func testHeaderLengthIs23() {
        XCTAssertEqual(Envelope.headerLength, 23)
        XCTAssertEqual(validEnvelope(payload: []).count, 23)
        XCTAssertEqual(validEnvelope(payload: [1, 2, 3]).count, 26)
    }

    func testContractVector() throws {
        let expected = hexToBytes("554e445201000807060504030201010700000003000000aabbcc")
        XCTAssertEqual(validEnvelope(), expected)
        let decoded = try decodeEnvelope(expected)
        XCTAssertEqual(decoded.kind, .call)
        XCTAssertEqual(decoded.seq, 7)
        XCTAssertEqual(decoded.schemaHash, schema)
        XCTAssertEqual(Array(decoded.payload), [0xAA, 0xBB, 0xCC])
    }

    func testMagicAndVersionConstants() {
        XCTAssertEqual(Envelope.magic, [0x55, 0x4E, 0x44, 0x52])
        XCTAssertEqual(Envelope.version, 1)
        let bytes = validEnvelope()
        XCTAssertEqual(Array(bytes[0 ..< 4]), Envelope.magic)
        XCTAssertEqual(Array(bytes[4 ..< 6]), [1, 0])
    }

    func testAllSixteenKindsRoundTripWithTheirSpecNumbers() throws {
        let expected: [(Envelope.Kind, UInt8)] = [
            (.call, 1), (.reply, 2), (.changeSet, 3), (.portCall, 4),
            (.portReply, 5), (.cancel, 6), (.streamCredit, 7), (.streamItem, 8),
            (.observe, 9), (.release, 10), (.event, 11), (.hello, 12),
            (.log, 13), (.timerFired, 14), (.snapshot, 15), (.restore, 16),
        ]
        XCTAssertEqual(Envelope.Kind.allCases.count, 16)
        XCTAssertEqual(expected.count, 16)
        for (kind, number) in expected {
            XCTAssertEqual(kind.rawValue, number)
            XCTAssertEqual(Envelope.Kind(rawValue: number), kind)
            let payload: [UInt8] = [number, number]
            let bytes = encodeEnvelope(kind: kind, seq: UInt32(number), schemaHash: schema, payload: payload)
            let decoded = try decodeEnvelope(bytes)
            XCTAssertEqual(decoded.kind, kind)
            XCTAssertEqual(decoded.seq, UInt32(number))
            XCTAssertEqual(Array(decoded.payload), payload)
        }
    }

    func testRoundTripPayloadSizesAndExtremeFields() throws {
        let sizes = [0, 1, 255, 256, 70_000]
        for size in sizes {
            var payload: [UInt8] = []
            payload.reserveCapacity(size)
            for index in 0 ..< size {
                payload.append(UInt8(truncatingIfNeeded: index))
            }
            let bytes = encodeEnvelope(kind: .changeSet, seq: UInt32.max, schemaHash: UInt64.max, payload: payload)
            XCTAssertEqual(bytes.count, 23 + size)
            let decoded = try decodeEnvelope(bytes)
            XCTAssertEqual(decoded.kind, .changeSet)
            XCTAssertEqual(decoded.seq, UInt32.max)
            XCTAssertEqual(decoded.schemaHash, UInt64.max)
            XCTAssertEqual(decoded.payload.count, size)
            XCTAssertEqual(Array(decoded.payload), payload)
        }
    }

    func testDecodedPayloadIsASliceOfTheInput() throws {
        let bytes = validEnvelope(payload: [1, 2, 3, 4])
        let decoded = try decodeEnvelope(bytes)
        XCTAssertEqual(decoded.payload.startIndex, 23)
        XCTAssertEqual(decoded.payload.endIndex, 27)
    }

    func testDecodeFromASliceWithANonZeroStartIndex() throws {
        let envelope = validEnvelope()
        let padded: [UInt8] = [0xEE, 0xEE, 0xEE] + envelope + [0xEE]
        let slice = padded[3 ..< 3 + envelope.count]
        let decoded = try decodeEnvelope(slice: slice)
        XCTAssertEqual(decoded.kind, .call)
        XCTAssertEqual(decoded.seq, 7)
        XCTAssertEqual(Array(decoded.payload), [0xAA, 0xBB, 0xCC])
        // Offsets in errors are relative to the slice.
        var corrupt = envelope
        corrupt[14] = 0
        let paddedCorrupt: [UInt8] = [0xEE, 0xEE, 0xEE] + corrupt
        expectWireError(.invalidTag(tag: 0, at: 14, type: "Kind")) {
            _ = try decodeEnvelope(slice: paddedCorrupt[3 ..< paddedCorrupt.count])
        }
    }

    func testWriteHeaderMatchesEncodeEnvelope() {
        var writer = UndraWriter()
        Envelope.writeHeader(into: &writer, kind: .call, seq: 7, schemaHash: schema, payloadLength: 3)
        writer.writeRaw([0xAA, 0xBB, 0xCC])
        XCTAssertEqual(writer.finish(), validEnvelope())
    }

    // MARK: Malformed input, one error at a time

    func testEmptyAndShortInputIsUnexpectedEOF() {
        let empty: [UInt8] = []
        expectWireError(.unexpectedEOF(needed: 4, at: 0)) {
            _ = try decodeEnvelope(empty)
        }
        let full = validEnvelope(payload: [])
        // Truncate at every length short of a full header: always unexpectedEOF, never a trap.
        for length in 0 ..< 23 {
            let cut = Array(full[0 ..< length])
            do {
                _ = try decodeEnvelope(cut)
                XCTFail("a \(length)-byte input cannot be a valid envelope")
            } catch let error as WireError {
                switch error {
                case .unexpectedEOF:
                    break
                default:
                    XCTFail("\(length) bytes: expected unexpectedEOF, got \(error)")
                }
            } catch {
                XCTFail("\(length) bytes: non-WireError \(error)")
            }
        }
    }

    func testBadMagic() {
        var bytes = validEnvelope()
        bytes[3] = UInt8(ascii: "X")
        expectWireError(.badMagic) {
            _ = try decodeEnvelope(bytes)
        }
        var lowercase = validEnvelope()
        lowercase[0] = UInt8(ascii: "u")
        expectWireError(.badMagic) {
            _ = try decodeEnvelope(lowercase)
        }
    }

    func testUnsupportedVersion() {
        for version: UInt16 in [0, 2, 0x0100, 0xFFFF] {
            var bytes = validEnvelope()
            bytes[4] = UInt8(truncatingIfNeeded: version)
            bytes[5] = UInt8(truncatingIfNeeded: version >> 8)
            expectWireError(.unsupportedVersion(version)) {
                _ = try decodeEnvelope(bytes)
            }
        }
    }

    func testUnknownKind() {
        for kind: UInt8 in [0, 17, 100, 255] {
            var bytes = validEnvelope()
            bytes[14] = kind
            expectWireError(.invalidTag(tag: UInt32(kind), at: 14, type: "Kind")) {
                _ = try decodeEnvelope(bytes)
            }
        }
    }

    func testPayloadLengthLargerThanTheInput() {
        var bytes = validEnvelope()
        bytes[19] = 4 // claims 4 payload bytes, only 3 follow
        expectWireError(.lengthTooLarge(len: 4, at: 19)) {
            _ = try decodeEnvelope(bytes)
        }
        var huge = validEnvelope()
        huge[19] = 0xFF
        huge[20] = 0xFF
        huge[21] = 0xFF
        huge[22] = 0xFF
        expectWireError(.lengthTooLarge(len: UInt32.max, at: 19)) {
            _ = try decodeEnvelope(huge)
        }
    }

    func testTrailingBytesAfterThePayload() {
        var bytes = validEnvelope()
        bytes.append(0)
        expectWireError(.trailingBytes(count: 1)) {
            _ = try decodeEnvelope(bytes)
        }
        var shorter = validEnvelope()
        shorter[19] = 2 // payload claims 2 bytes; the third is now trailing
        expectWireError(.trailingBytes(count: 1)) {
            _ = try decodeEnvelope(shorter)
        }
    }

    func testSchemaMismatch() throws {
        XCTAssertNoThrow(try Envelope.requireSchemaHash(schema, expected: schema))
        expectWireError(.schemaMismatch(expected: 1, got: 2)) {
            try Envelope.requireSchemaHash(2, expected: 1)
        }
        // The decoder returns the hash without judging it.
        let decoded = try decodeEnvelope(validEnvelope())
        expectWireError(.schemaMismatch(expected: 99, got: schema)) {
            try Envelope.requireSchemaHash(decoded.schemaHash, expected: 99)
        }
    }
}
