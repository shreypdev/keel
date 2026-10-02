// Runs the generated Kotlin of the `object_graph` golden case (ADR-040) against the real runtime and a fake core.
// Compiled and run by `tests/typecheck_kotlin.rs`; exits non-zero on the first failed expectation.

package golden.object_graph

import dev.undra.runtime.Mirror
import dev.undra.runtime.UndraCallError
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraReplyException
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.encodeToByteArray
import kotlinx.coroutines.runBlocking

private fun ByteArray.hex(): String = joinToString("") { "%02x".format(it) }

private fun expect(condition: Boolean, message: String) {
    if (!condition) throw AssertionError(message)
}

private fun <T> expectEq(actual: T, expected: T, message: String) {
    if (actual != expected) throw AssertionError("$message: expected <$expected>, got <$actual>")
}

private class FakeMirror : Mirror() {
    val registered = mutableSetOf<Long>()

    override fun register(handle: Long, apply: (UInt, ChangeOp, UndraReader) -> Unit) {
        registered += handle
    }

    override fun unregister(handle: Long) {
        registered -= handle
    }
}

/** A core that answers every call with the next queued reply body and records what crossed. */
private class FakeCore : UndraCore() {
    val calls = mutableListOf<Pair<UInt, String>>()
    val replies = ArrayDeque<Any>()
    val releases = mutableListOf<Long>()
    val observed = mutableListOf<Long>()
    val reports = mutableListOf<Pair<String, Throwable>>()
    var nextHandle = 1L
    private val fakeMirror = FakeMirror()
    override val mirror: Mirror get() = fakeMirror

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

    override fun construct(typeId: UInt, methodId: UInt, args: ByteArray): Long = nextHandle++

    override fun observe(handle: Long, signalId: UInt, on: Boolean) {
        if (on) observed += handle
    }

    override fun release(handle: Long) {
        releases += handle
    }

    override fun report(error: Throwable, operation: String) {
        reports += operation to UndraCallError.mapped(error)
    }

    fun answer(vararg handles: Long) = replies.addLast(Codecs.handle.encodeToByteArray(handles.single()))

    fun answerList(vararg handles: Long) = replies.addLast(Codecs.vec(Codecs.handle).encodeToByteArray(handles.toList()))

    fun answerOptional(handle: Long?) = replies.addLast(Codecs.option(Codecs.handle).encodeToByteArray(handle))
}

private fun handleHex(handle: Long): String = Codecs.handle.encodeToByteArray(handle).hex()

fun main() {
    val core = FakeCore()
    val account = Account(core)
    expect(account.core === core, "Account(ctx) uses the core it is given")

    // A returned object is one wrapper per handle; the duplicate reference is given back at once.
    core.answer(100L)
    val inbox = account.mailbox("inbox")
    core.answer(100L)
    expect(account.mailbox("inbox") === inbox, "mailbox(\"inbox\") twice is one wrapper")
    expectEq(core.releases.toList(), listOf(100L), "the duplicate reference")
    core.answerOptional(null)
    expectEq(account.drafts(), null, "drafts() with none")
    core.answerOptional(100L)
    expect(account.drafts() === inbox, "drafts() is the live wrapper")
    core.answerList(100L, 101L)
    val boxes = account.mailboxes()
    expect(boxes[0] === inbox && boxes[1] !== inbox, "mailboxes() adopts each handle")
    expectEq(core.releases.toList(), listOf(100L, 100L, 100L), "every duplicate given back")

    // A returned store is observed when its wrapper is made, once.
    core.answer(200L)
    val chat = account.chat(7u)
    core.answer(200L)
    expect(account.chat(7u) === chat, "chat(7) twice is one store")
    expectEq(core.observed.count { it == 200L }, 1, "the store observed once")

    // An async return, and a typed error that owes nothing.
    core.answer(300L)
    val thread = runBlocking { account.openThread(3u) }
    expectEq(thread.handle, 300L, "openThread(3)")
    core.replies.addLast(UndraReplyException(ReplyStatus.ERROR, MailError.encodeToByteArray(MailError.NoThread)))
    try {
        runBlocking { account.openThread(4u) }
        throw AssertionError("openThread(4) did not fail")
    } catch (e: MailError.NoThread) {
        // expected
    }

    // Object parameters are written as their handles; a command reports a foreign object and sends nothing.
    val sent = core.calls.size
    account.moveTo(5u, inbox)
    expectEq(core.calls.last(), UndraIds.Objects.Account.MOVE_TO to ("05000000" + handleHex(100L)), "moveTo(5, inbox)")
    core.replies.addLast(Codecs.u32.encodeToByteArray(2u))
    expectEq(account.merge(boxes, null), 2u, "merge(boxes, null)")
    expectEq(
        core.calls.last().second,
        "02000000" + handleHex(100L) + handleHex(101L) + "00",
        "merge's arguments: the list's handles and no extra",
    )
    val other = FakeCore()
    val foreign = Account(other).let {
        other.answer(100L) // the same handle number in another core
        it.mailbox("inbox")
    }
    val before = core.calls.size
    account.moveTo(6u, foreign)
    expectEq(core.calls.size, before, "nothing sent for a foreign object")
    expectEq(core.reports.single().first, "Account.moveTo", "the refusal is reported")
    expect(core.reports.single().second is UndraCallError.Refused, "as Refused")
    try {
        account.merge(listOf(foreign), inbox)
        throw AssertionError("merge with a foreign mailbox did not fail")
    } catch (e: UndraCallError.Refused) {
        expect(e.reason.contains("Mailbox"), "the refusal names the class: ${e.reason}")
    }
    expectEq(core.calls.size, before, "nothing sent for a foreign object in a list")

    // A free function takes and returns objects.
    core.answer(100L)
    expect(mailboxOf(account, "inbox", core) === inbox, "mailboxOf returns the live wrapper")
    expectEq(core.calls.last().second.take(16), handleHex(account.handle), "mailboxOf writes the account's handle first")

    // Closing gives back the wrapper's one reference; the next return is a new wrapper.
    val released = core.releases.size
    inbox.close()
    inbox.close()
    expectEq(core.releases.size, released + 1, "close releases once")
    core.answer(100L)
    expect(account.mailbox("inbox") !== inbox, "a closed wrapper is replaced")
    expect(sent > 0, "calls were made")
    println("ok")
}
