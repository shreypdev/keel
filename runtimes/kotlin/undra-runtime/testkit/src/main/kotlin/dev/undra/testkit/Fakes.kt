package dev.undra.testkit

import dev.undra.runtime.PortImpl
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraPortException
import dev.undra.runtime.adapters.AppState
import dev.undra.runtime.adapters.FsError
import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.HttpMethod
import dev.undra.runtime.adapters.HttpRequest
import dev.undra.runtime.adapters.HttpResponse
import dev.undra.runtime.adapters.NetKind
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.CopyOnWriteArrayList

// The deterministic fakes of the standard ports (docs/TESTING.md): the same behaviour as `undra::ports::fakes`, held to it by
// testkit/conformance/fakes.json, which the test suite replays against these classes.

private val NO_BYTES = ByteArray(0)
private const val NS_PER_MS = 1_000_000L

private typealias Method = suspend (ByteArray) -> ByteArray

private inline fun <T> readArgs(args: ByteArray, read: (UndraReader) -> T): T {
    val r = UndraReader(args)
    val v = read(r)
    r.finish()
    return v
}

/** A timer armed on the fake clock. */
private class Armed(val deadlineNs: Long, val seq: Long, val id: UInt)

/**
 * Thrown by [FakeClock.advance] and [PreviewCore.advance] when one call fires more timers than its cap and another is still due: a timer
 * that re-arms itself at the same instant (or every millisecond across a long window) never lets time move on. The clock stays at the last
 * deadline that fired and the timers still armed stay armed.
 *
 * @property fired how many timers fired before the cap stopped the call.
 * @property timerId the id of the timer that was due next.
 * @property atMs the monotonic reading of the clock when it stopped, in whole milliseconds.
 */
public class TimerStormException(public val fired: Int, public val timerId: UInt, public val atMs: Long) : IllegalStateException(
    "advance fired $fired timers and timer $timerId is due again at $atMs ms: a timer that re-arms itself without time passing never ends " +
        "(raise maxTimers if the window really holds that many)",
)

/**
 * A deterministic `Clock` and `Timer`: time only moves when the test says so.
 *
 * [nowMs] is a wall clock you can [setNowMs]; [monotonicNs] starts at 0 and [advance] moves both by the same amount. [set] arms a timer on the
 * monotonic counter; [advance] fires every timer that comes due in deadline order (ties in arming order) with the clock reading exactly the
 * deadline while each one fires. Firing calls [onTimerFired] (a [PreviewCore] points it at the core's `timerFired`). Nothing here reads
 * the system clock.
 */
public class FakeClock(nowMs: Long = DEFAULT_NOW_MS) {
    private val lock = Any()
    private var wallNs: Long = nowMs * NS_PER_MS
    private var monoNs: Long = 0
    private var seq: Long = 0
    private val timers = ArrayList<Armed>()

    /** Called with the id of every timer that fires, on the thread that called [advance] or [fireNext], with no lock held. */
    @Volatile
    public var onTimerFired: ((UInt) -> Unit)? = null

    /** The wall clock, milliseconds since the Unix epoch. */
    public val nowMs: Long get() = synchronized(lock) { Math.floorDiv(wallNs, NS_PER_MS) }

    /** The monotonic counter in nanoseconds (starts at 0). */
    public val monotonicNs: ULong get() = synchronized(lock) { monoNs.toULong() }

    /** Sets the wall clock. The monotonic counter and the armed timers are not affected: a wall-clock jump is not the passage of time. */
    public fun setNowMs(nowMs: Long) {
        synchronized(lock) { wallNs = nowMs * NS_PER_MS }
    }

    /** `Timer.set`: arms timer [timerId] to fire after [delayMs] of fake time. */
    public fun set(timerId: UInt, delayMs: ULong) {
        synchronized(lock) {
            seq += 1
            val delay = if (delayMs > (Long.MAX_VALUE / NS_PER_MS).toULong()) Long.MAX_VALUE / 2 else delayMs.toLong() * NS_PER_MS
            timers += Armed(monoNs + delay, seq, timerId)
            timers.sortWith(compareBy<Armed> { it.deadlineNs }.thenBy { it.seq })
        }
    }

    /** How many timers are armed and have not fired. */
    public val pendingTimers: Int get() = synchronized(lock) { timers.size }

    /** The ids of the armed timers, in the order they will fire. */
    public fun pendingTimerIds(): List<UInt> = synchronized(lock) { timers.map { it.id } }

    /** Milliseconds until the next armed timer is due, or `null` when none is armed. */
    public fun nextDueInMs(): Long? = synchronized(lock) {
        val next = timers.firstOrNull() ?: return null
        val left = next.deadlineNs - monoNs
        (left + NS_PER_MS - 1) / NS_PER_MS
    }

    /**
     * Fires the next armed timer if it is due within [limitMs] from now, moving the clock to its deadline first. Returns its id, or `null` when
     * none is due in the window (the clock does not move). [advance] is a loop over this; a harness that has to wait for the core between
     * timers ([PreviewCore.advance]) drives it itself.
     */
    public fun fireNext(limitMs: Long): UInt? {
        val hook = onTimerFired
        val id = synchronized(lock) {
            val next = timers.firstOrNull() ?: return null
            if (next.deadlineNs > monoNs + limitMs.coerceAtLeast(0) * NS_PER_MS) return null
            timers.removeAt(0)
            val moved = (next.deadlineNs - monoNs).coerceAtLeast(0)
            monoNs += moved
            wallNs += moved
            next.id
        }
        hook?.invoke(id)
        return id
    }

    /** Moves the clock by [ms] without firing anything: what is left of a window after its last timer. */
    public fun moveBy(ms: Long) {
        synchronized(lock) {
            val by = ms.coerceAtLeast(0) * NS_PER_MS
            monoNs += by
            wallNs += by
        }
    }

    /**
     * Moves time forward by [ms] and fires the timers that come due, in order; returns their ids. The hook runs synchronously, and may arm timers
     * that fall inside the window (they fire in the same call).
     *
     * @param maxTimers the most timers one call may fire.
     * @throws TimerStormException when [maxTimers] fired and another is still due.
     */
    public fun advance(ms: Long, maxTimers: Int = MAX_TIMERS_PER_ADVANCE): List<UInt> {
        val fired = ArrayList<UInt>()
        var left = ms.coerceAtLeast(0) * NS_PER_MS
        while (true) {
            val step = synchronized(lock) {
                val next = timers.firstOrNull() ?: return@synchronized null
                if (next.deadlineNs > monoNs + left) return@synchronized null
                if (fired.size >= maxTimers) throw TimerStormException(fired.size, next.id, monoNs / NS_PER_MS)
                (next.deadlineNs - monoNs).coerceAtLeast(0)
            } ?: break
            left -= step
            val id = fireNext((step + NS_PER_MS - 1) / NS_PER_MS) ?: break
            fired += id
        }
        moveBy(0)
        synchronized(lock) {
            monoNs += left
            wallNs += left
        }
        return fired
    }

    /** This clock as the `Clock` port (sync). */
    public fun clockPortImpl(): PortImpl = PortImpl(
        sync = true,
        methods = mapOf<UInt, Method>(
            StandardPorts.Clock.NOW_MS to { Codecs.i64.encodeToByteArray(nowMs) },
            StandardPorts.Clock.MONOTONIC_NS to { Codecs.u64.encodeToByteArray(monotonicNs) },
        ),
    )

    /** This clock as the `Timer` port (sync, fire and forget). */
    public fun timerPortImpl(): PortImpl = PortImpl(
        sync = true,
        methods = mapOf<UInt, Method>(
            StandardPorts.Timer.SET to { args ->
                val r = UndraReader(args)
                val id = r.readU32()
                val delay = r.readU64()
                r.finish()
                set(id, delay)
                NO_BYTES
            },
        ),
    )

    /** Defaults. */
    public companion object {
        /** The wall-clock reading of a new clock: 2023-11-14T22:13:20Z. */
        public const val DEFAULT_NOW_MS: Long = 1_700_000_000_000L

        /** The most timers one [advance] fires by default. */
        public const val MAX_TIMERS_PER_ADVANCE: Int = 100_000
    }
}

private const val ZERO_SEED_REPLACEMENT: ULong = 0x9E3779B97F4A7C15uL
private const val MULTIPLIER: ULong = 0x2545F4914F6CDD1DuL

/**
 * A deterministic `Rng`: xorshift64*, the same seed always yields the same bytes, and the same bytes as `SeededRng` in Rust, Swift and
 * TypeScript. Not cryptographically secure, on purpose. A seed of 0 is replaced by a fixed constant; `fill(n)` consumes `ceil(n / 8)`
 * outputs, little-endian, and drops the unused tail; a fill never returns more than 16 MiB.
 */
public class SeededRng(seed: ULong = DEFAULT_SEED) {
    private val lock = Any()
    private var state: ULong = initial(seed)

    /** Restarts the sequence from [seed]. */
    public fun reseed(seed: ULong) {
        synchronized(lock) { state = initial(seed) }
    }

    /** The next 64-bit output. */
    public fun nextU64(): ULong = synchronized(lock) { next() }

    private fun next(): ULong {
        var x = state
        x = x xor (x shr 12)
        x = x xor (x shl 25)
        x = x xor (x shr 27)
        state = x
        return x * MULTIPLIER
    }

    /** [len] bytes from the sequence (what `Rng.fill(len)` answers). */
    public fun fill(len: UInt): ByteArray = synchronized(lock) {
        val n = minOf(len, MAX_FILL).toInt()
        val out = ByteArray(n)
        var at = 0
        while (at < n) {
            var word = next()
            var i = 0
            while (i < 8 && at < n) {
                out[at++] = (word and 0xffuL).toByte()
                word = word shr 8
                i++
            }
        }
        out
    }

    /** This generator as the `Rng` port (sync). */
    public fun portImpl(): PortImpl = PortImpl(
        sync = true,
        methods = mapOf<UInt, Method>(
            StandardPorts.Rng.FILL to { args -> Codecs.bytes.encodeToByteArray(fill(readArgs(args) { it.readU32() })) },
        ),
    )

    /** Limits and defaults. */
    public companion object {
        /** The most bytes one fill returns: 16 MiB. */
        public const val MAX_FILL: UInt = 16_777_216u

        /** The seed of a generator made without one. */
        public const val DEFAULT_SEED: ULong = 0x4B45454C5F524E47uL

        private fun initial(seed: ULong): ULong = if (seed == 0uL) ZERO_SEED_REPLACEMENT else seed
    }
}

/** Decides whether a scripted reply applies to a request. */
public fun interface HttpMatcher {
    /** Whether [request] matches. */
    public fun matches(request: HttpRequest): Boolean

    /** Constructors of matchers. */
    public companion object {
        /** Every request. */
        public fun any(): HttpMatcher = HttpMatcher { true }

        /** Requests to exactly [url]. */
        public fun url(url: String): HttpMatcher = HttpMatcher { it.url == url }

        /** Requests whose URL starts with [prefix]. */
        public fun urlPrefix(prefix: String): HttpMatcher = HttpMatcher { it.url.startsWith(prefix) }

        /** Requests with [method]. */
        public fun method(method: HttpMethod): HttpMatcher = HttpMatcher { it.method == method }
    }
}

/** Requests both matchers match. */
public infix fun HttpMatcher.and(other: HttpMatcher): HttpMatcher = HttpMatcher { matches(it) && other.matches(it) }

/** What a scripted rule answers: a response, or an error to fail with. */
public sealed interface HttpReply {
    /** Answer with [response] (any status is a response). */
    public class Response(public val response: HttpResponse) : HttpReply

    /** Fail with [error]. */
    public class Failure(public val error: HttpError) : HttpReply
}

/** Builds a response: [body] as UTF-8, [headers] as given. */
public fun httpResponse(status: Int, body: String = "", headers: List<Pair<String, String>> = emptyList()): HttpResponse =
    HttpResponse(status.toUShort(), headers.map { Header(it.first, it.second) }, body.toByteArray(Charsets.UTF_8))

private sealed interface Script {
    class Fixed(val reply: HttpReply) : Script
    class Sequence(val replies: ArrayDeque<HttpReply>) : Script
    class Handler(val handler: suspend (HttpRequest) -> HttpReply) : Script
}

/**
 * An `Http` fake that answers from a script and remembers every request. Rules are tried in the order they were added and the first that
 * matches wins. A request nothing matches fails with [HttpError.Network] naming the request, and is still recorded.
 */
public class FakeHttp {
    private val lock = Any()
    private val rules = ArrayList<Pair<HttpMatcher, Script>>()
    private val recorded = CopyOnWriteArrayList<HttpRequest>()

    /** Answers every request [matcher] matches with [reply]. */
    public fun respond(matcher: HttpMatcher, reply: HttpReply): FakeHttp = add(matcher, Script.Fixed(reply))

    /** Answers every request [matcher] matches with [response]. */
    public fun respond(matcher: HttpMatcher, response: HttpResponse): FakeHttp = respond(matcher, HttpReply.Response(response))

    /** Answers every request to exactly [url] with [response]. */
    public fun respond(url: String, response: HttpResponse): FakeHttp = respond(HttpMatcher.url(url), response)

    /** Fails every request [matcher] matches with [error]. */
    public fun fail(matcher: HttpMatcher, error: HttpError): FakeHttp = respond(matcher, HttpReply.Failure(error))

    /** Answers the requests [matcher] matches with [replies], one each, in order; once used up the rule no longer matches. */
    public fun respondSequence(matcher: HttpMatcher, replies: List<HttpReply>): FakeHttp = add(matcher, Script.Sequence(ArrayDeque(replies)))

    /** Answers every request [matcher] matches by calling [handler]. */
    public fun respondWith(matcher: HttpMatcher, handler: suspend (HttpRequest) -> HttpReply): FakeHttp = add(matcher, Script.Handler(handler))

    private fun add(matcher: HttpMatcher, script: Script): FakeHttp {
        synchronized(lock) { rules += matcher to script }
        return this
    }

    /** Every request received so far, oldest first (unmatched ones included). */
    public val calls: List<HttpRequest> get() = recorded.toList()

    /** Forgets every rule and every recorded request. */
    public fun reset() {
        synchronized(lock) { rules.clear() }
        recorded.clear()
    }

    /** Performs [request] against the script: throws [HttpError] for a scripted or unmatched failure. */
    public suspend fun request(request: HttpRequest): HttpResponse {
        recorded += request
        var handler: (suspend (HttpRequest) -> HttpReply)? = null
        var found: HttpReply? = null
        synchronized(lock) {
            for ((matcher, script) in rules) {
                if (!matcher.matches(request)) continue
                when (script) {
                    is Script.Fixed -> found = script.reply
                    is Script.Sequence -> found = script.replies.removeFirstOrNull() ?: continue
                    is Script.Handler -> handler = script.handler
                }
                break
            }
        }
        val reply = found ?: handler?.invoke(request) ?: throw HttpError.Network("FakeHttp: no scripted response for ${request.method.name} ${request.url}")
        return when (reply) {
            is HttpReply.Response -> reply.response
            is HttpReply.Failure -> throw reply.error
        }
    }

    /** This fake as the `Http` port (async). */
    public fun portImpl(): PortImpl = PortImpl(
        sync = false,
        methods = mapOf<UInt, Method>(
            StandardPorts.Http.REQUEST to { args ->
                val decoded = readArgs(args) { HttpRequest.decode(it) }
                try {
                    HttpResponse.encodeToByteArray(request(decoded))
                } catch (e: HttpError) {
                    throw UndraPortException(HttpError.encodeToByteArray(e))
                }
            },
        ),
    )
}

/** One operation a [MemStore] served through its port, for asserting on persistence behaviour. */
public class StoreOp(public val op: String, public val key: String) {
    override fun toString(): String = "$op($key)"
}

/** An in-memory key-value store: keys ordered by UTF-8 bytes, operations recorded. [MemKv] and [MemSecureStore] are its two ports. */
public open class MemStore internal constructor(private val trait: String) {
    private val lock = Any()
    private val map = HashMap<String, ByteArray>()
    private val recorded = CopyOnWriteArrayList<StoreOp>()

    /** Puts [value] under [key] without recording an operation: seeds a test. */
    public fun insert(key: String, value: ByteArray) {
        synchronized(lock) { map[key] = value.copyOf() }
    }

    /** The value under [key], read directly (not an operation). */
    public fun value(key: String): ByteArray? = synchronized(lock) { map[key]?.copyOf() }

    /** The keys, ascending by UTF-8 bytes. */
    public fun keys(): List<String> = synchronized(lock) { map.keys.sortedWith(utf8Order) }

    /** How many entries. */
    public val size: Int get() = synchronized(lock) { map.size }

    /** The operations served through the port, oldest first. */
    public val ops: List<StoreOp> get() = recorded.toList()

    /** `get`. */
    public suspend fun get(key: String): ByteArray? {
        recorded += StoreOp("get", key)
        return value(key)
    }

    /** `set`. */
    public suspend fun set(key: String, value: ByteArray) {
        recorded += StoreOp("set", key)
        insert(key, value)
    }

    /** `delete` (a missing key is not an error). */
    public suspend fun delete(key: String) {
        recorded += StoreOp("delete", key)
        synchronized(lock) { map.remove(key) }
    }

    /** `list`: the keys that start with [prefix], ascending. */
    public suspend fun list(prefix: String): List<String> {
        recorded += StoreOp("list", prefix)
        return keys().filter { it.startsWith(prefix) }
    }

    /** This store as its port (async). */
    public fun portImpl(): PortImpl {
        val ids = if (trait == "Kv") {
            arrayOf(StandardPorts.Kv.GET, StandardPorts.Kv.SET, StandardPorts.Kv.DELETE, StandardPorts.Kv.LIST)
        } else {
            arrayOf(StandardPorts.SecureStore.GET, StandardPorts.SecureStore.SET, StandardPorts.SecureStore.DELETE, StandardPorts.SecureStore.LIST)
        }
        return PortImpl(
            sync = false,
            methods = mapOf<UInt, Method>(
                ids[0] to { args -> Codecs.option(Codecs.bytes).encodeToByteArray(get(readArgs(args) { it.readStr() })) },
                ids[1] to { args ->
                    val r = UndraReader(args)
                    val key = r.readStr()
                    val value = r.readBytes()
                    r.finish()
                    set(key, value)
                    NO_BYTES
                },
                ids[2] to { args ->
                    delete(readArgs(args) { it.readStr() })
                    NO_BYTES
                },
                ids[3] to { args -> Codecs.vec(Codecs.string).encodeToByteArray(list(readArgs(args) { it.readStr() })) },
            ),
        )
    }
}

/** The `Kv` fake. */
public class MemKv : MemStore("Kv")

/** The `SecureStore` fake: the same store under its own port id. */
public class MemSecureStore : MemStore("SecureStore")

private fun segments(path: String): List<String> {
    val parts = ArrayList<String>()
    for (part in path.split('/')) {
        if (part.isEmpty() || part == ".") continue
        if (part == "..") throw FsError.Denied
        parts += part
    }
    return parts
}

private fun emptyPath(): FsError = FsError.Io("the path is empty")

/**
 * An in-memory `Fs` with the semantics the platform adapters share: paths are `/`-separated, empty and `.` segments are ignored and `..` is
 * `Denied`; `write` creates missing directories and replaces a file; `read` of a missing path is `NotFound`, of a directory `Io`; `delete`
 * removes a file or a directory with everything under it; `list` answers the names directly inside a directory, ascending.
 */
public class MemFs {
    private val lock = Any()
    private val files = HashMap<String, ByteArray>()
    private val dirs = HashSet<String>()

    /** Creates the file at [path] without going through the port: seeds a test. Throws [FsError] like a write would. */
    public fun seed(path: String, contents: ByteArray) {
        writeFile(path, contents)
    }

    /** The contents of the file at [path], read directly. */
    public fun contents(path: String): ByteArray? = synchronized(lock) {
        try {
            files[segments(path).joinToString("/")]?.copyOf()
        } catch (e: FsError) {
            null
        }
    }

    /** The path of every file, ascending. */
    public fun filePaths(): List<String> = synchronized(lock) { files.keys.sortedWith(utf8Order) }

    /** `read`. */
    public suspend fun read(path: String): ByteArray = synchronized(lock) {
        val parts = segments(path)
        if (parts.isEmpty()) throw emptyPath()
        val key = parts.joinToString("/")
        files[key]?.copyOf() ?: if (key in dirs) throw FsError.Io("is a directory") else throw FsError.NotFound
    }

    /** `write`. */
    public suspend fun write(path: String, data: ByteArray) {
        writeFile(path, data)
    }

    private fun writeFile(path: String, data: ByteArray) = synchronized(lock) {
        val parts = segments(path)
        if (parts.isEmpty()) throw emptyPath()
        val key = parts.joinToString("/")
        if (key in dirs) throw FsError.Io("is a directory")
        val parents = parts.dropLast(1)
        var prefix = ""
        for (parent in parents) {
            prefix = if (prefix.isEmpty()) parent else "$prefix/$parent"
            if (prefix in files) throw FsError.Io("not a directory")
        }
        prefix = ""
        for (parent in parents) {
            prefix = if (prefix.isEmpty()) parent else "$prefix/$parent"
            dirs += prefix
        }
        files[key] = data.copyOf()
        Unit
    }

    /** `delete`. */
    public suspend fun delete(path: String): Unit = synchronized(lock) {
        val parts = segments(path)
        if (parts.isEmpty()) throw emptyPath()
        val key = parts.joinToString("/")
        if (files.remove(key) != null) return@synchronized
        if (!dirs.remove(key)) throw FsError.NotFound
        val below = "$key/"
        files.keys.removeAll { it.startsWith(below) }
        dirs.removeAll { it.startsWith(below) }
    }

    /** `list`. */
    public suspend fun list(dir: String): List<String> = synchronized(lock) {
        val parts = segments(dir)
        val key = parts.joinToString("/")
        if (parts.isNotEmpty()) {
            if (key in files) throw FsError.Io("not a directory")
            if (key !in dirs) throw FsError.NotFound
        }
        val prefix = if (parts.isEmpty()) "" else "$key/"
        val names = HashSet<String>()
        for (path in files.keys + dirs) {
            if (!path.startsWith(prefix)) continue
            val name = path.substring(prefix.length).substringBefore('/')
            if (name.isNotEmpty()) names += name
        }
        names.sortedWith(utf8Order)
    }

    /** This file system as the `Fs` port (async); failures answer with the [FsError]. */
    public fun portImpl(): PortImpl {
        suspend fun result(block: suspend () -> ByteArray): ByteArray =
            try {
                block()
            } catch (e: FsError) {
                throw UndraPortException(FsError.encodeToByteArray(e))
            }
        return PortImpl(
            sync = false,
            methods = mapOf<UInt, Method>(
                StandardPorts.Fs.READ to { args ->
                    val path = readArgs(args) { it.readStr() }
                    result { Codecs.bytes.encodeToByteArray(read(path)) }
                },
                StandardPorts.Fs.WRITE to { args ->
                    val r = UndraReader(args)
                    val path = r.readStr()
                    val data = r.readBytes()
                    r.finish()
                    result {
                        write(path, data)
                        NO_BYTES
                    }
                },
                StandardPorts.Fs.DELETE to { args ->
                    val path = readArgs(args) { it.readStr() }
                    result {
                        delete(path)
                        NO_BYTES
                    }
                },
                StandardPorts.Fs.LIST to { args ->
                    val dir = readArgs(args) { it.readStr() }
                    result { Codecs.vec(Codecs.string).encodeToByteArray(list(dir)) }
                },
            ),
        )
    }
}

/** One record the core logged. */
public class LogEntry(public val level: UByte, public val target: String, public val message: String)

/** A `Log` that keeps every record. */
public class CaptureLog {
    private val recorded = CopyOnWriteArrayList<LogEntry>()

    /** The records so far. */
    public val entries: List<LogEntry> get() = recorded.toList()

    /** The messages so far. */
    public fun messages(): List<String> = recorded.map { it.message }

    /** Whether any message contains [needle]. */
    public fun contains(needle: String): Boolean = recorded.any { it.message.contains(needle) }

    /** Forgets the records. */
    public fun clear(): Unit = recorded.clear()

    /** This log as the `Log` port (sync). */
    public fun portImpl(): PortImpl = PortImpl(
        sync = true,
        methods = mapOf<UInt, Method>(
            StandardPorts.Log.LOG to { args ->
                val r = UndraReader(args)
                val level = r.readU8()
                val target = r.readStr()
                val message = r.readStr()
                r.finish()
                recorded += LogEntry(level, target, message)
                NO_BYTES
            },
        ),
    )
}

/** A source of connectivity changes a test or a preview drives: [set] reports the new state to the core it is attached to. */
public class ScriptedConnectivity {
    private val lock = Any()
    private var online = true
    private var kind = NetKind.WIFI
    private var core: UndraCore? = null

    /** The state the app is in. */
    public val current: Pair<Boolean, NetKind> get() = synchronized(lock) { online to kind }

    /** Reports the current state to [core] (like a platform monitor does when it starts) and every change from now on. */
    public fun attach(core: UndraCore) {
        synchronized(lock) { this.core = core }
        emit()
    }

    /** Changes the state and tells the core, if attached. */
    public fun set(online: Boolean, kind: NetKind) {
        synchronized(lock) {
            this.online = online
            this.kind = kind
        }
        emit()
    }

    /** Goes offline (kind none). */
    public fun goOffline(): Unit = set(false, NetKind.NONE)

    /** Comes back online on [kind]. */
    public fun goOnline(kind: NetKind = NetKind.WIFI): Unit = set(true, kind)

    private fun emit() {
        val (target, state) = synchronized(lock) { core to (online to kind) }
        val w = UndraWriter(3)
        w.writeBool(state.first)
        NetKind.encode(w, state.second)
        target?.event(StandardPorts.Connectivity.PORT_ID, StandardPorts.Connectivity.CHANGED, w.toByteArray())
    }
}

/** A source of lifecycle changes a test or a preview drives. */
public class ScriptedLifecycle {
    private val lock = Any()
    private var state = AppState.ACTIVE
    private var core: UndraCore? = null

    /** The state the app is in. */
    public val current: AppState get() = synchronized(lock) { state }

    /** Reports the current state to [core] and every change from now on. */
    public fun attach(core: UndraCore) {
        synchronized(lock) { this.core = core }
        emit()
    }

    /** Changes the state and tells the core, if attached. */
    public fun set(state: AppState) {
        synchronized(lock) { this.state = state }
        emit()
    }

    private fun emit() {
        val (target, value) = synchronized(lock) { core to state }
        target?.event(StandardPorts.Lifecycle.PORT_ID, StandardPorts.Lifecycle.CHANGED, AppState.encodeToByteArray(value))
    }
}

/** One of each fake in the documented default state: the default time and seed, empty stores, no scripted replies, online on Wi-Fi, active. */
public class Fakes {
    /** The `Clock` and the `Timer`. */
    public val clock: FakeClock = FakeClock()

    /** The `Rng`. */
    public val rng: SeededRng = SeededRng()

    /** The `Log`. */
    public val log: CaptureLog = CaptureLog()

    /** The `Http`. */
    public val http: FakeHttp = FakeHttp()

    /** The `Kv`. */
    public val kv: MemKv = MemKv()

    /** The `SecureStore`. */
    public val secureStore: MemSecureStore = MemSecureStore()

    /** The `Fs`. */
    public val fs: MemFs = MemFs()

    /** The source of `Connectivity` events. */
    public val connectivity: ScriptedConnectivity = ScriptedConnectivity()

    /** The source of `Lifecycle` events. */
    public val lifecycle: ScriptedLifecycle = ScriptedLifecycle()

    /** Every request/reply fake as its port, by port id: what `LoadOptions.adapters` takes. */
    public fun ports(): Map<UInt, PortImpl> = linkedMapOf(
        StandardPorts.Clock.PORT_ID to clock.clockPortImpl(),
        StandardPorts.Timer.PORT_ID to clock.timerPortImpl(),
        StandardPorts.Rng.PORT_ID to rng.portImpl(),
        StandardPorts.Log.PORT_ID to log.portImpl(),
        StandardPorts.Http.PORT_ID to http.portImpl(),
        StandardPorts.Kv.PORT_ID to kv.portImpl(),
        StandardPorts.SecureStore.PORT_ID to secureStore.portImpl(),
        StandardPorts.Fs.PORT_ID to fs.portImpl(),
    )
}
