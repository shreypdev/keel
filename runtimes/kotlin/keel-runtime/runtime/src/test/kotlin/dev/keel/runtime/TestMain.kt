package dev.keel.runtime

import dev.keel.runtime.testing.Suite
import dev.keel.runtime.wire.ChangeSetTests
import dev.keel.runtime.wire.CodecTests
import dev.keel.runtime.wire.EnvelopeTests
import dev.keel.runtime.wire.FnvTests
import dev.keel.runtime.wire.FuzzTests
import dev.keel.runtime.wire.HandleTests
import dev.keel.runtime.wire.KeyedPatchTests
import dev.keel.runtime.wire.MalformedInputTests
import dev.keel.runtime.wire.PayloadTests
import dev.keel.runtime.wire.ReaderTests
import dev.keel.runtime.wire.WireVectorsTest
import dev.keel.runtime.wire.WriterTests
import kotlin.system.exitProcess

/**
 * Runs every suite without JUnit or reflection (see `scripts/test-local.sh`). Exits non-zero if any
 * case failed. Gradle ignores this file and runs the same classes through JUnit 5.
 */
fun main() {
    val suites: List<Suite> = listOf(
        WireVectorsTest(),
        WriterTests(),
        ReaderTests(),
        CodecTests(),
        HandleTests(),
        FnvTests(),
        EnvelopeTests(),
        PayloadTests(),
        ChangeSetTests(),
        KeyedPatchTests(),
        MalformedInputTests(),
        FuzzTests(),
        GoldenFullTests(),
        CoreCallTests(),
        StreamTests(),
        MirrorTests(),
        StoreTests(),
        PortTests(),
        InprocTransportTests(),
    )
    var failures = 0
    var cases = 0
    var skipped = 0
    for (suite in suites) {
        val started = System.nanoTime()
        val failed = suite.runAll()
        val millis = (System.nanoTime() - started) / 1_000_000
        val skipNote = if (suite.skipped.isEmpty()) "" else "  (${suite.skipped.size} skipped)"
        println("%-24s %4d cases  %4d failed  %6d ms%s".format(suite.suiteName, suite.caseCount, failed, millis, skipNote))
        failures += failed
        cases += suite.caseCount
        skipped += suite.skipped.size
    }
    println("----")
    println("$cases cases in ${suites.size} suites, $failures failed, $skipped skipped")
    exitProcess(if (failures == 0) 0 else 1)
}
