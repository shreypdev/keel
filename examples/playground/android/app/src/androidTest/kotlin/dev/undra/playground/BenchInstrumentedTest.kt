package dev.undra.playground

import android.os.Bundle
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import dev.undra.playground.bench.BenchConfig
import dev.undra.playground.bench.BenchRunner
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Runs the device benchmark (`bench/BenchRunner`) in the app's own process and hands the JSON to
 * `scripts/bench-device.sh --device android`, as the instrumentation status `undra_bench_json`
 * (`adb shell am instrument -w -r ...` prints it as `INSTRUMENTATION_STATUS: undra_bench_json=...`). The test is skipped
 * unless the run says `-e undra_bench 1`, so nothing starts a benchmark by accident.
 *
 * | Argument (`-e name value`) | Meaning |
 * |---|---|
 * | `undra_bench` | `1` to run |
 * | `undra_bench_mode` | `full` (every row and the drain experiment; also leaves the 100 KB snapshot behind) or `cold` (one cold start: the first load of this process and the restore of that snapshot) |
 * | `undra_bench_quick` | `1` for the short run (checks the plumbing in seconds) |
 */
@RunWith(AndroidJUnit4::class)
class BenchInstrumentedTest {
    @Test
    fun deviceBench() {
        val arguments = InstrumentationRegistry.getArguments()
        assumeTrue("pass -e undra_bench 1 to run the device benchmark", arguments.getString("undra_bench") == "1")
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val config = if (arguments.getString("undra_bench_quick") == "1") BenchConfig.QUICK else BenchConfig.FULL
        // The app loads its core when its activity starts (`UndraApp.start`, which `undra dev` needs to choose the core
        // by the launch intent); this test launches no activity, so it starts the in-process core the way the activity does.
        // `coreLoadNanos` is the time of that load, the first of this process.
        val app = instrumentation.targetContext.applicationContext as UndraApp
        instrumentation.runOnMainSync { app.start(null) }
        check(app.failure.value == null) { "the in-process core did not load: ${app.failure.value}" }
        val runner = BenchRunner(instrumentation.targetContext, config) { block -> instrumentation.runOnMainSync(block) }
        val json = when (val mode = arguments.getString("undra_bench_mode") ?: "full") {
            "full" -> runner.runFull()
            "cold" -> runner.runCold()
            else -> error("undra_bench_mode is full or cold, not $mode")
        }
        instrumentation.sendStatus(0, Bundle().apply { putString("undra_bench_json", json.toString()) })
    }
}
