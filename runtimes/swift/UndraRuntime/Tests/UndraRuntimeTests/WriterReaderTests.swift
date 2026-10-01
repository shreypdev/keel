import XCTest
import UndraRuntime

/// The byte-level behaviour of `UndraWriter` and `UndraReader`.
final class WriterReaderTests: XCTestCase {
    // MARK: Writer

    func testWriterEmitsLittleEndianIntegers() {
        var w = UndraWriter()
        w.writeU8(0x01)
        w.writeU16(0x0203)
        w.writeU32(0x0405_0607)
        w.writeU64(0x0809_0A0B_0C0D_0E0F)
        let expected: [UInt8] = [
            0x01,
            0x03, 0x02,
            0x07, 0x06, 0x05, 0x04,
            0x0F, 0x0E, 0x0D, 0x0C, 0x0B, 0x0A, 0x09, 0x08,
        ]
        XCTAssertEqual(w.finish(), expected)
    }

    func testWriterEmitsTwosComplementSignedIntegers() {
        var w = UndraWriter()
        w.writeI8(-1)
        w.writeI16(-2)
        w.writeI32(-2)
        w.writeI64(-2)
        // i8 -1 (ff) | i16 -2 (feff) | i32 -2 (feffffff) | i64 -2 (feffffffffffffff)
        XCTAssertEqual(bytesToHex(w.finish()), "fffefffefffffffeffffffffffffff")
    }

    func testWriterEmitsBigEndianU64() {
        var w = UndraWriter()
        w.writeU64BigEndian(0x0102_0304_0506_0708)
        XCTAssertEqual(w.finish(), [1, 2, 3, 4, 5, 6, 7, 8])
    }

    func testWriterEmitsFloatBits() {
        var w = UndraWriter()
        w.writeF32(3.14)
        w.writeF64(2.718281828459045)
        XCTAssertEqual(bytesToHex(w.finish()), "c3f548406957148b0abf0540")
    }

    func testWriterBool() {
        var w = UndraWriter()
        w.writeBool(true)
        w.writeBool(false)
        XCTAssertEqual(w.finish(), [1, 0])
    }

    func testWriterStringAndBytesArePrefixedWithU32Length() {
        var w = UndraWriter()
        w.writeString("hé")
        w.writeBytes([9, 8, 7])
        w.writeBytes(slice: ArraySlice([6, 5]))
        XCTAssertEqual(
            bytesToHex(w.finish()),
            "0300000068c3a903000000090807020000000605"
        )
    }

    func testWriterRawAppendsWithoutPrefix() {
        var w = UndraWriter()
        w.writeRaw([1, 2])
        w.writeRaw(slice: ArraySlice([3, 4]))
        XCTAssertEqual(w.finish(), [1, 2, 3, 4])
    }

    func testWriterLenWritesU32() {
        var w = UndraWriter()
        w.writeLen(0)
        w.writeLen(258)
        XCTAssertEqual(bytesToHex(w.finish()), "0000000002010000")
    }

    func testWriterCountCapacityAndSlice() {
        var w = UndraWriter(capacity: 64)
        XCTAssertEqual(w.count, 0)
        w.reserveCapacity(128)
        w.reserveCapacity(-5)
        w.writeU32(1)
        XCTAssertEqual(w.count, 4)
        XCTAssertEqual(Array(w.finishSlice()), w.finish())
        let negative = UndraWriter(capacity: -1)
        XCTAssertEqual(negative.count, 0)
    }

    // MARK: Reader

    func testReaderReadsWhatTheWriterWrote() throws {
        var w = UndraWriter()
        w.writeU8(200)
        w.writeU16(60000)
        w.writeU32(4_000_000_000)
        w.writeU64(UInt64.max - 1)
        w.writeI8(-100)
        w.writeI16(-30000)
        w.writeI32(-2_000_000_000)
        w.writeI64(Int64.min)
        w.writeF32(-1.5)
        w.writeF64(1e300)
        w.writeBool(true)
        w.writeString("héllo 🌊")
        w.writeBytes([0, 255])

        var r = UndraReader(w.finish())
        XCTAssertEqual(try r.readU8(), 200)
        XCTAssertEqual(try r.readU16(), 60000)
        XCTAssertEqual(try r.readU32(), 4_000_000_000)
        XCTAssertEqual(try r.readU64(), UInt64.max - 1)
        XCTAssertEqual(try r.readI8(), -100)
        XCTAssertEqual(try r.readI16(), -30000)
        XCTAssertEqual(try r.readI32(), -2_000_000_000)
        XCTAssertEqual(try r.readI64(), Int64.min)
        XCTAssertEqual(try r.readF32(), -1.5)
        XCTAssertEqual(try r.readF64(), 1e300)
        XCTAssertEqual(try r.readBool(), true)
        XCTAssertEqual(utf8Bytes(try r.readString()), utf8Bytes("héllo 🌊"))
        XCTAssertEqual(try r.readBytes(), [0, 255])
        XCTAssertTrue(r.isAtEnd)
        XCTAssertEqual(r.remaining, 0)
        XCTAssertNoThrow(try r.finish())
    }

    func testReaderBigEndianU64() throws {
        let bytes: [UInt8] = [1, 2, 3, 4, 5, 6, 7, 8]
        var r = UndraReader(bytes)
        XCTAssertEqual(try r.readU64BigEndian(), 0x0102_0304_0506_0708)
    }

    func testReaderPositionAndRemaining() throws {
        let bytes: [UInt8] = [1, 2, 3, 4, 5, 6]
        var r = UndraReader(bytes)
        XCTAssertEqual(r.position, 0)
        XCTAssertEqual(r.remaining, 6)
        XCTAssertFalse(r.isAtEnd)
        _ = try r.readU16()
        XCTAssertEqual(r.position, 2)
        XCTAssertEqual(r.remaining, 4)
        try r.skip(4)
        XCTAssertTrue(r.isAtEnd)
    }

    func testReaderOverSliceWithNonZeroStartIndex() throws {
        let backing: [UInt8] = [0xEE, 0xEE, 0x01, 0x00, 0x00, 0x00, 0x02, 0xEE]
        let slice = backing[2 ..< 7]
        XCTAssertEqual(slice.startIndex, 2)
        var r = UndraReader(slice: slice)
        XCTAssertEqual(r.position, 0)
        XCTAssertEqual(r.remaining, 5)
        XCTAssertEqual(try r.readU32(), 1)
        XCTAssertEqual(r.position, 4)
        XCTAssertEqual(try r.readU8(), 2)
        XCTAssertNoThrow(try r.finish())
        // Errors report offsets relative to the slice start, not to the backing array.
        expectWireError(.unexpectedEOF(needed: 1, at: 5)) {
            _ = try r.readU8()
        }
    }

    func testReaderCopyingFromUnsafeBufferPointerOwnsItsBytes() throws {
        let source: [UInt8] = [1, 0, 0, 0, 0xAA]
        var reader = source.withUnsafeBufferPointer { buffer in
            return UndraReader(copying: buffer)
        }
        // The pointer is gone; the reader must still work because it copied.
        XCTAssertEqual(try reader.readU32(), 1)
        XCTAssertEqual(try reader.readU8(), 0xAA)
        XCTAssertNoThrow(try reader.finish())
    }

    func testReaderCopyingFromPointerAndCount() throws {
        let source: [UInt8] = [7, 0, 0, 0]
        var reader = source.withUnsafeBufferPointer { buffer in
            return UndraReader(copying: buffer.baseAddress, count: buffer.count)
        }
        XCTAssertEqual(try reader.readU32(), 7)

        let empty = UndraReader(copying: nil, count: 0)
        XCTAssertTrue(empty.isAtEnd)
        let negative = UndraReader(copying: nil, count: -3)
        XCTAssertTrue(negative.isAtEnd)
    }

    func testReadBytesSliceAndRawShareStorage() throws {
        let bytes: [UInt8] = [3, 0, 0, 0, 10, 20, 30, 40, 50]
        var r = UndraReader(bytes)
        let slice = try r.readBytesSlice()
        XCTAssertEqual(Array(slice), [10, 20, 30])
        let raw = try r.readRaw(2)
        XCTAssertEqual(raw, [40, 50])
        XCTAssertTrue(r.isAtEnd)

        var r2 = UndraReader(bytes)
        _ = try r2.readU32()
        let tail = try r2.readSlice(5)
        XCTAssertEqual(Array(tail), [10, 20, 30, 40, 50])
    }

    func testReadRemainingConsumesEverything() throws {
        let bytes: [UInt8] = [1, 2, 3, 4]
        var r = UndraReader(bytes)
        _ = try r.readU8()
        let rest = r.readRemaining()
        XCTAssertEqual(Array(rest), [2, 3, 4])
        XCTAssertTrue(r.isAtEnd)
        XCTAssertEqual(Array(r.readRemaining()), [])
    }

    func testSubReaderIsRestrictedAndKeepsOffsets() throws {
        let bytes: [UInt8] = [0, 0, 1, 2, 3, 9, 9]
        var r = UndraReader(bytes)
        try r.skip(2)
        var sub = try r.readSubReader(length: 3)
        XCTAssertEqual(r.position, 5)
        XCTAssertEqual(sub.position, 2)
        XCTAssertEqual(sub.remaining, 3)
        XCTAssertEqual(try sub.readU8(), 1)
        XCTAssertEqual(try sub.readU16(), 0x0302)
        // The sub-reader cannot see the two bytes after its window.
        expectWireError(.unexpectedEOF(needed: 1, at: 5)) {
            _ = try sub.readU8()
        }
        XCTAssertNoThrow(try sub.finish())
        XCTAssertEqual(try r.readU8(), 9)
    }

    func testSubReaderLongerThanRemainingThrows() {
        let bytes: [UInt8] = [1, 2, 3]
        var r = UndraReader(bytes)
        expectWireError(.unexpectedEOF(needed: 4, at: 0)) {
            _ = try r.readSubReader(length: 4)
        }
        XCTAssertEqual(r.position, 0, "a failed read must not move the cursor")
    }

    func testFailedReadDoesNotAdvanceTheCursor() throws {
        let bytes: [UInt8] = [1, 2, 3]
        var r = UndraReader(bytes)
        _ = try r.readU8()
        expectWireError(.unexpectedEOF(needed: 4, at: 1)) {
            _ = try r.readU32()
        }
        XCTAssertEqual(r.position, 1)
        XCTAssertEqual(try r.readU16(), 0x0302)
    }

    func testEmptyReader() {
        let empty: [UInt8] = []
        var r = UndraReader(empty)
        XCTAssertTrue(r.isAtEnd)
        XCTAssertNoThrow(try r.finish())
        expectWireError(.unexpectedEOF(needed: 1, at: 0)) {
            _ = try r.readU8()
        }
    }

    func testFinishReportsTrailingBytes() throws {
        let bytes: [UInt8] = [1, 2, 3]
        var r = UndraReader(bytes)
        _ = try r.readU8()
        expectWireError(.trailingBytes(count: 2)) {
            try r.finish()
        }
    }
}
