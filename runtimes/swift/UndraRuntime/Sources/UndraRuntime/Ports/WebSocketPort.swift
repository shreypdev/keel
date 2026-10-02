// The `WebSocket` port (ADR-047): the adapter interface an app implements to replace the default
// (`WebSocketAdapter`, `WebSocketConnection`) and the binding that serves the port from one
// (`WebSocketPortAdapter`): it owns the connection ids, the core's pull and the read-ahead.
//
//   connect(url, protocols, headers) -> WsOpened      receive(conn, max) -> [WsMessage]
//   send(conn, message)                               close(conn, code, reason)

/// Opens WebSocket connections for the `WebSocket` port (ADR-047).
///
/// Implement it to replace ``URLSessionWebSocketAdapter`` (a socket library, a proxy, a test
/// server) and register it with ``WebSocketPortAdapter``:
///
/// ```swift
/// let adapters = Adapters.platformDefault.replacing(WebSocketPortAdapter(MyWebSocket()))
/// ```
///
/// The binding owns everything the core sees (ids, the pull, the read-ahead, the order of
/// closes); an adapter only opens connections and moves messages.
public protocol WebSocketAdapter: Sendable {
    /// Opens a connection to `url` (`ws://` or `wss://`, checked by the binding), offering
    /// `protocols` and sending `headers` with the upgrade request. Returns once the handshake
    /// succeeded; a refused upgrade is ``WsError/refused(status:message:)``, a connection that
    /// could not be made ``WsError/network(_:)``.
    func connect(url: String, protocols: [String], headers: [Header]) async throws(WsError) -> any WebSocketConnection
}

/// One open WebSocket connection, as an adapter provides it.
public protocol WebSocketConnection: Sendable {
    /// The subprotocol the server selected, or `""` when none was negotiated.
    var negotiatedProtocol: String { get }

    /// The inbound messages, pulled: the binding asks for the next one only while the core's
    /// window has room, so the stream must be lazy (`AsyncThrowingStream(unfolding:)` over a
    /// receive call) for TCP to push back on a fast server. It ends by throwing a ``WsError``
    /// (`closed` with the peer's close frame, `network` for a drop, `protocol` for a broken
    /// frame), or by finishing after ``close(code:reason:)``. The binding reads it once.
    var messages: AsyncThrowingStream<WsMessage, any Error> { get }

    /// Sends `message`; returns when the platform accepted it.
    func send(_ message: WsMessage) async throws(WsError)

    /// Starts the closing handshake with `code` and `reason` and lets the connection go.
    func close(code: UInt16, reason: String) async
}

/// Serves the `WebSocket` port with a ``WebSocketAdapter`` (ADR-047).
///
/// * `connect` checks the URL (`ws://` or `wss://`, else `refused(status: nil, "invalid URL:
///   <url>")`), asks the adapter and registers the connection under the next id (from 1, never
///   reused).
/// * A pump per connection reads the adapter's `messages` into a buffer only while it holds fewer
///   than the `max` of the core's latest `receive` (16 before the first), and records how the
///   stream ended: its `WsError`, or `network("the connection ended")` when it finished although
///   the core did not close it.
/// * `receive(conn, max)` answers up to `max` buffered messages at once, `[]` after the core
///   closed the connection, else the stream's end (again on every later call), else waits. One
///   `receive` may be pending per connection. A burst still arriving is one answer: a `receive`
///   holding fewer than `max` messages waits until `max` are there, no message came for 2 ms, or
///   8 ms passed since its first one (ADR-047 §3, "a burst of 16 frames is one crossing").
/// * `send` after the core's `close` fails with that close; after the stream's end, with the end.
/// * `close` answers a pending `receive` with `[]`, drops the buffer, stops the pump and closes the
///   adapter's connection; closing again is not an error.
/// * When the core shuts down, every connection still open is closed with 1001 (going away).
public final class WebSocketPortAdapter: UndraAdapter, @unchecked Sendable {
    private let adapter: any WebSocketAdapter
    private let bindings = BindingSet<WebSocketBinding>()

    /// Serves the port with `adapter`.
    public init(_ adapter: any WebSocketAdapter) {
        self.adapter = adapter
    }

    public var portId: UInt32 {
        return StandardPorts.WebSocket.portId
    }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return bindings.add(WebSocketBinding(adapter: adapter)).portImpl()
    }

    public func detach() {
        bindings.detachAll()
    }
}

/// The bindings an adapter made, one per core it was registered with, so `detach()` reaches them.
final class BindingSet<Binding: DetachableBinding>: @unchecked Sendable {
    private let all = Guarded<[Binding]>([])

    /// Remembers `binding` and returns it.
    func add(_ binding: Binding) -> Binding {
        all.withLock { (list: inout [Binding]) -> Void in
            list.append(binding)
        }
        return binding
    }

    /// Detaches every binding and forgets them.
    func detachAll() {
        let list = all.withLock { (list: inout [Binding]) -> [Binding] in
            let taken = list
            list = []
            return taken
        }
        for binding in list {
            binding.detach()
        }
    }
}

/// A binding that releases what it holds when its core shuts down.
protocol DetachableBinding: AnyObject, Sendable {
    func detach()
}

/// The state of the `WebSocket` port of one core.
final class WebSocketBinding: DetachableBinding, @unchecked Sendable {
    /// One open connection: the adapter's and its inbox.
    final class Entry: Sendable {
        let connection: any WebSocketConnection
        let inbox: PulledInbox<WsMessage, WsError>

        init(_ connection: any WebSocketConnection) {
            self.connection = connection
            self.inbox = PulledInbox(
                source: connection.messages,
                mapError: { (error: any Error) -> WsError in
                    return (error as? WsError) ?? .network(String(describing: error))
                },
                finished: .network("the connection ended")
            )
        }
    }

    /// What an id names.
    private enum Lookup {
        /// An open connection.
        case open(Entry)
        /// A connection the core closed, with the code and reason it closed it with.
        case closed(code: UInt16, reason: String)
    }

    private struct State {
        var lastId: UInt32 = 0
        var open: [UInt32: Entry] = [:]
        var closed: [UInt32: (code: UInt16, reason: String)] = [:]
        var detached = false
    }

    private let adapter: any WebSocketAdapter
    private let state = Guarded(State())

    init(adapter: any WebSocketAdapter) {
        self.adapter = adapter
    }

    /// The method table of the port.
    func portImpl() -> PortImpl {
        return .async([
            StandardPorts.WebSocket.connect: { [self] (args: [UInt8]) async throws -> [UInt8] in
                var reader = UndraReader(args)
                let url = try reader.readString()
                let protocols = try [String].undraDecode(&reader)
                let headers = try [Header].undraDecode(&reader)
                try reader.finish()
                return try await answer { () async throws(WsError) -> WsOpened in
                    try await self.connect(url: url, protocols: protocols, headers: headers)
                }
            },
            StandardPorts.WebSocket.send: { [self] (args: [UInt8]) async throws -> [UInt8] in
                var reader = UndraReader(args)
                let conn = try reader.readU32()
                let message = try WsMessage.undraDecode(&reader)
                try reader.finish()
                return try await answerUnit { () async throws(WsError) -> Void in
                    try await self.send(conn: conn, message: message)
                }
            },
            StandardPorts.WebSocket.receive: { [self] (args: [UInt8]) async throws -> [UInt8] in
                var reader = UndraReader(args)
                let conn = try reader.readU32()
                let max = try reader.readU32()
                try reader.finish()
                return try await answer { () async throws(WsError) -> [WsMessage] in
                    try await self.receive(conn: conn, max: max)
                }
            },
            StandardPorts.WebSocket.close: { [self] (args: [UInt8]) async throws -> [UInt8] in
                var reader = UndraReader(args)
                let conn = try reader.readU32()
                let code = try reader.readU16()
                let reason = try reader.readString()
                try reader.finish()
                return try await answerUnit { () async throws(WsError) -> Void in
                    try await self.close(conn: conn, code: code, reason: reason)
                }
            },
        ])
    }

    // MARK: The port's methods

    func connect(url: String, protocols: [String], headers: [Header]) async throws(WsError) -> WsOpened {
        let lowered = url.lowercased()
        guard lowered.hasPrefix("ws://") || lowered.hasPrefix("wss://") else {
            throw WsError.refused(status: nil, message: "invalid URL: \(url)")
        }
        let connection = try await adapter.connect(url: url, protocols: protocols, headers: headers)
        let entry = Entry(connection)
        let id = state.withLock { (current: inout State) -> UInt32? in
            if current.detached {
                return nil
            }
            current.lastId += 1
            current.open[current.lastId] = entry
            return current.lastId
        }
        guard let id = id else {
            // The core shut down while the handshake ran.
            entry.inbox.close()
            await connection.close(code: 1001, reason: "")
            throw WsError.network("the core shut down")
        }
        return WsOpened(conn: id, protocol: connection.negotiatedProtocol)
    }

    func send(conn: UInt32, message: WsMessage) async throws(WsError) {
        switch try find(conn) {
        case .closed(let code, let reason):
            throw WsError.closed(code: code, reason: reason)
        case .open(let entry):
            if let terminal = entry.inbox.terminal {
                throw terminal
            }
            try await entry.connection.send(message)
        }
    }

    func receive(conn: UInt32, max: UInt32) async throws(WsError) -> [WsMessage] {
        switch try find(conn) {
        case .closed:
            return []
        case .open(let entry):
            let pending = WsError.protocol("a receive is already pending on connection \(conn)")
            return try await entry.inbox.pull(max: max, alreadyPending: pending).get()
        }
    }

    func close(conn: UInt32, code: UInt16, reason: String) async throws(WsError) {
        let entry = state.withLock { (current: inout State) -> Lookup? in
            if let entry = current.open.removeValue(forKey: conn) {
                current.closed[conn] = (code, reason)
                return .open(entry)
            }
            if let closed = current.closed[conn] {
                return .closed(code: closed.code, reason: closed.reason)
            }
            return nil
        }
        switch entry {
        case nil:
            throw WsError.network("no WebSocket connection \(conn)")
        case .closed?:
            return
        case .open(let entry)?:
            entry.inbox.close()
            await entry.connection.close(code: code, reason: reason)
        }
    }

    /// The core shut down: every connection still open is closed with 1001 (going away).
    func detach() {
        let entries = state.withLock { (current: inout State) -> [Entry] in
            current.detached = true
            let open = Array(current.open.values)
            for id in current.open.keys {
                current.closed[id] = (1001, "")
            }
            current.open = [:]
            return open
        }
        for entry in entries {
            entry.inbox.close()
            Task {
                await entry.connection.close(code: 1001, reason: "")
            }
        }
    }

    /// How connection `conn` ended on the platform side, if it is open and did (diagnostics).
    func terminal(of conn: UInt32) -> WsError? {
        let entry = state.withLock { (current: inout State) -> Entry? in
            return current.open[conn]
        }
        return entry?.inbox.terminal
    }

    private func find(_ conn: UInt32) throws(WsError) -> Lookup {
        let found = state.withLock { (current: inout State) -> Lookup? in
            if let entry = current.open[conn] {
                return .open(entry)
            }
            if let closed = current.closed[conn] {
                return .closed(code: closed.code, reason: closed.reason)
            }
            return nil
        }
        guard let found = found else {
            throw WsError.network("no WebSocket connection \(conn)")
        }
        return found
    }
}

// MARK: - Answering with a typed error

/// Runs `body` and encodes its value; its typed error becomes the port's typed error (status 1).
func answer<Value: UndraCodec, Failure: UndraError>(
    _ body: () async throws(Failure) -> Value
) async throws -> [UInt8] {
    do throws(Failure) {
        return try await body().undraEncoded()
    } catch {
        throw UndraPortError(body: error.undraEncoded())
    }
}

/// Runs `body`, which answers nothing (`()`); its typed error becomes the port's typed error.
func answerUnit<Failure: UndraError>(
    _ body: () async throws(Failure) -> Void
) async throws -> [UInt8] {
    do throws(Failure) {
        try await body()
        return []
    } catch {
        throw UndraPortError(body: error.undraEncoded())
    }
}
