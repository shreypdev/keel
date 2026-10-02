package dev.undra.runtime.adapters

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/**
 * The inbound side of one connection of the `WebSocket` or `Sse` binding (ADR-047 §3): a **pump** collects [source] into a
 * buffer only while the buffer holds fewer items than the window (the `max` of the latest [pull], [INITIAL_WINDOW] before
 * the first), and [pull] hands the buffer to the core.
 *
 * A source that is lazy (the default adapters' flows: their reader thread reads the next frame or chunk only once the one
 * before was taken) therefore stops reading the network while the core is not pulling, and TCP pushes back on the server.
 *
 * Ends, in the order the port reports them: the core's close ([markClosed]: every later pull answers `[]`); otherwise the
 * buffered items, then the end of [source], sticky: the typed error it threw ([typed] passes the port's own error through and
 * turns anything else into one), or [finishedOnItsOwn] when it completed without one.
 */
internal class PulledStream<T : Any>(
    private val source: Flow<T>,
    private val scope: CoroutineScope,
    private val finishedOnItsOwn: () -> Throwable,
    private val typed: (Throwable) -> Throwable,
    private val busy: () -> Throwable,
    private val readAhead: ReadAheadSource? = null,
) {
    private val lock = Any()
    private val buffer = ArrayDeque<T>()
    private var window = INITIAL_WINDOW
    private var ended: Throwable? = null
    private var closed = false
    private var pulling = false

    /** Wakes the waiting pull: an item, the end or the close arrived. */
    private val changed = Channel<Unit>(Channel.CONFLATED)

    /** Wakes the pump: the buffer has room again, or the stream closed. */
    private val room = Channel<Unit>(Channel.CONFLATED)

    @Volatile
    private var pump: Job? = null

    /** How the source ended, once the pump saw it (`null` while it runs, and after the core's close). */
    val end: Throwable? get() = synchronized(lock) { if (closed) null else ended }

    /** How many items wait for a pull. */
    val buffered: Int get() = synchronized(lock) { buffer.size }

    /** Starts the pump. */
    fun start() {
        tellRoom()
        pump = scope.launch { pumpAll() }
    }

    /** Tells a [ReadAheadSource] how many more items the buffer can take. */
    private fun tellRoom() {
        val source = readAhead ?: return
        source.setRoom(synchronized(lock) { if (closed) Int.MAX_VALUE else (window - buffer.size).coerceAtLeast(0) })
    }

    private suspend fun pumpAll() {
        val outcome: Throwable = try {
            source.collect { item ->
                synchronized(lock) {
                    if (closed) throw CancellationException("closed by the core")
                    buffer.addLast(item)
                }
                tellRoom()
                changed.trySend(Unit)
                // Take the next item only once the buffer has room: the source waits meanwhile.
                while (synchronized(lock) { !closed && buffer.size >= window }) room.receive()
            }
            finishedOnItsOwn()
        } catch (e: CancellationException) {
            if (!scope.isActive || synchronized(lock) { closed }) return
            typed(e)
        } catch (e: Exception) {
            typed(e)
        }
        synchronized(lock) {
            if (!closed && ended == null) ended = outcome
        }
        changed.trySend(Unit)
    }

    /**
     * The next items, at least one and at most [max], once there are any; `[]` after [markClosed].
     *
     * A burst is answered as one reply (one crossing for many items, ADR-047 §3): a pull that finds fewer than [max] items
     * while more are arriving waits until there are [max], until [QUIET_MILLIS] pass with nothing new, or until
     * [MAX_WAIT_MILLIS] after it saw the first one, whichever comes first; the source's end answers at once.
     *
     * @throws Throwable the source's end (typed), or [busy] while another pull waits.
     */
    suspend fun pull(max: UInt): List<T> {
        synchronized(lock) {
            if (pulling) throw busy()
            pulling = true
        }
        try {
            var firstSeen = 0L
            var lastGrowth = 0L
            var seen = 0
            while (true) {
                var count = 0
                val now = System.nanoTime()
                val batch: List<T>? = synchronized(lock) {
                    if (closed) return emptyList()
                    window = minOf(max, Int.MAX_VALUE.toUInt()).toInt().coerceAtLeast(1)
                    count = buffer.size
                    val settled = count >= window || ended != null ||
                        (firstSeen != 0L && (now - lastGrowth >= QUIET_NANOS || now - firstSeen >= MAX_WAIT_NANOS))
                    when {
                        count > 0 && settled -> List(minOf(count, window)) { buffer.removeFirst() }
                        ended != null -> throw ended!!
                        else -> null
                    }
                }
                // The window may have grown and the buffer shrunk: the pump may go on.
                room.trySend(Unit)
                tellRoom()
                if (batch != null) return batch
                if (count == 0) {
                    changed.receive()
                    continue
                }
                if (firstSeen == 0L) firstSeen = now
                if (count > seen) {
                    seen = count
                    lastGrowth = now
                }
                val waitNanos = minOf(QUIET_NANOS - (now - lastGrowth), MAX_WAIT_NANOS - (now - firstSeen))
                withTimeoutOrNull(((waitNanos + 999_999) / 1_000_000).coerceAtLeast(1)) { changed.receive() }
            }
        } finally {
            synchronized(lock) { pulling = false }
        }
    }

    /** Ends the stream for the core: drops the buffer, answers a waiting pull with `[]`, stops the pump. `false` if it already was. */
    fun markClosed(): Boolean {
        synchronized(lock) {
            if (closed) return false
            closed = true
            buffer.clear()
        }
        changed.trySend(Unit)
        room.trySend(Unit)
        tellRoom()
        pump?.cancel()
        return true
    }

    companion object {
        /** The read-ahead before the first pull (SPEC 3.7's initial grant). */
        const val INITIAL_WINDOW: Int = 16

        /** A pull with some items answers once nothing new arrived for this long. */
        const val QUIET_MILLIS: Long = 2

        /** A pull with some items answers at the latest this long after it saw the first. */
        const val MAX_WAIT_MILLIS: Long = 8

        private const val QUIET_NANOS: Long = QUIET_MILLIS * 1_000_000
        private const val MAX_WAIT_NANOS: Long = MAX_WAIT_MILLIS * 1_000_000
    }
}

/**
 * A connection of a default adapter whose reader thread can read ahead of its lazy flow: the binding tells it how many
 * more items its buffer can take ([setRoom]), and the reader reads while fewer than that wait in the connection, so the
 * items in the connection and in the binding's buffer together stay within the window (ADR-047 §3) without a thread
 * hand-off per item. Without a binding (the flow collected directly) the room is one item: plainly lazy.
 */
internal interface ReadAheadSource {
    /** The binding's buffer can take [room] more items (`Int.MAX_VALUE` once the binding closed it: stop waiting). */
    fun setRoom(room: Int)
}
