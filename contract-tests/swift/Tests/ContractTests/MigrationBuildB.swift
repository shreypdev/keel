import Foundation
import UndraFFI
@testable import UndraRuntime
import PlaygroundCore
import PlaygroundCoreFFI
import XCTest

/// The build-B process of the two-build steps (scenarios.md, "Two builds"; S14 steps 8 and 9, S15 steps
/// 12 to 14, S35 step 10).
///
/// `run.sh` runs it after the main run, in a second `swift test` process (`--skip-build --filter
/// MigrationBuildB`, with `UNDRA_CONTRACT_PHASE=B`) whose `.build/core/libplayground_core.dylib` is build B's
/// library. Build B has no generated bindings: the test loads the core with the hash it reports
/// (its table's `schema_hash`) and drives it through `UndraCore`'s raw API with ids made by `fnv1a32`
/// (the generated `StorageStatus`, `QueryStatus` and `RemoteTodo` are used only as codecs: their layout
/// is the same in both builds). It reads what build A handed over (`Handover`) and prints only
/// `SCENARIO S14 FAIL ...` / `SCENARIO S15 FAIL ...` / `SCENARIO S35 FAIL ...` lines (and an informational
/// `MIGRATION ... ok`), so `check.sh`, which reads the last line of an id, keeps build A's `PASS` unless
/// build B fails. In the main run it is skipped.
///
/// S14 and S15 run on the first core. S35 runs on a **second core of build B**, loaded after the first was shut down (the
/// in-process runtime loads one core at a time, and a new load starts fresh, S17): step 10 is about a runtime that never held
/// the handles, and the first core is not one: S15's restore of snapshot `P` left it holding the `Todos`, `Counter`, `BigList`
/// and `Profile` stores of build A's S15, which S35's restore would drop (the growth of `live_handles` it measures would be
/// 7 less their number). The second core also starts with an empty `Kv` and its own `FakeServer`, so S35 depends on nothing
/// the earlier steps did.
@MainActor
final class MigrationBuildB: XCTestCase {
    private enum Ids {
        static let configureRemote = fnv1a32("fn.configure_remote")
        static let storageStatus = fnv1a32("fn.storage_status")
        static let add = fnv1a32("fn.add")
        static let profileDescribe = fnv1a32("Profile.describe")
        /// The method every query handle has, whatever its query: `refetch`.
        static let queryRefetch = fnv1a32("query.refetch")
    }

    private static let notes = "/lists/s14m/notes"
    private static let todos = "/lists/s35/todos"

    func testBuildB() async throws {
        guard ProcessInfo.processInfo.environment["UNDRA_CONTRACT_PHASE"] == "B" else {
            throw XCTSkip("the build-B phase runs in its own process (run.sh sets UNDRA_CONTRACT_PHASE=B)")
        }
        // Build B has build A's namespace, so its table is the same export (ADR-044).
        let reported = playgroundTable().schema_hash
        guard reported != UndraIds.schemaHash else {
            fail("S14", "the core in .build/core is build A (schema hash 0x\(Persisted.hex(reported, digits: 16))), not build B")
            fail("S15", "the core in .build/core is build A, not build B")
            fail("S35", "the core in .build/core is build A, not build B")
            return
        }
        let queue: Handover.Queue?
        let snapshots: Handover.Snapshots?
        let queryHandles: Handover.QueryHandles?
        do {
            queue = try Handover.read("s14", as: Handover.Queue.self)
            snapshots = try Handover.read("s15", as: Handover.Snapshots.self)
            queryHandles = try Handover.read("s35", as: Handover.QueryHandles.self)
        } catch {
            fail("S14", "the handover from build A does not read: \(error)")
            fail("S15", "the handover from build A does not read: \(error)")
            fail("S35", "the handover from build A does not read: \(error)")
            return
        }
        var entries: [String: [UInt8]] = [:]
        for (key, hex) in queue?.kv ?? [:] {
            entries[key] = Handover.bytes(hex)
        }

        // 8. Build B is loaded with exactly build A's Kv contents, offline.
        let server = FakeServer()
        let kv = MemoryKv(entries: entries)
        let log = CapturingLog()
        server.failNetwork("POST", MigrationBuildB.notes)
        let core: UndraCore
        do {
            core = try UndraCore.load(.inproc(
                api: playground_core_undra_api(),
                adapters: Adapters([ManualClock(), server, kv, log, RngAdapter(), TimerAdapter()]),
                expectedSchemaHash: reported
            ))
        } catch {
            fail("S14", "build B did not load: \(error)")
            fail("S15", "build B did not load: \(error)")
            fail("S35", "build B did not load: \(error)")
            return
        }
        defer { core.shutdown() }
        core.emitConnectivity(online: false, kind: .disconnected)
        do {
            _ = try core.callSync(
                .freeFunction(methodId: Ids.configureRemote),
                method: Ids.configureRemote,
                args: encoded { (w: inout UndraWriter) -> Void in w.writeString(FakeServer.baseURL) }
            )
        } catch {
            fail("S14", "configure_remote in build B failed: \(error)")
        }

        await step("S14") {
            let handed = try require(queue, "the Kv contents of build A's S14 step 7 (did build A's S14 pass?)")
            try await self.queueSteps(core: core, server: server, kv: kv, idempotencyKey: handed.idempotencyKey)
        }
        await step("S15") {
            let handed = try require(snapshots, "the snapshots of build A's S15 step 11 (did build A's S15 pass?)")
            try self.snapshotSteps(core: core, log: log, handed: handed)
        }
        await step("S35") {
            let handed = try require(queryHandles, "the snapshot and the handles of build A's S35 step 2 (did build A's S35 reach step 2?)")
            // A fresh runtime: the first core is shut down, and build B is loaded again over an empty `Kv` and a server of its own.
            core.shutdown()
            let freshServer = FakeServer()
            let freshLog = CapturingLog()
            let fresh: UndraCore
            do {
                fresh = try UndraCore.load(.inproc(
                    api: playground_core_undra_api(),
                    adapters: Adapters([ManualClock(), freshServer, MemoryKv(entries: [:]), freshLog, RngAdapter(), TimerAdapter()]),
                    expectedSchemaHash: reported
                ))
            } catch {
                throw ScenarioFailure(description: "the second core of build B did not load: \(error)")
            }
            defer { fresh.shutdown() }
            try await self.queryHandleSteps(core: fresh, server: freshServer, log: freshLog, handed: handed)
        }
    }

    // MARK: S14 steps 8 and 9

    private func queueSteps(core: UndraCore, server: FakeServer, kv: MemoryKv, idempotencyKey: String) async throws {
        // 8. Build B reads the queue: save_note migrates by parameter name, tag_note is a dead letter.
        try await waitUntil("build B to read build A's queue") {
            let status = try self.storageStatus(core)
            return status.queueReadable && status.pending == 1 && status.migrated == 1 && status.deadLettered == 1
        }
        let status = try storageStatus(core)
        try checkEqual(status.deadLetters.count, 1, "dead letters in build B: \(status.deadLetters)")
        let letter = status.deadLetters[0]
        try check(letter.hasPrefix("tag_note: ") && letter.contains("does not migrate"),
                  "the dead letter says tag_note's input does not migrate: \(letter)")
        try checkEqual(server.requests.count, 0, "requests build B made while offline")

        // 9. Online, the migrated save_note replays once, with build A's idempotency key; the dead letter stays.
        server.respond("POST", MigrationBuildB.notes, status: 201, body: "{}")
        core.emitConnectivity(online: true, kind: .wifi)
        try await waitUntil("the migrated save_note to replay") {
            try self.storageStatus(core).pending == 0 && !server.requests("POST", MigrationBuildB.notes).isEmpty
        }
        try await quietFor(milliseconds: 200)
        let posts = server.requests("POST", MigrationBuildB.notes)
        try checkEqual(posts.count, 1, "POSTs build B replayed")
        try checkEqual(posts[0].bodyText, "save:a", "the body of the replayed save_note (pinned is None)")
        try checkEqual(posts[0].header("Idempotency-Key"), idempotencyKey, "the Idempotency-Key of the replay")
        let after = try storageStatus(core)
        try checkEqual(after.pending, 0, "pending after the replay")
        try checkEqual(after.deadLetters, status.deadLetters, "the dead letters after the replay")
        try check(kv.value(for: Persisted.deadLetterKey) != nil, "the Kv holds no \(Persisted.deadLetterKey)")
        MigrationBuildB.report("MIGRATION S14 build B ok: \(letter)")
    }

    // MARK: S15 steps 12 to 14

    private func snapshotSteps(core: UndraCore, log: CapturingLog, handed: Handover.Snapshots) throws {
        let snapshotP = try require(Handover.bytes(handed.profile), "snapshot P")
        let snapshotL = try require(Handover.bytes(handed.legacy), "snapshot L")
        let profile = UndraHandle(rawValue: handed.profileHandle)

        // 12. P restores by name: the same handle describes build B's Profile, with the default theme.
        try core.restore(snapshotP)
        try checkEqual(try describe(core, profile), "name=ada;visits=2;theme=", "the Profile restored into build B")

        // 13. L is refused as incompatible (Legacy.score changed from i32 to String), with an ERROR record.
        let logBefore = log.all.count
        do {
            try core.restore(snapshotL)
            throw ScenarioFailure(description: "build B restored a snapshot whose Legacy.score cannot migrate")
        } catch let error as UndraRestoreError {
            try checkEqual(error, .incompatible, "the restore error of L in build B")
        }
        let errors = Array(log.all.dropFirst(logBefore)).filter { $0.level >= 4 }
        try check(errors.contains { $0.message.contains("Legacy") && $0.message.contains("score") },
                  "no ERROR record names Legacy and score: \(errors.map(\.message))")
        try checkEqual(try describe(core, profile), "name=ada;visits=2;theme=", "the Profile after the refused restore")
        try checkEqual(try add(core, 40, 2), 42, "a call after the refused restore")

        // 14. A snapshot from before layout 2 is malformed in build B too.
        do {
            try core.restore([0, 0, 0, 0, 0, 0, 0, 0])
            throw ScenarioFailure(description: "build B accepted the 8-byte snapshot of the layout before ADR-037")
        } catch let error as UndraRestoreError {
            try checkEqual(error, .badSnapshot, "the restore error of a layout-1 snapshot in build B")
        }
        try checkEqual(try describe(core, profile), "name=ada;visits=2;theme=", "the Profile after the malformed snapshot")
        MigrationBuildB.report("MIGRATION S15 build B ok")
    }

    // MARK: S35 step 10

    /// 10. A fresh runtime: build B restores the snapshot of build A's S35 step 2, which holds a `Counter`, a `Library` and the
    /// handles of four queries (`roster`'s parameter changed from `u32` to `String`, the others did not). Nothing is built until the
    /// runner uses a handle (R12), and the runner uses them the way a host does after a reload: it observes them again.
    private func queryHandleSteps(core: UndraCore, server: FakeServer, log: CapturingLog, handed: Handover.QueryHandles) async throws {
        let snapshot = try require(Handover.bytes(handed.snapshot), "the snapshot of S35 step 2")
        let remote = UndraHandle(rawValue: handed.remote)
        let ticker = UndraHandle(rawValue: handed.ticker)
        let library = UndraHandle(rawValue: handed.library)
        let roster = UndraHandle(rawValue: handed.roster)
        let path = MigrationBuildB.todos
        func listGets() -> Int { server.requests("GET", path).count }
        func allGets() -> Int { server.requests.filter { $0.method == "GET" }.count }
        func refetch(_ handle: UndraHandle) throws -> [UInt8] {
            return try core.callSync(
                .objectMethod(handle: handle, methodId: Ids.queryRefetch),
                method: Ids.queryRefetch,
                args: []
            )
        }
        try checkEqual(Ids.queryRefetch, UndraIds.Objects.RemoteTodosQueryHandle.refetch, "the id of query.refetch in the generated bindings")

        // The state of the host, whatever the steps before left: online and active (what polling needs).
        core.emitConnectivity(online: true, kind: .wifi)
        UndraLifecycle(core: core).changed(.active)

        // The restore succeeds; live_handles grew by the stores of the snapshot (a Counter, a Library and its two page servers) and
        // the three query handles build B honours: 7. The handle of `roster` is refused, not counted. No GET was made.
        let liveBefore = core.stat("live_handles")
        let getsBefore = allGets()
        let logBefore = log.all.count
        try core.restore(snapshot)
        let restoreLog = Array(log.all.dropFirst(logBefore)).filter { $0.level >= 3 }.map(\.message)
        try checkEqual(core.stat("live_handles") - liveBefore, 7,
                       "the growth of live_handles after restoring build A's snapshot into build B (WARN and above in the restore: \(restoreLog); dormant_handles: \(core.stat("dormant_handles")))")
        try await quietFor(milliseconds: 200)
        try checkEqual(allGets(), getsBefore, "GETs build B made during the restore")
        try checkEqual(listGets(), 0, "GETs of /lists/s35/todos before the runner observed anything")

        // The remote handle, observed again after the harness points the core at its server (core state outside stores): `status` is
        // fetching and `data` absent in the answer, then the data.
        let milk = ServerTodo(id: 1, title: "Buy milk", done: false)
        let shownMilk = [RemoteTodo(id: 1, title: "Buy milk", done: false)]
        server.respond("GET", path, json: [milk], delayMs: 100)
        _ = try core.callSync(
            .freeFunction(methodId: Ids.configureRemote),
            method: Ids.configureRemote,
            args: encoded { (w: inout UndraWriter) -> Void in w.writeString(FakeServer.baseURL) }
        )
        try checkEqual(listGets(), 0, "GETs of /lists/s35/todos after configure_remote")
        let remoteStore = RawStore(core: core, adopting: remote)
        defer { remoteStore.close() }
        remoteStore.observe()
        let firstStatus = try require(remoteStore.entries(of: 1).first, "a status entry in the answer to the observe of the remote handle")
        try checkEqual(try firstStatus.decode(QueryStatus.self), .fetching, "the remote handle's first status (wire value 1)")
        let firstData = try require(remoteStore.entries(of: 0).first, "a data entry in the answer to the observe of the remote handle")
        let initial = try firstData.decode([RemoteTodo]?.self)
        try check(initial == nil, "the remote handle's first data is \(String(describing: initial)), not absent")
        func remoteData() throws -> [RemoteTodo]? {
            return try remoteStore.entries(of: 0).last?.decode([RemoteTodo]?.self) ?? nil
        }
        try await waitUntil("the remote handle's data") { try remoteData() == shownMilk }
        let statuses = try remoteStore.entries(of: 1).map { try $0.decode(QueryStatus.self) }
        try checkEqual(statuses.last, .success, "the remote handle's status after the fetch (all: \(statuses))")
        try checkEqual(listGets(), 1, "GETs of /lists/s35/todos after the first observe")

        // `refetch` is status 0 and makes one more GET.
        try checkEqual(try refetch(remote), [], "the body of refetch on the remote handle")
        try await waitUntil("the GET of the refetch") { listGets() == 2 }

        // The ticker's handle (observed again like any handle after a reload) delivers ticks.
        let tickerStore = RawStore(core: core, adopting: ticker)
        defer { tickerStore.close() }
        tickerStore.observe()
        func tickerData() throws -> UInt32? {
            return try tickerStore.entries(of: 0).last?.decode(UInt32?.self) ?? nil
        }
        try await waitUntil("the ticker's first tick") { try tickerData() != nil }
        let firstTick = try require(try tickerData(), "the ticker's first tick")
        try await waitUntil("the ticker's next tick", timeout: .milliseconds(2_500)) { (try tickerData() ?? 0) > firstTick }

        // `Library`'s books, observed again, page through the server its op 0 names; the reply says 10,000 rows.
        let libraryStore = RawStore(core: core, adopting: library)
        defer { libraryStore.close() }
        libraryStore.observe()
        let books = try require(libraryStore.entries(of: 0).first, "the books entry in the answer to the observe of the library")
        try checkEqual(books.op, .fullValue, "the form of the books entry")
        let lazy = try books.decode(UndraLazyValue.self)
        try checkEqual(lazy.len, 10_000, "the length in the LazyValue of books")
        var page = UndraReader(try core.callSync(.lazyListPage(handle: lazy.handle, offset: 0, limit: 1), method: 0, args: []))
        let header = try UndraLazyPageHeader.undraDecode(&page)
        try checkEqual(header.total, 10_000, "the total in the reply of a page call through the new page server")
        try checkEqual(header.count, 1, "the rows in that reply")

        // The handle of `roster` was refused: status 5, and a WARN names it.
        do {
            _ = try refetch(roster)
            throw ScenarioFailure(description: "refetch on the roster handle was accepted, but build B changed the parameter's type")
        } catch let error as UndraReplyError {
            try checkEqual(error.status, .badRequest, "the status of refetch on the roster handle")
        }
        let named = "Handle(index=\(roster.index), gen=\(roster.generation))"
        let warnings = Array(log.all.dropFirst(logBefore)).filter { $0.level == 3 }.map(\.message)
        try check(
            warnings.contains { $0.contains(named) && $0.contains("is not re-issued") && $0.contains("the types it was made from changed") },
            "no WARN names \(named) as not re-issued because the types it was made from changed: \(warnings)"
        )
        try checkEqual(warnings.filter { $0.contains("is not re-issued") }.count, 1, "WARNs about a handle not re-issued: \(warnings)")
        MigrationBuildB.report("MIGRATION S35 build B ok")
    }

    // MARK: Raw calls

    private func storageStatus(_ core: UndraCore) throws -> StorageStatus {
        let body = try core.callSync(.freeFunction(methodId: Ids.storageStatus), method: Ids.storageStatus, args: [])
        return try StorageStatus.undraDecoded(from: body)
    }

    private func describe(_ core: UndraCore, _ handle: UndraHandle) throws -> String {
        let body = try core.callSync(
            .objectMethod(handle: handle, methodId: Ids.profileDescribe),
            method: Ids.profileDescribe,
            args: []
        )
        return try String.undraDecoded(from: body)
    }

    private func add(_ core: UndraCore, _ a: Int32, _ b: Int32) throws -> Int32 {
        let body = try core.callSync(
            .freeFunction(methodId: Ids.add),
            method: Ids.add,
            args: encoded { (w: inout UndraWriter) -> Void in
                w.writeI32(a)
                w.writeI32(b)
            }
        )
        return try Int32.undraDecoded(from: body)
    }

    // MARK: Reporting

    /// Runs the build-B steps of one scenario; prints `SCENARIO <id> FAIL ...` if they fail, nothing otherwise.
    private func step(_ id: String, _ body: @MainActor () async throws -> Void) async {
        do {
            try await body()
        } catch {
            fail(id, "\(error)")
        }
    }

    private func fail(_ id: String, _ reason: String) {
        let title: String
        switch id {
        case "S14": title = "offline queue replay"
        case "S15": title = "snapshot and restore"
        default: title = "query handles across a restore"
        }
        let line = "build B: " + reason.replacingOccurrences(of: "\n", with: " ")
        MigrationBuildB.report("SCENARIO \(id) FAIL \(title): \(line)")
        XCTFail("\(id) \(line)")
    }

    private nonisolated static func report(_ line: String) {
        print(line)
        fflush(stdout)
    }
}
