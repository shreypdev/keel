package dev.undra.runtime.testing

/**
 * Minimal, reflection-free test suite.
 *
 * Each test class extends [Suite], registers named cases in its `init` block, and exposes them to
 * JUnit 5 with one `@Test fun allCases() = assertPassed()`. The same cases run without JUnit through
 * [runAll], which `TestMain` calls (see `scripts/test-local.sh`); that path exists because the
 * sandbox this project is developed in has neither Gradle plugin nor JUnit jars.
 */
abstract class Suite {
    private class Case(val name: String, val body: () -> Unit)

    /** Thrown by [skip]: the case cannot run here (for example, no native library) and counts as neither pass nor failure. */
    private class Skipped(reason: String) : RuntimeException(reason)

    private val cases = ArrayList<Case>()
    private val failureLog = ArrayList<String>()

    /** Cases skipped in the last [runAll], with their reasons. */
    val skipped: List<String> get() = skippedLog
    private val skippedLog = ArrayList<String>()

    /** The class name, for reporting. */
    val suiteName: String get() = javaClass.simpleName

    /** Number of registered cases. */
    val caseCount: Int get() = cases.size

    /** Ends the running case as skipped, with a reason that is printed. Use it when the environment lacks something the case needs. */
    protected fun skip(reason: String): Nothing = throw Skipped(reason)

    /** Registers a case. Call from `init`. */
    protected fun case(name: String, body: () -> Unit) {
        require(cases.none { it.name == name }) { "$suiteName: duplicate case name '$name'" }
        cases += Case(name, body)
    }

    /**
     * Runs every case and returns the number of failures. Each failure is printed to stderr with
     * its case name, exception and the test frames it came from.
     */
    fun runAll(): Int {
        failureLog.clear()
        skippedLog.clear()
        var failures = 0
        for (c in cases) {
            try {
                c.body()
            } catch (s: Skipped) {
                skippedLog += "$suiteName > ${c.name}: ${s.message}"
                System.err.println("  SKIP $suiteName > ${c.name}: ${s.message}")
            } catch (t: Throwable) {
                failures++
                val entry = "  FAIL $suiteName > ${c.name}\n${describe(t)}"
                failureLog += entry
                System.err.println(entry)
            }
        }
        return failures
    }

    /** JUnit entry point: runs every case and fails with all failure details if any case failed. */
    protected fun assertPassed() {
        val failures = runAll()
        if (failures != 0) {
            throw AssertionError("$suiteName: $failures of $caseCount cases failed\n" + failureLog.joinToString("\n"))
        }
    }

    private fun describe(t: Throwable): String {
        val sb = StringBuilder()
        sb.append("    ").append(t.javaClass.name).append(": ").append(t.message).append('\n')
        var shown = 0
        for (frame in t.stackTrace) {
            if (frame.className.startsWith("dev.undra.runtime") && !frame.className.startsWith("dev.undra.runtime.testing.Suite")) {
                sb.append("      at ").append(frame).append('\n')
                if (++shown == 4) break
            }
        }
        t.cause?.let { sb.append("    caused by ").append(it.javaClass.name).append(": ").append(it.message).append('\n') }
        return sb.toString().trimEnd()
    }
}
