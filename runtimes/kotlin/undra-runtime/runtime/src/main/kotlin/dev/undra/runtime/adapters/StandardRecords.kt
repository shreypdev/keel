package dev.undra.runtime.adapters

import dev.undra.runtime.UndraEnum
import dev.undra.runtime.UndraException
import dev.undra.runtime.UndraRecord
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.WireException

// Hand-written codecs for the records, enums and errors of the standard ports (SPEC section 8): HttpMethod,
// Header, HttpRequest, HttpResponse, HttpError, FsError, StorageError (ADR-049), NetKind and AppState, and the three
// records of ADR-046 (UndraPanicFrame, UndraPanicReport, UndraBackgroundReport). They are
// what generated code for `undra-ports` would contain; the runtime carries its own so that the default adapters
// (and the `android-adapters` module) do not depend on generated code. Enum and error variants are numbered as
// `undra-ports` declares them (`crates/undra-ports/tests/encoding.rs` checks this file against it).

private val headerList: UndraCodec<List<Header>> = Codecs.vec(Header)
private val optionBytes: UndraCodec<ByteArray?> = Codecs.option(Codecs.bytes)
private val optionU32: UndraCodec<UInt?> = Codecs.option(Codecs.u32)
private val optionString: UndraCodec<String?> = Codecs.option(Codecs.string)

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

/**
 * `FsError { NotFound, Denied, Io(String), Full, Unavailable(String) }`: why a file operation failed.
 *
 * An `Fs` adapter throws it from its methods, and its port answers the core with it (port status 1), so the core
 * receives the typed error instead of `unavailable`.
 */
public sealed class FsError(message: String) : UndraException(message) {
    /** The file or directory does not exist. */
    public data object NotFound : FsError("not found")

    /** Access is not allowed: a permission error, or a path that leaves the file root. */
    public data object Denied : FsError("access denied")

    /** Any other I/O failure; [reason] is the platform's description. */
    public data class Io(val reason: String) : FsError("I/O error: $reason")

    /** The disk or the storage quota is exhausted (`ENOSPC`, `EDQUOT`; ADR-049). */
    public data object Full : FsError("the disk is full")

    /** No file system in this context, or no adapter registered; [reason] says which (ADR-049). */
    public data class Unavailable(val reason: String) : FsError("the file system is unavailable: $reason")

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
                Full -> w.writeU16(3u)
                is Unavailable -> {
                    w.writeU16(4u)
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
                3 -> Full
                4 -> Unavailable(r.readStr())
                else -> throw WireException.InvalidTag(tag.toUInt(), at, "FsError")
            }
        }
    }
}

/**
 * `StorageError { Unavailable(String), Full, Locked, Corrupt(String), Io(String) }`: why a `Kv` or `SecureStore`
 * operation failed (ADR-049).
 *
 * Storage is not infallible: a quota runs out, a key that needs the user's authentication cannot be used, a stored
 * file is damaged. Every method of the two storage ports reports those as one of these variants, never as a crash of
 * the core:
 *
 * | Variant | Meaning |
 * |---|---|
 * | [Unavailable] | no adapter, or no backend in this context (no Android Keystore) |
 * | [Full] | the quota or the disk is exhausted |
 * | [Locked] | protected data cannot be read now (a Keystore key that needs the user to authenticate) |
 * | [Corrupt] | stored bytes or ciphertext that cannot be read back; the key is still there |
 * | [Io] | anything else, with the platform's message |
 *
 * A storage adapter throws it from its methods ([KeyValueBackend]); [StoragePort] turns it into the port's typed
 * reply (status 1, this error encoded), which the core receives as its own `StorageError`. Anything else an adapter
 * throws is a bug in the adapter: the core is answered `unavailable` and the runtime logs it at error level.
 *
 * ```kotlin
 * override suspend fun set(key: String, value: ByteArray) {
 *     try {
 *         database.put(key, value)
 *     } catch (e: IOException) {
 *         throw StorageError.of(e) // Full for ENOSPC or EDQUOT, Io otherwise
 *     }
 * }
 * ```
 */
public sealed class StorageError(message: String) : UndraException(message) {
    /** No adapter is registered, or the platform has no backend in this context; [reason] says which. */
    public data class Unavailable(val reason: String) : StorageError("storage is unavailable: $reason")

    /** The quota or the disk is exhausted. */
    public data object Full : StorageError("the storage is full")

    /** Protected data cannot be read now (before the device's first unlock, or a key that needs the user to authenticate). */
    public data object Locked : StorageError("the storage is locked")

    /** The stored bytes (or ciphertext) cannot be read back; the key is still there. [reason] says why. */
    public data class Corrupt(val reason: String) : StorageError("stored data is corrupt: $reason")

    /** Any other failure; [reason] is the platform's description. */
    public data class Io(val reason: String) : StorageError("storage I/O error: $reason")

    /**
     * Whether retrying later can succeed without anything changing in the stored data: [Unavailable], [Locked] and
     * [Io] are about the moment, [Full] and [Corrupt] about the store (the core's `StorageError::is_transient`).
     */
    public val isTransient: Boolean
        get() = this is Unavailable || this is Locked || this is Io

    /** The wire codec of [StorageError], and the mapping of platform I/O failures onto it. */
    public companion object : UndraCodec<StorageError> {
        override fun encode(w: UndraWriter, v: StorageError) {
            when (v) {
                is Unavailable -> {
                    w.writeU16(0u)
                    w.writeStr(v.reason)
                }
                Full -> w.writeU16(1u)
                Locked -> w.writeU16(2u)
                is Corrupt -> {
                    w.writeU16(3u)
                    w.writeStr(v.reason)
                }
                is Io -> {
                    w.writeU16(4u)
                    w.writeStr(v.reason)
                }
            }
        }

        override fun decode(r: UndraReader): StorageError {
            val at = r.position
            return when (val tag = r.readU16().toInt()) {
                0 -> Unavailable(r.readStr())
                1 -> Full
                2 -> Locked
                3 -> Corrupt(r.readStr())
                4 -> Io(r.readStr())
                else -> throw WireException.InvalidTag(tag.toUInt(), at, "StorageError")
            }
        }

        /**
         * The [StorageError] an I/O failure of a storage backend stands for: [Full] when the disk or the quota is
         * exhausted (`ENOSPC` or `EDQUOT`, however the platform spells it: the JVM's `No space left on device`,
         * Android's `ENOSPC (No space left on device)`, anywhere in the cause chain), [Io] with the platform's
         * description otherwise.
         */
        public fun of(error: java.io.IOException): StorageError = if (StorageFailures.isOutOfSpace(error)) Full else Io(StorageFailures.describe(error))
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

/**
 * One frame of a panic's stack, innermost first: `PanicFrame { address: u64, symbol: Option<String>, file: Option<String>, line: Option<u32> }`
 * (ADR-046).
 *
 * A debug build of the core names its frames ([symbol], [file] and [line] are set); a release build keeps only
 * [address], which a symbolicator resolves with the symbol files `undra build --release` writes.
 *
 * @property address the instruction's offset into the core's image (its address less the image's load address), pointing
 *   into the call instruction; `0` when the core could not tell. A `u64` on the wire, a [Long] here.
 * @property symbol the function's name, when the image still has it.
 * @property file the source file, when the image still has line tables.
 * @property line the source line, when the image still has line tables.
 */
public data class UndraPanicFrame(
    val address: Long,
    val symbol: String? = null,
    val file: String? = null,
    val line: Int? = null,
) : UndraRecord {
    init {
        require(line == null || line >= 0) { "UndraPanicFrame.line is a u32 on the wire and cannot be negative: $line" }
    }

    /** The wire codec of [UndraPanicFrame] (type id `0x19a497d1`). */
    public companion object : UndraCodec<UndraPanicFrame> {
        override fun encode(w: UndraWriter, v: UndraPanicFrame) {
            w.writeI64(v.address)
            optionString.encode(w, v.symbol)
            optionString.encode(w, v.file)
            optionU32.encode(w, v.line?.toUInt())
        }

        override fun decode(r: UndraReader): UndraPanicFrame = UndraPanicFrame(
            address = r.readI64(),
            symbol = optionString.decode(r),
            file = optionString.decode(r),
            line = optionU32.decode(r)?.let { it.coerceAtMost(Int.MAX_VALUE.toUInt()).toInt() },
        )
    }
}

private val frameList: UndraCodec<List<UndraPanicFrame>> = Codecs.vec(UndraPanicFrame)

/**
 * What the core says about one contained panic, handed to `LoadOptions.onPanic` (ADR-046):
 * `PanicReport { message, location, operation, thread, frames, namespace, core_version, schema_hash, image_id }`.
 *
 * The core catches every panic at its boundary and keeps working; the call that panicked fails as before
 * (`UndraCallError.Panicked`), and this report is what an app forwards to its crash reporter. It is not an
 * exception: nothing is thrown from it.
 *
 * ```kotlin
 * UndraPlaygroundCore.load(LoadOptions(onPanic = { report ->
 *     Crashlytics.recordException(RuntimeException(report.summary))
 * }))
 * ```
 *
 * @property message the panic message.
 * @property location where the panic happened, `file:line:column`.
 * @property operation what the core was running: `Todos.add`, `explode`, `task`, `computed Todos.visible`, `observe Todos`...
 * @property thread the core's name for the thread it panicked on (`undra-core`...).
 * @property frames the stack, innermost first; may be empty.
 * @property namespace the core's namespace (`[core] namespace` in `undra.toml`).
 * @property coreVersion the core's version.
 * @property schemaHash the core's schema hash (a `u64` on the wire, a [Long] here; `UndraIds.SCHEMA_HASH.toLong()` is the bindings').
 * @property imageId lowercase hex of the Mach-O UUID or the ELF build id of the image that holds the core, which names the symbol
 *   files of its build; empty when the core could not read it.
 */
public data class UndraPanicReport(
    val message: String,
    val location: String,
    val operation: String,
    val thread: String,
    val frames: List<UndraPanicFrame>,
    val namespace: String,
    val coreVersion: String,
    val schemaHash: Long,
    val imageId: String,
) : UndraRecord {
    /** The report in one line, the way the runtime logs it when `onPanic` is not set: `operation: message (location)`. */
    public val summary: String
        get() = "${operation.ifEmpty { "the core" }}: $message ($location)"

    /** The wire codec of [UndraPanicReport] (type id `0xd08d5436`). */
    public companion object : UndraCodec<UndraPanicReport> {
        override fun encode(w: UndraWriter, v: UndraPanicReport) {
            w.writeStr(v.message)
            w.writeStr(v.location)
            w.writeStr(v.operation)
            w.writeStr(v.thread)
            frameList.encode(w, v.frames)
            w.writeStr(v.namespace)
            w.writeStr(v.coreVersion)
            w.writeI64(v.schemaHash)
            w.writeStr(v.imageId)
        }

        override fun decode(r: UndraReader): UndraPanicReport = UndraPanicReport(
            message = r.readStr(),
            location = r.readStr(),
            operation = r.readStr(),
            thread = r.readStr(),
            frames = frameList.decode(r),
            namespace = r.readStr(),
            coreVersion = r.readStr(),
            schemaHash = r.readI64(),
            imageId = r.readStr(),
        )
    }
}

/**
 * What one background run did, returned by `UndraCore.runInBackground` (ADR-046):
 * `BackgroundReport { finished: bool, replayed: u32, refetched: u32, still_pending: u32 }`.
 *
 * @property finished every background task finished before the deadline; `false` means work is still pending and the
 *   OS should be asked for another window (WorkManager: retry).
 * @property replayed offline mutations the run replayed.
 * @property refetched stale queries the run refetched.
 * @property stillPending items of background work left after the run (queued mutations, stale queries, unflushed persistence).
 */
public data class UndraBackgroundReport(
    val finished: Boolean,
    val replayed: Int,
    val refetched: Int,
    val stillPending: Int,
) : UndraRecord {
    init {
        require(replayed >= 0 && refetched >= 0 && stillPending >= 0) { "the counts of an UndraBackgroundReport are u32 on the wire and cannot be negative: $this" }
    }

    /** The wire codec of [UndraBackgroundReport] (type id `0x5dbea5f3`). */
    public companion object : UndraCodec<UndraBackgroundReport> {
        override fun encode(w: UndraWriter, v: UndraBackgroundReport) {
            w.writeBool(v.finished)
            w.writeU32(v.replayed.toUInt())
            w.writeU32(v.refetched.toUInt())
            w.writeU32(v.stillPending.toUInt())
        }

        override fun decode(r: UndraReader): UndraBackgroundReport = UndraBackgroundReport(
            finished = r.readBool(),
            replayed = count(r.readU32()),
            refetched = count(r.readU32()),
            stillPending = count(r.readU32()),
        )

        private fun count(wire: UInt): Int = wire.coerceAtMost(Int.MAX_VALUE.toUInt()).toInt()
    }
}
