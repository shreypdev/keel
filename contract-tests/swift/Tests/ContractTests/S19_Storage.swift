import Foundation
@testable import UndraRuntime
import PlaygroundCore
import XCTest

extension ContractScenarios {
    // MARK: S19

    /// S19, the native variant (scenarios.md, platform notes): steps 1, 2, 4 (what the harness's failed
    /// first read of the queue did) and 5. Step 3 needs a fresh core and is not run on native.
    func testS19_storageFailuresAreTyped() async {
        await scenario("S19", "storage failures are typed") {
            let core = try self.core
            let server = Fixture.shared.server
            let kv = Fixture.shared.kv
            let log = Fixture.shared.log
            let path = "/lists/s19/todos"
            let panicsBefore = core.stat("panics")
            let logBefore = log.all.count
            let statusBefore = try storageStatus(ctx: core)
            defer { kv.heal() }
            let s19Key = Persisted.cacheKey(queryId: UndraIds.Queries.remoteTodos, arguments: "s19".undraEncoded())
            let item = RemoteTodo(id: 1, title: "Stored", done: false)
            server.respond("GET", path, json: [ServerTodo(id: 1, title: "Stored", done: false)])

            // 1. Every write fails `Full`: the item shows, nothing of it is stored, one WARN says so, nothing panicked.
            kv.fail(.set, with: .full)
            let handle = try RemoteTodosQueryHandle(list: "s19", ctx: core)
            defer { handle.close() }
            try await waitUntil("the s19 item to show") { handle.data == [item] && !handle.fetching }
            try await quietFor(milliseconds: 300)
            try await waitUntil("the failed write to be counted") {
                try storageStatus(ctx: core).writeFailed > statusBefore.writeFailed
            }
            try check(kv.value(for: s19Key) == nil, "\(s19Key) is in the Kv although every write failed")
            try check(!kv.operations.contains { $0.key == s19Key && $0.isSet }, "a write of \(s19Key) succeeded")
            try check(kv.operations.contains { $0.key == s19Key && $0.kind == .set && $0.failure == .full },
                      "no write of \(s19Key) was attempted (and failed Full)")
            let warnings = Array(log.all.dropFirst(logBefore)).filter {
                $0.level == 3 && $0.target == "undra::query" && $0.message.contains("storage is full")
            }
            try checkEqual(warnings.count, 1, "WARN records of undra::query saying the storage is full")
            try checkEqual(core.stat("panics"), panicsBefore, "panics after the failed writes")
            try checkEqual(try PlaygroundCore.add(a: 1, b: 2, ctx: core), 3, "a call after the failed writes")

            // 2. The Kv heals: a refetch stores the entry, in format 2, next to the description of its type.
            kv.heal()
            handle.invalidate()
            try await waitUntil("the s19 entry to be stored") { kv.value(for: s19Key) != nil }
            let stored = try require(kv.value(for: s19Key), "the s19 entry")
            try checkEqual(Array(stored.prefix(2)), [2, 0], "the format of the stored entry")
            let fingerprint = try require(Persisted.fingerprint(stored, at: 10), "the fingerprint of the stored entry")
            let typesKey = Persisted.typesKey(fingerprint)
            try check(kv.value(for: typesKey) != nil, "the Kv holds no \(typesKey) for the stored entry")
            try await waitUntil("the data to still show") { handle.data == [item] && !handle.fetching }

            // 3. Not run on native: the core is not reloaded (scenarios.md, platform notes).

            // 4. The harness failed the first read of the queue (Locked, at load). The queue is readable now, it
            // was read again, and nothing wrote it in between.
            let readable = try storageStatus(ctx: core).queueReadable
            try check(readable, "the queue is still unreadable")
            let operations = kv.operations
            let failed = try require(
                operations.firstIndex { $0.kind == .get && $0.key == Persisted.queueKey && $0.failure == .locked },
                "the failed read of \(Persisted.queueKey) at load"
            )
            let after = operations[(failed + 1)...]
            let reread = try require(
                after.firstIndex { $0.kind == .get && $0.key == Persisted.queueKey },
                "a second read of \(Persisted.queueKey)"
            )
            try check(operations[reread].failure == nil, "the second read of the queue failed too: \(operations[reread].summary)")
            try check(!operations[(failed + 1) ..< reread].contains { $0.kind == .set && $0.key == Persisted.queueKey },
                      "\(Persisted.queueKey) was written while it could not be read")

            // 5. Nothing panicked, and this is the core the scenario started with.
            try checkEqual(core.stat("panics"), panicsBefore, "panics during the scenario")
            let current = try self.core
            try check(current === core, "the core was reloaded during the scenario")
        }
    }
}
