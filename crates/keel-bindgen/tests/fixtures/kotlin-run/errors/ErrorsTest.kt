// Runs the generated Kotlin of the `errors` golden case: messages, renamed fields and the
// references between errors and records.

package golden.errors

import dev.keel.runtime.wire.WireException
import dev.keel.runtime.wire.decodeAll
import dev.keel.runtime.wire.encodeToByteArray

private fun ByteArray.hex(): String = joinToString("") { "%02x".format(it) }

private fun bytes(text: String): ByteArray =
    ByteArray(text.length / 2) { text.substring(it * 2, it * 2 + 2).toInt(16).toByte() }

private fun <T> expectEq(actual: T, expected: T, message: String) {
    if (actual != expected) throw AssertionError("$message: expected <$expected>, got <$actual>")
}

fun main() {
    expectEq(HttpError.Timeout.message, "timed out", "unit message")
    expectEq(HttpError.Status(503u.toUShort()).message, "status 503", "tuple message")
    expectEq(HttpError.Network("boom").message, "network error: boom", "format spec is ignored")
    expectEq(TodoError.NotFound("id-1").message, "todo \"id-1\" not found, {sorry}", "quotes and escaped braces")
    expectEq(TodoError.Range(9u, 3u).message, "9 is not below 3", "two fields")
    expectEq(TodoError.Http(HttpError.Status(404u.toUShort())).message, "status 404", "transparent")

    // Fields that clash with `Throwable` members or keywords are renamed or escaped.
    val storage = TodoError.Storage("disk", 7, "full")
    expectEq(storage.message, "storage failure 7: disk (\$full)", "dollar sign before a placeholder")
    expectEq(storage.message_, "full", "renamed field")
    expectEq(TodoError.Detail("x").default, "x", "keyword field")

    expectEq(
        TodoError.encodeToByteArray(storage).hex(),
        "0300" + "04000000" + "6469736b" + "07000000" + "04000000" + "66756c6c",
        "encoding",
    )
    expectEq(TodoError.decodeAll(TodoError.encodeToByteArray(storage)), storage, "round trip")
    expectEq(HttpError.encodeToByteArray(HttpError.Cancelled).hex(), "0300", "unit variant")
    try {
        HttpError.decodeAll(bytes("0900"))
        throw AssertionError("an unknown tag must fail")
    } catch (e: WireException.InvalidTag) {
        expectEq(e.tag, 9u, "the offending tag")
    }

    val payload = Payload("outer", HttpError.Status(500u.toUShort()), listOf(Boxed.Holding(Payload("inner", null, emptyList()))))
    val encoded = Payload.encodeToByteArray(payload)
    expectEq(
        encoded.hex(),
        "05000000" + "6f75746572" + "01" + "0100" + "f401" + "01000000" + "0000" + "05000000" + "696e6e6572" + "00" + "00000000",
        "record holding errors holding a record",
    )
    expectEq(Payload.decodeAll(encoded), payload, "round trip of the cycle")
    println("ok")
}
