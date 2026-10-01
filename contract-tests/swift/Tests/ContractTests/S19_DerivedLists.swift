import Foundation
import UndraRuntime
import PlaygroundCore
import XCTest

extension ContractScenarios {
    // MARK: S19

    /// `Todos.visible` is a derived list (ADR-039): it reaches the platform as keyed patches, never as
    /// a whole list after the first; and 60,000 recorded operations over three derived views replay
    /// through this runtime's decoder and applier to exactly the views the core computed.
    func testS19_derivedKeyedList() async {
        await scenario("S19", "derived keyed list") {
            let core = try self.core
            let ids = UndraIds.Objects.Todos.self
            let raw = try RawStore(core: core, type: ids.typeId, method: ids.new)
            defer { raw.close() }
            var host: [Todo] = []

            /// Runs `call`, then returns the single `visible` entry it delivered (and every entry).
            @MainActor func visible(_ what: String, _ call: () async throws -> Void) async throws -> (RawStore.Entry, [RawStore.Entry]) {
                raw.clear()
                try await call()
                try await waitUntil("the change-set of \(what)") { !raw.entries(of: 2).isEmpty }
                let entries = raw.entries
                let visible = raw.entries(of: 2)
                try checkEqual(visible.count, 1, "visible entries of \(what)")
                return (visible[0], entries)
            }
            @MainActor func ops(_ entry: RawStore.Entry, _ what: String) throws -> [PatchOp<Todo>] {
                try check(entry.op == .keyedPatch, "the visible entry of \(what) is a keyed patch")
                return try entry.decodePatch(Todo.self)
            }
            @MainActor func apply(_ entry: RawStore.Entry) throws {
                if entry.op == .fullValue {
                    host = try entry.decode([Todo].self)
                } else {
                    try applyPatch(try entry.decodePatch(Todo.self), to: &host)
                }
            }
            @MainActor func add(_ title: String) async throws -> Todo {
                try decoded(Todo.self, try await raw.call(ids.add, encoded { (w: inout UndraWriter) in w.writeString(title) }))
            }
            @MainActor func toggle(_ todo: Todo) throws {
                try raw.callSync(ids.toggle, encoded { (w: inout UndraWriter) in todo.id.undraEncode(&w) })
            }
            @MainActor func setFilter(_ filter: Filter) throws {
                try raw.callSync(ids.setFilter, encoded { (w: inout UndraWriter) in filter.undraEncode(&w) })
            }
            @MainActor func remaining(_ entries: [RawStore.Entry]) throws -> UInt32 {
                try entries.last { $0.signal == 3 }.map { try $0.decode(UInt32.self) } ?? 0
            }

            // 1. The initial visible entry is a full value, [].
            raw.observe()
            let initial = raw.entries(of: 2)
            try checkEqual(initial.count, 1, "initial visible entries")
            try check(initial[0].op == .fullValue, "the initial visible entry is a full value")
            try apply(initial[0])
            try checkEqual(host, [], "the initial visible")

            // 2. add a, b, c: one Insert at 0, 1, 2 each; remaining 1, 2, 3.
            var todos: [Todo] = []
            for (index, title) in ["a", "b", "c"].enumerated() {
                var added: Todo?
                let (entry, entries) = try await visible("add(\(title))") { added = try await add(title) }
                let todo = try XCTUnwrap(added)
                todos.append(todo)
                try checkEqual(try ops(entry, "add(\(title))"), [.insert(index: UInt32(index), item: todo)], "add(\(title))")
                try apply(entry)
                try checkEqual(try remaining(entries), UInt32(index + 1), "remaining after add(\(title))")
            }
            let (a, b) = (todos[0], todos[1])
            var doneB = b
            doneB.done = true

            // 3. toggle(b) under All: one Update at 1; remaining 2.
            var (entry, entries) = try await visible("toggle(b)") { try toggle(b) }
            try checkEqual(try ops(entry, "toggle(b)"), [.update(index: 1, item: doneB)], "toggle(b)")
            try apply(entry)
            try checkEqual(try remaining(entries), 2, "remaining after toggle(b)")

            // 4. set_filter(Active): one Remove at 1; set_filter(All): one Insert of b at 1.
            (entry, _) = try await visible("set_filter(Active)") { try setFilter(.active) }
            try checkEqual(try ops(entry, "set_filter(Active)"), [.remove(index: 1)], "set_filter(Active)")
            try apply(entry)
            (entry, _) = try await visible("set_filter(All)") { try setFilter(.all) }
            try checkEqual(try ops(entry, "set_filter(All)"), [.insert(index: 1, item: doneB)], "set_filter(All)")
            try apply(entry)

            // 5. Under Active, toggle(a): one Remove at 0; again: one Insert of a at 0.
            try apply(try await visible("set_filter(Active)") { try setFilter(.active) }.0)
            (entry, _) = try await visible("toggle(a)") { try toggle(a) }
            try checkEqual(try ops(entry, "toggle(a)"), [.remove(index: 0)], "toggle(a) under Active")
            try apply(entry)
            (entry, _) = try await visible("toggle(a) again") { try toggle(a) }
            try checkEqual(try ops(entry, "toggle(a) again"), [.insert(index: 0, item: a)], "toggle(a) again")
            try apply(entry)
            try apply(try await visible("set_filter(All)") { try setFilter(.all) }.0)

            // 6. fill(10000), then a toggle of a visible item: one op, under 100 bytes.
            (entry, _) = try await visible("fill(10000)") {
                try raw.callSync(ids.fill, encoded { (w: inout UndraWriter) in w.writeU32(10_000) })
            }
            try apply(entry)
            try checkEqual(host.count, 10_003, "visible after fill(10000)")
            let target = host[5_000]
            try check(!target.done, "the target is open")
            var doneTarget = target
            doneTarget.done = true
            (entry, _) = try await visible("a toggle in 10,003 rows") { try toggle(target) }
            try checkEqual(try ops(entry, "a toggle in 10,003 rows"), [.update(index: 5_000, item: doneTarget)], "the toggle in 10,003 rows")
            try check(entry.value.count < 100, "one op, not the view: \(entry.value.count) bytes")
            try apply(entry)

            // 7. Through the generated class, visible equals the model after every step (on the main
            //    actor a synchronous call drains the mirror before it returns: read-your-writes).
            let store = try Todos(ctx: core)
            defer { store.close() }
            var model: [Todo] = []
            var filter = Filter.all
            @MainActor func checkModel(_ what: String) throws {
                let shown = model.filter { filter == .all || (filter == .active) != $0.done }
                try check(store.visible == shown, "\(what): visible equals the model")
                try checkEqual(store.remaining, UInt32(model.filter { !$0.done }.count), "\(what): remaining")
            }
            for title in ["a", "b", "c"] {
                model.append(try await store.add(title: title))
            }
            store.setFilter(.all)
            try checkModel("add a, b, c")
            @MainActor func flip(_ i: Int) throws {
                store.toggle(id: model[i].id)
                model[i].done.toggle()
                try checkModel("toggle(\(i))")
            }
            try flip(1)
            for next in [Filter.active, .all, .active] {
                store.setFilter(next)
                filter = next
                try checkModel("set_filter(\(next))")
            }
            try flip(0)
            try flip(0)
            store.setFilter(.all)
            filter = .all
            try checkModel("set_filter(All)")
            store.fill(count: 10_000)
            for n in 0 ..< 10_000 {
                model.append(Todo(id: idOf(UInt64(4 + n)), title: "Item \(n + 1)", done: n % 4 == 3))
            }
            try checkModel("fill(10000)")
            try flip(5_003)
            store.remove(id: model[0].id)
            model.removeFirst()
            try checkModel("remove")
            store.clearDone()
            model.removeAll { $0.done }
            try checkModel("clear_done")

            // 8. Reads never cross: 1,000 reads of visible leave crossings.calls unchanged.
            try await quietFor(milliseconds: 50)
            let crossings = core.stat("crossings.calls")
            var sink = 0
            for _ in 0 ..< 1_000 {
                sink += store.visible.count
            }
            try check(sink > 0, "the reads were performed")
            try checkEqual(core.stat("crossings.calls"), crossings, "crossings.calls after 1,000 reads of visible")

            // 9. 60,000 recorded operations over three views replay, change-set by change-set.
            try replayDerivedVectors()
        }
    }
}

/// The `n`th identity of a playground `Todos` store: `n` in the first eight bytes, big-endian.
private func idOf(_ n: UInt64) -> UUID {
    var bytes = [UInt8](repeating: 0, count: 16)
    for i in 0 ..< 8 {
        bytes[i] = UInt8(truncatingIfNeeded: n >> (56 - 8 * UInt64(i)))
    }
    return UUID(uuid: (bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
                       bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]))
}

private struct VectorRow: UndraCodec, Equatable {
    var id: UInt32
    var title: String
    var done: Bool
    var rank: UInt32

    static func undraDecode(_ r: inout UndraReader) throws -> VectorRow {
        VectorRow(id: try r.readU32(), title: try r.readString(), done: try r.readBool(), rank: try r.readU32())
    }

    func undraEncode(_ w: inout UndraWriter) {
        w.writeU32(id)
        w.writeString(title)
        w.writeBool(done)
        w.writeU32(rank)
    }
}

private struct VectorLabel: UndraCodec, Equatable {
    var id: UInt32
    var text: String

    static func undraDecode(_ r: inout UndraReader) throws -> VectorLabel {
        VectorLabel(id: try r.readU32(), text: try r.readString())
    }

    func undraEncode(_ w: inout UndraWriter) {
        w.writeU32(id)
        w.writeString(text)
    }
}

/// FNV-1a 64 of `bytes`: what the recording hashes each view's encoding with.
private func fnv1a64(_ bytes: [UInt8]) -> UInt64 {
    var hash: UInt64 = 0xcbf2_9ce4_8422_2325
    for byte in bytes {
        hash = (hash ^ UInt64(byte)) &* 0x0000_0100_0000_01b3
    }
    return hash
}

private func hashOf<T: UndraCodec>(_ list: [T]) -> UInt64 {
    var w = UndraWriter()
    list.undraEncode(&w)
    return fnv1a64(w.finish())
}

/// One view of the recording, applied with this runtime's decoder and applier.
private func apply<T: UndraCodec>(_ list: inout [T], _ op: ChangeOp, _ reader: inout UndraReader) throws -> Bool {
    if op == .fullValue {
        list = try [T].undraDecode(&reader)
        return false
    }
    let ops: [PatchOp<T>] = try decodePatch(&reader)
    try applyPatch(ops, to: &list)
    return true
}

/// Reads the `UDV1` recording (contract-tests/derived-vectors.sh) and checks every view after every change-set.
private func replayDerivedVectors() throws {
    guard let path = ProcessInfo.processInfo.environment["UNDRA_DERIVED_VECTORS"] else {
        throw ScenarioFailure(description: "UNDRA_DERIVED_VECTORS is not set (contract-tests/swift/run.sh sets it after writing the recording)")
    }
    guard let data = FileManager.default.contents(atPath: path) else {
        throw ScenarioFailure(description: "no derived-list recording at \(path) (contract-tests/derived-vectors.sh writes it)")
    }
    var r = UndraReader([UInt8](data))
    let magic = String(decoding: [try r.readU8(), try r.readU8(), try r.readU8(), try r.readU8()], as: UTF8.self)
    try checkEqual(magic, "UDV1", "the recording's magic")
    let views = Int(try r.readU32())
    let records = Int(try r.readU32())
    try check(records > 20_000, "the recording has \(records) change-sets")
    var open: [VectorRow] = []
    var ranked: [VectorRow] = []
    var labels: [VectorLabel] = []
    var patches = 0
    for i in 0 ..< records {
        let length = Int(try r.readU32())
        var payload: [UInt8] = []
        payload.reserveCapacity(length)
        for _ in 0 ..< length {
            payload.append(try r.readU8())
        }
        try Wire.ChangeSet.forEachEntry(slice: payload[...]) { _, signal, op, reader in
            let patched: Bool
            switch signal {
            case 0: patched = try apply(&open, op, &reader)
            case 1: patched = try apply(&ranked, op, &reader)
            default: patched = try apply(&labels, op, &reader)
            }
            if patched { patches += 1 }
        }
        let hashes = [hashOf(open), hashOf(ranked), hashOf(labels)]
        for v in 0 ..< views {
            let expected = try r.readU64()
            if hashes[v] != expected {
                throw ScenarioFailure(description: "view \(v) differs from the core's after change-set \(i)")
            }
        }
    }
    try r.finish()
    try check(patches > 50_000, "only \(patches) patches were replayed")
}
