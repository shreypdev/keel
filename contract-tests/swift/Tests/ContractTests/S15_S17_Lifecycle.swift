import Foundation
import UndraFFI
@testable import UndraRuntime
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
            let ghost = try core.construct(type: UndraIds.Objects.Probe.typeId, method: UndraIds.Objects.Probe.new, args: [])
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
            try check(UndraUUID(d.id) > UndraUUID(b.id), "the id of d is not above the id of b")
            _ = c
            try await waitUntil("d to show") { todos.todos.count == 3 }

            // 6. A rejected snapshot fails and leaves every store as it was.
            let titlesBefore = todos.todos.map(\.title)
            let countBefore = counter.count
            let garbage = (0 ..< 16).map { _ in UInt8.random(in: 0 ... 255) }
            do {
                try core.restore(garbage)
                throw ScenarioFailure(description: "16 random bytes were accepted as a snapshot")
            } catch let error as UndraRestoreError {
                try check(error.code != 0, "the restore error carries code 0")
            }
            try await quietFor(milliseconds: 100)
            try checkEqual(todos.todos.map(\.title), titlesBefore, "todos after a rejected restore")
            try checkEqual(counter.count, countBefore, "count after a rejected restore")

            // 7. The handle released before the snapshot was not resurrected.
            do {
                _ = try core.callSync(
                    .objectMethod(handle: ghost, methodId: UndraIds.Objects.Probe.counters),
                    method: UndraIds.Objects.Probe.counters,
                    args: []
                )
                throw ScenarioFailure(description: "a handle released before the snapshot answers a call after the restore")
            } catch let error as UndraReplyError {
                try checkEqual(error.status, .badRequest, "status of a call on the released handle")
            }

            // 8. The restore created no handles of its own: exactly the three stores are alive.
            try checkEqual(core.stat("live_handles"), liveBefore + 3, "live_handles after the restore")

            // 9. A call in flight across a restore ends as cancelled by the core (not as a platform
            // cancellation), and the invalidated object then refuses calls through the bindings.
            let probe = try Probe(ctx: core)
            defer { probe.close() }
            let hanging = Task { try await probe.hang() }
            try await waitUntil("the hanging call to start") { try probe.counters().started == 1 }
            let reportsBefore = Fixture.shared.unhandled.snapshot.count
            try core.restore(try core.snapshot())
            switch await hanging.result {
            case .success(let value):
                throw ScenarioFailure(description: "hang() returned \(value) across a restore")
            case .failure(let error):
                try check(!(error is CancellationError), "a call cancelled by the core failed as a CancellationError")
                try checkEqual(error as? UndraCallError, .cancelledByCore, "the error of hang() across a restore")
            }
            let stale = try callError("probe.counters() after the restore") { try probe.counters() }
            guard case .refused = stale else {
                throw ScenarioFailure(description: "probe.counters() after the restore failed with \(stale), not .refused")
            }
            probe.reset()
            let reports = Array(Fixture.shared.unhandled.snapshot.dropFirst(reportsBefore))
            try checkEqual(reports.count, 1, "reports to onError for Probe.reset after the restore")
            let report = try require(reports.first, "the report of Probe.reset")
            try checkEqual(report.operation, "Probe.reset", "the operation of the report")
            guard case .refused = report.error else {
                throw ScenarioFailure(description: "Probe.reset after the restore reported \(report.error), not .refused")
            }

            // 10. A stream in flight across a restore ends as cancelled by the core: the core's own "cancelled: ..."
            // String is not read as a typed error, and the loop's end is not a platform cancellation.
            let streamed = try Probe(ctx: core)
            defer { streamed.close() }
            var ticks = streamed.ticks(count: 1_000_000).makeAsyncIterator()
            _ = try await ticks.next()
            try core.restore(try core.snapshot())
            var streamOutcome: (any Error)?
            do {
                while try await ticks.next() != nil {}
            } catch {
                streamOutcome = error
            }
            let ended = try require(streamOutcome, "the stream ended normally across a restore")
            try check(!(ended is CancellationError), "a stream cancelled by the core ended as a CancellationError")
            try checkEqual(ended as? UndraCallError, .cancelledByCore, "the error of ticks() across a restore")
        }
    }

    // MARK: S16

    func testS16_schemaMismatchRejection() async {
        await scenario("S16", "schema mismatch rejection") {
            // The one core of this process is shut down first: the scenario needs a process in which
            // no core is initialised (`undra_init` is once per process).
            Fixture.shared.shutDown()
            try check(UndraCore.current == nil, "a core is still loaded after shutdown")
            let generated = UndraIds.schemaHash
            let wrong = generated ^ 1

            // 1. A load that expects another schema fails with the runtime's mismatch error.
            do {
                _ = try UndraCore.load(.inproc(adapters: Fixture.shared.makeAdapters(), expectedSchemaHash: wrong))
                throw ScenarioFailure(description: "a load with the wrong schema hash succeeded")
            } catch let error as UndraSchemaMismatchError {
                try checkEqual(error.expected, wrong, "the expected hash in the error")
                try checkEqual(error.got, generated, "the core's hash in the error")
                let message = "\(error)"
                try check(message.contains(ContractScenarios.hex(wrong)) && message.contains(ContractScenarios.hex(generated)),
                          "the message names both hashes in hex: \(message)")
            }
            try check(UndraCore.current == nil, "a failed load left a shared core behind")

            // 1a. With no core loaded `UndraCore.shared` does not trap: it is a closed placeholder, so a generated
            // call and a generated constructor with the default core fail as unavailable, and a failure reported on
            // the placeholder only logs.
            let placeholder = UndraCore.shared
            try check(UndraCore.current == nil, "UndraCore.shared became a loaded core")
            try checkThrows({ try PlaygroundCore.add(a: 1, b: 2) }, UndraCallError.unavailable(.closed), "add(1, 2) with no core loaded")
            try checkThrows({ try Counter() }, UndraCallError.unavailable(.closed), "Counter() with no core loaded")
            placeholder.report(UndraTransportError.closed, operation: "Counter.increment")
            try check(UndraCore.current == nil, "the placeholder became the loaded core after a report")

            // 1b. "Before the core is initialised" is what the error type shows when something else
            // already initialised it: `undra_init` would be refused, and a runtime that called it
            // before comparing the hashes would fail with `coreInitFailed` and never report the
            // mismatch. The check comes first, so the answer is the same.
            do {
                let foreignInit = ContractScenarios.initialiseCoreElsewhere()
                try checkEqual(foreignInit, 0, "undra_init by another embedder")
                defer { undra_shutdown() }
                do {
                    _ = try UndraCore.load(.inproc(adapters: Fixture.shared.makeAdapters(), expectedSchemaHash: wrong))
                    throw ScenarioFailure(description: "a load with the wrong schema hash succeeded on an initialised core")
                } catch let error as UndraSchemaMismatchError {
                    try checkEqual(error.got, generated, "the core's hash in the error, core initialised elsewhere")
                }
            }

            // 2. The failed attempt did not leave the process half-initialised: a load with the right hash works.
            let core = try Fixture.shared.core()
            try checkEqual(try PlaygroundCore.add(a: 20, b: 22, ctx: core), 42, "a call on the core loaded after the failed attempt")

            // 3. The hash of the bindings, of the statistics and of the exported schema agree.
            try checkEqual(core.schemaHash, generated, "UndraCore.schemaHash")
            let statistics = try JSONSerialization.jsonObject(with: Data(core.stats().json.utf8)) as? [String: Any]
            let reported = try require(statistics?["schema_hash"] as? String, "schema_hash in the statistics")
            try checkEqual(UInt64(reported.dropFirst(2), radix: 16), generated, "stats().schema_hash")
            try checkEqual(undra_schema_hash(), generated, "undra_schema_hash()")

            // 4. The exported schema lists the playground's types and the standard ports.
            let exported = undra_schema_json()
            defer { undra_buf_free(exported) }
            let json = Data(bytes: try require(exported.ptr, "undra_schema_json output"), count: Int(exported.len))
            let names = ContractScenarios.names(in: try JSONSerialization.jsonObject(with: json))
            for expected in ["Todos", "Counter", "BigList", "Bench", "Probe", "remote_todos", "post_remote_todo", "patch_remote_todo",
                             "Clock", "Rng", "Log", "Http", "Kv", "SecureStore", "Fs", "Timer", "Connectivity", "Lifecycle"] {
                try check(names.contains(expected), "the exported schema does not list \(expected)")
            }
        }
    }

    /// `undra_init` as another embedder of the core would call it, without the runtime; returns its
    /// status code (0 when the core is running).
    private static func initialiseCoreElsewhere() -> UInt32 {
        let config = RuntimeConfigRecord(platform: "macos", mode: "inproc", coreThreads: 1, blockingThreads: 0, logLevel: 2).undraEncoded()
        return config.withUnsafeBufferPointer { (bytes: UnsafeBufferPointer<UInt8>) -> UInt32 in
            return undra_init(bytes.baseAddress, UInt32(bytes.count), { _, _, _, _ in }, { _, _, _ in }, { _, _, _, _ in }, nil)
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

            // 1. A panic in a sync call is a reply with status panic, not a crash: the generated
            // `explode(reason:)` throws `UndraCallError.panicked` and the process lives (ADR-032).
            let exploded = try callError("explode(\"kaboom\")") { try explode(reason: "kaboom", ctx: core) }
            guard case .panicked(let kaboom, _) = exploded else {
                throw ScenarioFailure(description: "explode(\"kaboom\") failed with \(exploded), not .panicked")
            }
            try check(kaboom.contains("kaboom"), "the panic message names the reason: \(kaboom)")

            // 2. So is a panic in an async call.
            let explodedLater = try await callError("explode_later(10, \"later\")") {
                try await explodeLater(delayMs: 10, reason: "later", ctx: core)
            }
            guard case .panicked(let later, _) = explodedLater else {
                throw ScenarioFailure(description: "explode_later failed with \(explodedLater), not .panicked")
            }
            try check(later.contains("later"), "the panic message names the reason: \(later)")

            // 3. The core keeps working, a store built before still updates, two panics were counted.
            try checkEqual(try PlaygroundCore.add(a: 1, b: 2, ctx: core), 3, "add(1, 2) after the panics")
            store.add(amount: 1)
            try await waitUntil("the store built before the panics to update") { store.count == 1 }
            try checkEqual(core.stat("panics") - panicsBefore, 2, "panics delta")

            // 4. The Log port got an error-or-worse record from `undra::panic` for each.
            try await waitUntil("two panic records") {
                log.all.dropFirst(recordsBefore).filter { $0.level >= 4 && $0.target == "undra::panic" }.count >= 2
            }
            let records = log.all.dropFirst(recordsBefore).filter { $0.level >= 4 && $0.target == "undra::panic" }
            try check(records.contains { $0.message.contains("kaboom") }, "no panic record names kaboom: \(records)")
            try check(records.contains { $0.message.contains("later") }, "no panic record names later: \(records)")

            // 5. Re-entry is refused, not deadlocked or aborted. The core logs the panic of
            // `explode("reenter")` through the runner's Log adapter, a synchronous port that runs
            // on the thread that holds the core's lock; the adapter calls the generated `add(1, 1)`
            // once from there.
            let reentered = Locked<Result<Int32, any Error>?>(nil)
            log.onNextRecord(
                where: { level, target, message in level >= 4 && target == "undra::panic" && message.contains("reenter") },
                run: {
                    let result = Result { try PlaygroundCore.add(a: 1, b: 1, ctx: core) }
                    reentered.withLock { (current: inout Result<Int32, any Error>?) -> Void in current = result }
                }
            )
            let reenter = try callError("explode(\"reenter\")") { try explode(reason: "reenter", ctx: core) }
            guard case .panicked(let reenterMessage, _) = reenter, reenterMessage.contains("reenter") else {
                throw ScenarioFailure(description: "explode(\"reenter\") failed with \(reenter), not .panicked")
            }
            switch try require(reentered.snapshot, "the Log adapter ran while the core logged the panic") {
            case .success(let value):
                throw ScenarioFailure(description: "add(1, 1) from inside the Log port returned \(value)")
            case .failure(let error):
                guard case .refused(let reason)? = error as? UndraCallError else {
                    throw ScenarioFailure(description: "add(1, 1) from inside the Log port failed with \(error), not .refused")
                }
                try check(reason.contains("E_REENTRANT"), "the refusal names E_REENTRANT: \(reason)")
            }
            try checkEqual(try PlaygroundCore.add(a: 1, b: 2, ctx: core), 3, "add(1, 2) after the re-entrant call")

            // 6. Shutdown with a typed call in flight. This step ends the core, so it is the last of
            // the run (S17 is the last scenario XCTest runs, and no later step uses the core).
            let inFlight = Task { try await failLater(delayMs: 5_000, code: 1, ctx: core) }
            try await quietFor(milliseconds: 100)
            let reportsBefore = Fixture.shared.unhandled.snapshot.count
            let shutdownAt = ContinuousClock.now
            core.shutdown()
            switch await inFlight.result {
            case .success(let value):
                throw ScenarioFailure(description: "fail_later returned \(value) across a shutdown")
            case .failure(let error):
                try checkEqual(error as? UndraCallError, .unavailable(.closed), "the error of fail_later across a shutdown")
            }
            try check(ContinuousClock.now - shutdownAt < .seconds(1), "the call in flight took \(ContinuousClock.now - shutdownAt) to fail")
            try checkThrows({ try PlaygroundCore.add(a: 1, b: 2, ctx: core) }, UndraCallError.unavailable(.closed), "add(1, 2) on the shut-down core")
            store.increment()
            let reports = Array(Fixture.shared.unhandled.snapshot.dropFirst(reportsBefore))
            try checkEqual(reports.count, 1, "reports to onError for Counter.increment on the shut-down core")
            let report = try require(reports.first, "the report of Counter.increment")
            try checkEqual(report.operation, "Counter.increment", "the operation of the report")
            try checkEqual(report.error, UndraCallError.unavailable(.closed), "the reason of the report")
            // The shut-down core is no longer the shared one: a constructor with the default core fails as in S16.
            try check(UndraCore.current == nil, "the shut-down core is still UndraCore.current")
            try checkThrows({ try Counter() }, UndraCallError.unavailable(.closed), "Counter() after the shared core was shut down")
        }
    }
}
