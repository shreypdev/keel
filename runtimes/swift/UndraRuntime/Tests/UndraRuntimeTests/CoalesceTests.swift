import Foundation
import Observation
import XCTest
@testable import UndraRuntime

// Frame-coalesced delivery (ADR-031, docs/SPEC.md section 11): the merge rules of a drain, its
// equivalence with applying every change-set in order (property test), the bounded backlog,
// read-your-writes for replies and synchronous calls, frame scheduling, the counters and the drain
// listener, and the `no_coalesce` opt-out.

// MARK: - Helpers

private let storeHandle = UndraHandle(index: 1, generation: 1)

private func u32(_ value: UInt32) -> [UInt8] {
    return value.undraEncoded()
}

/// A keyed patch of `ops`, encoded by the runtime's own encoder.
private func patchBytes<T: UndraCodec>(_ ops: [PatchOp<T>]) -> [UInt8] {
    var writer = UndraWriter()
    encodePatch(ops, into: &writer)
    return writer.finish()
}

private func fullEntry(_ signal: UInt32, _ value: [UInt8], handle: UndraHandle = storeHandle) -> Wire.ChangeEntry {
    return Wire.ChangeEntry(handle: handle, signalId: signal, op: .fullValue, value: ArraySlice(value))
}

private func patchEntry(_ signal: UInt32, _ value: [UInt8], handle: UndraHandle = storeHandle) -> Wire.ChangeEntry {
    return Wire.ChangeEntry(handle: handle, signalId: signal, op: .keyedPatch, value: ArraySlice(value))
}

private func invalidationEntry(_ signal: UInt32, handle: UndraHandle = storeHandle) -> Wire.ChangeEntry {
    return Wire.ChangeEntry(handle: handle, signalId: signal, op: .lazyListInvalidated)
}

private func changeSet(_ entries: Wire.ChangeEntry...) -> Wire.ChangeSet {
    return Wire.ChangeSet(txnId: 1, entries: entries)
}

extension SplitMix64 {
    /// A value in `0 ..< bound`.
    mutating func below(_ bound: Int) -> Int {
        return Int(next() % UInt64(bound))
    }
}

/// What a raw mirror registration was given, in order.
@MainActor
private final class ApplyLog {
    var applied: [(signal: UInt32, op: ChangeOp, value: [UInt8])] = []

    var u32Values: [UInt32] {
        return applied.map { entry in (try? UInt32.undraDecoded(from: entry.value)) ?? UInt32.max }
    }

    var ops: [ChangeOp] {
        return applied.map { $0.op }
    }
}

@MainActor
private func recordApplies(_ core: UndraCore, _ handle: UndraHandle = storeHandle, noCoalesce: Set<UInt32> = [], into log: ApplyLog) {
    core.mirror.register(handle, noCoalesce: noCoalesce) { signal, op, reader in
        log.applied.append((signal: signal, op: op, value: Array(reader.readRemaining())))
    }
}

/// A store with one `u32` counter (signal 0), applied the way generated code applies it.
@MainActor
private final class CounterProbe: UndraStore, @unchecked Sendable {
    private(set) var count: UInt32 = 0
    private(set) var applies = 0

    override func apply(signal: UInt32, op: ChangeOp, reader: inout UndraReader) {
        applies += 1
        if signal == 0, op == .fullValue, let value = try? UInt32.undraDecode(&reader) {
            count = value
        }
    }
}

/// Little-endian helpers for hand-encoded payloads (the backlog test sends a million of them).
private extension Array where Element == UInt8 {
    mutating func appendU32(_ value: UInt32) {
        append(UInt8(truncatingIfNeeded: value))
        append(UInt8(truncatingIfNeeded: value >> 8))
        append(UInt8(truncatingIfNeeded: value >> 16))
        append(UInt8(truncatingIfNeeded: value >> 24))
    }

    mutating func appendU64(_ value: UInt64) {
        appendU32(UInt32(truncatingIfNeeded: value))
        appendU32(UInt32(truncatingIfNeeded: value >> 32))
    }
}

/// The core's side of a `ListHostStore`: the truth for keyed `u32` lists and `u32` scalars, and
/// the change-sets (one entry each, encoded by hand) that move it.
private final class ListModel: @unchecked Sendable {
    let handle: UndraHandle
    var lists: [UInt32: [UInt32]] = [:]
    var scalars: [UInt32: UInt32] = [:]
    private var nextItem: UInt32 = 1
    private var txn: UInt64 = 0

    init(handle: UndraHandle, lists: [UInt32], scalars: [UInt32]) {
        self.handle = handle
        for id in lists {
            self.lists[id] = []
        }
        for id in scalars {
            self.scalars[id] = 0
        }
    }

    private func payload(_ signal: UInt32, _ op: ChangeOp, _ value: [UInt8]) -> [UInt8] {
        txn += 1
        var out: [UInt8] = []
        out.reserveCapacity(33 + value.count)
        out.appendU64(txn)
        out.appendU32(1)
        out.appendU64(handle.rawValue)
        out.appendU32(signal)
        out.append(op.rawValue)
        out.appendU32(UInt32(value.count))
        out.append(contentsOf: value)
        return out
    }

    /// The current full value of `signal`.
    func full(_ signal: UInt32) -> [UInt8] {
        if let list = lists[signal] {
            return payload(signal, .fullValue, list.undraEncoded())
        }
        return payload(signal, .fullValue, u32(scalars[signal] ?? 0))
    }

    /// `count` random valid ops on list `signal` (applied to the truth); `corrupt` appends one
    /// out-of-bounds op the core never made.
    func patch(_ signal: UInt32, _ rng: inout SplitMix64, count: Int, corrupt: Bool) -> [UInt8] {
        var list = lists[signal] ?? []
        var ops: [UInt8] = []
        ops.appendU32(UInt32(count + (corrupt ? 1 : 0)))
        for _ in 0 ..< count {
            let kind = list.isEmpty ? 0 : rng.below(5)
            switch kind {
            case 0:
                let index = rng.below(list.count + 1)
                list.insert(nextItem, at: index)
                ops.append(0)
                ops.appendU32(UInt32(index))
                ops.appendU32(nextItem)
                nextItem += 1
            case 1:
                let index = rng.below(list.count)
                list.remove(at: index)
                ops.append(1)
                ops.appendU32(UInt32(index))
            case 2:
                let index = rng.below(list.count)
                list[index] = nextItem
                ops.append(2)
                ops.appendU32(UInt32(index))
                ops.appendU32(nextItem)
                nextItem += 1
            case 3:
                let from = rng.below(list.count)
                let to = rng.below(list.count)
                let item = list.remove(at: from)
                list.insert(item, at: to)
                ops.append(3)
                ops.appendU32(UInt32(from))
                ops.appendU32(UInt32(to))
            default:
                if rng.below(10) == 0 {
                    list.removeAll()
                    ops.append(4)
                } else {
                    list.append(nextItem)
                    ops.append(0)
                    ops.appendU32(UInt32(list.count - 1))
                    ops.appendU32(nextItem)
                    nextItem += 1
                }
            }
        }
        if corrupt {
            ops.append(1)
            ops.appendU32(UInt32(list.count + 3))
        }
        lists[signal] = list
        return payload(signal, .keyedPatch, ops)
    }

    /// A new full value of list `signal`.
    func replace(_ signal: UInt32, _ rng: inout SplitMix64) -> [UInt8] {
        var list: [UInt32] = []
        for _ in 0 ..< rng.below(6) {
            list.append(nextItem)
            nextItem += 1
        }
        lists[signal] = list
        return payload(signal, .fullValue, list.undraEncoded())
    }

    func scalar(_ signal: UInt32, _ value: UInt32) -> [UInt8] {
        scalars[signal] = value
        return payload(signal, .fullValue, u32(value))
    }
}

/// A store of keyed `u32` lists and `u32` scalars that applies entries the way generated code
/// does: a full value decodes, a keyed patch goes through `applyPatch`, and a patch that does not
/// fit the list re-observes the signal (off, then on).
@MainActor
private final class ListHostStore: UndraStore, @unchecked Sendable {
    private let listIds: Set<UInt32>
    private(set) var lists: [UInt32: [UInt32]] = [:]
    private(set) var scalars: [UInt32: UInt32] = [:]
    private(set) var applies: [UInt32: Int] = [:]
    private(set) var patchFailures = 0

    init(core: UndraCore, handle: UndraHandle, lists: [UInt32], scalars: [UInt32], noCoalesce: Set<UInt32> = []) {
        self.listIds = Set(lists)
        for id in lists {
            self.lists[id] = []
        }
        for id in scalars {
            self.scalars[id] = 0
        }
        super.init(core: core, handle: handle, noCoalesce: noCoalesce)
    }

    override func apply(signal: UInt32, op: ChangeOp, reader: inout UndraReader) {
        applies[signal, default: 0] += 1
        do {
            if listIds.contains(signal) {
                switch op {
                case .fullValue:
                    lists[signal] = try [UInt32].undraDecode(&reader)
                    try reader.finish()
                case .keyedPatch:
                    let ops: [PatchOp<UInt32>] = try decodePatch(&reader)
                    try reader.finish()
                    try applyPatch(ops, to: &lists[signal, default: []])
                case .lazyListInvalidated:
                    break
                }
            } else if op == .fullValue {
                scalars[signal] = try UInt32.undraDecode(&reader)
                try reader.finish()
            }
        } catch is PatchError {
            patchFailures += 1
            core.observe(handle, signal: signal, on: false)
            core.observe(handle, signal: signal, on: true)
        } catch {
            XCTFail("undecodable change for signal \(signal): \(error)")
        }
    }
}

/// A core over a fake transport that answers `observe` with `model`'s current full value
/// synchronously (in process), plus its store and its hand-driven frames.
@MainActor
private struct ListHost {
    let transport: FakeTransport
    let frames: ManualFrameScheduler
    let core: UndraCore
    let store: ListHostStore

    init(model: ListModel, lists: [UInt32], scalars: [UInt32], maxPendingEntries: Int = 65_536) throws {
        transport = FakeTransport()
        transport.onObserve = { _, signal, on, fake in
            if on {
                fake.deliverRawChangeSet(model.full(signal))
            }
        }
        frames = ManualFrameScheduler()
        core = try makeCore(transport, frames: frames, maxPendingEntries: maxPendingEntries)
        store = ListHostStore(core: core, handle: model.handle, lists: lists, scalars: scalars)
    }
}

// MARK: - Stores that use the generated shapes

/// A generated-shape store with a `no_coalesce` signal (`super.init(..., noCoalesce: [1])`).
@MainActor @Observable
public final class ObservableProgressStore: UndraStore, @unchecked Sendable {
    public private(set) var total: UInt32 = 0
    public private(set) var progress: UInt32 = 0
    @ObservationIgnored var progressSeen: [UInt32] = []

    init(adopting handle: UndraHandle, core: UndraCore) {
        super.init(core: core, handle: handle, noCoalesce: [1])
    }

    public override func apply(signal: UInt32, op: ChangeOp, reader: inout UndraReader) {
        guard op == .fullValue, let value = try? UInt32.undraDecode(&reader) else {
            return
        }
        if signal == 0 {
            total = value
        } else if signal == 1 {
            progress = value
            progressSeen.append(value)
        }
    }
}

/// A generated-shape store without such signals (`super.init(core:handle:)`).
@MainActor @Observable
public final class ObservableCounterStore: UndraStore, @unchecked Sendable {
    public private(set) var count: UInt32 = 0

    init(adopting handle: UndraHandle, core: UndraCore) {
        super.init(core: core, handle: handle)
    }

    public override func apply(signal: UInt32, op: ChangeOp, reader: inout UndraReader) {
        if signal == 0, op == .fullValue, let value = try? UInt32.undraDecode(&reader) {
            count = value
        }
    }
}

// MARK: - Tests

@MainActor
final class CoalesceTests: XCTestCase {
    // MARK: Merge rules

    func testOnlyTheLastFullValueOfASignalIsAppliedOnce() throws {
        let frames = ManualFrameScheduler()
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: frames)
        let log = ApplyLog()
        recordApplies(core, into: log)
        for index in 1 ... 1000 {
            transport.deliverChangeSet(changeSet(fullEntry(0, u32(UInt32(index)))))
        }
        XCTAssertEqual(frames.requests, 1, "one frame for the whole burst")
        XCTAssertEqual(log.applied.count, 0)
        frames.fire()
        XCTAssertEqual(log.u32Values, [1000])
        let stats = core.mirror.stats()
        XCTAssertEqual(stats.changeSetsReceived, 1000)
        XCTAssertEqual(stats.entriesReceived, 1000)
        XCTAssertEqual(stats.entriesApplied, 1)
        XCTAssertEqual(stats.drains, 1)
        XCTAssertEqual(stats.pendingEntries, 0)
        XCTAssertEqual(stats.pendingBytes, 0)
    }

    func testConsecutiveKeyedPatchesAreConcatenatedIntoOne() throws {
        let core = try makeCore(FakeTransport(), frames: ManualFrameScheduler())
        let log = ApplyLog()
        recordApplies(core, into: log)
        let first: [PatchOp<UInt32>] = [.insert(index: 0, item: 7)]
        let second: [PatchOp<UInt32>] = [.move(from: 0, to: 0), .update(index: 0, item: 8)]
        let third: [PatchOp<UInt32>] = [.clear]
        let transport = core.transport as! FakeTransport
        for ops in [first, second, third] {
            transport.deliverChangeSet(changeSet(patchEntry(3, patchBytes(ops))))
        }
        core.mirror.flush()
        XCTAssertEqual(log.applied.count, 1)
        XCTAssertEqual(log.applied[0].op, .keyedPatch)
        XCTAssertEqual(log.applied[0].signal, 3)
        // Count = 1 + 2 + 1, then the ops' bytes in arrival order: exactly the encoder's output.
        XCTAssertEqual(log.applied[0].value, patchBytes(first + second + third))
        var reader = UndraReader(log.applied[0].value)
        let decoded: [PatchOp<UInt32>] = try decodePatch(&reader)
        try reader.finish()
        XCTAssertEqual(decoded, first + second + third)
    }

    func testASinglePatchPassesAsItIs() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: ManualFrameScheduler())
        let log = ApplyLog()
        recordApplies(core, into: log)
        let bytes = patchBytes([PatchOp<UInt32>.insert(index: 0, item: 1)])
        transport.deliverChangeSet(changeSet(patchEntry(0, bytes)))
        core.mirror.flush()
        XCTAssertEqual(log.applied.map { $0.value }, [bytes])
    }

    func testASignalIsAppliedAtMostTwiceItsLastFullValueThenTheMergedPatch() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: ManualFrameScheduler())
        let log = ApplyLog()
        recordApplies(core, into: log)
        transport.deliverChangeSet(changeSet(patchEntry(0, patchBytes([PatchOp<UInt32>.insert(index: 0, item: 1)]))))
        transport.deliverChangeSet(changeSet(fullEntry(0, [UInt32(5)].undraEncoded())))
        transport.deliverChangeSet(changeSet(patchEntry(0, patchBytes([PatchOp<UInt32>.insert(index: 1, item: 6)]))))
        transport.deliverChangeSet(changeSet(patchEntry(0, patchBytes([PatchOp<UInt32>.insert(index: 2, item: 7)]))))
        core.mirror.flush()
        XCTAssertEqual(log.ops, [.fullValue, .keyedPatch])
        XCTAssertEqual(try [UInt32].undraDecoded(from: log.applied[0].value), [5])
        XCTAssertEqual(log.applied[1].value, patchBytes([PatchOp<UInt32>.insert(index: 1, item: 6), .insert(index: 2, item: 7)]))
    }

    func testALazyInvalidationSupersedesWhatCameBeforeIt() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: ManualFrameScheduler())
        let log = ApplyLog()
        recordApplies(core, into: log)
        transport.deliverChangeSet(changeSet(fullEntry(0, u32(1))))
        transport.deliverChangeSet(changeSet(patchEntry(0, patchBytes([PatchOp<UInt32>.clear]))))
        transport.deliverChangeSet(changeSet(invalidationEntry(0)))
        core.mirror.flush()
        XCTAssertEqual(log.ops, [.lazyListInvalidated])
        // A full value after an invalidation supersedes it in turn.
        transport.deliverChangeSet(changeSet(invalidationEntry(0)))
        transport.deliverChangeSet(changeSet(fullEntry(0, u32(2))))
        core.mirror.flush()
        XCTAssertEqual(log.ops, [.lazyListInvalidated, .fullValue])
    }

    func testSignalsAreAppliedInTheOrderOfTheirFirstEntry() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: ManualFrameScheduler())
        let order = ApplyLog()
        let first = UndraHandle(index: 1, generation: 1)
        let second = UndraHandle(index: 2, generation: 1)
        core.mirror.register(first) { signal, op, reader in
            order.applied.append((signal: 100 + signal, op: op, value: Array(reader.readRemaining())))
        }
        core.mirror.register(second) { signal, op, reader in
            order.applied.append((signal: 200 + signal, op: op, value: Array(reader.readRemaining())))
        }
        transport.deliverChangeSet(changeSet(fullEntry(0, u32(1), handle: second)))
        transport.deliverChangeSet(changeSet(fullEntry(1, u32(1), handle: first), fullEntry(0, u32(1), handle: first)))
        transport.deliverChangeSet(changeSet(fullEntry(0, u32(2), handle: second), fullEntry(0, u32(2), handle: first)))
        core.mirror.flush()
        XCTAssertEqual(order.applied.map { $0.signal }, [200, 101, 100])
        XCTAssertEqual(order.u32Values, [2, 1, 2])
    }

    func testEntriesOfAnUnregisteredHandleAreDroppedAndEveryOneIsCounted() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: ManualFrameScheduler())
        let stranger = UndraHandle(index: 9, generation: 9)
        for index in 0 ..< 5 {
            transport.deliverChangeSet(changeSet(fullEntry(0, u32(UInt32(index)), handle: stranger)))
        }
        transport.deliverChangeSet(changeSet(patchEntry(1, patchBytes([PatchOp<UInt32>.clear]), handle: stranger)))
        core.mirror.flush()
        let stats = core.mirror.stats()
        XCTAssertEqual(stats.droppedEntries, 6)
        XCTAssertEqual(stats.entriesApplied, 0)
        XCTAssertEqual(core.stats().mirror.droppedEntries, 6)
    }

    func testAPatchTooShortForItsCountIsDroppedAndItsSignalReobserved() throws {
        let model = ListModel(handle: storeHandle, lists: [0], scalars: [])
        model.lists[0] = [4, 5, 6]
        let host = try ListHost(model: model, lists: [0], scalars: [])
        host.transport.deliverRawChangeSet(model.full(0))
        host.core.mirror.flush()
        XCTAssertEqual(host.store.lists[0], [4, 5, 6])
        // The truth moves on; the host gets a patch it cannot merge, then a valid one relative to
        // a list it never had.
        model.lists[0] = [4, 5, 6, 7, 8]
        host.transport.deliverChangeSet(changeSet(patchEntry(0, [1, 0])))
        host.transport.deliverChangeSet(changeSet(patchEntry(0, patchBytes([PatchOp<UInt32>.insert(index: 4, item: 8)]))))
        host.core.mirror.flush()
        XCTAssertEqual(host.store.lists[0], [4, 5, 6, 7, 8])
        XCTAssertEqual(host.store.applies[0], 2, "the initial value, then the re-observed one: nothing in between")
        XCTAssertEqual(host.transport.sent, [.observe(storeHandle.rawValue, 0, true)])
        XCTAssertEqual(host.core.mirror.stats().resyncs, 1)
        // The signal was answered: patches apply again.
        host.transport.deliverChangeSet(changeSet(patchEntry(0, patchBytes([PatchOp<UInt32>.remove(index: 0)]))))
        host.core.mirror.flush()
        XCTAssertEqual(host.store.lists[0], [5, 6, 7, 8])
    }

    func testAPatchTooShortForItsCountNeedsNoResyncWhenAFullValueFollows() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: ManualFrameScheduler())
        let log = ApplyLog()
        recordApplies(core, into: log)
        transport.deliverChangeSet(changeSet(patchEntry(0, [9])))
        transport.deliverChangeSet(changeSet(patchEntry(0, patchBytes([PatchOp<UInt32>.clear]))))
        transport.deliverChangeSet(changeSet(fullEntry(0, [UInt32(3)].undraEncoded())))
        transport.deliverChangeSet(changeSet(patchEntry(0, patchBytes([PatchOp<UInt32>.insert(index: 1, item: 4)]))))
        core.mirror.flush()
        XCTAssertEqual(log.ops, [.fullValue, .keyedPatch])
        XCTAssertEqual(transport.sent, [])
        XCTAssertEqual(core.mirror.stats().resyncs, 0)
    }

    func testAMalformedChangeSetIsDroppedWholeAndNotCounted() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: ManualFrameScheduler())
        let log = ApplyLog()
        recordApplies(core, into: log)
        var bytes = changeSet(fullEntry(0, u32(1)), fullEntry(1, u32(2))).encode()
        bytes.removeLast()
        transport.deliverRawChangeSet(bytes)
        XCTAssertEqual(core.mirror.pendingCount, 0)
        XCTAssertEqual(core.mirror.stats().changeSetsReceived, 0)
        transport.deliverChangeSet(changeSet(fullEntry(0, u32(3))))
        core.mirror.flush()
        XCTAssertEqual(log.u32Values, [3])
    }

    // MARK: no_coalesce

    func testNoCoalesceSignalsAreAppliedEntryByEntryInArrivalOrder() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: ManualFrameScheduler())
        let log = ApplyLog()
        recordApplies(core, noCoalesce: [1], into: log)
        for index in UInt32(1) ... 3 {
            transport.deliverChangeSet(changeSet(fullEntry(0, u32(10 * index)), fullEntry(1, u32(index))))
        }
        core.mirror.flush()
        XCTAssertEqual(log.applied.map { $0.signal }, [0, 1, 1, 1])
        XCTAssertEqual(log.u32Values, [30, 1, 2, 3])
        XCTAssertEqual(core.mirror.stats().entriesApplied, 4)
    }

    func testNoCoalesceSignalsAreFoldedWhenTheBacklogPassesItsBound() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: ManualFrameScheduler(), maxPendingEntries: 10)
        let log = ApplyLog()
        recordApplies(core, noCoalesce: [0], into: log)
        for index in UInt32(1) ... 25 {
            transport.deliverChangeSet(changeSet(fullEntry(0, u32(index))))
        }
        core.mirror.flush()
        XCTAssertGreaterThan(core.mirror.stats().compactions, 0)
        XCTAssertEqual(log.u32Values.last, 25)
        XCTAssertLessThan(log.applied.count, 25)
        XCTAssertEqual(log.u32Values, log.u32Values.sorted())
    }

    func testGeneratedShapedStoresPassTheirNoCoalesceSignals() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: ManualFrameScheduler())
        let progress = ObservableProgressStore(adopting: storeHandle, core: core)
        let otherHandle = UndraHandle(index: 2, generation: 1)
        let counter = ObservableCounterStore(adopting: otherHandle, core: core)
        for index in UInt32(1) ... 4 {
            transport.deliverChangeSet(changeSet(
                fullEntry(0, u32(100 * index)),
                fullEntry(1, u32(index)),
                fullEntry(0, u32(index), handle: otherHandle)
            ))
        }
        core.mirror.flush()
        XCTAssertEqual(progress.progressSeen, [1, 2, 3, 4], "every value of the no_coalesce signal")
        XCTAssertEqual(progress.progress, 4)
        XCTAssertEqual(progress.total, 400)
        XCTAssertEqual(counter.count, 4)
        // 1 total + 4 progress + 1 counter.
        XCTAssertEqual(core.mirror.stats().entriesApplied, 6)
        progress.close()
        counter.close()
    }

    // MARK: Equivalence with sequential application (property test)

    func testAMergedDrainEqualsApplyingEveryChangeSetInOrder() throws {
        let lists: [UInt32] = [0, 2]
        let scalars: [UInt32] = [1]
        var corrupted = 0
        for seed in UInt64(1) ... 600 {
            var rng = SplitMix64(seed: seed)
            let model = ListModel(handle: storeHandle, lists: lists, scalars: scalars)
            // Sequential: one drain per change-set, as the core commits them.
            let sequential = try ListHost(model: model, lists: lists, scalars: scalars)
            // Merged: every change-set committed before one drain.
            let merged = try ListHost(model: model, lists: lists, scalars: scalars)
            func deliver(_ payload: [UInt8]) {
                sequential.transport.deliverRawChangeSet(payload)
                sequential.core.mirror.flush()
                merged.transport.deliverRawChangeSet(payload)
            }
            for id in lists + scalars {
                deliver(model.full(id))
            }
            var corrupt = false
            let steps = 1 + rng.below(40)
            for _ in 0 ..< steps {
                let pick = rng.below(100)
                let list = lists[rng.below(lists.count)]
                if pick < 10 {
                    deliver(model.replace(list, &rng))
                } else if pick < 25 {
                    deliver(model.scalar(1, UInt32(rng.below(1000))))
                } else {
                    let bad = rng.below(25) == 0
                    corrupt = corrupt || bad
                    deliver(model.patch(list, &rng, count: 1 + rng.below(4), corrupt: bad))
                }
            }
            merged.core.mirror.flush()

            let context = "seed \(seed) (replay with SplitMix64(seed: \(seed)))"
            if corrupt {
                corrupted += 1
            }
            var held = sequential.store.lists == model.lists && sequential.store.scalars == model.scalars
                && merged.store.lists == model.lists && merged.store.scalars == model.scalars
                && merged.core.mirror.pendingCount == 0 && merged.core.mirror.stats().drains == 1
            XCTAssertEqual(sequential.store.lists, model.lists, context)
            XCTAssertEqual(sequential.store.scalars, model.scalars, context)
            XCTAssertEqual(merged.store.lists, model.lists, context)
            XCTAssertEqual(merged.store.scalars, model.scalars, context)
            XCTAssertEqual(merged.core.mirror.pendingCount, 0, context)
            XCTAssertEqual(merged.core.mirror.stats().drains, 1, context)
            if !corrupt {
                // Merging applies each signal at most twice: its last full value, its merged patch.
                for (signal, count) in merged.store.applies where count > 2 {
                    XCTFail("\(context): signal \(signal) applied \(count) times in one drain")
                    held = false
                }
                XCTAssertEqual(merged.store.patchFailures, 0, context)
            }
            if !held {
                break
            }
        }
        XCTAssertGreaterThan(corrupted, 10, "some histories carried an out-of-bounds patch")
    }

    // MARK: The bounded backlog

    func testAMillionChangeSetsWithNoDrainStayUnderTheBoundAndOneDrainConverges() async throws {
        let lists: [UInt32] = [0, 1]
        let scalars: [UInt32] = [2, 3, 4, 5]
        let model = ListModel(handle: storeHandle, lists: lists, scalars: scalars)
        let host = try ListHost(model: model, lists: lists, scalars: scalars)
        for id in lists + scalars {
            host.transport.deliverRawChangeSet(model.full(id))
        }
        let transport = host.transport
        let mirror = host.core.mirror
        // The main thread is blocked: nothing drains until the producer is done.
        let peaks = await withCheckedContinuation { (continuation: CheckedContinuation<(entries: Int, bytes: Int), Never>) in
            DispatchQueue.global().async {
                var rng = SplitMix64(seed: 31337)
                var maxEntries = 0
                var maxBytes = 0
                for index in 0 ..< 1_000_000 {
                    if rng.below(10) < 6 {
                        transport.deliverRawChangeSet(model.patch(lists[index & 1], &rng, count: 1, corrupt: false))
                    } else {
                        transport.deliverRawChangeSet(model.scalar(scalars[index % scalars.count], UInt32(index)))
                    }
                    if index & 1023 == 0 {
                        let stats = mirror.stats()
                        maxEntries = Swift.max(maxEntries, stats.pendingEntries)
                        maxBytes = Swift.max(maxBytes, stats.pendingBytes)
                    }
                }
                continuation.resume(returning: (entries: maxEntries, bytes: maxBytes))
            }
        }
        let before = mirror.stats()
        XCTAssertLessThanOrEqual(peaks.entries, 65_536)
        XCTAssertLessThanOrEqual(peaks.bytes, 16 * 1024 * 1024)
        XCTAssertLessThanOrEqual(before.pendingEntries, 65_536)
        XCTAssertLessThanOrEqual(before.pendingBytes, 16 * 1024 * 1024)
        XCTAssertGreaterThan(before.compactions, 5)
        XCTAssertEqual(before.changeSetsReceived, 1_000_006)
        XCTAssertEqual(before.drains, 0)
        XCTAssertEqual(host.frames.requests, 1, "one frame request for the whole backlog")

        mirror.flush()
        XCTAssertEqual(host.store.lists, model.lists)
        XCTAssertEqual(host.store.scalars, model.scalars)
        XCTAssertEqual(mirror.pendingCount, 0)
        XCTAssertEqual(host.store.patchFailures, 0)
        // Each list's merged patch passed both patch bounds (well over 4,096 ops and 1 MiB), so a
        // compaction dropped it and the drain re-observed the list, once.
        XCTAssertEqual(host.transport.sent, [.observe(storeHandle.rawValue, 0, true), .observe(storeHandle.rawValue, 1, true)])
        let after = mirror.stats()
        XCTAssertEqual(after.resyncs, 2)
        XCTAssertEqual(after.drains, 1)
        XCTAssertLessThanOrEqual(host.store.applies.values.max() ?? 0, 2)
    }

    func testAMergedPatchPastBothBoundsIsDroppedAndReobservedOnce() throws {
        final class Truth: @unchecked Sendable {
            var items: [String] = []
        }
        let truth = Truth()
        let transport = FakeTransport()
        transport.onObserve = { _, signal, on, fake in
            if on {
                fake.deliverChangeSet(changeSet(fullEntry(signal, truth.items.undraEncoded())))
            }
        }
        let core = try makeCore(transport, frames: ManualFrameScheduler(), maxPendingEntries: 1000)
        let applied = ListBox()
        core.mirror.register(storeHandle) { _, op, reader in
            do {
                switch op {
                case .fullValue:
                    applied.items = try [String].undraDecode(&reader)
                case .keyedPatch:
                    let ops: [PatchOp<String>] = try decodePatch(&reader)
                    try applyPatch(ops, to: &applied.items)
                case .lazyListInvalidated:
                    break
                }
            } catch {
                XCTFail("\(error)")
            }
        }
        let text = String(repeating: "x", count: 300)
        // Compactions run every 1,000 entries; the one after the 4,096th op finds the merged patch
        // past both bounds (more than 4,096 ops, more than 1 MiB).
        for index in 0 ..< Mirror.maxMergedPatchOps + 1500 {
            let item = "\(index)\(text)"
            let op = PatchOp<String>.insert(index: UInt32(truth.items.count), item: item)
            truth.items.append(item)
            transport.deliverChangeSet(changeSet(patchEntry(0, patchBytes([op]))))
        }
        let stats = core.mirror.stats()
        XCTAssertGreaterThan(stats.compactions, 0)
        // Patches that arrive while the signal waits for its full value are discarded.
        XCTAssertLessThan(stats.pendingEntries, 1000)
        XCTAssertEqual(transport.sent, [], "nothing re-observed from the enqueuing thread")
        core.mirror.flush()
        XCTAssertEqual(transport.sent, [.observe(storeHandle.rawValue, 0, true)])
        XCTAssertEqual(core.mirror.stats().resyncs, 1)
        XCTAssertEqual(applied.items, truth.items)
        // Once answered, patches apply again.
        let extra = PatchOp<String>.insert(index: 0, item: "head")
        truth.items.insert("head", at: 0)
        transport.deliverChangeSet(changeSet(patchEntry(0, patchBytes([extra]))))
        core.mirror.flush()
        XCTAssertEqual(applied.items, truth.items)
        XCTAssertEqual(core.mirror.stats().resyncs, 1)
    }

    func testTheBacklogIsFoldedWhenItsBytesPassTheirBound() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: ManualFrameScheduler(), maxPendingBytes: 64 * 1024)
        let log = ApplyLog()
        recordApplies(core, into: log)
        let blob = [UInt8](repeating: 7, count: 4096)
        var peak = 0
        for _ in 0 ..< 100 {
            transport.deliverChangeSet(changeSet(fullEntry(0, blob)))
            peak = Swift.max(peak, core.mirror.stats().pendingBytes)
        }
        XCTAssertGreaterThan(core.mirror.stats().compactions, 0)
        XCTAssertLessThanOrEqual(peak, 64 * 1024)
        core.mirror.flush()
        XCTAssertEqual(log.applied.map { $0.value.count }, [4096])
    }

    // MARK: Read-your-writes

    func testAwaitingACallSeesTheChangeSetsThatArrivedBeforeItsReply() async throws {
        let frames = ManualFrameScheduler()
        let transport = FakeTransport()
        let target = storeHandle
        transport.onCall = { call, fake in
            // The core thread: the method's change-set, then its reply.
            DispatchQueue.global().async {
                let value = (try? UInt32.undraDecoded(from: Array(call.args))) ?? 0
                fake.deliverChangeSet(changeSet(fullEntry(0, u32(value), handle: target)))
                fake.replyOk(call.callId)
            }
            return true
        }
        let core = try makeCore(transport, frames: frames)
        let store = CounterProbe(core: core, handle: target)
        // No frame ever comes: only the replies can apply what the core sent.
        for value in UInt32(1) ... 200 {
            _ = try await core.call(.objectMethod(handle: target, methodId: 1), method: 1, args: u32(value))
            XCTAssertEqual(store.count, value, "after call \(value)")
        }
        XCTAssertEqual(core.mirror.pendingCount, 0)
        XCTAssertEqual(frames.pending, 1, "the frame requested by the first change-set never ran")
    }

    func testAwaitingACallAnsweredInsideSendSeesItsChangeSets() async throws {
        let transport = FakeTransport()
        let target = storeHandle
        transport.onCall = { call, fake in
            fake.deliverChangeSet(changeSet(fullEntry(0, u32(8), handle: target)))
            fake.replyOk(call.callId)
            return true
        }
        let core = try makeCore(transport, frames: ManualFrameScheduler())
        let store = CounterProbe(core: core, handle: target)
        _ = try await core.call(.objectMethod(handle: target, methodId: 1), method: 1, args: [])
        XCTAssertEqual(store.count, 8)
    }

    func testAFailedCallStillSeesTheChangeSetsThatArrivedBeforeItsReply() async throws {
        let transport = FakeTransport()
        let target = storeHandle
        transport.onCall = { call, fake in
            DispatchQueue.global().async {
                fake.deliverChangeSet(changeSet(fullEntry(0, u32(13), handle: target)))
                fake.replyError(call.callId, [0])
            }
            return true
        }
        let core = try makeCore(transport, frames: ManualFrameScheduler())
        let store = CounterProbe(core: core, handle: target)
        let error = await captureError {
            _ = try await core.call(.objectMethod(handle: target, methodId: 2), method: 2, args: [])
        }
        XCTAssertEqual((error as? UndraReplyError)?.status, .error)
        XCTAssertEqual(store.count, 13)
    }

    func testCallSyncOnTheMainThreadAppliesItsChangeSetsBeforeItReturns() throws {
        let frames = ManualFrameScheduler()
        let transport = FakeTransport()
        let target = storeHandle
        transport.onCallSync = { [unowned transport] call in
            // In process the core commits and delivers on the calling thread, before the reply.
            transport.deliverChangeSet(changeSet(fullEntry(0, u32(9), handle: target)))
            return Wire.Reply(callId: call.callId, status: .ok).encode()
        }
        let core = try makeCore(transport, frames: frames)
        let store = CounterProbe(core: core, handle: target)
        _ = try core.callSync(.objectMethod(handle: target, methodId: 1), method: 1, args: [])
        XCTAssertEqual(store.count, 9)
        XCTAssertEqual(core.mirror.pendingCount, 0)
        XCTAssertEqual(core.mirror.stats().drains, 1)
        XCTAssertEqual(frames.requests, 0, "a synchronous call on the main thread drains itself: no frame")
    }

    func testAFailedCallSyncStillAppliesItsChangeSets() throws {
        let transport = FakeTransport()
        let target = storeHandle
        transport.onCallSync = { [unowned transport] call in
            transport.deliverChangeSet(changeSet(fullEntry(0, u32(21), handle: target)))
            return Wire.Reply(callId: call.callId, status: .error, body: [1]).encode()
        }
        let core = try makeCore(transport, frames: ManualFrameScheduler())
        let store = CounterProbe(core: core, handle: target)
        XCTAssertThrowsError(try core.callSync(.objectMethod(handle: target, methodId: 1), method: 1, args: []))
        XCTAssertEqual(store.count, 21)
    }

    func testBlockingCallSyncOnTheMainThreadAppliesItsChangeSetsBeforeItReturns() throws {
        // Over a transport without an inline path the reply arrives on another thread while the
        // main thread waits, so the drain queued for it cannot run first: callSync drains itself.
        let transport = FakeTransport(directSync: false)
        let target = storeHandle
        transport.onCall = { call, fake in
            DispatchQueue.global().async {
                fake.deliverChangeSet(changeSet(fullEntry(0, u32(11), handle: target)))
                fake.replyOk(call.callId, [5])
            }
            return true
        }
        let core = try makeCore(transport, frames: ManualFrameScheduler())
        let store = CounterProbe(core: core, handle: target)
        let body = try core.callSync(.objectMethod(handle: target, methodId: 1), method: 1, args: [])
        XCTAssertEqual(body, [5])
        XCTAssertEqual(store.count, 11)
    }

    func testConstructOnTheMainThreadAppliesWhatTheConstructorChanged() throws {
        let transport = FakeTransport()
        let existing = storeHandle
        let created = UndraHandle(index: 4, generation: 2)
        transport.onCallSync = { [unowned transport] call in
            transport.deliverChangeSet(changeSet(fullEntry(0, u32(3), handle: existing)))
            return Wire.Reply(callId: call.callId, status: .ok, body: ArraySlice(created.undraEncoded())).encode()
        }
        let core = try makeCore(transport, frames: ManualFrameScheduler())
        let store = CounterProbe(core: core, handle: existing)
        let handle = try core.construct(type: 1, method: 0, args: [])
        XCTAssertEqual(handle, created)
        XCTAssertEqual(store.count, 3)
    }

    func testCallSyncFromInsideApplyIsAppliedByTheRunningDrain() throws {
        let transport = FakeTransport()
        let target = storeHandle
        transport.onCallSync = { [unowned transport] call in
            transport.deliverChangeSet(changeSet(fullEntry(1, u32(2), handle: target)))
            return Wire.Reply(callId: call.callId, status: .ok).encode()
        }
        let core = try makeCore(transport, frames: ManualFrameScheduler())
        let log = EventLog()
        core.mirror.register(target) { [core] signal, _, reader in
            let value = (try? reader.readU32()) ?? 0
            log.depth += 1
            log.maxDepth = Swift.max(log.maxDepth, log.depth)
            log.events.append("\(signal):\(value)")
            if signal == 0 {
                _ = try? core.callSync(.objectMethod(handle: target, methodId: 1), method: 1, args: [])
                // The nested drain was skipped: the change-set waits for the next round.
                log.events.append("returned")
            }
            log.depth -= 1
        }
        transport.deliverChangeSet(changeSet(fullEntry(0, u32(1), handle: target)))
        core.mirror.flush()
        XCTAssertEqual(log.events, ["0:1", "returned", "1:2"])
        XCTAssertEqual(log.maxDepth, 1)
        XCTAssertEqual(core.mirror.stats().drains, 1)
    }

    func testAChangeTheCoreMakesOnItsOwnWaitsForTheNextFrame() async throws {
        let frames = ManualFrameScheduler()
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: frames)
        let store = CounterProbe(core: core, handle: storeHandle)
        let target = storeHandle
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            DispatchQueue.global().async {
                transport.deliverChangeSet(changeSet(fullEntry(0, u32(21), handle: target)))
                continuation.resume()
            }
        }
        try await Task.sleep(nanoseconds: 20_000_000)
        XCTAssertEqual(store.count, 0, "not applied before a frame")
        XCTAssertEqual(core.mirror.pendingCount, 1)
        frames.fire()
        XCTAssertEqual(store.count, 21)
    }

    // MARK: Frame scheduling

    func testABurstFromAnotherThreadRequestsOneFrameAndDrainsOnce() async throws {
        let frames = ManualFrameScheduler()
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: frames)
        let log = ApplyLog()
        recordApplies(core, into: log)
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            DispatchQueue.global().async {
                for index in UInt32(1) ... 100 {
                    transport.deliverChangeSet(changeSet(fullEntry(index % 3, u32(index))))
                }
                continuation.resume()
            }
        }
        XCTAssertEqual(frames.requests, 1)
        XCTAssertEqual(core.mirror.pendingCount, 100)
        frames.fire()
        XCTAssertEqual(log.applied.map { $0.signal }, [1, 2, 0])
        XCTAssertEqual(log.u32Values, [100, 98, 99])
        XCTAssertEqual(core.mirror.stats().drains, 1)
        // The next change-set asks for the next frame.
        transport.deliverChangeSet(changeSet(fullEntry(0, u32(101))))
        XCTAssertEqual(frames.requests, 2)
        frames.fire()
        XCTAssertEqual(log.u32Values.last, 101)
        // A frame with nothing queued is not a drain.
        frames.fire()
        XCTAssertEqual(core.mirror.stats().drains, 2)
    }

    func testAFrameThatFindsTheQueueDrainedByAReplyDoesNothing() async throws {
        let frames = ManualFrameScheduler()
        let transport = FakeTransport()
        let target = storeHandle
        transport.onCall = { call, fake in
            fake.deliverChangeSet(changeSet(fullEntry(0, u32(4), handle: target)))
            fake.replyOk(call.callId)
            return true
        }
        let core = try makeCore(transport, frames: frames)
        let store = CounterProbe(core: core, handle: target)
        _ = try await core.call(.freeFunction(methodId: 1), method: 1, args: [])
        XCTAssertEqual(store.count, 4)
        XCTAssertEqual(frames.pending, 1)
        frames.fire()
        XCTAssertEqual(store.applies, 1)
        XCTAssertEqual(core.mirror.stats().drains, 1)
    }

    func testAChangeSetFromAnotherThreadDuringADrainWaitsForTheNextFrame() throws {
        let frames = ManualFrameScheduler()
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: frames)
        let log = ApplyLog()
        let target = storeHandle
        core.mirror.register(target) { signal, op, reader in
            log.applied.append((signal: signal, op: op, value: Array(reader.readRemaining())))
            if signal == 0 {
                // The core delivers from its own thread while the main actor is draining.
                let done = DispatchSemaphore(value: 0)
                Thread {
                    transport.deliverChangeSet(changeSet(fullEntry(1, u32(5), handle: target)))
                    done.signal()
                }.start()
                done.wait()
            }
        }
        transport.deliverChangeSet(changeSet(fullEntry(0, u32(1))))
        frames.fire()
        XCTAssertEqual(log.applied.map { $0.signal }, [0])
        XCTAssertEqual(core.mirror.pendingCount, 1)
        XCTAssertEqual(frames.pending, 1, "the drain asked for another frame")
        frames.fire()
        XCTAssertEqual(log.applied.map { $0.signal }, [0, 1])
    }

    func testADrainStopsAfterAThousandRoundsAndLeavesTheRestToTheNextFrame() throws {
        let frames = ManualFrameScheduler()
        let transport = FakeTransport()
        final class Counter: @unchecked Sendable {
            var value: UInt32 = 0
        }
        let counter = Counter()
        transport.onObserve = { observed, signal, on, fake in
            if on {
                counter.value += 1
                fake.deliverChangeSet(changeSet(fullEntry(signal, u32(counter.value), handle: observed)))
            }
        }
        let core = try makeCore(transport, frames: frames)
        let target = storeHandle
        let log = ApplyLog()
        // An apply that always causes another change to the store it observes.
        core.mirror.register(target) { [core] signal, op, reader in
            log.applied.append((signal: signal, op: op, value: Array(reader.readRemaining())))
            core.observe(target, signal: signal, on: true)
        }
        transport.deliverChangeSet(changeSet(fullEntry(0, u32(0))))
        frames.fire()
        XCTAssertEqual(log.applied.count, Mirror.maxRounds)
        XCTAssertEqual(core.mirror.pendingCount, 1)
        XCTAssertEqual(frames.pending, 1, "the rest waits for the next frame")
        XCTAssertEqual(core.mirror.stats().drains, 1)
        core.mirror.unregister(target)
        frames.fire()
        XCTAssertEqual(core.mirror.pendingCount, 0)
    }

    func testThePlatformSchedulerOnThisMachineIsTheMainActorHop() {
        #if os(macOS)
        XCTAssertTrue(makePlatformFrameScheduler() is MainActorHopScheduler)
        #endif
    }

    func testTheMainActorHopDrainsWithoutAnExplicitFlush() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: MainActorHopScheduler())
        let log = ApplyLog()
        recordApplies(core, into: log)
        transport.deliverChangeSet(changeSet(fullEntry(0, u32(1))))
        transport.deliverChangeSet(changeSet(fullEntry(0, u32(2))))
        let applied = await waitUntil { log.u32Values == [2] }
        XCTAssertTrue(applied)
        XCTAssertEqual(core.mirror.stats().drains, 1)
    }

    func testShutdownReleasesTheFrameScheduler() throws {
        let frames = ManualFrameScheduler()
        let core = try makeCore(FakeTransport(), frames: frames)
        XCTAssertFalse(frames.wasInvalidated)
        core.shutdown()
        XCTAssertTrue(frames.wasInvalidated)
    }

    // MARK: Drain listener and counters

    func testTheDrainListenerReportsEachDrainUntilRemoved() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: ManualFrameScheduler())
        recordApplies(core, into: ApplyLog())
        let drains = DrainBox()
        let registration = core.mirror.addDrainListener { stats in
            drains.reports.append(stats)
        }
        transport.deliverChangeSet(changeSet(fullEntry(0, u32(1)), fullEntry(1, u32(1))))
        transport.deliverChangeSet(changeSet(fullEntry(0, u32(2))))
        core.mirror.flush()
        XCTAssertEqual(drains.reports.count, 1)
        XCTAssertEqual(drains.reports.first?.changeSets, 2)
        XCTAssertEqual(drains.reports.first?.entries, 3)
        XCTAssertEqual(drains.reports.first?.appliedEntries, 2)
        XCTAssertGreaterThanOrEqual(drains.reports.first?.duration ?? .seconds(-1), .zero)
        registration.remove()
        registration.remove()
        transport.deliverChangeSet(changeSet(fullEntry(0, u32(3))))
        core.mirror.flush()
        XCTAssertEqual(drains.reports.count, 1)
        XCTAssertEqual(core.mirror.stats(), MirrorStats(
            changeSetsReceived: 3,
            entriesReceived: 4,
            entriesApplied: 3,
            drains: 2,
            compactions: 0,
            resyncs: 0,
            pendingEntries: 0,
            pendingBytes: 0,
            droppedEntries: 0
        ))
    }

    func testEveryListenerHearsEveryDrainAndOneCanBeRemovedFromAnotherThread() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: ManualFrameScheduler())
        recordApplies(core, into: ApplyLog())
        let first = DrainBox()
        let second = DrainBox()
        let registration = core.mirror.addDrainListener { first.reports.append($0) }
        core.mirror.addDrainListener { second.reports.append($0) }
        transport.deliverChangeSet(changeSet(fullEntry(0, u32(1))))
        core.mirror.flush()
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            DispatchQueue.global().async {
                registration.remove()
                continuation.resume()
            }
        }
        transport.deliverChangeSet(changeSet(fullEntry(0, u32(2))))
        core.mirror.flush()
        XCTAssertEqual(first.reports.count, 1)
        XCTAssertEqual(second.reports.count, 2)
        XCTAssertEqual(second.reports.map { $0.changeSets }, [1, 1])
    }

    func testTheCoresStatsCarryTheMirrorsCounters() throws {
        let transport = FakeTransport()
        let target = storeHandle
        transport.onObserve = { observed, _, on, fake in
            if on {
                fake.deliverChangeSet(changeSet(fullEntry(0, u32(1), handle: observed)))
            }
        }
        let frames = ManualFrameScheduler()
        let core = try makeCore(transport, frames: frames)
        XCTAssertEqual(core.stats().mirror, MirrorStats())
        let store = CounterProbe(core: core, handle: target)
        core.observe(target, signal: Observe.allSignals, on: true)
        XCTAssertEqual(store.count, 1)
        XCTAssertEqual(frames.requests, 0, "observe drains itself: no frame")
        let stats = core.stats().mirror
        XCTAssertEqual(stats.changeSetsReceived, 1)
        XCTAssertEqual(stats.entriesReceived, 1)
        XCTAssertEqual(stats.entriesApplied, 1)
        XCTAssertEqual(stats.drains, 1)
        XCTAssertEqual(UndraStats().mirror, MirrorStats())
    }

    func testLoadOptionsCarryTheBacklogBoundsToTheMirror() throws {
        let defaults = LoadOptions.inproc(expectedSchemaHash: 1)
        XCTAssertEqual(defaults.maxPendingEntries, 65_536)
        XCTAssertEqual(defaults.maxPendingBytes, 16 * 1024 * 1024)
        let spelled = LoadOptions(mode: .inproc, expectedSchemaHash: 1, maxPendingEntries: 4, maxPendingBytes: 1024)
        XCTAssertEqual(spelled.maxPendingEntries, 4)
        XCTAssertEqual(spelled.maxPendingBytes, 1024)
        let transport = FakeTransport()
        let core = try makeCore(transport, frames: ManualFrameScheduler(), maxPendingEntries: 4)
        for index in UInt32(0) ..< 10 {
            transport.deliverChangeSet(changeSet(fullEntry(0, u32(index))))
        }
        XCTAssertGreaterThan(core.stats().mirror.compactions, 0)
        XCTAssertLessThanOrEqual(core.stats().mirror.pendingEntries, 4)
    }
}

/// Mutable state shared with `@MainActor` closures in these tests.
@MainActor
private final class ListBox {
    var items: [String] = []
}

@MainActor
private final class DrainBox {
    var reports: [DrainStats] = []
}
