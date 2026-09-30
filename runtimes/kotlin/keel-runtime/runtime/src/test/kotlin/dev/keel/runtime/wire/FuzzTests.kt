package dev.keel.runtime.wire

import dev.keel.runtime.testing.Suite
import dev.keel.runtime.testing.assertThrows
import dev.keel.runtime.testing.assertTrue
import dev.keel.runtime.testing.bytesOf
import dev.keel.runtime.testing.directBuffer
import dev.keel.runtime.testing.fail
import dev.keel.runtime.testing.hex
import dev.keel.runtime.testing.unhex
import org.junit.jupiter.api.Test
import java.util.UUID
import kotlin.random.Random

/**
 * Byte-level fuzzing of every decoder: whatever the input, the only thing a decoder may throw is a
 * [WireException]. Inputs are deterministic (fixed seed, override with `KEEL_FUZZ_SEED`; iteration count
 * with `KEEL_FUZZ_ITERATIONS`) so a failure reproduces, and every failure message carries the target,
 * the seed, the iteration and the input bytes.
 *
 * On top of "only WireException" it checks two properties on every input:
 *  - a byte-array reader and a direct-buffer reader agree exactly (same value or same error and offset);
 *  - whatever decodes successfully re-encodes to bytes that decode to the same value.
 */
class FuzzTests : Suite() {

    /** One decoder under test. [reencode] is null when the value has no encoder to compare against. */
    private class Target(val name: String, val decode: (KeelReader) -> Any?, val reencode: ((Any?) -> ByteArray)? = null)

    private class Outcome(val text: String, val value: Any?, val ok: Boolean)

    private val iterations = System.getenv("KEEL_FUZZ_ITERATIONS")?.toIntOrNull() ?: 5000
    private val seed = System.getenv("KEEL_FUZZ_SEED")?.toLongOrNull() ?: 0x4B45454CL

    @Suppress("UNCHECKED_CAST")
    private fun <T> codecTarget(name: String, codec: KeelCodec<T>) = Target(
        name,
        { r -> codec.decode(r).also { r.finish() } },
        { v -> codec.encodeToByteArray(v as T) },
    )

    private fun payloadTarget(name: String, decode: (KeelReader) -> Payloads.Payload) = Target(
        name,
        { r -> decode(r).also { r.finish() } },
        { v -> (v as Payloads.Payload).toByteArray() },
    )

    private val ints = Codecs.i32
    private val sampleList = listOf(1, 2, 3, 4, 5)

    private val targets: List<Target> = listOf(
        codecTarget("bool", Codecs.bool), codecTarget("u8", Codecs.u8), codecTarget("i8", Codecs.i8),
        codecTarget("u16", Codecs.u16), codecTarget("i16", Codecs.i16), codecTarget("u32", Codecs.u32),
        codecTarget("i32", Codecs.i32), codecTarget("u64", Codecs.u64), codecTarget("i64", Codecs.i64),
        codecTarget("f32", Codecs.f32), codecTarget("f64", Codecs.f64), codecTarget("string", Codecs.string),
        codecTarget("bytes", Codecs.bytes), codecTarget("duration", Codecs.duration), codecTarget("timestamp", Codecs.timestamp),
        codecTarget("uuid", Codecs.uuid), codecTarget("handle", Codecs.handle),
        codecTarget("option<string>", Codecs.option(Codecs.string)), codecTarget("option<i64>", Codecs.option(Codecs.i64)),
        codecTarget("vec<i32>", Codecs.vec(Codecs.i32)), codecTarget("vec<string>", Codecs.vec(Codecs.string)),
        codecTarget("vec<bytes>", Codecs.vec(Codecs.bytes)), codecTarget("vec<vec<u8>>", Codecs.vec(Codecs.vec(Codecs.u8))),
        codecTarget("vec<option<string>>", Codecs.vec(Codecs.option(Codecs.string))),
        codecTarget("map<string,i32>", Codecs.map(Codecs.string, Codecs.i32)),
        codecTarget("map<i32,vec<string>>", Codecs.map(Codecs.i32, Codecs.vec(Codecs.string))),
        codecTarget("map<uuid,bool>", Codecs.map(Codecs.uuid, Codecs.bool)),
        codecTarget("result<i32,string>", Codecs.result(Codecs.i32, Codecs.string)),
        codecTarget("result<string,vec<i32>>", Codecs.result(Codecs.string, Codecs.vec(Codecs.i32))),
        codecTarget("Todo", Todo), codecTarget("vec<Todo>", Codecs.vec(Todo)), codecTarget("Filter", Filter),
        codecTarget("Shape", Shape), codecTarget("vec<Shape>", Codecs.vec(Shape)),
        Target("Envelope", { r -> Envelope.decode(r.readRemaining()) }, { v -> (v as Envelope).encode() }),
        payloadTarget("Call") { Payloads.Call.decode(it) },
        payloadTarget("Reply") { Payloads.Reply.decode(it) },
        payloadTarget("ChangeSet") { Payloads.ChangeSet.decode(it) },
        payloadTarget("PortCall") { Payloads.PortCall.decode(it) },
        payloadTarget("PortReply") { Payloads.PortReply.decode(it) },
        payloadTarget("Cancel") { Payloads.Cancel.decode(it) },
        payloadTarget("StreamCredit") { Payloads.StreamCredit.decode(it) },
        payloadTarget("StreamItem") { Payloads.StreamItem.decode(it) },
        payloadTarget("Observe") { Payloads.Observe.decode(it) },
        payloadTarget("Release") { Payloads.Release.decode(it) },
        payloadTarget("Event") { Payloads.Event.decode(it) },
        payloadTarget("Hello") { Payloads.Hello.decode(it) },
        payloadTarget("Log") { Payloads.Log.decode(it) },
        payloadTarget("TimerFired") { Payloads.TimerFired.decode(it) },
        payloadTarget("Snapshot") { Payloads.Snapshot.decode(it) },
        Target(
            "ChangeSet.forEachEntry",
            { r ->
                val sb = StringBuilder()
                val txn = Payloads.ChangeSet.forEachEntry(r) { handle, signalId, op, value ->
                    sb.append(handle.raw).append(':').append(signalId).append(':').append(op).append(':')
                    sb.append(hex(value.readRemaining())).append(';')
                }
                "$txn|$sb"
            },
        ),
        Target("Reply.readPanic", { r -> Payloads.Reply(1u, Payloads.ReplyStatus.PANIC, r.readRemaining()).readPanic() }),
        Target(
            "Reply.readBadRequestReason",
            { r -> Payloads.Reply(1u, Payloads.ReplyStatus.BAD_REQUEST, r.readRemaining()).readBadRequestReason() },
        ),
        Target(
            "KeyedPatch<i32>",
            { r -> KeyedPatch.decodePatch(r, ints).also { r.finish() } },
            { v -> @Suppress("UNCHECKED_CAST") KeyedPatch.encodePatch(v as List<PatchOp<Int>>, ints) },
        ),
        Target(
            "KeyedPatch<string>",
            { r -> KeyedPatch.decodePatch(r, Codecs.string).also { r.finish() } },
            { v -> @Suppress("UNCHECKED_CAST") KeyedPatch.encodePatch(v as List<PatchOp<String>>, Codecs.string) },
        ),
        Target(
            "KeyedPatch<i32>.apply",
            { r -> KeyedPatch.applyPatch(sampleList, KeyedPatch.decodePatch(r, ints).also { r.finish() }) },
        ),
    )

    // ---- inputs -----------------------------------------------------------------------------------------

    private val interesting = byteArrayOf(0, 1, 2, 3, 4, 5, 0x7F, 0x80.toByte(), 0xFF.toByte(), 0xFE.toByte(), 0x10, 0x20)

    private fun randomBytes(rnd: Random): ByteArray {
        val len = when (rnd.nextInt(10)) {
            in 0..4 -> rnd.nextInt(0, 17)
            in 5..8 -> rnd.nextInt(17, 129)
            else -> rnd.nextInt(129, 1025)
        }
        return rnd.nextBytes(len)
    }

    /** Bytes drawn from a small alphabet so that tags, counts and lengths are plausible and decoding gets deep. */
    private fun structuredBytes(rnd: Random): ByteArray {
        val len = rnd.nextInt(0, 65)
        return ByteArray(len) {
            when (rnd.nextInt(6)) {
                0, 1 -> 0
                2 -> interesting[rnd.nextInt(interesting.size)]
                3 -> rnd.nextInt(0, 9).toByte()
                4 -> (0x61 + rnd.nextInt(3)).toByte()
                else -> rnd.nextInt(256).toByte()
            }
        }
    }

    private fun mutate(source: ByteArray, rnd: Random): ByteArray {
        var b = source.copyOf()
        repeat(rnd.nextInt(1, 5)) {
            when (rnd.nextInt(8)) {
                0 -> if (b.isNotEmpty()) b[rnd.nextInt(b.size)] = rnd.nextInt(256).toByte()
                1 -> if (b.isNotEmpty()) {
                    val i = rnd.nextInt(b.size)
                    b[i] = (b[i].toInt() xor (1 shl rnd.nextInt(8))).toByte()
                }
                2 -> if (b.isNotEmpty()) b[rnd.nextInt(b.size)] = interesting[rnd.nextInt(interesting.size)]
                3 -> b = b.copyOf(rnd.nextInt(b.size + 1))
                4 -> b += rnd.nextBytes(rnd.nextInt(1, 9))
                5 -> if (b.isNotEmpty()) {
                    val i = rnd.nextInt(b.size)
                    b = b.copyOfRange(0, i) + b.copyOfRange(i + 1, b.size)
                }
                6 -> {
                    val i = rnd.nextInt(b.size + 1)
                    b = b.copyOfRange(0, i) + rnd.nextBytes(1) + b.copyOfRange(i, b.size)
                }
                else -> if (b.size >= 4) {
                    val i = rnd.nextInt(b.size - 3)
                    val huge = intArrayOf(0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F, 0x00, 0x00, 0x00, 0x80)
                    val start = rnd.nextInt(3) * 4
                    for (k in 0 until 4) b[i + k] = huge[start + k].toByte()
                }
            }
        }
        return b
    }

    /** Valid encodings of many shapes, the raw material for mutation. */
    private val corpus: List<ByteArray> = buildList {
        for (v in WireVectors.all) add(unhex(v.hex))
        val todo = Todo(UUID(3, 4), "Milk", true)
        add(Codecs.vec(Todo).encodeToByteArray(listOf(todo, todo.copy(title = "héllo"))))
        add(Codecs.map(Codecs.string, Codecs.vec(Codecs.string)).encodeToByteArray(mapOf("a" to listOf("x", "y"), "b" to emptyList())))
        add(Codecs.result(Codecs.string, Codecs.vec(Codecs.i32)).encodeToByteArray(KeelResult.Ok("ok")))
        add(Codecs.vec(Shape).encodeToByteArray(listOf(Shape.Circle(1.0), Shape.Rect(2.0, 3.0))))
        val entries = listOf(
            Payloads.ChangeEntry(Handle.make(1u, 1u), 0u, Payloads.ChangeOp.FULL, bytesOf(1, 2, 3, 4)),
            Payloads.ChangeEntry(Handle.make(2u, 1u), 1u, Payloads.ChangeOp.PATCH, KeyedPatch.encodePatch(listOf(PatchOp.Insert(0u, 5), PatchOp.Clear), ints)),
            Payloads.ChangeEntry(Handle.make(3u, 1u), UInt.MAX_VALUE, Payloads.ChangeOp.INVALIDATED, ByteArray(0)),
        )
        add(Payloads.ChangeSet(9u, entries).toByteArray())
        add(Payloads.Call(Payloads.CallTarget.ObjectMethod(Handle.make(1u, 1u), 7u), 3u, bytesOf(1, 0, 0, 0)).toByteArray())
        add(Payloads.Call(Payloads.CallTarget.Constructor(1u, 2u), 3u, bytesOf(9)).toByteArray())
        add(Payloads.Call(Payloads.CallTarget.LazyListPage(Handle.make(1u, 1u), 0u, 10u), 3u, ByteArray(0)).toByteArray())
        add(Payloads.Reply(4u, Payloads.ReplyStatus.PANIC, KeelWriter().also { it.writeStr("boom"); it.writeStr("trace") }.toByteArray()).toByteArray())
        add(Payloads.PortCall(1u, 2u, 3u, bytesOf(4, 5)).toByteArray())
        add(Payloads.PortReply(3u, Payloads.PortStatus.ERROR, bytesOf(1)).toByteArray())
        add(Payloads.StreamItem(5u, Payloads.StreamFlag.ITEM, bytesOf(1, 2)).toByteArray())
        add(Payloads.Observe(Handle.make(1u, 1u), 2u, true).toByteArray())
        add(Payloads.Hello("0.1.0", 42uL, "jvm", "dev").toByteArray())
        add(Payloads.Log(2u, "core", "hello").toByteArray())
        add(Payloads.Snapshot(1u, listOf(Payloads.Snapshot.Store(Handle.make(1u, 1u), 5u, listOf(Payloads.Snapshot.Signal(0u, bytesOf(1, 2)))))).toByteArray())
        for (k in Envelope.Kind.entries) add(Envelope.encode(k, k.code.toUInt(), 0x0102030405060708uL, bytesOf(k.code.toInt(), 1, 2)))
        add(KeyedPatch.encodePatch(listOf(PatchOp.Insert(1u, 2), PatchOp.Remove(0u), PatchOp.Update(0u, 1), PatchOp.Move(0u, 1u), PatchOp.Clear), ints))
    }

    // ---- harness ----------------------------------------------------------------------------------------

    private fun preview(bytes: ByteArray): String = "${bytes.size} bytes: " + hex(bytes.copyOf(minOf(bytes.size, 96))) + if (bytes.size > 96) "..." else ""

    private fun digest(v: Any?): String = when (v) {
        null -> "null"
        is ByteArray -> "b" + hex(v)
        is Float -> "f" + v.toRawBits()
        is Double -> "d" + v.toRawBits()
        is List<*> -> v.joinToString(",", "[", "]") { digest(it) }
        is Map<*, *> -> v.entries.map { digest(it.key) + "=" + digest(it.value) }.sorted().joinToString(",", "{", "}")
        is KeelResult.Ok<*> -> "ok(" + digest(v.value) + ")"
        is KeelResult.Err<*> -> "err(" + digest(v.error) + ")"
        is PatchOp.Insert<*> -> "ins(${v.index},${digest(v.item)})"
        is PatchOp.Update<*> -> "upd(${v.index},${digest(v.item)})"
        is Payloads.Payload -> digest(v.toByteArray())
        is Envelope -> digest(v.encode())
        else -> v.toString()
    }

    private fun decodeWith(target: Target, bytes: ByteArray, direct: Boolean, context: String): Outcome {
        val reader = if (direct) KeelReader(directBuffer(bytes)) else KeelReader(bytes)
        return try {
            val value = target.decode(reader)
            Outcome("ok:" + digest(value), value, true)
        } catch (e: WireException) {
            Outcome("err:" + e.javaClass.simpleName + ":" + e.message, null, false)
        } catch (t: Throwable) {
            throw AssertionError(
                "$context: decoder '${target.name}' threw ${t.javaClass.name} (${t.message}) instead of a WireException; input ${preview(bytes)}",
                t,
            )
        }
    }

    /** Runs every target over [bytes] and checks the three properties. */
    private fun exercise(bytes: ByteArray, context: String, alsoDirect: Boolean) {
        for (target in targets) {
            val viaArray = decodeWith(target, bytes, false, context)
            if (alsoDirect) {
                val viaDirect = decodeWith(target, bytes, true, context)
                if (viaArray.text != viaDirect.text) {
                    fail("$context: '${target.name}' disagrees between backings\n  array:  ${viaArray.text.take(300)}\n  direct: ${viaDirect.text.take(300)}\n  input ${preview(bytes)}")
                }
            }
            val reencode = target.reencode
            if (viaArray.ok && reencode != null) {
                val again = reencode(viaArray.value)
                val second = decodeWith(target, again, false, "$context (re-decode)")
                if (!second.ok || second.text != viaArray.text) {
                    fail("$context: '${target.name}' is not stable under re-encoding\n  first:  ${viaArray.text.take(300)}\n  second: ${second.text.take(300)}\n  input ${preview(bytes)}")
                }
            }
        }
    }

    init {
        case("random bytes only ever raise WireException ($iterations iterations, seed $seed)") {
            val rnd = Random(seed)
            for (i in 0 until iterations) exercise(randomBytes(rnd), "random #$i (seed $seed)", alsoDirect = i % 4 == 0)
        }

        case("bytes from a small alphabet, which reach deep into the decoders ($iterations iterations)") {
            val rnd = Random(seed + 1)
            for (i in 0 until iterations) exercise(structuredBytes(rnd), "structured #$i (seed ${seed + 1})", alsoDirect = i % 4 == 0)
        }

        case("mutations of valid encodings ($iterations iterations)") {
            val rnd = Random(seed + 2)
            for (i in 0 until iterations) {
                val source = corpus[rnd.nextInt(corpus.size)]
                exercise(mutate(source, rnd), "mutation #$i (seed ${seed + 2})", alsoDirect = i % 4 == 0)
            }
        }

        case("every prefix of every valid encoding, and a bit flip at every byte") {
            val rnd = Random(seed + 3)
            for ((index, valid) in corpus.withIndex()) {
                for (n in valid.indices) exercise(valid.copyOf(n), "corpus #$index truncated to $n", alsoDirect = true)
                for (i in valid.indices) {
                    val flipped = valid.copyOf()
                    flipped[i] = (flipped[i].toInt() xor (1 shl rnd.nextInt(8))).toByte()
                    exercise(flipped, "corpus #$index flipped at $i", alsoDirect = i % 2 == 0)
                }
            }
        }

        case("the unmodified corpus decodes without error under its own type and never crashes elsewhere") {
            for ((index, valid) in corpus.withIndex()) exercise(valid, "corpus #$index", alsoDirect = true)
            assertTrue(corpus.size > 40, "corpus unexpectedly small: ${corpus.size}")
        }

        case("the harness itself reports any exception that is not a WireException") {
            val faulty = Target("faulty", { r -> if (r.remaining > 1) throw IndexOutOfBoundsException("oops") else 0 })
            val e = assertThrows<AssertionError> { decodeWith(faulty, ByteArray(4), false, "self-test") }
            assertTrue(e.message!!.contains("IndexOutOfBoundsException") && e.message!!.contains("faulty"), "unhelpful report: ${e.message}")
            val wire = Target("wire", { r -> r.readU32() })
            assertTrue(!decodeWith(wire, ByteArray(1), false, "self-test").ok, "a WireException must be reported as an error outcome, not thrown")
        }
    }

    @Test
    fun allCases() = assertPassed()
}
