package dev.undra.contract

import dev.undra.playground.core.Composite
import dev.undra.playground.core.Figure
import dev.undra.playground.core.LabError
import dev.undra.playground.core.area
import dev.undra.playground.core.echoComposite
import dev.undra.playground.core.echoFigure
import dev.undra.playground.core.parseCount

/** S02: records, data enums and typed errors, in and out of the core. */
fun s02RecordsEnumsErrors(w: World) {
    // 1. Every variant of Figure comes back equal and of the same variant.
    val figures = listOf(Figure.Circle(2.5), Figure.Rect(2.0, 3.5), Figure.Label(""), Figure.Label("héllo"), Figure.Empty)
    for (figure in figures) {
        val back = echoFigure(figure)
        expectEq("echo_figure($figure)", figure, back)
        expectEq("the variant of $figure", figure::class, back::class)
    }

    // 2. A record that nests lists, options, maps with string and integer keys, and enums with data.
    val everything = Composite(
        name = "everything",
        tags = listOf("a", "", "ü"),
        figure = Figure.Rect(2.0, 3.5),
        history = listOf(Figure.Circle(1.0), Figure.Label("x"), Figure.Empty),
        scores = mapOf("high" to 99, "low" to -3),
        names = mapOf(7u to "seven", 1u to "one"),
        limit = 7u,
    )
    expectEq("echo_composite(everything)", everything, echoComposite(everything))
    val nothing = Composite("everything", emptyList(), null, emptyList(), emptyMap(), emptyMap(), null)
    expectEq("echo_composite(nothing)", nothing, echoComposite(nothing))

    // 3. Results.
    expectEq("area(Rect(2, 4))", 8.0, area(Figure.Rect(2.0, 4.0)))
    val circle = area(Figure.Circle(1.0))
    check(Math.abs(circle - Math.PI) < 1e-12) { "area(Circle(1)) is $circle, not pi" }

    // 4. Errors arrive as values of the typed error, never as a generic failure.
    val rejected = expectFails<LabError.Rejected>("area(Label(\"hat\"))") { area(Figure.Label("hat")) }
    expectEq("the code of the rejection", 1, rejected.code)
    expectEq("the reason of the rejection", "`hat` has no area", rejected.reason)
    val empty = expectFails<LabError>("area(Empty)") { area(Figure.Empty) }
    expectEq("area(Empty) fails with", LabError.Empty, empty)
    expectEq("parse_count(\"\")", LabError.Empty, expectFails<LabError>("parse_count(\"\")") { parseCount("") })
    val tooLong = expectFails<LabError.TooLong>("parse_count of ten digits") { parseCount("1234567890") }
    expectEq("the limit of TooLong", 9u, tooLong.max)
    val notANumber = expectFails<LabError.NotANumber>("parse_count(\"4x2\")") { parseCount("4x2") }
    expectEq("the text of NotANumber", "4x2", notANumber.value)
    expectEq("parse_count(\" 42 \")", 42u, parseCount(" 42 "))

    // 5. The message of an error is the core's Display.
    expectEq("the message of TooLong(9)", "longer than 9 characters", tooLong.message)
    expectEq("the message of NotANumber", "`4x2` is not a number", notANumber.message)
}
