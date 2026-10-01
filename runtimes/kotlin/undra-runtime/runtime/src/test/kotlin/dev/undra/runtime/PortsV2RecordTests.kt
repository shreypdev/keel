package dev.undra.runtime

import dev.undra.runtime.adapters.DbConstraint
import dev.undra.runtime.adapters.DbError
import dev.undra.runtime.adapters.DbExecuted
import dev.undra.runtime.adapters.DbMigration
import dev.undra.runtime.adapters.DbOpened
import dev.undra.runtime.adapters.DbRows
import dev.undra.runtime.adapters.DbValue
import dev.undra.runtime.adapters.SseError
import dev.undra.runtime.adapters.SseEvent
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.adapters.WsError
import dev.undra.runtime.adapters.WsMessage
import dev.undra.runtime.adapters.WsOpened
import dev.undra.runtime.adapters.fromSqliteCode
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertBytes
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.wire.Fnv
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.WireException
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import org.junit.jupiter.api.Test

private fun <T> roundTrip(codec: UndraCodec<T>, value: T) {
    assertEq(value, codec.decodeAll(codec.encodeToByteArray(value)), "round trip of $value")
}

/**
 * The twelve records of the opt-in ports (ADR-047, ADR-048): the exact bytes of the Rust unit tests in
 * `crates/undra-ports/src/{ws,sse,db}.rs`, round trips of every variant, the Rust `#[error]` texts and the ids.
 */
class PortsV2RecordTests : Suite() {
    init {
        case("the ids of WebSocket, Sse and Db are fnv1a32 of their names (and the brief's hex)") {
            fun id(name: String) = Fnv.fnv1a32(name)
            assertEq(id("port.WebSocket"), StandardPorts.WebSocket.PORT_ID)
            assertEq(0x7388b95fu, StandardPorts.WebSocket.PORT_ID)
            for ((method, value) in listOf("connect" to StandardPorts.WebSocket.CONNECT, "send" to StandardPorts.WebSocket.SEND, "receive" to StandardPorts.WebSocket.RECEIVE, "close" to StandardPorts.WebSocket.CLOSE)) {
                assertEq(id("WebSocket.$method"), value, method)
            }
            assertEq(id("port.Sse"), StandardPorts.Sse.PORT_ID)
            assertEq(0x75d2ef19u, StandardPorts.Sse.PORT_ID)
            for ((method, value) in listOf("open" to StandardPorts.Sse.OPEN, "next" to StandardPorts.Sse.NEXT, "close" to StandardPorts.Sse.CLOSE)) {
                assertEq(id("Sse.$method"), value, method)
            }
            assertEq(id("port.Db"), StandardPorts.Db.PORT_ID)
            assertEq(0x559eda82u, StandardPorts.Db.PORT_ID)
            val db = listOf(
                "open" to StandardPorts.Db.OPEN, "execute" to StandardPorts.Db.EXECUTE, "query" to StandardPorts.Db.QUERY,
                "begin" to StandardPorts.Db.BEGIN, "commit" to StandardPorts.Db.COMMIT, "rollback" to StandardPorts.Db.ROLLBACK,
                "close" to StandardPorts.Db.CLOSE,
            )
            for ((method, value) in db) assertEq(id("Db.$method"), value, method)
            assertEq(0xee6f26dbu, StandardPorts.Db.OPEN)
            assertEq(0xde3dc7edu, StandardPorts.Db.CLOSE)
        }

        case("WebSocket records: the bytes of ws.rs's exact_bytes_of_the_records") {
            assertBytes("0000" + "02000000" + "6869", WsMessage.encodeToByteArray(WsMessage.Text("hi")))
            assertBytes("0100" + "01000000" + "07", WsMessage.encodeToByteArray(WsMessage.Binary(byteArrayOf(7))))
            assertBytes("03000000" + "01000000" + "70", WsOpened.encodeToByteArray(WsOpened(3u, "p")))
            assertBytes("0300" + "e803" + "00000000", WsError.encodeToByteArray(WsError.Closed(1000u, "")))
            assertBytes("0000" + "01" + "9101" + "00000000", WsError.encodeToByteArray(WsError.Refused(401u, "")))
            assertBytes("0000" + "00" + "01000000" + "78", WsError.encodeToByteArray(WsError.Refused(null, "x")))
            assertBytes("0100" + "03000000" + "646e73", WsError.encodeToByteArray(WsError.Network("dns")))
            assertBytes("0200" + "01000000" + "6d", WsError.encodeToByteArray(WsError.Protocol("m")))
        }

        case("Sse records: the bytes of sse.rs's records_round_trip_with_exact_bytes") {
            assertBytes("00" + "00000000" + "00000000" + "00", SseEvent.encodeToByteArray(SseEvent(null, "", "", null)))
            assertBytes("0300", SseError.encodeToByteArray(SseError.Ended))
            assertBytes(
                "01" + "01000000" + "37" + "07000000" + "6d657373616765" + "03000000" + "610a62" + "01" + "b80b0000",
                SseEvent.encodeToByteArray(SseEvent("7", "message", "a\nb", 3000u)),
            )
            assertBytes("0000" + "01" + "cc00" + "02000000" + "6e6f", SseError.encodeToByteArray(SseError.Refused(204u, "no")))
        }

        case("Db records: the bytes of db.rs's exact_bytes_of_the_records") {
            assertBytes("0000", DbValue.encodeToByteArray(DbValue.Null))
            assertBytes("0100" + "feffffffffffffff", DbValue.encodeToByteArray(DbValue.Integer(-2)))
            assertBytes("0200" + "000000000000f03f", DbValue.encodeToByteArray(DbValue.Real(1.0)))
            assertBytes("0300" + "01000000" + "61", DbValue.encodeToByteArray(DbValue.Text("a")))
            assertBytes("0400" + "01000000" + "09", DbValue.encodeToByteArray(DbValue.Blob(byteArrayOf(9))))
            assertBytes("01000000" + "02000000", DbOpened.encodeToByteArray(DbOpened(1u, 2u)))
            assertBytes("0100" + "0100" + "00000000", DbError.encodeToByteArray(DbError.Constraint(DbConstraint.NOT_NULL, "")))
            assertBytes("0600" + "03000000" + "00000000", DbError.encodeToByteArray(DbError.Migration(3u, "")))
        }

        case("Db records the Rust tests do not spell out, by the layout of SPEC 3.1") {
            assertBytes("02000000" + "01000000" + "78", DbMigration.encodeToByteArray(DbMigration(2u, "x")))
            assertBytes("0500000000000000" + "ffffffffffffffff", DbExecuted.encodeToByteArray(DbExecuted(5uL, -1L)))
            assertBytes(
                "01000000" + "01000000" + "6e" + "01000000" + "01000000" + "0100" + "0700000000000000",
                DbRows.encodeToByteArray(DbRows(listOf("n"), listOf(listOf(DbValue.Integer(7))))),
            )
            assertBytes("0400", DbConstraint.encodeToByteArray(DbConstraint.OTHER))
            assertBytes("0000", DbError.encodeToByteArray(DbError.Busy))
            assertBytes("0300", DbError.encodeToByteArray(DbError.Full))
            assertBytes("0500" + "01000000" + "71", DbError.encodeToByteArray(DbError.Sql("q")))
        }

        case("every variant of the twelve types round-trips") {
            roundTrip(WsOpened, WsOpened(UInt.MAX_VALUE, "chat.v2"))
            for (m in listOf(WsMessage.Text(""), WsMessage.Text("é😀"), WsMessage.Binary(ByteArray(0)), WsMessage.Binary(ByteArray(70_000) { it.toByte() }))) roundTrip(WsMessage, m)
            for (e in listOf(WsError.Refused(403u, "forbidden"), WsError.Refused(null, "x"), WsError.Network("reset"), WsError.Protocol("masked frame"), WsError.Closed(1000u, "bye"))) roundTrip(WsError, e)
            roundTrip(SseEvent, SseEvent("7", "message", "a\nb", 3000u))
            roundTrip(SseEvent, SseEvent(null, "tick", "", null))
            for (e in listOf(SseError.Refused(204u, "no content"), SseError.Refused(null, ""), SseError.Network("x"), SseError.Protocol("text/html"), SseError.Ended)) roundTrip(SseError, e)
            roundTrip(DbMigration, DbMigration(1u, "CREATE TABLE a (x); CREATE TABLE b (y)"))
            roundTrip(DbOpened, DbOpened(9u, 0u))
            for (v in listOf(DbValue.Null, DbValue.Integer(Long.MIN_VALUE), DbValue.Integer(Long.MAX_VALUE), DbValue.Real(-0.0), DbValue.Real(Double.MAX_VALUE), DbValue.Text("é😀"), DbValue.Blob(ByteArray(0)), DbValue.Blob(byteArrayOf(0, -1, 7)))) roundTrip(DbValue, v)
            roundTrip(DbExecuted, DbExecuted(ULong.MAX_VALUE, Long.MIN_VALUE))
            roundTrip(DbRows, DbRows(listOf("a", "b"), listOf(listOf(DbValue.Null, DbValue.Text("x")), listOf(DbValue.Integer(1), DbValue.Blob(byteArrayOf(1))))))
            roundTrip(DbRows, DbRows(emptyList(), emptyList()))
            for (k in DbConstraint.entries) roundTrip(DbConstraint, k)
            val errors = listOf(
                DbError.Busy, DbError.Constraint(DbConstraint.UNIQUE, "UNIQUE constraint failed: t.id"), DbError.Corrupt("file is not a database"),
                DbError.Full, DbError.Unavailable("closed"), DbError.Sql("no such table: x"), DbError.Migration(2u, "boom"),
            )
            for (e in errors) roundTrip(DbError, e)
            assertEq(DbConstraint.entries.map { it.index.toInt() }, listOf(0, 1, 2, 3, 4))
        }

        case("unknown variants and truncated records are WireExceptions") {
            assertThrows<WireException.InvalidTag> { WsMessage.decodeAll(byteArrayOf(2, 0)) }
            assertThrows<WireException.InvalidTag> { WsError.decodeAll(byteArrayOf(4, 0)) }
            assertThrows<WireException.InvalidTag> { SseError.decodeAll(byteArrayOf(4, 0)) }
            assertThrows<WireException.InvalidTag> { DbValue.decodeAll(byteArrayOf(5, 0)) }
            assertThrows<WireException.InvalidTag> { DbConstraint.decodeAll(byteArrayOf(5, 0)) }
            assertThrows<WireException.InvalidTag> { DbError.decodeAll(byteArrayOf(7, 0)) }
            val event = SseEvent.encodeToByteArray(SseEvent("1", "message", "data", 5u))
            for (cut in 0 until event.size) assertThrows<WireException>("cut at $cut") { SseEvent.decodeAll(event.copyOf(cut)) }
            val rows = DbRows.encodeToByteArray(DbRows(listOf("a"), listOf(listOf(DbValue.Text("x")))))
            for (cut in 0 until rows.size) assertThrows<WireException>("cut at $cut") { DbRows.decodeAll(rows.copyOf(cut)) }
        }

        case("the errors' messages are the Rust #[error] texts") {
            assertEq("the WebSocket was closed (1001): away", WsError.Closed(1001u, "away").message)
            assertEq("WebSocket network error: dns", WsError.Network("dns").message)
            assertEq("WebSocket protocol error: masked frame", WsError.Protocol("masked frame").message)
            assertEq("the WebSocket was refused: forbidden", WsError.Refused(403u, "forbidden").message)
            assertEq("the server ended the event stream", SseError.Ended.message)
            assertEq("the event stream was refused: gone", SseError.Refused(204u, "gone").message)
            assertEq("event stream network error: x", SseError.Network("x").message)
            assertEq("event stream protocol error: y", SseError.Protocol("y").message)
            assertEq("the database is busy", DbError.Busy.message)
            assertEq("migration 4 failed: x", DbError.Migration(4u, "x").message)
            assertEq("constraint failed: UNIQUE constraint failed: t.id", DbError.Constraint(DbConstraint.UNIQUE, "UNIQUE constraint failed: t.id").message)
            assertEq("the database is corrupt: c", DbError.Corrupt("c").message)
            assertEq("the database is full", DbError.Full.message)
            assertEq("the database is unavailable: u", DbError.Unavailable("u").message)
            assertEq("SQL error: s", DbError.Sql("s").message)
            val errors: List<Throwable> = listOf(WsError.Network("x"), DbError.Busy, SseError.Ended)
            assertTrue(errors.all { it is UndraException }, "the errors are UndraExceptions, so generated code can throw them")
        }

        case("values compare by content: blobs, binary messages and reals") {
            assertEq(DbValue.Blob(byteArrayOf(1, 2)), DbValue.Blob(byteArrayOf(1, 2)))
            assertEq(DbValue.Blob(byteArrayOf(1, 2)).hashCode(), DbValue.Blob(byteArrayOf(1, 2)).hashCode())
            assertTrue(DbValue.Blob(byteArrayOf(1)) != DbValue.Blob(byteArrayOf(2)))
            assertEq(WsMessage.Binary(byteArrayOf(3)), WsMessage.Binary(byteArrayOf(3)))
            assertTrue(WsMessage.Binary(byteArrayOf(3)) != WsMessage.Text("\u0003"))
            assertEq(DbValue.Real(Double.NaN), DbValue.Real(Double.NaN))
        }

        case("SQLite result codes pick the DbError variant (ADR-048), never the text") {
            assertEq(DbError.Busy, DbError.fromSqliteCode(5, "database is locked"))
            assertEq(DbError.Busy, DbError.fromSqliteCode(6, "database table is locked"))
            assertEq(DbError.Busy, DbError.fromSqliteCode(517, "busy snapshot"))
            assertEq(DbError.Constraint(DbConstraint.UNIQUE, "m"), DbError.fromSqliteCode(2067, "m"))
            assertEq(DbError.Constraint(DbConstraint.UNIQUE, "m"), DbError.fromSqliteCode(1555, "m"))
            assertEq(DbError.Constraint(DbConstraint.NOT_NULL, "m"), DbError.fromSqliteCode(1299, "m"))
            assertEq(DbError.Constraint(DbConstraint.FOREIGN_KEY, "m"), DbError.fromSqliteCode(787, "m"))
            assertEq(DbError.Constraint(DbConstraint.CHECK, "m"), DbError.fromSqliteCode(275, "m"))
            assertEq(DbError.Constraint(DbConstraint.OTHER, "m"), DbError.fromSqliteCode(1811, "m"))
            assertEq(DbError.Constraint(DbConstraint.OTHER, "m"), DbError.fromSqliteCode(19, "m"))
            assertEq(DbError.Corrupt("c"), DbError.fromSqliteCode(11, "c"))
            assertEq(DbError.Corrupt("c"), DbError.fromSqliteCode(26, "c"))
            assertEq(DbError.Full, DbError.fromSqliteCode(13, "full"))
            for (code in listOf(14, 3, 8, 10, 266)) assertEq(DbError.Unavailable("u"), DbError.fromSqliteCode(code, "u"), "code $code")
            assertEq(DbError.Sql("near \"SELEC\": syntax error"), DbError.fromSqliteCode(1, "near \"SELEC\": syntax error"))
        }
    }

    @Test
    fun allCases() = assertPassed()
}
