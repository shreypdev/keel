package dev.undra.runtime.adapters

/**
 * The `text/event-stream` parser of the HTML standard (section 9.2.6, "Interpreting an event stream"), the one every
 * [SseAdapter] of this runtime uses. Feed it the decoded text in any pieces with [push]; it returns the events those pieces
 * completed.
 *
 * * Lines end with CRLF, LF or CR (a CR at the end of one piece and an LF at the start of the next are one line end).
 * * A line starting with `:` is a comment. `field: value` drops one space after the colon; a line without a colon is a field
 *   with an empty value. Unknown fields are ignored.
 * * `event` sets the event type; `data` appends its value and a line feed to the data; `id` sets the last event id (unless
 *   the value contains NUL), which persists across events; `retry` with only ASCII digits sets this event's [SseEvent.retryMs].
 * * A blank line dispatches: when the data is empty, the data and type are reset and nothing is dispatched; else one trailing
 *   line feed is removed and `SseEvent(id = last event id or null when it is empty, event = type or "message", data, retryMs)`
 *   is emitted, and the data, type and retry are reset.
 * * A UTF-8 byte order mark at the very start is skipped. [end] discards an event the stream ended in the middle of.
 *
 * @param lastEventId the last event id the stream was resumed from (the `Last-Event-ID` the request sent): like the
 *   standard's `EventSource`, which keeps it across reconnections, events without an `id` field carry it.
 */
public class SseParser(lastEventId: String? = null) {
    private val line = StringBuilder()
    private val data = StringBuilder()
    private var eventType = ""
    private var lastEventId = lastEventId.orEmpty()
    private var retryMs: UInt? = null
    private var started = false
    private var afterCr = false

    /** Parses [text], the next piece of the stream, and returns the events it completed, in order. */
    public fun push(text: String): List<SseEvent> {
        val out = ArrayList<SseEvent>()
        var i = 0
        if (!started && text.isNotEmpty()) {
            started = true
            if (text[0] == '﻿') i = 1
        }
        while (i < text.length) {
            val c = text[i++]
            if (afterCr) {
                afterCr = false
                if (c == '\n') continue
            }
            when (c) {
                '\r' -> {
                    afterCr = true
                    endLine(out)
                }
                '\n' -> endLine(out)
                else -> line.append(c)
            }
        }
        return out
    }

    /** The stream ended: an event that was not completed by a blank line is discarded. The last event id is kept. */
    public fun end() {
        line.setLength(0)
        data.setLength(0)
        eventType = ""
        retryMs = null
        afterCr = false
    }

    private fun endLine(out: MutableList<SseEvent>) {
        if (line.isEmpty()) {
            dispatch(out)
            return
        }
        if (line[0] == ':') {
            line.setLength(0)
            return
        }
        val colon = line.indexOf(":")
        val field: String
        val value: String
        if (colon < 0) {
            field = line.toString()
            value = ""
        } else {
            field = line.substring(0, colon)
            val start = if (colon + 1 < line.length && line[colon + 1] == ' ') colon + 2 else colon + 1
            value = line.substring(start)
        }
        line.setLength(0)
        when (field) {
            "event" -> eventType = value
            "data" -> data.append(value).append('\n')
            "id" -> if (value.indexOf('\u0000') < 0) lastEventId = value
            "retry" -> if (value.isNotEmpty() && value.all { it in '0'..'9' }) value.toUIntOrNull()?.let { retryMs = it }
            else -> Unit
        }
    }

    private fun dispatch(out: MutableList<SseEvent>) {
        if (data.isEmpty()) {
            eventType = ""
            return
        }
        data.setLength(data.length - 1) // the trailing line feed
        out.add(
            SseEvent(
                id = lastEventId.ifEmpty { null },
                event = eventType.ifEmpty { "message" },
                data = data.toString(),
                retryMs = retryMs,
            ),
        )
        data.setLength(0)
        eventType = ""
        retryMs = null
    }
}
