import Combine
import Foundation
import Observation
import XCTest
@testable import UndraRuntime

/// What SwiftUI sees of a lazy list: `@Observable` dependencies on `count` and `list[index]`, the `ObservableObject` twin's
/// `objectWillChange`, the collection conformance, and a store feeding a list the way generated code does.
@available(iOS 17, macOS 14, *)
@MainActor
final class LazyListObservationTests: XCTestCase {
    // MARK: @Observable

    func testReadingCountRegistersADependencyOnTheLength() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let list = UndraLazyList<Int32>(core: core)
        let server = LazyServer(rows: 40)
        let fired = LazyCounter()
        withObservationTracking {
            _ = list.count
        } onChange: {
            fired.bump()
        }
        var reader = UndraReader(server.value())
        try list.applyFull(&reader)
        XCTAssertEqual(fired.count, 1)
        XCTAssertEqual(list.count, 40)
    }

    func testAnInvalidationThatChangesTheLengthNotifiesAReaderOfCount() throws {
        let rig = try LazyRig(rows: 100)
        let fired = LazyCounter()
        withObservationTracking {
            _ = rig.list.count
        } onChange: {
            fired.bump()
        }
        try rig.change { $0.append(1) }
        XCTAssertEqual(fired.count, 1)
    }

    func testReadingARowRegistersADependencyOnItsPageArriving() throws {
        let rig = try LazyRig(rows: 100)
        let fired = LazyCounter()
        var seen: Int32?
        withObservationTracking {
            seen = rig.list[3]
        } onChange: {
            fired.bump()
        }
        XCTAssertNil(seen)
        XCTAssertEqual(fired.count, 0, "asking changes nothing")
        rig.turns.run()
        XCTAssertEqual(fired.count, 1, "the page arrived")
        XCTAssertEqual(rig.list[3], 3)
    }

    func testAReaderOfARowIsToldWhenItsPageIsReplaced() throws {
        let rig = try LazyRig(rows: 100)
        rig.read(3)
        let fired = LazyCounter()
        withObservationTracking {
            _ = rig.list[3]
        } onChange: {
            fired.bump()
        }
        try rig.change { $0[3] = 33 }
        XCTAssertEqual(fired.count, 0, "the new length is the same and the stale row is still shown")
        rig.turns.runUntilQuiet()
        XCTAssertEqual(fired.count, 1)
        XCTAssertEqual(rig.list[3], 33)
    }

    func testReadingAnIndexBeyondTheEndStillDependsOnTheLength() throws {
        let rig = try LazyRig(rows: 100)
        let fired = LazyCounter()
        withObservationTracking {
            _ = rig.list[150]
        } onChange: {
            fired.bump()
        }
        try rig.change { $0.append(contentsOf: [Int32](repeating: 0, count: 100)) }
        XCTAssertEqual(fired.count, 1, "the row exists now")
    }

    func testArrivalOfSeveralPagesInOneBatchIsOneChange() throws {
        let rig = try LazyRig(rows: 500)
        let changes = LazyCounter()
        _ = rig.list[60]
        func track() {
            withObservationTracking {
                _ = rig.list[60]
            } onChange: {
                changes.bump()
            }
        }
        track()
        rig.turns.run()
        XCTAssertEqual(rig.server.pages(), [0, 1, 2])
        XCTAssertEqual(changes.count, 1)
    }

    // MARK: ObservableObject twin

    func testTheTwinPagesLikeTheObservableList() throws {
        let transport = FakeTransport()
        let server = LazyServer(rows: 500)
        server.install(on: transport)
        let core = try makeCore(transport)
        let turns = LazyTurns()
        let list = UndraLazyListObject<Int32>(core: core, schedule: { turns.schedule($0) })
        var reader = UndraReader(server.value())
        try list.applyFull(&reader)
        XCTAssertEqual(list.count, 500)
        XCTAssertNil(list[260])
        XCTAssertNil(list[-1])
        XCTAssertNil(list[500])
        XCTAssertEqual(turns.scheduled, 1)
        turns.run()
        XCTAssertEqual(server.pages(), [4, 5, 6])
        XCTAssertEqual(list[260], 260)
        XCTAssertEqual(list.pageSize, 50)
        XCTAssertEqual(list.maxCachedPages, 24)
        list.prefetch(0 ..< 60)
        turns.run()
        XCTAssertEqual(server.pages(), [4, 5, 6, 0, 1])
        XCTAssertEqual(list.startIndex, 0)
        XCTAssertEqual(list.endIndex, 500)
        XCTAssertEqual(Array(list.prefix(3)), [0, 1, 2])
    }

    func testTheTwinSendsObjectWillChangeOncePerChange() throws {
        let transport = FakeTransport()
        let server = LazyServer(rows: 500)
        server.install(on: transport)
        let core = try makeCore(transport)
        let turns = LazyTurns()
        let list = UndraLazyListObject<Int32>(core: core, schedule: { turns.schedule($0) })
        let sends = LazyCounter()
        let subscription = list.objectWillChange.sink { _ in
            sends.bump()
        }
        defer {
            subscription.cancel()
        }
        var first = UndraReader(server.value())
        try list.applyFull(&first)
        XCTAssertEqual(sends.count, 1, "the length")
        _ = list[60]
        XCTAssertEqual(sends.count, 1, "asking changes nothing")
        turns.run()
        XCTAssertEqual(server.pages(), [0, 1, 2])
        XCTAssertEqual(sends.count, 2, "three pages arrived in one batch: one change")
        server.mutate { $0[60] = -1; $0.append(7) }
        var invalidated = UndraReader(server.invalidated())
        try list.applyInvalidated(&invalidated)
        XCTAssertEqual(sends.count, 3, "a new length: one change")
        turns.runUntilQuiet()
        XCTAssertEqual(sends.count, 4, "the window arrived again: one change")
        XCTAssertEqual(list[60], -1)
        XCTAssertEqual(list.count, 501)
    }

    func testTheTwinPublishesItsCount() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let list = UndraLazyListObject<Int32>(core: core)
        var counts: [Int] = []
        let subscription = list.$count.sink { counts.append($0) }
        defer {
            subscription.cancel()
        }
        var reader = UndraReader(LazyServer(rows: 12).value())
        try list.applyFull(&reader)
        XCTAssertEqual(counts, [0, 12])
    }

    // MARK: A store, as generated code writes it

    func testAStoreFeedsItsListFromTheMirror() throws {
        let transport = FakeTransport()
        let server = LazyServer(rows: 300)
        server.install(on: transport)
        let core = try makeCore(transport)
        let turns = LazyTurns()
        let store = LazyLibraryStore(core: core, handle: UndraHandle(index: 1, generation: 1), schedule: { turns.schedule($0) })
        XCTAssertEqual(store.books.count, 0)
        transport.deliverChangeSet(Wire.ChangeSet(txnId: 1, entries: [
            Wire.ChangeEntry(handle: store.handle, signalId: 3, op: .fullValue, value: ArraySlice(server.value())),
        ]))
        core.mirror.flush()
        XCTAssertEqual(store.books.count, 300)
        XCTAssertNil(store.books[120])
        turns.runUntilQuiet()
        XCTAssertEqual(store.books[120], 120)
        server.mutate { $0[120] = 1200; $0.removeLast(100) }
        transport.deliverChangeSet(Wire.ChangeSet(txnId: 2, entries: [
            Wire.ChangeEntry(handle: store.handle, signalId: 3, op: .lazyListInvalidated, value: ArraySlice(server.invalidated())),
            Wire.ChangeEntry(handle: store.handle, signalId: 3, op: .keyedPatch, value: [0, 0, 0, 0]),
        ]))
        core.mirror.flush()
        XCTAssertEqual(store.books.count, 200)
        XCTAssertEqual(store.books[120], 120, "stale until the window arrives")
        turns.runUntilQuiet()
        XCTAssertEqual(store.books[120], 1200)
    }

    func testAMalformedValueInAChangeSetIsReportedNotApplied() throws {
        let transport = FakeTransport()
        let errors = LazyErrors()
        var options = LoadOptions.inproc(adapters: Adapters.none, expectedSchemaHash: 0x1234)
        options.onError = { errors.append($0) }
        let core = try UndraCore.connect(transport: transport, options: options, frameScheduler: nil)
        let store = LazyLibraryStore(core: core, handle: UndraHandle(index: 1, generation: 1), schedule: { _ in })
        transport.deliverChangeSet(Wire.ChangeSet(txnId: 1, entries: [
            Wire.ChangeEntry(handle: store.handle, signalId: 3, op: .fullValue, value: [1, 2, 3]),
        ]))
        core.mirror.flush()
        XCTAssertEqual(errors.count, 1)
        XCTAssertTrue(errors.all[0].description.contains("Library.apply(signal: 3)"))
        XCTAssertEqual(store.books.count, 0)
    }

    // MARK: The collection

    func testTheListIsARandomAccessCollectionOfOptionalRows() throws {
        let rig = try LazyRig(rows: 400)
        rig.read(0, 60)
        let rows: [Int32?] = Array(rig.list)
        XCTAssertEqual(rows.count, 400)
        XCTAssertEqual(rows[5], 5)
        XCTAssertEqual(rows[110], 110, "page 2 was prefetched")
        XCTAssertEqual(rows[300], nil, "page 6 was never read")
        XCTAssertEqual(rig.list.indices, 0 ..< 400)
        XCTAssertEqual(rig.list.index(after: 4), 5)
        XCTAssertEqual(rig.list.index(before: 4), 3)
        XCTAssertEqual(rig.list.first ?? nil, 0)
        XCTAssertEqual(rig.list.map { $0 ?? -1 }.prefix(3), [0, 1, 2])
        XCTAssertEqual(rig.list.distance(from: 3, to: 100), 97)
        rig.turns.runUntilQuiet()
        XCTAssertFalse(rig.list.isEmpty)
    }

    // MARK: Wire vectors

    func testTheLazyPayloadsRoundTripTheirSharedVectors() throws {
        let value = UndraLazyValue(handle: UndraHandle(rawValue: 1_099_511_627_777), len: 50_000, version: 7)
        assertCodec(value, hex: "010000000001000050c300000700000000000000", "lazy value")
        let invalidated = UndraLazyInvalidated(len: 50_001, version: 8)
        assertCodec(invalidated, hex: "51c300000800000000000000", "lazy invalidated")
        let header = UndraLazyPageHeader(version: 8, total: 50_001, count: 3)
        var writer = UndraWriter()
        header.undraEncode(&writer)
        for item: Int32 in [10, -1, 7] {
            item.undraEncode(&writer)
        }
        XCTAssertEqual(bytesToHex(writer.finish()), "080000000000000051c30000030000000a000000ffffffff07000000")
    }
}
