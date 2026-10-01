package dev.undra.runtime.wire

import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertBytes
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.testing.assertWire
import dev.undra.runtime.testing.bytesOf
import dev.undra.runtime.testing.directBuffer
import dev.undra.runtime.testing.unhex
import org.junit.jupiter.api.Test
import java.util.UUID
import kotlin.random.Random

class KeyedPatchTests : Suite() {
    private val ints = Codecs.i32

    private fun applyOps(list: List<Int>, vararg ops: PatchOp<Int>): List<Int> = KeyedPatch.applyPatch(list, ops.toList())

    private fun insert(index: Int, item: Int): PatchOp<Int> = PatchOp.Insert(index.toUInt(), item)

    private fun remove(index: Int): PatchOp<Int> = PatchOp.Remove(index.toUInt())

    private fun update(index: Int, item: Int): PatchOp<Int> = PatchOp.Update(index.toUInt(), item)

    private fun move(from: Int, to: Int): PatchOp<Int> = PatchOp.Move(from.toUInt(), to.toUInt())

    init {
        case("each op has the wire layout of the spec") {
            assertBytes("01000000" + "00" + "00000000" + "05000000", KeyedPatch.encodePatch(listOf(insert(0, 5)), ints))
            assertBytes("01000000" + "01" + "01000000", KeyedPatch.encodePatch(listOf(remove(1)), ints))
            assertBytes("01000000" + "02" + "02000000" + "ffffffff", KeyedPatch.encodePatch(listOf(update(2, -1)), ints))
            assertBytes("01000000" + "03" + "00000000" + "01000000", KeyedPatch.encodePatch(listOf(move(0, 1)), ints))
            assertBytes("01000000" + "04", KeyedPatch.encodePatch(listOf(PatchOp.Clear), ints))
            assertBytes("00000000", KeyedPatch.encodePatch(emptyList(), ints))
        }

        case("the shared vector decodes to its four ops") {
            val bytes = unhex("04000000000000000005000000010100000003000000000100000004")
            assertEq(listOf(insert(0, 5), remove(1), move(0, 1), PatchOp.Clear), KeyedPatch.decodePatch(bytes, ints))
        }

        case("patches round-trip through every backing") {
            val ops = listOf(insert(0, 1), update(0, 2), move(0, 0), remove(0), PatchOp.Clear, insert(UInt.MAX_VALUE.toInt(), Int.MIN_VALUE))
            val bytes = KeyedPatch.encodePatch(ops, ints)
            assertEq(ops, KeyedPatch.decodePatch(bytes, ints))
            assertEq(ops, KeyedPatch.decodePatch(UndraReader(directBuffer(bytes)), ints))
            val padded = ByteArray(bytes.size + 6) { 0x11 }
            System.arraycopy(bytes, 0, padded, 3, bytes.size)
            assertEq(ops, KeyedPatch.decodePatch(UndraReader(padded, 3, bytes.size), ints))
        }

        case("indices use the full unsigned range on the wire") {
            val ops = listOf<PatchOp<Int>>(PatchOp.Move(UInt.MAX_VALUE, 0x80000000u), PatchOp.Remove(0x80000000u))
            assertEq(ops, KeyedPatch.decodePatch(KeyedPatch.encodePatch(ops, ints), ints))
        }

        case("items are decoded with the item codec, records included") {
            val todo = Todo(UUID(1, 2), "Milk", true)
            val ops = listOf(PatchOp.Insert(0u, todo), PatchOp.Update(0u, todo.copy(done = false)))
            assertEq(ops, KeyedPatch.decodePatch(KeyedPatch.encodePatch(ops, Todo), Todo))
            val strings = listOf(PatchOp.Insert(3u, "héllo"), PatchOp.Remove(1u))
            assertEq(strings, KeyedPatch.decodePatch(KeyedPatch.encodePatch(strings, Codecs.string), Codecs.string))
        }

        case("decodePatch(reader) stops after the patch and decodePatch(bytes) demands the end") {
            val w = UndraWriter()
            KeyedPatch.encodePatch(w, listOf(remove(4)), ints)
            w.writeU8(0x7Fu)
            val r = UndraReader(w.toByteArray())
            assertEq(listOf(remove(4)), KeyedPatch.decodePatch(r, ints))
            assertEq(1, r.remaining)
            assertEq(1, assertWire<WireException.TrailingBytes> { KeyedPatch.decodePatch(w.toByteArray(), ints) }.count)
        }

        case("insert places the item at the given index, up to and including the end") {
            assertEq(listOf(9, 1, 2, 3), applyOps(listOf(1, 2, 3), insert(0, 9)))
            assertEq(listOf(1, 9, 2, 3), applyOps(listOf(1, 2, 3), insert(1, 9)))
            assertEq(listOf(1, 2, 3, 9), applyOps(listOf(1, 2, 3), insert(3, 9)))
            assertEq(listOf(9), applyOps(emptyList(), insert(0, 9)))
        }

        case("remove and update address existing items") {
            assertEq(listOf(2, 3), applyOps(listOf(1, 2, 3), remove(0)))
            assertEq(listOf(1, 3), applyOps(listOf(1, 2, 3), remove(1)))
            assertEq(listOf(1, 2), applyOps(listOf(1, 2, 3), remove(2)))
            assertEq(emptyList(), applyOps(listOf(1), remove(0)))
            assertEq(listOf(7, 2, 3), applyOps(listOf(1, 2, 3), update(0, 7)))
            assertEq(listOf(1, 2, 7), applyOps(listOf(1, 2, 3), update(2, 7)))
        }

        case("move removes the item and re-inserts it so it ends up at the target index") {
            val abcd = listOf(1, 2, 3, 4)
            assertEq(listOf(2, 3, 1, 4), applyOps(abcd, move(0, 2)))
            assertEq(listOf(2, 3, 4, 1), applyOps(abcd, move(0, 3)))
            assertEq(listOf(4, 1, 2, 3), applyOps(abcd, move(3, 0)))
            assertEq(listOf(1, 3, 2, 4), applyOps(abcd, move(2, 1)))
            assertEq(abcd, applyOps(abcd, move(2, 2)), "moving onto itself changes nothing")
            assertEq(listOf(1), applyOps(listOf(1), move(0, 0)))
        }

        case("clear empties the list, and later ops see the empty list") {
            assertEq(emptyList(), applyOps(listOf(1, 2, 3), PatchOp.Clear))
            assertEq(emptyList(), applyOps(emptyList(), PatchOp.Clear))
            assertEq(listOf(5), applyOps(listOf(1, 2, 3), PatchOp.Clear, insert(0, 5)))
        }

        case("indices refer to the list after the previous op, and the shared vector's ops apply in order") {
            // [10, 20] -> insert(0,5) [5,10,20] -> remove(1) [5,20] -> move(0,1) [20,5] -> clear []
            val list = listOf(10, 20)
            assertEq(listOf(5, 10, 20), applyOps(list, insert(0, 5)))
            assertEq(listOf(5, 20), applyOps(list, insert(0, 5), remove(1)))
            assertEq(listOf(20, 5), applyOps(list, insert(0, 5), remove(1), move(0, 1)))
            assertEq(emptyList(), applyOps(list, insert(0, 5), remove(1), move(0, 1), PatchOp.Clear))
            assertEq(listOf(1, 2, 3), applyOps(emptyList(), insert(0, 1), insert(1, 3), insert(1, 2)))
        }

        case("applyPatch never modifies its input and always returns a new list") {
            val original = mutableListOf(1, 2, 3)
            val out = KeyedPatch.applyPatch(original, listOf(insert(0, 0), remove(3), update(1, 99), PatchOp.Clear))
            assertEq(listOf(1, 2, 3), original.toList(), "input must be untouched")
            assertEq(emptyList(), out)
            val same = KeyedPatch.applyPatch(original, emptyList())
            assertEq(original.toList(), same)
            assertTrue(same !== original, "an empty patch still returns a new list")
            original.add(4)
            assertEq(listOf(1, 2, 3), same, "the result must not be a view of the input")
            val fromImmutable = KeyedPatch.applyPatch(listOf(1, 2), listOf(insert(2, 3)))
            assertEq(listOf(1, 2, 3), fromImmutable)
        }

        case("out-of-range indices raise PatchOutOfBounds with the op, index and size") {
            val cases = listOf(
                Triple(listOf(1, 2, 3), insert(4, 0), "Insert"),
                Triple(listOf(1, 2, 3), remove(3), "Remove"),
                Triple(listOf(1, 2, 3), update(3, 0), "Update"),
                Triple(listOf(1, 2, 3), move(3, 0), "Move"),
                Triple(emptyList<Int>(), remove(0), "Remove"),
                Triple(emptyList<Int>(), update(0, 1), "Update"),
                Triple(emptyList<Int>(), move(0, 0), "Move"),
                Triple(listOf(1), PatchOp.Remove(UInt.MAX_VALUE), "Remove"),
                Triple(listOf(1), PatchOp.Insert(0x80000000u, 1), "Insert"),
            )
            for ((list, op, name) in cases) {
                val e = assertWire<WireException.PatchOutOfBounds>("$name on ${list.size} items") { KeyedPatch.applyPatch(list, listOf(op)) }
                assertEq(name, e.op)
                assertEq(0, e.opIndex)
                assertEq(list.size, e.size)
            }
            assertEq(4u, assertWire<WireException.PatchOutOfBounds> { applyOps(listOf(1, 2, 3), insert(4, 0)) }.index)
        }

        case("a move reports whichever of its two indices is out of range") {
            assertEq(9u, assertWire<WireException.PatchOutOfBounds> { applyOps(listOf(1, 2, 3), move(9, 0)) }.index)
            assertEq(3u, assertWire<WireException.PatchOutOfBounds> { applyOps(listOf(1, 2, 3), move(0, 3)) }.index, "to must be below the size")
        }

        case("a failure mid-patch names the failing op and the size at that moment") {
            // [1,2,3] -> remove(0) [2,3] -> remove(0) [3] -> remove(1) fails: size is 1.
            val e = assertWire<WireException.PatchOutOfBounds> { applyOps(listOf(1, 2, 3), remove(0), remove(0), remove(1)) }
            assertEq(2, e.opIndex)
            assertEq(1, e.size)
            assertEq(1u, e.index)
            assertTrue(e.message!!.contains("op #2") && e.message!!.contains("Remove"), "message is descriptive: ${e.message}")
            val cleared = assertWire<WireException.PatchOutOfBounds> { applyOps(listOf(1, 2, 3), PatchOp.Clear, update(0, 1)) }
            assertEq(0, cleared.size)
            assertEq(1, cleared.opIndex)
        }

        case("decoding rejects unknown op tags and reports their offset") {
            for (tag in listOf(5, 6, 100, 255)) {
                val e = assertWire<WireException.InvalidTag>("tag $tag") { KeyedPatch.decodePatch(bytesOf(1, 0, 0, 0, tag), ints) }
                assertEq(tag.toUInt(), e.tag)
                assertEq(4, e.at)
                assertEq("PatchOp", e.type)
            }
            val second = assertWire<WireException.InvalidTag> { KeyedPatch.decodePatch(bytesOf(2, 0, 0, 0, 4, 9), ints) }
            assertEq(5, second.at)
        }

        case("decoding rejects impossible counts and truncated ops") {
            assertEq(UInt.MAX_VALUE, assertWire<WireException.LengthTooLarge> { KeyedPatch.decodePatch(bytesOf(0xFF, 0xFF, 0xFF, 0xFF), ints) }.len)
            assertEq(2u, assertWire<WireException.LengthTooLarge> { KeyedPatch.decodePatch(bytesOf(2, 0, 0, 0, 4), ints) }.len)
            assertEq(0, KeyedPatch.decodePatch(bytesOf(0, 0, 0, 0), ints).size)
            val whole = KeyedPatch.encodePatch(listOf(insert(0, 5), remove(1), update(2, 3), move(0, 1), PatchOp.Clear), ints)
            for (n in 0 until whole.size) {
                assertWire<WireException>("truncated to $n bytes") { KeyedPatch.decodePatch(whole.copyOf(n), ints) }
            }
        }

        case("random patches match an independent model, through encode, decode and apply") {
            val rnd = Random(20240930)
            repeat(400) {
                val start = List(rnd.nextInt(0, 8)) { rnd.nextInt(-100, 100) }
                val model = start.toMutableList()
                val ops = ArrayList<PatchOp<Int>>()
                repeat(rnd.nextInt(0, 25)) {
                    when (rnd.nextInt(5)) {
                        0 -> {
                            val i = rnd.nextInt(0, model.size + 1)
                            val x = rnd.nextInt()
                            ops.add(insert(i, x))
                            // model: rebuild around the index rather than reuse the implementation's insert
                            val rebuilt = model.subList(0, i) + listOf(x) + model.subList(i, model.size)
                            model.clear(); model.addAll(rebuilt)
                        }
                        1 -> if (model.isNotEmpty()) {
                            val i = rnd.nextInt(model.size)
                            ops.add(remove(i))
                            val rebuilt = model.subList(0, i) + model.subList(i + 1, model.size)
                            model.clear(); model.addAll(rebuilt)
                        }
                        2 -> if (model.isNotEmpty()) {
                            val i = rnd.nextInt(model.size)
                            val x = rnd.nextInt()
                            ops.add(update(i, x))
                            model[i] = x
                        }
                        3 -> if (model.isNotEmpty()) {
                            val from = rnd.nextInt(model.size)
                            val to = rnd.nextInt(model.size)
                            ops.add(move(from, to))
                            val item = model[from]
                            val without = model.subList(0, from) + model.subList(from + 1, model.size)
                            val rebuilt = without.subList(0, to) + listOf(item) + without.subList(to, without.size)
                            model.clear(); model.addAll(rebuilt)
                        }
                        else -> if (rnd.nextInt(6) == 0) {
                            ops.add(PatchOp.Clear)
                            model.clear()
                        }
                    }
                }
                val decoded = KeyedPatch.decodePatch(KeyedPatch.encodePatch(ops, ints), ints)
                assertEq<List<PatchOp<Int>>>(ops, decoded)
                assertEq(model.toList(), KeyedPatch.applyPatch(start, decoded))
            }
        }

        case("PatchOp values compare by content") {
            assertEq(insert(1, 2), insert(1, 2))
            assertTrue(insert(1, 2) != insert(1, 3))
            assertTrue(insert(1, 2) != update(1, 2))
            assertEq(PatchOp.Clear, PatchOp.Clear)
            assertEq<PatchOp<Int>>(move(1, 2), move(1, 2))
        }
    }

    @Test
    fun allCases() = assertPassed()
}
