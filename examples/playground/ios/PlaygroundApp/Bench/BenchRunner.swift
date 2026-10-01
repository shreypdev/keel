import Foundation
import UndraRuntime
import PlaygroundCore
#if canImport(UIKit)
import UIKit
#endif

/// How much of a run each part gets.
struct BenchConfig {
    /// `sync_call`: operations per timed batch, timed batches, and batches thrown away first. The call is tens of nanoseconds
    /// to a few microseconds, which one read of the clock (41.67 ns a tick on Apple silicon) cannot time alone.
    var syncBatch = 1_000
    var syncBatches = 200
    var warmupBatches = 20
    /// The other rows are timed one operation at a time: samples, and operations thrown away first.
    var eachSamples = 2_000
    var eachWarmup = 200
    /// The list is put back to its 10,000 rows after this many inserts (outside the timed region).
    var resetEveryInserts = 500
    /// The ADR-031 drain experiment: frames that count, and frames before them that do not.
    var drainFrames = 240
    var drainWarmupFrames = 60
    /// After each frame of the drain experiment, this many calls on the main thread, each drained alone: the cost of an update that is
    /// not merged with any other, measured in the same minutes as the merged frames.
    var singlesPerRound = 8
    /// Reloads of the core inside one process, for the in-process cold-start row.
    var reloads = 30
    /// The 100 KB snapshot: this many to-dos of this many characters each.
    var snapshotTodos = 1_000
    var snapshotTitleLength = 80

    static let full = BenchConfig()
    static let quick = BenchConfig(
        syncBatch: 20, syncBatches: 4, warmupBatches: 1, eachSamples: 20, eachWarmup: 2, resetEveryInserts: 10,
        drainFrames: 4, drainWarmupFrames: 1, singlesPerRound: 2, reloads: 2, snapshotTodos: 50, snapshotTitleLength: 80
    )
}

/// The iOS half of the device benchmark: the blueprint's section 14 operations, measured the way the app
/// experiences them, through the generated `Bench` store and the runtime's mirror, on the main actor. The XCUITest
/// `PlaygroundBenchTests` launches the app with `-bench full` (everything) or `-bench cold` (one cold start) and
/// reads the JSON this runner shows in `BenchScreen`; `scripts/bench-device.sh --device ios` wraps both.
///
/// Every row is checked: after each operation the mirror's copy of the state is compared with what the operation did,
/// so a number is never reported for a run in which the apply did not happen.
@MainActor
final class BenchRunner {
    /// Where the 100 KB snapshot of the full run waits for the cold-start launches.
    static var snapshotURL: URL {
        return FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("undra-bench-snapshot.bin")
    }

    private let config: BenchConfig
    private var notes: [String] = []

    init(config: BenchConfig) {
        self.config = config
    }

    // MARK: Loading the core

    /// Loads the linked core with the platform's defaults, as the app does, except for the two ports a benchmark does
    /// not want to wake: connectivity (a path monitor) and the on-disk key-value store, which is emptied.
    static func loadCore() throws -> UndraCore {
        let store = FileManager.default.temporaryDirectory.appendingPathComponent("undra-bench-kv", isDirectory: true)
        try? FileManager.default.removeItem(at: store)
        let adapters = Adapters.platformDefault
            .removing(portId: fnv1a32("port.Connectivity"))
            .replacing(KvAdapter(directory: store))
        return try UndraCore.load(.inproc(adapters: adapters, expectedSchemaHash: UndraIds.schemaHash, onError: { _ in }))
    }

    // MARK: The full run

    /// Runs everything and returns the runner's half of the result file (`undra-device-bench-raw/1`).
    func runFull() async throws -> [String: Any] {
        let timer = BenchClock.facts()
        let firstLoadStart = BenchClock.now()
        let core = try BenchRunner.loadCore()
        let firstLoadNs = Double(BenchClock.now() - firstLoadStart)
        let bench = try Bench(ctx: core)
        if bench.rows.count != Int(BenchRunner.rows) {
            throw BenchCheckError(message: "the bench list has \(bench.rows.count) rows after observing, not \(BenchRunner.rows)")
        }

        var ops: [[String: Any]] = []
        ops.append(try measureSyncCall(bench))
        ops.append(try measureRecord(bench))
        ops.append(try measureInsert(bench))
        ops.append(try measureChangeSet(bench))
        let drain = try await measureDrain(core: core, bench: bench)
        bench.close()
        let cold = try await measureReloads(core: core, firstLoadNs: firstLoadNs)

        return [
            "schema": "undra-device-bench-raw/1",
            "platform": "ios",
            "runtime": "inproc (C ABI, linked static library)",
            "device": deviceFacts(),
            "timer": ["kind": "clock_gettime_nsec_np(CLOCK_UPTIME_RAW)", "resolution_ns": timer.resolutionNs, "overhead_ns": timer.overheadNs],
            "config": [
                "sync_batch": config.syncBatch, "sync_batches": config.syncBatches, "warmup_batches": config.warmupBatches,
                "each_samples": config.eachSamples, "each_warmup": config.eachWarmup, "reset_every_inserts": config.resetEveryInserts,
                "drain_frames": config.drainFrames, "drain_warmup_frames": config.drainWarmupFrames, "singles_per_round": config.singlesPerRound,
                "reloads": config.reloads,
            ],
            "ops": ops,
            "cold": cold,
            "drain": drain,
            "notes": notes,
        ]
    }

    /// One cold start in a fresh process: the first `UndraCore.load` of the process, then the restore of the snapshot the
    /// full run left behind.
    func runCold() throws -> [String: Any] {
        let t0 = BenchClock.now()
        let core = try BenchRunner.loadCore()
        let t1 = BenchClock.now()
        let snapshot = [UInt8](try Data(contentsOf: BenchRunner.snapshotURL))
        let t2 = BenchClock.now()
        try core.restore(snapshot)
        let t3 = BenchClock.now()
        let stores = core.stats().coreLiveStores
        if stores != 1 { throw BenchCheckError(message: "the restore left \(stores) live stores, not 1") }
        return [
            "schema": "undra-device-bench-cold/1",
            "load_ns": Double(t1 - t0),
            "restore_ns": Double(t3 - t2),
            "snapshot_bytes": snapshot.count,
        ]
    }

    // MARK: The rows

    static let rows: UInt32 = 10_000
    static let insertAt: UInt32 = 5_000

    private func measureSyncCall(_ bench: Bench) throws -> [String: Any] {
        let size = config.syncBatch
        var perOp: [Double] = []
        var sum: UInt64 = 0
        var expected: UInt64 = 0
        for batch in 0 ..< (config.warmupBatches + config.syncBatches) {
            let t0 = BenchClock.now()
            for i in 0 ..< size {
                sum &+= UInt64(try bench.benchAdd(a: UInt32(truncatingIfNeeded: i & 0xffff), b: 3))
            }
            let t1 = BenchClock.now()
            for i in 0 ..< size {
                expected &+= UInt64((i & 0xffff) + 3)
            }
            if sum != expected { throw BenchCheckError(message: "benchAdd answered \(sum), not \(expected)") }
            if batch >= config.warmupBatches { perOp.append(Double(t1 - t0) / Double(size)) }
        }
        return benchOp(id: "sync_call", batch: size, perOpNs: perOp, note: "`try bench.benchAdd(a:b:)` on the main actor: encode, `callSync` through the C ABI, decode, through the generated binding")
    }

    private func measureRecord(_ bench: Bench) throws -> [String: Any] {
        let payload = (0 ..< 1_024).map { UInt8(truncatingIfNeeded: $0 &* 31 &+ 7) }
        var perOp: [Double] = []
        for i in 0 ..< (config.eachWarmup + config.eachSamples) {
            let t0 = BenchClock.now()
            let back = try bench.benchEchoBytes(data: payload)
            let t1 = BenchClock.now()
            if back.count != payload.count || back.last != payload.last { throw BenchCheckError(message: "benchEchoBytes returned another payload") }
            if i >= config.eachWarmup { perOp.append(Double(t1 - t0)) }
        }
        return benchOp(id: "record_1kb", batch: 1, perOpNs: perOp, note: "`try bench.benchEchoBytes(data:)` with 1,024 bytes: the payload crosses the boundary twice (a byte string, which is cheaper than a record of the same size)")
    }

    private func measureInsert(_ bench: Bench) throws -> [String: Any] {
        bench.benchListReset()
        var perOp: [Double] = []
        var inserted = 0
        for i in 0 ..< (config.eachWarmup + config.eachSamples) {
            if inserted >= config.resetEveryInserts {
                bench.benchListReset()
                inserted = 0
            }
            let t0 = BenchClock.now()
            bench.benchListInsert(i: BenchRunner.insertAt)
            let t1 = BenchClock.now()
            inserted += 1
            if bench.rows.count != Int(BenchRunner.rows) + inserted {
                throw BenchCheckError(message: "the mirror holds \(bench.rows.count) rows after \(inserted) inserts")
            }
            if bench.rows[Int(BenchRunner.insertAt)].id <= BenchRunner.rows {
                throw BenchCheckError(message: "the newest insert is not at the insert position in the mirror")
            }
            if i >= config.eachWarmup { perOp.append(Double(t1 - t0)) }
        }
        return benchOp(id: "keyed_insert_10k", batch: 1, perOpNs: perOp, note: "`bench.benchListInsert(i: 5000)` on about 10,000 rows: the call, the core's recorded insert, the one-operation patch, applied to the mirror's list before the call returns")
    }

    private func measureChangeSet(_ bench: Bench) throws -> [String: Any] {
        bench.benchListReset()
        var perOp: [Double] = []
        var touched: UInt32 = 0
        for i in 0 ..< (config.eachWarmup + config.eachSamples) {
            let t0 = BenchClock.now()
            bench.benchTouchSignals(k: 100)
            let t1 = BenchClock.now()
            touched += 1
            if bench.s099 != touched { throw BenchCheckError(message: "the mirror's hundredth counter is \(bench.s099) after \(touched) touches") }
            if i >= config.eachWarmup { perOp.append(Double(t1 - t0)) }
        }
        return benchOp(id: "changeset_100", batch: 1, perOpNs: perOp, note: "`bench.benchTouchSignals(k: 100)`: one call, one change-set of 100 entries, applied to 100 `@Observable` properties of the mirror before the call returns")
    }

    // MARK: The ADR-031 drain

    private static let updatesPerFrame = 1_667

    private func measureDrain(core: UndraCore, bench: Bench) async throws -> [String: Any] {
        bench.benchListReset()
        let recorder = DrainRecorder()
        let registration = core.mirror.addDrainListener { stats in recorder.record(stats) }
        defer { registration.remove() }

        // A burst of 1,667 one-update patches per frame, committed by the core on a thread of its own, the way a socket or a
        // timer-driven core would: the main thread meets them only as the drain at the next frame (`CADisplayLink`). The
        // call goes through the core directly, because the store's own methods belong to the main actor.
        let handle = bench.handle
        let methodId = UndraIds.Objects.Bench.benchListUpdateBurst
        var w = UndraWriter()
        UInt32(BenchRunner.updatesPerFrame).undraEncode(&w)
        let args = w.finish()
        let rounds = config.drainWarmupFrames + config.drainFrames
        let warmup = config.drainWarmupFrames
        let perBurst = BenchRunner.updatesPerFrame

        struct Burst: Sendable {
            var frameNs: Double
            var drains: Int
            var changeSets: Int
            var entries: Int
            var applied: Int
        }
        var bursts: [Burst] = []
        var singles: [Double] = []
        for round in 1 ... rounds {
            let burst: Burst = try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Burst, Error>) in
                Thread.detachNewThread {
                    do {
                        let start = recorder.position
                        _ = try core.callSync(.objectMethod(handle: handle, methodId: methodId), method: methodId, args: args)
                        guard recorder.wait(forChangeSets: start.changeSets + perBurst, timeout: 10) else {
                            throw BenchCheckError(message: "burst \(round): the main thread did not drain \(perBurst) change-sets within 10 s")
                        }
                        let drains = recorder.drains(since: start.drains)
                        continuation.resume(returning: Burst(
                            frameNs: drains.reduce(0) { $0 + $1.durationNs }, drains: drains.count,
                            changeSets: drains.reduce(0) { $0 + $1.changeSets }, entries: drains.reduce(0) { $0 + $1.entries },
                            applied: drains.reduce(0) { $0 + $1.applied }
                        ))
                    } catch {
                        continuation.resume(throwing: error)
                    }
                }
            }
            // The cost of one update on its own, in the same minutes as the frames: a call on the main thread drains before it
            // returns, so each of these is a drain of exactly one entry, and the drain listener times it.
            let mark = recorder.position.drains
            for _ in 0 ..< config.singlesPerRound {
                bench.benchListUpdateBurst(n: 1)
            }
            let alone = recorder.drains(since: mark).filter { $0.changeSets == 1 && $0.entries == 1 && $0.applied == 1 }
            if alone.count != config.singlesPerRound {
                throw BenchCheckError(message: "round \(round): \(alone.count) of \(config.singlesPerRound) calls were drained as one entry on their own")
            }
            if round > warmup {
                bursts.append(burst)
                singles.append(contentsOf: alone.map(\.durationNs))
            }
        }
        if bench.rows.count != Int(BenchRunner.rows) {
            throw BenchCheckError(message: "the update bursts changed the list's length to \(bench.rows.count)")
        }
        if let wrong = bursts.first(where: { $0.changeSets != perBurst }) {
            throw BenchCheckError(message: "a frame's drains consumed \(wrong.changeSets) change-sets, not \(perBurst)")
        }
        let drainDurations = recorder.drains(since: 0).filter { $0.changeSets > 1 }.map(\.durationNs)
        let perEntry = BenchSummary(samples: singles)
        let merged = BenchSummary(samples: bursts.map(\.frameNs))
        func median(_ xs: [Int]) -> Double { return BenchSummary(samples: xs.map(Double.init)).p50 }

        return [
            "rows": Int(BenchRunner.rows),
            "updates_per_frame": perBurst,
            "frames": config.drainFrames,
            "warmup_frames": config.drainWarmupFrames,
            "producer": "a thread of its own commits the burst (the core's side of a socket or a timer): the main thread's cost of a frame is the drain at the next frame, as `DrainStats.duration` times it (`ContinuousClock`)",
            "merged": [
                "frame_ns": merged.json,
                "drain_ns": BenchSummary(samples: drainDurations).json,
                "change_sets_per_frame_p50": median(bursts.map(\.changeSets)),
                "entries_per_frame_p50": median(bursts.map(\.entries)),
                "applied_per_frame_p50": median(bursts.map(\.applied)),
                "drains_per_frame_p50": median(bursts.map(\.drains)),
            ] as [String: Any],
            "unmerged_estimate": [
                "per_entry_ns": perEntry.json,
                "frame_ns": Double(perBurst) * perEntry.p50,
                "frame_ns_mean": Double(perBurst) * perEntry.mean,
                "method": "the drain of a single entry (a call on the main thread drains before it returns, so the drain listener times a drain of exactly one change-set of one entry applied on its own), \(config.singlesPerRound) after each frame of the experiment so that both are measured in the same minutes (\(perEntry.n) in all); times \(perBurst), by the median entry and by the mean entry. The runtime was not reverted: this is what applying every entry on its own would cost, from the entry cost measured here",
            ] as [String: Any],
            "ratio_unmerged_over_merged": Double(perBurst) * perEntry.p50 / merged.p50,
            "ratio_unmerged_over_merged_mean": Double(perBurst) * perEntry.mean / merged.p50,
            "note": "merged frame = the drain(s) that consumed one burst, summed: decode and apply of the merged patch to the `@Observable` store",
        ]
    }

    // MARK: Cold start

    /// Restores a 100 KB snapshot into a fresh core, over and over in this process (a warm reload; the cold one is the
    /// first load of a process, which `-bench cold` measures in a launch of its own).
    private func measureReloads(core firstCore: UndraCore, firstLoadNs: Double) async throws -> [String: Any] {
        // A to-do list of about 100 KB: the bench and big-list stores are gone, so the snapshot holds only this.
        var todos: Todos? = try Todos(ctx: firstCore)
        for i in 0 ..< config.snapshotTodos {
            let title = String(format: "todo %05d ", i).padding(toLength: config.snapshotTitleLength, withPad: "x", startingAt: 0)
            _ = try await todos!.add(title: title)
        }
        let snapshot = try firstCore.snapshot()
        try snapshot.withUnsafeBytes { try Data($0).write(to: BenchRunner.snapshotURL, options: .atomic) }
        todos = nil

        var core = firstCore
        var loads: [Double] = []
        var restores: [Double] = []
        for _ in 0 ..< config.reloads {
            core.shutdown()
            let t0 = BenchClock.now()
            core = try BenchRunner.loadCore()
            let t1 = BenchClock.now()
            try core.restore(snapshot)
            let t2 = BenchClock.now()
            let stores = core.stats().coreLiveStores
            if stores != 1 { throw BenchCheckError(message: "the restore left \(stores) live stores, not 1") }
            loads.append(Double(t1 - t0))
            restores.append(Double(t2 - t1))
        }
        core.shutdown()
        if abs(Double(snapshot.count) - 100_000) > 10_000 {
            notes.append("the snapshot is \(snapshot.count) bytes, not about 100,000")
        }
        return [
            "snapshot_bytes": snapshot.count,
            "first_load_in_run_ns": firstLoadNs,
            "launches": [] as [[String: Any]],
            "reload_in_process": ["load_ns": BenchSummary(samples: loads).json, "restore_ns": BenchSummary(samples: restores).json],
            "note": "a 100 KB snapshot is \(config.snapshotTodos) to-dos of \(config.snapshotTitleLength) characters (the host row restores four stores of 250 rows of 100 bytes); load = `UndraCore.load` of the linked core, which starts the runtime and its core thread; the C library is statically linked, so there is no dlopen",
        ]
    }

    // MARK: Facts

    private func deviceFacts() -> [String: Any] {
        var facts: [String: Any] = [:]
        #if targetEnvironment(simulator)
        facts["model"] = (ProcessInfo.processInfo.environment["SIMULATOR_MODEL_IDENTIFIER"] ?? "unknown") + " (simulated)"
        facts["is_virtual"] = true
        #else
        var info = utsname()
        uname(&info)
        let machine = withUnsafePointer(to: &info.machine) {
            $0.withMemoryRebound(to: CChar.self, capacity: 1) { String(cString: $0) }
        }
        facts["model"] = machine
        facts["is_virtual"] = false
        #endif
        #if canImport(UIKit)
        facts["os"] = "\(UIDevice.current.systemName) \(UIDevice.current.systemVersion)"
        #endif
        #if arch(arm64)
        facts["arch"] = "arm64"
        #elseif arch(x86_64)
        facts["arch"] = "x86_64"
        #endif
        facts["cores"] = ProcessInfo.processInfo.activeProcessorCount
        facts["thermal_state"] = ["nominal", "fair", "serious", "critical"][min(3, ProcessInfo.processInfo.thermalState.rawValue)]
        facts["low_power_mode"] = ProcessInfo.processInfo.isLowPowerModeEnabled
        facts["physical_memory_bytes"] = Int(ProcessInfo.processInfo.physicalMemory)
        #if DEBUG
        facts["swift_configuration"] = "Debug"
        #else
        facts["swift_configuration"] = "Release"
        #endif
        return facts
    }
}

/// The JSON of a result as one line; the numbers that cannot be written (the summary of nothing) are written as `null`.
func benchJSON(_ object: [String: Any]) throws -> String {
    func clean(_ value: Any) -> Any {
        switch value {
        case let d as Double where !d.isFinite: return NSNull()
        case let dict as [String: Any]: return dict.mapValues(clean)
        case let list as [Any]: return list.map(clean)
        default: return value
        }
    }
    let data = try JSONSerialization.data(withJSONObject: clean(object), options: [.sortedKeys])
    return String(decoding: data, as: UTF8.self)
}
