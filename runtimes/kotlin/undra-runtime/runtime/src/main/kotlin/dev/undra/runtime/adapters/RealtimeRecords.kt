package dev.undra.runtime.adapters

import dev.undra.runtime.UndraEnum
import dev.undra.runtime.UndraException
import dev.undra.runtime.UndraRecord
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.WireException

// The records of the opt-in `WebSocket` and `Sse` ports (ADR-047), hand-written like StandardRecords.kt: what generated
// code for `undra-ports` with the `websocket` and `sse` features would contain, so that generated bindings of a core that
// enables them refer to these types and the default adapters need no generated code. Variants are numbered as the Rust
// enums declare them (crates/undra-ports/src/ws.rs and sse.rs); every error's message is the Rust `#[error]` text.

private val optionU16: UndraCodec<UShort?> = Codecs.option(Codecs.u16)
private val optionU32Sse: UndraCodec<UInt?> = Codecs.option(Codecs.u32)
private val optionString: UndraCodec<String?> = Codecs.option(Codecs.string)

/**
 * What `WebSocket.connect` answers: `WsOpened { conn: u32, protocol: String }`.
 *
 * @property conn the connection's id, chosen by the binding; never reused by it.
 * @property protocol the subprotocol the server selected, or `""` when none was negotiated.
 */
public data class WsOpened(val conn: UInt, val protocol: String) : UndraRecord {
    /** The wire codec of [WsOpened]. */
    public companion object : UndraCodec<WsOpened> {
        override fun encode(w: UndraWriter, v: WsOpened) {
            w.writeU32(v.conn)
            w.writeStr(v.protocol)
        }

        override fun decode(r: UndraReader): WsOpened = WsOpened(conn = r.readU32(), protocol = r.readStr())
    }
}

/** One WebSocket message: `WsMessage { Text(String), Binary(Bytes) }`. Control frames (ping, pong, close) never surface as messages. */
public sealed interface WsMessage : UndraEnum {
    /** A text message (valid UTF-8 by RFC 6455). */
    public data class Text(val value: String) : WsMessage

    /** A binary message. */
    public data class Binary(val value: ByteArray) : WsMessage {
        override fun equals(other: Any?): Boolean = this === other || (other is Binary && value.contentEquals(other.value))

        override fun hashCode(): Int = value.contentHashCode()

        override fun toString(): String = "Binary(${value.size} bytes)"
    }

    /** The wire codec of [WsMessage]. */
    public companion object : UndraCodec<WsMessage> {
        override fun encode(w: UndraWriter, v: WsMessage) {
            when (v) {
                is Text -> {
                    w.writeU16(0u)
                    w.writeStr(v.value)
                }
                is Binary -> {
                    w.writeU16(1u)
                    w.writeBytes(v.value)
                }
            }
        }

        override fun decode(r: UndraReader): WsMessage {
            val at = r.position
            return when (val tag = r.readU16().toInt()) {
                0 -> Text(r.readStr())
                1 -> Binary(r.readBytes())
                else -> throw WireException.InvalidTag(tag.toUInt(), at, "WsMessage")
            }
        }
    }
}

/**
 * Why a WebSocket could not be opened, or how it ended (ADR-047 §5):
 * `WsError { Refused { status: Option<u16>, message: String }, Network(String), Protocol(String), Closed { code: u16, reason: String } }`.
 *
 * | What happened | Variant |
 * |---|---|
 * | the upgrade was answered non-101, the URL is unusable, headers the platform cannot send | [Refused] |
 * | the connection dropped without a close frame (DNS, reset, TLS, timeout) | [Network] |
 * | the peer broke RFC 6455 (a text frame that is not UTF-8 included), or a reply did not decode | [Protocol] |
 * | the peer sent a close frame (1000 included), or the adapter closed past its backlog limit (1008) | [Closed] |
 */
public sealed class WsError(message: String) : UndraException(message) {
    /**
     * The connection was not established.
     *
     * @property status the HTTP status of the refused upgrade where the platform reports it (this runtime does), else `null`.
     * @property reason what the platform said (the Rust field `message`).
     */
    public data class Refused(val status: UShort?, val reason: String) : WsError("the WebSocket was refused: $reason")

    /** The connection failed or dropped without a closing handshake; [reason] is the platform's text. */
    public data class Network(val reason: String) : WsError("WebSocket network error: $reason")

    /** The peer broke the protocol (or a port reply did not decode); [reason] says how. */
    public data class Protocol(val reason: String) : WsError("WebSocket protocol error: $reason")

    /**
     * The connection was closed with a close frame.
     *
     * @property code the close code (RFC 6455 §7.4): 1000 normal, 1001 going away, 1008 policy, ...
     * @property reason the close reason, possibly empty.
     */
    public data class Closed(val code: UShort, val reason: String) : WsError("the WebSocket was closed ($code): $reason")

    /** The wire codec of [WsError]. */
    public companion object : UndraCodec<WsError> {
        override fun encode(w: UndraWriter, v: WsError) {
            when (v) {
                is Refused -> {
                    w.writeU16(0u)
                    optionU16.encode(w, v.status)
                    w.writeStr(v.reason)
                }
                is Network -> {
                    w.writeU16(1u)
                    w.writeStr(v.reason)
                }
                is Protocol -> {
                    w.writeU16(2u)
                    w.writeStr(v.reason)
                }
                is Closed -> {
                    w.writeU16(3u)
                    w.writeU16(v.code)
                    w.writeStr(v.reason)
                }
            }
        }

        override fun decode(r: UndraReader): WsError {
            val at = r.position
            return when (val tag = r.readU16().toInt()) {
                0 -> Refused(status = optionU16.decode(r), reason = r.readStr())
                1 -> Network(r.readStr())
                2 -> Protocol(r.readStr())
                3 -> Closed(code = r.readU16(), reason = r.readStr())
                else -> throw WireException.InvalidTag(tag.toUInt(), at, "WsError")
            }
        }
    }
}

/**
 * One server-sent event: `SseEvent { id: Option<String>, event: String, data: String, retry_ms: Option<u32> }`.
 *
 * @property id the event's `id` field, if the event (or an earlier one, per the HTML standard's last-event-id buffer) set
 *   one. Send it back as `lastEventId` to resume.
 * @property event the event type: the `event` field, `"message"` when absent.
 * @property data the `data` lines, joined with `\n`.
 * @property retryMs the `retry` field in milliseconds, when this event carried one: how long the server asks clients to
 *   wait before reconnecting.
 */
public data class SseEvent(
    val id: String?,
    val event: String,
    val data: String,
    val retryMs: UInt? = null,
) : UndraRecord {
    /** The wire codec of [SseEvent]. */
    public companion object : UndraCodec<SseEvent> {
        override fun encode(w: UndraWriter, v: SseEvent) {
            optionString.encode(w, v.id)
            w.writeStr(v.event)
            w.writeStr(v.data)
            optionU32Sse.encode(w, v.retryMs)
        }

        override fun decode(r: UndraReader): SseEvent =
            SseEvent(id = optionString.decode(r), event = r.readStr(), data = r.readStr(), retryMs = optionU32Sse.decode(r))
    }
}

/**
 * Why a server-sent event stream could not be opened, or how it ended (ADR-047 §5):
 * `SseError { Refused { status: Option<u16>, message: String }, Network(String), Protocol(String), Ended }`.
 */
public sealed class SseError(message: String) : UndraException(message) {
    /**
     * The request was answered with a status other than 2xx (a 204 means "stop"), or the URL is unusable.
     *
     * @property status the HTTP status, if there was an answer.
     * @property reason what the platform said (the Rust field `message`).
     */
    public data class Refused(val status: UShort?, val reason: String) : SseError("the event stream was refused: $reason")

    /** The connection failed or dropped; [reason] is the platform's text. */
    public data class Network(val reason: String) : SseError("event stream network error: $reason")

    /** The answer was not `text/event-stream`, was not UTF-8, or a port reply did not decode. */
    public data class Protocol(val reason: String) : SseError("event stream protocol error: $reason")

    /** The server ended the response. Reconnect with the last event id to resume. */
    public data object Ended : SseError("the server ended the event stream")

    /** The wire codec of [SseError]. */
    public companion object : UndraCodec<SseError> {
        override fun encode(w: UndraWriter, v: SseError) {
            when (v) {
                is Refused -> {
                    w.writeU16(0u)
                    optionU16.encode(w, v.status)
                    w.writeStr(v.reason)
                }
                is Network -> {
                    w.writeU16(1u)
                    w.writeStr(v.reason)
                }
                is Protocol -> {
                    w.writeU16(2u)
                    w.writeStr(v.reason)
                }
                Ended -> w.writeU16(3u)
            }
        }

        override fun decode(r: UndraReader): SseError {
            val at = r.position
            return when (val tag = r.readU16().toInt()) {
                0 -> Refused(status = optionU16.decode(r), reason = r.readStr())
                1 -> Network(r.readStr())
                2 -> Protocol(r.readStr())
                3 -> Ended
                else -> throw WireException.InvalidTag(tag.toUInt(), at, "SseError")
            }
        }
    }
}
