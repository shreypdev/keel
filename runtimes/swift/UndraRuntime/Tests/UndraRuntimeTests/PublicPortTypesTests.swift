import Foundation
import XCTest
// A plain `import`, not `@testable`: this file sees only what the runtime exports, so it stops
// compiling if a type, initializer, case, conformance or protocol requirement an app needs to
// read the opt-in ports' records or to write its own adapter becomes internal.
import UndraRuntime

/// The records and adapter interfaces of the opt-in WebSocket, Sse and Db ports as an app and the
/// generated bindings see them (ADR-047, ADR-048).
final class PublicPortTypesTests: XCTestCase {
    func testTheRecordsAreBuiltAndReadFromOutsideTheModule() throws {
        let opened = WsOpened(conn: 1, protocol: "chat")
        XCTAssertEqual(opened.protocol, "chat")
        XCTAssertEqual(WsMessage.text("é").bytes, [0xC3, 0xA9])
        XCTAssertEqual(WsMessage.binary([1]).bytes, [1])
        let event = SseEvent(data: "hello")
        XCTAssertEqual(event, SseEvent(id: nil, event: "message", data: "hello", retryMs: nil))
        let rows = DbRows(columns: ["n"], rows: [[.integer(1)]])
        XCTAssertEqual(rows.rows.first?.first, .integer(1))
        XCTAssertEqual(DbRows().columns, [])
        XCTAssertEqual(DbExecuted(changes: 1, lastInsertId: 7).lastInsertId, 7)
        XCTAssertEqual(DbMigration(version: 1, sql: "CREATE TABLE t (x)").version, 1)
        XCTAssertEqual(DbOpened(db: 1, version: 2).version, 2)
        XCTAssertEqual(DbConstraint.allCases, [.unique, .notNull, .foreignKey, .check, .other])
        XCTAssertEqual(try SseEvent.undraDecoded(from: event.undraEncoded()), event)
    }

    func testTheErrorsAreTheDomainOfAGeneratedCall() {
        // What a generated method does with a failed call (`UndraCallError.mapped(_:domain:)`).
        let ws = UndraCallError.mapped(UndraReplyError(status: .error, body: WsError.closed(code: 4001, reason: "kicked").undraEncoded()), domain: WsError.self)
        XCTAssertEqual(ws as? WsError, .closed(code: 4001, reason: "kicked"))
        let sse = UndraCallError.mapped(UndraReplyError(status: .error, body: SseError.ended.undraEncoded()), domain: SseError.self)
        XCTAssertEqual(sse as? SseError, .ended)
        let db = UndraCallError.mapped(UndraReplyError(status: .error, body: DbError.constraint(kind: .unique, message: "m").undraEncoded()), domain: DbError.self)
        XCTAssertEqual(db as? DbError, .constraint(kind: .unique, message: "m"))
    }

    func testTheTypesAreCodableSendableAndHashableWhereGeneratedCodeNeedsIt() async throws {
        // A generated record holding one of these derives `Codable` (`SseFollow` holds events).
        let event = SseEvent(id: "1", event: "tick", data: "x", retryMs: 1500)
        XCTAssertEqual(try JSONDecoder().decode(SseEvent.self, from: JSONEncoder().encode(event)), event)
        let opened = WsOpened(conn: 2, protocol: "")
        XCTAssertEqual(try JSONDecoder().decode(WsOpened.self, from: JSONEncoder().encode(opened)), opened)
        let migration = DbMigration(version: 1, sql: "x")
        XCTAssertEqual(try JSONDecoder().decode(DbMigration.self, from: JSONEncoder().encode(migration)), migration)
        XCTAssertEqual(try JSONDecoder().decode([DbConstraint].self, from: JSONEncoder().encode([DbConstraint.check])), [.check])
        // The generator derives `Codable` for a record holding any standard type, so the enums with
        // data and `DbRows` are `Codable` too.
        let messages: [WsMessage] = [.text("a"), .binary([1, 2])]
        XCTAssertEqual(try JSONDecoder().decode([WsMessage].self, from: JSONEncoder().encode(messages)), messages)
        let rows = DbRows(columns: ["a", "b"], rows: [[.null, .integer(-1)], [.real(1.5), .text("x")], [.blob([7]), .null]])
        XCTAssertEqual(try JSONDecoder().decode(DbRows.self, from: JSONEncoder().encode(rows)), rows)
        let (message, failure) = await Task.detached { (WsMessage.text("a"), DbError.busy) }.value
        XCTAssertEqual(message, .text("a"))
        XCTAssertEqual(failure, .busy)
        XCTAssertEqual(Set([DbValue.integer(1), .integer(1), .real(1)]).count, 2)
        XCTAssertEqual(Set([WsError.network("a"), .network("a")]).count, 1)
    }

    func testAnAppCanWriteItsOwnAdaptersAndRegisterThem() {
        struct NoSockets: WebSocketAdapter {
            func connect(url: String, protocols: [String], headers: [Header]) async throws(WsError) -> any WebSocketConnection {
                throw WsError.refused(status: nil, message: "offline build")
            }
        }
        struct NoStreams: SseAdapter {
            func open(url: String, headers: [Header], lastEventId: String?) async throws(SseError) -> any SseStream {
                throw SseError.refused(status: nil, message: "offline build")
            }
        }
        struct NoDatabases: DbAdapter {
            func open(name: String) async throws(DbError) -> any DbConnection {
                throw DbError.unavailable("offline build")
            }
        }
        let adapters = Adapters.platformDefault
            .replacing(WebSocketPortAdapter(NoSockets()))
            .replacing(SsePortAdapter(NoStreams()))
            .replacing(DbPortAdapter(NoDatabases(), busyTimeoutMs: 100))
        XCTAssertEqual(adapters.all.count, Adapters.platformDefault.all.count)
        XCTAssertTrue(adapters.all.contains { $0 is WebSocketPortAdapter })
        XCTAssertFalse(adapters.all.contains { $0 is URLSessionWebSocketAdapter })
        XCTAssertTrue(Adapters.platformDefault.all.contains { $0 is SQLiteDbAdapter })
        XCTAssertTrue(Adapters.platformDefault.all.contains { $0 is URLSessionSseAdapter })
        XCTAssertEqual(DbPortAdapter.defaultBusyTimeoutMs, 5_000)
        // The one parser is public, for an adapter that reads a body of its own.
        var parser = SseParser()
        XCTAssertEqual(try parser.push(Array("data: x\n\n".utf8)), [SseEvent(data: "x")])
    }
}
