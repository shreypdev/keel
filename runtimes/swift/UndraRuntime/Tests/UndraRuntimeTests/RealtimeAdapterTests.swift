import Foundation
import XCTest
@testable import UndraRuntime

// MARK: - The shared local server

/// `contract-tests/servers/realtime-server.mjs`, the server every runtime's WebSocket and Sse
/// adapters are tested against, run with Node: `--port 0 --exit-on-stdin-close`, so it picks a free
/// port (read from its `READY <port>` line) and exits when this process lets go of its stdin.
final class RealtimeServer: @unchecked Sendable {
    /// One connection the server saw (its `/stats`).
    struct Connection: Decodable {
        let id: Int
        let path: String
        let headers: [String: String]
        let protocols: [String]
        let closeCode: Int?
        let closeReason: String?
        let written: Int
        let clientClosed: Bool
    }

    private struct Stats: Decodable {
        let connections: [Connection]
    }

    let port: Int
    private let process: Process
    private let stdin: Pipe

    private init(port: Int, process: Process, stdin: Pipe) {
        self.port = port
        self.process = process
        self.stdin = stdin
    }

    /// `ws://127.0.0.1:<port>`.
    var ws: String {
        return "ws://127.0.0.1:\(port)"
    }

    /// `http://127.0.0.1:<port>`.
    var http: String {
        return "http://127.0.0.1:\(port)"
    }

    /// The repository this test file is in.
    static var repository: URL {
        var url = URL(fileURLWithPath: #filePath)
        for _ in 0 ..< 6 {
            url.deleteLastPathComponent()
        }
        return url
    }

    /// Node, from the PATH or where Homebrew and the installer put it.
    static func node() -> URL? {
        var candidates = (ProcessInfo.processInfo.environment["PATH"] ?? "")
            .split(separator: ":")
            .map { URL(fileURLWithPath: String($0)).appendingPathComponent("node") }
        candidates += ["/opt/homebrew/bin/node", "/usr/local/bin/node"].map { URL(fileURLWithPath: $0) }
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0.path) }
    }

    /// Starts the server. Without Node the test is skipped, unless `UNDRA_REQUIRE_TOOLCHAINS=1`
    /// makes a missing toolchain a failure.
    static func start() throws -> RealtimeServer {
        let script = repository.appendingPathComponent("contract-tests/servers/realtime-server.mjs")
        guard let node = node(), FileManager.default.fileExists(atPath: script.path) else {
            let why = "no node on the PATH, or no \(script.path): the realtime adapter suite needs both"
            if ProcessInfo.processInfo.environment["UNDRA_REQUIRE_TOOLCHAINS"] == "1" {
                throw ServerError(description: why)
            }
            throw XCTSkip(why)
        }
        let process = Process()
        process.executableURL = node
        process.arguments = [script.path, "--port", "0", "--exit-on-stdin-close"]
        let stdin = Pipe()
        let stdout = Pipe()
        process.standardInput = stdin
        process.standardOutput = stdout
        process.standardError = FileHandle.standardError
        try process.run()
        let ready = DispatchSemaphore(value: 0)
        let line = Locked("")
        let reader = Thread {
            var text = ""
            while !text.contains("\n") {
                let data = stdout.fileHandleForReading.availableData
                if data.isEmpty {
                    break
                }
                text += String(decoding: data, as: UTF8.self)
            }
            line.withLock { $0 = text }
            ready.signal()
        }
        reader.start()
        guard ready.wait(timeout: .now() + 15) == .success,
              let first = line.withLock({ $0 }).split(separator: "\n").first,
              first.hasPrefix("READY "), let port = Int(first.dropFirst("READY ".count))
        else {
            process.terminate()
            throw ServerError(description: "the realtime server did not print READY <port>: \(line.withLock { $0 })")
        }
        return RealtimeServer(port: port, process: process, stdin: stdin)
    }

    /// Ends the server (closing its stdin is enough; terminate makes sure).
    func stop() {
        try? stdin.fileHandleForWriting.close()
        process.terminate()
        process.waitUntilExit()
    }

    /// Every connection the server saw, newest last.
    func connections() async throws -> [Connection] {
        let (data, _) = try await URLSession.shared.data(from: URL(string: "\(http)/stats")!)
        return try JSONDecoder().decode(Stats.self, from: data).connections
    }

    /// The newest connection to `path`.
    func last(_ path: String) async throws -> Connection? {
        return try await connections().last { $0.path == path }
    }

    /// Polls the newest connection to `path` until `condition` holds (5 s).
    func waitFor(_ path: String, file: StaticString = #filePath, line: UInt = #line, _ condition: (Connection) -> Bool) async throws -> Connection? {
        let deadline = Date().addingTimeInterval(5)
        while Date() < deadline {
            if let connection = try await last(path), condition(connection) {
                return connection
            }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        XCTFail("the server never saw the expected state of \(path)", file: file, line: line)
        return try await last(path)
    }

    struct ServerError: Error, CustomStringConvertible {
        let description: String
    }
}

// MARK: - WebSocket against the server

/// The default WebSocket adapter (`URLSessionWebSocketAdapter`) behind the binding the core talks
/// to, against the shared server: the failure-injection suite of the ports-v2 brief §5.
final class URLSessionWebSocketAdapterTests: XCTestCase {
    private var server: RealtimeServer!
    private var binding: WebSocketBinding!

    override func setUpWithError() throws {
        if RealtimeAdapterServer.current == nil {
            RealtimeAdapterServer.current = try RealtimeServer.start()
        }
        server = RealtimeAdapterServer.current
        binding = WebSocketBinding(adapter: URLSessionWebSocketAdapter())
    }

    override func tearDown() {
        binding?.detach()
    }

    /// Reads until `count` messages arrived (or the stream ended, which throws).
    private func read(_ conn: UInt32, _ count: Int, max: UInt32 = 16) async throws(WsError) -> [WsMessage] {
        var all: [WsMessage] = []
        while all.count < count {
            let batch = try await binding.receive(conn: conn, max: Swift.min(max, UInt32(count - all.count)))
            if batch.isEmpty {
                break
            }
            all += batch
        }
        return all
    }

    func testEchoTextAndBinaryAndAClientClose() async throws {
        let conn = try await binding.connect(url: "\(server.ws)/ws/echo", protocols: [], headers: []).conn
        let sent: [WsMessage] = [.text("a"), .binary([1, 2, 3]), .text("é"), .binary([]), .text("")]
        for message in sent {
            try await binding.send(conn: conn, message: message)
        }
        let echoed = try await read(conn, sent.count)
        XCTAssertEqual(echoed, sent)
        try await binding.close(conn: conn, code: 1000, reason: "done")
        let seen = try await server.waitFor("/ws/echo") { $0.closeCode != nil }
        XCTAssertEqual(seen?.closeCode, 1000)
        XCTAssertEqual(seen?.closeReason, "done")
    }

    func testSubprotocolHeadersAndACloseCodeOfTheClients() async throws {
        let opened = try await binding.connect(
            url: "\(server.ws)/ws/headers",
            protocols: ["v2", "v1"],
            headers: [Header(name: "X-Token", value: "t")]
        )
        XCTAssertEqual(opened.protocol, "v2")
        let first = try await read(opened.conn, 1)
        guard case .text(let json)? = first.first,
              let headers = try JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
        else {
            return XCTFail("the first message is the upgrade's headers as JSON: \(first)")
        }
        XCTAssertEqual(headers["x-token"] as? String, "t")
        XCTAssertEqual(headers["sec-websocket-protocol"] as? String, "v2, v1")
        try await binding.send(conn: opened.conn, message: .text("ping"))
        let pong = try await read(opened.conn, 1)
        XCTAssertEqual(pong, [.text("ping")])
        try await binding.close(conn: opened.conn, code: 4000, reason: "bye")
        let seen = try await server.waitFor("/ws/headers") { $0.closeCode != nil }
        XCTAssertEqual(seen?.closeCode, 4000)
        XCTAssertEqual(seen?.closeReason, "bye")
        XCTAssertEqual(seen?.protocols, ["v2", "v1"])
    }

    func testARefusedUpgradeCarriesItsStatus() async throws {
        do {
            _ = try await binding.connect(url: "\(server.ws)/ws/deny?status=401", protocols: [], headers: [])
            XCTFail("the upgrade was refused")
        } catch {
            guard case .refused(let status, let message) = error else {
                return XCTFail("\(error)")
            }
            XCTAssertEqual(status, 401)
            XCTAssertTrue(message.contains("401"), message)
        }
        do {
            _ = try await binding.connect(url: "ws://127.0.0.1:1/nothing", protocols: [], headers: [])
            XCTFail("nothing listens on port 1")
        } catch {
            guard case .network = error else {
                return XCTFail("a connection that cannot be made is a network error: \(error)")
            }
        }
        await expectThrows(WsError.refused(status: nil, message: "invalid URL: ws://")) { () async throws(WsError) -> any WebSocketConnection in
            try await URLSessionWebSocketAdapter().connect(url: "ws://", protocols: [], headers: [])
        }
    }

    func testThePeersCloseFrameIsClosedWithItsCodeAndReason() async throws {
        let conn = try await binding.connect(url: "\(server.ws)/ws/close?code=4001&reason=kicked", protocols: [], headers: []).conn
        let hello = try await read(conn, 1)
        XCTAssertEqual(hello, [.text("hello")])
        await expectThrows(WsError.closed(code: 4001, reason: "kicked")) { () async throws(WsError) -> [WsMessage] in
            try await binding.receive(conn: conn, max: 16)
        }
        await expectThrows(WsError.closed(code: 4001, reason: "kicked")) { () async throws(WsError) -> Void in
            try await binding.send(conn: conn, message: .text("late"))
        }
    }

    func testAnAbruptDropIsANetworkError() async throws {
        let conn = try await binding.connect(url: "\(server.ws)/ws/drop", protocols: [], headers: []).conn
        let hello = try await read(conn, 1)
        XCTAssertEqual(hello, [.text("hello")])
        do {
            _ = try await binding.receive(conn: conn, max: 16)
            XCTFail("the server dropped the connection")
        } catch {
            guard case .network = error else {
                return XCTFail("a drop without a close frame is a network error: \(error)")
            }
        }
    }

    func testATextFrameThatIsNotUtf8IsAProtocolError() async throws {
        let conn = try await binding.connect(url: "\(server.ws)/ws/bad-utf8", protocols: [], headers: []).conn
        do {
            _ = try await binding.receive(conn: conn, max: 16)
            XCTFail("the frame is not UTF-8")
        } catch {
            guard case .protocol = error else {
                return XCTFail("an invalid text frame is a protocol error: \(error)")
            }
        }
    }

    func testAStalledReaderStopsTheServerThenGetsEverythingInOrder() async throws {
        let total = 2000
        let conn = try await binding.connect(url: "\(server.ws)/ws/flood?n=\(total)&size=65536", protocols: [], headers: []).conn
        let first = try await binding.receive(conn: conn, max: 16)
        XCTAssertFalse(first.isEmpty)
        // The core stops reading: the binding reads ahead one window, URLSession stops asking the
        // socket, and TCP pushes back on the server.
        try await Task.sleep(nanoseconds: 1_000_000_000)
        let stalled = try await server.last("/ws/flood")
        let written = try XCTUnwrap(stalled?.written)
        XCTAssertLessThan(written, 200, "the server could write \(written) of \(total) messages to a stalled reader")
        var received = first
        while true {
            do {
                received += try await binding.receive(conn: conn, max: 16)
            } catch {
                XCTAssertEqual(error, .closed(code: 1000, reason: "end"))
                break
            }
        }
        XCTAssertEqual(received.count, total)
        for (index, message) in received.enumerated() {
            guard case .text(let text) = message, text.hasPrefix("\(index).") || text == "\(index)", text.utf8.count == 65536 else {
                return XCTFail("message \(index) is out of order or the wrong size")
            }
        }
    }

    func testAConnectionTheCoreLetsGoIsClosedGoingAway() async throws {
        let conn = try await binding.connect(url: "\(server.ws)/ws/stall", protocols: [], headers: []).conn
        try await binding.close(conn: conn, code: 1001, reason: "")
        let seen = try await server.waitFor("/ws/stall") { $0.closeCode != nil }
        XCTAssertEqual(seen?.closeCode, 1001)
    }
}

/// The server the realtime suites share for the life of the test process.
enum RealtimeAdapterServer {
    nonisolated(unsafe) static var current: RealtimeServer?
}

// MARK: - Sse against the server

/// The default Sse adapter (`URLSessionSseAdapter`) behind the binding, against the shared server.
final class URLSessionSseAdapterTests: XCTestCase {
    private var server: RealtimeServer!
    private var binding: SseBinding!

    override func setUpWithError() throws {
        if RealtimeAdapterServer.current == nil {
            RealtimeAdapterServer.current = try RealtimeServer.start()
        }
        server = RealtimeAdapterServer.current
        binding = SseBinding(adapter: URLSessionSseAdapter())
    }

    override func tearDown() {
        binding?.detach()
    }

    /// Every event until the stream ends; the end is returned too.
    private func drain(_ stream: UInt32) async -> ([SseEvent], SseError) {
        var events: [SseEvent] = []
        while true {
            do {
                events += try await binding.next(stream: stream, max: 16)
            } catch {
                return (events, error)
            }
        }
    }

    func testTheFeedParsesAsTheStandardSaysAndEnds() async throws {
        let stream = try await binding.open(url: "\(server.http)/sse/feed", headers: [Header(name: "X-Token", value: "t")], lastEventId: nil)
        let (events, end) = await drain(stream)
        XCTAssertEqual(events, SseParserTests.feedEvents)
        XCTAssertEqual(end, .ended)
        let seen = try await server.last("/sse/feed")
        XCTAssertNil(seen?.headers["last-event-id"])
        XCTAssertEqual(seen?.headers["accept"], "text/event-stream")
        XCTAssertEqual(seen?.headers["cache-control"], "no-cache")
        XCTAssertEqual(seen?.headers["x-token"], "t")
    }

    func testAResumeSendsTheLastEventIdAndKeepsItForEventsWithoutOne() async throws {
        let stream = try await binding.open(url: "\(server.http)/sse/feed", headers: [], lastEventId: "2")
        let (events, end) = await drain(stream)
        XCTAssertEqual(events, Array(SseParserTests.feedEvents.suffix(2)))
        XCTAssertEqual(end, .ended)
        let seen = try await server.last("/sse/feed")
        XCTAssertEqual(seen?.headers["last-event-id"], "2")
    }

    func testRefusalsCarryTheStatusAndAWrongContentTypeIsAProtocolError() async throws {
        for code in [204, 500] {
            do {
                _ = try await binding.open(url: "\(server.http)/sse/status?code=\(code)", headers: [], lastEventId: nil)
                XCTFail("\(code) is refused")
            } catch {
                guard case .refused(let status, _) = error else {
                    return XCTFail("\(code): \(error)")
                }
                XCTAssertEqual(status, UInt16(code))
            }
        }
        await expectThrows(SseError.protocol("expected text/event-stream, got text/html")) { () async throws(SseError) -> UInt32 in
            try await binding.open(url: "\(server.http)/sse/html", headers: [], lastEventId: nil)
        }
        do {
            _ = try await binding.open(url: "http://127.0.0.1:1/sse", headers: [], lastEventId: nil)
            XCTFail("nothing listens on port 1")
        } catch {
            guard case .network = error else {
                return XCTFail("\(error)")
            }
        }
    }

    func testClosingAHangingStreamLetsTheServerSeeTheClientLeave() async throws {
        let stream = try await binding.open(url: "\(server.http)/sse/hang", headers: [], lastEventId: nil)
        let binding = self.binding!
        let waiting = Task { try await binding.next(stream: stream, max: 16) }
        try await Task.sleep(nanoseconds: 100_000_000)
        let open = try await server.last("/sse/hang")
        XCTAssertEqual(open?.clientClosed, false)
        try await binding.close(stream: stream)
        let answered = try await waiting.value
        XCTAssertEqual(answered, [])
        let started = Date()
        let left = try await server.waitFor("/sse/hang") { $0.clientClosed }
        XCTAssertEqual(left?.clientClosed, true)
        XCTAssertLessThan(Date().timeIntervalSince(started), 1.0)
    }

    func testAFloodIsReadAsTheCorePulls() async throws {
        let stream = try await binding.open(url: "\(server.http)/sse/flood?n=500&size=100", headers: [], lastEventId: nil)
        let (events, end) = await drain(stream)
        XCTAssertEqual(end, .ended)
        XCTAssertEqual(events.count, 500)
        XCTAssertEqual(events.map(\.id), (0 ..< 500).map { Optional("\($0)") })
    }
}
