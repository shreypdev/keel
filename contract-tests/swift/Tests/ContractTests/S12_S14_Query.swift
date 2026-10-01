import Foundation
@testable import UndraRuntime
import PlaygroundCore
import XCTest

extension ContractScenarios {
    private static let milk = ServerTodo(id: 1, title: "Buy milk", done: false)

    // MARK: S12

    func testS12_queryFetchStaleRefetch() async {
        await scenario("S12", "query: fetch, stale, refetch") {
            let core = try self.core
            let server = Fixture.shared.server
            let clock = Fixture.shared.clock
            let path = "/lists/s12/todos"
            let liveBefore = core.stat("live_handles")
            func gets() -> Int { server.requests("GET", path).count }

            // 1. The first observer fetches: idle or fetching while pending, then success.
            server.respond("GET", path, json: [ContractScenarios.milk], delayMs: 30)
            let first = try RemoteTodosQueryHandle(list: "s12", ctx: core)
            let statuses = ChangeRecorder { first.status }
            try await waitUntil("the first fetch") { first.status == .success && !first.fetching }
            statuses.stop()
            try check([QueryStatus.idle, .fetching].contains(statuses.values[0]), "the status at the start is \(statuses.values[0])")
            try checkEqual(statuses.values.last, .success, "the last status")
            try check(statuses.values.contains(.fetching) || statuses.values[0] == .fetching, "the query was never fetching: \(statuses.values)")
            try checkEqual(first.data, [RemoteTodo(id: 1, title: "Buy milk", done: false)], "data")
            try check(first.error == nil, "error is \(String(describing: first.error))")
            try checkEqual(first.fetching, false, "fetching")
            try checkEqual(first.updatedAt.map(self.milliseconds), clock.nowMs, "updatedAt against the manual clock")
            try checkEqual(gets(), 1, "GET count after the first handle")

            // 2. Ten seconds later a second handle is served from the cache: no request.
            clock.advance(seconds: 10)
            let second = try RemoteTodosQueryHandle(list: "s12", ctx: core)
            try checkEqual(second.data, first.data, "the second handle's data, right after init")
            try await quietFor(milliseconds: 100)
            try checkEqual(gets(), 1, "GET count after a second handle inside the 30 s fresh window")

            // 3. Forty-one seconds after the fetch the data is stale: a third handle fetches.
            clock.advance(seconds: 31)
            let firstFetchedAt = first.updatedAt
            server.respond("GET", path, json: [ContractScenarios.milk, ServerTodo(id: 2, title: "Walk", done: false)])
            let third = try RemoteTodosQueryHandle(list: "s12", ctx: core)
            try await waitUntil("the stale fetch of the third handle") { gets() == 2 && !third.fetching }
            let two = [RemoteTodo(id: 1, title: "Buy milk", done: false), RemoteTodo(id: 2, title: "Walk", done: false)]
            for (name, handle) in [("first", first), ("second", second), ("third", third)] {
                try await waitUntil("the \(name) handle to show two items") { handle.data == two }
                try check((handle.updatedAt ?? .distantPast) > (firstFetchedAt ?? .distantFuture), "the \(name) handle's updatedAt is newer")
            }

            // 4. `refetch()` fetches although the data is fresh.
            first.refetch()
            try await waitUntil("the refetch") { gets() == 3 }
            try await waitUntil("the refetch to end") { !first.fetching }

            // 5. `invalidate()` marks stale and, being observed, refetches.
            second.invalidate()
            try await waitUntil("the refetch after invalidate") { gets() == 4 }
            try await waitUntil("the refetch to end") { !first.fetching }

            // 6. A 503: after the core's retry the error is typed and the last good data is kept.
            server.respond("GET", path, status: 503, body: "down")
            first.refetch()
            try await waitUntil("the 503 to become an error", timeout: .seconds(10)) { first.status == .error }
            try checkEqual(first.error, .status(code: 503), "error after a 503")
            try checkEqual(first.data, two, "data after a 503 (the last good list)")

            // 7. A body that is not JSON.
            server.respond("GET", path, body: "not json")
            first.refetch()
            try await waitUntil("the bad body to become an error", timeout: .seconds(10)) {
                if case .badBody? = first.error {
                    return first.status == .error
                }
                return false
            }

            // 8. Releasing the handles releases their objects.
            first.close()
            second.close()
            third.close()
            try await waitUntil("live_handles to return to \(liveBefore)") { core.stat("live_handles") == liveBefore }
        }
    }

    private func milliseconds(_ date: Date) -> Int64 {
        return Int64((date.timeIntervalSince1970 * 1000).rounded())
    }

    // MARK: S13

    func testS13_optimisticMutationAndRollback() async {
        await scenario("S13", "optimistic mutation and rollback") {
            let core = try self.core
            let server = Fixture.shared.server
            let path = "/lists/s13/todos"
            let milk = RemoteTodo(id: 1, title: "Buy milk", done: false)
            server.respond("GET", path, json: [ContractScenarios.milk])
            let handle = try RemoteTodosQueryHandle(list: "s13", ctx: core)
            defer { handle.close() }
            try await waitUntil("the first fetch") { handle.status == .success && !handle.fetching }
            let recorder = ChangeRecorder { handle.data ?? [] }
            defer { recorder.stop() }

            // 1. The server refuses the creation: the placeholder shows during the 50 ms, then is rolled back.
            server.respond("POST", path, status: 500, body: "boom", delayMs: 50)
            try checkFailure(await outcome { () async throws(RemoteError) -> RemoteTodo in
                try await createRemoteTodo(list: "s13", title: "Walk", ctx: core)
            }, .status(code: 500), "create_remote_todo against a 500")
            try await waitUntil("the rollback") { handle.data == [milk] && handle.status == .success && !handle.fetching }
            let placeholder = RemoteTodo(id: UInt32.max, title: "Walk", done: false)
            try checkEqual(recorder.values, [[milk], [milk, placeholder], [milk]], "data while creating against a 500")

            // 2. The server accepts it: optimistic, then the server's item after the refetch.
            let walk = RemoteTodo(id: 2, title: "Walk", done: false)
            server.respond("POST", path, status: 201, json: ServerTodo(id: 2, title: "Walk", done: false), delayMs: 50)
            server.respond("GET", path, json: [ContractScenarios.milk, ServerTodo(id: 2, title: "Walk", done: false)])
            let created = try success(await outcome { () async throws(RemoteError) -> RemoteTodo in
                try await createRemoteTodo(list: "s13", title: "Walk", ctx: core)
            }, "create_remote_todo against a 201")
            try checkEqual(created, walk, "the created item")
            try await waitUntil("the refetch after the creation") { handle.data == [milk, walk] && !handle.fetching }
            let shown = recorder.values
            try check(shown.count >= 2, "no optimistic state was recorded: \(shown)")
            let optimistic = shown[shown.count - 2]
            try check(optimistic.count == 2 && optimistic[1].title == "Walk" && optimistic[1].id > 4_000_000_000,
                      "expected a placeholder before the server's item, recorded \(shown)")
            try checkEqual(shown.last, [milk, walk], "the list after the creation")
            let key = try require(server.requests("POST", path).last?.header("Idempotency-Key"), "the POST carries an Idempotency-Key")
            try check(UUID(uuidString: key) != nil, "Idempotency-Key \(key) is not a UUID")

            // 3. A refused completion flips back.
            recorder.stop()
            let flips = ChangeRecorder { handle.data?.first?.done }
            server.respond("PATCH", "\(path)/1", status: 500, body: "boom", delayMs: 50)
            try checkFailure(await outcome { () async throws(RemoteError) -> RemoteTodo in
                try await setRemoteDone(list: "s13", id: 1, done: true, ctx: core)
            }, .status(code: 500), "set_remote_done against a 500")
            try await waitUntil("the rollback of the flag") { handle.data?.first?.done == false && !handle.fetching }
            flips.stop()
            try checkEqual(flips.values, [false, true, false], "milk's done flag while the PATCH fails")

            // 4. An accepted completion sticks, after the refetch.
            server.respond("PATCH", "\(path)/1", status: 200, json: ServerTodo(id: 1, title: "Buy milk", done: true), delayMs: 50)
            server.respond("GET", path, json: [ServerTodo(id: 1, title: "Buy milk", done: true), ServerTodo(id: 2, title: "Walk", done: false)])
            let done = try success(await outcome { () async throws(RemoteError) -> RemoteTodo in
                try await setRemoteDone(list: "s13", id: 1, done: true, ctx: core)
            }, "set_remote_done against a 200")
            try checkEqual(done.done, true, "the item the PATCH returned")
            try await waitUntil("the refetch after the PATCH") { handle.data?.first?.done == true && !handle.fetching }
            try checkEqual(handle.status, .success, "status at the end")
        }
    }

    // MARK: S14

    func testS14_offlineQueueReplay() async {
        await scenario("S14", "offline queue replay") {
            let core = try self.core
            let server = Fixture.shared.server
            let kv = Fixture.shared.kv
            let path = "/lists/s14/todos"
            server.respond("GET", path, body: "[]")
            let handle = try RemoteTodosQueryHandle(list: "s14", ctx: core)
            defer { handle.close() }
            try await waitUntil("the first fetch") { handle.status == .success && !handle.fetching }

            // 1. The device goes offline.
            core.emitConnectivity(online: false, kind: .disconnected)
            try await quietFor(milliseconds: 50)

            // 2. A creation fails on the network and is queued: it stays pending, with its placeholder on screen.
            server.failNetwork("POST", path)
            let finished = Locked<Result<RemoteTodo, RemoteError>?>(nil)
            Task {
                let result = await outcome { () async throws(RemoteError) -> RemoteTodo in
                    try await createRemoteTodo(list: "s14", title: "Offline item", ctx: core)
                }
                finished.withLock { (current: inout Result<RemoteTodo, RemoteError>?) -> Void in current = result }
            }
            try await waitUntil("the first POST attempt") { server.requests("POST", path).count == 1 }
            try await quietFor(milliseconds: 200)
            try check(finished.snapshot == nil, "create_remote_todo finished while offline: \(String(describing: finished.snapshot))")
            try checkEqual(server.requests("POST", path).count, 1, "POSTs the server saw while offline")
            try checkEqual(handle.data?.map(\.title), ["Offline item"], "the placeholder on screen")

            // 3. A mutation that is not idempotent does not queue: it fails at once.
            server.failNetwork("PATCH", "\(path)/1")
            let patchStarted = ContinuousClock.now
            try checkFailure(await outcome { () async throws(RemoteError) -> RemoteTodo in
                try await setRemoteDone(list: "s14", id: 1, done: true, ctx: core)
            }, .http(.network("offline")), "set_remote_done while offline")
            try check(ContinuousClock.now - patchStarted < waitLimit, "set_remote_done while offline did not fail at once (bounded by waitLimit for loaded CI runners)")

            // 4. The network returns: the queued POST is replayed.
            let queuedWhileOffline = kv.operations
            server.respond("POST", path, status: 201, json: ServerTodo(id: 9, title: "Offline item", done: false))
            server.respond("GET", path, json: [ServerTodo(id: 9, title: "Offline item", done: false)])
            core.emitConnectivity(online: true, kind: .wifi)

            // 5. The pending call resolves, both POSTs carry the same idempotency key, the list shows the server's item.
            try await waitUntil("the queued creation to resolve") { finished.snapshot != nil }
            let created = try success(try require(finished.snapshot, "the result"), "the replayed creation")
            try checkEqual(created, RemoteTodo(id: 9, title: "Offline item", done: false), "the replayed creation's result")
            let posts = server.requests("POST", path)
            try checkEqual(posts.count, 2, "POSTs the server saw in total")
            let keys = posts.map { $0.header("Idempotency-Key") }
            try check(keys[0] != nil && keys[0] == keys[1], "both POSTs carry the same Idempotency-Key, got \(keys)")
            try await waitUntil("the list to show the server's item") {
                handle.data == [RemoteTodo(id: 9, title: "Offline item", done: false)] && !handle.fetching
            }

            // 6. The queue was persisted while offline and is gone after the replay.
            let queueKey = "undra.query.queue"
            try check(queuedWhileOffline.contains { $0.key == queueKey && $0.isSet }, "no write of \(queueKey) while offline: \(queuedWhileOffline)")
            try await waitUntil("the queue to be emptied") {
                guard let last = kv.operations.last(where: { $0.key == queueKey }) else {
                    return false
                }
                return last.isEmptyQueue
            }
        }
    }
}

extension MemoryKv.Operation {
    /// Whether this is a write (not a delete).
    var isSet: Bool {
        if case .set = self {
            return true
        }
        return false
    }

    /// Whether this operation leaves the offline queue empty: a delete, or a write whose encoding
    /// (`schema_hash u64, count u32, ..`) has no entries. The core deletes the key.
    var isEmptyQueue: Bool {
        switch self {
        case .delete:
            return true
        case .set(_, let value):
            guard value.count >= 12 else {
                return false
            }
            var reader = UndraReader(Array(value[8...]))
            return (try? reader.readU32()) == 0
        }
    }
}
