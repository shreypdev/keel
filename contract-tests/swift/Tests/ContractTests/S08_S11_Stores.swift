import Foundation
import UndraRuntime
import PlaygroundCore
import XCTest

extension ContractScenarios {
    // MARK: S08

    func testS08_storeObserveInitialChangeSet() async {
        await scenario("S08", "store observe: initial change-set") {
            let core = try self.core

            // 1. Raw: observing delivers the current values before `observe` returns.
            let raw = try RawStore(core: core, type: UndraIds.Objects.Todos.typeId, method: UndraIds.Objects.Todos.new)
            defer { raw.close() }
            try checkEqual(raw.entries.count, 0, "entries before observe")
            raw.observe()
            try checkEqual(raw.entries.count, 4, "entries when observe returned")
            try checkEqual(raw.entries.map(\.signal).sorted(), [0, 1, 2, 3], "signals of the initial change-set")
            try check(raw.entries.allSatisfy { $0.op == .fullValue }, "every initial entry is a full value")
            try checkEqual(try raw.entries(of: 0)[0].decode([Todo].self), [], "todos")
            try checkEqual(try raw.entries(of: 1)[0].decode(Filter.self), .all, "filter")
            try checkEqual(try raw.entries(of: 2)[0].decode([Todo].self), [], "visible")
            try checkEqual(try raw.entries(of: 3)[0].decode(UInt32.self), 0, "remaining")

            // 2. Through the generated class, with no await in between.
            let todos = try Todos(ctx: core)
            defer { todos.close() }
            try checkEqual(todos.todos, [], "Todos.todos right after init")
            try checkEqual(todos.filter, .all, "Todos.filter right after init")
            try checkEqual(todos.visible, [], "Todos.visible right after init")
            try checkEqual(todos.remaining, 0, "Todos.remaining right after init")

            // 3. The other stores.
            let counter = try Counter(ctx: core)
            defer { counter.close() }
            try checkEqual(counter.count, 0, "Counter.count")
            try checkEqual(counter.changes, 0, "Counter.changes")
            try checkEqual(counter.parity, .even, "Counter.parity")
            let list = try BigList(ctx: core)
            defer { list.close() }
            try checkEqual(list.items.count, 10_000, "BigList.items.count")
            try checkEqual(list.items[0], Item(id: 1, label: "Item 1", version: 0), "BigList.items[0]")
            try checkEqual(list.items[9_999], Item(id: 10_000, label: "Item 10000", version: 0), "BigList.items[9999]")
            try checkEqual(list.count, 10_000, "BigList.count")

            // 4. Observation off: a write delivers nothing; observing again delivers the current values once.
            raw.stopObserving()
            raw.clear()
            let added: Todo = try decoded(Todo.self, try await raw.call(UndraIds.Objects.Todos.add, encoded { (w: inout UndraWriter) in w.writeString("x") }))
            try checkEqual(added.title, "x", "the added item")
            try await quietFor(milliseconds: 200)
            try checkEqual(raw.entries.count, 0, "entries delivered while not observing")
            raw.observe()
            try checkEqual(raw.entries.count, 4, "entries of the second observe")
            let current = try raw.entries(of: 0)[0].decode([Todo].self)
            try checkEqual(current.map(\.title), ["x"], "todos delivered by the second observe")
            try checkEqual(try raw.entries(of: 3)[0].decode(UInt32.self), 1, "remaining delivered by the second observe")

            // 5. Releasing a store drops the live handles by one.
            let before = core.stat("live_handles")
            let released = try Todos(ctx: core)
            try checkEqual(core.stat("live_handles"), before + 1, "live_handles after creating a store")
            released.close()
            try await waitUntil("live_handles to drop") { core.stat("live_handles") == before }
        }
    }

    // MARK: S09

    func testS09_transactionIsASingleChangeSet() async {
        await scenario("S09", "transaction: a single change-set") {
            let core = try self.core
            let counterType = UndraIds.Objects.Counter.self
            let counter = try RawStore(core: core, type: counterType.typeId, method: counterType.new)
            defer { counter.close() }
            counter.observe()
            counter.clear()

            /// The count, changes and parity a change-set of the counter carried.
            @MainActor func readings() throws -> (count: Int32, changes: UInt32, parity: Parity) {
                return (
                    try counter.entries(of: 0)[0].decode(Int32.self),
                    try counter.entries(of: 1)[0].decode(UInt32.self),
                    try counter.entries(of: 2)[0].decode(Parity.self)
                )
            }

            // 1. `add(5)` is one transaction: three entries, one change-set.
            let transactions = core.stat("transactions")
            let changeSets = core.stat("crossings.change_sets")
            try counter.callSync(counterType.add, encoded { (w: inout UndraWriter) in w.writeI32(5) })
            try await waitUntil("the entries of add(5)") { counter.entries.count >= 3 }
            try await quietFor(milliseconds: 50)
            try checkEqual(counter.entries.count, 3, "entries of add(5)")
            let added = try readings()
            try checkEqual(added.count, 5, "count after add(5)")
            try checkEqual(added.changes, 1, "changes after add(5)")
            try checkEqual(added.parity, .odd, "parity after add(5)")
            try checkEqual(core.stat("transactions") - transactions, 1, "transactions for add(5)")
            try checkEqual(core.stat("crossings.change_sets") - changeSets, 1, "change_sets for add(5)")

            // 2. Three separate calls are three transactions.
            let beforeThree = core.stat("transactions")
            try counter.callSync(counterType.increment)
            try counter.callSync(counterType.increment)
            try counter.callSync(counterType.decrement)
            try await waitUntil("the entries of three calls") { counter.entries.count >= 3 + 9 }
            try checkEqual(core.stat("transactions") - beforeThree, 3, "transactions for increment, increment, decrement")

            // 3. `reset()` writes two signals in one transaction: one change-set.
            counter.clear()
            let beforeReset = core.stat("transactions")
            try counter.callSync(counterType.reset)
            try await waitUntil("the entries of reset") { counter.entries.count >= 3 }
            try await quietFor(milliseconds: 50)
            try checkEqual(core.stat("transactions") - beforeReset, 1, "transactions for reset")
            try checkEqual(counter.entries.count, 3, "entries of reset")
            let reset = try readings()
            try checkEqual(reset.count, 0, "count after reset")
            try checkEqual(reset.changes, 0, "changes after reset")
            try checkEqual(reset.parity, .even, "parity after reset")

            // 4. A hundred dirty signals are one change-set of a hundred entries.
            let bench = try RawStore(core: core, type: UndraIds.Objects.Bench.typeId, method: UndraIds.Objects.Bench.new)
            defer { bench.close() }
            bench.observe()
            try checkEqual(bench.entries.count, 129, "entries of the bench store's initial change-set (list + 128 counters)")
            @MainActor func touch(_ count: UInt32) throws {
                try bench.callSync(UndraIds.Objects.Bench.benchTouchSignals, encoded { (w: inout UndraWriter) in w.writeU32(count) })
            }
            bench.clear()
            let beforeHundred = core.stat("transactions")
            try touch(100)
            try await waitUntil("a hundred entries") { bench.entries.count >= 100 }
            try await quietFor(milliseconds: 50)
            try checkEqual(bench.entries.count, 100, "entries of bench_touch_signals(100)")
            try checkEqual(core.stat("transactions") - beforeHundred, 1, "transactions for bench_touch_signals(100)")
            try checkEqual(bench.entries.map(\.signal), Array(1 ... 100), "signals of bench_touch_signals(100)")
            try check(bench.entries.allSatisfy { $0.op == .fullValue }, "every touched counter is a full value")
            for entry in bench.entries {
                try checkEqual(try entry.decode(UInt32.self), 1, "value of signal \(entry.signal)")
            }
            bench.clear()
            try touch(1)
            try await waitUntil("one entry") { bench.entries.count >= 1 }
            try await quietFor(milliseconds: 50)
            try checkEqual(bench.entries.count, 1, "entries of bench_touch_signals(1)")
            bench.clear()
            try touch(1000)
            try await waitUntil("all 128 entries") { bench.entries.count >= 128 }
            try await quietFor(milliseconds: 50)
            try checkEqual(bench.entries.count, 128, "entries of bench_touch_signals(1000)")

            // 5. Nothing observes: nothing is delivered. Observing afterwards delivers the current values once.
            let quiet = try RawStore(core: core, type: counterType.typeId, method: counterType.new)
            defer { quiet.close() }
            let beforeQuiet = core.stat("transactions")
            try quiet.callSync(counterType.add, encoded { (w: inout UndraWriter) in w.writeI32(1) })
            try await quietFor(milliseconds: 100)
            try checkEqual(core.stat("transactions") - beforeQuiet, 0, "transactions of a write nobody observes")
            try checkEqual(quiet.entries.count, 0, "entries of a write nobody observes")
            quiet.observe()
            try checkEqual(quiet.entries.count, 3, "entries delivered when observing afterwards")
            try checkEqual(try quiet.entries(of: 0)[0].decode(Int32.self), 1, "count delivered when observing afterwards")
        }
    }

    // MARK: S10

    func testS10_keyedPatch() async {
        await scenario("S10", "keyed patch") {
            let core = try self.core
            let type = UndraIds.Objects.BigList.self

            // 1. The initial entry for `items` is a full value of 10,000 items.
            let store = try RawStore(core: core, type: type.typeId, method: type.new)
            defer { store.close() }
            store.observe()
            let initial = store.entries(of: 0)
            try checkEqual(initial.count, 1, "initial entries for items")
            try checkEqual(initial[0].op, .fullValue, "form of the initial items entry")
            var mirrored = try initial[0].decode([Item].self)
            try checkEqual(mirrored.count, 10_000, "initial items")
            var model = mirrored

            /// Waits for the next change-set of the list, checks it is one keyed patch of exactly
            /// one operation, applies it to the mirror's copy and compares that copy, in full, with the model.
            @MainActor func expectPatch(_ what: String, _ expected: PatchOp<Item>, then mutate: (inout [Item]) -> Void) async throws -> RawStore.Entry {
                try await waitUntil("the change-set of \(what)") { !store.entries(of: 0).isEmpty }
                let entries = store.entries(of: 0)
                try checkEqual(entries.count, 1, "items entries after \(what)")
                let entry = entries[0]
                try checkEqual(entry.op, .keyedPatch, "form of the items entry after \(what)")
                let ops: [PatchOp<Item>] = try entry.decodePatch(Item.self)
                try checkEqual(ops, [expected], "operations after \(what)")
                try applyPatch(ops, to: &mirrored)
                mutate(&model)
                try check(mirrored == model, "the mirror diverged from the model after \(what)")
                return entry
            }

            // 2. An insert is one operation, and small.
            store.clear()
            let inserted = try decoded(UInt32.self, try store.callSync(type.insertAt, encoded { (w: inout UndraWriter) in
                w.writeU32(5000)
                w.writeString("fresh")
            }))
            try checkEqual(inserted, 10_001, "insert_at(5000, \"fresh\")")
            let fresh = Item(id: 10_001, label: "fresh", version: 0)
            let insert = try await expectPatch("insert_at", .insert(index: 5000, item: fresh)) { $0.insert(fresh, at: 5000) }
            try check(insert.value.count < 100, "the insert patch is \(insert.value.count) bytes (expected < 100)")
            try await waitUntil("the count entry") { !store.entries(of: 1).isEmpty }
            try checkEqual(try store.entries(of: 1)[0].decode(UInt32.self), 10_001, "count after the insert")

            // 3. An update.
            store.clear()
            try store.callSync(type.updateAt, encoded { (w: inout UndraWriter) in
                w.writeU32(42)
                w.writeString("renamed")
            })
            let renamed = Item(id: 43, label: "renamed", version: 1)
            _ = try await expectPatch("update_at", .update(index: 42, item: renamed)) { $0[42] = renamed }

            // 4. A move.
            store.clear()
            try store.callSync(type.moveItem, encoded { (w: inout UndraWriter) in
                w.writeU32(10)
                w.writeU32(9000)
            })
            _ = try await expectPatch("move_item", .move(from: 10, to: 9000)) { (list: inout [Item]) -> Void in
                let moved = list.remove(at: 10)
                list.insert(moved, at: 9000)
            }

            // 5. A removal.
            store.clear()
            try store.callSync(type.removeAt, encoded { (w: inout UndraWriter) in w.writeU32(0) })
            _ = try await expectPatch("remove_at", .remove(index: 0)) { $0.remove(at: 0) }

            // 7. The bench list takes the same path: one insert into 10,000 rows.
            let bench = try RawStore(core: core, type: UndraIds.Objects.Bench.typeId, method: UndraIds.Objects.Bench.new)
            defer { bench.close() }
            bench.observe()
            try checkEqual(try bench.entries(of: 0)[0].decode([Item].self).count, 10_000, "the bench list's rows")
            bench.clear()
            try bench.callSync(UndraIds.Objects.Bench.benchListInsert, encoded { (w: inout UndraWriter) in w.writeU32(123) })
            try await waitUntil("the bench insert") { !bench.entries(of: 0).isEmpty }
            let benchEntry = bench.entries(of: 0)[0]
            try checkEqual(benchEntry.op, .keyedPatch, "form of the bench insert")
            let benchOps: [PatchOp<Item>] = try benchEntry.decodePatch(Item.self)
            try checkEqual(benchOps, [.insert(index: 123, item: Item(id: 10_001, label: "Item 10001", version: 0))], "bench_list_insert(123)")

            // 8. A reset after removing the first item is one insert at the front.
            let fresher = try RawStore(core: core, type: type.typeId, method: type.new)
            defer { fresher.close() }
            fresher.observe()
            fresher.clear()
            try fresher.callSync(type.removeAt, encoded { (w: inout UndraWriter) in w.writeU32(0) })
            try await waitUntil("the removal") { !fresher.entries(of: 0).isEmpty }
            fresher.clear()
            try fresher.callSync(type.reset)
            try await waitUntil("the reset") { !fresher.entries(of: 0).isEmpty }
            let resetEntry = fresher.entries(of: 0)[0]
            try checkEqual(resetEntry.op, .keyedPatch, "form of the reset after a removal")
            let resetOps: [PatchOp<Item>] = try resetEntry.decodePatch(Item.self)
            try checkEqual(resetOps, [.insert(index: 0, item: Item(id: 1, label: "Item 1", version: 0))], "reset() after remove_at(0)")
        }
    }

    // MARK: S11

    func testS11_computed() async {
        await scenario("S11", "computed") {
            let core = try self.core
            let todos = try Todos(ctx: core)
            defer { todos.close() }

            // 1. Three items, one finished.
            let a = try await todos.add(title: "a")
            let b = try await todos.add(title: "b")
            let c = try await todos.add(title: "c")
            try check(a.id != b.id && b.id != c.id && a.id != c.id, "ids are distinct")
            todos.toggle(id: b.id)
            try await waitUntil("the list to show the toggle") { todos.remaining == 2 && todos.todos.count == 3 }
            try checkEqual(todos.visible.map(\.title), ["a", "b", "c"], "visible")
            try checkEqual(todos.visible.map(\.done), [false, true, false], "done flags")

            // 2. One change-set carries the filter and the recomputed `visible`; `remaining` does not change.
            let raw = try RawStore(core: core, type: UndraIds.Objects.Todos.typeId, method: UndraIds.Objects.Todos.new)
            defer { raw.close() }
            raw.observe()
            var rawTodos: [Todo] = []
            for title in ["a", "b", "c"] {
                let todo: Todo = try decoded(Todo.self, try await raw.call(UndraIds.Objects.Todos.add, encoded { (w: inout UndraWriter) in w.writeString(title) }))
                rawTodos.append(todo)
            }
            try raw.callSync(UndraIds.Objects.Todos.toggle, encoded { (w: inout UndraWriter) in rawTodos[1].id.undraEncode(&w) })
            rawTodos[1].done = true
            try await waitUntil("the raw store to settle") { raw.entries(of: 3).last.flatMap { try? $0.decode(UInt32.self) } == 2 }
            raw.clear()
            let transactions = core.stat("transactions")
            try raw.callSync(UndraIds.Objects.Todos.setFilter, encoded { (w: inout UndraWriter) in Filter.done.undraEncode(&w) })
            try await waitUntil("the change-set of set_filter(Done)") { raw.entries.count >= 2 }
            try await quietFor(milliseconds: 50)
            try checkEqual(core.stat("transactions") - transactions, 1, "change-sets for set_filter(Done)")
            try checkEqual(raw.entries.map(\.signal).sorted(), [1, 2], "signals of set_filter(Done): filter and visible only")
            try checkEqual(try raw.entries(of: 1)[0].decode(Filter.self), .done, "filter")
            // `visible` is a derived list (ADR-039): its entry is the keyed patch that leaves [b].
            let visibleEntry = raw.entries(of: 2)[0]
            try check(visibleEntry.op == .keyedPatch, "visible arrives as a keyed patch")
            var shown = rawTodos
            try applyPatch(try visibleEntry.decodePatch(Todo.self), to: &shown)
            try checkEqual(shown.map(\.title), ["b"], "visible in the same change-set")

            // 3. The generated store follows the filter.
            todos.setFilter(.done)
            try await waitUntil("visible to follow Done") { todos.visible.map(\.title) == ["b"] }
            try checkEqual(todos.remaining, 2, "remaining under Done")
            todos.setFilter(.active)
            try await waitUntil("visible to follow Active") { todos.visible.map(\.title) == ["a", "c"] }
            todos.setFilter(.all)
            try await waitUntil("visible to follow All") { todos.visible.map(\.title) == ["a", "b", "c"] }

            // 4. Removing and clearing.
            todos.remove(id: a.id)
            try await waitUntil("visible to lose a") { todos.visible.map(\.title) == ["b", "c"] }
            try checkEqual(todos.remaining, 1, "remaining after remove(a)")
            todos.clearDone()
            try await waitUntil("visible to lose b") { todos.visible.map(\.title) == ["c"] }
            try checkEqual(todos.remaining, 1, "remaining after clear_done")

            // 5. The parity of the counter always agrees with its count.
            let counter = try Counter(ctx: core)
            defer { counter.close() }
            for (amount, count, parity) in [(3, 3, Parity.odd), (1, 4, .even), (-1, 3, .odd), (0, 3, .odd)] as [(Int32, Int32, Parity)] {
                counter.add(amount: amount)
                try await waitUntil("count \(count)") { counter.count == count }
                try checkEqual(counter.parity, parity, "parity at count \(count)")
                try checkEqual(counter.parity, count % 2 == 0 ? .even : .odd, "parity agrees with count \(count)")
            }

            // 6. Reading never crosses the boundary.
            let list = try BigList(ctx: core)
            defer { list.close() }
            try await quietFor(milliseconds: 50)
            let crossings = core.stat("crossings.calls")
            var sink = 0
            for _ in 0 ..< 1_000 {
                sink += todos.visible.count + Int(todos.remaining) + (counter.parity == .odd ? 1 : 0) + list.items.count
            }
            try check(sink > 0, "the reads were performed")
            try checkEqual(core.stat("crossings.calls"), crossings, "crossings.calls after 4,000 reads")
        }
    }
}
