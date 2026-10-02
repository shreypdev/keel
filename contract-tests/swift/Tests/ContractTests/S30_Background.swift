import Foundation
@testable import UndraRuntime
import PlaygroundCore
import XCTest

extension ContractScenarios {
    // MARK: S30

    /// ADR-046: `runInBackground(deadline:)` over the standard function `run_background`, against the offline queue of S14.
    func testS30_backgroundRun() async {
        await scenario("S30", "background run") {
            let core = try self.core
            let server = Fixture.shared.server
            let kv = Fixture.shared.kv
            let path = "/lists/s30/todos"
            let runsBefore = core.stats().background

            // The harness failed the first read of the offline queue (S20.4); the client reads it again after a backoff.
            try await waitUntil("the offline queue to be readable") { try storageStatus(ctx: core).queueReadable }
            try await waitUntil("an empty offline queue") { try storageStatus(ctx: core).pending == 0 }

            // 1. Offline work is pending.
            server.respond("GET", path, body: "[]")
            let handle = try RemoteTodosQueryHandle(list: "s30", ctx: core)
            defer { handle.close() }
            try await waitUntil("the first fetch") { handle.status == .success && !handle.fetching }
            core.emitConnectivity(online: false, kind: .disconnected)
            try await quietFor(milliseconds: 20)
            server.failNetwork("POST", path)
            let queued = Locked<Result<RemoteTodo, any Error>?>(nil)
            Task { await ContractScenarios.create(core, "Queued", into: queued) }
            try await waitUntil("the first POST attempt") { server.requests("POST", path).count == 1 }
            try await waitUntil("the creation to wait in the queue") { try storageStatus(ctx: core).pending == 1 }
            let background = core.stats().background
            try check(background.tasks >= 3, "background.tasks is \(background.tasks)")
            try check(background.pending >= 1, "background.pending is \(background.pending): nothing tells the platform a window is worth asking for")
            // Going to the background writes a cache entry that waits out its persistence debounce at once, not 250 ms later.
            try await ContractScenarios.backgroundFlushesAtOnce(core, server, kv, handle, path: path)

            // 2. Still offline, the run says so and does not wait. A run that waited would return at its deadline less the half
            // second it keeps for the host (4.5 s); under half the deadline tells the two apart on a machine that stalls for a second.
            let first = ContinuousClock.now
            let offline = try await core.runInBackground(deadline: 5)
            try check(ContinuousClock.now - first < .milliseconds(2_500), "a run while offline took \(ContinuousClock.now - first), not under half its 5 s deadline")
            try checkEqual(offline.finished, false, "finished while offline")
            try checkEqual(offline.replayed, 0, "replayed while offline")
            try check(offline.stillPending >= 1, "stillPending while offline is \(offline.stillPending)")
            try checkEqual(try storageStatus(ctx: core).pending, 1, "the item is still queued")

            // 3. Online, the run drains the queue.
            server.respond("POST", path, status: 201, json: ServerTodo(id: 9, title: "Queued", done: false), delayMs: 400)
            server.respond("GET", path, json: [ServerTodo(id: 9, title: "Queued", done: false)])
            core.emitConnectivity(online: true, kind: .wifi)
            let drained = try await core.runInBackground(deadline: 10)
            try checkEqual(drained.finished, true, "finished after the replay (\(drained))")
            try checkEqual(drained.replayed, 1, "replayed")
            try checkEqual(drained.stillPending, 0, "stillPending after the replay")
            try await waitUntil("the pending create_remote_todo to resolve") { queued.snapshot != nil }
            let created = try success(try require(queued.snapshot, "the result"), "the replayed creation")
            try checkEqual(created, RemoteTodo(id: 9, title: "Queued", done: false), "the replayed creation's result")
            let posts = server.requests("POST", path)
            try checkEqual(posts.count, 2, "POSTs the server saw")
            let keys = posts.map { $0.header("Idempotency-Key") }
            try check(keys[0] != nil && keys[0] == keys[1], "both POSTs carry the same Idempotency-Key, got \(keys)")
            try checkEqual(try storageStatus(ctx: core).pending, 0, "pending after the drain")

            // 4. A run cut at its deadline leaves the work intact.
            core.emitConnectivity(online: false, kind: .disconnected)
            try await quietFor(milliseconds: 20)
            server.failNetwork("POST", path)
            let slow = Locked<Result<RemoteTodo, any Error>?>(nil)
            Task { await ContractScenarios.create(core, "Slow", into: slow) }
            try await waitUntil("the failed POST of Slow") { server.requests("POST", path).filter { $0.bodyText?.contains("Slow") == true }.count == 1 }
            try await waitUntil("Slow to wait in the queue") { try storageStatus(ctx: core).pending == 1 }
            server.respond("POST", path, status: 201, json: ServerTodo(id: 10, title: "Slow", done: false), delayMs: 5_000)
            server.respond("GET", path, json: [ServerTodo(id: 9, title: "Queued", done: false), ServerTodo(id: 10, title: "Slow", done: false)])
            core.emitConnectivity(online: true, kind: .wifi)
            try await waitUntil("the replay of Slow to reach the server") {
                server.requests("POST", path).filter { $0.bodyText?.contains("Slow") == true }.count == 2
            }
            // The run is cut at its deadline less the half second it keeps for the host: measured against a sleep of that length started
            // beside it, which a slow machine ends as late as it ends the core's, and not against 900 ms of wall clock. A run that kept no
            // half second, or waited for the POST, ends 500 ms or more after the sleep.
            let cutAt = ContinuousClock.now
            let reference = Task { () async -> ContinuousClock.Instant in
                try? await Task.sleep(for: .milliseconds(500))
                return ContinuousClock.now
            }
            let cut = try await core.runInBackground(deadline: 1)
            let cutEnded = ContinuousClock.now
            let took = cutEnded - cutAt
            let late = cutEnded - (await reference.value)
            try check(late < .milliseconds(400), "a run with a 1 s deadline returned \(late) after a 500 ms sleep started beside it (it took \(took))")
            try check(took > .milliseconds(300), "a run with a 1 s deadline returned after \(took), before the half second it keeps for the host")
            try checkEqual(cut.finished, false, "finished at the deadline")
            try checkEqual(cut.replayed, 0, "replayed at the deadline")
            try checkEqual(cut.stillPending, 1, "stillPending at the deadline")
            try checkEqual(try storageStatus(ctx: core).pending, 1, "the cut run left Slow queued")
            try check(kv.value(for: Persisted.queueKey).flatMap(Persisted.queueCount) == 1, "the Kv queue no longer holds Slow")
            try await waitUntil("the POST of Slow to answer", timeout: .seconds(10)) { slow.snapshot != nil }
            try await waitUntil("the queue to empty") { try storageStatus(ctx: core).pending == 0 }
            let slowPosts = server.requests("POST", path).filter { $0.bodyText?.contains("Slow") == true }
            try checkEqual(slowPosts.count, 2, "POSTs of Slow (the failed one and one replay): nothing was sent twice")
            let slowKeys = slowPosts.map { $0.header("Idempotency-Key") }
            try check(slowKeys[0] != nil && slowKeys[0] == slowKeys[1], "both POSTs of Slow carry the same Idempotency-Key, got \(slowKeys)")

            // 5. A host that cancels the call: the task is cancelled after 100 ms.
            core.emitConnectivity(online: false, kind: .disconnected)
            try await quietFor(milliseconds: 20)
            server.failNetwork("POST", path)
            let held = Locked<Result<RemoteTodo, any Error>?>(nil)
            Task { await ContractScenarios.create(core, "Held", into: held) }
            try await waitUntil("the failed POST of Held") { server.requests("POST", path).filter { $0.bodyText?.contains("Held") == true }.count == 1 }
            try await waitUntil("Held to wait in the queue") { try storageStatus(ctx: core).pending == 1 }
            server.respond("POST", path, status: 201, json: ServerTodo(id: 11, title: "Held", done: false), delayMs: 3_000)
            server.respond("GET", path, json: [ServerTodo(id: 9, title: "Queued", done: false), ServerTodo(id: 10, title: "Slow", done: false), ServerTodo(id: 11, title: "Held", done: false)])
            core.emitConnectivity(online: true, kind: .wifi)
            let run = Task { try await core.runInBackground(deadline: 30) }
            try await quietFor(milliseconds: 100)
            run.cancel()
            switch await run.result {
            case .success(let report):
                throw ScenarioFailure(description: "a cancelled run returned \(report)")
            case .failure(let error):
                try check(error is CancellationError, "a cancelled run failed with \(type(of: error)): \(error), not CancellationError")
            }
            try checkEqual(try PlaygroundCore.add(a: 1, b: 2, ctx: core), 3, "add(1, 2) after the cancelled run")
            try checkEqual(try storageStatus(ctx: core).pending, 1, "the cancelled run left Held queued until its POST answers")
            try await waitUntil("the POST of Held to answer", timeout: .seconds(10)) { held.snapshot != nil }
            try await waitUntil("the queue to empty after Held") { try storageStatus(ctx: core).pending == 0 }

            // 6. The counters of the four runs (relative to what the core had counted before this scenario).
            let counters = core.stats().background
            try checkEqual(counters.runs - runsBefore.runs, 4, "background.runs")
            try checkEqual(counters.finished - runsBefore.finished, 1, "background.finished")
            try checkEqual(counters.replayed - runsBefore.replayed, 1, "background.replayed")
        }
    }

    /// Step 1's claim: going to the background writes a cache entry that waits out its 250 ms persistence debounce at once, rather than
    /// leaving it to the debounce. Shown against the debounce's own clock, not a deadline of the machine's (100 ms was one, and a hosted
    /// runner stalled past it): the list is fetched again with new contents, which makes its cache entry dirty and arms a debounce only
    /// after `armed` was read, so a write of it seen less than 240 ms after `armed` (10 ms short, for the clocks' granularity) cannot be that debounce's. (The first fetch's own
    /// debounce is waited out first, so it cannot be either.) A trial in which the machine stalled past that (the entry written before
    /// Background, or seen 240 ms or more after `armed`) says nothing and is repeated with new contents, up to five times; a core that
    /// leaves the entry to its debounce writes it only once the debounce could have fired, and fails all five.
    @MainActor
    fileprivate static func backgroundFlushesAtOnce(_ core: UndraCore, _ server: FakeServer, _ kv: MemoryKv, _ handle: RemoteTodosQueryHandle, path: String) async throws {
        let key = Persisted.cacheKey(queryId: UndraIds.Queries.remoteTodos, arguments: "s30".undraEncoded())
        let writes = { kv.operations.filter { $0.isSet && $0.key == key }.count }
        try await waitUntil("the first fetch's cache entry to be written by its debounce") { writes() > 0 }
        // 10 ms short of the debounce: the clocks the core's timer and this read can disagree by their granularity (a millisecond).
        let debounce: Duration = .milliseconds(240)
        var seen: [String] = []
        for trial in 1 ... 5 {
            let title = "Server \(trial)"
            server.respond("GET", path, json: [ServerTodo(id: 1, title: title, done: false)])
            let armed = ContinuousClock.now
            let atArmed = writes()
            handle.refetch()
            try await waitUntil("the refetched list") { handle.data?.contains { $0.title == title } == true }
            let before = writes()
            if before > atArmed {
                // Its debounce wrote it already: the machine stalled 250 ms between the refetch and here.
                seen.append("before Background, \(ContinuousClock.now - armed)")
                continue
            }
            UndraLifecycle(core: core).changed(.background)
            do {
                try await waitUntil("the cache entry of the list to be written after Background") { writes() > before }
            } catch {
                UndraLifecycle(core: core).changed(.active)
                throw error
            }
            let at = ContinuousClock.now - armed
            UndraLifecycle(core: core).changed(.active)
            if at < debounce {
                server.respond("GET", path, body: "[]")
                return
            }
            seen.append("\(at)")
        }
        throw ScenarioFailure(description: "in five trials the cache entry was written only after the refetch that made it dirty (\(seen)), when its 250 ms debounce could have fired: Background did not write it at once")
    }

    /// `create_remote_todo(list: "s30", title)`, its outcome stored in `result` when it settles.
    @MainActor
    fileprivate static func create(_ core: UndraCore, _ title: String, into result: Locked<Result<RemoteTodo, any Error>?>) async {
        let outcome: Result<RemoteTodo, any Error>
        do {
            outcome = .success(try await createRemoteTodo(list: "s30", title: title, ctx: core))
        } catch {
            outcome = .failure(error)
        }
        result.withLock { (current: inout Result<RemoteTodo, any Error>?) -> Void in current = outcome }
    }
}
