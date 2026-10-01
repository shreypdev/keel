package dev.undra.testkit

import dev.undra.runtime.UndraException

/** The `format` of a recording. */
public const val RECORDING_FORMAT: String = "undra.recording"

/** The `version` this kit reads and writes. */
public const val RECORDING_VERSION: Int = 1

/** Why a recording could not be read: names the event ([event] is `null` for the document header) and the [field]. */
public class RecordingException(message: String, public val event: Int? = null, public val field: String? = null) : UndraException(message)

/** Which call a `call` event is (docs/SPEC.md section 3.3). Handles are the signed longs the runtime uses for them. */
public sealed interface RecordedTarget {
    /** A free function. */
    public data class Function(val method: UInt) : RecordedTarget

    /** A method of the object [handle]. */
    public data class Method(val handle: Long, val method: UInt) : RecordedTarget

    /** A constructor of the object type [type]. */
    public data class Constructor(val type: UInt, val method: UInt) : RecordedTarget

    /** A page of a lazy list. */
    public data class Page(val handle: Long, val offset: UInt, val limit: UInt) : RecordedTarget
}

/** The status of a recorded `reply`; [json] is its name in the file. */
public enum class ReplyStatusName(public val json: String, public val code: Int) {
    OK("ok", 0), ERROR("error", 1), PANIC("panic", 2), CANCELLED("cancelled", 3), STREAM_OPENED("stream_opened", 4), BAD_REQUEST("bad_request", 5)
}

/** The status of a recorded `port_reply`. */
public enum class PortStatusName(public val json: String, public val code: Int) {
    OK("ok", 0), ERROR("error", 1), UNAVAILABLE("unavailable", 2)
}

/** What a recorded `stream_item` carries. */
public enum class StreamFlagName(public val json: String, public val code: Int) {
    ITEM("item", 0), END("end", 1), ERROR("error", 2), FAILED("failed", 3)
}

/** How a recorded change-set entry's value reads. */
public enum class ChangeOpName(public val json: String, public val code: Int) {
    FULL("full", 0), PATCH("patch", 1), LAZY_INVALIDATED("lazy_invalidated", 2)
}

/** One signal update of a recorded change-set. */
public class RecordedEntry(public val handle: Long, public val signal: UInt, public val op: ChangeOpName, public val value: ByteArray)

/** What happened. Equality is by the canonical JSON line, so two events are equal when they would be written as the same text. */
public sealed class RecordedKind {
    /** Host to core: a call. */
    public class Call(public val target: RecordedTarget, public val call: UInt, public val args: ByteArray) : RecordedKind()

    /** Core to host: a call's reply. [body] is the SPEC 3.4 body of [status]. */
    public class Reply(public val call: UInt, public val status: ReplyStatusName, public val body: ByteArray) : RecordedKind()

    /** Core to host: one transaction's updates. */
    public class ChangeSet(public val txn: ULong, public val entries: List<RecordedEntry>) : RecordedKind()

    /** Core to host: an item, end or failure of a stream. */
    public class StreamItem(public val call: UInt, public val flag: StreamFlagName, public val body: ByteArray) : RecordedKind()

    /** Core to host: the core calls a port. */
    public class PortCall(public val port: UInt, public val method: UInt, public val call: UInt, public val args: ByteArray) : RecordedKind()

    /** Host to core: the answer to a port call. */
    public class PortReply(public val call: UInt, public val status: PortStatusName, public val body: ByteArray) : RecordedKind()

    /** Host to core: an event of an event port. */
    public class PortEvent(public val port: UInt, public val method: UInt, public val payload: ByteArray) : RecordedKind()

    /** Host to core: a platform timer fired. */
    public class TimerFired(public val timer: UInt) : RecordedKind()

    /** Host to core: start or stop observing a signal (`UInt.MAX_VALUE` is every signal). */
    public class Observe(public val handle: Long, public val signal: UInt, public val on: Boolean) : RecordedKind()

    /** Host to core: an object was released. */
    public class Release(public val handle: Long) : RecordedKind()

    /** Host to core: a call or stream was cancelled. */
    public class Cancel(public val call: UInt) : RecordedKind()
}

/** One event and when it happened: whole milliseconds since the session started. */
public class RecordedEvent(public val t: Long, public val kind: RecordedKind) {
    /** The event as one line of canonical JSON. */
    public fun toJsonLine(): String = eventLine(this)

    override fun equals(other: Any?): Boolean = other is RecordedEvent && toJsonLine() == other.toJsonLine()

    override fun hashCode(): Int = toJsonLine().hashCode()

    override fun toString(): String = toJsonLine()
}

/**
 * A recorded session (`undra.recording`, version 1): the events of one core's life in order, with the schema hash of that core.
 * [toJson] writes the canonical text (the same bytes the Rust, Swift and TypeScript writers produce); [fromJson] reads it.
 */
public class Recording(
    public val schemaHash: ULong,
    public val source: String,
    public val platform: String?,
    public val events: List<RecordedEvent>,
) {
    /** The canonical JSON text, ending with a newline: equal recordings are equal bytes. */
    public fun toJson(): String {
        val out = StringBuilder()
        out.append("{\n  \"format\": \"$RECORDING_FORMAT\",\n  \"version\": $RECORDING_VERSION,\n")
        out.append("  \"schema_hash\": \"${hex64(schemaHash)}\",\n  \"source\": ${jsonString(source)},\n")
        if (platform != null) out.append("  \"platform\": ${jsonString(platform)},\n")
        if (events.isEmpty()) return out.append("  \"events\": []\n}\n").toString()
        out.append("  \"events\": [\n")
        events.forEachIndexed { index, e ->
            out.append("    ").append(e.toJsonLine()).append(if (index + 1 < events.size) ",\n" else "\n")
        }
        return out.append("  ]\n}\n").toString()
    }

    /** A copy with [schemaHash] replaced. */
    public fun withSchemaHash(hash: ULong): Recording = Recording(hash, source, platform, events)

    /** Reads and writes recordings. */
    public companion object {
        /**
         * Reads a recording.
         *
         * @throws RecordingException for text that is not JSON, another `format`, a `version` this kit does not read, or a field that is missing
         *   or malformed (named, with the event's index).
         */
        public fun fromJson(text: String): Recording = readRecording(text)
    }
}

private fun nameField(port: UInt, method: UInt): String = standardName(port, method)?.let { ",\"name\":${jsonString(it)}" } ?: ""

private fun eventLine(e: RecordedEvent): String {
    val head = "{\"t\":${e.t}"
    val k = e.kind
    return when (k) {
        is RecordedKind.Call -> {
            val target = when (val t = k.target) {
                is RecordedTarget.Function -> "\"target\":\"function\",\"method\":${t.method}"
                is RecordedTarget.Method -> "\"target\":\"method\",\"handle\":\"${hex64(t.handle)}\",\"method\":${t.method}"
                is RecordedTarget.Constructor -> "\"target\":\"constructor\",\"type\":${t.type},\"method\":${t.method}"
                is RecordedTarget.Page -> "\"target\":\"page\",\"handle\":\"${hex64(t.handle)}\",\"offset\":${t.offset},\"limit\":${t.limit}"
            }
            "$head,\"kind\":\"call\",$target,\"call\":${k.call},\"args\":\"${k.args.toHex()}\"}"
        }
        is RecordedKind.Reply -> "$head,\"kind\":\"reply\",\"call\":${k.call},\"status\":\"${k.status.json}\",\"body\":\"${k.body.toHex()}\"}"
        is RecordedKind.ChangeSet -> {
            val entries = k.entries.joinToString(",") {
                "{\"handle\":\"${hex64(it.handle)}\",\"signal\":${it.signal},\"op\":\"${it.op.json}\",\"value\":\"${it.value.toHex()}\"}"
            }
            "$head,\"kind\":\"change_set\",\"txn\":${k.txn},\"entries\":[$entries]}"
        }
        is RecordedKind.StreamItem -> "$head,\"kind\":\"stream_item\",\"call\":${k.call},\"flag\":\"${k.flag.json}\",\"body\":\"${k.body.toHex()}\"}"
        is RecordedKind.PortCall ->
            "$head,\"kind\":\"port_call\",\"port\":${k.port},\"method\":${k.method},\"call\":${k.call},\"args\":\"${k.args.toHex()}\"${nameField(k.port, k.method)}}"
        is RecordedKind.PortReply -> "$head,\"kind\":\"port_reply\",\"call\":${k.call},\"status\":\"${k.status.json}\",\"body\":\"${k.body.toHex()}\"}"
        is RecordedKind.PortEvent ->
            "$head,\"kind\":\"event\",\"port\":${k.port},\"method\":${k.method},\"payload\":\"${k.payload.toHex()}\"${nameField(k.port, k.method)}}"
        is RecordedKind.TimerFired -> "$head,\"kind\":\"timer_fired\",\"timer\":${k.timer}}"
        is RecordedKind.Observe -> "$head,\"kind\":\"observe\",\"handle\":\"${hex64(k.handle)}\",\"signal\":${k.signal},\"on\":${k.on}}"
        is RecordedKind.Release -> "$head,\"kind\":\"release\",\"handle\":\"${hex64(k.handle)}\"}"
        is RecordedKind.Cancel -> "$head,\"kind\":\"cancel\",\"call\":${k.call}}"
    }
}

/** A typed view of one JSON object, reporting the first bad field. */
private class Fields(private val index: Int?, private val obj: Map<String, Json>) {
    fun bad(field: String, problem: String): RecordingException {
        val where = if (index == null) "" else "event $index: "
        return RecordingException("$where\"$field\" $problem", index, field)
    }

    fun long(field: String): Long {
        val v = obj[field] as? Json.Num ?: throw bad(field, "must be a non-negative integer")
        val n = v.raw.toLongOrNull() ?: throw bad(field, "must be a non-negative integer")
        if (n < 0) throw bad(field, "must be a non-negative integer")
        return n
    }

    fun u32(field: String): UInt {
        val n = long(field)
        if (n > 0xffff_ffffL) throw bad(field, "does not fit a u32")
        return n.toUInt()
    }

    fun str(field: String): String = (obj[field] as? Json.Str)?.value ?: throw bad(field, "must be a string")

    fun handle(field: String): Long = parseHex64(str(field))?.toLong() ?: throw bad(field, "must be a \"0x..\" string")

    fun bytes(field: String): ByteArray = str(field).fromHex() ?: throw bad(field, "must be hex")

    fun bool(field: String): Boolean = (obj[field] as? Json.Bool)?.value ?: throw bad(field, "must be true or false")

    fun <T> oneOf(field: String, all: Array<T>, name: (T) -> String, what: String): T {
        val v = str(field)
        return all.firstOrNull { name(it) == v } ?: throw bad(field, "is not $what")
    }
}

private fun parseEvent(index: Int, raw: Json): RecordedEvent {
    val obj = (raw as? Json.Obj)?.fields ?: throw RecordingException("event $index must be an object", index)
    val f = Fields(index, obj)
    val t = f.long("t")
    val kind: RecordedKind = when (f.str("kind")) {
        "call" -> {
            val target = when (f.str("target")) {
                "function" -> RecordedTarget.Function(f.u32("method"))
                "method" -> RecordedTarget.Method(f.handle("handle"), f.u32("method"))
                "constructor" -> RecordedTarget.Constructor(f.u32("type"), f.u32("method"))
                "page" -> RecordedTarget.Page(f.handle("handle"), f.u32("offset"), f.u32("limit"))
                else -> throw f.bad("target", "is not function, method, constructor or page")
            }
            RecordedKind.Call(target, f.u32("call"), f.bytes("args"))
        }
        "reply" -> RecordedKind.Reply(f.u32("call"), f.oneOf("status", ReplyStatusName.entries.toTypedArray(), { it.json }, "a reply status"), f.bytes("body"))
        "change_set" -> {
            val list = (obj["entries"] as? Json.Arr)?.items ?: throw f.bad("entries", "must be an array")
            val entries = list.map { e ->
                val ef = Fields(index, (e as? Json.Obj)?.fields ?: throw f.bad("entries", "must hold objects"))
                RecordedEntry(
                    ef.handle("handle"),
                    ef.u32("signal"),
                    ef.oneOf("op", ChangeOpName.entries.toTypedArray(), { it.json }, "full, patch or lazy_invalidated"),
                    ef.bytes("value"),
                )
            }
            RecordedKind.ChangeSet(f.long("txn").toULong(), entries)
        }
        "stream_item" ->
            RecordedKind.StreamItem(f.u32("call"), f.oneOf("flag", StreamFlagName.entries.toTypedArray(), { it.json }, "item, end, error or failed"), f.bytes("body"))
        "port_call" -> RecordedKind.PortCall(f.u32("port"), f.u32("method"), f.u32("call"), f.bytes("args"))
        "port_reply" ->
            RecordedKind.PortReply(f.u32("call"), f.oneOf("status", PortStatusName.entries.toTypedArray(), { it.json }, "ok, error or unavailable"), f.bytes("body"))
        "event" -> RecordedKind.PortEvent(f.u32("port"), f.u32("method"), f.bytes("payload"))
        "timer_fired" -> RecordedKind.TimerFired(f.u32("timer"))
        "observe" -> RecordedKind.Observe(f.handle("handle"), f.u32("signal"), f.bool("on"))
        "release" -> RecordedKind.Release(f.handle("handle"))
        "cancel" -> RecordedKind.Cancel(f.u32("call"))
        else -> throw f.bad("kind", "is not a known event kind")
    }
    return RecordedEvent(t, kind)
}

private fun readRecording(text: String): Recording {
    val doc = try {
        parseJson(text)
    } catch (e: JsonException) {
        throw RecordingException("the recording is not valid JSON: ${e.message}")
    }
    val obj = (doc as? Json.Obj)?.fields ?: throw RecordingException("the recording must be a JSON object")
    val format = (obj["format"] as? Json.Str)?.value ?: ""
    if (format != RECORDING_FORMAT) throw RecordingException("not a recording: format is \"$format\", expected \"$RECORDING_FORMAT\"")
    val version = (obj["version"] as? Json.Num)?.raw
    if (version != RECORDING_VERSION.toString()) {
        throw RecordingException("recording version ${version ?: "(none)"} is not supported (this reader knows version $RECORDING_VERSION)")
    }
    val head = Fields(null, obj)
    val schemaHash = parseHex64(head.str("schema_hash")) ?: throw head.bad("schema_hash", "must be a \"0x..\" string")
    val source = head.str("source")
    val platform = when (val p = obj["platform"]) {
        null, Json.Null -> null
        is Json.Str -> p.value
        else -> throw head.bad("platform", "must be a string")
    }
    val events = (obj["events"] as? Json.Arr)?.items ?: throw head.bad("events", "must be an array")
    return Recording(schemaHash, source, platform, events.mapIndexed { i, e -> parseEvent(i, e) })
}
