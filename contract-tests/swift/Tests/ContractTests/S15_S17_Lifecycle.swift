import Foundation
import KeelFFI
@testable import KeelRuntime
import PlaygroundCore
import XCTest

extension ContractScenarios {
    // MARK: S15

    func testS15_snapshotAndRestore() async {
        await scenario("S15", "snapshot and restore") {
            let core = try self.core
            let liveBefore = core.stat("live_handles")

            // 1. Three observed stores, and a handle released before the snapshot.
            let todos = try Todos(ctx: core)
            defer { todos.close() }
            let counter = try Counter(ctx: core)
            defer { counter.close() }
            let list = try BigList(ctx: core)
            defer { list.close() }
            let ghost = try core.construct(type: KeelIds.Objects.Probe.typeId, method: KeelIds.Objects.Probe.new, args: [])
            core.release(ghost)

            let a = try await todos.add(title: "a")
            let b = try await todos.add(title: "b")
            todos.toggle(id: b.id)
            counter.add(amount: 5)
            try await waitUntil("the stores to show their state") {
                todos.todos.count == 2 && todos.remaining == 1 && counter.count == 5
            }

            // 2. The snapshot is non-empty and opaque.
            let snapshot = try core.snapshot()
            try check(!snapshot.isEmpty, "the snapshot is empty")

            // 3. Change everything.
            let c = try await todos.add(title: "c")
            todos.toggle(id: a.id)
            todos.setFilter(.done)
            counter.add(amount: 10)
            try list.removeAt(index: 0)
            try await waitUntil("the stores to show the changes") {
                todos.todos.count == 3 && todos.filter == .done && counter.count == 15 && list.items.count == 9_999
            }

            // 4. Restore: the same handles show the snapshot's state again, delivered through the mirror.
            try core.restore(snapshot)
            try await waitUntil("the todos to return to the snapshot") {
                todos.todos.map(\.title) == ["a", "b"] && todos.todos.map(\.done) == [false, true]
                    && todos.filter == .all && todos.visible.map(\.title) == ["a", "b"] && todos.remaining == 1
            }
            try await waitUntil("the counter to return to the snapshot") {
                counter.count == 5 && counter.changes == 1 && counter.parity == .odd
            }
            try await waitUntil("the list to return to the snapshot") {
                list.items.count == 10_000 && list.items.first?.id == 1 && list.count == 10_000
            }

            // 5. Identities continue: the next item collides with neither a nor b.
            let d = try await todos.add(title: "d")
            try check(d.id != a.id && d.id != b.id, "the id of d collides with a or b")
            try check(KeelUUID(d.id) > KeelUUID(b.id), "the id of d is not above the id of b")
            _ = c
            try await waitUntil("d to show") { todos.todos.count == 3 }

            // 6. A rejected snapshot fails and leaves every store as it was.
            let titlesBefore = todos.todos.map(\.title)
            let countBefore = counter.count
            let garbage = (0 ..< 16).map { _ in UInt8.random(in: 0 ... 255) }
            do {
                try core.restore(garbage)
                throw ScenarioFailure(description: "16 random bytes were accepted as a snapshot")
            } catch let error as KeelRestoreError {
                try check(error.code != 0, "the restore error carries code 0")
            }
            try await quietFor(milliseconds: 100)
            try checkEqual(todos.todos.map(\.title), titlesBefore, "todos after a rejected restore")
            try checkEqual(counter.count, countBefore, "count after a rejected restore")

            // 7. The handle released before the snapshot was not resurrected.
            do {
                _ = try core.callSync(
                    .objectMethod(handle: ghost, methodId: KeelIds.Objects.Probe.counters),
                    method: KeelIds.Objects.Probe.counters,
                    args: []
                )
                throw ScenarioFailure(description: "a handle released before the snapshot answers a call after the restore")
            } catch let error as KeelReplyError {
                try checkEqual(error.status, .badRequest, "status of a call on the released handle")
            }

            // 8. The restore created no handles of its own: exactly the three stores are alive.
            try checkEqual(core.stat("live_handles"), liveBefore + 3, "live_handles after the restore")
        }
    }

    // MARK: S16

    func testS16_schemaMismatchRejection() async {
        await scenario("S16", "schema mismatch rejection") {
            // The one core of this process is shut down first: the scenario needs a process in which
            // no core is initialised (`keel_init` is once per process).
            Fixture.shared.shutDown()
            try check(KeelCore.current == nil, "a core is still loaded after shutdown")
            let generated = KeelIds.schemaHash
            let wrong = generated ^ 1

            // 1. A load that expects another schema fails with the runtime's mismatch error.
            do {
                _ = try KeelCore.load(.inproc(adapters: Fixture.shared.makeAdapters(), expectedSchemaHash: wrong))
                throw ScenarioFailure(description: "a load with the wrong schema hash succeeded")
            } catch let error as KeelSchemaMismatchError {
                try checkEqual(error.expected, wrong, "the expected hash in the error")
                try checkEqual(error.got, generated, "the core's hash in the error")
                let message = "\(error)"
                try check(message.contains(ContractScenarios.hex(wrong)) && message.contains(ContractScenarios.hex(generated)),
                          "the message names both hashes in hex: \(message)")
            }
            try check(KeelCore.current == nil, "a failed load left a shared core behind")

            // 1b. "Before the core is initialised" is what the error type shows when something else
            // already initialised it: `keel_init` would be refused, and a runtime that called it
            // before comparing the hashes would fail with `coreInitFailed` and never report the
            // mismatch. The check comes first, so the answer is the same.
            do {
                let foreignInit = ContractScenarios.initialiseCoreElsewhere()
                try checkEqual(foreignInit, 0, "keel_init by another embedder")
                defer { keel_shutdown() }
                do {
                    _ = try KeelCore.load(.inproc(adapters: Fixture.shared.makeAdapters(), expectedSchemaHash: wrong))
                    throw ScenarioFailure(description: "a load with the wrong schema hash succeeded on an initialised core")
                } catch let error as KeelSchemaMismatchError {
                    try checkEqual(error.got, generated, "the core's hash in the error, core initialised elsewhere")
                }
            }

            // 2. The failed attempt did not leave the process half-initialised: a load with the right hash works.
            let core = try Fixture.shared.core()
            try checkEqual(PlaygroundCore.add(a: 20, b: 22, ctx: core), 42, "a call on the core loaded after the failed attempt")

            // 3. The hash of the bindings, of the statistics and of the exported schema agree.
            try checkEqual(core.schemaHash, generated, "KeelCore.schemaHash")
            let statistics = try JSONSerialization.jsonObject(with: Data(core.stats().json.utf8)) as? [String: Any]
            let reported = try require(statistics?["schema_hash"] as? String, "schema_hash in the statistics")
            try checkEqual(UInt64(reported.dropFirst(2), radix: 16), generated, "stats().schema_hash")
            try checkEqual(keel_schema_hash(), generated, "keel_schema_hash()")

            // 4. The exported schema lists the playground's types and the standard ports.
            let exported = keel_schema_json()
            defer { keel_buf_free(exported) }
            let json = Data(bytes: try require(exported.ptr, "keel_schema_json output"), count: Int(exported.len))
            let names = ContractScenarios.names(in: try JSONSerialization.jsonObject(with: json))
            for expected in ["Todos", "Counter", "BigList", "Bench", "Probe", "remote_todos", "post_remote_todo", "patch_remote_todo",
                             "Clock", "Rng", "Log", "Http", "Kv", "SecureStore", "Fs", "Timer", "Connectivity", "Lifecycle"] {
                try check(names.contains(expected), "the exported schema does not list \(expected)")
            }
        }
    }

    /// `keel_init` as another embedder of the core would call it, without the runtime; returns its
    /// status code (0 when the core is running).
    private static func initialiseCoreElsewhere() -> UInt32 {
        let config = RuntimeConfigRecord(platform: "macos", mode: "inproc", coreThreads: 1, blockingThreads: 0, logLevel: 2).keelEncoded()
        return config.withUnsafeBufferPointer { (bytes: UnsafeBufferPointer<UInt8>) -> UInt32 in
            return keel_init(bytes.baseAddress, UInt32(bytes.count), { _, _, _, _ in }, { _, _, _ in }, { _, _, _, _ in }, nil)
        }
    }

    /// Sixteen lowercase hex digits, the way the runtime writes a schema hash.
    private static func hex(_ value: UInt64) -> String {
        let digits = String(value, radix: 16)
        return String(repeating: "0", count: 16 - digits.count) + digits
    }

    /// Every string that is the value of a `name` key anywhere in a JSON document.
    private static func names(in json: Any) -> Set<String> {
        var found: Set<String> = []
        if let object = json as? [String: Any] {
            for (key, value) in object {
                if key == "name", let name = value as? String {
                    found.insert(name)
                }
                found.formUnion(names(in: value))
            }
        } else if let array = json as? [Any] {
            for value in array {
                found.formUnion(names(in: value))
            }
        }
        return found
    }

    // MARK: S17

    func testS17_panicContainment() async {
        await scenario("S17", "panic containment") {
            let core = try self.core
            let log = Fixture.shared.log
            let store = try Counter(ctx: core)
            defer { store.close() }
            let panicsBefore = core.stat("panics")
            let recordsBefore = log.all.count

            // 1. A panic in a sync call is a reply with status panic, not a crash. (The generated
            // `explode(reason:)` treats a panic as an unexpected failure and stops the process on
            // purpose, so the raw call is used.)
            do {
                _ = try core.callSync(
                    .freeFunction(methodId: KeelIds.Functions.explode),
                    method: KeelIds.Functions.explode,
                    args: encoded { (w: inout KeelWriter) in w.writeString("kaboom") }
                )
                throw ScenarioFailure(description: "explode(\"kaboom\") returned")
            } catch let error as KeelReplyError {
                try checkEqual(error.status, .panic, "status of explode")
                try check((error.message ?? "").contains("kaboom"), "the panic message names the reason: \(error.message ?? "<none>")")
                var reader = KeelReader(error.body)
                _ = try reader.readString()
                _ = try reader.readString()
                try reader.finish()
            }

            // 2. So is a panic in an async call.
            do {
                _ = try await explodeLater(delayMs: 10, reason: "later", ctx: core)
                throw ScenarioFailure(description: "explode_later returned")
            } catch let error as KeelReplyError {
                try checkEqual(error.status, .panic, "status of explode_later")
                try check((error.message ?? "").contains("later"), "the panic message names the reason: \(error.message ?? "<none>")")
            }

            // 3. The core keeps working, a store built before still updates, two panics were counted.
            try checkEqual(PlaygroundCore.add(a: 1, b: 2, ctx: core), 3, "add(1, 2) after the panics")
            store.add(amount: 1)
            try await waitUntil("the store built before the panics to update") { store.count == 1 }
            try checkEqual(core.stat("panics") - panicsBefore, 2, "panics delta")

            // 4. The Log port got an error-or-worse record from `keel::panic` for each.
            try await waitUntil("two panic records") {
                log.all.dropFirst(recordsBefore).filter { $0.level >= 4 && $0.target == "keel::panic" }.count >= 2
            }
            let records = log.all.dropFirst(recordsBefore).filter { $0.level >= 4 && $0.target == "keel::panic" }
            try check(records.contains { $0.message.contains("kaboom") }, "no panic record names kaboom: \(records)")
            try check(records.contains { $0.message.contains("later") }, "no panic record names later: \(records)")
        }
    }
}
