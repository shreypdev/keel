package dev.undra.contract

import dev.undra.playground.core.sseFollow
import dev.undra.runtime.adapters.SseError
import dev.undra.runtime.adapters.SseEvent
import kotlinx.coroutines.runBlocking

/**
 * S24: the opt-in `Sse` port through the runtime's default adapter on a JVM (`JdkHttpSseAdapter`, `java.net.http`, which
 * `UndraCore.load` installs with the JVM defaults) against the shared local server; the core's side is `sse_follow`.
 */
fun s24Sse(w: World) {
    val http = RealtimeServer.http
    val feed = listOf(
        SseEvent("1", "message", "one", 1500u),
        SseEvent("2", "tick", "two\nlines", null),
        SseEvent("2", "message", "three", null),
        SseEvent("4", "message", "four", null),
    )

    // 1. The feed, parsed as the HTML standard says; no Last-Event-ID.
    val all = runBlocking { sseFollow("$http/sse/feed", null, 10u) }
    expectEq("ended after the feed", true, all.ended)
    expectEq("the events of the feed", feed, all.events)
    expectEq("the Last-Event-ID of the first request", null, RealtimeServer.last("/sse/feed").headers["last-event-id"])

    // 2. Resume after id 2.
    val resumed = runBlocking { sseFollow("$http/sse/feed", "2", 10u) }
    expectEq("ended after the resumed feed", true, resumed.ended)
    expectEq("the events after id 2", feed.drop(2), resumed.events)
    expectEq("the Last-Event-ID of the resumed request", "2", RealtimeServer.last("/sse/feed").headers["last-event-id"])

    // 3. A reader that stops.
    val two = runBlocking { sseFollow("$http/sse/feed", null, 2u) }
    expectEq("two events, not ended", feed.take(2) to false, two.events to two.ended)
    val hang = runBlocking { sseFollow("$http/sse/hang", null, 0u) }
    expectEq("no event from /sse/hang, not ended", emptyList<SseEvent>() to false, hang.events to hang.ended)
    // Bounded by WAIT_MS, a hang detector: the server never ends /sse/hang, and the port leaves it when the core closes the
    // subscription, on no timer, so a port that did not would stay; how soon the server sees it is the machine's.
    awaitUntil("the server to see the client leave /sse/hang", timeoutMs = WAIT_MS) { RealtimeServer.last("/sse/hang").clientClosed }

    // 4. Typed failures.
    for (code in listOf(204, 500)) {
        val refused = expectFailsAsync<SseError.Refused>("/sse/status?code=$code") { sseFollow("$http/sse/status?code=$code", null, 10u) }
        expectEq("the status of the refusal", code.toUShort(), refused.status)
    }
    expectFailsAsync<SseError.Protocol>("/sse/html") { sseFollow("$http/sse/html", null, 10u) }
}
