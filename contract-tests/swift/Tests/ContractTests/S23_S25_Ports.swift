import Foundation
import UndraRuntime
import PlaygroundCore
import XCTest

extension ContractScenarios {
    // MARK: S23

    /// The opt-in `WebSocket` port (ADR-047) through the platform's default adapter
    /// (`URLSessionWebSocketAdapter`) against the shared local server; the core's side is `ws_echo`
    /// and `Live` (examples/playground/core/src/live.rs).
    func testS23_webSocket() async {
        await scenario("S23", "websocket") {
            let core = try self.core
            let server = try Fixture.shared.realtime()
            let ws = server.ws

            // 1. Echo: the three messages come back in order; the client closed with (1000, "done").
            let sent: [WsMessage] = [.text("a"), .binary([1, 2, 3]), .text("é")]
            let echoed = try await wsEcho(url: "\(ws)/ws/echo", messages: sent, ctx: core)
            try checkEqual(echoed, sent, "ws_echo(WS/ws/echo, [a, [1, 2, 3], é])")
            let echo = try await server.waitFor("/ws/echo", "the client's close of /ws/echo") { $0.closeCode != nil }
            try checkEqual(echo.closeCode, 1000, "the close code the server saw")
            try checkEqual(echo.closeReason, "done", "the close reason the server saw")

            // 2. Subprotocol and headers.
            let live = try Live(ctx: core)
            defer { live.close() }
            let negotiated = try await live.connect(
                url: "\(ws)/ws/headers",
                protocols: ["v2", "v1"],
                headers: [Header(name: "X-Token", value: "t")]
            )
            try checkEqual(negotiated, "v2", "the subprotocol the server chose")
            let upgrade = try await live.read(count: 1)
            guard upgrade.count == 1, case .text(let json) = upgrade[0],
                  let headers = try JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
            else {
                throw ScenarioFailure(description: "the first message of /ws/headers is not the JSON of the upgrade's headers: \(upgrade)")
            }
            try checkEqual(headers["x-token"] as? String, "t", "the X-Token header of the upgrade")
            try await live.send(.text("ping"))
            let pong = try await live.read(count: 1)
            try checkEqual(pong, [.text("ping")], "the echo of ping")
            try await live.disconnect(code: 4000, reason: "bye")
            let headersClose = try await server.waitFor("/ws/headers", "the client's close of /ws/headers") { $0.closeCode != nil }
            try checkEqual(headersClose.closeCode, 4000, "the close code of disconnect(4000, bye)")
            try checkEqual(headersClose.closeReason, "bye", "the close reason of disconnect(4000, bye)")

            // 3. Credit: a core that stopped reading has pulled at most twice.
            _ = try await live.connect(url: "\(ws)/ws/flood?n=1000", protocols: [], headers: [])
            let five = try await live.read(count: 5)
            try checkEqual(five, (0 ..< 5).map { WsMessage.text("\($0)") }, "read(5) of the flood")
            try await quietFor(milliseconds: 200)
            let pulls = try live.pulls()
            try check(pulls <= 2, "live.pulls() after read(5) and 200 ms of nothing is \(pulls), more than 2")
            let rest = try await live.read(count: 995)
            try checkEqual(rest.count, 995, "messages read by read(995)")
            try check(rest == (5 ..< 1000).map { WsMessage.text("\($0)") }, "read(995) is \"5\"..\"999\" in order")
            try await checkThrows({ try await live.read(count: 1) }, WsError.closed(code: 1000, reason: "end"), "read(1) after the flood")

            // 4. Typed ends.
            let denied = await outcome { () async throws -> [WsMessage] in
                try await wsEcho(url: "\(ws)/ws/deny?status=401", messages: [.text("x")], ctx: core)
            }
            switch denied {
            case .failure(WsError.refused(let status, _)):
                try checkEqual(status, 401, "the status of the refused upgrade")
            default:
                throw ScenarioFailure(description: "ws_echo(WS/ws/deny?status=401) is \(denied), not WsError.refused(401)")
            }
            _ = try await live.connect(url: "\(ws)/ws/close?code=4001&reason=kicked", protocols: [], headers: [])
            let hello = try await live.read(count: 1)
            try checkEqual(hello, [.text("hello")], "the first message of /ws/close")
            try await checkThrows({ try await live.read(count: 1) }, WsError.closed(code: 4001, reason: "kicked"), "read(1) after the server's close frame")
            _ = try await live.connect(url: "\(ws)/ws/drop", protocols: [], headers: [])
            let dropHello = try await live.read(count: 1)
            try checkEqual(dropHello, [.text("hello")], "the first message of /ws/drop")
            let dropped = await outcome { () async throws -> [WsMessage] in try await live.read(count: 1) }
            guard case .failure(WsError.network) = dropped else {
                throw ScenarioFailure(description: "read(1) after the drop is \(dropped), not WsError.network")
            }

            // 5. A connection nobody closes is closed going away (1001), within a second.
            _ = try await live.connect(url: "\(ws)/ws/stall", protocols: [], headers: [])
            live.abandon()
            let stalled = try await server.waitFor("/ws/stall", "the 1001 close of an abandoned connection", timeout: .seconds(1)) {
                $0.closeCode != nil
            }
            try checkEqual(stalled.closeCode, 1001, "the close code of an abandoned connection")
        }
    }

    // MARK: S24

    /// The opt-in `Sse` port (ADR-047) through the platform's default adapter
    /// (`URLSessionSseAdapter`) against the shared local server; the core's side is `sse_follow`.
    func testS24_serverSentEvents() async {
        await scenario("S24", "server-sent events") {
            let core = try self.core
            let server = try Fixture.shared.realtime()
            let http = server.http
            let feed = [
                SseEvent(id: "1", event: "message", data: "one", retryMs: 1500),
                SseEvent(id: "2", event: "tick", data: "two\nlines", retryMs: nil),
                SseEvent(id: "2", event: "message", data: "three", retryMs: nil),
                SseEvent(id: "4", event: "message", data: "four", retryMs: nil),
            ]

            // 1. The feed, parsed as the HTML standard says.
            let all = try await sseFollow(url: "\(http)/sse/feed", lastEventId: nil, max: 10, ctx: core)
            try checkEqual(all.ended, true, "sse_follow(feed, nil, 10).ended")
            try checkEqual(all.events, feed, "the events of the feed")
            let first = try require(try await server.last("/sse/feed"), "the server's record of /sse/feed")
            try checkEqual(first.headers["last-event-id"], nil, "the Last-Event-ID of a fresh subscription")

            // 2. Resume after id 2.
            let resumed = try await sseFollow(url: "\(http)/sse/feed", lastEventId: "2", max: 10, ctx: core)
            try checkEqual(resumed.ended, true, "sse_follow(feed, 2, 10).ended")
            try checkEqual(resumed.events, Array(feed.suffix(2)), "the events after id 2")
            let second = try require(try await server.last("/sse/feed"), "the server's record of the resume")
            try checkEqual(second.headers["last-event-id"], "2", "the Last-Event-ID of the resume")

            // 3. A reader that stops.
            let two = try await sseFollow(url: "\(http)/sse/feed", lastEventId: nil, max: 2, ctx: core)
            try checkEqual(two.events, Array(feed.prefix(2)), "sse_follow(feed, nil, 2).events")
            try checkEqual(two.ended, false, "sse_follow(feed, nil, 2).ended")
            let hang = try await sseFollow(url: "\(http)/sse/hang", lastEventId: nil, max: 0, ctx: core)
            try checkEqual(hang.events, [], "sse_follow(hang, nil, 0).events")
            try checkEqual(hang.ended, false, "sse_follow(hang, nil, 0).ended")
            _ = try await server.waitFor("/sse/hang", "the client leaving /sse/hang", timeout: .seconds(1)) { $0.clientClosed }

            // 4. Typed failures.
            for code in [204, 500] {
                let refused = await outcome { () async throws -> SseFollow in
                    try await sseFollow(url: "\(http)/sse/status?code=\(code)", lastEventId: nil, max: 10, ctx: core)
                }
                switch refused {
                case .failure(SseError.refused(let status, _)):
                    try checkEqual(status, UInt16(code), "the status of the refused stream")
                default:
                    throw ScenarioFailure(description: "sse_follow(status?code=\(code)) is \(refused), not SseError.refused(\(code))")
                }
            }
            let html = await outcome { () async throws -> SseFollow in
                try await sseFollow(url: "\(http)/sse/html", lastEventId: nil, max: 10, ctx: core)
            }
            guard case .failure(SseError.protocol) = html else {
                throw ScenarioFailure(description: "sse_follow(HTTP/sse/html) is \(html), not SseError.protocol")
            }
        }
    }

    // MARK: S25

    /// The opt-in `Db` port (ADR-048) through the platform's real SQLite adapter
    /// (`SQLiteDbAdapter`, the SQLite3 C API) rooted in a fresh temporary directory; the core's side
    /// is `Notes`, `db_cells`, `db_run` and `db_migrate` (examples/playground/core/src/notes.rs).
    func testS25_db() async {
        await scenario("S25", "db") {
            let core = try self.core
            let directory = Fixture.shared.databases
            defer { try? FileManager.default.removeItem(at: directory) }

            // 1. Open with two migrations.
            let notes = try Notes(ctx: core)
            defer { notes.close() }
            let version = try await notes.open(name: "contract-s25")
            try checkEqual(version, 2, "notes.open(contract-s25)")
            try checkEqual(notes.notes, [], "the notes of a new database")
            try check(
                FileManager.default.fileExists(atPath: directory.appendingPathComponent("contract-s25.sqlite").path),
                "the database file is in the adapter's directory"
            )

            // 2. Rows and the mirror: a keyed patch per add.
            let milk = try await notes.add(title: "milk")
            let eggs = try await notes.add(title: "eggs")
            try checkEqual([milk.id, eggs.id], [1, 2], "the ids of add(milk), add(eggs)")
            try await waitUntil("the mirror to hold both notes") { notes.notes.count == 2 }
            try checkEqual(notes.notes.map(\.title), ["milk", "eggs"], "the notes the mirror holds")
            try await notes.toggle(id: 1)
            try await waitUntil("note 1 to be done") { notes.notes.first?.done == true }
            let count = try await notes.count()
            try checkEqual(count, 2, "count()")
            let raw = try RawStore(core: core, type: UndraIds.Objects.Notes.typeId, method: UndraIds.Objects.Notes.new)
            defer { raw.close() }
            raw.observe()
            _ = try await raw.call(UndraIds.Objects.Notes.`open`, encoded { (w: inout UndraWriter) in w.writeString(":memory:") })
            _ = try await raw.call(UndraIds.Objects.Notes.add, encoded { (w: inout UndraWriter) in w.writeString("first") })
            try await waitUntil("the raw store's change for the first add") { !raw.entries(of: 0).isEmpty }
            raw.clear()
            _ = try await raw.call(UndraIds.Objects.Notes.add, encoded { (w: inout UndraWriter) in w.writeString("second") })
            try await waitUntil("the raw store's change for the second add") { !raw.entries(of: 0).isEmpty }
            // (The first add onto an empty list is a full value: the core's diff sends the full value
            // when no key overlaps the baseline. See NOTES.md.)
            try check(raw.entries(of: 0).allSatisfy { $0.op == .keyedPatch }, "add reaches the mirror as a keyed patch: \(raw.entries(of: 0).map(\.op))")

            // 3. Every storage class there and back.
            let cells = try await dbCells(int: -9_007_199_254_740_993, real: 1.5, text: "é😀", blob: [0, 255, 7], none: nil, ctx: core)
            try checkEqual(cells.int, -9_007_199_254_740_993, "the INTEGER")
            try checkEqual(cells.real, 1.5, "the REAL")
            try checkEqual(cells.text, "é😀", "the TEXT")
            try checkEqual(cells.blob, [0, 255, 7], "the BLOB")
            try checkEqual(cells.none, nil, "the NULL")
            try checkEqual(cells.types, ["integer", "real", "text", "blob", "null"], "typeof() of the five columns")

            // 4. Constraints, typed; a failed batch changes nothing.
            let duplicate = await outcome { () async throws -> Note in try await notes.addWithId(id: 1, title: "dup") }
            guard case .failure(DbError.constraint(kind: .unique, _)) = duplicate else {
                throw ScenarioFailure(description: "addWithId(1, dup) is \(duplicate), not DbError.constraint(.unique)")
            }
            let missing = await outcome { () async throws -> UInt32 in try await notes.addAll(titles: ["a", nil]) }
            guard case .failure(DbError.constraint(kind: .notNull, _)) = missing else {
                throw ScenarioFailure(description: "addAll([a, nil]) is \(missing), not DbError.constraint(.notNull)")
            }
            let afterFailures = try await notes.count()
            try checkEqual(afterFailures, 2, "count() after the failed batch (rolled back)")
            try checkEqual(notes.notes.count, 2, "the notes the mirror holds after the failed batch")
            let added = try await notes.addAll(titles: ["a", "b"])
            try checkEqual(added, 2, "addAll([a, b])")
            let four = try await notes.count()
            try checkEqual(four, 4, "count() after addAll([a, b])")

            // 5. SQL errors, typed.
            for sql in ["INSERT INTO missing VALUES (1)", "SELEC 1"] {
                let failed = await outcome { () async throws -> UInt64 in try await dbRun(name: ":memory:", sql: sql, ctx: core) }
                guard case .failure(DbError.sql) = failed else {
                    throw ScenarioFailure(description: "db_run(:memory:, \(sql)) is \(failed), not DbError.sql")
                }
            }

            // 6. Migrations run in one transaction.
            let broken = await outcome { () async throws -> UInt32 in try await dbMigrate(name: "contract-s25-m", broken: true, ctx: core) }
            guard case .failure(DbError.migration(version: 2, _)) = broken else {
                throw ScenarioFailure(description: "db_migrate(contract-s25-m, true) is \(broken), not DbError.migration(version: 2)")
            }
            let migrated = try await dbMigrate(name: "contract-s25-m", broken: false, ctx: core)
            try checkEqual(migrated, 2, "db_migrate(contract-s25-m, false) after the failed one")

            // 7. Persistence: a new store reads the same file.
            try await notes.closeDatabase()
            let reopened = try Notes(ctx: core)
            defer { reopened.close() }
            let again = try await reopened.open(name: "contract-s25")
            try checkEqual(again, 2, "open(contract-s25) again")
            try await waitUntil("the reopened store's notes") { reopened.notes.count == 4 }
            try checkEqual(reopened.notes.map(\.title), ["milk", "eggs", "a", "b"], "the notes of the reopened database")
            try checkEqual(reopened.notes.first?.done, true, "note 1 is still done")
            try await reopened.closeDatabase()

            // 8. A name that would leave the directory.
            let third = try Notes(ctx: core)
            defer { third.close() }
            let escaped = await outcome { () async throws -> UInt32 in try await third.open(name: "../escape") }
            guard case .failure(DbError.unavailable) = escaped else {
                throw ScenarioFailure(description: "open(../escape) is \(escaped), not DbError.unavailable")
            }
        }
    }
}
