import Foundation
import XCTest
import KeelRuntime

/// `KeelUUID`, `KeelHandle`, `KeelTimestamp`, `KeelDuration`, `KeelResult`, `KeelUnit` and the
/// Foundation bridges.
final class TypedValueTests: XCTestCase {
    // MARK: KeelUUID

    func testUUIDWireFormatIsSixteenRawBytes() throws {
        let text = "123e4567-e89b-12d3-a456-426614174000"
        let uuid = try XCTUnwrap(KeelUUID(uuidString: text))
        assertCodec(uuid, hex: "123e4567e89b12d3a456426614174000")
        XCTAssertEqual(uuid.uuidString, text)
        XCTAssertEqual(uuid.description, text)
        XCTAssertEqual(uuid.bytes, hexToBytes("123e4567e89b12d3a456426614174000"))
        XCTAssertEqual(uuid.high, 0x123E_4567_E89B_12D3)
        XCTAssertEqual(uuid.low, 0xA456_4266_1417_4000)
    }

    func testUUIDStringIsLowercaseHyphenatedAndParsingIsCaseInsensitive() throws {
        let upper = try XCTUnwrap(KeelUUID(uuidString: "123E4567-E89B-12D3-A456-426614174000"))
        XCTAssertEqual(upper.uuidString, "123e4567-e89b-12d3-a456-426614174000")
        let zero = KeelUUID(high: 0, low: 0)
        XCTAssertEqual(zero.uuidString, "00000000-0000-0000-0000-000000000000")
        let ones = KeelUUID(high: UInt64.max, low: UInt64.max)
        XCTAssertEqual(ones.uuidString, "ffffffff-ffff-ffff-ffff-ffffffffffff")
        assertRoundTrip(zero)
        assertRoundTrip(ones)
    }

    func testUUIDRejectsMalformedStrings() {
        let bad: [String] = [
            "",
            "123e4567e89b12d3a456426614174000", // no hyphens
            "123e4567-e89b-12d3-a456-42661417400", // one digit short
            "123e4567-e89b-12d3-a456-4266141740000", // one digit long
            "123e4567-e89b-12d3-a456-42661417400g", // non-hex digit
            "123e4567_e89b-12d3-a456-426614174000", // wrong separator
            "123e4567-e89b-12d3-a456-42661417400\u{E9}", // non-ASCII
            "{123e4567-e89b-12d3-a456-426614174000}",
        ]
        for text in bad {
            XCTAssertNil(KeelUUID(uuidString: text), "should reject \"\(text)\"")
        }
    }

    func testUUIDFromBytes() throws {
        let bytes = hexToBytes("00112233445566778899aabbccddeeff")
        let uuid = try XCTUnwrap(KeelUUID(bytes: bytes))
        XCTAssertEqual(uuid.bytes, bytes)
        XCTAssertEqual(uuid.uuidString, "00112233-4455-6677-8899-aabbccddeeff")
        for index in 0 ..< 16 {
            XCTAssertEqual(uuid.byte(at: index), bytes[index])
        }
        XCTAssertNil(KeelUUID(bytes: []))
        XCTAssertNil(KeelUUID(bytes: [UInt8](repeating: 0, count: 15)))
        XCTAssertNil(KeelUUID(bytes: [UInt8](repeating: 0, count: 17)))
    }

    func testUUIDOrderingAndHashing() throws {
        let low = KeelUUID(high: 1, low: 5)
        let mid = KeelUUID(high: 1, low: 6)
        let high = KeelUUID(high: 2, low: 0)
        XCTAssertTrue(low < mid)
        XCTAssertTrue(mid < high)
        XCTAssertFalse(high < low)
        XCTAssertEqual(Set([low, mid, high, low]).count, 3)
    }

    func testUUIDBridgesToFoundation() throws {
        let foundation = try XCTUnwrap(UUID(uuidString: "123E4567-E89B-12D3-A456-426614174000"))
        let keel = KeelUUID(foundation)
        XCTAssertEqual(keel.uuidString, "123e4567-e89b-12d3-a456-426614174000")
        XCTAssertEqual(keel.uuid, foundation)
        // `Foundation.UUID` itself is a `KeelCodec` with the same wire bytes.
        assertCodec(foundation, hex: "123e4567e89b12d3a456426614174000")
        for _ in 0 ..< 50 {
            let random = UUID()
            XCTAssertEqual(KeelUUID(random).uuid, random)
            XCTAssertEqual(KeelUUID(random).uuidString, random.uuidString.lowercased())
            assertRoundTrip(random)
        }
    }

    // MARK: KeelHandle

    func testHandleFields() {
        let handle = KeelHandle(rawValue: 4_294_967_297)
        XCTAssertEqual(handle.index, 1)
        XCTAssertEqual(handle.generation, 1)
        XCTAssertFalse(handle.isNull)
        assertCodec(handle, hex: "0100000001000000")

        let built = KeelHandle(index: 0xDEAD_BEEF, generation: 0x0000_00FF)
        XCTAssertEqual(built.rawValue, 0x0000_00FF_DEAD_BEEF)
        XCTAssertEqual(built.index, 0xDEAD_BEEF)
        XCTAssertEqual(built.generation, 0xFF)
        assertRoundTrip(built)
    }

    func testNullHandle() {
        XCTAssertTrue(KeelHandle.null.isNull)
        XCTAssertEqual(KeelHandle.null.rawValue, 0)
        XCTAssertEqual(KeelHandle.null.index, 0)
        XCTAssertEqual(KeelHandle.null.generation, 0)
        assertCodec(KeelHandle.null, hex: "0000000000000000")
        // Index 0 with generation 1 is a real handle, not null.
        XCTAssertFalse(KeelHandle(index: 0, generation: 1).isNull)
        XCTAssertEqual(KeelHandle.null.description, "handle(null)")
        XCTAssertEqual(KeelHandle(index: 3, generation: 2).description, "handle(index: 3, generation: 2)")
    }

    func testHandleExtremesAndHashing() {
        let maximum = KeelHandle(rawValue: UInt64.max)
        XCTAssertEqual(maximum.index, UInt32.max)
        XCTAssertEqual(maximum.generation, UInt32.max)
        assertRoundTrip(maximum)
        XCTAssertEqual(Set([KeelHandle.null, maximum, KeelHandle.null]).count, 2)
    }

    // MARK: KeelTimestamp

    func testTimestampWireFormat() {
        let stamp = KeelTimestamp(millisecondsSinceEpoch: 1_727_654_400_000)
        assertCodec(stamp, hex: "00103a4092010000")
        assertRoundTrip(KeelTimestamp(millisecondsSinceEpoch: Int64.min))
        assertRoundTrip(KeelTimestamp(millisecondsSinceEpoch: Int64.max))
        assertRoundTrip(KeelTimestamp(millisecondsSinceEpoch: -1))
        XCTAssertTrue(KeelTimestamp(millisecondsSinceEpoch: 1) < KeelTimestamp(millisecondsSinceEpoch: 2))
    }

    func testTimestampDateConversion() {
        let epoch = KeelTimestamp(Date(timeIntervalSince1970: 0))
        XCTAssertEqual(epoch.millisecondsSinceEpoch, 0)
        let samples: [Int64] = [0, 1, -1, 999, 1000, -1500, 1_727_654_400_000, 1_727_654_400_001, -62_135_596_800_000]
        for milliseconds in samples {
            let stamp = KeelTimestamp(millisecondsSinceEpoch: milliseconds)
            XCTAssertEqual(KeelTimestamp(stamp.date).millisecondsSinceEpoch, milliseconds, "\(milliseconds) ms")
        }
        // Rounds to the nearest millisecond.
        XCTAssertEqual(KeelTimestamp(Date(timeIntervalSince1970: 1.0004)).millisecondsSinceEpoch, 1000)
        XCTAssertEqual(KeelTimestamp(Date(timeIntervalSince1970: 1.0006)).millisecondsSinceEpoch, 1001)
        XCTAssertEqual(KeelTimestamp(Date(timeIntervalSince1970: -1.0006)).millisecondsSinceEpoch, -1001)
    }

    func testTimestampSaturatesOutOfRangeDates() {
        XCTAssertEqual(KeelTimestamp(Date(timeIntervalSince1970: 1e30)).millisecondsSinceEpoch, Int64.max)
        XCTAssertEqual(KeelTimestamp(Date(timeIntervalSince1970: -1e30)).millisecondsSinceEpoch, Int64.min)
        XCTAssertEqual(KeelTimestamp(Date(timeIntervalSince1970: Double.nan)).millisecondsSinceEpoch, 0)
    }

    func testDateIsAKeelCodec() throws {
        let date = Date(timeIntervalSince1970: 1_727_654_400)
        assertCodec(date, hex: "00103a4092010000")
    }

    // MARK: KeelDuration

    func testDurationWireFormat() {
        assertCodec(KeelDuration(nanoseconds: 1_500_000_000), hex: "002f685900000000")
        assertCodec(KeelDuration(.milliseconds(1500)), hex: "002f685900000000")
        assertCodec(KeelDuration(nanoseconds: 0), hex: "0000000000000000")
        assertCodec(KeelDuration(nanoseconds: Int64.max), hex: "ffffffffffffff7f")
        assertRoundTrip(KeelDuration(nanoseconds: 1))
        assertRoundTrip(KeelDuration(.seconds(86_400 * 365)))
    }

    func testDurationConversions() {
        XCTAssertEqual(KeelDuration(.milliseconds(1500)).nanoseconds, 1_500_000_000)
        XCTAssertEqual(KeelDuration(.seconds(2)).nanoseconds, 2_000_000_000)
        XCTAssertEqual(KeelDuration(.microseconds(7)).nanoseconds, 7000)
        XCTAssertEqual(KeelDuration(nanoseconds: 1_500_000_000).duration, .milliseconds(1500))
        XCTAssertEqual(KeelDuration(nanoseconds: 0).duration, .zero)
        XCTAssertTrue(KeelDuration(.seconds(1)) < KeelDuration(.seconds(2)))
        // Sub-nanosecond precision is truncated.
        let oneAndAHalfNanoseconds: Duration = Duration.nanoseconds(3) / 2
        XCTAssertEqual(KeelDuration(oneAndAHalfNanoseconds).nanoseconds, 1)
    }

    func testDurationSaturatesBeyondTheInt64NanosecondRange() {
        XCTAssertEqual(KeelDuration(.seconds(Int64.max)).nanoseconds, Int64.max)
        XCTAssertEqual(KeelDuration(.seconds(Int64.min)).nanoseconds, Int64.min)
        // 10 billion seconds is about 317 years: too large for 64-bit nanoseconds.
        XCTAssertEqual(KeelDuration(.seconds(10_000_000_000)).nanoseconds, Int64.max)
    }

    func testNegativeDurationEncodesAsIsAndDecodingRejectsIt() {
        let negative = KeelDuration(.seconds(-5))
        XCTAssertEqual(negative.nanoseconds, -5_000_000_000)
        let bytes = negative.keelEncoded()
        expectWireError(.negativeDuration(at: 0)) {
            _ = try KeelDuration.keelDecoded(from: bytes)
        }
        expectWireError(.negativeDuration(at: 0)) {
            _ = try Duration.keelDecoded(from: bytes)
        }
        // Int64.min is negative too.
        expectWireError(.negativeDuration(at: 0)) {
            _ = try KeelDuration.keelDecoded(from: hexToBytes("0000000000000080"))
        }
    }

    func testDurationIsAKeelCodec() {
        assertCodec(Duration.milliseconds(1500), hex: "002f685900000000")
    }

    // MARK: KeelResult

    func testResultWireFormat() {
        let ok: KeelResult<Int32, String> = .ok(5)
        let err: KeelResult<Int32, String> = .err("bad")
        assertCodec(ok, hex: "0005000000")
        assertCodec(err, hex: "0103000000626164")
        let unitOk: KeelResult<KeelUnit, String> = .ok(KeelUnit())
        assertCodec(unitOk, hex: "00")
        let nested: KeelResult<[Int32], Shape> = .err(.circle(radius: 1.0))
        assertRoundTrip(nested)
    }

    func testResultBridgesToSwiftError() throws {
        struct Failure: Error, Equatable {
            let code: Int
        }
        let ok: KeelResult<String, Failure> = .ok("fine")
        let err: KeelResult<String, Failure> = .err(Failure(code: 7))
        XCTAssertEqual(try ok.get(), "fine")
        XCTAssertThrowsError(try err.get()) { error in
            XCTAssertEqual(error as? Failure, Failure(code: 7))
        }
        XCTAssertEqual(ok.result, .success("fine"))
        XCTAssertEqual(err.result, .failure(Failure(code: 7)))
    }

    // MARK: KeelUnit and Void helpers

    func testUnitOccupiesNoBytes() throws {
        XCTAssertEqual(KeelUnit().keelEncoded(), [])
        XCTAssertEqual(try KeelUnit.keelDecoded(from: []), KeelUnit())

        var writer = KeelWriter()
        keelEncodeVoid(&writer)
        XCTAssertEqual(writer.count, 0)

        let bytes: [UInt8] = [1, 2]
        var reader = KeelReader(bytes)
        try keelDecodeVoid(&reader)
        XCTAssertEqual(reader.position, 0)
    }
}
