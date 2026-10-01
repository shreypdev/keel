package dev.undra.playground.bench

import android.content.Context
import android.hardware.display.DisplayManager
import android.os.Build
import android.os.PowerManager
import dev.undra.playground.UndraApp
import dev.undra.playground.core.Bench
import dev.undra.playground.core.Todos
import dev.undra.runtime.DrainStats
import dev.undra.runtime.UndraCore
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import java.io.File
import kotlin.concurrent.thread

/** How much of a run each part gets. */
class BenchConfig(
    /** `sync_call`: operations per timed batch, timed batches, and batches thrown away first. */
    val syncBatch: Int = 1_000,
    val syncBatches: Int = 200,
    val warmupBatches: Int = 30,
    /** The other rows are timed one operation at a time: samples, and operations thrown away first. */
    val eachSamples: Int = 2_000,
    val eachWarmup: Int = 500,
    /** The list is put back to its 10,000 rows after this many inserts (outside the timed region). */
    val resetEveryInserts: Int = 500,
    /** The ADR-031 drain experiment: frames that count, and frames before them that do not. */
    val drainFrames: Int = 240,
    val drainWarmupFrames: Int = 60,
    /**
     * After each frame of the drain experiment, this many calls on the main thread, each drained alone: the cost of an update that
     * is not merged with any other, measured in the same minutes as the merged frames.
     */
    val singlesPerRound: Int = 8,
    /** Restores of the 100 KB snapshot inside one process (an in-process core cannot be loaded twice). */
    val restores: Int = 30,
    /** The 100 KB snapshot: this many to-dos of this many characters each. */
    val snapshotTodos: Int = 1_000,
    val snapshotTitleLength: Int = 80,
) {
    companion object {
        val FULL = BenchConfig()
        val QUICK = BenchConfig(
            syncBatch = 20, syncBatches = 4, warmupBatches = 1, eachSamples = 20, eachWarmup = 2, resetEveryInserts = 10,
            drainFrames = 4, drainWarmupFrames = 1, singlesPerRound = 2, restores = 2, snapshotTodos = 50,
        )
    }
}

/**
 * The Android half of the device benchmark: the blueprint's section 14 operations, measured the way the app
 * experiences them, through the generated `Bench` store and the runtime's mirror, with the app's own `UndraApp`
 * configuration (the Choreographer frame pacer). `BenchInstrumentedTest` runs it from an instrumented test and hands the
 * JSON to `scripts/bench-device.sh --device android`.
 *
 * The rows run on the main thread (what a UI handler does); [onMain] runs a block there and returns when it has finished.
 * The drain experiment runs on the calling thread, which commits the bursts the way a socket or a timer-driven core would,
 * while the main thread is free to run the frame callbacks.
 *
 * Every row is checked: after each operation the mirror's copy of the state is compared with what the operation did, so a
 * number is never reported for a run in which the apply did not happen.
 */
class BenchRunner(
    private val context: Context,
    private val config: BenchConfig,
    private val onMain: (Runnable) -> Unit,
) {
    private val notes = mutableListOf<String>()

    private fun <T> main(block: () -> T): T {
        var out: Result<T>? = null
        onMain { out = runCatching(block) }
        return (out ?: throw BenchCheckException("the main thread did not run the block")).getOrThrow()
    }

    private fun now(): Long = System.nanoTime()

    /** The smallest step of the clock and what one read costs, in nanoseconds. */
    private fun timerFacts(): Pair<Double, Double> {
        var smallest = Long.MAX_VALUE
        var last = now()
        var seen = 0
        var reads = 0
        while (seen < 20 && reads < 5_000_000) {
            reads++
            val t = now()
            if (t > last) {
                smallest = minOf(smallest, t - last)
                last = t
                seen++
            }
        }
        val count = 100_000
        val t0 = now()
        var sink = 0L
        repeat(count) { sink += now() }
        val t1 = now()
        if (sink == 0L) println()
        return Pair(if (smallest == Long.MAX_VALUE) 0.0 else smallest.toDouble(), (t1 - t0).toDouble() / count)
    }

    // ---- the full run ----------------------------------------------------------------------------

    /** Runs everything and returns the runner's half of the result file (`undra-device-bench-raw/1`). */
    fun runFull(): JSONObject {
        val (resolution, overhead) = timerFacts()
        val core = UndraCore.shared
        val bench = main { Bench(core) }
        main {
            if (bench.rows.value.size != ROWS) throw BenchCheckException("the bench list has ${bench.rows.value.size} rows after observing, not $ROWS")
        }
        val ops = listOf(measureSyncCall(bench), measureRecord(bench), measureInsert(bench), measureChangeSet(bench))
        val drain = measureDrain(core, bench)
        main { bench.close() }
        val cold = measureRestores(core)

        return JSONObject()
            .put("schema", "undra-device-bench-raw/1")
            .put("platform", "android")
            .put("runtime", "inproc (JNI, libplayground_core.so)")
            .put("device", deviceFacts())
            .put("timer", JSONObject().put("kind", "System.nanoTime").put("resolution_ns", resolution).put("overhead_ns", overhead))
            .put(
                "config",
                JSONObject()
                    .put("sync_batch", config.syncBatch).put("sync_batches", config.syncBatches).put("warmup_batches", config.warmupBatches)
                    .put("each_samples", config.eachSamples).put("each_warmup", config.eachWarmup)
                    .put("reset_every_inserts", config.resetEveryInserts).put("drain_frames", config.drainFrames)
                    .put("drain_warmup_frames", config.drainWarmupFrames).put("singles_per_round", config.singlesPerRound)
                    .put("restores", config.restores),
            )
            .put("ops", jsonArray(ops))
            .put("cold", cold)
            .put("drain", drain)
            .put("notes", org.json.JSONArray(notes))
    }

    /**
     * One cold start in a fresh process: the first `UndraPlaygroundCore.load` of the process (which `UndraApp` timed, and which loads
     * the native library), then the restore of the snapshot the full run left behind.
     */
    fun runCold(): JSONObject {
        val loadNs = (context.applicationContext as UndraApp).coreLoadNanos
        val snapshot = snapshotFile().readBytes()
        val core = UndraCore.shared
        val restoreNs = timedRestoreOnMain(core, snapshot)
        val stores = core.stats().liveStores
        if (stores != 1) throw BenchCheckException("the restore left $stores live stores, not 1")
        return JSONObject()
            .put("schema", "undra-device-bench-cold/1")
            .put("load_ns", loadNs.toDouble())
            .put("restore_ns", restoreNs.toDouble())
            .put("snapshot_bytes", snapshot.size)
    }

    private fun snapshotFile(): File = File(context.filesDir, "undra-bench-snapshot.bin")

    /**
     * A restore on the main thread (with the drain it ends with), timed there: the hop from the instrumentation thread to
     * the main looper and back (`runOnMainSync`) is the harness's, not the app's, so it stays outside the clock.
     */
    private fun timedRestoreOnMain(core: UndraCore, snapshot: ByteArray): Long = main {
        val t0 = now()
        core.restore(snapshot)
        now() - t0
    }

    // ---- the rows --------------------------------------------------------------------------------

    private fun measureSyncCall(bench: Bench): JSONObject = main {
        val size = config.syncBatch
        val perOp = ArrayList<Double>()
        var sum = 0L
        var expected = 0L
        for (batch in 0 until config.warmupBatches + config.syncBatches) {
            val t0 = now()
            for (i in 0 until size) sum += bench.benchAdd((i and 0xffff).toUInt(), 3u).toLong()
            val t1 = now()
            for (i in 0 until size) expected += (i and 0xffff) + 3
            if (sum != expected) throw BenchCheckException("benchAdd answered $sum, not $expected")
            if (batch >= config.warmupBatches) perOp.add((t1 - t0).toDouble() / size)
        }
        benchOp("sync_call", size, perOp, "`bench.benchAdd(a, b)` on the main thread: encode, `callSync` through JNI, decode, through the generated binding")
    }

    private fun measureRecord(bench: Bench): JSONObject = main {
        val payload = ByteArray(1_024) { (it * 31 + 7).toByte() }
        val perOp = ArrayList<Double>()
        for (i in 0 until config.eachWarmup + config.eachSamples) {
            val t0 = now()
            val back = bench.benchEchoBytes(payload)
            val t1 = now()
            if (back.size != payload.size || back.last() != payload.last()) throw BenchCheckException("benchEchoBytes returned another payload")
            if (i >= config.eachWarmup) perOp.add((t1 - t0).toDouble())
        }
        benchOp("record_1kb", 1, perOp, "`bench.benchEchoBytes(1,024 bytes)`: the payload crosses the boundary twice (a byte string, which is cheaper than a record of the same size)")
    }

    private fun measureInsert(bench: Bench): JSONObject = main {
        bench.benchListReset()
        val perOp = ArrayList<Double>()
        var inserted = 0
        for (i in 0 until config.eachWarmup + config.eachSamples) {
            if (inserted >= config.resetEveryInserts) {
                bench.benchListReset()
                inserted = 0
            }
            val t0 = now()
            bench.benchListInsert(INSERT_AT.toUInt())
            val t1 = now()
            inserted++
            val rows = bench.rows.value
            if (rows.size != ROWS + inserted) throw BenchCheckException("the mirror holds ${rows.size} rows after $inserted inserts")
            if (rows[INSERT_AT].id <= ROWS.toUInt()) throw BenchCheckException("the newest insert is not at the insert position in the mirror")
            if (i >= config.eachWarmup) perOp.add((t1 - t0).toDouble())
        }
        benchOp("keyed_insert_10k", 1, perOp, "`bench.benchListInsert(5000)` on about 10,000 rows: the call, the core's recorded insert, the one-operation patch, applied to the mirror's list before the call returns")
    }

    private fun measureChangeSet(bench: Bench): JSONObject = main {
        bench.benchListReset()
        val perOp = ArrayList<Double>()
        var touched = 0u
        for (i in 0 until config.eachWarmup + config.eachSamples) {
            val t0 = now()
            bench.benchTouchSignals(100u)
            val t1 = now()
            touched++
            if (bench.s099.value != touched) throw BenchCheckException("the mirror's hundredth counter is ${bench.s099.value} after $touched touches")
            if (i >= config.eachWarmup) perOp.add((t1 - t0).toDouble())
        }
        benchOp("changeset_100", 1, perOp, "`bench.benchTouchSignals(100)`: one call, one change-set of 100 entries, applied to 100 `StateFlow`s of the mirror before the call returns")
    }

    // ---- the ADR-031 drain -----------------------------------------------------------------------

    /** What the drain listener reported: the main thread appends, the producer waits. */
    private class DrainRecorder {
        class Drain(val changeSets: Int, val entries: Int, val applied: Int, val durationNs: Double)

        private val drains = ArrayList<Drain>()
        private var changeSetsSeen = 0

        @Synchronized
        fun record(stats: DrainStats) {
            drains.add(Drain(stats.changeSets, stats.entries, stats.appliedEntries, stats.duration.inWholeNanoseconds.toDouble()))
            changeSetsSeen += stats.changeSets
        }

        @Synchronized
        fun reset() {
            drains.clear()
            changeSetsSeen = 0
        }

        @Synchronized
        fun drainCount(): Int = drains.size

        @Synchronized
        fun changeSets(): Int = changeSetsSeen

        @Synchronized
        fun drainsSince(index: Int): List<Drain> = drains.subList(index, drains.size).toList()

        fun waitForChangeSets(total: Int, timeoutMs: Long): Boolean {
            val deadline = System.nanoTime() + timeoutMs * 1_000_000
            while (changeSets() < total) {
                if (System.nanoTime() > deadline) return false
                Thread.sleep(0, 200_000)
            }
            return true
        }
    }

    private class Burst(val frameNs: Double, val drains: Int, val changeSets: Int, val entries: Int, val applied: Int)

    private fun measureDrain(core: UndraCore, bench: Bench): JSONObject {
        main { bench.benchListReset() }
        val recorder = DrainRecorder()
        val registration = core.mirror.addDrainListener { stats -> recorder.record(stats) }
        try {
            // A burst of 1,667 one-update patches per frame, committed by this (non-main) thread the way a socket or a timer-driven
            // core would: the main thread meets them only as the drain at the next frame (the Choreographer frame pacer).
            val perBurst = UPDATES_PER_FRAME
            val bursts = ArrayList<Burst>()
            val singles = ArrayList<Double>()
            for (round in 1..config.drainWarmupFrames + config.drainFrames) {
                val start = recorder.drainCount() to recorder.changeSets()
                bench.benchListUpdateBurst(perBurst.toUInt())
                if (!recorder.waitForChangeSets(start.second + perBurst, 10_000)) {
                    throw BenchCheckException("burst $round: the main thread did not drain $perBurst change-sets within 10 s")
                }
                val drains = recorder.drainsSince(start.first)
                // The cost of one update on its own, in the same minutes as the frames: a call on the main thread drains before it
                // returns, so each of these is a drain of exactly one entry, and the drain listener times it.
                val mark = recorder.drainCount()
                main { repeat(config.singlesPerRound) { bench.benchListUpdateBurst(1u) } }
                val alone = recorder.drainsSince(mark).filter { it.changeSets == 1 && it.entries == 1 && it.applied == 1 }
                if (alone.size != config.singlesPerRound) {
                    throw BenchCheckException("round $round: ${alone.size} of ${config.singlesPerRound} calls were drained as one entry on their own")
                }
                if (round > config.drainWarmupFrames) {
                    bursts.add(Burst(drains.sumOf { it.durationNs }, drains.size, drains.sumOf { it.changeSets }, drains.sumOf { it.entries }, drains.sumOf { it.applied }))
                    alone.forEach { singles.add(it.durationNs) }
                }
            }
            main {
                if (bench.rows.value.size != ROWS) throw BenchCheckException("the update bursts changed the list's length to ${bench.rows.value.size}")
            }
            bursts.firstOrNull { it.changeSets != perBurst }?.let { throw BenchCheckException("a frame's drains consumed ${it.changeSets} change-sets, not $perBurst") }
            val drainDurations = recorder.drainsSince(0).filter { it.changeSets > 1 }.map { it.durationNs }
            val perEntry = BenchSummary(singles)
            val merged = BenchSummary(bursts.map { it.frameNs })
            fun median(values: List<Int>): Double = BenchSummary(values.map { it.toDouble() }).p50

            return JSONObject()
                .put("rows", ROWS)
                .put("updates_per_frame", perBurst)
                .put("frames", config.drainFrames)
                .put("warmup_frames", config.drainWarmupFrames)
                .put(
                    "producer",
                    "a thread of its own commits the burst (the core's side of a socket or a timer): the main thread's cost of a frame is the drain at the next frame (`ChoreographerFramePacer`), as `DrainStats.duration` times it (`System.nanoTime`)",
                )
                .put(
                    "merged",
                    JSONObject()
                        .put("frame_ns", merged.toJson())
                        .put("drain_ns", BenchSummary(drainDurations).toJson())
                        .put("change_sets_per_frame_p50", median(bursts.map { it.changeSets }))
                        .put("entries_per_frame_p50", median(bursts.map { it.entries }))
                        .put("applied_per_frame_p50", median(bursts.map { it.applied }))
                        .put("drains_per_frame_p50", median(bursts.map { it.drains })),
                )
                .put(
                    "unmerged_estimate",
                    JSONObject()
                        .put("per_entry_ns", perEntry.toJson())
                        .put("frame_ns", perBurst * perEntry.p50)
                        .put("frame_ns_mean", perBurst * perEntry.mean)
                        .put(
                            "method",
                            "the drain of a single entry (a call on the main thread drains before it returns, so the drain listener times a drain of exactly one change-set of one entry applied on its own), ${config.singlesPerRound} after each frame of the experiment so that both are measured in the same minutes (${perEntry.n} in all); times $perBurst, by the median entry and by the mean entry. The runtime was not reverted: this is what applying every entry on its own would cost, from the entry cost measured here",
                        ),
                )
                .put("ratio_unmerged_over_merged", perBurst * perEntry.p50 / merged.p50)
                .put("ratio_unmerged_over_merged_mean", perBurst * perEntry.mean / merged.p50)
                .put("note", "merged frame = the drain(s) that consumed one burst, summed: decode and apply of the merged patch to the `StateFlow` store (the list is copied once per drain)")
        } finally {
            registration.close()
        }
    }

    // ---- cold start ------------------------------------------------------------------------------

    /**
     * Restores a 100 KB snapshot into the running core, over and over. An in-process core cannot be loaded twice in one process,
     * so a core's own cold start is only the first load of a process, which `runCold` measures in a launch of its own.
     */
    private fun measureRestores(core: UndraCore): JSONObject {
        val todos = main { Todos(core) }
        runBlocking {
            for (i in 0 until config.snapshotTodos) {
                todos.add("todo %05d ".format(i).padEnd(config.snapshotTitleLength, 'x'))
            }
        }
        val snapshot = core.snapshot()
        snapshotFile().writeBytes(snapshot)
        val restores = ArrayList<Double>()
        repeat(config.restores) {
            val restoreNs = timedRestoreOnMain(core, snapshot)
            val stores = core.stats().liveStores
            if (stores != 1) throw BenchCheckException("the restore left $stores live stores, not 1")
            restores.add(restoreNs.toDouble())
        }
        main { todos.close() }
        if (kotlin.math.abs(snapshot.size - 100_000) > 10_000) notes.add("the snapshot is ${snapshot.size} bytes, not about 100,000")
        return JSONObject()
            .put("snapshot_bytes", snapshot.size)
            .put("first_load_in_run_ns", (context.applicationContext as UndraApp).coreLoadNanos.toDouble())
            .put("launches", org.json.JSONArray())
            .put("reload_in_process", JSONObject().put("load_ns", JSONObject.NULL).put("restore_ns", BenchSummary(restores).toJson()))
            .put(
                "note",
                "a 100 KB snapshot is ${config.snapshotTodos} to-dos of ${config.snapshotTitleLength} characters (the host row restores four stores of 250 rows of 100 bytes); " +
                    "load = the first `UndraPlaygroundCore.load` of the process, which includes `System.loadLibrary`; an in-process core cannot be loaded twice, so the in-process row is restores only, and the restore of a cold launch includes the main-thread drain",
            )
    }

    // ---- facts -----------------------------------------------------------------------------------

    private fun deviceFacts(): JSONObject {
        val facts = JSONObject()
        // `scripts/bench-device-report.mjs finalize` refuses a device label when this is true, so err towards true.
        val emulator = Build.FINGERPRINT.startsWith("generic") || Build.HARDWARE in setOf("ranchu", "goldfish", "vbox86") ||
            Build.PRODUCT.contains("sdk") || Build.MODEL.contains("Emulator")
        facts.put("model", "${Build.MANUFACTURER} ${Build.MODEL}")
        facts.put("device", Build.DEVICE)
        facts.put("hardware", Build.HARDWARE)
        facts.put("os", "Android ${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT})")
        facts.put("arch", Build.SUPPORTED_ABIS.firstOrNull() ?: "unknown")
        facts.put("abis", Build.SUPPORTED_ABIS.joinToString(","))
        facts.put("cores", Runtime.getRuntime().availableProcessors())
        facts.put("is_virtual", emulator)
        facts.put("build_fingerprint", Build.FINGERPRINT)
        facts.put("max_memory_bytes", Runtime.getRuntime().maxMemory())
        runCatching {
            facts.put("display_refresh_hz", context.getSystemService(DisplayManager::class.java).getDisplay(0).refreshRate.toDouble())
            val power = context.getSystemService(PowerManager::class.java)
            facts.put("power_save_mode", power.isPowerSaveMode)
            if (Build.VERSION.SDK_INT >= 29) facts.put("thermal_status", power.currentThermalStatus)
        }
        facts.put("debuggable_app", (context.applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE) != 0)
        return facts
    }

    private companion object {
        const val ROWS = 10_000
        const val INSERT_AT = 5_000
        const val UPDATES_PER_FRAME = 1_667
    }
}
