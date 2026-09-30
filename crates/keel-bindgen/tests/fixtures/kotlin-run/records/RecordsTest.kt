// Runs the generated Kotlin of the `records` golden case: every number type, containers, keywords,
// an empty record and a recursive one.

package golden.records

import dev.keel.runtime.wire.WireException
import dev.keel.runtime.wire.decodeAll
import dev.keel.runtime.wire.encodeToByteArray
import java.util.UUID
import kotlin.time.Duration.Companion.milliseconds

private fun ByteArray.hex(): String = joinToString("") { "%02x".format(it) }

private fun bytes(text: String): ByteArray =
    ByteArray(text.length / 2) { text.substring(it * 2, it * 2 + 2).toInt(16).toByte() }

private fun <T> expectEq(actual: T, expected: T, message: String) {
    if (actual != expected) throw AssertionError("$message: expected <$expected>, got <$actual>")
}

fun main() {
    val numbers = Numbers(
        a = -1, b = -2, c = -3, d = -4L,
        e = 255u.toUByte(), f = 65535u.toUShort(), g = 4294967295u, h = ULong.MAX_VALUE,
        i = 1.5f, j = 2.5, elapsed = 1500.milliseconds,
    )
    val text = "ff" + "feff" + "fdffffff" + "fcffffffffffffff" + "ff" + "ffff" + "ffffffff" + "ffffffffffffffff" +
        "0000c03f" + "0000000000000440" + "002f685900000000"
    expectEq(Numbers.encodeToByteArray(numbers).hex(), text, "Numbers encoding")
    expectEq(Numbers.decodeAll(bytes(text)), numbers, "Numbers decoding")
    try {
        Numbers.decodeAll(bytes(text.replace("002f685900000000", "ffffffffffffffff")))
        throw AssertionError("a negative duration must fail")
    } catch (e: WireException.NegativeDuration) {
        // expected
    }
    try {
        Numbers.decodeAll(bytes(text.dropLast(2)))
        throw AssertionError("truncated input must fail")
    } catch (e: WireException.UnexpectedEof) {
        // expected
    }

    val todo = Todo(UUID(2L, 2L), "t", priority = Priority.LOW, due = null)
    expectEq(todo.done, false, "default field")
    expectEq(todo.tags, emptyList(), "default list")
    val containers = Containers(
        lines = listOf("a", null, ""),
        scores = mapOf("k" to 7, "j" to -1),
        byId = mapOf(UUID(0L, 1L) to listOf(todo)),
        blob = byteArrayOf(1, 2, 3),
        maybeBlob = null,
        matrix = listOf(listOf(1.5), emptyList()),
        counts = mapOf(1u to 2uL),
    )
    val back = Containers.decodeAll(Containers.encodeToByteArray(containers))
    expectEq(back, containers, "Containers round trip (byte arrays compare by content)")
    expectEq(back.hashCode(), containers.hashCode(), "hash")
    expectEq(containers.copy(maybeBlob = byteArrayOf(1)) == containers, false, "differing optional bytes")

    val keywords = Keywords(default = "d", `in` = 1, `object` = true, delete = false, new = true, className = "c")
    expectEq(Keywords.decodeAll(Keywords.encodeToByteArray(keywords)), keywords, "keyword fields")

    expectEq(Empty.encodeToByteArray(Empty()).size, 0, "empty record")
    expectEq(Empty.decodeAll(ByteArray(0)), Empty(), "empty record equality")
    expectEq(Empty().toString(), "Empty()", "empty record string")

    val page = Page(emptyList(), "n", listOf(Page(emptyList(), null, emptyList())))
    expectEq(Page.decodeAll(Page.encodeToByteArray(page)), page, "recursive record")
    println("ok")
}
