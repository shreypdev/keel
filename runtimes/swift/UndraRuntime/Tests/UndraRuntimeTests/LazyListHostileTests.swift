import Foundation
import XCTest
@testable import UndraRuntime

/// A core that answers pages wrongly: every case is a typed error reported through `onError`, nothing is cached from the bad
/// reply, nothing traps, and the list keeps working (ADR-043, constitution R6).
@available(iOS 17, macOS 14, *)
@MainActor
final class LazyListHostileTests: XCTestCase {
    /// Reads page 0 of a list of 1,000 rows against `reply`, and returns the errors reported for it.
    private func reading(
        _ reply: @escaping @Sendable (_ offset: UInt32, _ limit: UInt32) -> [UInt8],
        file: StaticString = #filePath,
        line: UInt = #line
    ) throws -> (rig: LazyRig, errors: [UndraUnhandledError]) {
        let rig = try LazyRig(rows: 1000)
        rig.server.respond(reply)
        rig.list.prefetch(0 ..< 1)
        rig.turns.runUntilQuiet()
        XCTAssertEqual(rig.list.testEngine.cachedPages, [], "nothing is cached from a bad reply", file: file, line: line)
        XCTAssertNil(rig.list[0], file: file, line: line)
        XCTAssertEqual(rig.list.count, 1000, file: file, line: line)
        return (rig, rig.errors.all)
    }

    func testMoreItemsThanTheLimitIsRefused() throws {
        let (_, errors) = try reading { _, _ in
            lazyPageBody(version: 1, total: 1000, count: 51, rows: (0 ..< 51).map { Int32($0) })
        }
        XCTAssertEqual(errors.count, 1)
        XCTAssertTrue(errors[0].description.contains("UndraLazyList.page(0)"), errors[0].description)
        XCTAssertTrue(errors[0].description.contains("51 items, more than the 50"), errors[0].description)
    }

    func testAnEnormousCountIsRefusedWithoutAllocatingForIt() throws {
        let (_, errors) = try reading { _, _ in
            lazyPageBody(version: 1, total: 1000, count: UInt32.max, rows: [1, 2, 3])
        }
        XCTAssertEqual(errors.count, 1)
        XCTAssertTrue(errors[0].description.contains("more than the 50"))
    }

    func testTruncatedItemsAreRefused() throws {
        let (_, errors) = try reading { _, _ in
            lazyPageBody(version: 1, total: 1000, count: 50, rows: (0 ..< 10).map { Int32($0) })
        }
        XCTAssertEqual(errors.count, 1)
        XCTAssertTrue(errors[0].description.contains("does not decode"), errors[0].description)
    }

    func testAnItemCutInTheMiddleIsRefused() throws {
        let (_, errors) = try reading { _, _ in
            var body = lazyPageBody(version: 1, total: 1000, count: 50, rows: (0 ..< 50).map { Int32($0) })
            body.removeLast(2)
            return body
        }
        XCTAssertEqual(errors.count, 1)
        XCTAssertTrue(errors[0].description.contains("does not decode"))
    }

    func testTrailingBytesAreRefused() throws {
        let (_, errors) = try reading { _, _ in
            lazyPageBody(version: 1, total: 1000, count: 50, rows: (0 ..< 50).map { Int32($0) }, trailing: [9, 9])
        }
        XCTAssertEqual(errors.count, 1)
        XCTAssertTrue(errors[0].description.contains("trailing"), errors[0].description)
    }

    func testAShortPageThatTheTotalDoesNotExplainIsRefused() throws {
        // total 1000 at offset 0 with limit 50 means exactly 50 items; 20 is a lie (and so is 0).
        for claimed in [20, 0] {
            let (_, errors) = try reading { _, _ in
                lazyPageBody(version: 1, total: 1000, count: UInt32(claimed), rows: (0 ..< claimed).map { Int32($0) })
            }
            XCTAssertEqual(errors.count, 1, "\(claimed) items")
            XCTAssertTrue(errors[0].description.contains("\(claimed) items, but its total says 50"), errors[0].description)
        }
    }

    func testATotalSmallerThanThePageIsRefused() throws {
        // A total of 10 allows 10 items at offset 0, not 50.
        let (_, errors) = try reading { _, _ in
            lazyPageBody(version: 1, total: 10, count: 50, rows: (0 ..< 50).map { Int32($0) })
        }
        XCTAssertEqual(errors.count, 1)
        XCTAssertTrue(errors[0].description.contains("50 items, but its total says 10"), errors[0].description)
    }

    func testATotalThatDisagreesWithTheListAtTheSameVersionIsRefused() throws {
        let (_, errors) = try reading { _, _ in
            lazyPageBody(version: 1, total: 999, count: 50, rows: (0 ..< 50).map { Int32($0) })
        }
        XCTAssertEqual(errors.count, 1)
        XCTAssertTrue(errors[0].description.contains("total 999"), errors[0].description)
        XCTAssertTrue(errors[0].description.contains("1000 rows"), errors[0].description)
    }

    func testAnEmptyOrHeaderlessReplyIsRefused() throws {
        for body in [[UInt8](), [1, 2, 3], [UInt8](repeating: 0, count: 15)] {
            let (_, errors) = try reading { _, _ in body }
            XCTAssertEqual(errors.count, 1, "\(body.count) bytes")
            XCTAssertTrue(errors[0].description.contains("does not decode"))
        }
    }

    func testAnItemThatIsNotAValidItemIsRefused() throws {
        // Strings are length-prefixed: a length far past the end of the page.
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let list = UndraLazyList<String>(core: core)
        let page = lazyStringPage(version: 1, total: 3, items: [0xFF, 0xFF, 0xFF, 0x7F])
        transport.onCallSync = { call in Wire.Reply(callId: call.callId, status: .ok, body: ArraySlice(page)).encode() }
        var reader = UndraReader(UndraLazyValue(handle: UndraHandle(index: 2, generation: 1), len: 3, version: 1).undraEncoded())
        try list.applyFull(&reader)
        list.prefetch(0 ..< 1)
        list.testEngine.flush()
        XCTAssertNil(list[0])
        XCTAssertEqual(list.testEngine.cachedPages, [])
    }

    func testABadReplyDoesNotStopTheListFromWorkingAfterwards() throws {
        let (rig, errors) = try reading { _, _ in
            lazyPageBody(version: 1, total: 1000, count: 51, rows: (0 ..< 51).map { Int32($0) })
        }
        XCTAssertEqual(errors.count, 1)
        rig.server.respond(nil)
        XCTAssertNil(rig.list[0])
        rig.turns.runUntilQuiet()
        XCTAssertEqual(rig.list[0], 0, "the next read asks again and the honest reply is used")
        XCTAssertEqual(rig.errors.count, 1)
    }

    func testAPageReadAtANewerVersionWithABadTotalDoesNotRaiseTheList() throws {
        // Newer version, but the count it carries does not match its own total: refused before anything is adopted.
        let (rig, errors) = try reading { _, _ in
            lazyPageBody(version: 9, total: 5000, count: 7, rows: (0 ..< 7).map { Int32($0) })
        }
        XCTAssertEqual(errors.count, 1)
        XCTAssertEqual(rig.list.testEngine.version, 1)
        XCTAssertEqual(rig.list.count, 1000)
    }

    func testRandomBytesAsAPageNeverTrap() throws {
        var generator = SplitMix64(seed: 0x4C41_5A59)
        for _ in 0 ..< 200 {
            let length = Int(generator.next() % 120)
            var bytes: [UInt8] = []
            for _ in 0 ..< length {
                bytes.append(UInt8(truncatingIfNeeded: generator.next()))
            }
            let body = bytes
            let rig = try LazyRig(rows: 300)
            rig.server.respond { _, _ in body }
            rig.list.prefetch(0 ..< 300)
            rig.turns.runUntilQuiet()
            XCTAssertEqual(rig.list.count, 300)
        }
    }

    func testRandomValuesOfTheSignalNeverTrap() throws {
        var generator = SplitMix64(seed: 0x51_4E41_4C)
        let rig = try LazyRig(rows: 300)
        for _ in 0 ..< 300 {
            let length = Int(generator.next() % 30)
            var bytes: [UInt8] = []
            for _ in 0 ..< length {
                bytes.append(UInt8(truncatingIfNeeded: generator.next()))
            }
            var full = UndraReader(bytes)
            _ = try? rig.list.applyFull(&full)
            var invalidated = UndraReader(bytes)
            _ = try? rig.list.applyInvalidated(&invalidated)
            _ = rig.list[Int(truncatingIfNeeded: generator.next())]
            rig.list.prefetch(Int(truncatingIfNeeded: generator.next() % 1000) ..< 2000)
            rig.turns.runUntilQuiet()
        }
    }

    func testAnEnormousLengthIsHarmless() throws {
        let rig = try LazyRig(rows: 10)
        try rig.applyInvalidated(UndraLazyInvalidated(len: UInt32.max, version: 2).undraEncoded())
        XCTAssertEqual(rig.list.count, Int(UInt32.max))
        XCTAssertNil(rig.list[Int(UInt32.max) - 1])
        rig.turns.runUntilQuiet()
        XCTAssertEqual(rig.list.testEngine.cachedPages.count <= 3, true)
        rig.list.prefetch(0 ..< Int.max)
        rig.turns.runUntilQuiet()
        XCTAssertLessThanOrEqual(rig.list.testEngine.cachedPages.count, 24)
    }
}

/// A page of strings: the header, then `items` as written (for an item that does not decode).
private func lazyStringPage(version: UInt64, total: UInt32, items: [UInt8]) -> [UInt8] {
    var writer = UndraWriter()
    UndraLazyPageHeader(version: version, total: total, count: 3).undraEncode(&writer)
    writer.writeRaw(items)
    return writer.finish()
}
