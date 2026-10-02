package dev.undra.testkit

import dev.undra.runtime.PortImpl
import dev.undra.runtime.UndraException
import dev.undra.runtime.UndraPortException

private typealias PortMethod = suspend (ByteArray) -> ByteArray

/**
 * Records the port traffic of the [PortImpl]s it wraps: for each call, the port, the method, the encoded arguments, the reply (or the typed
 * error, or "unavailable") and the time. Hand the wrapped ports to `LoadOptions.adapters` and write the result with [toJson].
 *
 * The recording holds `port_call` and `port_reply` events only: `undra dev --record` captures the whole session.
 *
 * @param schemaHash the schema hash of the core the traffic belongs to.
 * @param now milliseconds, any origin: the recording's times are relative to the first reading. Default the system's monotonic clock; pass a
 *   manual clock for byte-for-byte reproducible recordings.
 * @param platform informational: the host platform, for example `"android"`.
 * @param source informational: where the recording came from.
 */
public class PortRecorder(
    private val schemaHash: ULong,
    private val now: () -> Long = { System.nanoTime() / 1_000_000 },
    private val platform: String? = null,
    private val source: String = "adapters",
) {
    private val lock = Any()
    private val events = ArrayList<RecordedEvent>()
    private val start = now()
    private var next = 0u

    private fun push(kind: RecordedKind) = synchronized(lock) {
        val t = maxOf(0L, now() - start)
        events += RecordedEvent(maxOf(events.lastOrNull()?.t ?: 0L, t), kind)
    }

    /** [impl] with every method recording its calls. */
    public fun wrap(port: UInt, impl: PortImpl): PortImpl = PortImpl(
        sync = impl.sync,
        methods = impl.methods.mapValues { (method, body) ->
            val recorded: PortMethod = { args ->
                val call = synchronized(lock) { ++next }
                push(RecordedKind.PortCall(port, method, call, args.copyOf()))
                try {
                    body(args).also { push(RecordedKind.PortReply(call, PortStatusName.OK, it.copyOf())) }
                } catch (e: UndraPortException) {
                    push(RecordedKind.PortReply(call, PortStatusName.ERROR, e.body.copyOf()))
                    throw e
                } catch (e: Exception) {
                    push(RecordedKind.PortReply(call, PortStatusName.UNAVAILABLE, ByteArray(0)))
                    throw e
                }
            }
            recorded
        },
    )

    /** Wraps every port of [ports] (by port id); the result is what to register. */
    public fun wrapAll(ports: Map<UInt, PortImpl>): Map<UInt, PortImpl> = ports.mapValues { (id, impl) -> wrap(id, impl) }

    /** What has been recorded. */
    public fun record(): Recording = synchronized(lock) { Recording(schemaHash, source, platform, events.toList()) }

    /** The recording as canonical JSON. */
    public fun toJson(): String = record().toJson()
}

/** Where a replay left the recording. */
public sealed interface ReplayError {
    /** The core called a different method (or the same method with other arguments) than the recording's next call of this port. */
    public class Mismatch(public val port: UInt, public val called: String, public val expected: String, public val argsDiffer: Boolean, public val nth: Int) : ReplayError

    /** The core called a port more often than the recording did. */
    public class Exhausted(public val port: UInt, public val called: String, public val recorded: Int) : ReplayError

    /** The core made the recorded call, but the recording holds no reply for it (the session ended while it was in flight, or the file was cut): it was answered "unavailable". */
    public class Unanswered(public val port: UInt, public val called: String, public val nth: Int) : ReplayError

    /** The replay ended with recorded calls the core never made. */
    public class Unconsumed(public val port: UInt, public val next: String, public val remaining: Int) : ReplayError

    /** The error in a sentence. */
    public fun describe(): String = when (this) {
        is Mismatch ->
            if (argsDiffer) "replay: call $nth of the port was $called with other arguments than the recording's"
            else "replay: call $nth of the port was $called, the recording has $expected next"
        is Exhausted -> "replay: $called was called after the recording's $recorded call(s) of the port were used up"
        is Unanswered -> "replay: call $nth of the port, $called, has no reply in the recording; it was answered unavailable"
        is Unconsumed -> "replay: $remaining recorded call(s) were never made, the next is $next"
    }
}

/** Thrown by [Replayer.finish] when the replay deviated; [errors] has every deviation. */
public class ReplayException(public val errors: List<ReplayError>) : UndraException(errors.joinToString("\n") { it.describe() })

/** How strictly a replayed call must match the recorded one. */
public enum class ArgsPolicy {
    /** The encoded arguments must be byte for byte the recorded ones (the default). */
    EXACT,

    /** Only the port and method must match: for calls whose arguments carry something that changes between runs. */
    IGNORE,
}

private class Expected(val method: UInt, val args: ByteArray, var reply: Pair<PortStatusName, ByteArray>?)

private val SYNC_PORTS = setOf(portId("Clock"), portId("Rng"), portId("Log"), portId("Timer"))

private fun label(port: UInt, method: UInt): String = standardName(port, method) ?: "port $port method $method"

/**
 * Answers a core's port calls from a recording, in order: the recording's `port_call` events, per port, are the script. A call that is the
 * next recorded one of its port (same method and, by default, the same arguments) gets the recorded reply; anything else is a typed
 * [ReplayError], kept for [finish], and the core is answered "unavailable". A deviation consumes nothing, so one wrong call does not shift
 * every later answer. Time is not replayed: answers are immediate.
 */
public class Replayer(recording: Recording, private val policy: ArgsPolicy = ArgsPolicy.EXACT) {
    private val lock = Any()
    private val queues = LinkedHashMap<UInt, ArrayDeque<Expected>>()
    private val recorded = HashMap<UInt, Int>()
    private val answered = HashMap<UInt, Int>()
    private val deviations = ArrayList<ReplayError>()

    init {
        val open = HashMap<UInt, Expected>()
        for (e in recording.events) {
            when (val k = e.kind) {
                is RecordedKind.PortCall -> {
                    val expected = Expected(k.method, k.args, null)
                    queues.getOrPut(k.port) { ArrayDeque() }.addLast(expected)
                    open[k.call] = expected
                    recorded[k.port] = (recorded[k.port] ?: 0) + 1
                }
                is RecordedKind.PortReply -> open.remove(k.call)?.reply = k.status to k.body
                else -> {}
            }
        }
    }

    private fun answer(port: UInt, method: UInt, args: ByteArray): ByteArray = synchronized(lock) {
        val nth = answered[port] ?: 0
        val next = queues[port]?.firstOrNull()
        if (next == null) {
            val error = ReplayError.Exhausted(port, label(port, method), recorded[port] ?: 0)
            deviations += error
            throw UndraException(error.describe())
        }
        val sameMethod = next.method == method
        if (!(sameMethod && (policy == ArgsPolicy.IGNORE || next.args.contentEquals(args)))) {
            val error = ReplayError.Mismatch(port, label(port, method), label(port, next.method), sameMethod, nth)
            deviations += error
            throw UndraException(error.describe())
        }
        queues[port]?.removeFirst()
        answered[port] = nth + 1
        val reply = next.reply
        if (reply == null) deviations += ReplayError.Unanswered(port, label(port, method), nth)
        when {
            reply == null || reply.first == PortStatusName.UNAVAILABLE -> throw UndraException("the recorded port reply is unavailable")
            reply.first == PortStatusName.ERROR -> throw UndraPortException(reply.second)
            else -> reply.second
        }
    }

    /**
     * One [PortImpl] per recorded port, ready to register (`LoadOptions.adapters`). Every method id of the port answers, so a call the
     * recording never made is reported as a deviation.
     */
    public fun ports(): Map<UInt, PortImpl> = queues.keys.associateWith { port ->
        val methods = object : AbstractMap<UInt, PortMethod>() {
            override val entries: Set<Map.Entry<UInt, PortMethod>> get() = emptySet()
            override fun containsKey(key: UInt): Boolean = true
            override fun get(key: UInt): PortMethod = { args -> answer(port, key, args) }
        }
        PortImpl(sync = port in SYNC_PORTS, methods = methods)
    }

    /** The deviations seen so far. */
    public fun errors(): List<ReplayError> = synchronized(lock) { deviations.toList() }

    /** How many recorded calls are still waiting to be made. */
    public val remaining: Int get() = synchronized(lock) { queues.values.sumOf { it.size } }

    /** The deviations so far followed by one [ReplayError.Unconsumed] per port with calls left; empty when the replay was clean. */
    public fun problems(): List<ReplayError> = synchronized(lock) {
        val out = ArrayList<ReplayError>(deviations)
        for ((port, queue) in queues.entries.sortedBy { it.key }) {
            val next = queue.firstOrNull() ?: continue
            out += ReplayError.Unconsumed(port, label(port, next.method), queue.size)
        }
        out
    }

    /**
     * Ends the replay.
     *
     * @throws ReplayException when a call deviated or recorded calls were never made.
     */
    public fun finish() {
        val problems = problems()
        if (problems.isNotEmpty()) throw ReplayException(problems)
    }
}
