package dev.undra.testkit

import dev.undra.testkit.testing.Suite
import kotlin.system.exitProcess

/** Runs every suite without JUnit or reflection (see `scripts/test-local.sh`). Gradle ignores this file and runs the same classes through JUnit 5. */
fun main() {
    val suites: List<Suite> = listOf(
        RecordingTests(),
        ConformanceTests(),
        PortTests(),
        RecordedCoreTests(),
    )
    var failures = 0
    var cases = 0
    for (suite in suites) {
        val started = System.nanoTime()
        val failed = suite.runAll()
        println("%-24s %4d cases  %4d failed  %6d ms".format(suite.suiteName, suite.caseCount, failed, (System.nanoTime() - started) / 1_000_000))
        failures += failed
        cases += suite.caseCount
    }
    println("----")
    println("$cases cases in ${suites.size} suites, $failures failed")
    exitProcess(if (failures == 0) 0 else 1)
}
