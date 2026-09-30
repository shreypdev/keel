package dev.keel.runtime

import dev.keel.runtime.adapters.StandardPorts
import dev.keel.runtime.support.FakeTransport
import dev.keel.runtime.support.LogCapture
import dev.keel.runtime.support.NO_BYTES
import dev.keel.runtime.support.attach
import dev.keel.runtime.support.eventually
import dev.keel.runtime.testing.Suite
import dev.keel.runtime.testing.assertEq
import dev.keel.runtime.testing.assertThrows
import dev.keel.runtime.testing.assertTrue
import dev.keel.runtime.wire.Codecs
import dev.keel.runtime.wire.KeelReader
import dev.keel.runtime.wire.Payloads.PortReply
import dev.keel.runtime.wire.Payloads.PortStatus
import dev.keel.runtime.wire.decodeAll
import dev.keel.runtime.wire.encodeToByteArray
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.withTimeout
import org.junit.jupiter.api.Test

private const val PORT = 0xAAAA0001u
private const val ECHO = 0xBBBB0001u
private const val FAIL_TYPED = 0xBBBB0002u
private const val FAIL_OTHER = 0xBBBB0003u
private const val SLOW = 0xBBBB0004u
private const val TIMEOUT = 0xBBBB0005u
private const val NAME = 0xBBBB0006u

private fun reply(o: PortOutcome): PortReply = PortReply.decode((o as PortOutcome.Sync).reply)

class PortTests : Suite() {
    init {
        case("a sync port is answered inline with a whole PortReply payload") {
            val t = FakeTransport()
            val impl = PortImpl(true, mapOf(ECHO to { args: ByteArray -> args + byteArrayOf(9) }))
            attach(t, adapters = mapOf(PORT to impl)).use {
                val outcome = t.portCall(PORT, ECHO, 41u, byteArrayOf(1, 2))
                assertTrue(outcome is PortOutcome.Sync, "got $outcome")
                val r = reply(outcome)
                assertEq(41u, r.portCallId)
                assertEq(PortStatus.OK, r.status)
                assertEq(listOf<Byte>(1, 2, 9), r.body.toList())
                assertEq(0, t.portReplies.size, "a sync answer is returned, not sent separately")
            }
        }

        case("a sync port runs on the thread that called, with no hop") {
            val t = FakeTransport()
            val thread = CopyOnWriteArrayList<String>()
            val impl = PortImpl(true, mapOf(NAME to { _: ByteArray -> thread.add(Thread.currentThread().name); NO_BYTES }))
            attach(t, adapters = mapOf(PORT to impl)).use {
                t.onCore { t.portCall(PORT, NAME, 1u, NO_BYTES) }
                t.awaitCore()
                assertEq(listOf("fake-core"), thread.toList())
            }
        }

        case("a sync port method that suspends is reported as unavailable, not waited for") {
            val t = FakeTransport()
            val gate = CompletableDeferred<Unit>()
            val impl = PortImpl(true, mapOf(SLOW to { _: ByteArray -> gate.await(); NO_BYTES }))
            LogCapture("dev.keel.runtime").use { log ->
                attach(t, adapters = mapOf(PORT to impl)).use {
                    assertEq(PortOutcome.Unavailable, t.portCall(PORT, SLOW, 1u, NO_BYTES))
                    assertTrue(log.messages().any { it.contains("suspended") })
                    gate.complete(Unit)
                }
            }
        }

        case("a sync port answers a typed error with status 1 and its body; other failures are unavailable") {
            val t = FakeTransport()
            val impl = PortImpl(
                true,
                mapOf(
                    FAIL_TYPED to { _: ByteArray -> throw KeelPortException(byteArrayOf(7, 7)) },
                    FAIL_OTHER to { _: ByteArray -> throw IllegalStateException("boom") },
                ),
            )
            LogCapture("dev.keel.runtime").use { log ->
                attach(t, adapters = mapOf(PORT to impl)).use {
                    val typed = reply(t.portCall(PORT, FAIL_TYPED, 5u, NO_BYTES))
                    assertEq(PortStatus.ERROR, typed.status)
                    assertEq(listOf<Byte>(7, 7), typed.body.toList())
                    assertEq(PortOutcome.Unavailable, t.portCall(PORT, FAIL_OTHER, 6u, NO_BYTES))
                    assertTrue(log.records.any { it.thrown is IllegalStateException })
                }
            }
        }

        case("an unregistered port or method is unavailable") {
            val t = FakeTransport()
            attach(t, adapters = mapOf(PORT to PortImpl(true, mapOf(ECHO to { a: ByteArray -> a }))), defaultAdapters = false).use {
                assertEq(PortOutcome.Unavailable, t.portCall(0x1234u, ECHO, 1u, NO_BYTES))
                assertEq(PortOutcome.Unavailable, t.portCall(PORT, 0x1234u, 1u, NO_BYTES))
            }
        }

        case("an async port answers Async at once and replies later through the transport, off the calling thread") {
            val t = FakeTransport()
            val thread = CopyOnWriteArrayList<String>()
            val impl = PortImpl(
                false,
                mapOf(
                    SLOW to { args: ByteArray ->
                        thread.add(Thread.currentThread().name)
                        delay(50)
                        args.reversedArray()
                    },
                ),
            )
            attach(t, adapters = mapOf(PORT to impl)).use {
                t.onCore { assertEq(PortOutcome.Async, t.portCall(PORT, SLOW, 12u, byteArrayOf(1, 2, 3))) }
                eventually("the reply is sent") { t.portReplies.isNotEmpty() }
                val r = t.portReplies.single()
                assertEq(12u, r.portCallId)
                assertEq(PortStatus.OK, r.status)
                assertEq(listOf<Byte>(3, 2, 1), r.body.toList())
                assertTrue(thread.single() != "fake-core", "the implementation ran on the core thread")
            }
        }

        case("an async port's typed error, other failure and own timeout are answered, never dropped") {
            val t = FakeTransport()
            val impl = PortImpl(
                false,
                mapOf(
                    FAIL_TYPED to { _: ByteArray -> throw KeelPortException(byteArrayOf(4)) },
                    FAIL_OTHER to { _: ByteArray -> throw IllegalStateException("boom") },
                    TIMEOUT to { _: ByteArray -> withTimeout(20) { delay(60_000) }; NO_BYTES },
                ),
            )
            LogCapture("dev.keel.runtime").use {
                attach(t, adapters = mapOf(PORT to impl)).use {
                    assertEq(PortOutcome.Async, t.portCall(PORT, FAIL_TYPED, 1u, NO_BYTES))
                    assertEq(PortOutcome.Async, t.portCall(PORT, FAIL_OTHER, 2u, NO_BYTES))
                    assertEq(PortOutcome.Async, t.portCall(PORT, TIMEOUT, 3u, NO_BYTES))
                    eventually("three replies") { t.portReplies.size == 3 }
                    val byId = t.portReplies.associateBy { it.portCallId }
                    assertEq(PortStatus.ERROR, byId.getValue(1u).status)
                    assertEq(listOf<Byte>(4), byId.getValue(1u).body.toList())
                    assertEq(PortStatus.UNAVAILABLE, byId.getValue(2u).status)
                    assertEq(0, byId.getValue(2u).body.size)
                    assertEq(PortStatus.UNAVAILABLE, byId.getValue(3u).status)
                }
            }
        }

        case("many async port calls run concurrently and each is answered once") {
            val t = FakeTransport()
            val impl = PortImpl(false, mapOf(ECHO to { args: ByteArray -> delay(10); args }))
            attach(t, adapters = mapOf(PORT to impl)).use {
                for (i in 0 until 200) t.portCall(PORT, ECHO, i.toUInt(), byteArrayOf(i.toByte()))
                eventually("all answered") { t.portReplies.size == 200 }
                assertEq((0 until 200).map { it.toUInt() }.toSet(), t.portReplies.map { it.portCallId }.toSet())
                assertTrue(t.portReplies.all { it.status == PortStatus.OK })
            }
        }

        case("closing the core cancels port work that is still running and sends nothing after") {
            val t = FakeTransport()
            val started = CompletableFuture<Unit>()
            val cancelled = CompletableFuture<Boolean>()
            val impl = PortImpl(
                false,
                mapOf(
                    SLOW to { _: ByteArray ->
                        started.complete(Unit)
                        try {
                            delay(60_000)
                        } catch (e: kotlinx.coroutines.CancellationException) {
                            cancelled.complete(true)
                            throw e
                        }
                        NO_BYTES
                    },
                ),
            )
            val core = attach(t, adapters = mapOf(PORT to impl))
            t.portCall(PORT, SLOW, 1u, NO_BYTES)
            started.get(10, TimeUnit.SECONDS)
            core.close()
            assertTrue(cancelled.get(10, TimeUnit.SECONDS))
            assertEq(0, t.portReplies.size)
        }

        case("registerPort after loading works and replaces an earlier registration") {
            val t = FakeTransport()
            attach(t).use { core ->
                assertEq(PortOutcome.Unavailable, t.portCall(PORT, ECHO, 1u, NO_BYTES))
                core.registerPort(PORT, PortImpl(true, mapOf(ECHO to { _: ByteArray -> byteArrayOf(1) })))
                assertEq(listOf<Byte>(1), reply(t.portCall(PORT, ECHO, 2u, NO_BYTES)).body.toList())
                core.registerPort(PORT, PortImpl(true, mapOf(ECHO to { _: ByteArray -> byteArrayOf(2) })))
                assertEq(listOf<Byte>(2), reply(t.portCall(PORT, ECHO, 3u, NO_BYTES)).body.toList())
            }
        }

        case("the default JVM adapters serve the standard ports, and explicit adapters win over them") {
            val t = FakeTransport()
            attach(t, defaultAdapters = true).use {
                // Clock is a default: a sync port answering the current time.
                val now = Codecs.i64.decodeAll(reply(t.portCall(StandardPorts.Clock.PORT_ID, StandardPorts.Clock.NOW_MS, 1u, NO_BYTES)).body)
                assertTrue(kotlin.math.abs(now - System.currentTimeMillis()) < 5_000, "clock is wall time: $now")
            }
            val override = PortImpl(true, mapOf(StandardPorts.Clock.NOW_MS to { _: ByteArray -> Codecs.i64.encodeToByteArray(123L) }))
            val t2 = FakeTransport()
            attach(t2, adapters = mapOf(StandardPorts.Clock.PORT_ID to override), defaultAdapters = true).use {
                val body = reply(t2.portCall(StandardPorts.Clock.PORT_ID, StandardPorts.Clock.NOW_MS, 1u, NO_BYTES)).body
                assertEq(123L, Codecs.i64.decodeAll(body))
            }
        }

        case("timers set through the Timer port come back as timerFired on the core") {
            val t = FakeTransport()
            attach(t, defaultAdapters = true).use {
                fun set(id: UInt, delayMs: ULong) {
                    val w = dev.keel.runtime.wire.KeelWriter()
                    w.writeU32(id)
                    w.writeU64(delayMs)
                    val outcome = t.portCall(StandardPorts.Timer.PORT_ID, StandardPorts.Timer.SET, id, w.toByteArray())
                    assertEq(PortStatus.OK, reply(outcome).status)
                }
                set(2u, 120uL)
                set(1u, 30uL)
                assertEq(0, t.timers.size)
                eventually("both timers fire") { t.timers.size == 2 }
                assertEq(listOf(1u, 2u), t.timers.toList(), "in due order")
            }
        }

        case("a sync port implementation that calls back into the core from a core callback is told off, not deadlocked") {
            // Uses the in-process transport over a fake of the JNI contract: see InprocTransportTests.
            val native = dev.keel.runtime.support.FakeNative()
            val transport = InprocTransport(native)
            var failure: Throwable? = null
            val core = KeelCore.attach(
                transport,
                LoadOptions(
                    expectedSchemaHash = dev.keel.runtime.support.HASH,
                    defaultAdapters = false,
                    adapters = mapOf(
                        PORT to PortImpl(
                            true,
                            mapOf(
                                ECHO to { _: ByteArray ->
                                    try {
                                        // A sync port must not call the core: the lock may be held.
                                        transport.event(1u, 2u, NO_BYTES)
                                    } catch (e: KeelException) {
                                        failure = e
                                    }
                                    NO_BYTES
                                },
                            ),
                        ),
                    ),
                ),
                makeShared = false,
            )
            core.use {
                val outcome = native.portCall(PORT.toInt(), ECHO.toInt(), 1, NO_BYTES)
                assertEq(0, outcome, "the port itself still answers")
                assertTrue(failure?.message?.contains("inside a core callback") == true, "got $failure")
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
