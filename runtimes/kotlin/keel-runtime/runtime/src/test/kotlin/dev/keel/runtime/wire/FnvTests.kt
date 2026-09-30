package dev.keel.runtime.wire

import dev.keel.runtime.testing.Suite
import dev.keel.runtime.testing.assertEq
import dev.keel.runtime.testing.assertThrows
import dev.keel.runtime.testing.cp
import org.junit.jupiter.api.Test
import kotlin.random.Random

class FnvTests : Suite() {
    /** Straightforward reference: hash the JDK's own UTF-8 bytes with Long arithmetic. */
    private fun reference32(s: String): UInt {
        var h = 0x811c9dc5L
        for (b in s.toByteArray(Charsets.UTF_8)) h = ((h xor (b.toLong() and 0xFF)) * 0x01000193L) and 0xFFFFFFFFL
        return h.toUInt()
    }

    private fun reference64(s: String): ULong {
        var h = 0xcbf29ce484222325uL
        for (b in s.toByteArray(Charsets.UTF_8)) h = (h xor (b.toLong() and 0xFF).toULong()) * 0x100000001b3uL
        return h
    }

    init {
        case("the vectors from contract-tests/wire-vectors.json") {
            assertEq(2353348832u, Fnv.fnv1a32("Calculator.add"))
            assertEq(6367360722358687308uL, Fnv.fnv1a64("keel"))
        }

        case("the published FNV-1a test vectors, 32-bit") {
            assertEq(0x811c9dc5u, Fnv.fnv1a32(""))
            assertEq(0xe40c292cu, Fnv.fnv1a32("a"))
            assertEq(0xbf9cf968u, Fnv.fnv1a32("foobar"))
        }

        case("the published FNV-1a test vectors, 64-bit") {
            assertEq(0xcbf29ce484222325uL, Fnv.fnv1a64(""))
            assertEq(0xaf63dc4c8601ec8cuL, Fnv.fnv1a64("a"))
            assertEq(0x85944171f73967e8uL, Fnv.fnv1a64("foobar"))
        }

        case("non-ASCII input is hashed over its UTF-8 bytes (values cross-checked against Python)") {
            val s = "h" + cp(0xE9) + "llo " + cp(0x1F30A)
            assertEq(0xcfec3d8bu, Fnv.fnv1a32(s))
            assertEq(0x7f9aefcbd1f509cbuL, Fnv.fnv1a64(s))
        }

        case("the String overloads agree with a reference over random strings, unicode included") {
            val rnd = Random(31337)
            val alphabet = intArrayOf(0x41, 0x7A, 0x30, 0x2E, 0x5F, 0x7F, 0x80, 0xE9, 0x7FF, 0x800, 0x20AC, 0xFFFD, 0x10000, 0x1F30A, 0x10FFFF, 0)
            repeat(3000) {
                val n = rnd.nextInt(0, 40)
                val s = cp(*IntArray(n) { alphabet[rnd.nextInt(alphabet.size)] })
                assertEq(reference32(s), Fnv.fnv1a32(s), "fnv1a32 of ${s.length} chars")
                assertEq(reference64(s), Fnv.fnv1a64(s), "fnv1a64 of ${s.length} chars")
            }
        }

        case("the byte-array overloads agree with the String overloads") {
            for (s in listOf("", "x", "Todos.add", "port.Http", "h" + cp(0xE9, 0x1F30A))) {
                val bytes = s.toByteArray(Charsets.UTF_8)
                assertEq(Fnv.fnv1a32(s), Fnv.fnv1a32(bytes))
                assertEq(Fnv.fnv1a64(s), Fnv.fnv1a64(bytes))
            }
        }

        case("byte ranges hash only the selected bytes") {
            val bytes = "xxfoobaryy".toByteArray()
            assertEq(0xbf9cf968u, Fnv.fnv1a32(bytes, 2, 6))
            assertEq(0x85944171f73967e8uL, Fnv.fnv1a64(bytes, 2, 6))
            assertEq(0x811c9dc5u, Fnv.fnv1a32(bytes, 10, 0))
            assertEq(0xcbf29ce484222325uL, Fnv.fnv1a64(bytes, 4, 0))
            assertThrows<IllegalArgumentException> { Fnv.fnv1a32(bytes, -1, 2) }
            assertThrows<IllegalArgumentException> { Fnv.fnv1a32(bytes, 5, 6) }
            assertThrows<IllegalArgumentException> { Fnv.fnv1a64(bytes, 0, -1) }
            assertThrows<IllegalArgumentException> { Fnv.fnv1a64(bytes, Int.MAX_VALUE, 2) }
        }

        case("a string with an unpaired surrogate has no UTF-8 form and is rejected") {
            assertThrows<IllegalArgumentException> { Fnv.fnv1a32("a\ud800") }
            assertThrows<IllegalArgumentException> { Fnv.fnv1a64("\udc00b") }
        }

        case("different names hash differently (spot check of the SPEC 1.1 id shapes)") {
            val ids = listOf("Calculator.add", "Calculator.sub", "fn.hello", "port.Http", "Http.request", "query.todos", "mutation.add_todo")
            assertEq(ids.size, ids.map { Fnv.fnv1a32(it) }.toSet().size)
            assertEq(ids.size, ids.map { Fnv.fnv1a64(it) }.toSet().size)
        }
    }

    @Test
    fun allCases() = assertPassed()
}
