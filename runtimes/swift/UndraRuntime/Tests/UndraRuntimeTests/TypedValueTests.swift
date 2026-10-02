import Foundation
import XCTest
import UndraRuntime

/// `UndraUUID`, `UndraHandle`, `UndraTimestamp`, `UndraDuration`, `UndraResult`, `UndraUnit` and the
/// Foundation bridges.
final class TypedValueTests: XCTestCase {
    // MARK: UndraUUID

    func testUUIDWireFormatIsSixteenRawBytes() throws {
        let text = "123e4567-e89b-12d3-a456-426614174000"
        let uuid = try XCTUnwrap(UndraUUID(uuidString: text))
        assertCodec(uuid, hex: "123e4567e89b12d3a456426614174000")
        XCTAssertEqual(uuid.uuidString, text)
        XCTAssertEqual(uuid.description, text)
        XCTAssertEqual(uuid.bytes, hexToBytes("123e4567e89b12d3a456426614174000"))
        XCTAssertEqual(uuid.high, 0x123E_4567_E89B_12D3)
        XCTAssertEqual(uuid.low, 0xA456_4266_1417_4000)
    }

    func testUUIDStringIsLowercaseHyphenatedAndParsingIsCaseInsensitive() throws {
        let upper = try XCTUnwrap(UndraUUID(uuidString: "123E4567-E89B-12D3-A456-426614174000"))
        XCTAssertEqual(upper.uuidString, "123e4567-e89b-12d3-a456-426614174000")
        let zero = UndraUUID(high: 0, low: 0)
        XCTAssertEqual(zero.uuidString, "00000000-0000-0000-0000-000000000000")
        let ones = UndraUUID(high: UInt64.max, low: UInt64.max)
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
            XCTAssertNil(UndraUUID(uuidString: text), "should reject \"\(text)\"")
        }
    }

    func testUUIDFromBytes() throws {
        let bytes = hexToBytes("00112233445566778899aabbccddeeff")
        let uuid = try XCTUnwrap(UndraUUID(bytes: bytes))
        XCTAssertEqual(uuid.bytes, bytes)
        XCTAssertEqual(uuid.uuidString, "00112233-4455-6677-8899-aabbccddeeff")
        for index in 0 ..< 16 {
            XCTAssertEqual(uuid.byte(at: index), bytes[index])
        }
        XCTAssertNil(UndraUUID(bytes: []))
        XCTAssertNil(UndraUUID(bytes: [UInt8](repeating: 0, count: 15)))
        XCTAssertNil(UndraUUID(bytes: [UInt8](repeating: 0, count: 17)))
    }

    func testUUIDOrderingAndHashing() throws {
        let low = UndraUUID(high: 1, low: 5)
        let mid = UndraUUID(high: 1, low: 6)
        let high = UndraUUID(high: 2, low: 0)
        XCTAssertTrue(low < mid)
        XCTAssertTrue(mid < high)
        XCTAssertFalse(high < low)
        XCTAssertEqual(Set([low, mid, high, low]).count, 3)
    }

    func testUUIDBridgesToFoundation() throws {
        let foundation = try XCTUnwrap(UUID(uuidString: "123E4567-E89B-12D3-A456-426614174000"))
        let undra = UndraUUID(foundation)
        XCTAssertEqual(undra.uuidString, "123e4567-e89b-12d3-a456-426614174000")
        XCTAssertEqual(undra.uuid, foundation)
        // `Foundation.UUID` itself is an `UndraCodec` with the same wire bytes.
        assertCodec(foundation, hex: "123e4567e89b12d3a456426614174000")
        for _ in 0 ..< 50 {
            let random = UUID()
            XCTAssertEqual(UndraUUID(random).uuid, random)
            XCTAssertEqual(UndraUUID(random).uuidString, random.uuidString.lowercased())
            assertRoundTrip(random)
        }
    }

    // MARK: UndraHandle

    func testHandleFields() {
        // 24 bits of slot, 40 bits of generation (ADR-040).
        let handle = UndraHandle(rawValue: 0x0100_0001)
        XCTAssertEqual(handle.index, 1)
        XCTAssertEqual(handle.generation, 1)
        XCTAssertFalse(handle.isNull)
        assertCodec(handle, hex: "0100000100000000")

        let built = UndraHandle(index: 0x00AB_CDEF, generation: 0xFF_0000_00FF)
        XCTAssertEqual(built.rawValue, 0xFF00_0000_FFAB_CDEF)
        XCTAssertEqual(built.index, 0x00AB_CDEF)
        XCTAssertEqual(built.generation, 0xFF_0000_00FF)
        assertRoundTrip(built)
    }

    func testNullHandle() {
        XCTAssertTrue(UndraHandle.null.isNull)
        XCTAssertEqual(UndraHandle.null.rawValue, 0)
        XCTAssertEqual(UndraHandle.null.index, 0)
        XCTAssertEqual(UndraHandle.null.generation, 0)
        assertCodec(UndraHandle.null, hex: "0000000000000000")
        // Index 0 with generation 1 is a real handle, not null.
        XCTAssertFalse(UndraHandle(index: 0, generation: 1).isNull)
        XCTAssertEqual(UndraHandle.null.description, "handle(null)")
        XCTAssertEqual(UndraHandle(index: 3, generation: 2).description, "handle(index: 3, generation: 2)")
    }

    func testHandleExtremesAndHashing() {
        let maximum = UndraHandle(rawValue: UInt64.max)
        // 24 bits of slot, 40 of generation (ADR-040).
        XCTAssertEqual(maximum.index, (1 << 24) - 1)
        XCTAssertEqual(maximum.generation, (1 << 40) - 1)
        XCTAssertEqual(UndraHandle(index: 1, generation: 1).rawValue, 0x0100_0001)
        assertRoundTrip(maximum)
        XCTAssertEqual(Set([UndraHandle.null, maximum, UndraHandle.null]).count, 2)
    }

    // MARK: UndraTimestamp

    func testTimestampWireFormat() {
        let stamp = UndraTimestamp(millisecondsSinceEpoch: 1_727_654_400_000)
        assertCodec(stamp, hex: "00103a4092010000")
        assertRoundTrip(UndraTimestamp(millisecondsSinceEpoch: Int64.min))
        assertRoundTrip(UndraTimestamp(millisecondsSinceEpoch: Int64.max))
        assertRoundTrip(UndraTimestamp(millisecondsSinceEpoch: -1))
        XCTAssertTrue(UndraTimestamp(millisecondsSinceEpoch: 1) < UndraTimestamp(millisecondsSinceEpoch: 2))
    }

    func testTimestampDateConversion() {
        let epoch = UndraTimestamp(Date(timeIntervalSince1970: 0))
        XCTAssertEqual(epoch.millisecondsSinceEpoch, 0)
        let samples: [Int64] = [0, 1, -1, 999, 1000, -1500, 1_727_654_400_000, 1_727_654_400_001, -62_135_596_800_000]
        for milliseconds in samples {
            let stamp = UndraTimestamp(millisecondsSinceEpoch: milliseconds)
            XCTAssertEqual(UndraTimestamp(stamp.date).millisecondsSinceEpoch, milliseconds, "\(milliseconds) ms")
        }
        // Rounds to the nearest millisecond.
        XCTAssertEqual(UndraTimestamp(Date(timeIntervalSince1970: 1.0004)).millisecondsSinceEpoch, 1000)
        XCTAssertEqual(UndraTimestamp(Date(timeIntervalSince1970: 1.0006)).millisecondsSinceEpoch, 1001)
        XCTAssertEqual(UndraTimestamp(Date(timeIntervalSince1970: -1.0006)).millisecondsSinceEpoch, -1001)
    }

    func testTimestampSaturatesOutOfRangeDates() {
        XCTAssertEqual(UndraTimestamp(Date(timeIntervalSince1970: 1e30)).millisecondsSinceEpoch, Int64.max)
        XCTAssertEqual(UndraTimestamp(Date(timeIntervalSince1970: -1e30)).millisecondsSinceEpoch, Int64.min)
        XCTAssertEqual(UndraTimestamp(Date(timeIntervalSince1970: Double.nan)).millisecondsSinceEpoch, 0)
    }

    func testDateIsAUndraCodec() throws {
        let date = Date(timeIntervalSince1970: 1_727_654_400)
        assertCodec(date, hex: "00103a4092010000")
    }

    // MARK: UndraDuration

    func testDurationWireFormat() {
        assertCodec(UndraDuration(nanoseconds: 1_500_000_000), hex: "002f685900000000")
        assertCodec(UndraDuration(.milliseconds(1500)), hex: "002f685900000000")
        assertCodec(UndraDuration(nanoseconds: 0), hex: "0000000000000000")
        assertCodec(UndraDuration(nanoseconds: Int64.max), hex: "ffffffffffffff7f")
        assertRoundTrip(UndraDuration(nanoseconds: 1))
        assertRoundTrip(UndraDuration(.seconds(86_400 * 365)))
    }

    func testDurationConversions() {
        XCTAssertEqual(UndraDuration(.milliseconds(1500)).nanoseconds, 1_500_000_000)
        XCTAssertEqual(UndraDuration(.seconds(2)).nanoseconds, 2_000_000_000)
        XCTAssertEqual(UndraDuration(.microseconds(7)).nanoseconds, 7000)
        XCTAssertEqual(UndraDuration(nanoseconds: 1_500_000_000).duration, .milliseconds(1500))
        XCTAssertEqual(UndraDuration(nanoseconds: 0).duration, .zero)
        XCTAssertTrue(UndraDuration(.seconds(1)) < UndraDuration(.seconds(2)))
        // Sub-nanosecond precision is truncated.
        let oneAndAHalfNanoseconds: Duration = Duration.nanoseconds(3) / 2
        XCTAssertEqual(UndraDuration(oneAndAHalfNanoseconds).nanoseconds, 1)
    }

    func testDurationSaturatesBeyondTheInt64NanosecondRange() {
        XCTAssertEqual(UndraDuration(.seconds(Int64.max)).nanoseconds, Int64.max)
        XCTAssertEqual(UndraDuration(.seconds(Int64.min)).nanoseconds, Int64.min)
        // 10 billion seconds is about 317 years: too large for 64-bit nanoseconds.
        XCTAssertEqual(UndraDuration(.seconds(10_000_000_000)).nanoseconds, Int64.max)
    }

    func testNegativeDurationEncodesAsIsAndDecodingRejectsIt() {
        let negative = UndraDuration(.seconds(-5))
        XCTAssertEqual(negative.nanoseconds, -5_000_000_000)
        let bytes = negative.undraEncoded()
        expectWireError(.negativeDuration(at: 0)) {
            _ = try UndraDuration.undraDecoded(from: bytes)
        }
        expectWireError(.negativeDuration(at: 0)) {
            _ = try Duration.undraDecoded(from: bytes)
        }
        // Int64.min is negative too.
        expectWireError(.negativeDuration(at: 0)) {
            _ = try UndraDuration.undraDecoded(from: hexToBytes("0000000000000080"))
        }
    }

    func testDurationIsAUndraCodec() {
        assertCodec(Duration.milliseconds(1500), hex: "002f685900000000")
    }

    // MARK: UndraResult

    func testResultWireFormat() {
        let ok: UndraResult<Int32, String> = .ok(5)
        let err: UndraResult<Int32, String> = .err("bad")
        assertCodec(ok, hex: "0005000000")
        assertCodec(err, hex: "0103000000626164")
        let unitOk: UndraResult<UndraUnit, String> = .ok(UndraUnit())
        assertCodec(unitOk, hex: "00")
        let nested: UndraResult<[Int32], Shape> = .err(.circle(radius: 1.0))
        assertRoundTrip(nested)
    }

    func testResultBridgesToSwiftError() throws {
        struct Failure: Error, Equatable {
            let code: Int
        }
        let ok: UndraResult<String, Failure> = .ok("fine")
        let err: UndraResult<String, Failure> = .err(Failure(code: 7))
        XCTAssertEqual(try ok.get(), "fine")
        XCTAssertThrowsError(try err.get()) { error in
            XCTAssertEqual(error as? Failure, Failure(code: 7))
        }
        XCTAssertEqual(ok.result, .success("fine"))
        XCTAssertEqual(err.result, .failure(Failure(code: 7)))
    }

    // MARK: UndraUnit and Void helpers

    func testUnitOccupiesNoBytes() throws {
        XCTAssertEqual(UndraUnit().undraEncoded(), [])
        XCTAssertEqual(try UndraUnit.undraDecoded(from: []), UndraUnit())

        var writer = UndraWriter()
        undraEncodeVoid(&writer)
        XCTAssertEqual(writer.count, 0)

        let bytes: [UInt8] = [1, 2]
        var reader = UndraReader(bytes)
        try undraDecodeVoid(&reader)
        XCTAssertEqual(reader.position, 0)
    }
}
