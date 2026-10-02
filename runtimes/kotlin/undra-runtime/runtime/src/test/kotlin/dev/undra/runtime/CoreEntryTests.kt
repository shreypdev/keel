package dev.undra.runtime

import dev.undra.runtime.support.FakeNative
import dev.undra.runtime.support.HASH
import dev.undra.runtime.support.NO_BYTES
import dev.undra.runtime.support.WsTestServer
import dev.undra.runtime.support.replyPayload
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Envelope
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.atomic.AtomicInteger
import org.junit.jupiter.api.Test

private val FN = CallTarget.FreeFunction(0x51u)

/** Options that keep the JVM default adapters (files, timers) out of these tests. */
private fun bare(): LoadOptions = LoadOptions(defaultAdapters = false)

/** A [FakeNative] for [namespace] whose `callSync` answers a `u32` [answer]. */
private fun fake(namespace: String, answer: UInt = 1u): FakeNative =
    FakeNative(namespace).also { f ->
        f.onCallSync = { call -> replyPayload(call.callId, ReplyStatus.OK, Codecs.u32.encodeToByteArray(answer)) }
    }

/**
 * [CoreEntry], what the generated `Undra<Namespace>` objects delegate to (ADR-044), and the two ways
 * [UndraCore.load] is reached now: the entry fills in the schema hash and hands over the core's natives; a direct
 * `UndraCore.load(options)` only starts remote cores, and refuses options without a hash.
 */
class CoreEntryTests : Suite() {
    init {
        case("before a load, core is a closed placeholder that never touches the natives; load makes it the core; close returns it to the placeholder") {
            val native = fake("entry_lifecycle", answer = 7u)
            val made = AtomicInteger()
            val entry = CoreEntry("entry_lifecycle", HASH) { made.incrementAndGet(); native }
            assertEq("entry_lifecycle", entry.namespace)
            assertEq(HASH, entry.schemaHash)

            val placeholder = entry.core
            assertTrue(entry.core === placeholder, "one placeholder per entry")
            val gone = assertThrows<UndraTransportException> { placeholder.callSync(FN, 0x51u, NO_BYTES) }
            assertEq(UndraTransportException.Reason.CLOSED, gone.reason)
            assertTrue(gone.message!!.contains("`entry_lifecycle` is not loaded"), gone.message!!)
            assertTrue(gone.message!!.contains("Undra<Namespace>.load()"), gone.message!!)
            // A generated call on it is Unavailable, like UndraCore.shared's placeholder.
            assertTrue(UndraCallError.mapped(gone) is UndraCallError.Unavailable)
            placeholder.release(1L) // inert, never throws
            assertEq(0, made.get(), "reading core never asks for the natives")

            val sharedBefore = UndraCore.current
            val core = entry.load(bare())
            try {
                assertEq("entry_lifecycle", core.namespace, "the entry's namespace is the core's: the default stores are kept under it (ADR-044 amendment A)")
                assertEq(1, made.get())
                assertEq(1, native.inits.get())
                assertTrue(entry.core === core, "the loaded core is the entry's core")
                assertEq(Mode.INPROC, core.mode)
                assertEq(7u, Codecs.u32.decodeAll(entry.core.callSync(FN, 0x51u, NO_BYTES)))
                if (sharedBefore == null) assertTrue(UndraCore.current === core, "the first core loaded is also shared")
            } finally {
                core.close()
            }
            assertEq(1, native.shutdowns.get(), "closing ended the native core (ADR-034)")
            assertTrue(entry.core === placeholder, "after close, core is the placeholder again")
            assertTrue(UndraCore.current === sharedBefore, "closing it gave back UndraCore.shared")

            // ...and the core loads again, fresh.
            val again = entry.load(bare())
            try {
                assertTrue(entry.core === again && again !== core)
                assertEq(2, native.inits.get())
            } finally {
                again.close()
            }
            assertTrue(entry.core === placeholder)
        }

        case("a core's namespace is the entry's, else its natives', else the unnamed one") {
            val viaEntry = CoreEntry("entry_ns_a", HASH) { fake("entry_ns_a") }.load(bare())
            val direct = UndraCore.load(bare().let { LoadOptions(defaultAdapters = false, expectedSchemaHash = HASH) }, fake("entry_ns_b"))
            try {
                assertEq("entry_ns_a", viaEntry.namespace)
                assertEq("entry_ns_b", direct.namespace, "a direct load takes the natives' namespace")
            } finally {
                viaEntry.close()
                direct.close()
            }
            assertEq(UndraCore.UNNAMED_NAMESPACE, UnloadedCore(null).namespace)
            assertEq("_", UndraCore.UNNAMED_NAMESPACE, "no real namespace is `_`: it starts with a lowercase letter")
            assertEq("entry_ns_c", UnloadedCore("entry_ns_c").namespace)
            // The options the entry fills in keep everything else, `onDevNotice` included.
            val notice: (String) -> Unit = { }
            val filled = LoadOptions(defaultAdapters = false, onDevNotice = notice).withSchemaHashDefault(HASH).withNamespaceDefault("x")
            assertTrue(filled.onDevNotice === notice, "the copy keeps onDevNotice")
            assertEq("x", filled.namespace)
            assertEq("y", LoadOptions(namespace = "y").withNamespaceDefault("x").namespace, "a namespace in the options wins")
        }

        case("an entry refuses a second load while its core is loaded; another entry for the namespace is refused by the in-process claim") {
            val entry = CoreEntry("entry_twice", HASH) { fake("entry_twice") }
            val core = entry.load(bare())
            try {
                val again = assertThrows<UndraException> { entry.load(bare()) }
                assertTrue(again.message!!.contains("`entry_twice` is already loaded"), again.message!!)
                val twinNative = fake("entry_twice")
                val twin = CoreEntry("entry_twice", HASH) { twinNative }
                val refused = assertThrows<UndraException> { twin.load(bare()) }
                assertTrue(refused.message!!.contains("`entry_twice` is already loaded"), refused.message!!)
                assertEq(0, twinNative.inits.get(), "the refused core never starts")
                assertThrows<UndraTransportException> { twin.core.callSync(FN, 0x51u, NO_BYTES) }
                assertTrue(entry.core === core, "the refusals left the loaded core alone")
            } finally {
                core.close()
            }
        }

        case("two cores with different namespaces load side by side, each the core of its own entry") {
            val nativeA = fake("entry_side_a", answer = 1u)
            val nativeB = fake("entry_side_b", answer = 2u)
            val a = CoreEntry("entry_side_a", HASH) { nativeA }
            val b = CoreEntry("entry_side_b", 0x5150uL) { nativeB }
            nativeB.hash = 0x5150L
            val coreA = a.load(bare())
            try {
                val coreB = b.load(bare())
                try {
                    assertTrue(a.core === coreA && b.core === coreB && coreA !== coreB)
                    assertEq(1u, Codecs.u32.decodeAll(a.core.callSync(FN, 0x51u, NO_BYTES)))
                    assertEq(2u, Codecs.u32.decodeAll(b.core.callSync(FN, 0x51u, NO_BYTES)))
                    assertEq(1, nativeA.syncCalls.size)
                    assertEq(1, nativeB.syncCalls.size)
                } finally {
                    coreB.close()
                }
                assertEq(0, nativeA.shutdowns.get(), "closing one core leaves the other running")
                assertTrue(a.core === coreA)
                assertTrue(b.core !== coreB, "b is back to its placeholder")
            } finally {
                coreA.close()
            }
        }

        case("the entry fills in its schema hash, keeps one the options set, and a mismatch is typed and never starts the core") {
            val native = fake("entry_hash")
            native.hash = 0x0BADL
            val entry = CoreEntry("entry_hash", HASH) { native }
            val mismatch = assertThrows<UndraSchemaMismatchException> { entry.load(bare()) }
            assertEq(HASH, mismatch.expected, "the entry's hash was filled in")
            assertEq(0x0BADuL, mismatch.got)
            assertEq(0, native.inits.get())
            assertThrows<UndraTransportException> { entry.core.callSync(FN, 0x51u, NO_BYTES) }
            // An explicit hash in the options wins over the entry's.
            entry.load(LoadOptions(expectedSchemaHash = 0x0BADuL, defaultAdapters = false)).use { core ->
                assertTrue(entry.core === core)
                assertEq(1, native.inits.get())
            }
        }

        case("a remote load through the entry never touches the natives and sends the entry's schema hash") {
            WsTestServer().use { server ->
                server.onConnect = { conn ->
                    conn.onMessage = { bytes ->
                        val env = Envelope.decode(bytes)
                        if (env.kind == Envelope.Kind.HELLO) conn.sendHello(HASH)
                    }
                }
                val entry = CoreEntry("entry_remote", HASH) { throw AssertionError("a remote load asked for the natives") }
                val core = entry.load(LoadOptions(Mode.REMOTE, remoteUrl = server.url, defaultAdapters = false))
                try {
                    assertEq(Mode.REMOTE, core.mode)
                    assertTrue(entry.core === core)
                    val hello = Payloads.Hello.decode(server.awaitConnection().awaitEnvelope(Envelope.Kind.HELLO).payload)
                    assertEq(HASH, hello.schemaHash)
                    assertThrows<UndraException> { entry.load(LoadOptions(Mode.REMOTE, remoteUrl = server.url, defaultAdapters = false)) }
                } finally {
                    core.close()
                }
                assertTrue(entry.core !== core)
            }
        }

        case("UndraCore.load(options) refuses an in-process core and options without a schema hash") {
            val inproc = assertThrows<UndraModeException> { UndraCore.load(LoadOptions()) }
            assertTrue(inproc.message!!.contains("Undra<Namespace>.load"), inproc.message!!)
            // A contradiction is still reported as such first.
            val withUrl = assertThrows<UndraModeException> { UndraCore.load(LoadOptions(Mode.INPROC, remoteUrl = "ws://localhost:1")) }
            assertTrue(withUrl.message!!.contains("did you mean Mode.REMOTE"), withUrl.message!!)
            val noHash = assertThrows<UndraModeException> { UndraCore.load(LoadOptions(Mode.REMOTE, remoteUrl = "ws://127.0.0.1:1")) }
            assertTrue(noHash.message!!.contains("expectedSchemaHash"), noHash.message!!)
            assertTrue(noHash.message!!.contains("Undra<Namespace>.load"), noHash.message!!)
        }

        case("UndraCore.load(options, native) refuses options without a hash, or with a URL, before touching the core") {
            val native = fake("entry_direct")
            val noHash = assertThrows<UndraModeException> { UndraCore.load(bare(), native) }
            assertTrue(noHash.message!!.contains("expectedSchemaHash"), noHash.message!!)
            assertThrows<UndraModeException> {
                UndraCore.load(LoadOptions(remoteUrl = "ws://localhost:1", expectedSchemaHash = HASH, defaultAdapters = false), native)
            }
            assertEq(0, native.inits.get())
            // With a hash it loads; the native is the core's.
            UndraCore.load(LoadOptions(expectedSchemaHash = HASH, defaultAdapters = false), native).use { core ->
                assertEq(1, native.inits.get())
                assertEq(1u, Codecs.u32.decodeAll(core.callSync(FN, 0x51u, NO_BYTES)))
            }
            assertEq(1, native.shutdowns.get())
        }

        case("LoadOptions: the schema hash is unset by default and printed as such") {
            val o = LoadOptions()
            assertEq(null, o.expectedSchemaHash)
            assertTrue(o.toString().contains("expectedSchemaHash=unset"), o.toString())
            assertTrue(o.withSchemaHashDefault(HASH).expectedSchemaHash == HASH)
            val set = LoadOptions(expectedSchemaHash = 5uL, defaultAdapters = false)
            assertTrue(set.withSchemaHashDefault(HASH) === set, "a set hash is kept")
        }
    }

    @Test
    fun allCases() = assertPassed()
}
