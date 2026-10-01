// The real JNI shim driven by the real Kotlin runtime (UndraCore over UndraNative), against the
// fixture core (crates/undra-ffi/tests/fixture). It complements the runtime's own
// NativeSmokeTests, which only use unknown method ids: this one crosses every callback (reply,
// change-set, stream, sync and async ports, logs) with real payloads. Run through run.sh.
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraNative
import dev.undra.runtime.UndraReplyException
import dev.undra.runtime.LoadOptions
import dev.undra.runtime.PortImpl
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import dev.undra.runtime.wire.Fnv
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking

private var failures = 0
private var passed = 0

private fun check(name: String, body: () -> Unit) {
    try {
        body()
        passed++
        println("ok   $name")
    } catch (e: Throwable) {
        failures++
        println("FAIL $name: $e")
        e.printStackTrace(System.out)
    }
}

private fun expect(cond: Boolean, message: () -> String) {
    if (!cond) throw AssertionError(message())
}

private fun <T> expectEq(expected: T, actual: T, what: String = "") {
    if (expected != actual) throw AssertionError("$what: expected <$expected> but was <$actual>")
}

private inline fun <reified E : Throwable> expectThrows(body: () -> Unit): E {
    try {
        body()
    } catch (e: Throwable) {
        if (e is E) return e
        throw AssertionError("expected ${E::class.simpleName}, got $e")
    }
    throw AssertionError("expected ${E::class.simpleName}, nothing thrown")
}

private fun port(trait: String): UInt = Fnv.fnv1a32("port.$trait")
private fun portMethod(trait: String, name: String): UInt = Fnv.fnv1a32("$trait.$name")
private fun method(type: String, name: String): UInt = Fnv.fnv1a32("$type.$name")
private fun fn(name: String): UInt = Fnv.fnv1a32("fn.$name")

private fun i64(v: Long) = Codecs.i64.encodeToByteArray(v)
private fun u32(v: UInt) = Codecs.u32.encodeToByteArray(v)

private fun le32(bytes: ByteArray, at: Int): UInt =
    (0 until 4).fold(0u) { acc, i -> acc or ((bytes[at + i].toUInt() and 0xffu) shl (8 * i)) }

fun main() {
    System.setProperty("undra.data.dir", java.nio.file.Files.createTempDirectory("undra-jni-e2e").toString())
    if (!UndraNative.isAvailable) {
        println("FAIL native library not loaded: ${UndraNative.unavailableReason}")
        System.exit(2)
    }
    val logs = CopyOnWriteArrayList<Triple<Int, String, String>>()
    val sumPort = PortImpl(
        true,
        mapOf(portMethod("Sum", "add") to { args: ByteArray -> u32(le32(args, 0) + le32(args, 4)) }),
    )
    val echoPort = PortImpl(
        false,
        mapOf(portMethod("Echo", "ping") to { args: ByteArray -> kotlinx.coroutines.delay(5); u32(le32(args, 0) + 1000u) }),
    )
    val logPort = PortImpl(
        true,
        mapOf(
            StandardPorts.Log.LOG to { args: ByteArray ->
                val r = dev.undra.runtime.wire.UndraReader(java.nio.ByteBuffer.wrap(args))
                logs.add(Triple(r.readU8().toInt(), r.readStr(), r.readStr()))
                ByteArray(0)
            },
        ),
    )
    val core = UndraCore.load(
        LoadOptions(
            expectedSchemaHash = UndraNative.schemaHash().toULong(),
            adapters = mapOf(port("Sum") to sumPort, port("Echo") to echoPort, StandardPorts.Log.PORT_ID to logPort),
        ),
    )
    fun calc(base: Long): Long = core.construct(Fnv.fnv1a32("Calculator"), method("Calculator", "new"), i64(base))
    fun on(handle: Long, name: String, type: String = "Calculator") = CallTarget.ObjectMethod(Handle(handle), method(type, name))
    val calculator = calc(100)

    check("schema hash and JSON come from the fixture core") {
        expect(UndraNative.schemaHash() != 0L) { "empty schema" }
        val json = String(UndraNative.schemaJson(), Charsets.UTF_8)
        expect(json.contains("Calculator") && json.contains("Counter")) { json.take(200) }
    }

    check("a sync free function replies through callSync") {
        val body = core.callSync(CallTarget.FreeFunction(fn("version")), fn("version"), ByteArray(0))
        expectEq("undra-ffi test core 1", Codecs.string.decodeAll(body))
    }

    check("sync method and the async path through the reply callback") {
        val args = i64(2) + i64(3)
        expectEq(105L, Codecs.i64.decodeAll(core.callSync(on(calculator, "add"), method("Calculator", "add"), args)))
        expectEq(105L, Codecs.i64.decodeAll(runBlocking { core.call(on(calculator, "slow_add"), method("Calculator", "slow_add"), args) }))
    }

    check("unknown methods and typed errors") {
        val unknown = expectThrows<UndraReplyException> { core.callSync(CallTarget.FreeFunction(0xDEADBEEFu), 0xDEADBEEFu, ByteArray(0)) }
        expectEq(ReplyStatus.BAD_REQUEST, unknown.status)
        val failure = expectThrows<UndraReplyException> { core.callSync(on(calculator, "fail"), method("Calculator", "fail"), ByteArray(0)) }
        expectEq(ReplyStatus.ERROR, failure.status)
    }

    check("a panic in the core is a status 2 reply and the JVM lives on") {
        val boom = expectThrows<UndraReplyException> { core.callSync(on(calculator, "boom"), method("Calculator", "boom"), ByteArray(0)) }
        expectEq(ReplyStatus.PANIC, boom.status)
        expectEq("kaboom", boom.panicInfo?.message)
        val asyncBoom = expectThrows<UndraReplyException> {
            runBlocking { core.call(on(calculator, "async_boom"), method("Calculator", "async_boom"), ByteArray(0)) }
        }
        expectEq("async kaboom", asyncBoom.panicInfo?.message)
        expectEq(105L, Codecs.i64.decodeAll(core.callSync(on(calculator, "add"), method("Calculator", "add"), i64(2) + i64(3))))
        expect(logs.any { it.first == 5 && it.second == "undra::panic" }) { "no fatal log: $logs" }
    }

    check("streams deliver items through the stream callback") {
        val items = runBlocking {
            core.stream(on(calculator, "ticks"), method("Calculator", "ticks"), u32(40u)).toList()
        }
        expectEq((0u until 40u).toList(), items.map { Codecs.u32.decodeAll(it) })
    }

    check("a store is observed: initial values then updates arrive on the mirror") {
        val counter = core.construct(Fnv.fnv1a32("Counter"), method("Counter", "new"), ByteArray(0))
        val seen = CopyOnWriteArrayList<UInt>()
        val gate = AtomicInteger(0)
        val latch = CountDownLatch(2)
        core.mirror.register(counter) { _, _, reader ->
            seen.add(reader.readU32())
            gate.incrementAndGet()
            latch.countDown()
        }
        core.observe(counter, 0xFFFFFFFFu, true)
        core.callSync(on(counter, "bump", "Counter"), method("Counter", "bump"), ByteArray(0))
        expect(latch.await(10, TimeUnit.SECONDS)) { "change-sets did not arrive: $seen" }
        expectEq(listOf(0u, 1u), seen.toList())
        core.observe(counter, 0xFFFFFFFFu, false)
        core.release(counter)
    }

    check("host ports: a sync port answers through portSyncReply, an async one through portReply") {
        expectEq(42u, Codecs.u32.decodeAll(core.callSync(on(calculator, "sum_on_host"), method("Calculator", "sum_on_host"), u32(20u) + u32(22u))))
        expectEq(1007u, Codecs.u32.decodeAll(runBlocking { core.call(on(calculator, "ping_host"), method("Calculator", "ping_host"), u32(7u)) }))
    }

    check("the default Clock adapter answers the Clock port") {
        val now = Codecs.i64.decodeAll(core.callSync(on(calculator, "clock_now"), method("Calculator", "clock_now"), ByteArray(0)))
        expect(Math.abs(now - System.currentTimeMillis()) < 5_000) { "clock $now vs ${System.currentTimeMillis()}" }
    }

    check("core log records reach the Log port, filtered by the configured level") {
        logs.clear()
        core.observe(0x7777777700000001L, 0u, true)
        expect(logs.any { it.first == 3 && it.second == "undra::runtime" }) { "no warning: $logs" }
        core.callSync(on(calculator, "log_line"), method("Calculator", "log_line"), byteArrayOf(3) + Codecs.string.encodeToByteArray("from rust"))
        expect(logs.any { it.second == "calculator" && it.third == "from rust" }) { "no rust log: $logs" }
    }

    check("many threads call at once (sync and async paths)") {
        val errors = CopyOnWriteArrayList<Throwable>()
        val workers = (0 until 8).map { t ->
            Thread {
                try {
                    repeat(200) { i ->
                        val args = i64(t.toLong()) + i64(i.toLong())
                        val sync = Codecs.i64.decodeAll(core.callSync(on(calculator, "add"), method("Calculator", "add"), args))
                        expectEq(100L + t + i, sync, "sync")
                        val async = Codecs.i64.decodeAll(runBlocking { core.call(on(calculator, "add"), method("Calculator", "add"), args) })
                        expectEq(100L + t + i, async, "async")
                    }
                } catch (e: Throwable) {
                    errors.add(e)
                }
            }.also { it.start() }
        }
        workers.forEach { it.join(60_000) }
        expect(errors.isEmpty()) { "worker failures: $errors" }
    }

    check("stats report live handles") {
        val stats = core.stats()
        expect(stats.raw.contains("live_handles")) { stats.raw }
        expect(stats.liveHandles >= 1) { stats.raw }
    }

    check("snapshot and restore through JNI (last: restore invalidates non-store handles)") {
        val counter = core.construct(Fnv.fnv1a32("Counter"), method("Counter", "new"), ByteArray(0))
        repeat(3) { core.callSync(on(counter, "bump", "Counter"), method("Counter", "bump"), ByteArray(0)) }
        val snapshot = core.snapshot()
        core.callSync(on(counter, "bump", "Counter"), method("Counter", "bump"), ByteArray(0))
        core.restore(snapshot)
        val seen = CopyOnWriteArrayList<UInt>()
        val latch = CountDownLatch(1)
        core.mirror.register(counter) { _, _, reader -> seen.add(reader.readU32()); latch.countDown() }
        core.observe(counter, 0xFFFFFFFFu, true)
        expect(latch.await(10, TimeUnit.SECONDS)) { "no change-set after restore" }
        expectEq(3u, seen.first())
        expectThrows<dev.undra.runtime.UndraException> { core.restore(byteArrayOf(1, 2, 3)) }
        core.release(counter)
    }

    check("close while JVM threads are calling (ADR-034): they end, the native threads exit, a new load works") {
        val stop = java.util.concurrent.atomic.AtomicBoolean(false)
        val unexpected = CopyOnWriteArrayList<Throwable>()
        val workers = (0 until 4).map { t ->
            Thread {
                var i = 0L
                while (!stop.get()) {
                    try {
                        core.callSync(on(calculator, "add"), method("Calculator", "add"), i64(t.toLong()) + i64(i++))
                        runBlocking { core.call(on(calculator, "add"), method("Calculator", "add"), i64(1) + i64(2)) }
                    } catch (e: dev.undra.runtime.UndraException) {
                        // "closed", once close has begun: expected.
                    } catch (e: Throwable) {
                        unexpected.add(e)
                    }
                }
            }.also { it.start() }
        }
        Thread.sleep(50)
        val closer = Thread { core.close() }.also { it.start() }
        core.close()
        closer.join(10_000)
        stop.set(true)
        workers.forEach { it.join(10_000) }
        expect(unexpected.isEmpty()) { "unexpected failures: $unexpected" }
        expect(workers.none { it.isAlive } && !closer.isAlive) { "a thread is stuck after close" }
        val stats = UndraNative.statsJson()
        expect("\"initialized\":false" in stats && "\"runtime_threads\":0" in stats) { "after close: $stats" }
        val again = UndraCore.load(LoadOptions(expectedSchemaHash = UndraNative.schemaHash().toULong()))
        try {
            val fresh = again.construct(Fnv.fnv1a32("Calculator"), method("Calculator", "new"), i64(7))
            val sum = again.callSync(CallTarget.ObjectMethod(Handle(fresh), method("Calculator", "add")), method("Calculator", "add"), i64(1) + i64(2))
            expectEq(10L, Codecs.i64.decodeAll(sum), "add on the fresh core")
        } finally {
            again.close()
        }
    }

    core.close()
    println("---- $passed passed, $failures failed")
    // Exit through the normal path on purpose: the core thread is a daemon and must not keep the JVM alive.
    if (failures > 0) System.exit(1)
}
