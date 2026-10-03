package dev.undra.runtime.adapters

import dev.undra.runtime.UndraEmbeddingApi
import java.io.IOException
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URI
import java.net.URISyntaxException
import java.net.http.HttpClient
import java.nio.ByteBuffer
import java.nio.CharBuffer
import java.nio.charset.CodingErrorAction
import java.nio.charset.StandardCharsets
import java.time.Duration
import java.util.Locale
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CompletionException
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import java.net.http.HttpRequest as JdkRequest
import java.net.http.HttpResponse as JdkResponse

/**
 * The default [SseAdapter] on a JVM: `java.net.http.HttpClient` (HTTP/1.1), the body read as a stream on a reader thread of
 * the stream's own and parsed with [SseParser].
 *
 * * The request carries `Accept: text/event-stream`, `Cache-Control: no-cache`, the headers and `Last-Event-ID`; redirects
 *   are followed (never from `https` to `http`). A header `java.net.http` refuses (`Host`, `Connection`, ...) is `Refused`.
 * * `open` returns after a 2xx answer other than 204 whose content type is `text/event-stream`: another status is
 *   `Refused(status)`, another type `Protocol`, an unreachable server `Network`. Cancelling `open` aborts the request.
 * * **Backpressure**: the reader reads the next chunk only while the binding's buffer has room, and the client asks the
 *   connection for more only as it is read, so a core that stops pulling stalls the server's writes.
 * * The body's end is `Ended`, a failure while reading `Network`, bytes that are not UTF-8 `Protocol`. [SseStream.close]
 *   closes the body (the connection is released and the server sees the client leave) even while the reader waits for data.
 *
 * (`HttpURLConnection` is not used on the JVM: the JDK's `disconnect()` waits for a blocked read of a chunked body to return,
 * so a stream whose server sends nothing could never be closed. On Android it can, and [UrlConnectionSseAdapter] uses it.)
 *
 * @param client the client to use; `null` for one of its own (HTTP/1.1, redirects followed, a 30 s connect timeout).
 */
public class JdkHttpSseAdapter(client: HttpClient? = null) : SseAdapter {
    private val client: HttpClient by lazy {
        client ?: HttpClient.newBuilder()
            .version(HttpClient.Version.HTTP_1_1)
            .followRedirects(HttpClient.Redirect.NORMAL)
            .connectTimeout(Duration.ofSeconds(30))
            .build()
    }

    override suspend fun open(url: String, headers: List<Header>, lastEventId: String?): SseStream {
        val uri = checkUrl(url)
        val builder = JdkRequest.newBuilder(uri).GET()
        try {
            for ((name, value) in requestHeaders(headers, lastEventId)) builder.header(name, value)
        } catch (e: IllegalArgumentException) {
            throw SseError.Refused(null, describe(e))
        }
        val exchange = client.sendAsync(builder.build(), JdkResponse.BodyHandlers.ofInputStream())
        val response = try {
            exchange.awaitCancellable()
        } catch (e: IOException) {
            throw SseError.Network(describe(e))
        } catch (e: IllegalArgumentException) {
            throw SseError.Refused(null, describe(e))
        } catch (e: SecurityException) {
            throw SseError.Network(describe(e))
        }
        val body = response.body()
        try {
            accept(response.statusCode(), response.headers().firstValue("content-type").orElse(null))
        } catch (e: SseError) {
            closeQuietly(body)
            throw e
        }
        return SseStreamReader(body, lastEventId) { closeQuietly(body) }
    }
}

/**
 * The default [SseAdapter] on Android: `HttpURLConnection` (the platform's HTTP stack, which honours the app's network
 * security config), the body read on a reader thread of the stream's own and parsed with [SseParser]. It behaves as
 * [JdkHttpSseAdapter] does, with Android's own redirect rules.
 *
 * Use it on Android, where `disconnect()` aborts a read in progress. On a desktop JVM the JDK's `disconnect()` waits for a
 * blocked read of a chunked body, so closing a stream whose server sends nothing would not return until it does: use
 * [JdkHttpSseAdapter] there.
 *
 * @param connectTimeoutMillis how long connecting may take; reading has no timeout (a quiet stream is not a dead one).
 */
public class UrlConnectionSseAdapter(private val connectTimeoutMillis: Int = 30_000) : SseAdapter {
    override suspend fun open(url: String, headers: List<Header>, lastEventId: String?): SseStream {
        val uri = checkUrl(url)
        val connection = try {
            uri.toURL().openConnection() as HttpURLConnection
        } catch (e: IllegalArgumentException) {
            throw SseError.Refused(null, "invalid URL: $url (${describe(e)})")
        } catch (e: IOException) {
            throw SseError.Refused(null, "invalid URL: $url (${describe(e)})")
        } catch (e: ClassCastException) {
            throw SseError.Refused(null, "invalid URL: $url (not an HTTP URL)")
        }
        try {
            connection.requestMethod = "GET"
            connection.connectTimeout = connectTimeoutMillis
            connection.readTimeout = 0
            connection.useCaches = false
            connection.instanceFollowRedirects = true
            for ((name, value) in requestHeaders(headers, lastEventId)) connection.addRequestProperty(name, value)
        } catch (e: IllegalArgumentException) {
            throw SseError.Refused(null, describe(e))
        } catch (e: IllegalStateException) {
            throw SseError.Refused(null, describe(e))
        }
        val body = blockingIo(abort = connection::disconnect) {
            val status = try {
                connection.responseCode
            } catch (e: IOException) {
                connection.disconnect()
                throw SseError.Network(describe(e))
            }
            try {
                accept(status, connection.contentType)
                connection.inputStream
            } catch (e: SseError) {
                connection.disconnect()
                throw e
            } catch (e: IOException) {
                connection.disconnect()
                throw SseError.Network(describe(e))
            }
        }
        return SseStreamReader(body, lastEventId) { connection.disconnect() }
    }
}

/** The request headers of an event stream, in order. */
private fun requestHeaders(headers: List<Header>, lastEventId: String?): List<Pair<String, String>> {
    val all = ArrayList<Pair<String, String>>(headers.size + 3)
    all.add("Accept" to "text/event-stream")
    all.add("Cache-Control" to "no-cache")
    for (header in headers) {
        if (header.value.any { it == '\r' || it == '\n' || it == '\u0000' }) {
            throw SseError.Refused(null, "the value of header '${header.name}' contains a line break or a NUL")
        }
        all.add(header.name to header.value)
    }
    if (lastEventId != null) {
        if (lastEventId.any { it == '\r' || it == '\n' || it == '\u0000' }) throw SseError.Refused(null, "the last event id contains a line break or a NUL")
        all.add("Last-Event-ID" to lastEventId)
    }
    return all
}

/** [url] as a URI, if it is an `http` or `https` URL with a host. */
private fun checkUrl(url: String): URI {
    val uri = try {
        URI(url)
    } catch (e: URISyntaxException) {
        throw SseError.Refused(null, "invalid URL: $url (${e.reason})")
    }
    val scheme = uri.scheme?.lowercase(Locale.ROOT)
    if (scheme != "http" && scheme != "https") throw SseError.Refused(null, "invalid URL: $url (only http:// and https:// URLs are supported)")
    if (uri.host.isNullOrEmpty()) throw SseError.Refused(null, "invalid URL: $url (the URL has no host)")
    return uri
}

/**
 * Checks an answer: a 2xx status other than 204 (a 204 means "stop", HTML standard) and the content type
 * `text/event-stream`.
 */
private fun accept(status: Int, contentType: String?) {
    if (status !in 200..299 || status == 204) {
        throw SseError.Refused(if (status in 0..65535) status.toUShort() else null, "the server answered HTTP $status")
    }
    val type = contentType?.substringBefore(';')?.trim()?.lowercase(Locale.ROOT)
    if (type != "text/event-stream") throw SseError.Protocol("expected text/event-stream, got ${contentType ?: "no content type"}")
}

private fun closeQuietly(stream: InputStream) {
    try {
        stream.close()
    } catch (e: IOException) {
        // nothing left to release
    }
}

/** Closes [stream] after its connection was aborted, when whatever the platform throws for that no longer matters. */
private fun releaseQuietly(stream: InputStream) {
    try {
        stream.close()
    } catch (e: Exception) {
        // already released, or the socket under it is gone: either way nothing is held
    }
}

/** Daemon threads for the blocking parts of opening a stream with `HttpURLConnection`. */
private val openThreads: ExecutorService = Executors.newCachedThreadPool { task ->
    Thread(task, "undra-sse-open").also { it.isDaemon = true }
}

/** Runs [work] on a thread of its own; cancelling the caller calls [abort] (which makes [work] fail) and returns at once. */
private suspend fun <T> blockingIo(abort: () -> Unit, work: () -> T): T =
    suspendCancellableCoroutine { continuation ->
        continuation.invokeOnCancellation { abort() }
        openThreads.execute {
            val outcome = runCatching(work)
            // A cancelled continuation ignores the outcome; [abort] already released the connection.
            continuation.resumeWith(outcome)
        }
    }

/** Suspends until this future completes; cancelling the coroutine cancels the future (and the request). */
private suspend fun <T> CompletableFuture<T>.awaitCancellable(): T =
    suspendCancellableCoroutine { continuation ->
        whenComplete { value, failure ->
            if (failure == null) {
                continuation.resumeWith(Result.success(value))
            } else {
                continuation.resumeWith(Result.failure(if (failure is CompletionException && failure.cause != null) failure.cause!! else failure))
            }
        }
        continuation.invokeOnCancellation { cancel(true) }
    }

/**
 * An [SseStream] over a response body: a reader thread decodes the bytes as UTF-8, parses them with [SseParser] and hands
 * the events to [events]; it reads the next chunk only while fewer events wait than the binding's buffer has room for.
 *
 * It is what the default adapters return and what an adapter over any other HTTP client returns too (the OkHttp one does, ADR-060):
 * check the answer, then `return SseStreamReader(body, lastEventId) { cancelTheCall() }`. It is part of the embedding API: it may
 * change between releases.
 *
 * @param body the response body.
 * @param lastEventId the `Last-Event-ID` the request sent: the parser's last event id to begin with.
 * @param abort releases the connection; it must make a read that is blocked fail (or end). It is called from the reader thread when
 *   the body ended or failed, and from [close].
 */
@UndraEmbeddingApi
public class SseStreamReader(
    private val body: InputStream,
    private val lastEventId: String?,
    private val abort: () -> Unit,
) : SseStream, ReadAheadSource {
    private sealed interface Item {
        class Event(val event: SseEvent) : Item

        class End(val error: SseError) : Item
    }

    private val inbox = Channel<Item>(Channel.UNLIMITED)
    private val gate = ReentrantLock()
    private val roomChanged = gate.newCondition()

    /** Events the reader put into [inbox] that [events] has not taken yet. Guarded by [gate]. */
    private var waiting = 0

    /** How many events may wait before the reader stops: the room of the binding's buffer (one without one). Guarded by [gate]. */
    private var room = 1

    @Volatile
    private var stopping = false
    private val collected = AtomicBoolean(false)
    private val started = AtomicBoolean(false)

    override val events: Flow<SseEvent> = flow {
        if (!collected.compareAndSet(false, true)) throw SseError.Protocol("the events of this stream are already being read")
        if (started.compareAndSet(false, true)) {
            Thread(::readLoop, "undra-sse-reader").also { it.isDaemon = true }.start()
        }
        for (item in inbox) {
            gate.withLock {
                waiting--
                roomChanged.signalAll()
            }
            when (item) {
                is Item.Event -> emit(item.event)
                is Item.End -> throw item.error
            }
        }
    }

    private fun readLoop() {
        try {
            readAll()
        } finally {
            // The reader is the one thread that reads the body, so it is the one that lets it go, however the stream ended: a body
            // left open keeps its connection (an OkHttp call holds it in its pool) even after [abort] closed the socket.
            releaseQuietly(body)
        }
    }

    private fun readAll() {
        val parser = SseParser(lastEventId)
        val decoder = StandardCharsets.UTF_8.newDecoder()
            .onMalformedInput(CodingErrorAction.REPORT)
            .onUnmappableCharacter(CodingErrorAction.REPORT)
        val bytes = ByteBuffer.allocate(CHUNK + 8)
        val chars = CharBuffer.allocate(CHUNK + 8)
        val end: SseError = try {
            while (true) {
                // A chunk may hold several events, so the read-ahead is the room plus at most one chunk.
                gate.withLock {
                    while (waiting >= room && !stopping) roomChanged.awaitUninterruptibly()
                }
                if (stopping) return
                val n = body.read(bytes.array(), bytes.position(), bytes.remaining())
                if (n < 0) {
                    // The end: what is left must decode completely (an incomplete sequence is malformed here).
                    bytes.flip()
                    chars.clear()
                    if (decoder.decode(bytes, chars, true).isError || decoder.flush(chars).isError) throw NotUtf8()
                    chars.flip()
                    deliver(parser.push(chars.toString()))
                    parser.end()
                    break
                }
                bytes.position(bytes.position() + n)
                bytes.flip()
                chars.clear()
                val result = decoder.decode(bytes, chars, false)
                if (result.isError) {
                    throw NotUtf8()
                }
                bytes.compact()
                chars.flip()
                deliver(parser.push(chars.toString()))
            }
            SseError.Ended
        } catch (e: NotUtf8) {
            SseError.Protocol("the event stream is not UTF-8")
        } catch (e: IOException) {
            if (stopping) return
            SseError.Network(describe(e))
        } catch (e: RuntimeException) {
            if (stopping) return
            SseError.Network(describe(e))
        }
        inbox.trySend(Item.End(end))
        inbox.close()
        abort()
    }

    private class NotUtf8 : Exception()

    private fun deliver(events: List<SseEvent>) {
        if (events.isEmpty()) return
        gate.withLock { waiting += events.size }
        for (event in events) inbox.trySend(Item.Event(event))
    }

    override suspend fun close() {
        stopping = true
        gate.withLock { roomChanged.signalAll() }
        inbox.close()
        // A stream nobody started reading has no reader to let its body go: this does, and no reader starts after it.
        val unread = started.compareAndSet(false, true)
        withContext(Dispatchers.IO) {
            abort()
            if (unread) releaseQuietly(body)
        }
    }

    override fun setRoom(room: Int) {
        gate.withLock {
            this.room = room
            roomChanged.signalAll()
        }
    }

    private companion object {
        /** The most bytes one read takes. */
        const val CHUNK = 8 * 1024
    }
}
