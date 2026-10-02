package dev.undra.runtime.wire

import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertBytes
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.testing.assertWire
import dev.undra.runtime.testing.bytesOf
import dev.undra.runtime.testing.directBuffer
import dev.undra.runtime.testing.fail
import dev.undra.runtime.testing.unhex
import org.junit.jupiter.api.Test
import java.lang.management.ManagementFactory

/** The zero-allocation change-set iterator, [Payloads.ChangeSet.forEachEntry]. */
class ChangeSetTests : Suite() {
    private class Seen(val handle: Handle, val signalId: UInt, val op: Payloads.ChangeOp, val value: ByteArray)

    private fun entry(index: Int, op: Payloads.ChangeOp = Payloads.ChangeOp.FULL, value: ByteArray = bytesOf(index, 0, 0, 0)) =
        Payloads.ChangeEntry(Handle.make(index.toUInt() + 1u, 1u), index.toUInt(), op, value)

    private fun changeSet(txn: ULong, entries: List<Payloads.ChangeEntry>): ByteArray = Payloads.ChangeSet(txn, entries).toByteArray()

    private fun collect(reader: UndraReader): Pair<ULong, List<Seen>> {
        val seen = ArrayList<Seen>()
        val txn = Payloads.ChangeSet.forEachEntry(reader) { handle, signalId, op, value ->
            seen.add(Seen(handle, signalId, op, value.readRemaining()))
        }
        return txn to seen
    }

    /** Bytes the current thread allocated while running [block], or -1 if the JVM cannot say. */
    private fun allocatedBytes(block: () -> Unit): Long {
        return try {
            val bean = ManagementFactory.getThreadMXBean() as? com.sun.management.ThreadMXBean ?: return -1
            if (!bean.isThreadAllocatedMemorySupported) return -1
            bean.isThreadAllocatedMemoryEnabled = true
            val before = bean.currentThreadAllocatedBytes
            block()
            bean.currentThreadAllocatedBytes - before
        } catch (e: NoSuchMethodError) {
            -1 // JDK older than 14 has no getCurrentThreadAllocatedBytes
        } catch (e: UnsupportedOperationException) {
            -1
        }
    }

    private fun sumEntries(bytes: ByteArray): Long {
        var sum = 0L
        Payloads.ChangeSet.forEachEntry(bytes) { handle, signalId, op, value ->
            sum += handle.index.toLong() + signalId.toLong() + op.ordinal + value.readU32().toLong()
        }
        return sum
    }

    init {
        case("entries arrive in order with handle, signal id, op and a reader over exactly the value") {
            val entries = listOf(entry(0), entry(1, Payloads.ChangeOp.PATCH, bytesOf(1, 2, 3)), entry(2, Payloads.ChangeOp.INVALIDATED, ByteArray(0)))
            val bytes = changeSet(77u, entries)
            val (txn, seen) = collect(UndraReader(bytes))
            assertEq(77uL, txn)
            assertEq(3, seen.size)
            for (i in 0 until 3) {
                assertEq(entries[i].handle, seen[i].handle)
                assertEq(entries[i].signalId, seen[i].signalId)
                assertEq(entries[i].op, seen[i].op)
                assertBytes(entries[i].value.joinToString("") { "%02x".format(it) }, seen[i].value)
            }
        }

        case("the value reader is bounded by the entry's length and starts at offset 0") {
            val bytes = changeSet(1u, listOf(entry(0, value = bytesOf(1, 2)), entry(1, value = bytesOf(3, 4, 5, 6))))
            var calls = 0
            Payloads.ChangeSet.forEachEntry(bytes) { _, _, _, value ->
                calls++
                assertEq(0, value.position)
                assertEq(if (calls == 1) 2 else 4, value.remaining)
                if (calls == 1) {
                    assertEq(0x0201.toShort(), value.readI16())
                    assertWire<WireException.UnexpectedEof>("reading into the next entry must be impossible") { value.readU8() }
                }
            }
            assertEq(2, calls)
        }

        case("a block need not consume the value, and unread values are skipped by their length") {
            val entries = List(5) { entry(it, value = ByteArray(it * 3) { j -> j.toByte() }) }
            val bytes = changeSet(9u, entries)
            val ids = ArrayList<UInt>()
            Payloads.ChangeSet.forEachEntry(bytes) { _, signalId, _, _ -> ids.add(signalId) } // never touches the value
            assertEq(listOf(0u, 1u, 2u, 3u, 4u), ids)
            val partial = ArrayList<Int>()
            Payloads.ChangeSet.forEachEntry(bytes) { _, _, _, value -> if (value.remaining > 0) partial.add(value.readU8().toInt()) }
            assertEq(listOf(0, 0, 0, 0), partial, "first byte only; the rest of each value is skipped")
        }

        case("a bytes-based and a reader-based call see the same entries, from an array window and a direct buffer") {
            val bytes = changeSet(5u, List(6) { entry(it) })
            val (txnA, a) = collect(UndraReader(bytes))
            val padded = ByteArray(bytes.size + 9) { 0x55 }
            System.arraycopy(bytes, 0, padded, 4, bytes.size)
            val (txnB, b) = collect(UndraReader(padded, 4, bytes.size))
            val (txnC, c) = collect(UndraReader(directBuffer(bytes)))
            val viaBytes = ArrayList<UInt>()
            val txnD = Payloads.ChangeSet.forEachEntry(bytes) { _, signalId, _, _ -> viaBytes.add(signalId) }
            assertEq(5uL, txnA); assertEq(5uL, txnB); assertEq(5uL, txnC); assertEq(5uL, txnD)
            assertEq(a.map { it.signalId }, b.map { it.signalId })
            assertEq(a.map { it.signalId }, c.map { it.signalId })
            assertEq(a.map { it.signalId }, viaBytes)
            for (i in a.indices) {
                assertTrue(a[i].value.contentEquals(b[i].value) && a[i].value.contentEquals(c[i].value), "values of entry $i differ across backings")
            }
        }

        case("an empty change-set yields the txn id and no calls") {
            var calls = 0
            val txn = Payloads.ChangeSet.forEachEntry(changeSet(123u, emptyList())) { _, _, _, _ -> calls++ }
            assertEq(123uL, txn)
            assertEq(0, calls)
        }

        case("values decode with real codecs: a full Vec<i32> and a keyed patch") {
            val full = Codecs.vec(Codecs.i32).encodeToByteArray(listOf(1, 2, 3))
            val patch = KeyedPatch.encodePatch(listOf(PatchOp.Insert(1u, 9), PatchOp.Remove(0u)), Codecs.i32)
            val bytes = changeSet(
                1u,
                listOf(
                    Payloads.ChangeEntry(Handle(1), 0u, Payloads.ChangeOp.FULL, full),
                    Payloads.ChangeEntry(Handle(1), 0u, Payloads.ChangeOp.PATCH, patch),
                ),
            )
            var list: List<Int> = emptyList()
            Payloads.ChangeSet.forEachEntry(bytes) { _, _, op, value ->
                when (op) {
                    Payloads.ChangeOp.FULL -> list = Codecs.vec(Codecs.i32).decode(value).also { value.finish() }
                    Payloads.ChangeOp.PATCH -> {
                        val ops = KeyedPatch.decodePatch(value, Codecs.i32)
                        value.finish()
                        list = KeyedPatch.applyPatch(list, ops)
                    }
                    Payloads.ChangeOp.INVALIDATED -> fail("not expected")
                }
            }
            assertEq(listOf(9, 2, 3), list)
        }

        case("an unknown op stops the walk after the entries before it were delivered") {
            val good = changeSet(1u, listOf(entry(0), entry(1)))
            val bytes = good.copyOf()
            // entry 1's op byte: 8 txn + 4 count + (17 + 4 value) + handle 8 + signal 4
            bytes[8 + 4 + 21 + 8 + 4] = 9
            val ids = ArrayList<UInt>()
            val e = assertWire<WireException.InvalidTag> { Payloads.ChangeSet.forEachEntry(bytes) { _, signalId, _, _ -> ids.add(signalId) } }
            assertEq(listOf(0u), ids)
            assertEq(9u, e.tag)
            assertEq("ChangeOp", e.type)
            assertEq(8 + 4 + 21 + 8 + 4, e.at)
        }

        case("an oversized count, entry length or truncation is a WireException") {
            val head = unhex("2a00000000000000")
            assertWire<WireException.LengthTooLarge> { Payloads.ChangeSet.forEachEntry(head + unhex("ffffffff")) { _, _, _, _ -> } }
            val badLen = unhex("2a00000000000000" + "01000000" + "0100000001000000" + "00000000" + "00" + "05000000" + "0102")
            assertEq(5u, assertWire<WireException.LengthTooLarge> { Payloads.ChangeSet.forEachEntry(badLen) { _, _, _, _ -> } }.len)
            val whole = changeSet(1u, List(3) { entry(it) })
            for (n in 0 until whole.size) {
                assertWire<WireException>("truncated to $n bytes") { Payloads.ChangeSet.forEachEntry(whole.copyOf(n)) { _, _, _, _ -> } }
            }
        }

        case("trailing bytes are reported after every entry has been delivered") {
            val bytes = changeSet(1u, listOf(entry(0), entry(1))) + bytesOf(7, 7, 7)
            var calls = 0
            val e = assertWire<WireException.TrailingBytes> { Payloads.ChangeSet.forEachEntry(bytes) { _, _, _, _ -> calls++ } }
            assertEq(3, e.count)
            assertEq(2, calls)
        }

        case("an exception thrown by the block propagates unchanged and stops the walk") {
            class Boom : RuntimeException("boom")
            val bytes = changeSet(1u, List(4) { entry(it) })
            var calls = 0
            assertThrows<Boom> { Payloads.ChangeSet.forEachEntry(bytes) { _, _, _, _ -> if (++calls == 2) throw Boom() } }
            assertEq(2, calls)
        }

        case("the reader passed in ends up exhausted, so a following payload can be read from it") {
            val w = UndraWriter()
            w.writeU8(0xEEu)
            Payloads.ChangeSet(3u, listOf(entry(0))).encode(w)
            val r = UndraReader(w.toByteArray())
            r.readU8()
            assertEq(3uL, Payloads.ChangeSet.forEachEntry(r) { _, _, _, _ -> })
            assertEq(0, r.remaining)
        }

        case("materializing and iterating agree") {
            val entries = List(50) { entry(it, Payloads.ChangeOp.entries[it % 3], ByteArray(it % 7) { j -> (it + j).toByte() }) }
            val bytes = changeSet(99u, entries)
            val decoded = Payloads.ChangeSet.decode(bytes)
            val (txn, seen) = collect(UndraReader(bytes))
            assertEq(decoded.txnId, txn)
            assertEq(decoded.entries.size, seen.size)
            for (i in seen.indices) {
                assertEq(decoded.entries[i], Payloads.ChangeEntry(seen[i].handle, seen[i].signalId, seen[i].op, seen[i].value))
            }
        }

        case("iteration allocates nothing per entry (measured with the JVM's per-thread allocation counter)") {
            fun build(n: Int) = changeSet(1u, List(n) { entry(it) })
            val small = build(10)
            val large = build(20_000)
            sumEntries(small) // load classes and settle lazy initialization before measuring
            sumEntries(large)
            val smallBytes = allocatedBytes { sumEntries(small) }
            val largeBytes = allocatedBytes { sumEntries(large) }
            if (smallBytes < 0 || largeBytes < 0) {
                println("  note: per-thread allocation counter unavailable on this JVM; allocation assertion skipped")
            } else {
                val extraEntries = 20_000 - 10
                val extraBytes = largeBytes - smallBytes
                // The claim is "nothing per entry", so the bound is per entry: less than one byte per extra entry.
                // An allocation per entry costs at least 16 bytes (an object header), 320 KB over these entries,
                // and fails it 16 times over; what this thread allocates once in a while whatever the entry count
                // (a JIT compilation finishing, a TLAB refill: 2,120 bytes once on a CI runner, against a fixed
                // 2,048 that used to be the bound) is a few KB, which cannot reach 19,990.
                assertTrue(
                    extraBytes < extraEntries,
                    "iterating $extraEntries more entries allocated $extraBytes more bytes (small=$smallBytes, large=$largeBytes); expected less than one byte per entry",
                )
                // The counter must be able to see allocation: materializing allocates far more per entry.
                val materialized = allocatedBytes { Payloads.ChangeSet.decode(large) }
                assertTrue(materialized > 20_000L * 16, "sanity check of the measurement failed: decode allocated only $materialized bytes")
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
