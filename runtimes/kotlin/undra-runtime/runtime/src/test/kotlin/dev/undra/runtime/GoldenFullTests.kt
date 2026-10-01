package dev.undra.runtime

import dev.undra.runtime.support.FakeTransport
import dev.undra.runtime.support.attach
import dev.undra.runtime.support.replyPayload
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.WireException
import dev.undra.runtime.wire.encodeToByteArray
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Test

/**
 * The bindgen `full` golden case (`crates/undra-bindgen/tests/golden/full/kotlin`) compiled against this
 * real runtime, and the execution test bindgen ships for it (`tests/fixtures/kotlin-run/full/FullTest.kt`,
 * which drives the generated objects, stores, ports and queries through a fake `UndraCore` subclass, a
 * fake `Mirror` and the real wire layer).
 *
 * Compiling is the point: generated code depends on exactly the runtime surface of SPEC 17.2 plus the
 * additions in bindgen's `UndraBase.kt` fixture, and this is where a runtime change that breaks it shows.
 * `scripts/test-local.sh` and the Gradle build add both source trees to the test compilation.
 *
 * The generated classes are reached by reflection, so this suite still compiles (and skips) when they are
 * left out of the build (`UNDRA_SKIP_GOLDEN=1`).
 */
class GoldenFullTests : Suite() {
    private fun goldenClass(name: String): Class<*> =
        try {
            Class.forName(name)
        } catch (e: ClassNotFoundException) {
            skip("$name is not on the classpath (bindgen sources not found)")
        }

    /**
     * Collects the generated `Calculator.watch(priority)`, a `Result<Stream<Todo>, TodoError>` method, over a
     * real [UndraCore] whose fake core opens the stream and ends it with [end], and returns what the
     * collection threw.
     */
    private fun watchEndingWith(end: (FakeTransport, Payloads.Call) -> Unit): Throwable {
        val calculator = goldenClass("golden.full.Calculator")
        val priority = goldenClass("golden.full.Priority")
        val t = FakeTransport()
        t.onCallSync = { call -> replyPayload(call.callId, ReplyStatus.OK, Codecs.handle.encodeToByteArray(0x100000002L)) }
        t.onCall = { call -> end(t, call) }
        attach(t).use { core ->
            (calculator.getConstructor(UndraCore::class.java).newInstance(core) as AutoCloseable).use { calc ->
                val watch = calculator.getMethod("watch", priority)
                val flow = watch.invoke(calc, priority.enumConstants!!.first()) as Flow<*>
                return assertThrows<Throwable> { runBlocking { flow.toList() } }
            }
        }
    }

    init {
        case("the generated full golden output compiles against the real runtime and passes its execution test") {
            val main = try {
                Class.forName("golden.full.FullTestKt").getMethod("main")
            } catch (e: ClassNotFoundException) {
                skip("golden.full.FullTestKt is not on the classpath (bindgen sources not found)")
            }
            main.invoke(null)
        }

        case("a typed stream the core cancels (flag 3) reaches the app as UndraCallError.CancelledByCore, not as its E (ADR-036)") {
            val e = watchEndingWith { t, call ->
                t.serveStream(call, emptyList(), failed = Payloads.StreamFailure(ReplyStatus.CANCELLED, "a restore replaced the receiver", ""))
            }
            // Before ADR-036 the core sent a String under flag 2 and the generated fromReply decoded it as
            // TodoError, which threw a WireException.
            assertTrue(e !is WireException, "a WireException escaped the UndraException hierarchy: $e")
            assertTrue(e is UndraCallError.CancelledByCore, "expected UndraCallError.CancelledByCore, got $e")
        }

        case("a typed stream that panicked (flag 3) reaches the app as UndraCallError.Panicked with its message and backtrace") {
            val e = watchEndingWith { t, call ->
                t.serveStream(call, emptyList(), failed = Payloads.StreamFailure(ReplyStatus.PANIC, "boom", "at core.rs:1"))
            }
            assertTrue(e is UndraCallError.Panicked, "expected UndraCallError.Panicked, got $e")
            assertEq("boom", (e as UndraCallError.Panicked).panicMessage)
            assertEq("at core.rs:1", e.backtrace)
        }

        case("a typed stream the core refused (flag 3, status 5) reaches the app as UndraCallError.Refused with its reason") {
            val e = watchEndingWith { t, call ->
                t.serveStream(call, emptyList(), failed = Payloads.StreamFailure(ReplyStatus.BAD_REQUEST, "stale handle", ""))
            }
            assertTrue(e is UndraCallError.Refused, "expected UndraCallError.Refused, got $e")
            assertEq("stale handle", (e as UndraCallError.Refused).reason)
        }

        case("a typed stream that ends with its own E (flag 2) still reaches the app as that E") {
            // TodoError::EmptyTitle is variant 0: a u16 tag and nothing else.
            val e = watchEndingWith { t, call -> t.serveStream(call, emptyList(), failure = byteArrayOf(0, 0)) }
            assertEq("golden.full.TodoError\$EmptyTitle", e.javaClass.name)
            assertTrue(e is UndraException, "a generated error is an UndraException: $e")
        }
    }

    @Test
    fun allCases() = assertPassed()
}
