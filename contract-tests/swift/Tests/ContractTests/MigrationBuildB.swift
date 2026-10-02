import Foundation
import UndraFFI
@testable import UndraRuntime
import PlaygroundCore
import PlaygroundCoreFFI
import XCTest

/// The build-B process of the two-build steps (scenarios.md, "Two builds"; S14 steps 8 and 9, S15 steps
/// 12 to 14).
///
/// `run.sh` runs it after the main run, in a second `swift test` process (`--skip-build --filter
/// MigrationBuildB`, with `UNDRA_CONTRACT_PHASE=B`) whose `.build/core/libplayground_core.dylib` is build B's
/// library. Build B has no generated bindings: the test loads the core with the hash it reports
/// (its table's `schema_hash`) and drives it through `UndraCore`'s raw API with ids made by `fnv1a32`
/// (the generated `StorageStatus` is used only as a codec: its layout is the same in both builds). It
/// reads what build A handed over (`Handover`) and prints only `SCENARIO S14 FAIL ...` /
/// `SCENARIO S15 FAIL ...` lines (and an informational `MIGRATION ... ok`), so `check.sh`, which reads
/// the last line of an id, keeps build A's `PASS` unless build B fails. In the main run it is skipped.
@MainActor
final class MigrationBuildB: XCTestCase {
    private enum Ids {
        static let configureRemote = fnv1a32("fn.configure_remote")
        static let storageStatus = fnv1a32("fn.storage_status")
        static let add = fnv1a32("fn.add")
        static let profileDescribe = fnv1a32("Profile.describe")
    }

    private static let notes = "/lists/s14m/notes"

    func testBuildB() async throws {
        guard ProcessInfo.processInfo.environment["UNDRA_CONTRACT_PHASE"] == "B" else {
            throw XCTSkip("the build-B phase runs in its own process (run.sh sets UNDRA_CONTRACT_PHASE=B)")
        }
        // Build B has build A's namespace, so its table is the same export (ADR-044).
        let reported = playgroundTable().schema_hash
        guard reported != UndraIds.schemaHash else {
            fail("S14", "the core in .build/core is build A (schema hash 0x\(Persisted.hex(reported, digits: 16))), not build B")
            fail("S15", "the core in .build/core is build A, not build B")
            return
        }
        let queue: Handover.Queue?
        let snapshots: Handover.Snapshots?
        do {
            queue = try Handover.read("s14", as: Handover.Queue.self)
            snapshots = try Handover.read("s15", as: Handover.Snapshots.self)
        } catch {
            fail("S14", "the handover from build A does not read: \(error)")
            fail("S15", "the handover from build A does not read: \(error)")
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
        let title = id == "S14" ? "offline queue replay" : "snapshot and restore"
        let line = "build B: " + reason.replacingOccurrences(of: "\n", with: " ")
        MigrationBuildB.report("SCENARIO \(id) FAIL \(title): \(line)")
        XCTFail("\(id) \(line)")
    }

    private nonisolated static func report(_ line: String) {
        print(line)
        fflush(stdout)
    }
}
