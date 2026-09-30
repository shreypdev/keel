// Runs the generated Kotlin of the `enums` golden case, including variants named like standard
// types.

package golden.enums

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
    // Unit enums use their wire indexes, which need not be dense.
    expectEq(NetKind.encodeToByteArray(NetKind.DEFAULT).hex(), "0300", "NetKind.DEFAULT")
    expectEq(NetKind.decodeAll(bytes("0200")), NetKind.NONE, "NetKind.NONE")
    expectEq(Sparse.encodeToByteArray(Sparse.SECOND).hex(), "0700", "Sparse.SECOND")
    expectEq(Sparse.decodeAll(bytes("0100")), Sparse.FIRST, "Sparse.FIRST")
    for (bad in listOf("0000", "0200")) {
        try {
            Sparse.decodeAll(bytes(bad))
            throw AssertionError("tag $bad must fail")
        } catch (e: WireException.InvalidTag) {
            expectEq(e.type, "Sparse", "the type")
        }
    }

    val shapes = listOf<Pair<Shape, String>>(
        Shape.Circle(1.5) to "0000" + "000000000000f83f",
        Shape.Rect(1.5, 1.5) to "0100" + "000000000000f83f" + "000000000000f83f",
        Shape.Labelled("a", -2, Filter.DONE) to "0200" + "01000000" + "61" + "feffffff" + "01" + "0200",
        Shape.Labelled("", 0, null) to "0200" + "00000000" + "00000000" + "00",
        Shape.Single("s") to "0300" + "01000000" + "73",
        Shape.Empty to "0400",
    )
    for ((shape, text) in shapes) {
        expectEq(Shape.encodeToByteArray(shape).hex(), text, "encoding of $shape")
        expectEq(Shape.decodeAll(bytes(text)), shape, "decoding of $shape")
    }

    // Variants named like `String`, `Int`, `List` and like other types of the schema.
    val value: Value = Value.List(
        listOf(
            Value.Int(5),
            Value.String("x"),
            Value.Bool(true),
            Value.Shape(Shape.Empty),
            Value.Filter(Filter.DONE),
            Value.List(emptyList()),
            Value.Null,
        ),
    )
    val text = "0300" + "07000000" + "0100" + "0500000000000000" + "0000" + "01000000" + "78" + "0200" + "01" +
        "0400" + "0400" + "0500" + "0200" + "0300" + "00000000" + "0600"
    expectEq(Value.encodeToByteArray(value).hex(), text, "recursive enum encoding")
    expectEq(Value.decodeAll(bytes(text)), value, "recursive enum decoding")
    println("ok")
}
