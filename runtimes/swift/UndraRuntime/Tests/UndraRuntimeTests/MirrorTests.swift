import Foundation
import XCTest
@testable import UndraRuntime

/// Records what the mirror applies, on the main actor.
@MainActor
final class EventLog {
    var events: [String] = []
    var onlyOnMainThread = true
    var depth = 0
    var maxDepth = 0
}

/// A store that records what it is given.
@MainActor
final class RecordingStore: UndraStore, @unchecked Sendable {
    var applied: [String] = []

    override func apply(signal: UInt32, op: ChangeOp, reader: inout UndraReader) {
        let value = (try? reader.readU32()) ?? 0
        applied.append("\(signal):\(op):\(value)")
    }
}

/// The mirror, stores and objects (docs/SPEC.md sections 5.5, 10.1 and 11).
@MainActor
final class MirrorTests: XCTestCase {
    private let handle = UndraHandle(index: 1, generation: 1)

    private func value(_ number: UInt32) -> [UInt8] {
        return number.undraEncoded()
    }

    private func register(_ core: UndraCore, _ handle: UndraHandle, into log: EventLog) {
        core.mirror.register(handle) { signal, op, reader in
            let number = (try? reader.readU32()) ?? 0
            log.events.append("\(signal):\(op):\(number)")
            if !Thread.isMainThread {
                log.onlyOnMainThread = false
            }
        }
    }

    // MARK: Observe

    func testObserveAppliesTheInitialValuesBeforeItReturns() throws {
        let transport = FakeTransport()
        let target = handle
        transport.onObserve = { observed, signal, on, fake in
            if on {
                fake.deliverChangeSet(makeChangeSet([
                    (observed, 0, UInt32(11).undraEncoded()),
                    (observed, 1, UInt32(22).undraEncoded()),
                ]))
            }
        }
        let core = try makeCore(transport)
        let log = EventLog()
        register(core, target, into: log)
        core.observe(target, signal: Observe.allSignals, on: true)
        XCTAssertEqual(log.events, ["0:fullValue:11", "1:fullValue:22"])
        XCTAssertEqual(core.mirror.pendingCount, 0)
        XCTAssertEqual(transport.sent, [.observe(target.rawValue, UInt32.max, true)])
    }

    func testObserveOffAndAfterShutdownReachTheTransportOnlyWhileRunning() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        core.observe(handle, signal: 3, on: false)
        core.shutdown()
        core.observe(handle, signal: 3, on: true)
        XCTAssertEqual(transport.sent, [.observe(handle.rawValue, 3, false)])
    }

    func testTheAllSignalsConstantIsTheWireValue() {
        XCTAssertEqual(Observe.allSignals, UInt32.max)
        XCTAssertEqual(Observe.allSignals, Wire.Observe.allSignals)
    }

    // MARK: Delivery

    func testChangeSetsFromAnotherThreadAreAppliedInOrderOnTheMainActor() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let log = EventLog()
        register(core, handle, into: log)
        let target = handle
        DispatchQueue.global().async {
            for index in 0 ..< 50 {
                let set = makeChangeSet(txn: UInt64(index), [(target, 0, UInt32(index).undraEncoded())])
                transport.deliverChangeSet(set)
            }
        }
        let finished = await waitUntil { log.events.last == "0:fullValue:49" }
        XCTAssertTrue(finished, "last applied: \(log.events.last ?? "nothing")")
        // A drain applies the last value of a signal it holds (ADR-031): the values applied grow
        // strictly, one per drain at most, and end at the last one committed.
        let values = log.events.compactMap { Int($0.split(separator: ":")[2]) }
        XCTAssertEqual(values.count, log.events.count)
        XCTAssertEqual(values, values.sorted())
        XCTAssertEqual(Set(values).count, values.count)
        XCTAssertLessThanOrEqual(values.count, 50)
        XCTAssertEqual(core.mirror.stats().entriesApplied, values.count)
        XCTAssertTrue(log.onlyOnMainThread)
    }

    func testChangeSetsQueuedWhileTheMainActorIsBusyAreAppliedTogether() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let log = EventLog()
        register(core, handle, into: log)
        for index in 0 ..< 3 {
            transport.deliverChangeSet(makeChangeSet([(handle, UInt32(index), value(UInt32(index)))]))
        }
        // Nothing has been applied yet: this method has not yielded the main actor.
        XCTAssertEqual(core.mirror.pendingCount, 3)
        XCTAssertEqual(log.events, [])
        core.mirror.flush()
        XCTAssertEqual(log.events, ["0:fullValue:0", "1:fullValue:1", "2:fullValue:2"])
        XCTAssertEqual(core.mirror.pendingCount, 0)
    }

    func testTheHopAppliesWhatWasQueuedWithoutAnExplicitFlush() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let log = EventLog()
        register(core, handle, into: log)
        transport.deliverChangeSet(makeChangeSet([(handle, 4, value(9))]))
        let applied = await waitUntil { log.events == ["4:fullValue:9"] }
        XCTAssertTrue(applied)
    }

    func testEntriesForOtherHandlesAndForUnregisteredHandlesAreDropped() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let log = EventLog()
        let other = UndraHandle(index: 9, generation: 9)
        register(core, handle, into: log)
        transport.deliverChangeSet(makeChangeSet([(other, 0, value(1)), (handle, 1, value(2))]))
        core.mirror.flush()
        XCTAssertEqual(log.events, ["1:fullValue:2"])
        core.mirror.unregister(handle)
        transport.deliverChangeSet(makeChangeSet([(handle, 1, value(3))]))
        core.mirror.flush()
        XCTAssertEqual(log.events, ["1:fullValue:2"])
        XCTAssertEqual(core.mirror.registeredCount, 0)
    }

    func testAMalformedChangeSetIsDroppedAndTheNextOneStillApplies() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let log = EventLog()
        register(core, handle, into: log)
        transport.deliverRawChangeSet([0xFF, 0xFF, 0xFF])
        transport.deliverChangeSet(makeChangeSet([(handle, 0, value(5))]))
        core.mirror.flush()
        XCTAssertEqual(log.events, ["0:fullValue:5"])
    }

    func testTheOpIsPassedThrough() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let log = EventLog()
        register(core, handle, into: log)
        let set = Wire.ChangeSet(txnId: 1, entries: [
            Wire.ChangeEntry(handle: handle, signalId: 2, op: .keyedPatch, value: ArraySlice(value(0))),
            Wire.ChangeEntry(handle: handle, signalId: 3, op: .lazyListInvalidated),
        ])
        transport.deliverChangeSet(set)
        core.mirror.flush()
        XCTAssertEqual(log.events, ["2:keyedPatch:0", "3:lazyListInvalidated:0"])
    }

    func testReobservingFromInsideApplyIsAppliedAfterwardsWithoutRecursion() throws {
        let transport = FakeTransport()
        let target = handle
        transport.onObserve = { observed, signal, on, fake in
            if on {
                fake.deliverChangeSet(makeChangeSet([(observed, signal, UInt32(2).undraEncoded())]))
            }
        }
        let core = try makeCore(transport)
        let log = EventLog()
        core.mirror.register(target) { [core] signal, op, reader in
            let number = (try? reader.readU32()) ?? 0
            log.depth += 1
            log.maxDepth = Swift.max(log.maxDepth, log.depth)
            log.events.append("\(signal):\(number)")
            if number == 1 {
                // A store that lost sync re-observes the signal, exactly like generated code.
                core.observe(target, signal: signal, on: false)
                core.observe(target, signal: signal, on: true)
            }
            log.depth -= 1
        }
        transport.deliverChangeSet(makeChangeSet([(target, 7, value(1))]))
        core.mirror.flush()
        XCTAssertEqual(log.events, ["7:1", "7:2"])
        XCTAssertEqual(log.maxDepth, 1, "apply must not be re-entered")
    }

    // MARK: Stores

    func testAStoreRegistersWithTheMirrorAndReceivesItsChangeSets() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let store = RecordingStore(core: core, handle: handle)
        XCTAssertEqual(core.mirror.registeredCount, 1)
        transport.deliverChangeSet(makeChangeSet([(handle, 0, value(7))]))
        core.mirror.flush()
        XCTAssertEqual(store.applied, ["0:fullValue:7"])
        XCTAssertEqual(store.handle, handle)
        XCTAssertTrue(store.core === core)
    }

    func testAStoreThatObservesInItsInitializerHasItsValuesWhenItReturns() throws {
        let transport = FakeTransport()
        let target = handle
        transport.onObserve = { observed, _, on, fake in
            if on {
                fake.deliverChangeSet(makeChangeSet([(observed, 0, UInt32(42).undraEncoded())]))
            }
        }
        let core = try makeCore(transport)
        let store = RecordingStore(core: core, handle: target)
        core.observe(target, signal: Observe.allSignals, on: true)
        XCTAssertEqual(store.applied, ["0:fullValue:42"])
    }

    func testClosingAStoreUnregistersAndReleasesExactlyOnce() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let store = RecordingStore(core: core, handle: handle)
        XCTAssertFalse(store.isClosed)
        store.close()
        store.close()
        XCTAssertTrue(store.isClosed)
        XCTAssertEqual(transport.releases, [handle.rawValue])
        XCTAssertEqual(core.mirror.registeredCount, 0)
        transport.deliverChangeSet(makeChangeSet([(handle, 0, value(7))]))
        core.mirror.flush()
        XCTAssertEqual(store.applied, [])
    }

    func testDroppingAStoreReleasesItsHandleAsABackstop() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        var store: RecordingStore? = RecordingStore(core: core, handle: handle)
        XCTAssertEqual(core.stats().hostLiveHandles, 1)
        XCTAssertEqual(transport.releases, [])
        store = nil
        XCTAssertNil(store)
        XCTAssertEqual(transport.releases, [handle.rawValue])
        XCTAssertEqual(core.mirror.registeredCount, 0)
        XCTAssertEqual(core.stats().hostLiveHandles, 0)
    }

    func testClosingThenDroppingAStoreDoesNotReleaseTwice() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        var store: RecordingStore? = RecordingStore(core: core, handle: handle)
        store?.close()
        store = nil
        XCTAssertNil(store)
        XCTAssertEqual(transport.releases, [handle.rawValue])
    }

    // MARK: Objects

    func testClosingAnObjectFromManyThreadsReleasesOnce() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let object = UndraObject(core: core, handle: handle)
        DispatchQueue.concurrentPerform(iterations: 64) { _ in
            object.close()
        }
        XCTAssertEqual(transport.releases, [handle.rawValue])
        XCTAssertTrue(object.isClosed)
    }

    func testDroppingAnObjectReleasesItsHandle() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        do {
            let object = UndraObject(core: core, handle: handle)
            XCTAssertEqual(object.handle, handle)
        }
        XCTAssertEqual(transport.releases, [handle.rawValue])
        XCTAssertEqual(core.stats().hostLiveHandles, 0)
    }

    func testReleasingAfterShutdownIsANoOp() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let object = UndraObject(core: core, handle: handle)
        core.shutdown()
        object.close()
        XCTAssertEqual(transport.releases, [])
    }
}
