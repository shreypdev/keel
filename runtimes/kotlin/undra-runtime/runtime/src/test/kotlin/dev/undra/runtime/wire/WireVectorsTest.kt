package dev.undra.runtime.wire

import dev.undra.runtime.testing.JV
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.WireVector
import dev.undra.runtime.testing.assertBytes
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.testing.assertWire
import dev.undra.runtime.testing.directBuffer
import dev.undra.runtime.testing.fail
import dev.undra.runtime.testing.unhex
import org.junit.jupiter.api.Test
import java.util.UUID
import kotlin.time.Duration.Companion.nanoseconds

/**
 * Every vector of `contract-tests/wire-vectors.json` (via the generated `WireVectors` table), in both
 * directions and over every reader backing. A vector whose `type` is not handled here fails the suite,
 * so nothing in the shared file is ever silently skipped.
 */
class WireVectorsTest : Suite() {
    init {
        for (v in WireVectors.all) case("${v.name} [${v.type}]") { checkVector(v) }
        case("vector names are unique and the table is not empty") {
            assertTrue(WireVectors.all.isNotEmpty(), "no vectors were generated")
            assertEq(WireVectors.all.size, WireVectors.all.map { it.name }.toSet().size, "duplicate vector names")
        }
        case("the shared file's spec reference matches") {
            assertEq("docs/SPEC.md §3", WireVectors.SPEC)
        }
    }

    @Test
    fun allCases() = assertPassed()

    // ---- dispatch ------------------------------------------------------------------------------------

    private fun checkVector(v: WireVector) {
        val value = v.value
        when (v.type) {
            "bool" -> codec(v, Codecs.bool, value.asBool())
            "u8" -> codec(v, Codecs.u8, value.asInt().toUByte())
            "i8" -> codec(v, Codecs.i8, value.asInt().toByte())
            "u16" -> codec(v, Codecs.u16, value.asInt().toUShort())
            "i16" -> codec(v, Codecs.i16, value.asInt().toShort())
            "u32" -> codec(v, Codecs.u32, value.asLong().toUInt())
            "i32" -> codec(v, Codecs.i32, value.asInt())
            "u64" -> codec(v, Codecs.u64, value.asULong())
            "i64" -> codec(v, Codecs.i64, value.asLong())
            "f32" -> codec(v, Codecs.f32, value.asDouble().toFloat())
            "f64" -> codec(v, Codecs.f64, value.asDouble())
            "string" -> codec(v, Codecs.string, value.asString())
            "bytes" -> codec(v, Codecs.bytes, value.asList().map { it.asInt().toByte() }.toByteArray())
            "option<string>" -> codec(v, Codecs.option(Codecs.string), if (value is JV.Null) null else value.asString())
            "vec<i32>" -> codec(v, Codecs.vec(Codecs.i32), value.asList().map { it.asInt() })
            "vec<string>" -> codec(v, Codecs.vec(Codecs.string), value.asList().map { it.asString() })
            "map<string,i32>" -> {
                // Source order (b, a) on purpose: the encoder must sort it to (a, b).
                val expected: Map<String, Int> = LinkedHashMap<String, Int>().apply {
                    for ((k, x) in value.asObj().entries) put(k, x.asInt())
                }
                codec(v, Codecs.map(Codecs.string, Codecs.i32), expected)
            }
            "duration" -> codec(v, Codecs.duration, value.asLong().nanoseconds)
            "timestamp" -> codec(v, Codecs.timestamp, Timestamp(value.asLong()))
            "uuid" -> codec(v, Codecs.uuid, UUID.fromString(value.asString()))
            "handle" -> handle(v)
            "record Todo{id:uuid,title:string,done:bool}" -> {
                val o = value.asObj()
                codec(v, Todo, Todo(UUID.fromString(o["id"].asString()), o["title"].asString(), o["done"].asBool()))
            }
            "enum Filter{All,Active,Done}" -> {
                val expected = Filter.entries.first { it.name.equals(value.asString(), ignoreCase = true) }
                codec(v, Filter, expected)
            }
            "enum Shape{Circle{radius:f64},Rect{w:f64,h:f64}}" -> {
                val o = value.asObj()
                val expected = when (o["kind"].asString()) {
                    "circle" -> Shape.Circle(o["radius"].asDouble())
                    "rect" -> Shape.Rect(o["w"].asDouble(), o["h"].asDouble())
                    else -> fail("unknown Shape kind in vector")
                }
                codec(v, Shape, expected)
            }
            "result<i32,string>" -> {
                val o = value.asObj()
                val expected: UndraResult<Int, String> =
                    if (o.has("ok")) UndraResult.Ok(o["ok"].asInt()) else UndraResult.Err(o["err"].asString())
                codec(v, Codecs.result(Codecs.i32, Codecs.string), expected)
            }
            "envelope" -> envelope(v)
            "call payload" -> callPayload(v)
            "reply payload" -> replyPayload(v)
            "changeset payload" -> changeSetPayload(v)
            "keyed patch (item i32)" -> keyedPatch(v)
            else -> fnv(v)
        }
    }

    // ---- core check ----------------------------------------------------------------------------------

    /**
     * Encodes with [encode] and expects exactly the vector's bytes; decodes them back through every way
     * a runtime might read them (array, array window, direct buffer) and expects [expected]. For
     * self-delimiting encodings it also proves that trailing bytes are rejected and that no strict
     * prefix decodes.
     */
    private fun <T> verify(
        v: WireVector,
        expected: T,
        encode: (UndraWriter) -> Unit,
        decodeBytes: (ByteArray) -> T,
        decodeReader: ((UndraReader) -> T)?,
        selfDelimiting: Boolean = true,
        minLen: Int = 0,
    ) {
        val canonical = unhex(v.hex)
        val w = UndraWriter()
        encode(w)
        assertBytes(v.hex, w.toByteArray(), "${v.name}: encoding")
        assertEq(expected, decodeBytes(canonical), "${v.name}: decode from array")
        if (decodeReader != null) {
            val direct = UndraReader(directBuffer(canonical))
            assertEq(expected, decodeReader(direct), "${v.name}: decode from direct buffer")
            direct.finish()
            val padded = ByteArray(canonical.size + 5) { 0x5A }
            System.arraycopy(canonical, 0, padded, 3, canonical.size)
            val window = UndraReader(padded, 3, canonical.size)
            assertEq(expected, decodeReader(window), "${v.name}: decode from array window")
            window.finish()
        }
        if (selfDelimiting) {
            assertWire<WireException.TrailingBytes>("${v.name}: trailing byte") { decodeBytes(canonical + 0) }
        }
        val shortest = if (selfDelimiting) canonical.size else minLen
        for (n in 0 until shortest) {
            assertWire<WireException>("${v.name}: prefix of $n bytes") { decodeBytes(canonical.copyOf(n)) }
        }
    }

    private fun <T> codec(v: WireVector, c: UndraCodec<T>, expected: T) =
        verify(v, expected, { c.encode(it, expected) }, { c.decodeAll(it) }, { c.decode(it) })

    private fun handle(v: WireVector) {
        val raw = v.value.asLong()
        codec(v, Codecs.handle, raw)
        val h = Handle(raw)
        assertEq(1u, h.index, "index (vector note: index 1, generation 1)")
        assertEq(1u, h.generation, "generation")
        assertEq(h, Handle.make(1u, 1u))
    }

    private fun envelope(v: WireVector) {
        val o = v.value.asObj()
        val expected = Envelope(
            Envelope.Kind.fromByte(o["kind"].asInt().toUByte()),
            o["seq"].asLong().toUInt(),
            o["schema"].asULong(),
            unhex(o["payload_hex"].asString()),
        )
        verify(v, expected, { it.writeRaw(Envelope.encode(expected.kind, expected.seq, expected.schemaHash, expected.payload)) },
            { Envelope.decode(it) }, null)
        assertEq(Envelope.HEADER_LEN, unhex(v.hex).size - expected.payload.size, "header is 23 bytes")
    }

    private fun i32s(values: List<JV>): ByteArray {
        val w = UndraWriter()
        for (x in values) w.writeI32(x.asInt())
        return w.toByteArray()
    }

    private fun callPayload(v: WireVector) {
        val o = v.value.asObj()
        val target = when (val t = o["target"].asInt()) {
            1 -> Payloads.CallTarget.ObjectMethod(Handle(o["handle"].asLong()), o["method_id"].asLong().toUInt())
            else -> fail("vector uses call target $t, which this test does not know")
        }
        // The vector's args are the encoded parameters (two i32).
        val expected = Payloads.Call(target, o["call_id"].asLong().toUInt(), i32s(o["args"].asList()))
        verify(v, expected, { expected.encode(it) }, { Payloads.Call.decode(it) }, { Payloads.Call.decode(it) },
            selfDelimiting = false, minLen = 1 + 8 + 4 + 4)
    }

    private fun replyPayload(v: WireVector) {
        val o = v.value.asObj()
        val body = UndraWriter().also { it.writeI32(o["body"].asInt()) }.toByteArray() // return value i32
        val expected = Payloads.Reply(o["call_id"].asLong().toUInt(), Payloads.ReplyStatus.fromByte(o["status"].asInt().toUByte()), body)
        verify(v, expected, { expected.encode(it) }, { Payloads.Reply.decode(it) }, { Payloads.Reply.decode(it) },
            selfDelimiting = false, minLen = 5)
        assertEq(5, expected.bodyReader().readI32())
    }

    private fun changeSetPayload(v: WireVector) {
        val o = v.value.asObj()
        val entries = o["entries"].asList().map { e ->
            val eo = e.asObj()
            // The signal value is a Vec<i32>.
            val value = Codecs.vec(Codecs.i32).encodeToByteArray(eo["value"].asList().map { it.asInt() })
            Payloads.ChangeEntry(
                Handle(eo["handle"].asLong()),
                eo["signal_id"].asLong().toUInt(),
                Payloads.ChangeOp.fromByte(eo["op"].asInt().toUByte()),
                value,
            )
        }
        val expected = Payloads.ChangeSet(o["txn_id"].asULong(), entries)
        verify(v, expected, { expected.encode(it) }, { Payloads.ChangeSet.decode(it) }, { Payloads.ChangeSet.decode(it) })

        // The zero-allocation iterator must see exactly the same entries, from either backing.
        for (reader in listOf(UndraReader(unhex(v.hex)), UndraReader(directBuffer(unhex(v.hex))))) {
            val seen = ArrayList<Payloads.ChangeEntry>()
            val txn = Payloads.ChangeSet.forEachEntry(reader) { handle, signalId, op, value ->
                seen.add(Payloads.ChangeEntry(handle, signalId, op, value.readRemaining()))
            }
            assertEq(expected.txnId, txn, "forEachEntry txn id")
            assertEq(expected.entries, seen, "forEachEntry entries")
        }
    }

    private fun keyedPatch(v: WireVector) {
        val ops: List<PatchOp<Int>> = v.value.asObj()["ops"].asList().map { op ->
            val o = op.asObj()
            when (val name = o["op"].asString()) {
                "insert" -> PatchOp.Insert(o["index"].asLong().toUInt(), o["item"].asInt())
                "remove" -> PatchOp.Remove(o["index"].asLong().toUInt())
                "update" -> PatchOp.Update(o["index"].asLong().toUInt(), o["item"].asInt())
                "move" -> PatchOp.Move(o["from"].asLong().toUInt(), o["to"].asLong().toUInt())
                "clear" -> PatchOp.Clear
                else -> fail("unknown patch op '$name' in vector")
            }
        }
        verify(v, ops, { KeyedPatch.encodePatch(it, ops, Codecs.i32) }, { KeyedPatch.decodePatch(it, Codecs.i32) },
            { KeyedPatch.decodePatch(it, Codecs.i32) })
    }

    private val fnvType = Regex("""^fnv1a(32|64)\("(.*)"\)$""")

    private fun fnv(v: WireVector) {
        val m = fnvType.matchEntire(v.type) ?: fail("vector '${v.name}' has type '${v.type}', which no test handles")
        val text = m.groupValues[2]
        val canonical = unhex(v.hex)
        if (m.groupValues[1] == "32") {
            val expected = v.value.asLong().toUInt()
            assertEq(expected, Fnv.fnv1a32(text), "fnv1a32(String)")
            assertEq(expected, Fnv.fnv1a32(text.toByteArray(Charsets.UTF_8)), "fnv1a32(bytes)")
            assertBytes(v.hex, UndraWriter().also { it.writeU32(expected) }.toByteArray(), "hex is the u32, little-endian")
            assertEq(expected, UndraReader(canonical).readU32())
        } else {
            val expected = v.value.asULong()
            assertEq(expected, Fnv.fnv1a64(text), "fnv1a64(String)")
            assertEq(expected, Fnv.fnv1a64(text.toByteArray(Charsets.UTF_8)), "fnv1a64(bytes)")
            assertBytes(v.hex, UndraWriter().also { it.writeU64(expected) }.toByteArray(), "hex is the u64, little-endian")
            assertEq(expected, UndraReader(canonical).readU64())
        }
    }
}
