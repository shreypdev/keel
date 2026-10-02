package dev.undra.testkit

import dev.undra.runtime.UndraPortException
import dev.undra.runtime.PortImpl
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.UndraWriter
import dev.undra.testkit.testing.Suite
import dev.undra.testkit.testing.assertEq
import dev.undra.testkit.testing.assertThrows
import dev.undra.testkit.testing.assertTrue
import dev.undra.testkit.testing.fixture
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Test

private fun keyArgs(key: String): ByteArray = UndraWriter().also { it.writeStr(key) }.toByteArray()

private fun setArgs(key: String, value: ByteArray): ByteArray = UndraWriter().also {
    it.writeStr(key)
    it.writeBytes(value)
}.toByteArray()

private fun PortImpl.call(method: UInt, args: ByteArray): ByteArray = runBlocking { methods.getValue(method)(args) }

private val KV = StandardPorts.Kv

class PortTests : Suite() {
    init {
        case("a recorder and a replayer round-trip a session's port traffic, in order") {
            var now = 1_000L
            val recorder = PortRecorder(0x2auL, now = { now }, platform = "android")
            val ports = recorder.wrapAll(mapOf(KV.PORT_ID to MemKv().portImpl()))
            val kv = ports.getValue(KV.PORT_ID)
            kv.call(KV.SET, setArgs("a", byteArrayOf(1, 2)))
            now = 1_040L
            val got = kv.call(KV.GET, keyArgs("a"))
            assertEq("01020000000102", got.toHex()) // Some(Bytes [1, 2])

            val text = recorder.toJson()
            val recording = Recording.fromJson(text)
            assertEq("android", recording.platform)
            assertEq("adapters", recording.source)
            assertEq(listOf(0L, 0L, 40L, 40L), recording.events.map { it.t })
            assertTrue(text.contains("\"name\":\"Kv.set\""), text)

            // A fresh run answers from the recording, with no store behind it.
            val replayer = Replayer(recording)
            val replayed = replayer.ports().getValue(KV.PORT_ID)
            assertEq(0, replayed.call(KV.SET, setArgs("a", byteArrayOf(1, 2))).size)
            assertEq(got.toHex(), replayed.call(KV.GET, keyArgs("a")).toHex())
            replayer.finish()
        }
        case("a call that deviates is a typed error and consumes nothing") {
            val recorder = PortRecorder(1uL, now = { 0L })
            recorder.wrapAll(mapOf(KV.PORT_ID to MemKv().portImpl())).getValue(KV.PORT_ID).call(KV.GET, keyArgs("a"))
            val replayer = Replayer(recorder.record())
            val kv = replayer.ports().getValue(KV.PORT_ID)
            assertTrue(assertThrows<dev.undra.runtime.UndraException> { kv.call(KV.GET, keyArgs("other")) }.message!!.contains("other arguments"))
            assertTrue(assertThrows<dev.undra.runtime.UndraException> { kv.call(KV.SET, keyArgs("a")) }.message!!.contains("recording has Kv.get next"))
            val first = replayer.errors()[0] as ReplayError.Mismatch
            assertEq(true, first.argsDiffer)
            assertEq("Kv.get", first.called)
            assertEq(0, first.nth)
            // Nothing was consumed: the right call still matches, and then the port is used up.
            assertTrue(kv.call(KV.GET, keyArgs("a")).isNotEmpty())
            assertTrue(assertThrows<dev.undra.runtime.UndraException> { kv.call(KV.GET, keyArgs("a")) }.message!!.contains("used up"))
            assertEq(listOf("Mismatch", "Mismatch", "Exhausted"), replayer.errors().map { it::class.simpleName })
            assertThrows<ReplayException> { replayer.finish() }
        }
        case("recorded calls that were never made are unconsumed") {
            val replayer = Replayer(Recording.fromJson(fixture("fixtures/ports-remote-todos.json")))
            assertEq(3, replayer.remaining)
            val problems = replayer.problems()
            assertTrue(problems.all { it is ReplayError.Unconsumed })
            assertEq(setOf("Clock.now_ms", "Rng.fill", "Http.request"), problems.map { (it as ReplayError.Unconsumed).next }.toSet())
            assertTrue(assertThrows<ReplayException> { replayer.finish() }.message!!.contains("never made"))
        }
        case("a call the recording holds no reply for is reported, naming the port and the method") {
            val recording = Recording(
                1uL, "hand", null,
                listOf(RecordedEvent(0, RecordedKind.PortCall(KV.PORT_ID, KV.GET, 1u, keyArgs("a")))),
            )
            val replayer = Replayer(recording)
            assertThrows<dev.undra.runtime.UndraException> { replayer.ports().getValue(KV.PORT_ID).call(KV.GET, keyArgs("a")) }
            val errors = replayer.errors()
            assertEq(1, errors.size)
            assertTrue(errors[0] is ReplayError.Unanswered && (errors[0] as ReplayError.Unanswered).called == "Kv.get", errors[0].describe())
            assertTrue(assertThrows<ReplayException> { replayer.finish() }.message!!.contains("Kv.get"), "finish names the method")
            assertTrue(errors[0].describe().contains("no reply in the recording"), errors[0].describe())
        }
        case("arguments that change from run to run can be ignored") {
            val recorder = PortRecorder(1uL, now = { 0L })
            recorder.wrapAll(mapOf(KV.PORT_ID to MemKv().portImpl())).getValue(KV.PORT_ID).call(KV.GET, keyArgs("a"))
            val replayer = Replayer(recorder.record(), ArgsPolicy.IGNORE)
            replayer.ports().getValue(KV.PORT_ID).call(KV.GET, keyArgs("generated-id-7"))
            assertEq(0, replayer.errors().size)
        }
        case("a recorded typed error and an unavailable answer replay as themselves") {
            val failing = PortImpl(
                sync = false,
                methods = mapOf<UInt, suspend (ByteArray) -> ByteArray>(
                    1u to { throw UndraPortException(byteArrayOf(7)) },
                    2u to { throw IllegalStateException("no such port") },
                ),
            )
            val recorder = PortRecorder(1uL, now = { 0L })
            val wrapped = recorder.wrap(99u, failing)
            assertThrows<UndraPortException> { wrapped.call(1u, ByteArray(0)) }
            assertThrows<IllegalStateException> { wrapped.call(2u, ByteArray(0)) }
            val replayer = Replayer(Recording.fromJson(recorder.toJson()))
            val methods = replayer.ports().getValue(99u)
            assertEq("07", assertThrows<UndraPortException> { methods.call(1u, ByteArray(0)) }.body.toHex())
            assertTrue(assertThrows<dev.undra.runtime.UndraException> { methods.call(2u, ByteArray(0)) }.message!!.contains("unavailable"))
            replayer.finish()
        }
    }

    @Test
    fun allCases() = assertPassed()
}
