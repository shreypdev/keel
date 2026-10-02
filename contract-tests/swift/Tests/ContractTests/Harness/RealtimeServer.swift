import Foundation

/// `contract-tests/servers/realtime-server.mjs`, the shared local server of S23 and S24, run with
/// Node (`--port 0 --exit-on-stdin-close`): it picks a free port, prints `READY <port>`, and exits
/// when this process lets go of its stdin, so a runner that dies takes it along.
final class RealtimeServer: @unchecked Sendable {
    /// One connection the server saw (its `/stats`).
    struct Connection: Decodable, Sendable {
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

    /// `WS` of scenarios.md: `ws://127.0.0.1:<port>`.
    var ws: String {
        return "ws://127.0.0.1:\(port)"
    }

    /// `HTTP` of scenarios.md: `http://127.0.0.1:<port>`.
    var http: String {
        return "http://127.0.0.1:\(port)"
    }

    /// The repository this file is in (contract-tests/swift/Tests/ContractTests/Harness/..).
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

    /// Starts the server and waits (15 s at most) for its `READY <port>` line.
    static func start() throws -> RealtimeServer {
        let script = repository.appendingPathComponent("contract-tests/servers/realtime-server.mjs")
        guard let node = node() else {
            throw ScenarioFailure(description: "no node on the PATH: S23 and S24 run the shared server with Node")
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
            line.withLock { (current: inout String) -> Void in current = text }
            ready.signal()
        }
        reader.start()
        guard ready.wait(timeout: .now() + 15) == .success,
              let first = line.snapshot.split(separator: "\n").first,
              first.hasPrefix("READY "), let port = Int(first.dropFirst("READY ".count))
        else {
            process.terminate()
            throw ScenarioFailure(description: "the realtime server did not print READY <port>: \(line.snapshot)")
        }
        return RealtimeServer(port: port, process: process, stdin: stdin)
    }

    /// Ends the server.
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
        return try await connections().last { (connection: Connection) -> Bool in connection.path == path }
    }

    /// Polls the newest connection to `path` every 10 ms until `condition` holds, and fails the
    /// scenario after `timeout`.
    func waitFor(
        _ path: String,
        _ what: String,
        timeout: Duration = .seconds(5),
        file: StaticString = #fileID,
        line: UInt = #line,
        _ condition: @Sendable (Connection) -> Bool
    ) async throws -> Connection {
        let deadline = ContinuousClock.now + timeout
        while true {
            if let connection = try await last(path), condition(connection) {
                return connection
            }
            if ContinuousClock.now >= deadline {
                let seen = try await last(path)
                throw ScenarioFailure(description: "timed out after \(timeout) waiting for \(what); the server saw \(String(describing: seen)) (\(file):\(line))")
            }
            try await Task.sleep(for: .milliseconds(10))
        }
    }
}
