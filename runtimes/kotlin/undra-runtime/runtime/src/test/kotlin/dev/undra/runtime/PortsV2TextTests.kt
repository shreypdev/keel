package dev.undra.runtime

import dev.undra.runtime.adapters.SqlText
import dev.undra.runtime.adapters.SseEvent
import dev.undra.runtime.adapters.SseParser
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import org.junit.jupiter.api.Test

private fun message(data: String, id: String? = null, retry: UInt? = null): SseEvent = SseEvent(id, "message", data, retry)

/** The two text parsers of the opt-in ports: the event-stream parser (HTML standard) and SQLite statement boundaries. */
class PortsV2TextTests : Suite() {
    init {
        // ---- SseParser ----------------------------------------------------------------------------------------------

        case("SSE: the feed of the shared server parses as scenarios.md S24 says") {
            val feed = ": a comment\nretry: 1500\nid: 1\ndata: one\n\n" +
                "event: tick\nid: 2\ndata: two\ndata: lines\n\n" +
                "data: three\n\n" +
                "id: 4\r\ndata: four\r\n\r\n" +
                "event: ignored\n\n"
            assertEq(
                listOf(
                    SseEvent("1", "message", "one", 1500u),
                    SseEvent("2", "tick", "two\nlines", null),
                    SseEvent("2", "message", "three", null),
                    SseEvent("4", "message", "four", null),
                ),
                SseParser().push(feed),
            )
        }

        case("SSE: the same stream in pieces of every size parses the same, CRLF split across pieces included") {
            val feed = "﻿data: a\r\n\r\nid: 7\rdata:b\r\rdata\n\ndata: x\r\ndata: y\r\n\r\n"
            val whole = SseParser().push(feed)
            assertEq(listOf(message("a"), message("b", "7"), message("", "7"), message("x\ny", "7")), whole)
            for (size in 1..feed.length) {
                val parser = SseParser()
                val events = feed.chunked(size).flatMap { parser.push(it) }
                assertEq(whole, events, "pieces of $size")
            }
        }

        case("SSE: field rules (one space dropped, no colon, unknown fields, id with NUL, retry digits only)") {
            val events = SseParser().push(
                "data:  two spaces\n\n" + "data\n\n" + "foo: bar\ndata: x\n\n" + "id: a\u0000b\ndata: y\n\n" +
                    "retry: 12a\ndata: z\n\n" + "retry: 99999999999\ndata: w\n\n" + "retry: 0\ndata: v\n\n" + "id\ndata: u\n\n",
            )
            assertEq(
                listOf(message(" two spaces"), message(""), message("x"), message("y"), message("z"), message("w"), message("v", retry = 0u), message("u")),
                events,
            )
        }

        case("SSE: a blank line without data resets the type; retry belongs to the event it came with") {
            val parser = SseParser()
            assertEq(emptyList<SseEvent>(), parser.push("event: e\n\n"))
            assertEq(listOf(message("a")), parser.push("data: a\n\n"))
            assertEq(listOf(message("b", retry = 5u), message("c")), parser.push("retry: 5\ndata: b\n\ndata: c\n\n"))
        }

        case("SSE: an event the stream ends in the middle of is discarded; the last event id survives") {
            val parser = SseParser()
            assertEq(emptyList<SseEvent>(), parser.push("id: 3\ndata: half"))
            parser.end()
            assertEq(listOf(message("next", "3")), parser.push("\ndata: next\n\n"))
        }

        case("SSE: a stream resumed from an id gives that id to the events without one, until an id field changes it") {
            val parser = SseParser("2")
            assertEq(listOf(message("three", "2"), message("four", "4"), message("five", "4")), parser.push("data: three\n\nid: 4\ndata: four\n\ndata: five\n\n"))
            assertEq(listOf(message("six")), SseParser("9").push("id\ndata: six\n\n"))
        }

        case("SSE: a byte order mark is skipped only at the very start") {
            assertEq(listOf(message("a")), SseParser().push("﻿data: a\n\n"))
            val parser = SseParser()
            parser.push("data: a\n\n")
            // Later, U+FEFF is part of the field name: "﻿data" is an unknown field.
            assertEq(listOf(message("b")), parser.push("﻿data: c\n\ndata: b\n\n"))
        }

        // ---- SqlText ------------------------------------------------------------------------------------------------

        case("SQL: statements end at semicolons outside strings, identifiers and comments") {
            assertEq(listOf("SELECT 1"), SqlText.statements("SELECT 1"))
            assertEq(listOf("SELECT 1"), SqlText.statements("  SELECT 1 ;  -- trailing comment\n /* block */ "))
            assertEq(listOf("SELECT 1", "SELECT 2"), SqlText.statements("SELECT 1; SELECT 2;"))
            assertEq(listOf("SELECT ';', \"a;b\", [c;d], `e;f`"), SqlText.statements("SELECT ';', \"a;b\", [c;d], `e;f`;"))
            assertEq(listOf("SELECT 'it''s; fine'"), SqlText.statements("SELECT 'it''s; fine'"))
            assertEq(listOf("SELECT 1 /* ; */ + 2"), SqlText.statements("SELECT 1 /* ; */ + 2 -- ;\n"))
            assertEq(emptyList<String>(), SqlText.statements(" ; ;; -- nothing\n"))
            assertEq(
                listOf("CREATE INDEX notes_done ON notes (done)", "CREATE TABLE tags (note INTEGER NOT NULL REFERENCES notes (id) ON DELETE CASCADE, tag TEXT NOT NULL)"),
                SqlText.statements("CREATE INDEX notes_done ON notes (done); CREATE TABLE tags (note INTEGER NOT NULL REFERENCES notes (id) ON DELETE CASCADE, tag TEXT NOT NULL)"),
            )
        }

        case("SQL: a trigger body's semicolons do not end it; `; END ;` does") {
            val trigger = "CREATE TEMP TRIGGER t AFTER INSERT ON a BEGIN INSERT INTO b VALUES (1); UPDATE c SET x = CASE WHEN 1 THEN 2 END; END"
            assertEq(listOf(trigger, "SELECT 1"), SqlText.statements("$trigger; SELECT 1;"))
            val plain = "create trigger t2 before delete on a begin select raise(abort, 'no'); end"
            assertEq(listOf(plain), SqlText.statements("$plain;"))
            assertEq(listOf("SELECT trigger FROM x", "SELECT 2"), SqlText.statements("SELECT trigger FROM x; SELECT 2"))
        }

        case("SQL: parameter counts are sqlite3_bind_parameter_count's") {
            assertEq(0, SqlText.parameterCount("SELECT 1"))
            assertEq(2, SqlText.parameterCount("INSERT INTO t VALUES (?, ?)"))
            assertEq(5, SqlText.parameterCount("SELECT ?5"))
            assertEq(6, SqlText.parameterCount("SELECT ?5, ?"))
            assertEq(2, SqlText.parameterCount("SELECT :a, @b, :a"))
            assertEq(2, SqlText.parameterCount("SELECT \$x, ?, \$x, ?2"))
            assertEq(3, SqlText.parameterCount("SELECT \$x, ?, :y"))
            assertEq(0, SqlText.parameterCount("SELECT '?', \"?\", [?] -- ?\n /* :a */"))
            assertEq(1, SqlText.parameterCount("SELECT * FROM t WHERE a = ? AND b = 'x:y'"))
        }

        case("SQL: every variable form SQLite's tokenizer knows counts as it does (`#name`, `::` and `(...)` after any prefix)") {
            // The counts the SQLite of sqlite-jdbc reports (sqlite3_bind_parameter_count) for each statement; Android, which
            // has no such call, relies on these. A form counted short would bind the values to the wrong parameters there.
            assertEq(1, SqlText.parameterCount("SELECT #a"))
            assertEq(2, SqlText.parameterCount("SELECT #a, #a, :a"))
            assertEq(2, SqlText.parameterCount("SELECT ?, #b"))
            assertEq(1, SqlText.parameterCount("SELECT :a::b"))
            assertEq(1, SqlText.parameterCount("SELECT @a::b, @a::b"))
            assertEq(2, SqlText.parameterCount("SELECT :a(1), :a(2)"))
            assertEq(1, SqlText.parameterCount("SELECT \$x(y), \$x(y)"))
            assertEq(1, SqlText.parameterCount("SELECT :\$"))
            assertEq(1, SqlText.parameterCount("SELECT @a\$"))
            assertEq(2, SqlText.parameterCount("SELECT ?1, :1"))
            assertEq(listOf("SELECT :a(;)", "SELECT 2"), SqlText.statements("SELECT :a(;); SELECT 2"))
        }
    }

    @Test
    fun allCases() = assertPassed()
}
