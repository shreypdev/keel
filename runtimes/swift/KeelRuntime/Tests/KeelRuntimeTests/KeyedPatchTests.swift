import XCTest
import KeelRuntime

/// Keyed list patches (docs/SPEC.md section 3.8): wire format and application.
final class KeyedPatchTests: XCTestCase {
    // MARK: Wire format

    func testEveryOpRoundTripsWithItsSpecTag() {
        assertCodec(PatchOp<Int32>.insert(index: 3, item: 5), hex: "000300000005000000")
        assertCodec(PatchOp<Int32>.remove(index: 1), hex: "0101000000")
        assertCodec(PatchOp<Int32>.update(index: 2, item: -1), hex: "0202000000ffffffff")
        assertCodec(PatchOp<Int32>.move(from: 0, to: 1), hex: "030000000001000000")
        assertCodec(PatchOp<Int32>.clear, hex: "04")
    }

    func testContractVectorPatch() throws {
        let ops: [PatchOp<Int32>] = [
            .insert(index: 0, item: 5),
            .remove(index: 1),
            .move(from: 0, to: 1),
            .clear,
        ]
        let hex = "04000000000000000005000000010100000003000000000100000004"
        var writer = KeelWriter()
        encodePatch(ops, into: &writer)
        XCTAssertEqual(bytesToHex(writer.finish()), hex)

        var reader = KeelReader(hexToBytes(hex))
        let decoded: [PatchOp<Int32>] = try decodePatch(&reader)
        XCTAssertNoThrow(try reader.finish())
        XCTAssertEqual(decoded, ops)
    }

    func testEmptyPatch() throws {
        var writer = KeelWriter()
        let none: [PatchOp<String>] = []
        encodePatch(none, into: &writer)
        XCTAssertEqual(writer.finish(), [0, 0, 0, 0])
        var reader = KeelReader([0, 0, 0, 0])
        let decoded: [PatchOp<String>] = try decodePatch(&reader)
        XCTAssertTrue(decoded.isEmpty)
    }

    func testPatchOfUnicodeStrings() throws {
        let ops: [PatchOp<String>] = [
            .insert(index: 0, item: "h\u{E9}llo"),
            .update(index: 0, item: "\u{1F30A}"),
            .insert(index: 1, item: ""),
        ]
        var writer = KeelWriter()
        encodePatch(ops, into: &writer)
        var reader = KeelReader(writer.finish())
        let decoded: [PatchOp<String>] = try decodePatch(&reader)
        XCTAssertEqual(decoded, ops)
    }

    func testMalformedPatch() {
        expectWireError(.invalidTag(tag: 5, at: 4, type: "PatchOp")) {
            var reader = KeelReader(hexToBytes("0100000005"))
            let _: [PatchOp<Int32>] = try decodePatch(&reader)
        }
        // A count larger than the bytes that remain is rejected before anything is allocated.
        expectWireError(.lengthTooLarge(len: 1_000_000, at: 0)) {
            var reader = KeelReader(hexToBytes("40420f00"))
            let _: [PatchOp<Int32>] = try decodePatch(&reader)
        }
        // An op cut short.
        expectWireError(.unexpectedEOF(needed: 4, at: 5)) {
            var reader = KeelReader(hexToBytes("010000000100"))
            let _: [PatchOp<Int32>] = try decodePatch(&reader)
        }
        // An op whose item is malformed.
        expectWireError(.unexpectedEOF(needed: 4, at: 9)) {
            var reader = KeelReader(hexToBytes("0100000000000000000500"))
            let _: [PatchOp<Int32>] = try decodePatch(&reader)
        }
    }

    // MARK: Applying

    private func apply(_ ops: [PatchOp<Int>], to list: [Int]) throws -> [Int] {
        var copy = list
        try applyPatch(ops, to: &copy)
        return copy
    }

    func testInsert() throws {
        XCTAssertEqual(try apply([.insert(index: 0, item: 9)], to: []), [9])
        XCTAssertEqual(try apply([.insert(index: 0, item: 9)], to: [1, 2]), [9, 1, 2])
        XCTAssertEqual(try apply([.insert(index: 1, item: 9)], to: [1, 2]), [1, 9, 2])
        XCTAssertEqual(try apply([.insert(index: 2, item: 9)], to: [1, 2]), [1, 2, 9])
    }

    func testRemove() throws {
        XCTAssertEqual(try apply([.remove(index: 0)], to: [1, 2, 3]), [2, 3])
        XCTAssertEqual(try apply([.remove(index: 2)], to: [1, 2, 3]), [1, 2])
        XCTAssertEqual(try apply([.remove(index: 0)], to: [1]), [])
    }

    func testUpdate() throws {
        XCTAssertEqual(try apply([.update(index: 1, item: 7)], to: [1, 2, 3]), [1, 7, 3])
    }

    func testMoveEndsUpAtTheTargetIndex() throws {
        XCTAssertEqual(try apply([.move(from: 0, to: 2)], to: [1, 2, 3]), [2, 3, 1])
        XCTAssertEqual(try apply([.move(from: 2, to: 0)], to: [1, 2, 3]), [3, 1, 2])
        XCTAssertEqual(try apply([.move(from: 0, to: 1)], to: [1, 2, 3]), [2, 1, 3])
        XCTAssertEqual(try apply([.move(from: 1, to: 1)], to: [1, 2, 3]), [1, 2, 3])
        XCTAssertEqual(try apply([.move(from: 0, to: 0)], to: [5]), [5])
    }

    func testClear() throws {
        XCTAssertEqual(try apply([.clear], to: [1, 2, 3]), [])
        XCTAssertEqual(try apply([.clear], to: []), [])
    }

    func testOpsApplySequentiallyAgainstTheUpdatedList() throws {
        // Each index refers to the list after the previous op.
        let ops: [PatchOp<Int>] = [
            .insert(index: 0, item: 10), // [10, 1, 2, 3]
            .remove(index: 3), // [10, 1, 2]
            .update(index: 1, item: 11), // [10, 11, 2]
            .move(from: 2, to: 0), // [2, 10, 11]
            .insert(index: 3, item: 12), // [2, 10, 11, 12]
        ]
        XCTAssertEqual(try apply(ops, to: [1, 2, 3]), [2, 10, 11, 12])
        // An index valid only because an earlier op grew the list.
        XCTAssertEqual(try apply([.insert(index: 0, item: 1), .update(index: 0, item: 2)], to: []), [2])
        // Clear then rebuild.
        XCTAssertEqual(try apply([.clear, .insert(index: 0, item: 4), .insert(index: 1, item: 5)], to: [1, 2, 3]), [4, 5])
        XCTAssertEqual(try apply([], to: [1, 2]), [1, 2])
    }

    func testMatchesTheContractVectorSemantics() throws {
        // insert 5 at 0, remove index 1, move 0 to 1, clear: applied to [1, 2, 3].
        let ops: [PatchOp<Int>] = [.insert(index: 0, item: 5), .remove(index: 1), .move(from: 0, to: 1), .clear]
        XCTAssertEqual(try apply(ops, to: [1, 2, 3]), [])
        // Without the final clear: [1,2,3] -> [5,1,2,3] -> [5,2,3] -> (move 0 to 1) [2,5,3].
        XCTAssertEqual(try apply(Array(ops.dropLast()), to: [1, 2, 3]), [2, 5, 3])
    }

    // MARK: Bounds checking

    private func expectOutOfBounds(
        _ ops: [PatchOp<Int>],
        on list: [Int],
        opIndex: Int,
        index: UInt32,
        count: Int,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        var copy = list
        do {
            try applyPatch(ops, to: &copy)
            XCTFail("expected an out-of-bounds error", file: file, line: line)
        } catch let error as PatchError {
            XCTAssertEqual(error, .indexOutOfBounds(opIndex: opIndex, index: index, count: count), file: file, line: line)
        } catch {
            XCTFail("unexpected error \(error)", file: file, line: line)
        }
        XCTAssertEqual(copy, list, "a failed patch must leave the list untouched", file: file, line: line)
    }

    func testEveryOpIsBoundsChecked() {
        expectOutOfBounds([.insert(index: 3, item: 0)], on: [1, 2], opIndex: 0, index: 3, count: 2)
        expectOutOfBounds([.insert(index: 1, item: 0)], on: [], opIndex: 0, index: 1, count: 0)
        expectOutOfBounds([.remove(index: 2)], on: [1, 2], opIndex: 0, index: 2, count: 2)
        expectOutOfBounds([.remove(index: 0)], on: [], opIndex: 0, index: 0, count: 0)
        expectOutOfBounds([.update(index: 2, item: 0)], on: [1, 2], opIndex: 0, index: 2, count: 2)
        expectOutOfBounds([.move(from: 2, to: 0)], on: [1, 2], opIndex: 0, index: 2, count: 2)
        expectOutOfBounds([.move(from: 0, to: 2)], on: [1, 2], opIndex: 0, index: 2, count: 2)
        expectOutOfBounds([.move(from: 0, to: 0)], on: [], opIndex: 0, index: 0, count: 0)
    }

    func testHugeIndicesAreRejectedNotTrapped() {
        expectOutOfBounds([.insert(index: UInt32.max, item: 0)], on: [1], opIndex: 0, index: UInt32.max, count: 1)
        expectOutOfBounds([.remove(index: UInt32.max)], on: [1], opIndex: 0, index: UInt32.max, count: 1)
        expectOutOfBounds([.update(index: UInt32.max, item: 0)], on: [1], opIndex: 0, index: UInt32.max, count: 1)
        expectOutOfBounds([.move(from: UInt32.max, to: 0)], on: [1], opIndex: 0, index: UInt32.max, count: 1)
    }

    func testAFailingLaterOpLeavesTheListUntouched() {
        // The first two ops are fine; the third indexes past the end. Nothing may be applied.
        let ops: [PatchOp<Int>] = [.insert(index: 0, item: 9), .remove(index: 1), .update(index: 5, item: 0)]
        expectOutOfBounds(ops, on: [1, 2], opIndex: 2, index: 5, count: 2)
    }

    func testBoundsTrackTheSimulatedLength() {
        // After the clear the list is empty, so removing index 0 is out of range.
        expectOutOfBounds([.clear, .remove(index: 0)], on: [1, 2, 3], opIndex: 1, index: 0, count: 0)
        // After two removes from a list of two, an update at 0 is out of range.
        expectOutOfBounds([.remove(index: 0), .remove(index: 0), .update(index: 0, item: 1)], on: [1, 2], opIndex: 2, index: 0, count: 0)
    }

    func testPatchErrorDescription() {
        let error = PatchError.indexOutOfBounds(opIndex: 2, index: 9, count: 3)
        XCTAssertEqual(error.description, "patch op 2 refers to index 9, out of range for a list of 3 element(s)")
    }

    func testApplyingADecodedPatchEndToEnd() throws {
        var writer = KeelWriter()
        let sent: [PatchOp<String>] = [.insert(index: 0, item: "b"), .insert(index: 0, item: "a"), .update(index: 1, item: "B")]
        encodePatch(sent, into: &writer)
        var reader = KeelReader(writer.finish())
        let received: [PatchOp<String>] = try decodePatch(&reader)
        var list = ["z"]
        try applyPatch(received, to: &list)
        XCTAssertEqual(list, ["a", "B", "z"])
    }
}
