package dev.undra.runtime

import dev.undra.runtime.testing.Suite
import dev.undra.runtime.wire.ChangeSetTests
import dev.undra.runtime.wire.CodecTests
import dev.undra.runtime.wire.EnvelopeTests
import dev.undra.runtime.wire.FnvTests
import dev.undra.runtime.wire.FuzzTests
import dev.undra.runtime.wire.HandleTests
import dev.undra.runtime.wire.KeyedPatchTests
import dev.undra.runtime.wire.MalformedInputTests
import dev.undra.runtime.wire.PayloadTests
import dev.undra.runtime.wire.ReaderTests
import dev.undra.runtime.wire.WireVectorsTest
import dev.undra.runtime.wire.WriterTests
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
        CallErrorTests(),
        StreamTests(),
        MirrorTests(),
        CoalesceTests(),
        CoalesceModelTests(),
        StoreTests(),
        PortTests(),
        InprocTransportTests(),
        CoreEntryTests(),
        RemoteTransportTests(),
        WebSocketClientTests(),
        WebSocketHostileServerTests(),
        ReconnectCoreTests(),
        RemoteReconnectTests(),
        AdapterTests(),
        DiagnosticsTests(),
        FileAdapterTests(),
        StorageFailureTests(),
        HttpAdapterTests(),
        ErrorTests(),
        StatsTests(),
        CleanerTests(),
        DispatcherTests(),
        NativeShapeTests(),
        NativeSmokeTests(),
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
