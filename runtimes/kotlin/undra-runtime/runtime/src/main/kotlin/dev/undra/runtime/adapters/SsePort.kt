package dev.undra.runtime.adapters

import dev.undra.runtime.PortImpl
import dev.undra.runtime.UndraLog
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineName
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.launch

/**
 * Opens server-sent event streams for the `Sse` port (ADR-047). Implement it to replace the defaults ([JdkHttpSseAdapter]
 * on the JVM, [UrlConnectionSseAdapter] on Android) and serve it with [SsePortAdapter].
 */
public interface SseAdapter {
    /**
     * Requests [url] (`http://` or `https://`, checked by the binding first) with `Accept: text/event-stream`,
     * `Cache-Control: no-cache`, [headers] and, when given, `Last-Event-ID: <lastEventId>`; returns once a 2xx answer (not
     * 204) whose content type is `text/event-stream` arrived.
     *
     * @throws SseError `Refused` with the status for another answer (or without one for an unusable URL or header),
     *   `Protocol("expected text/event-stream, got <type>")` for another content type, `Network` when the server cannot be
     *   reached. Anything else thrown is reported as `Network`.
     */
    public suspend fun open(url: String, headers: List<Header>, lastEventId: String?): SseStream
}

/** One open event stream of an [SseAdapter]. */
public interface SseStream {
    /**
     * The events, parsed with [SseParser], collected once, by the binding. It must be lazy: the binding stops collecting while
     * its buffer is full, and the stream should then stop reading the body so that TCP pushes back. It ends by throwing an
     * [SseError]: `Ended` when the body ended, `Network` when reading it failed, `Protocol` for a body that is not UTF-8; or
     * by completing after [close].
     */
    public val events: Flow<SseEvent>

    /** Stops reading and releases the connection (the server sees the client leave). Never throws. */
    public suspend fun close()
}

/**
 * The binding of the `Sse` port (ADR-047): the pull discipline of [WebSocketPortAdapter] for event streams.
 *
 * * `open` asks the adapter (a URL that is not `http://` or `https://` is refused first) and registers the stream under the
 *   next id (from 1, never reused); a pump reads ahead at most the window (the `max` of the latest `next`, 16 before the first).
 * * `next(stream, max)` answers up to `max` buffered events, else waits for one; `[]` after the core's close; the stream's
 *   end (`Ended` when the body ended) after the events before it, every time. One `next` per stream at a time.
 * * `close` marks the stream closed (again is fine), answers a waiting `next` with `[]` and closes the adapter's stream.
 * * When the core closes ([PortImpl.detach]) or [close] is called, every open stream is closed.
 *
 * An unknown id is `Network("no event stream <id>")`.
 *
 * @param adapter the platform's event streams.
 */
public class SsePortAdapter(private val adapter: SseAdapter) : AutoCloseable {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default + CoroutineName("undra-sse"))
    private val nextId = AtomicInteger(1)
    private val streams = ConcurrentHashMap<UInt, Feed>()

    /** Streams the core closed. */
    private val closed: MutableSet<UInt> = ConcurrentHashMap.newKeySet()

    /** How many times [close] ran: an open that was in flight across one is closed as that [close] would have closed it. */
    private val closings = AtomicInteger(0)

    private class Feed(val id: UInt, val stream: SseStream, val inbound: PulledStream<SseEvent>)

    /**
     * Opens a stream through the adapter and returns its id.
     *
     * @throws SseError as the port's `open` does.
     */
    public suspend fun open(url: String, headers: List<Header>, lastEventId: String?): UInt {
        if (!(url.startsWith("http://", ignoreCase = true) || url.startsWith("https://", ignoreCase = true))) {
            throw SseError.Refused(null, "invalid URL: $url")
        }
        val closingsBefore = closings.get()
        val stream = try {
            adapter.open(url, headers, lastEventId)
        } catch (e: SseError) {
            throw e
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            throw SseError.Network(describe(e))
        }
        val id = nextId.getAndIncrement().toUInt()
        val inbound = PulledStream(
            source = stream.events,
            scope = scope,
            finishedOnItsOwn = { SseError.Ended },
            typed = { e -> e as? SseError ?: SseError.Network(describe(e)) },
            busy = { SseError.Protocol("a next is already pending on stream $id") },
            readAhead = stream as? ReadAheadSource,
        )
        val feed = Feed(id, stream, inbound)
        streams[id] = feed
        inbound.start()
        // A close() (the core went away) that ran while the adapter opened could not see this stream: it goes too.
        if (closings.get() != closingsBefore && markClosed(feed)) scope.launch { closeQuietly(feed) }
        return id
    }

    /**
     * The next events of [stream], at least one and at most [max], once there are any; `[]` after the core's close.
     *
     * @throws SseError the stream's end, an unknown id, or a second `next` while one waits.
     */
    public suspend fun next(stream: UInt, max: UInt): List<SseEvent> {
        val feed = streams[stream] ?: if (stream in closed) return emptyList() else throw unknown(stream)
        return feed.inbound.pull(max)
    }

    /**
     * Closes [stream]; closing it again is fine.
     *
     * @throws SseError `Network` for an unknown id.
     */
    public suspend fun close(stream: UInt) {
        val feed = streams[stream] ?: if (stream in closed) return else throw unknown(stream)
        if (!markClosed(feed)) return
        closeQuietly(feed)
    }

    private fun markClosed(feed: Feed): Boolean {
        if (!feed.inbound.markClosed()) return false
        closed.add(feed.id)
        streams.remove(feed.id)
        return true
    }

    private suspend fun closeQuietly(feed: Feed) {
        try {
            feed.stream.close()
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            UndraLog.debug("closing event stream ${feed.id} failed: $e")
        }
    }

    /** How many streams are open (not closed by the core). */
    public val openStreams: Int get() = streams.size

    /**
     * Closes every open stream, without waiting, and so every stream whose `open` is in flight once it opens. The binding
     * stays usable. The core does this when it closes ([PortImpl.detach]).
     */
    override fun close() {
        closings.incrementAndGet()
        for (feed in streams.values) {
            if (markClosed(feed)) scope.launch { closeQuietly(feed) }
        }
    }

    /** This binding as the async [PortImpl] of [StandardPorts.Sse]; it closes every stream when detached. */
    public fun portImpl(): PortImpl = PortImpl(
        sync = false,
        methods = portMethods {
            this[StandardPorts.Sse.OPEN] = { args ->
                val (url, headers, lastEventId) = decodeArgs(args) { Triple(it.readStr(), headerList.decode(it), optionString.decode(it)) }
                typed(SseError) { Codecs.u32.encodeToByteArray(open(url, headers, lastEventId)) }
            }
            this[StandardPorts.Sse.NEXT] = { args ->
                val (stream, max) = decodeArgs(args) { it.readU32() to it.readU32() }
                typed(SseError) { eventList.encodeToByteArray(next(stream, max)) }
            }
            this[StandardPorts.Sse.CLOSE] = { args ->
                val stream = decodeArgs(args) { it.readU32() }
                typed(SseError) {
                    close(stream)
                    EMPTY_REPLY
                }
            }
        },
        detach = ::close,
    )

    private companion object {
        val headerList: UndraCodec<List<Header>> = Codecs.vec(Header)
        val optionString: UndraCodec<String?> = Codecs.option(Codecs.string)
        val eventList: UndraCodec<List<SseEvent>> = Codecs.vec(SseEvent)

        fun unknown(stream: UInt): SseError = SseError.Network("no event stream $stream")
    }
}

/** [adapter] served as the `Sse` port: `core.registerPort(StandardPorts.Sse.PORT_ID, ssePort(adapter))`. */
public fun ssePort(adapter: SseAdapter): PortImpl = SsePortAdapter(adapter).portImpl()
