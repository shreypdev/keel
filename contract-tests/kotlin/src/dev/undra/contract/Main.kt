package dev.undra.contract

import dev.undra.runtime.UndraNative
import java.nio.file.Files
import kotlin.system.exitProcess

/** How long one scenario may run before it is reported as failed (each wait inside it has its own 5 s). */
private const val SCENARIO_LIMIT_MS: Long = 120_000L

/**
 * The Kotlin column of the contract tests (`contract-tests/scenarios.md`): runs S01 to S19 on the JVM
 * over JNI against the real `libundra_core` of the playground core and prints one line per scenario,
 * `SCENARIO S07 PASS|FAIL <title>`, which `contract-tests/check.sh kotlin` reads. Exits 1 if any fails.
 */
fun main() {
    // The default file-backed adapters (Fs, SecureStore) stay in a throwaway directory.
    System.setProperty("undra.data.dir", Files.createTempDirectory("undra-contract-kotlin").toString())
    if (!UndraNative.isAvailable) {
        println("the native core library could not be loaded: ${UndraNative.unavailableReason}")
        SCENARIOS.forEach { println("SCENARIO ${it.id} FAIL ${it.title}: native library not loaded") }
        exitProcess(2)
    }
    val boot = Bootstrap()
    var failures = 0
    for (scenario in SCENARIOS) {
        val started = System.nanoTime()
        val problem = run(scenario, boot)
        val ms = (System.nanoTime() - started) / 1_000_000L
        if (problem == null) {
            println("SCENARIO ${scenario.id} PASS ${scenario.title}")
        } else {
            failures++
            println("SCENARIO ${scenario.id} FAIL ${scenario.title}: ${problem.message?.lineSequence()?.firstOrNull() ?: problem.toString()}")
            problem.stackTrace.take(12).forEach { println("    at $it") }
        }
        println("note ${scenario.id} took $ms ms")
    }
    println("---- ${SCENARIOS.size - failures} of ${SCENARIOS.size} scenarios passed")
    // Exit explicitly: the core's threads are daemons, but a scenario that timed out may leave a worker behind.
    exitProcess(if (failures == 0) 0 else 1)
}

/** Runs [scenario] on its own thread so that a hang is a failure, not a stuck run; `null` means it passed. */
private fun run(scenario: Scenario, boot: Bootstrap): Throwable? {
    var outcome: Throwable? = null
    val worker = Thread({
        try {
            scenario.body(boot)
        } catch (e: Throwable) {
            outcome = e
        }
    }, "scenario-${scenario.id}")
    worker.isDaemon = true
    worker.start()
    worker.join(SCENARIO_LIMIT_MS)
    if (worker.isAlive) return Mismatch("did not finish within ${SCENARIO_LIMIT_MS / 1000} s")
    return outcome
}
