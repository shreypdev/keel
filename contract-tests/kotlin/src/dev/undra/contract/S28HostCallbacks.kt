package dev.undra.contract

import dev.undra.playground.core.ReportError
import dev.undra.playground.core.Reporter
import dev.undra.playground.core.Workshop
import dev.undra.runtime.UndraCallError
import java.lang.ref.WeakReference
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.coroutines.cancellation.CancellationException
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking

/**
 * The runner's `Reporter`: records every call in order, with the workshop's `notes` as the app sees it at that
 * moment; [confirm] does what the step says.
 */
private open class RecordingReporter(private val workshop: Workshop?) : Reporter {
    val calls = CopyOnWriteArrayList<String>()

    @Volatile var confirm: suspend (String) -> Boolean = { true }

    @Volatile var onNote: (String) -> Unit = {}

    override fun progress(done: UInt, total: UInt) {
        calls.add("progress $done/$total")
    }

    override fun note(line: String) {
        calls.add(if (workshop == null) "note $line" else "note $line @${workshop.notes.value}")
        onNote(line)
    }

    override suspend fun confirm(question: String): Boolean = confirm.invoke(question)

    fun notes(): List<String> = calls.filter { it.startsWith("note ") }

    fun progress(): List<String> = calls.filter { it.startsWith("progress ") }
}

/**
 * S28 (ADR-041): the core calls the app's `Reporter` back. Calls run on the main thread through the mirror's drain,
 * after the change-sets committed before them; an `async` method answers with a value or its typed error; any other
 * throw is reported; cancellation reaches the host's job; the registry interns, counts and empties; `coalesce` keeps
 * the newest; a refused call gives its reference back; the registry holds strongly, `Reporter.weak` weakly.
 */
fun s28HostCallbacks(w: World) {
    val core = w.core
    val callbacks = core.callbacks
    w.takeUnhandled()
    val workshop = onMain { Workshop() }
    val baseline = callbacks.liveCount
    try {
        // 1. Ordered, after the change-sets committed before them; the listener may call into the core.
        val rep = RecordingReporter(workshop)
        var nested = false
        rep.onNote = { line ->
            if (line == "two" && !nested) {
                nested = true
                workshop.announce("from note")
            }
        }
        val watch = onMain { workshop.watch(rep) }
        val notes0 = workshop.notes.value
        onMain {
            workshop.announce("one")
            workshop.announce("two")
        }
        awaitEq("the notes the reporter heard", listOf("note one @${notes0 + 1u}", "note two @${notes0 + 2u}", "note from note @${notes0 + 3u}")) { rep.notes() }
        expectEq("reports after the nested announce", emptyList<String>(), w.takeUnhandled().map { it.operation })
        watch.close()

        // 2. An async callback returns a value, and throws its typed error.
        val r2 = RecordingReporter(null)
        expectEq("run(3) when confirm answers true", 3u, runBlocking { workshop.run(3u, r2) })
        expectEq("the notes of run(3)", listOf("note step 1 of 3", "note step 2 of 3", "note step 3 of 3"), r2.notes())
        val progress = r2.progress()
        check(progress.isNotEmpty() && progress.last() == "progress 3/3") { "the progress of run(3): $progress" }
        check(progress == progress.sortedBy { it.substringAfter(' ').substringBefore('/').toInt() }) { "progress out of order: $progress" }
        r2.confirm = { false }
        expectFailsAsync<ReportError.Declined>("run(1) when confirm answers false") { workshop.run(1u, r2) }
        r2.confirm = { throw ReportError.Unavailable("x") }
        val typed = expectFailsAsync<ReportError.Unavailable>("run(1) when confirm throws Unavailable(x)") { workshop.run(1u, r2) }
        expectEq("the typed error's value", "x", typed.value)

        // 3. Any other throw is reported and answered unavailable; a throw from note goes nowhere else.
        r2.confirm = { throw IllegalStateException("not a ReportError") }
        expectFailsAsync<ReportError.Unavailable>("run(1) when confirm throws IllegalStateException") { workshop.run(1u, r2) }
        awaitEq("the report of the throwing confirm", listOf("Reporter.confirm")) { w.unhandled.map { it.operation } }
        w.takeUnhandled()
        val throwing = object : RecordingReporter(null) {
            override fun note(line: String) = throw IllegalStateException("a note that throws")
        }
        val throwingWatch = onMain { workshop.watch(throwing) }
        onMain { workshop.announce("boom") }
        awaitEq("the report of the throwing note", listOf("Reporter.note")) { w.unhandled.map { it.operation } }
        w.takeUnhandled()
        throwingWatch.close()

        // 4. Cancellation reaches the host task; the registry lets go once the core's proxy is gone.
        cancellation(w, workshop)

        // 5. Interning and the registry.
        interning(w, workshop, baseline)

        // 6. coalesce delivers only the newest per drain (the burst runs on the main thread, which holds the frame).
        val r6 = RecordingReporter(null)
        onMain { workshop.burst(50u, r6) }
        awaitEq("the notes of burst(50)", (1..50).map { "note burst $it" }) { r6.notes() }
        expectEq("the progress of burst(50)", listOf("progress 50/50"), r6.progress())

        // 7. A refused call leaves the registry unchanged.
        val closed = onMain { Workshop() }
        closed.close()
        val r7 = RecordingReporter(null)
        val before7 = callbacks.liveCount
        expectFails<UndraCallError.Refused>("watch on a closed workshop") { closed.watch(r7) }
        expectEq("the registry's count of the refused reporter", 0, callbacks.count(r7))
        expectEq("the registry after the refused call", before7, callbacks.liveCount)

        // 8. Strong by default; Reporter.weak(target) forwards while the target lives.
        strongAndWeak(w, workshop)
        awaitEq("the registry back at its baseline", baseline) { callbacks.liveCount }
        expectEq("reports at the end", emptyList<String>(), w.takeUnhandled().map { it.operation })
        // 9. No background-delivery interface in the playground: covered by the golden case and the runtime's tests.

        // 10. A stream takes a callback (objects-followups O1): the core holds the reference while the stream runs and
        //     lets go when it ends; a stream the core refuses gives it back.
        val r10 = RecordingReporter(null)
        expectEq("walk(3)", listOf(1u, 2u, 3u), runBlocking { workshop.walk(3u, r10).toList() })
        awaitEq("the notes of walk(3)", (1..3).map { "note walk $it of 3" }) { r10.notes() }
        awaitEq("the registry's count of the walk's reporter once the stream ended", 0) { callbacks.count(r10) }
        val closed10 = onMain { Workshop() }
        closed10.close()
        val r10b = RecordingReporter(null)
        expectFailsAsync<UndraCallError.Refused>("walk on a closed workshop") { closed10.walk(1u, r10b).toList() }
        expectEq("the registry's count of the refused stream's reporter", 0, callbacks.count(r10b))
        awaitEq("the registry back at its baseline after the streams", baseline) { callbacks.liveCount }
        expectEq("reports after the streams", emptyList<String>(), w.takeUnhandled().map { it.operation })
    } finally {
        workshop.close()
    }
}

/** Step 4. */
private fun cancellation(w: World, workshop: Workshop) {
    val observed = CopyOnWriteArrayList<String>()
    val r4 = RecordingReporter(null)
    r4.confirm = {
        observed.add("started")
        try {
            awaitCancellation()
        } catch (e: CancellationException) {
            observed.add("cancelled")
            throw e
        }
    }
    val outcome = runBlocking {
        val running = async(Dispatchers.Default, start = CoroutineStart.UNDISPATCHED) { workshop.run(1u, r4) }
        awaitUntil("confirm to start") { observed.contains("started") }
        running.cancel()
        try {
            running.await()
            "returned"
        } catch (e: CancellationException) {
            "cancelled"
        } catch (e: Throwable) {
            "failed with $e"
        }
    }
    expectEq("run(1) after its cancellation", "cancelled", outcome)
    // The default wait (WAIT_MS), a hang detector: `confirm` waits for its cancellation and nothing else, on no timer, so a
    // cancellation that did not reach it would leave it waiting; how soon it is seen is the machine's.
    awaitUntil("the host's confirm to observe the cancellation") { observed.contains("cancelled") }
    awaitEq("the registry's count of the cancelled run's reporter once the core dropped its proxy", 0) { w.core.callbacks.count(r4) }
    holdsFor("no report after the cancellation") { w.unhandled.isEmpty() }
}

/** Step 5: one instance handle for one reporter; the duplicate reference is given back; closing the watches empties it. */
private fun interning(w: World, workshop: Workshop, baseline: Int) {
    val callbacks = w.core.callbacks
    val ref = watchTwice(w, workshop)
    val (first, second) = ref.second
    expectEq("watching() with two subscriptions", 2u, workshop.watching())
    first.close()
    second.close()
    expectEq("watching() after closing both", 0u, workshop.watching())
    awaitEq("the registry once the core dropped the last proxy", baseline) { callbacks.liveCount }
    awaitUntil("the reporter to be collected once nothing holds it") {
        System.gc()
        ref.first.get() == null
    }
}

/** Watches with one reporter twice and returns a weak reference to it and the two subscriptions. */
private fun watchTwice(w: World, workshop: Workshop): Pair<WeakReference<RecordingReporter>, Pair<AutoCloseable, AutoCloseable>> {
    val callbacks = w.core.callbacks
    val r5 = RecordingReporter(null)
    val first = onMain { workshop.watch(r5) }
    val handle = callbacks.instanceOf(r5)
    val second = onMain { workshop.watch(r5) }
    expectEq("the instance handle of the reporter passed twice", handle, callbacks.instanceOf(r5))
    awaitEq("the registry's count once the core gave the duplicate back", 1) { callbacks.count(r5) }
    return WeakReference(r5) to (first to second)
}

/** Step 8. */
private fun strongAndWeak(w: World, workshop: Workshop) {
    val heard = CopyOnWriteArrayList<String>()
    val inline = onMain { workshop.watch(InlineReporter(heard)) }
    repeat(3) {
        System.gc()
        Thread.sleep(20)
    }
    onMain { workshop.announce("after a GC") }
    awaitEq("what the inline reporter heard after a GC", listOf("after a GC")) { heard.toList() }
    inline.close()

    // The weak wrapper forwards while its target lives...
    val target = RecordingReporter(null)
    val weakWatch = onMain { workshop.watch(Reporter.weak(target)) }
    onMain { workshop.announce("alive") }
    awaitEq("what the weak wrapper's target heard", listOf("note alive")) { target.notes() }
    weakWatch.close()
    // ... and once it is gone, does nothing (fire-and-forget) or answers unavailable (async), unreported.
    val (weak, gone) = weakOfDropped()
    awaitUntil("the weak wrapper's target to be collected") {
        System.gc()
        gone.get() == null
    }
    val goneWatch = onMain { workshop.watch(weak) }
    onMain { workshop.announce("nobody") }
    expectFailsAsync<ReportError.Unavailable>("run(1) through a weak wrapper whose target is gone") { workshop.run(1u, weak) }
    holdsFor("no report from the weak wrapper") { w.unhandled.isEmpty() }
    goneWatch.close()
}

/** A weak wrapper of a reporter nothing else holds, and a weak reference to that reporter. */
private fun weakOfDropped(): Pair<Reporter, WeakReference<Reporter>> {
    val target = RecordingReporter(null)
    return Reporter.weak(target) to WeakReference(target)
}

/** A reporter created inline that nothing but the registry holds. */
private class InlineReporter(private val heard: MutableList<String>) : Reporter {
    override fun progress(done: UInt, total: UInt) = Unit

    override fun note(line: String) {
        heard.add(line)
    }

    override suspend fun confirm(question: String): Boolean = true
}
