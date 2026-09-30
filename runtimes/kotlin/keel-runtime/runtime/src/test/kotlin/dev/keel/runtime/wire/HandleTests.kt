package dev.keel.runtime.wire

import dev.keel.runtime.testing.Suite
import dev.keel.runtime.testing.assertEq
import dev.keel.runtime.testing.assertTrue
import org.junit.jupiter.api.Test
import kotlin.random.Random

class HandleTests : Suite() {
    init {
        case("a handle is generation in the high 32 bits and index in the low 32") {
            val h = Handle.make(1u, 1u)
            assertEq(4294967297L, h.raw)
            assertEq(1u, h.index)
            assertEq(1u, h.generation)
            assertTrue(!h.isNull)
            assertEq(0x0000000200000005L, Handle.make(5u, 2u).raw)
        }

        case("zero is the null handle") {
            assertTrue(Handle(0).isNull)
            assertTrue(Handle.NULL.isNull)
            assertEq(0L, Handle.NULL.raw)
            assertEq(Handle.NULL, Handle.make(0u, 0u))
            assertEq(0u, Handle.NULL.index)
            assertEq(0u, Handle.NULL.generation)
        }

        case("a handle with generation 0 but a non-zero index is not null") {
            assertTrue(!Handle.make(3u, 0u).isNull)
            assertTrue(!Handle.make(0u, 1u).isNull)
        }

        case("extremes: the full unsigned range of both halves survives") {
            val max = Handle.make(UInt.MAX_VALUE, UInt.MAX_VALUE)
            assertEq(-1L, max.raw)
            assertEq(UInt.MAX_VALUE, max.index)
            assertEq(UInt.MAX_VALUE, max.generation)
            val highGeneration = Handle.make(7u, 0x80000000u)
            assertTrue(highGeneration.raw < 0, "generations of 2^31 and above make the signed raw value negative")
            assertEq(0x80000000u, highGeneration.generation)
            assertEq(7u, highGeneration.index)
            val highIndex = Handle.make(0x80000000u, 1u)
            assertEq(0x80000000u, highIndex.index, "a high index must not sign-extend into the generation")
            assertEq(1u, highIndex.generation)
        }

        case("make, index and generation are inverse for random values") {
            val rnd = Random(7)
            repeat(5000) {
                val index = rnd.nextInt().toUInt()
                val generation = rnd.nextInt().toUInt()
                val h = Handle.make(index, generation)
                assertEq(index, h.index)
                assertEq(generation, h.generation)
                assertEq(h, Handle(h.raw))
            }
        }

        case("handles compare by raw value and print readably") {
            assertEq(Handle(5), Handle(5))
            assertTrue(Handle(5) != Handle(6))
            assertEq("Handle(null)", Handle.NULL.toString())
            assertEq("Handle(index=1, generation=2)", Handle.make(1u, 2u).toString())
        }

        case("the handle codec round-trips the raw u64") {
            for (h in listOf(Handle.NULL, Handle.make(1u, 1u), Handle.make(UInt.MAX_VALUE, UInt.MAX_VALUE), Handle.make(0u, 0x80000000u))) {
                val bytes = Codecs.handle.encodeToByteArray(h.raw)
                assertEq(8, bytes.size)
                assertEq(h, Handle(Codecs.handle.decodeAll(bytes)))
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
