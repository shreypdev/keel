package dev.undra.runtime.adapters

import dev.undra.runtime.UndraEnum
import dev.undra.runtime.UndraException
import dev.undra.runtime.UndraRecord
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.WireException

// Hand-written codecs for the records of the standard ports (SPEC section 8). They are what generated code
// for `undra-ports` would contain; the runtime carries its own so that the default adapters (and the
// `android-adapters` module) do not depend on generated code. Enum and error variants are numbered in the
// order SPEC section 8 lists them.

private val headerList: UndraCodec<List<Header>> = Codecs.vec(Header)
private val optionBytes: UndraCodec<ByteArray?> = Codecs.option(Codecs.bytes)
private val optionU32: UndraCodec<UInt?> = Codecs.option(Codecs.u32)

/** One HTTP header: `Header { name: String, value: String }`. */
public data class Header(val name: String, val value: String) : UndraRecord {
    /** The wire codec of [Header]. */
    public companion object : UndraCodec<Header> {
        override fun encode(w: UndraWriter, v: Header) {
            w.writeStr(v.name)
            w.writeStr(v.value)
        }

        override fun decode(r: UndraReader): Header = Header(name = r.readStr(), value = r.readStr())
    }
}

/**
 * The HTTP method of a request. SPEC section 8 names the type but not its variants: this runtime numbers
 * them `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, `OPTIONS` from 0, and `undra-ports` must declare
 * `HttpMethod` in the same order.
 */
public enum class HttpMethod(public val index: UShort) : UndraEnum {
    /** `GET`. */
    GET(0u),

    /** `POST`. */
    POST(1u),

    /** `PUT`. */
    PUT(2u),

    /** `DELETE`. */
    DELETE(3u),

    /** `PATCH`. */
    PATCH(4u),

    /** `HEAD`. */
    HEAD(5u),

    /** `OPTIONS`. */
    OPTIONS(6u),
    ;

    /** The wire codec of [HttpMethod] (a `u16` variant index). */
    public companion object : UndraCodec<HttpMethod> {
        override fun encode(w: UndraWriter, v: HttpMethod): Unit = w.writeU16(v.index)

        override fun decode(r: UndraReader): HttpMethod {
            val at = r.position
            val tag = r.readU16()
            return entries.firstOrNull { it.index == tag } ?: throw WireException.InvalidTag(tag.toUInt(), at, "HttpMethod")
        }
    }
}

/**
 * `HttpRequest { method, url, headers, body: Option<Bytes>, timeout_ms: Option<u32> }`.
 *
 * @property timeoutMs the whole request's timeout in milliseconds, or `null` for the adapter's default.
 */
public data class HttpRequest(
    val method: HttpMethod,
    val url: String,
    val headers: List<Header> = emptyList(),
    val body: ByteArray? = null,
    val timeoutMs: UInt? = null,
) : UndraRecord {
    override fun equals(other: Any?): Boolean =
        this === other || (
            other is HttpRequest && method == other.method && url == other.url && headers == other.headers &&
                body.contentEquals(other.body) && timeoutMs == other.timeoutMs
            )

    override fun hashCode(): Int {
        var result = method.hashCode()
        result = 31 * result + url.hashCode()
        result = 31 * result + headers.hashCode()
        result = 31 * result + body.contentHashCode()
        return 31 * result + timeoutMs.hashCode()
    }

    /** The wire codec of [HttpRequest]. */
    public companion object : UndraCodec<HttpRequest> {
        override fun encode(w: UndraWriter, v: HttpRequest) {
            HttpMethod.encode(w, v.method)
            w.writeStr(v.url)
            headerList.encode(w, v.headers)
            optionBytes.encode(w, v.body)
            optionU32.encode(w, v.timeoutMs)
        }

        override fun decode(r: UndraReader): HttpRequest = HttpRequest(
            method = HttpMethod.decode(r),
            url = r.readStr(),
            headers = headerList.decode(r),
            body = optionBytes.decode(r),
            timeoutMs = optionU32.decode(r),
        )
    }
}

/** `HttpResponse { status: u16, headers: Vec<Header>, body: Bytes }`. */
public data class HttpResponse(val status: UShort, val headers: List<Header>, val body: ByteArray) : UndraRecord {
    override fun equals(other: Any?): Boolean =
        this === other || (other is HttpResponse && status == other.status && headers == other.headers && body.contentEquals(other.body))

    override fun hashCode(): Int = 31 * (31 * status.hashCode() + headers.hashCode()) + body.contentHashCode()

    /** The wire codec of [HttpResponse]. */
    public companion object : UndraCodec<HttpResponse> {
        override fun encode(w: UndraWriter, v: HttpResponse) {
            w.writeU16(v.status)
            headerList.encode(w, v.headers)
            w.writeBytes(v.body)
        }

        override fun decode(r: UndraReader): HttpResponse =
            HttpResponse(status = r.readU16(), headers = headerList.decode(r), body = r.readBytes())
    }
}

/** `HttpError { Network(String), Timeout, Cancelled, InvalidUrl(String) }`: why an HTTP request failed. */
public sealed class HttpError(message: String) : UndraException(message) {
    /** The connection failed or broke. */
    public data class Network(val reason: String) : HttpError("network error: $reason")

    /** The request took longer than its timeout. */
    public data object Timeout : HttpError("the request timed out")

    /** The request was cancelled before it finished. */
    public data object Cancelled : HttpError("the request was cancelled")

    /** The URL (or a header) could not be used. */
    public data class InvalidUrl(val reason: String) : HttpError("invalid URL: $reason")

    /** The wire codec of [HttpError]. */
    public companion object : UndraCodec<HttpError> {
        override fun encode(w: UndraWriter, v: HttpError) {
            when (v) {
                is Network -> {
                    w.writeU16(0u)
                    w.writeStr(v.reason)
                }
                Timeout -> w.writeU16(1u)
                Cancelled -> w.writeU16(2u)
                is InvalidUrl -> {
                    w.writeU16(3u)
                    w.writeStr(v.reason)
                }
            }
        }

        override fun decode(r: UndraReader): HttpError {
            val at = r.position
            return when (val tag = r.readU16().toInt()) {
                0 -> Network(r.readStr())
                1 -> Timeout
                2 -> Cancelled
                3 -> InvalidUrl(r.readStr())
                else -> throw WireException.InvalidTag(tag.toUInt(), at, "HttpError")
            }
        }
    }
}

/** `FsError { NotFound, Denied, Io(String) }`: why a file operation failed. */
public sealed class FsError(message: String) : UndraException(message) {
    /** The file or directory does not exist. */
    public data object NotFound : FsError("not found")

    /** Access is not allowed: a permission error, or a path that leaves the file root. */
    public data object Denied : FsError("access denied")

    /** Any other I/O failure. */
    public data class Io(val reason: String) : FsError("I/O error: $reason")

    /** The wire codec of [FsError]. */
    public companion object : UndraCodec<FsError> {
        override fun encode(w: UndraWriter, v: FsError) {
            when (v) {
                NotFound -> w.writeU16(0u)
                Denied -> w.writeU16(1u)
                is Io -> {
                    w.writeU16(2u)
                    w.writeStr(v.reason)
                }
            }
        }

        override fun decode(r: UndraReader): FsError {
            val at = r.position
            return when (val tag = r.readU16().toInt()) {
                0 -> NotFound
                1 -> Denied
                2 -> Io(r.readStr())
                else -> throw WireException.InvalidTag(tag.toUInt(), at, "FsError")
            }
        }
    }
}

/** `NetKind { Wifi, Cellular, Wired, Unknown, None }`: what kind of network the device is on. */
public enum class NetKind(public val index: UShort) : UndraEnum {
    /** Wi-Fi. */
    WIFI(0u),

    /** Mobile data. */
    CELLULAR(1u),

    /** Ethernet or another wired link. */
    WIRED(2u),

    /** Connected, but the kind is not known. */
    UNKNOWN(3u),

    /** No network. */
    NONE(4u),
    ;

    /** The wire codec of [NetKind] (a `u16` variant index). */
    public companion object : UndraCodec<NetKind> {
        override fun encode(w: UndraWriter, v: NetKind): Unit = w.writeU16(v.index)

        override fun decode(r: UndraReader): NetKind {
            val at = r.position
            val tag = r.readU16()
            return entries.firstOrNull { it.index == tag } ?: throw WireException.InvalidTag(tag.toUInt(), at, "NetKind")
        }
    }
}

/**
 * `AppState { Active, Inactive, Background }`: where the app is in its lifecycle. Numbered in the order
 * the type is declared in SPEC section 8 (the trait's comment lists `Active | Background | Inactive`;
 * the type definition is taken as authoritative).
 */
public enum class AppState(public val index: UShort) : UndraEnum {
    /** In the foreground and receiving input. */
    ACTIVE(0u),

    /** In the foreground but not receiving input (an interruption is showing). */
    INACTIVE(1u),

    /** Not visible. */
    BACKGROUND(2u),
    ;

    /** The wire codec of [AppState] (a `u16` variant index). */
    public companion object : UndraCodec<AppState> {
        override fun encode(w: UndraWriter, v: AppState): Unit = w.writeU16(v.index)

        override fun decode(r: UndraReader): AppState {
            val at = r.position
            val tag = r.readU16()
            return entries.firstOrNull { it.index == tag } ?: throw WireException.InvalidTag(tag.toUInt(), at, "AppState")
        }
    }
}
