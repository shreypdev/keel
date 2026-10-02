import Foundation
import XCTest
@testable import UndraRuntime

// A model-based check of frame-coalesced delivery (ADR-031, docs/SPEC.md section 11), written by the
// review of the piece (the TypeScript twin is runtimes/ts/@undra/runtime/test/coalesce-model.test.ts).
// A model core commits transactions over two stores (one change-set per store, entries in signal-id
// order), a "wire" delivers a random prefix of what it holds, drains run at random points (a
// transaction can straddle two), the backlog bound is small (compactions run mid-history), one signal
// is `no_coalesce`, out-of-bounds patches ask for a resync, and resyncs are answered later, behind what
// the core already sent. A rare bulk patch passes both patch bounds on its own, so a compaction drops it
// and the mirror re-observes.
//
// Invariants: once the wire is empty, no resync is outstanding and a drain left nothing queued, every
// mirrored value equals the core's; between those points (without corrupt patches) a signal never
// shows a value the core never had; the `no_coalesce` signal sees a subsequence of its committed values
// (all of them while no compaction ran), ending with the last.
//
// One signal of each store is a lazy list (ADR-043; added by the types-paging review): a commit sends an
// invalidation (length, version) and now and then a restart of its page server sends a full value with a
// new handle. ADR-031's amended fold rule (a full value supersedes everything before it, an invalidation
// only earlier invalidations) must never lose the handle: reverting it fails this model.

private let modelHandles: [UndraHandle] = [UndraHandle(index: 1, generation: 1), UndraHandle(index: 2, generation: 1)]
private let u32Lists: [UInt32] = [0, 2]
private let stringList: UInt32 = 4
private let modelScalars: [UInt32] = [1, 3]
private let noCoalesceHandle = UndraHandle(index: 2, generation: 1)
private let noCoalesceSignal: UInt32 = 3
/// A `Lazy<T>` signal: its value is (page server handle, length, version).
private let lazySignal: UInt32 = 5
private let allKeys: [UInt32] = u32Lists + [stringList] + modelScalars + [lazySignal]

private struct ModelState {
    var lists: [UInt32: [UInt32]] = Dictionary(uniqueKeysWithValues: u32Lists.map { ($0, []) })
    var strings: [String] = []
    var scalars: [UInt32: UInt32] = Dictionary(uniqueKeysWithValues: modelScalars.map { ($0, 0) })
    var lazy = UndraLazyValue(handle: UndraHandle(index: 1, generation: 1), len: 0, version: 0)

    func render(_ id: UInt32) -> String {
        if id == lazySignal {
            return "\(lazy.handle.rawValue)/\(lazy.len)/\(lazy.version)"
        }
        if id == stringList {
            return "#\(strings.count)/" + strings.map { String($0.prefix(12)) }.joined(separator: ",")
        }
        if u32Lists.contains(id) {
            return "\(lists[id] ?? [])"
        }
        return "\(scalars[id] ?? 0)"
    }

    func render() -> String {
        return allKeys.map { "\($0):\(render($0))" }.joined(separator: " ")
    }
}

private func encodedPatch<T: UndraCodec>(_ ops: [PatchOp<T>]) -> [UInt8] {
    var writer = UndraWriter()
    encodePatch(ops, into: &writer)
    return writer.finish()
}

@MainActor
private final class ModelCore {
    var rng: SplitMix64
    let bulk: Bool
    let corrupt: Bool
    var truth: [UInt64: ModelState] = Dictionary(uniqueKeysWithValues: modelHandles.map { ($0.rawValue, ModelState()) })
    var wire: [[UInt8]] = []
    var resyncRequests: [(UndraHandle, UInt32)] = []
    var progressCommitted: [UInt32] = []
    var history: [String: Set<String>] = [:]
    private var txn: UInt64 = 0
    private var item: UInt32 = 1

    init(seed: UInt64, bulk: Bool, corrupt: Bool) {
        rng = SplitMix64(seed: seed)
        self.bulk = bulk
        self.corrupt = corrupt
        for h in modelHandles {
            for id in allKeys {
                remember(h, id)
            }
        }
    }

    func below(_ n: Int) -> Int {
        return Int(rng.next() % UInt64(n))
    }

    private func remember(_ h: UndraHandle, _ id: UInt32) {
        history["\(h.rawValue)/\(id)", default: []].insert(truth[h.rawValue]!.render(id))
    }

    func full(_ h: UndraHandle, _ id: UInt32) -> Wire.ChangeEntry {
        let s = truth[h.rawValue]!
        let value: [UInt8]
        if id == lazySignal {
            value = s.lazy.undraEncoded()
        } else if id == stringList {
            value = s.strings.undraEncoded()
        } else if u32Lists.contains(id) {
            value = (s.lists[id] ?? []).undraEncoded()
        } else {
            value = (s.scalars[id] ?? 0).undraEncoded()
        }
        return Wire.ChangeEntry(handle: h, signalId: id, op: .fullValue, value: ArraySlice(value))
    }

    private func nextItem() -> UInt32 {
        defer { item += 1 }
        return item
    }

    func commit() {
        txn += 1
        let stores = below(3) == 0 ? modelHandles : [modelHandles[below(modelHandles.count)]]
        for h in stores {
            var touched: [UInt32: Wire.ChangeEntry] = [:]
            for _ in 0 ..< 1 + below(3) {
                let pick = below(100)
                if pick >= 92 {
                    // The lazy list changes (an invalidation), or its page server restarts (a full value with
                    // a new handle). One entry per signal per change-set: after a restart in this transaction,
                    // a full value.
                    let restart = pick >= 97 || touched[lazySignal]?.op == .fullValue
                    var lazy = truth[h.rawValue]!.lazy
                    if pick >= 97 {
                        lazy.handle = UndraHandle(rawValue: lazy.handle.rawValue &+ 1)
                    }
                    lazy.len = UInt32(below(500))
                    lazy.version += 1
                    truth[h.rawValue]!.lazy = lazy
                    touched[lazySignal] = restart
                        ? full(h, lazySignal)
                        : Wire.ChangeEntry(
                            handle: h, signalId: lazySignal, op: .lazyListInvalidated,
                            value: ArraySlice(UndraLazyInvalidated(len: lazy.len, version: lazy.version).undraEncoded()))
                } else if pick < 30 {
                    let id = modelScalars[below(modelScalars.count)]
                    let v = nextItem()
                    truth[h.rawValue]!.scalars[id] = v
                    if h == noCoalesceHandle, id == noCoalesceSignal {
                        if touched[id] != nil {
                            progressCommitted.removeLast()
                        }
                        progressCommitted.append(v)
                    }
                    touched[id] = full(h, id)
                } else if pick < 38 {
                    let id = u32Lists[below(u32Lists.count)]
                    truth[h.rawValue]!.lists[id] = (0 ..< below(6)).map { _ in nextItem() }
                    touched[id] = full(h, id)
                } else if pick < 39, bulk {
                    touched[stringList] = bulkPatch(h, again: touched[stringList] != nil)
                } else if bulk, below(3) == 0 {
                    touched[stringList] = stringPatch(h, again: touched[stringList] != nil)
                } else {
                    let id = u32Lists[below(u32Lists.count)]
                    touched[id] = listPatch(h, id, again: touched[id] != nil)
                }
            }
            for id in touched.keys {
                remember(h, id)
            }
            let entries = touched.values.sorted { $0.signalId < $1.signalId }
            wire.append(Wire.ChangeSet(txnId: txn, entries: entries).encode())
        }
    }

    private func listPatch(_ h: UndraHandle, _ id: UInt32, again: Bool) -> Wire.ChangeEntry {
        var ops: [PatchOp<UInt32>] = []
        for _ in 0 ..< 1 + below(4) {
            var list = truth[h.rawValue]!.lists[id]!
            let n = list.count
            let op: PatchOp<UInt32>
            switch n == 0 ? 0 : below(6) {
            case 0: op = .insert(index: UInt32(below(n + 1)), item: nextItem())
            case 1: op = .remove(index: UInt32(below(n)))
            case 2: op = .update(index: UInt32(below(n)), item: nextItem())
            case 3: op = .move(from: UInt32(below(n)), to: UInt32(below(n)))
            case 4: op = below(8) == 0 ? .clear : .remove(index: UInt32(n - 1))
            default: op = .insert(index: UInt32(n), item: nextItem())
            }
            ops.append(op)
            try! applyPatch([op], to: &list)
            truth[h.rawValue]!.lists[id] = list
        }
        if again {
            return full(h, id)
        }
        if corrupt, below(30) == 0 {
            ops.insert(.remove(index: 1_000_000), at: below(ops.count + 1))
        }
        return Wire.ChangeEntry(handle: h, signalId: id, op: .keyedPatch, value: ArraySlice(encodedPatch(ops)))
    }

    private func stringPatch(_ h: UndraHandle, again: Bool) -> Wire.ChangeEntry {
        var ops: [PatchOp<String>] = []
        for _ in 0 ..< 1 + below(3) {
            var list = truth[h.rawValue]!.strings
            let n = list.count
            let op: PatchOp<String>
            switch n == 0 ? 0 : below(4) {
            case 0: op = .insert(index: UInt32(below(n + 1)), item: "s\(nextItem())")
            case 1: op = .remove(index: UInt32(below(n)))
            case 2: op = .update(index: UInt32(below(n)), item: "u\(nextItem())")
            default: op = .move(from: UInt32(below(n)), to: UInt32(below(n)))
            }
            ops.append(op)
            try! applyPatch([op], to: &list)
            truth[h.rawValue]!.strings = list
        }
        if again {
            return full(h, stringList)
        }
        return Wire.ChangeEntry(handle: h, signalId: stringList, op: .keyedPatch, value: ArraySlice(encodedPatch(ops)))
    }

    /// Clear, then 4,200 inserts of about 270 bytes: past both patch bounds on its own.
    private func bulkPatch(_ h: UndraHandle, again: Bool) -> Wire.ChangeEntry {
        let tag = nextItem()
        let filler = String(repeating: "y", count: 256)
        let items = (0 ..< Mirror.maxMergedPatchOps + 104).map { "\(tag):\($0):\(filler)" }
        truth[h.rawValue]!.strings = items
        if again {
            return full(h, stringList)
        }
        var ops: [PatchOp<String>] = [.clear]
        for (i, s) in items.enumerated() {
            ops.append(.insert(index: UInt32(i), item: s))
        }
        return Wire.ChangeEntry(handle: h, signalId: stringList, op: .keyedPatch, value: ArraySlice(encodedPatch(ops)))
    }

    func answerResyncs(_ max: Int) {
        var n = 0
        while n < max, !resyncRequests.isEmpty {
            let (h, id) = resyncRequests.removeFirst()
            txn += 1
            wire.append(Wire.ChangeSet(txnId: txn, entries: [full(h, id)]).encode())
            n += 1
        }
    }
}

@MainActor
private final class ModelHost {
    let core: ModelCore
    var state: [UInt64: ModelState] = Dictionary(uniqueKeysWithValues: modelHandles.map { ($0.rawValue, ModelState()) })
    var progressSeen: [UInt32] = []

    init(core: ModelCore) {
        self.core = core
    }

    func apply(_ h: UndraHandle, _ id: UInt32, _ op: ChangeOp, _ reader: inout UndraReader) {
        let key = h.rawValue
        if id == lazySignal {
            if op == .fullValue {
                state[key]!.lazy = try! UndraLazyValue.undraDecode(&reader)
            } else {
                // An invalidation is relative to the page server a full value named: it keeps the handle.
                let v = try! UndraLazyInvalidated.undraDecode(&reader)
                state[key]!.lazy.len = v.len
                state[key]!.lazy.version = v.version
            }
        } else if id == stringList {
            if op == .fullValue {
                state[key]!.strings = try! [String].undraDecode(&reader)
            } else {
                let ops: [PatchOp<String>] = try! decodePatch(&reader)
                var list = state[key]!.strings
                do {
                    try applyPatch(ops, to: &list)
                    state[key]!.strings = list
                } catch {
                    core.resyncRequests.append((h, id))
                }
            }
        } else if u32Lists.contains(id) {
            if op == .fullValue {
                state[key]!.lists[id] = try! [UInt32].undraDecode(&reader)
            } else {
                let ops: [PatchOp<UInt32>] = try! decodePatch(&reader)
                var list = state[key]!.lists[id]!
                do {
                    try applyPatch(ops, to: &list)
                    state[key]!.lists[id] = list
                } catch {
                    // What generated code does: re-observe (the core answers later).
                    core.resyncRequests.append((h, id))
                }
            }
        } else {
            let v = try! UInt32.undraDecode(&reader)
            state[key]!.scalars[id] = v
            if h == noCoalesceHandle, id == noCoalesceSignal {
                progressSeen.append(v)
            }
        }
    }
}

/// Frames that never come: only explicit drains apply anything.
private final class NoFrames: FrameScheduler, @unchecked Sendable {
    func requestFrame(_ tick: @escaping @MainActor @Sendable () -> Void) {}
    func invalidate() {}
}

private func isSubsequence(_ sub: [UInt32], of all: [UInt32]) -> Bool {
    var at = 0
    for v in sub {
        while at < all.count, all[at] != v {
            at += 1
        }
        if at == all.count {
            return false
        }
        at += 1
    }
    return true
}

@MainActor
final class CoalesceModelTests: XCTestCase {
    /// Returns (compactions, settled checks, resyncs), or nil after the first failure.
    private func runHistory(seed: UInt64, bulk: Bool) -> (compactions: Int, checks: Int, resyncs: Int)? {
        var pre = SplitMix64(seed: seed &* 7919)
        let corrupt = pre.next() % 2 == 0
        let small = pre.next() % 2 == 0
        let core = ModelCore(seed: seed, bulk: bulk, corrupt: corrupt)
        let host = ModelHost(core: core)
        let mirror = Mirror(
            maxPendingEntries: small ? 2 + Int(pre.next() % 24) : Mirror.defaultMaxPendingEntries,
            maxPendingBytes: small ? 64 + Int(pre.next() % 4096) : Mirror.defaultMaxPendingBytes,
            scheduler: NoFrames()
        )
        mirror.setResyncHandler { h, id in
            core.resyncRequests.append((h, id))
        }
        for h in modelHandles {
            mirror.register(h, noCoalesce: h == noCoalesceHandle ? [noCoalesceSignal] : []) { id, op, reader in
                host.apply(h, id, op, &reader)
            }
        }
        func deliver(_ n: Int) {
            var i = 0
            while i < n, !core.wire.isEmpty {
                mirror.enqueue(core.wire.removeFirst())
                i += 1
            }
        }
        var checks = 0
        func settle(_ context: String) -> Bool {
            for _ in 0 ..< 100 {
                core.answerResyncs(Int.max)
                deliver(Int.max)
                mirror.flush()
                if core.wire.isEmpty, core.resyncRequests.isEmpty, mirror.pendingCount == 0 {
                    break
                }
            }
            checks += 1
            for h in modelHandles {
                let want = core.truth[h.rawValue]!.render()
                let got = host.state[h.rawValue]!.render()
                if want != got {
                    XCTFail("\(context) handle \(h.rawValue): \(got.prefix(300)) != \(want.prefix(300))")
                    return false
                }
            }
            return true
        }
        let steps = 20 + core.below(120)
        for step in 0 ..< steps {
            let pick = core.below(100)
            if pick < 55 {
                core.commit()
            } else if pick < 75 {
                deliver(1 + core.below(6))
            } else if pick < 87 {
                mirror.flush()
                if !corrupt {
                    for h in modelHandles {
                        for id in allKeys {
                            let shown = host.state[h.rawValue]!.render(id)
                            if core.history["\(h.rawValue)/\(id)"]?.contains(shown) != true {
                                XCTFail("seed \(seed) step \(step): \(h.rawValue)/\(id) shows \(shown.prefix(80))")
                                return nil
                            }
                        }
                    }
                }
            } else if pick < 95 {
                core.answerResyncs(1 + core.below(2))
            } else if !settle("seed \(seed) step \(step)") {
                return nil
            }
        }
        if !settle("seed \(seed) end") {
            return nil
        }
        let stats = mirror.stats()
        let committed = core.progressCommitted
        if let last = committed.last {
            XCTAssertEqual(host.progressSeen.last, last, "seed \(seed): the no_coalesce signal ends on the last value")
            XCTAssertTrue(isSubsequence(host.progressSeen, of: committed), "seed \(seed): no_coalesce values in commit order")
            if stats.compactions == 0 {
                XCTAssertEqual(host.progressSeen, committed, "seed \(seed): every no_coalesce value applied")
            }
        }
        return (stats.compactions, checks, stats.resyncs)
    }

    func testReviewRandomHistoriesOverTwoStoresWithPartialDeliveryRandomDrainsSmallBoundsAndAsyncResyncs() {
        let seeds = UInt64(ProcessInfo.processInfo.environment["UNDRA_MODEL_SEEDS"].flatMap { Int($0) } ?? 1000)
        var compactions = 0
        var checks = 0
        for seed in 1 ... seeds {
            guard let r = runHistory(seed: seed, bulk: false) else {
                return
            }
            compactions += r.compactions
            checks += r.checks
        }
        XCTAssertGreaterThan(compactions, 100)
        XCTAssertGreaterThan(checks, Int(seeds))
    }

    func testReviewHistoriesWithPatchesPastBothBoundsAreDroppedAndReobserved() {
        var resyncs = 0
        for seed in UInt64(10_001) ... 10_030 {
            guard let r = runHistory(seed: seed, bulk: true) else {
                return
            }
            resyncs += r.resyncs
        }
        XCTAssertGreaterThan(resyncs, 0, "no merged patch was dropped")
    }
}
