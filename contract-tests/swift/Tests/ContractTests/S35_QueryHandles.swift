import Foundation
import PlaygroundCore
@testable import UndraRuntime
import XCTest

/// What the mirror applied to a handle during a restore, recorded beside the generated wrapper: it is registered as one more
/// function of the handle's registration, so it receives every entry the wrapper's own `apply` does, in the generated
/// `@Observable` shape and in the `ObservableObject` one of `run.sh --floor` alike.
@MainActor
private final class MirrorTap {
    private let core: UndraCore
    private let handle: UndraHandle
    private var registered = true
    /// Every entry applied since the last `clear()`: the signal and how it was applied.
    private(set) var entries: [(signal: UInt32, op: ChangeOp)] = []

    init(core: UndraCore, handle: UndraHandle) {
        self.core = core
        self.handle = handle
        core.mirror.register(handle, owner: self) { [weak self] signal, op, _ in
            self?.entries.append((signal: signal, op: op))
        }
    }

    /// Forgets what was recorded.
    func clear() {
        entries.removeAll()
    }

    /// Removes the tap from the handle's registration (the wrapper's own function stays). Idempotent.
    func stop() {
        guard registered else {
            return
        }
        registered = false
        core.mirror.unregister(handle, owner: self)
    }
}

extension ContractScenarios {
    // MARK: S35

    /// ADR-059: a query handle is a view of the query cache, so a snapshot keeps what it is made of and a restore re-issues the handle
    /// under the same value. Steps 1 to 9 restore into the runtime that holds the handles, through the generated bindings; the wrappers
    /// are the ones the app holds and nothing in this file touches them but what an app would call. Step 10 (a fresh runtime, build B)
    /// runs in the second process (`MigrationBuildB`), from what step 2 hands over.
    func testS35_queryHandlesAcrossARestore() async {
        await scenario("S35", "query handles across a restore") {
            let core = try self.core
            let server = Fixture.shared.server
            let list = "s35"
            let path = "/lists/\(list)/todos"
            let milk = ServerTodo(id: 1, title: "Buy milk", done: false)
            let dog = ServerTodo(id: 2, title: "Walk the dog", done: false)
            let shownMilk = [RemoteTodo(id: 1, title: "Buy milk", done: false)]
            let shownBoth = shownMilk + [RemoteTodo(id: 2, title: "Walk the dog", done: false)]
            func gets() -> Int { server.requests("GET", path).count }
            let pageCalls = { core.stats().hostPageCalls }
            Handover.discard("s35")

            // 1. Opening. The baseline of live_handles first; then a remote query, a polling one, an infinite one, a library, the
            // handle whose query build B changes, a counter and a probe.
            let liveBefore = core.stat("live_handles")
            core.emitConnectivity(online: true, kind: .wifi)
            UndraLifecycle(core: core).changed(.active)
            server.respond("GET", path, json: [milk])
            let remote = try RemoteTodosQueryHandle(list: list, ctx: core)
            defer { remote.close() }
            try await waitUntil("the remote handle to show milk") {
                remote.status == .success && !remote.fetching && remote.data == shownMilk
            }
            try checkEqual(gets(), 1, "GET count after the remote handle's first fetch")
            let ticker = try TickerQueryHandle(ctx: core)
            defer { ticker.close() }
            try await waitUntil("the ticker's first tick") { ticker.data != nil }
            let feed = try FeedQueryHandle(evenOnly: false, ctx: core)
            defer { feed.close() }
            try await waitUntil("the first page of the feed") { feed.data.count >= 50 && !feed.fetching }
            if feed.data.count < 100 {
                feed.fetchNextPage()
                try await waitUntil("the second page of the feed") { feed.data.count == 100 && !feed.fetchingNextPage }
            }
            try checkEqual(feed.data.count, 100, "the feed's rows (two pages)")
            let library = try Library(ctx: core)
            defer { library.close() }
            try await waitUntil("books[0]") { library.books[0] != nil }
            let pageServer = try require(library.books.testEngine.handle, "the page server of books")
            let roster = try RosterQueryHandle(team: 7, ctx: core)
            defer { roster.close() }
            try await waitUntil("the roster to show its team") { roster.status == .success && roster.data == ["team 7"] }
            let counter = try Counter(ctx: core)
            defer { counter.close() }
            counter.add(amount: 5)
            try await waitUntil("the counter to show 5") { counter.count == 5 }
            let probe = try Probe(ctx: core)
            defer { probe.close() }
            _ = try probe.counters()

            // 2. The snapshot (handed over for step 10), a change, the server's new answer, and the restore. The taps are what the
            // mirror applies to the wrappers from here on.
            let snapshot = try core.snapshot()
            try Handover.write("s35", Handover.QueryHandles(
                snapshot: Handover.hex(snapshot),
                remote: remote.handle.rawValue,
                ticker: ticker.handle.rawValue,
                feed: feed.handle.rawValue,
                library: library.handle.rawValue,
                roster: roster.handle.rawValue
            ))
            counter.add(amount: 10)
            try await waitUntil("the counter to show 15") { counter.count == 15 }
            server.respond("GET", path, json: [milk, dog])
            let remoteHandle = remote.handle
            let remoteTap = MirrorTap(core: core, handle: remote.handle)
            defer { remoteTap.stop() }
            let feedTap = MirrorTap(core: core, handle: feed.handle)
            defer { feedTap.stop() }
            let counterTap = MirrorTap(core: core, handle: counter.handle)
            defer { counterTap.stop() }
            let liveBeforeRestore = core.stat("live_handles")
            let getsBeforeRestore = gets()
            try core.restore(snapshot)

            // 3. Same wrappers, nothing blinked. A restore sends nothing for a live query handle, and starts no fetch: the quiet
            // window is what lets a fetch it wrongly started show in the GET count.
            try checkEqual(counter.count, 5, "the counter after the restore")
            try check(counterTap.entries.count > 0, "the restore delivered nothing to the counter (the taps would see nothing either)")
            try check(core.identities.live(remoteHandle) === remote && !remote.isClosed, "the remote handle's wrapper is no longer the live one of its handle")
            try checkEqual(remote.handle, remoteHandle, "the remote wrapper's handle")
            try await quietFor(milliseconds: 200)
            try check(remoteTap.entries.isEmpty, "entries the mirror applied to the remote wrapper during the restore: \(remoteTap.entries)")
            try check(feedTap.entries.isEmpty, "entries the mirror applied to the feed wrapper during the restore: \(feedTap.entries)")
            try checkEqual(remote.data, shownMilk, "the remote wrapper's data after the restore")
            try checkEqual(gets(), getsBeforeRestore, "GET count after the restore")
            try checkEqual(feed.data.count, 100, "the feed's rows after the restore")
            // The Probe is the one object the restore makes stale (step 7): no query handle was dropped and no handle was made.
            let liveAfterRestore = core.stat("live_handles")
            try checkEqual(liveAfterRestore, liveBeforeRestore - 1, "live_handles after the restore (before it, less the Probe)")

            // 4. `refetch` is accepted on the same wrapper (a refused one would report through `onError`).
            let reportsBefore = Fixture.shared.unhandled.snapshot.count
            remote.refetch()
            try await waitUntil("the refetch to reach the server") { gets() == getsBeforeRestore + 1 }
            try await waitUntil("the refetch to show two items") { remote.data == shownBoth && !remote.fetching }
            try checkEqual(Fixture.shared.unhandled.snapshot.count - reportsBefore, 0, "reports to onError for the remote handle's refetch")

            // 5. Polling continues with no call from the runner.
            let shown = ticker.data ?? 0
            try await waitUntil("the ticker to advance", timeout: .milliseconds(2_500)) { (ticker.data ?? 0) > shown }

            // 6. Pages still load: the next page of the same feed wrapper, and a page of books through the server the restore made.
            feed.fetchNextPage()
            try await waitUntil("150 rows") { feed.data.count == 150 && !feed.fetchingNextPage }
            try checkEqual(library.books.count, 10_000, "books.count after the restore")
            let newServer = try require(library.books.testEngine.handle, "the page server of books after the restore")
            try check(newServer != pageServer, "books still names the page server it had before the restore")
            let pageCallsBeforeRead = pageCalls()
            try await waitUntil("books[0] after the restore") { library.books[0]?.id == 1 }
            try check(pageCalls() > pageCallsBeforeRead, "books[0] came back without a page call through the new server")

            // 7. What is not re-creatable stays stale: the Probe is refused, as in S15 step 9.
            let stale = try callError("probe.counters() after the restore") { try probe.counters() }
            guard case .refused = stale else {
                throw ScenarioFailure(description: "probe.counters() after the restore failed with \(stale), not .refused")
            }

            // 8. Idempotent: the second restore changes nothing more.
            remoteTap.clear()
            feedTap.clear()
            let getsBeforeSecond = gets()
            try core.restore(snapshot)
            try await quietFor(milliseconds: 200)
            try checkEqual(counter.count, 5, "the counter after the second restore")
            try check(core.identities.live(remoteHandle) === remote && !remote.isClosed, "the remote handle's wrapper after the second restore")
            try check(remoteTap.entries.isEmpty, "entries the mirror applied to the remote wrapper during the second restore: \(remoteTap.entries)")
            try check(feedTap.entries.isEmpty, "entries the mirror applied to the feed wrapper during the second restore: \(feedTap.entries)")
            try checkEqual(remote.data, shownBoth, "the remote wrapper's data after the second restore")
            try checkEqual(gets(), getsBeforeSecond, "GET count after the second restore")
            try checkEqual(feed.data.count, 150, "the feed's rows after the second restore")
            try checkEqual(core.stat("live_handles"), liveAfterRestore, "live_handles after the second restore")

            // 9. Closing the wrappers returns live_handles to its value before step 1.
            remoteTap.stop()
            feedTap.stop()
            counterTap.stop()
            for wrapper in [remote, ticker, feed, roster] as [UndraObject] {
                wrapper.close()
            }
            library.close()
            counter.close()
            probe.close()
            try await waitUntil("live_handles to return to \(liveBefore)") { core.stat("live_handles") == liveBefore }
        }
    }
}
