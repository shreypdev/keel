package dev.undra.testkit

import dev.undra.runtime.LoadOptions
import dev.undra.runtime.Mode
import dev.undra.runtime.PortOutcome
import dev.undra.runtime.Transport
import dev.undra.runtime.TransportEvents
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraDispatchers
import dev.undra.runtime.UndraModeException
import dev.undra.runtime.UndraUnhandledError
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.ReplyStatus
import dev.undra.runtime.wire.Payloads.StreamFlag
import kotlinx.coroutines.runBlocking

/** What a call that no recording answers does. */
public enum class Exhausted {
    /** Answer a refused request naming the call (the default). */
    FAIL,

    /** Answer again with the last recorded reply. */
    REPEAT_LAST,
}

/** How a [RecordedCore] matches the calls the app makes to the recorded ones. */
public class ReplayOptions(
    /** `IGNORE` (default) matches a call on its target and method alone, in order; `EXACT` also compares the encoded arguments. */
    public val args: ArgsPolicy = ArgsPolicy.IGNORE,
    /** What a call does when the recording has no (more) reply for it. */
    public val exhausted: Exhausted = Exhausted.FAIL,
    /**
     * Where the playhead starts, in milliseconds of the session. Default (`null`): the time of the first recorded change-set, so that observing a store
     * shows the state the recording first saw.
     */
    public val startAtMs: Long? = null,
)

private class RecordedCall(val args: ByteArray, val callId: UInt, val status: ReplyStatus, val body: ByteArray, val items: MutableList<Pair<StreamFlag, ByteArray>>)

private class ChangeEvent(val t: Long, val txn: ULong, val entries: List<RecordedEntry>)

private fun key(t: RecordedTarget): String = when (t) {
    is RecordedTarget.Function -> "f:${t.method}"
    is RecordedTarget.Method -> "m:${t.handle}:${t.method}"
    is RecordedTarget.Constructor -> "c:${t.type}:${t.method}"
    is RecordedTarget.Page -> "p:${t.handle}:${t.offset}:${t.limit}"
}

private fun key(t: Payloads.CallTarget): String = when (t) {
    is Payloads.CallTarget.FreeFunction -> "f:${t.methodId}"
    is Payloads.CallTarget.ObjectMethod -> "m:${t.handle.raw}:${t.methodId}"
    is Payloads.CallTarget.Constructor -> "c:${t.typeId}:${t.methodId}"
    is Payloads.CallTarget.LazyListPage -> "p:${t.handle.raw}:${t.offset}:${t.limit}"
}

private fun describe(t: Payloads.CallTarget): String = when (t) {
    is Payloads.CallTarget.FreeFunction -> "function ${t.methodId}"
    is Payloads.CallTarget.ObjectMethod -> "method ${t.methodId} of ${t.handle}"
    is Payloads.CallTarget.Constructor -> "constructor ${t.methodId} of type ${t.typeId}"
    is Payloads.CallTarget.LazyListPage -> "page ${t.offset}+${t.limit} of ${t.handle}"
}

private val ALL_SIGNALS = UInt.MAX_VALUE

/**
 * A transport that plays a recording instead of reaching a core: replies come from the recorded replies (call ids rewritten), and the recorded
 * change-sets are released by a manual playhead. Under the unchanged `UndraCore`, mirror and generated stores. Synchronous, like an in-process core:
 * an `observe` has delivered the state up to the playhead before it returns.
 */
internal class ReplayTransport(private val recording: Recording, private val options: ReplayOptions) : Transport {
    override val mode: Mode get() = Mode.INPROC
    override val isSynchronous: Boolean get() = true

    private val lock = Any()
    private lateinit var events: TransportEvents
    private val calls = HashMap<String, ArrayDeque<RecordedCall>>()
    private val last = HashMap<String, RecordedCall>()
    private val sets = ArrayList<ChangeEvent>()
    private val observed = HashMap<Long, MutableSet<UInt>>()
    private var cursor = 0
    private var closed = false

    /** The playhead: milliseconds of the recording released so far. */
    @Volatile
    var playhead: Long = 0
        private set

    init {
        val pending = HashMap<UInt, Pair<String, ByteArray>>()
        val byCall = HashMap<UInt, RecordedCall>()
        for (e in recording.events) {
            when (val k = e.kind) {
                is RecordedKind.Call -> pending[k.call] = key(k.target) to k.args
                is RecordedKind.Reply -> {
                    val call = pending.remove(k.call) ?: continue
                    val recorded = RecordedCall(call.second, k.call, ReplyStatus.fromByte(k.status.code.toUByte()), k.body, ArrayList())
                    byCall[k.call] = recorded
                    calls.getOrPut(call.first) { ArrayDeque() }.addLast(recorded)
                }
                is RecordedKind.StreamItem -> byCall[k.call]?.items?.add(StreamFlag.fromByte(k.flag.code.toUByte()) to k.body)
                is RecordedKind.ChangeSet -> sets += ChangeEvent(e.t, k.txn, k.entries)
                else -> {}
            }
        }
        playhead = options.startAtMs ?: sets.firstOrNull()?.t ?: 0L
        // What the recording had said by then is history: an observe is answered with it.
        while (cursor < sets.size && sets[cursor].t <= playhead) cursor++
    }

    /** The time of the last recorded change-set. */
    val durationMs: Long get() = sets.lastOrNull()?.t ?: 0L

    /** Change-sets the playhead has not reached yet. */
    val pendingChangeSets: Int get() = synchronized(lock) { sets.size - cursor }

    override fun connect(events: TransportEvents, expectedSchemaHash: ULong): ULong {
        this.events = events
        return recording.schemaHash
    }

    private fun entry(e: RecordedEntry) = Payloads.ChangeEntry(Handle(e.handle), e.signal, Payloads.ChangeOp.fromByte(e.op.code.toUByte()), e.value)

    private fun wanted(handle: Long, signal: UInt): Boolean = observed[handle]?.let { ALL_SIGNALS in it || signal in it } ?: false

    private fun releaseUntil(t: Long) {
        val out = ArrayList<ByteArray>()
        synchronized(lock) {
            while (cursor < sets.size && sets[cursor].t <= t) {
                val set = sets[cursor++]
                val entries = set.entries.filter { wanted(it.handle, it.signal) }.map(::entry)
                if (entries.isNotEmpty()) out += Payloads.ChangeSet(set.txn, entries).toByteArray()
            }
        }
        for (payload in out) events.onChangeSet(payload)
    }

    /** Moves the playhead forward by [ms] and releases the change-sets it passes. */
    fun advance(ms: Long) {
        val to = synchronized(lock) { playhead += ms.coerceAtLeast(0); playhead }
        releaseUntil(to)
    }

    /** Releases everything that is left. */
    fun playAll() {
        val to = synchronized(lock) { playhead = maxOf(playhead, durationMs); playhead }
        releaseUntil(to)
    }

    override fun observe(handle: Long, signalId: UInt, on: Boolean) {
        val initial: ByteArray? = synchronized(lock) {
            if (!on) {
                if (signalId == ALL_SIGNALS) observed.remove(handle) else observed[handle]?.remove(signalId)
                return
            }
            observed.getOrPut(handle) { HashSet() } += signalId
            // The core answers an observe with what the signals hold: here, everything recorded up to the playhead, in order.
            val entries = ArrayList<Payloads.ChangeEntry>()
            for (i in 0 until cursor) {
                for (e in sets[i].entries) if (e.handle == handle && (signalId == ALL_SIGNALS || e.signal == signalId)) entries += entry(e)
            }
            if (entries.isEmpty()) null else Payloads.ChangeSet(0uL, entries).toByteArray()
        }
        initial?.let { events.onChangeSet(it) }
    }

    /** Finds the recorded reply for [call], or the failure to give instead. */
    private fun find(call: Payloads.Call): Pair<RecordedCall?, String?> = synchronized(lock) {
        val k = key(call.target)
        val queue = calls[k]
        var found: RecordedCall? = null
        if (queue != null && queue.isNotEmpty()) {
            val at = if (options.args == ArgsPolicy.EXACT && call.target !is Payloads.CallTarget.LazyListPage) queue.indexOfFirst { it.args.contentEquals(call.args) } else 0
            if (at >= 0) found = queue.removeAt(at).also { last[k] = it }
        }
        if (found == null && options.exhausted == Exhausted.REPEAT_LAST) found = last[k]
        if (found != null) return found to null
        null to "RecordedCore: the recording has no reply for ${describe(call.target)}${if (options.args == ArgsPolicy.EXACT) " with these arguments" else ""}"
    }

    private fun refusal(callId: UInt, reason: String): Payloads.Reply {
        val w = dev.undra.runtime.wire.UndraWriter()
        w.writeStr(reason)
        return Payloads.Reply(callId, ReplyStatus.BAD_REQUEST, w.toByteArray())
    }

    override fun call(payload: ByteArray): Int {
        val call = Payloads.Call.decode(payload)
        val (found, why) = find(call)
        if (found == null) {
            val r = refusal(call.callId, why ?: "no reply")
            events.onReply(r.callId, r.status, r.body)
            return 0
        }
        events.onReply(call.callId, found.status, found.body)
        for ((flag, body) in found.items) events.onStreamItem(call.callId, flag, body)
        return 0
    }

    override fun callSync(payload: ByteArray): ByteArray {
        val call = Payloads.Call.decode(payload)
        val (found, why) = find(call)
        val reply = if (found == null) refusal(call.callId, why ?: "no reply") else Payloads.Reply(call.callId, found.status, found.body)
        return reply.toByteArray()
    }

    override fun cancel(callId: UInt) {}

    override fun streamCredit(callId: UInt, credit: UInt) {}

    override fun release(handle: Long) {}

    override fun portReply(payload: ByteArray) {}

    override fun event(portId: UInt, methodId: UInt, payload: ByteArray) {}

    override fun timerFired(timerId: UInt) {}

    override fun snapshot(): ByteArray = throw UndraModeException("a recorded core has no state of its own to snapshot")

    override fun restore(snapshot: ByteArray): Int = throw UndraModeException("a recorded core cannot restore a snapshot")

    override fun statsJson(): String? = null

    override fun close() {
        closed = true
    }
}

/**
 * A core that replays a recording: the generated stores run on the recorded replies and change-sets with no core behind them, for previews of
 * states that are expensive to reach, and in Android Studio's preview pane, where the app's native core cannot be loaded. Interaction the
 * recording does not hold fails with a typed refusal naming the call; for an app that has to react, use [PreviewCore], the real core with fakes.
 *
 * ```kotlin
 * val recorded = RecordedCore.load(Recording.fromJson(text), expectedSchemaHash = UndraIds.SCHEMA_HASH)
 * val todos = Todos(recorded.core)       // the recorded constructor reply
 * recorded.advance(300)                  // the change-sets up to 300 ms into the session
 * ```
 */
public class RecordedCore private constructor(
    /** The core: the generated bindings take it as their `core`, or it becomes `UndraCore.shared`. */
    public val core: UndraCore,
    private val transport: ReplayTransport,
) : AutoCloseable {
    /** The playhead, in milliseconds. */
    public val playhead: Long get() = transport.playhead

    /** The time of the last recorded change-set. */
    public val durationMs: Long get() = transport.durationMs

    /**
     * Moves the playhead forward by [ms], releases the recorded change-sets it passes for the signals that are observed, and returns once the
     * mirror has applied them to the stores (on the main thread; call it from any other thread, or from the main thread).
     */
    public fun advance(ms: Long) {
        transport.advance(ms)
        applied()
    }

    /** Plays the recording to its end. */
    public fun playAll() {
        transport.playAll()
        applied()
    }

    private fun applied() {
        if (UndraDispatchers.isMainThread()) core.mirror.flush() else runBlocking(UndraDispatchers.main) { core.mirror.flush() }
    }

    /** Closes the core. */
    override fun close() {
        core.close()
    }

    /** Loading. */
    public companion object {
        /**
         * Loads [recording].
         *
         * @param expectedSchemaHash the schema hash of the bindings (`UndraIds.SCHEMA_HASH`).
         * @param makeShared whether the core becomes `UndraCore.shared` (default: yes, if none is).
         * @throws dev.undra.runtime.UndraSchemaMismatchException when the recording belongs to another schema.
         */
        public fun load(
            recording: Recording,
            expectedSchemaHash: ULong,
            options: ReplayOptions = ReplayOptions(),
            makeShared: Boolean = true,
            onError: ((UndraUnhandledError) -> Unit)? = null,
        ): RecordedCore {
            val transport = ReplayTransport(recording, options)
            // No default adapter is installed: nothing touches the platform.
            val load = LoadOptions(expectedSchemaHash = expectedSchemaHash, defaultAdapters = false, onError = onError)
            return RecordedCore(UndraCore.attachTransport(transport, load, makeShared), transport)
        }

        /** [load] for the JSON text of a recording. */
        public fun load(json: String, expectedSchemaHash: ULong, options: ReplayOptions = ReplayOptions(), makeShared: Boolean = true): RecordedCore =
            load(Recording.fromJson(json), expectedSchemaHash, options, makeShared)
    }
}
