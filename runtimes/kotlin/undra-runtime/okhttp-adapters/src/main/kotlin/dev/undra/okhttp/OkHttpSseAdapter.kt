package dev.undra.okhttp

import dev.undra.runtime.adapters.Header
import dev.undra.runtime.adapters.SseAdapter
import dev.undra.runtime.adapters.SseError
import dev.undra.runtime.adapters.SseStream
import dev.undra.runtime.adapters.SseStreamReader
import java.io.IOException
import java.util.Locale
import java.util.concurrent.TimeUnit
import kotlin.coroutines.resume
import kotlinx.coroutines.suspendCancellableCoroutine
import okhttp3.Call
import okhttp3.Callback
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.OkHttpClient
import okhttp3.Protocol
import okhttp3.Request
import okhttp3.Response
import okhttp3.ResponseBody

/**
 * The `Sse` port over the app's own [OkHttpClient] (ADR-060, ADR-047): every stream is opened through [client], so its
 * interceptors, `Authenticator` (a `401` with a refreshed token reopens the stream), event listeners, certificate pinner, `Dns`,
 * proxy and connection pool apply to it with nothing Undra-specific configured. Serve it with `SsePortAdapter`, which owns the ids,
 * the pull and the read-ahead.
 *
 * It behaves as `UrlConnectionSseAdapter` (the default on Android) does, and the shared suite that checks it
 * (`RealtimeAdapterContract`) runs on this one:
 *
 *  - The request is a `GET` with `Accept: text/event-stream`, `Cache-Control: no-cache`, the given headers and, to resume,
 *    `Last-Event-ID`; redirects are the client's.
 *  - `open` returns after a 2xx answer other than 204 whose content type is `text/event-stream`: another status is `Refused` with
 *    it, another type `Protocol`, an unreachable server, a timeout or a TLS failure `Network`, an unusable URL or header `Refused`
 *    without a status. Cancelling `open` cancels the call.
 *  - The body is read as the HTML standard says (UTF-8, `SseParser`): its end is `Ended`, a failure while reading `Network`, bytes
 *    that are not UTF-8 `Protocol`.
 *  - **Backpressure**: the reader reads the next chunk only while the binding's buffer has room, so a core that stops pulling
 *    stalls the server's writes (the reading is `SseStreamReader`, the one the default adapters use).
 *  - Closing a stream cancels the call, so the connection is released and the server sees the client leave, even while the
 *    reader waits for data from a server that sends nothing.
 *
 * An event stream is open for as long as the app wants it, which the client's timeouts cannot know, so the stream is opened on a
 * client derived from the app's (`newBuilder`, sharing its connection pool, dispatcher, interceptors, authenticator, pinner and
 * everything else) that has **no read timeout and no call timeout** (a quiet stream is not a dead one) and speaks **HTTP/1.1**, as
 * `okhttp-sse` and the default adapters do: the unread data of a stalled HTTP/2 stream counts against the connection's
 * flow-control window, which the app's other requests share, and a reader that waits for the core must not hold them up. The
 * client's connect timeout still bounds the connect.
 *
 * @param client the app's client, asked for on every open.
 */
public class OkHttpSseAdapter(private val client: () -> OkHttpClient) : SseAdapter {
    /** An adapter over [client]; see the class documentation. */
    public constructor(client: OkHttpClient) : this({ client })

    override suspend fun open(url: String, headers: List<Header>, lastEventId: String?): SseStream {
        val request = build(url, headers, lastEventId)
        val call = try {
            streaming(client()).newCall(request)
        } catch (e: IllegalArgumentException) {
            throw SseError.Refused(null, OkHttpRules.describe(e))
        } catch (e: Exception) {
            throw SseError.Network(OkHttpRules.describe(e)) // the app's own client provider failed
        }
        val answered = suspendCancellableCoroutine<Answer> { continuation ->
            continuation.invokeOnCancellation { call.cancel() }
            call.enqueue(object : Callback {
                override fun onFailure(call: Call, e: IOException) {
                    // A resume after the caller's cancellation is ignored.
                    continuation.resume(Answer.Failed(SseError.Network(OkHttpRules.describe(e))))
                }

                override fun onResponse(call: Call, response: Response) {
                    if (!continuation.isActive) {
                        response.close() // the caller gave up; the call is cancelled already
                        return
                    }
                    continuation.resume(Answer.Opened(response))
                }
            })
        }
        val response = when (answered) {
            is Answer.Failed -> throw answered.error
            is Answer.Opened -> answered.response
        }
        // Nullable by declaration: OkHttp 4's `body` is, 5's is not (a response of a call always has one).
        val body: ResponseBody? = response.body
        try {
            accept(response.code, response.header("Content-Type"))
        } catch (e: SseError) {
            response.close()
            throw e
        }
        if (body == null) {
            response.close()
            throw SseError.Network("the server answered without a body")
        }
        return SseStreamReader(body.byteStream(), lastEventId) { call.cancel() }
    }

    /** How the request ended: the answer, or the typed error to throw (thrown by [open], not resumed through a coroutine). */
    private sealed interface Answer {
        class Opened(val response: Response) : Answer

        class Failed(val error: SseError) : Answer
    }

    private fun build(url: String, headers: List<Header>, lastEventId: String?): Request {
        val target = url.toHttpUrlOrNull() ?: throw SseError.Refused(null, "invalid URL: $url (not an http or https URL with a host)")
        val builder = Request.Builder().url(target).get()
        try {
            builder.header("Accept", "text/event-stream")
            builder.header("Cache-Control", "no-cache")
            for (header in headers) builder.addHeader(header.name, header.value)
            if (lastEventId != null) {
                if (lastEventId.any { it == '\r' || it == '\n' || it == '\u0000' }) throw SseError.Refused(null, "the last event id contains a line break or a NUL")
                builder.header("Last-Event-ID", lastEventId)
            }
        } catch (e: IllegalArgumentException) {
            throw SseError.Refused(null, OkHttpRules.describe(e))
        }
        return builder.build()
    }

    /** [base] as a long-lived stream wants it: no read or call timeout, HTTP/1.1 only. Everything else is the app's. */
    internal fun streaming(base: OkHttpClient): OkHttpClient =
        base.newBuilder()
            .readTimeout(0, TimeUnit.MILLISECONDS)
            .callTimeout(0, TimeUnit.MILLISECONDS)
            .protocols(listOf(Protocol.HTTP_1_1))
            .build()

    /**
     * Checks an answer: a 2xx status other than 204 (a 204 means "stop", HTML standard) and the content type `text/event-stream`.
     */
    private fun accept(status: Int, contentType: String?) {
        if (status !in 200..299 || status == 204) {
            throw SseError.Refused(if (status in 0..65535) status.toUShort() else null, "the server answered HTTP $status")
        }
        val type = contentType?.substringBefore(';')?.trim()?.lowercase(Locale.ROOT)
        if (type != "text/event-stream") throw SseError.Protocol("expected text/event-stream, got ${contentType ?: "no content type"}")
    }
}
