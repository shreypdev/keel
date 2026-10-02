// Runs the generated Kotlin of the `callbacks` golden case (ADR-041) against the real runtime and a fake core: call
// sites lend and give back, the bridges decode the core's calls and map the implementations' answers.
// Compiled and run by `tests/typecheck_kotlin.rs`; exits non-zero on the first failed expectation.

package golden.callbacks

import dev.undra.runtime.UndraCallError
import dev.undra.runtime.UndraCallbackGoneException
import dev.undra.runtime.UndraCallbackInvocation
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraPortException
import dev.undra.runtime.UndraReplyException
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking

private fun ByteArray.hex(): String = joinToString("") { "%02x".format(it) }

private fun expect(condition: Boolean, message: String) {
    if (!condition) throw AssertionError(message)
}

private fun <T> expectEq(actual: T, expected: T, message: String) {
    if (actual != expected) throw AssertionError("$message: expected <$expected>, got <$actual>")
}

private class FakeCore : UndraCore() {
    val calls = mutableListOf<Pair<UInt, String>>()
    val constructed = mutableListOf<String>()
    val replies = ArrayDeque<Any>()
    val streams = ArrayDeque<List<Any>>()
    val reports = mutableListOf<String>()
    var nextHandle = 1L

    private fun reply(): ByteArray = when (val next = replies.removeFirstOrNull()) {
        null -> ByteArray(0)
        is Throwable -> throw next
        else -> next as ByteArray
    }

    override fun callSync(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
        calls += methodId to args.hex()
        return reply()
    }

    override suspend fun call(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray = callSync(target, methodId, args)

    override fun stream(target: CallTarget, methodId: UInt, args: ByteArray): Flow<ByteArray> {
        calls += methodId to args.hex()
        val items = streams.removeFirstOrNull() ?: emptyList()
        return flow {
            for (item in items) {
                if (item is Throwable) throw item
                emit(item as ByteArray)
            }
        }
    }

    override fun construct(typeId: UInt, methodId: UInt, args: ByteArray): Long {
        constructed += args.hex()
        reply()
        return nextHandle++
    }

    override fun release(handle: Long) = Unit

    override fun report(error: Throwable, operation: String) {
        reports += operation
    }
}

private class Listener : UploadListener {
    val heard = mutableListOf<String>()
    var answer: () -> Boolean = { true }

    override fun progress(sent: ULong, total: ULong) {
        heard += "progress $sent/$total"
    }

    override fun finished(name: String) {
        heard += "finished $name"
    }

    override suspend fun confirmReplace(name: String): Boolean = answer()
}

private fun refused(): UndraReplyException {
    val w = UndraWriter()
    w.writeStr("stale handle")
    return UndraReplyException(ReplyStatus.BAD_REQUEST, w.toByteArray())
}

private fun u64(v: ULong): String = Codecs.u64.encodeToByteArray(v).hex()

private fun handleHex(handle: Long): String = Codecs.handle.encodeToByteArray(handle).hex()

fun main() {
    val core = FakeCore()
    val listener = Listener()

    // A constructor with an optional callback: none, then one (lent: one reference the core now owns).
    Uploader.create(null, core)
    expectEq(core.constructed.last(), "00", "create(null) writes no instance")
    val uploader = Uploader.create(listener, core)
    val instance = core.callbacks.instanceOf(listener) ?: throw AssertionError("create(listener) lent nothing")
    expectEq(core.constructed.last(), "01" + u64(instance), "create(listener) writes the instance handle")
    expectEq(core.callbacks.count(listener), 1, "one crossing, one reference")

    // The same listener again is the same instance handle, one more reference.
    core.replies.addLast(Codecs.handle.encodeToByteArray(50L))
    val watch = uploader.watch(listener)
    expectEq(watch.handle, 50L, "watch returns its object")
    expectEq(core.calls.last().second, u64(instance), "watch writes the same instance")
    expectEq(core.callbacks.count(listener), 2, "two crossings")

    // A refused call gives its reference back; a call the core took keeps it.
    core.replies.addLast(refused())
    try {
        runBlocking { uploader.upload("a.txt", listener) }
        throw AssertionError("a refused upload did not fail")
    } catch (e: UndraCallError.Refused) {
        // expected
    }
    expectEq(core.callbacks.count(listener), 2, "the refused crossing was given back")
    core.replies.addLast(UndraReplyException(ReplyStatus.ERROR, UploadError.encodeToByteArray(UploadError.Failed)))
    try {
        runBlocking { uploader.upload("a.txt", listener) }
        throw AssertionError("a failed upload did not fail")
    } catch (e: UploadError.Failed) {
        // expected
    }
    expectEq(core.callbacks.count(listener), 3, "the core took the failed upload's reference")

    // A command reports a refusal and gives back; an optional callback that is null lends nothing.
    val provider = object : TokenProvider {
        override suspend fun token(account: String): String = "t-$account"
        override fun refreshed(account: String) = Unit
    }
    core.replies.addLast(refused())
    uploader.setProvider(provider)
    expectEq(core.reports.toList(), listOf("Uploader.setProvider"), "the refused command is reported")
    expectEq(core.callbacks.count(provider), 0, "and its reference given back")
    uploader.notify(null)
    expectEq(core.calls.last().second, "00", "notify(null)")
    core.replies.addLast(Codecs.u32.encodeToByteArray(1u))
    expectEq(withListener(listener, core), 1u, "a free function takes a callback")

    // Streams that take callbacks (objects-followups O1). Every collection lends once; a stream the core refuses (or
    // that never reached it) gives the reference back, and anything the core served leaves it there: its own
    // failures, the decoding of an item it sent, and what the collector does.
    core.streams.clear()
    val streamListener = Listener()
    val count = { core.callbacks.count(streamListener) }
    core.replies.addLast(Codecs.handle.encodeToByteArray(51L))
    val followed = uploader.watch(streamListener)
    expectEq(count(), 1, "watch lent the listener once")
    val u32 = { n: UInt -> Codecs.u32.encodeToByteArray(n) }
    val follow = uploader.follow(followed, streamListener)
    expectEq(count(), 1, "nothing is lent until the stream is collected")
    core.streams.add(listOf(u32(1u), u32(2u)))
    expectEq(runBlocking { follow.toList() }, listOf(1u, 2u), "follow's items")
    expectEq(count(), 2, "one crossing, one reference")
    core.streams.add(listOf(u32(3u)))
    expectEq(runBlocking { follow.toList() }, listOf(3u), "a cold flow: the next collection is another crossing")
    expectEq(count(), 3, "lent again")
    val streamInstance = core.callbacks.instanceOf(streamListener) ?: throw AssertionError("the listener is lent")
    expectEq(core.calls.last().second, handleHex(51L) + u64(streamInstance), "the object's handle, then the instance")

    // Refused: given back. A failure the core produced, an item, a failing decode and a throwing collector: kept.
    core.streams.add(listOf(refused()))
    try {
        runBlocking { follow.toList() }
        throw AssertionError("a refused stream did not fail")
    } catch (e: UndraCallError.Refused) {
        // expected
    }
    expectEq(count(), 3, "the refused stream's reference was given back")
    core.streams.add(listOf(UndraReplyException(ReplyStatus.CANCELLED, ByteArray(0))))
    try {
        runBlocking { follow.toList() }
        throw AssertionError("a cancelled stream did not fail")
    } catch (e: UndraCallError.CancelledByCore) {
        // expected
    }
    expectEq(count(), 4, "the core owns the reference of a stream it cancelled")
    core.streams.add(listOf(ByteArray(1)))
    try {
        runBlocking { follow.toList() }
        throw AssertionError("an item that does not decode did not fail")
    } catch (e: UndraCallError.Malformed) {
        // expected
    }
    expectEq(count(), 5, "an item the core sent that does not decode (not a refusal) leaves the reference with it")
    core.streams.add(listOf(u32(7u)))
    try {
        runBlocking { follow.collect { throw UndraCallError.Refused("the collector's own failure") } }
        throw AssertionError("a throwing collector did not fail")
    } catch (e: UndraCallError.Refused) {
        expectEq(e.reason, "the collector's own failure", "the collector's failure passes through")
    }
    expectEq(count(), 6, "a collector that throws a runtime exception is not the core refusing the stream")

    // An optional callback that is absent lends nothing; the checked stream is refused like the others.
    core.streams.add(listOf(u32(5u)))
    expectEq(runBlocking { uploader.followChecked(null).toList() }, listOf(5u), "followChecked(null)")
    expectEq(count(), 6, "nothing lent")
    core.streams.add(listOf(refused()))
    try {
        runBlocking { uploader.followChecked(streamListener).toList() }
        throw AssertionError("a refused checked stream did not fail")
    } catch (e: UndraCallError.Refused) {
        // expected
    }
    expectEq(count(), 6, "given back")
    core.streams.add(listOf(u32(4u)))
    expectEq(runBlocking { tail(streamListener, core).toList() }, listOf(4u), "a free function stream lends too")
    expectEq(count(), 7, "lent")

    // The bridges decode the core's calls and answer for the implementation.
    fun args(write: UndraWriter.() -> Unit): UndraReader = UndraReader(UndraWriter().apply(write).toByteArray())
    val progress = UploadListenerBridge.invocation(UndraIds.Callbacks.UploadListener.PROGRESS, args { writeU64(5u); writeU64(9u) })
    (progress as UndraCallbackInvocation.Notify<UploadListener>).run(listener)
    expectEq(listener.heard.toList(), listOf("progress 5/9"), "progress decoded and called")
    expectEq(UploadListenerBridge.coalesced, setOf(UndraIds.Callbacks.UploadListener.PROGRESS), "progress coalesces")
    expect(!UploadListenerBridge.background && TokenProviderBridge.background, "main and background interfaces")
    val ask = UploadListenerBridge.invocation(UndraIds.Callbacks.UploadListener.CONFIRM_REPLACE, args { writeStr("x") })
        as UndraCallbackInvocation.Ask<UploadListener>
    expectEq(runBlocking { Codecs.bool.decodeAll(ask.run(listener)) }, true, "confirmReplace answers true")
    listener.answer = { throw PromptError.Declined }
    try {
        runBlocking { ask.run(listener) }
        throw AssertionError("a declined confirmReplace did not throw")
    } catch (e: UndraPortException) {
        expectEq(PromptError.decodeAll(e.body), PromptError.Declined, "the typed error crosses as the port's error")
    }
    val token = TokenProviderBridge.invocation(UndraIds.Callbacks.TokenProvider.TOKEN, args { writeStr("me") })
        as UndraCallbackInvocation.Ask<TokenProvider>
    expectEq(runBlocking { Codecs.string.decodeAll(token.run(provider)) }, "t-me", "token decoded and answered")
    expectEq(UploadListenerBridge.invocation(0x1234u, args { }), null, "an unknown method")
    expectEq(UndraIds.Callbacks.UploadListener.PORT_ID, UploadListenerBridge.portId, "the bridge's port")

    // The weak wrapper forwards while its target lives.
    val weak = UploadListener.weak(listener)
    weak.finished("f")
    expectEq(listener.heard.last(), "finished f", "the weak wrapper forwards")
    expect(UndraCallbackGoneException("UploadListener").message!!.contains("UploadListener"), "the gone exception names the interface")
    // `new` has a call syntax for every signature (objects-followups O8): `Uploader(listener, core)` is `create`.
    val invoked = Listener()
    Uploader(invoked, core)
    expectEq(core.callbacks.count(invoked), 1, "Uploader(listener, core) lends like Uploader.create")
    Uploader(null, core)
    expectEq(core.constructed.last(), "00", "Uploader(null, core) writes no instance")
    println("ok")
}
