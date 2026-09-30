package dev.undra.runtime

import dev.undra.runtime.support.LogCapture
import dev.undra.runtime.support.ManualMainThread
import dev.undra.runtime.support.eventually
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.wire.Payloads.PanicInfo
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.UndraWriter
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlin.coroutines.EmptyCoroutineContext
import org.junit.jupiter.api.Test

private fun strings(vararg s: String): ByteArray = UndraWriter().also { w -> s.forEach { w.writeStr(it) } }.toByteArray()

class ErrorTests : Suite() {
    init {
        case("UndraReplyException describes each status and exposes the status and body") {
            val panic = UndraReplyException(ReplyStatus.PANIC, strings("index out of bounds", "frame 1\nframe 2"))
            assertEq(ReplyStatus.PANIC, panic.status)
            assertEq(PanicInfo("index out of bounds", "frame 1\nframe 2"), panic.panicInfo)
            assertEq(null, panic.badRequestReason)
            assertTrue(panic.message!!.contains("index out of bounds"))

            val bad = UndraReplyException(ReplyStatus.BAD_REQUEST, strings("unknown method 0x1"))
            assertEq("unknown method 0x1", bad.badRequestReason)
            assertEq(null, bad.panicInfo)
            assertTrue(bad.message!!.contains("unknown method 0x1"))

            val typed = UndraReplyException(ReplyStatus.ERROR, byteArrayOf(1, 2, 3))
            assertTrue(typed.message!!.contains("typed error"), typed.message!!)
            assertEq(null, typed.panicInfo)
            assertEq(null, typed.badRequestReason)
            assertTrue(UndraReplyException(ReplyStatus.CANCELLED, ByteArray(0)).message!!.contains("cancelled"))
            assertTrue(UndraReplyException(ReplyStatus.STREAM_OPENED, ByteArray(0)).message!!.contains("stream"))
            assertTrue(UndraReplyException(ReplyStatus.OK, ByteArray(0)).message!!.contains("ok"))
        }

        case("an unreadable panic or bad-request body degrades to a generic message instead of throwing") {
            val panic = UndraReplyException(ReplyStatus.PANIC, byteArrayOf(9, 9))
            assertEq(null, panic.panicInfo)
            assertTrue(panic.message!!.contains("unreadable"), panic.message!!)
            val bad = UndraReplyException(ReplyStatus.BAD_REQUEST, ByteArray(0))
            assertEq(null, bad.badRequestReason)
            assertTrue(bad.message!!.contains("unreadable"), bad.message!!)
        }

        case("UndraSchemaMismatchException carries both hashes as hex") {
            val e = UndraSchemaMismatchException(0xABCDEF0123456789uL, 1uL)
            assertEq(0xABCDEF0123456789uL, e.expected)
            assertEq(1uL, e.got)
            assertTrue(e.message!!.contains("0xabcdef0123456789") && e.message!!.contains("0x1"), e.message!!)
            assertTrue(e.message!!.contains("regenerate"), "the message says how to fix it")
        }

        case("UndraPortException keeps the encoded error; UndraException keeps its cause; all are UndraExceptions") {
            assertEq(listOf<Byte>(1, 2), UndraPortException(byteArrayOf(1, 2)).body.toList())
            val cause = IllegalStateException("root")
            assertTrue(UndraException("wrapped", cause).cause === cause)
            assertEq(null, UndraException("plain").cause)
            val all: List<UndraException> = listOf(
                UndraReplyException(ReplyStatus.ERROR, ByteArray(0)),
                UndraModeException("mode"),
                UndraSchemaMismatchException(1uL, 2uL),
                UndraPortException(ByteArray(0)),
            )
            assertEq(4, all.size)
            assertEq("mode", UndraModeException("mode").message)
        }
    }

    @Test
    fun allCases() = assertPassed()
}

class StatsTests : Suite() {
    init {
        case("the core's statistics document is read field by field, ignoring what it does not need") {
            val json = "{\"platform\":\"jvm\",\"mode\":\"inproc\",\"schema_hash\":\"0x691eee0733e4a44f\",\"live_handles\":3,\"live_stores\":1," +
                "\"poisoned_stores\":0,\"tasks\":2,\"active_calls\":1,\"open_streams\":4,\"pending_port_calls\":5,\"abandoned_port_calls\":0," +
                "\"pending_timers\":6,\"blocking_threads\":{\"started\":1,\"max\":4},\"transactions\":12,\"panics\":8,\"turns\":5,\"polls\":9," +
                "\"crossings\":{\"calls\":7,\"replies\":6,\"change_sets\":12,\"change_set_bytes\":900,\"port_calls\":2,\"port_replies\":2," +
                "\"stream_items\":0,\"events\":1,\"bad_requests\":0,\"cancelled\":0}}"
            val s = UndraStats.fromCoreJson(json, hostPendingCalls = 2, hostMirrorHandles = 3)
            assertEq(3, s.liveHandles)
            assertEq(1, s.liveStores)
            assertEq(2, s.tasks)
            assertEq(1, s.activeCalls)
            assertEq(4, s.openStreams)
            assertEq(5, s.pendingPortCalls)
            assertEq(6, s.pendingTimers)
            assertEq(12L, s.transactions)
            assertEq(8L, s.panics)
            assertEq(2, s.hostPendingCalls)
            assertEq(3, s.hostMirrorHandles)
            assertEq(json, s.raw)
            assertTrue(s.toString().contains("liveHandles=3"))
        }

        case("missing fields are unknown and a document that is not an object is all unknown") {
            val partial = UndraStats.fromCoreJson("{\"live_handles\":9}", 0, 0)
            assertEq(9, partial.liveHandles)
            assertEq(UndraStats.UNKNOWN, partial.tasks)
            assertEq(UndraStats.UNKNOWN.toLong(), partial.transactions)
            for (bad in listOf("", "[]", "null", "{", "{\"a\":}", "garbage", "{\"live_handles\":9} trailing")) {
                val s = UndraStats.fromCoreJson(bad, 1, 2)
                assertEq(UndraStats.UNKNOWN, s.liveHandles, "for '$bad'")
                assertEq(bad, s.raw)
                assertEq(1, s.hostPendingCalls)
            }
            // A count too large for an Int is clamped, and a negative one cannot appear as a count.
            assertEq(Int.MAX_VALUE, UndraStats.fromCoreJson("{\"live_handles\":99999999999}", 0, 0).liveHandles)
            assertEq(0, UndraStats.fromCoreJson("{\"live_handles\":-4}", 0, 0).liveHandles)
        }

        case("the constructor default matches the bindgen fixture: UndraStats(liveHandles) alone") {
            val s = UndraStats(0)
            assertEq(0, s.liveHandles)
            assertEq(UndraStats.UNKNOWN, s.liveStores)
            assertEq("{}", s.raw)
        }

        case("MiniJson handles nesting, escapes, unicode, arrays, literals and numbers") {
            val doc = MiniJson.parseObject(
                " {\"s\":\"a\\\"b\\\\c\\n\\u00e9\",\"n\":-12,\"f\":1.5e3,\"t\":true,\"z\":null,\"arr\":[1,[2,3],{\"k\":\"v\"}],\"o\":{\"x\":{\"y\":7}},\"empty\":{},\"ea\":[]} ",
            )!!
            assertEq("a\"b\\c\n\u00e9", doc["s"])
            assertEq(-12L, doc["n"])
            assertEq(null, doc["f"], "non-integers are not needed and come back as null")
            assertEq(true, doc["t"])
            assertEq(null, doc["z"])
            @Suppress("UNCHECKED_CAST")
            assertEq(7L, ((doc["o"] as Map<String, Any?>)["x"] as Map<String, Any?>)["y"])
            assertEq(emptyMap<String, Any?>(), doc["empty"])
            assertEq(9, doc.size)
        }

        case("MiniJson rejects malformed documents without throwing") {
            for (bad in listOf("{\"a\"}", "{\"a\":1,}", "{'a':1}", "{\"a\":\"unterminated}", "{\"a\":tru}", "{\"a\":1 \"b\":2}", "{\"a\":\"\\q\"}", "{\"a\":\"\\u12\"}", "{\"a\":[1,}", "{\"a\":--1}")) {
                assertEq(null, MiniJson.parseObject(bad), "for $bad")
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}

class CleanerTests : Suite() {
    init {
        case("explicit clean runs the action once, however often it is asked") {
            for (register in listOf<(Any, Runnable) -> HandleCleaner.Cleanable>(HandleCleaner::register, HandleCleaner::registerWithQueue)) {
                val runs = AtomicInteger()
                val owner = Any()
                val cleanable = register(owner, Runnable { runs.incrementAndGet() })
                cleanable.clean()
                cleanable.clean()
                assertEq(1, runs.get())
                assertTrue(owner.hashCode() == owner.hashCode()) // keep the owner reachable until here
            }
        }

        case("the action runs after the owner is collected, on a daemon thread, exactly once") {
            for ((name, register) in listOf<Pair<String, (Any, Runnable) -> HandleCleaner.Cleanable>>("default" to HandleCleaner::register, "queue" to HandleCleaner::registerWithQueue)) {
                val runs = AtomicInteger()
                val thread = CopyOnWriteArrayList<Thread>()
                register(Any(), Runnable { runs.incrementAndGet(); thread.add(Thread.currentThread()) })
                eventually("$name backend cleans a collected owner", timeoutMs = 20_000) {
                    System.gc()
                    runs.get() > 0
                }
                Thread.sleep(50)
                assertEq(1, runs.get(), name)
                assertTrue(thread.single().isDaemon, "$name: the cleaner thread must be a daemon")
            }
        }

        case("the action does not run while the owner is reachable") {
            for (register in listOf<(Any, Runnable) -> HandleCleaner.Cleanable>(HandleCleaner::register, HandleCleaner::registerWithQueue)) {
                val runs = AtomicInteger()
                val owner = Any()
                register(owner, Runnable { runs.incrementAndGet() })
                repeat(5) {
                    System.gc()
                    Thread.sleep(10)
                }
                assertEq(0, runs.get())
                assertTrue(owner.toString().isNotEmpty())
            }
        }

        case("a failing action is logged and does not stop the cleaner") {
            LogCapture("dev.undra.runtime").use { log ->
                val runs = AtomicInteger()
                HandleCleaner.registerWithQueue(Any(), Runnable { throw IllegalStateException("cleanup failed") }).clean()
                HandleCleaner.registerWithQueue(Any(), Runnable { runs.incrementAndGet() }).clean()
                assertEq(1, runs.get())
                assertTrue(log.records.any { it.thrown is IllegalStateException })
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}

class DispatcherTests : Suite() {
    init {
        case("the main dispatcher runs on the undra-main thread and knows it") {
            assertEq(false, UndraDispatchers.isMainThread())
            val seen = CopyOnWriteArrayList<Any>()
            val done = CountDownLatch(1)
            UndraDispatchers.main.dispatch(EmptyCoroutineContext, Runnable {
                seen.add(Thread.currentThread().name)
                seen.add(Thread.currentThread().isDaemon)
                seen.add(UndraDispatchers.isMainThread())
                done.countDown()
            })
            assertTrue(done.await(10, TimeUnit.SECONDS))
            assertEq(listOf<Any>("undra-main", true, true), seen.toList())
            assertEq(false, UndraDispatchers.isMainThread())
        }

        case("tasks posted to the main thread run in order and never inline") {
            val main = UndraDispatchers.mainThread()
            val order = CopyOnWriteArrayList<Int>()
            val done = CountDownLatch(1)
            val gate = CountDownLatch(1)
            main.post(Runnable { gate.await() }) // holds the queue so that both tasks below are queued before either runs
            main.post(Runnable {
                // Posted from the main thread itself: still queued behind task 2, not run in place.
                main.post(Runnable { order.add(3); done.countDown() })
                order.add(1)
            })
            main.post(Runnable { order.add(2) })
            gate.countDown()
            assertTrue(done.await(10, TimeUnit.SECONDS))
            assertEq(listOf(1, 2, 3), order.toList())
        }

        case("a task that throws is logged and does not take the main thread down") {
            LogCapture("dev.undra.runtime").use { log ->
                val main = ExecutorMainThread()
                main.post(Runnable { throw IllegalStateException("boom") })
                val names = CopyOnWriteArrayList<Boolean>()
                val done = CountDownLatch(1)
                main.post(Runnable { names.add(main.isCurrent()); done.countDown() })
                assertTrue(done.await(10, TimeUnit.SECONDS))
                assertEq(listOf(true), names.toList())
                assertTrue(log.records.any { it.thrown is IllegalStateException })
            }
        }

        case("delivery is a single named daemon thread") {
            val names = CopyOnWriteArrayList<String>()
            val daemon = CopyOnWriteArrayList<Boolean>()
            val done = CountDownLatch(2)
            repeat(2) {
                UndraDispatchers.delivery.execute {
                    names.add(Thread.currentThread().name)
                    daemon.add(Thread.currentThread().isDaemon)
                    done.countDown()
                }
            }
            assertTrue(done.await(10, TimeUnit.SECONDS))
            assertEq(listOf("undra-delivery", "undra-delivery"), names.toList())
            assertEq(listOf(true, true), daemon.toList())
        }

        case("the manual main thread used by the mirror tests behaves like a queue") {
            val main = ManualMainThread()
            val ran = AtomicInteger()
            main.post(Runnable { ran.incrementAndGet() })
            assertEq(0, ran.get())
            assertEq(1, main.pending)
            main.runPending()
            assertEq(1, ran.get())
        }
    }

    @Test
    fun allCases() = assertPassed()
}
