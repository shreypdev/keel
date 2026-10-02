import Foundation
import XCTest
@testable import UndraRuntime

/// The twelve records of the opt-in WebSocket, Sse and Db ports (ADR-047, ADR-048): their ids,
/// their exact bytes (the vectors of the Rust unit tests in `crates/undra-ports/src/{ws,sse,db}.rs`),
/// their messages (the Rust `#[error]` texts) and the conformances generated code relies on.
final class PortRecordsV2Tests: XCTestCase {
    // MARK: Ids

    func testThePortAndMethodIdsAreTheBriefs() {
        XCTAssertEqual(StandardPorts.WebSocket.portId, 0x7388_b95f)
        XCTAssertEqual(StandardPorts.WebSocket.connect, 0x8347_7638)
        XCTAssertEqual(StandardPorts.WebSocket.send, 0x117b_2158)
        XCTAssertEqual(StandardPorts.WebSocket.receive, 0x8f31_f08f)
        XCTAssertEqual(StandardPorts.WebSocket.close, 0x6015_4b86)
        XCTAssertEqual(StandardPorts.Sse.portId, 0x75d2_ef19)
        XCTAssertEqual(StandardPorts.Sse.open, 0xc003_3c14)
        XCTAssertEqual(StandardPorts.Sse.next, 0x4035_cbed)
        XCTAssertEqual(StandardPorts.Sse.close, 0x5bfe_2c88)
        XCTAssertEqual(StandardPorts.Db.portId, 0x559e_da82)
        XCTAssertEqual(StandardPorts.Db.open, 0xee6f_26db)
        XCTAssertEqual(StandardPorts.Db.execute, 0xffac_2f0a)
        XCTAssertEqual(StandardPorts.Db.query, 0x3a4d_eefd)
        XCTAssertEqual(StandardPorts.Db.begin, 0xae2b_a428)
        XCTAssertEqual(StandardPorts.Db.commit, 0xf866_d5ae)
        XCTAssertEqual(StandardPorts.Db.rollback, 0x3e7b_24b3)
        XCTAssertEqual(StandardPorts.Db.close, 0xde3d_c7ed)
    }

    func testTheTypeIdsFollowTheNamingRule() {
        // The type ids of the brief, `fnv1a32("<Name>")`, for logs and the schema.
        XCTAssertEqual(fnv1a32("WsOpened"), 0x9364_0662)
        XCTAssertEqual(fnv1a32("WsMessage"), 0x9f2d_9b9e)
        XCTAssertEqual(fnv1a32("WsError"), 0xc4e7_cc8f)
        XCTAssertEqual(fnv1a32("SseEvent"), 0xa898_28ce)
        XCTAssertEqual(fnv1a32("SseError"), 0x2e78_01f4)
        XCTAssertEqual(fnv1a32("DbMigration"), 0x36b3_1925)
        XCTAssertEqual(fnv1a32("DbOpened"), 0xaf76_040e)
        XCTAssertEqual(fnv1a32("DbValue"), 0x48f7_4ac0)
        XCTAssertEqual(fnv1a32("DbExecuted"), 0x41a1_a3a6)
        XCTAssertEqual(fnv1a32("DbRows"), 0xffd1_2f2e)
        XCTAssertEqual(fnv1a32("DbConstraint"), 0x856f_0900)
        XCTAssertEqual(fnv1a32("DbError"), 0x1dfc_036b)
    }

    // MARK: Exact bytes (crates/undra-ports/src/ws.rs `exact_bytes_of_the_records`)

    func testWebSocketRecordsHaveTheRustBytes() {
        assertCodec(WsMessage.text("hi"), hex: "0000 02000000 6869")
        assertCodec(WsMessage.binary([7]), hex: "0100 01000000 07")
        assertCodec(WsOpened(conn: 3, protocol: "p"), hex: "03000000 01000000 70")
        assertCodec(WsError.closed(code: 1000, reason: ""), hex: "0300 e803 00000000")
        assertCodec(WsError.refused(status: 401, message: ""), hex: "0000 01 9101 00000000")
        assertCodec(WsError.refused(status: nil, message: "x"), hex: "0000 00 01000000 78")
        assertCodec(WsError.network("e"), hex: "0100 01000000 65")
        assertCodec(WsError.protocol("e"), hex: "0200 01000000 65")
        expectWireError(.invalidTag(tag: 2, at: 0, type: "WsMessage")) {
            _ = try WsMessage.undraDecoded(from: [2, 0])
        }
        expectWireError(.invalidTag(tag: 4, at: 0, type: "WsError")) {
            _ = try WsError.undraDecoded(from: [4, 0])
        }
    }

    // MARK: crates/undra-ports/src/sse.rs `records_round_trip_with_exact_bytes`

    func testSseRecordsHaveTheRustBytes() {
        assertCodec(SseEvent(id: nil, event: "", data: "", retryMs: nil), hex: "00 00000000 00000000 00")
        assertCodec(
            SseEvent(id: "7", event: "message", data: "a\nb", retryMs: 3000),
            hex: "01 01000000 37 07000000 6d657373616765 03000000 610a62 01 b80b0000"
        )
        assertCodec(SseError.ended, hex: "0300")
        assertCodec(SseError.refused(status: 204, message: "no"), hex: "0000 01 cc00 02000000 6e6f")
        assertCodec(SseError.network("x"), hex: "0100 01000000 78")
        assertCodec(SseError.protocol("x"), hex: "0200 01000000 78")
        expectWireError(.invalidTag(tag: 4, at: 0, type: "SseError")) {
            _ = try SseError.undraDecoded(from: [4, 0])
        }
    }

    // MARK: crates/undra-ports/src/db.rs `exact_bytes_of_the_records`

    func testDbRecordsHaveTheRustBytes() {
        assertCodec(DbValue.null, hex: "0000")
        assertCodec(DbValue.integer(-2), hex: "0100 feffffffffffffff")
        assertCodec(DbValue.real(1.0), hex: "0200 000000000000f03f")
        assertCodec(DbValue.text("a"), hex: "0300 01000000 61")
        assertCodec(DbValue.blob([9]), hex: "0400 01000000 09")
        assertCodec(DbOpened(db: 1, version: 2), hex: "01000000 02000000")
        assertCodec(DbError.constraint(kind: .notNull, message: ""), hex: "0100 0100 00000000")
        assertCodec(DbError.migration(version: 3, message: ""), hex: "0600 03000000 00000000")
        assertCodec(DbError.busy, hex: "0000")
        assertCodec(DbError.corrupt("c"), hex: "0200 01000000 63")
        assertCodec(DbError.full, hex: "0300")
        assertCodec(DbError.unavailable("u"), hex: "0400 01000000 75")
        assertCodec(DbError.sql(message: "s"), hex: "0500 01000000 73")
        assertCodec(DbMigration(version: 1, sql: "x"), hex: "01000000 01000000 78")
        assertCodec(DbExecuted(changes: 1, lastInsertId: -1), hex: "0100000000000000 ffffffffffffffff")
        assertCodec(
            DbRows(columns: ["a"], rows: [[.integer(1), .null]]),
            hex: "01000000 01000000 61 01000000 02000000 0100 0100000000000000 0000"
        )
        assertCodec(DbRows(), hex: "00000000 00000000")
        XCTAssertEqual(DbConstraint.allCases.map { $0.rawValue }, [0, 1, 2, 3, 4])
        assertCodec(DbConstraint.check, hex: "0300")
        expectWireError(.invalidTag(tag: 5, at: 0, type: "DbConstraint")) {
            _ = try DbConstraint.undraDecoded(from: [5, 0])
        }
        expectWireError(.invalidTag(tag: 5, at: 0, type: "DbValue")) {
            _ = try DbValue.undraDecoded(from: [5, 0])
        }
        expectWireError(.invalidTag(tag: 7, at: 0, type: "DbError")) {
            _ = try DbError.undraDecoded(from: [7, 0])
        }
    }

    func testEveryErrorRoundTripsAndTruncationsThrowWireErrors() {
        let errors: [any UndraCodec] = [
            WsError.refused(status: 403, message: "forbidden"), WsError.refused(status: nil, message: "x"),
            WsError.network("reset"), WsError.protocol("masked frame"), WsError.closed(code: 1000, reason: "bye"),
            SseError.refused(status: 204, message: "no content"), SseError.network("x"),
            SseError.protocol("text/html"), SseError.ended,
            DbError.busy, DbError.constraint(kind: .unique, message: "UNIQUE constraint failed: t.id"),
            DbError.corrupt("file is not a database"), DbError.full, DbError.unavailable("closed"),
            DbError.sql(message: "no such table: x"), DbError.migration(version: 2, message: "boom"),
        ]
        for error in errors {
            let bytes = error.undraEncoded()
            for cut in 0 ..< bytes.count {
                XCTAssertThrowsError(try type(of: error).undraDecoded(from: Array(bytes[0 ..< cut])), "\(error) cut \(cut)") { thrown in
                    XCTAssertTrue(thrown is WireError, "\(thrown)")
                }
            }
        }
        XCTAssertEqual(try WsError.undraDecoded(from: WsError.closed(code: 4000, reason: "é").undraEncoded()), .closed(code: 4000, reason: "é"))
        XCTAssertEqual(try DbError.undraDecoded(from: DbError.migration(version: 9, message: "m").undraEncoded()), .migration(version: 9, message: "m"))
    }

    func testDbValuesRoundTripTheirExtremes() throws {
        let values: [DbValue] = [
            .integer(.min), .integer(.max), .integer(0), .real(-0.0), .real(.infinity), .real(1e-300),
            .text(""), .text("é😀\u{0}x"), .blob([]), .blob([0, 255, 7]), .null,
        ]
        for value in values {
            XCTAssertEqual(try DbValue.undraDecoded(from: value.undraEncoded()), value)
        }
        XCTAssertEqual(values.map(\.storageClass), [
            "integer", "integer", "integer", "real", "real", "real", "text", "text", "blob", "blob", "null",
        ])
    }

    // MARK: Messages

    func testTheErrorsCarryTheMessagesOfTheRustErrorAttributes() {
        XCTAssertEqual(WsError.closed(code: 1001, reason: "away").description, "the WebSocket was closed (1001): away")
        XCTAssertEqual(WsError.network("dns").description, "WebSocket network error: dns")
        XCTAssertEqual(WsError.protocol("p").description, "WebSocket protocol error: p")
        XCTAssertEqual(WsError.refused(status: 401, message: "m").description, "the WebSocket was refused: m")
        XCTAssertEqual(SseError.ended.description, "the server ended the event stream")
        XCTAssertEqual(SseError.refused(status: nil, message: "m").description, "the event stream was refused: m")
        XCTAssertEqual(SseError.network("n").description, "event stream network error: n")
        XCTAssertEqual(SseError.protocol("p").description, "event stream protocol error: p")
        XCTAssertEqual(DbError.busy.description, "the database is busy")
        XCTAssertEqual(DbError.migration(version: 4, message: "x").description, "migration 4 failed: x")
        XCTAssertEqual(DbError.constraint(kind: .check, message: "c").description, "constraint failed: c")
        XCTAssertEqual(DbError.corrupt("c").description, "the database is corrupt: c")
        XCTAssertEqual(DbError.full.description, "the database is full")
        XCTAssertEqual(DbError.unavailable("u").description, "the database is unavailable: u")
        XCTAssertEqual(DbError.sql(message: "s").description, "SQL error: s")
        // `LocalizedError`, as every generated error is.
        XCTAssertEqual(WsError.network("dns").localizedDescription, "WebSocket network error: dns")
        XCTAssertEqual((SseError.ended as any Error).localizedDescription, "the server ended the event stream")
        XCTAssertEqual((DbError.full as any Error).localizedDescription, "the database is full")
    }
}
