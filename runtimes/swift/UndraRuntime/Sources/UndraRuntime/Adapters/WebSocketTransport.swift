// The remote transport: a core running elsewhere (`undra dev`), reached over a WebSocket that
// carries the envelope of docs/SPEC.md section 3.2 (one envelope per binary message).
//
// This file imports Foundation for `URLSessionWebSocketTask`; it is the only transport file that
// does. The socket sits behind `SocketConnection`, and the protocol logic (`handleFrame`,
// `sendFrame`) is separate from it, so tests drive the transport with a scripted connector or a
// frame sink.

import Foundation

// MARK: - The socket seam

/// One message of a WebSocket.
enum SocketMessage: Sendable {
    case binary([UInt8])
    case text
}

/// One WebSocket connection, as the transport sees it.
protocol SocketConnection: AnyObject, Sendable {
    /// Starts connecting.
    func resume()
    /// Sends one binary message; `completion` hears of a failure.
    func send(_ frame: [UInt8], completion: @escaping @Sendable ((any Error)?) -> Void)
    /// Waits for the next message (one call, one message).
    func receive(_ handler: @escaping @Sendable (Result<SocketMessage, any Error>) -> Void)
    /// Ends the connection.
    func cancel()
    /// The close code and reason the server sent, once it did.
    var peerClose: (code: Int, reason: String)? { get }
}

/// Opens connections.
protocol SocketConnector: Sendable {
    func makeConnection(url: URL) -> any SocketConnection
}

/// The real thing: `URLSessionWebSocketTask`.
final class URLSessionSocketConnector: SocketConnector, @unchecked Sendable {
    private let session = URLSession(configuration: .default)

    func makeConnection(url: URL) -> any SocketConnection {
        return TaskConnection(task: session.webSocketTask(with: url))
    }
}

private final class TaskConnection: SocketConnection, @unchecked Sendable {
    private let task: URLSessionWebSocketTask

    init(task: URLSessionWebSocketTask) {
        self.task = task
        // The default is 1 MiB; a change-set can be larger.
        task.maximumMessageSize = 64 * 1024 * 1024
    }

    func resume() {
        task.resume()
    }

    func send(_ frame: [UInt8], completion: @escaping @Sendable ((any Error)?) -> Void) {
        task.send(.data(Data(frame)), completionHandler: completion)
    }

    func receive(_ handler: @escaping @Sendable (Result<SocketMessage, any Error>) -> Void) {
        task.receive { result in
            switch result {
            case .failure(let error):
                handler(.failure(error))
            case .success(.data(let data)):
                handler(.success(.binary([UInt8](data))))
            case .success(.string):
                handler(.success(.text))
            @unknown default:
                handler(.success(.text))
            }
        }
    }

    func cancel() {
        task.cancel(with: .goingAway, reason: nil)
    }

    var peerClose: (code: Int, reason: String)? {
        let code = task.closeCode.rawValue
        guard code != 0 else {
            return nil
        }
        let reason = task.closeReason.flatMap { String(data: $0, encoding: .utf8) } ?? ""
        return (code, reason)
    }
}

// MARK: - The reconnect clock

/// A scheduled piece of work that can be called off.
protocol ReconnectToken: Sendable {
    func cancel()
}

/// Runs the transport's reconnect attempts after their backoff. Tests replace it with one they fire by hand.
protocol ReconnectScheduler: Sendable {
    func schedule(after seconds: Double, _ work: @escaping @Sendable () -> Void) -> any ReconnectToken
}

/// The wall clock, on a serial queue of its own (an attempt blocks for its handshake).
final class DispatchReconnectScheduler: ReconnectScheduler, @unchecked Sendable {
    private let queue = DispatchQueue(label: "dev.undra.runtime.reconnect")

    private final class Token: ReconnectToken, @unchecked Sendable {
        let item: DispatchWorkItem

        init(_ item: DispatchWorkItem) {
            self.item = item
        }

        func cancel() {
            item.cancel()
        }
    }

    func schedule(after seconds: Double, _ work: @escaping @Sendable () -> Void) -> any ReconnectToken {
        let item = DispatchWorkItem(block: work)
        queue.asyncAfter(deadline: .now() + max(0, seconds), execute: item)
        return Token(item)
    }
}

// MARK: - The transport

/// Speaks the envelope protocol to a dev core over a WebSocket.
///
/// The handshake: the host sends `Hello` (announcing the schema hash the bindings expect), the
/// core answers with its own `Hello`, and `start` returns the core's hash for `UndraCore` to
/// compare. Every later envelope carries the core's hash, and one that does not disconnects.
///
/// **Reconnecting** (ADR-034). With an `UndraReconnectPolicy`, a connection that drops is not the
/// end: the transport tells its core (`onReconnecting`, and what was in flight fails), waits the
/// policy's backoff, connects again and tells the core (`onReconnected`), which observes its
/// stores again. The app shutting the core down, a core with another schema, a session the server
/// lost and a server that breaks the protocol are final (`onDisconnect`). Every connection carries
/// the same session token in its URL (and `undra_resume=1` when the core holds objects), so
/// `undra dev` can keep the objects of a client that dropped and give them back to it.
final class WebSocketTransport: UndraTransport, @unchecked Sendable {
    private struct State {
        var inbound: (any UndraInbound)? = nil
        var connection: (any SocketConnection)? = nil
        /// Counts connections, so that a late callback of an old one is recognised and dropped.
        var generation = 0
        var handshake: OneShot<Result<TransportInfo, any Error>>? = nil
        var nextSeq: UInt32 = 0
        /// The schema hash the bindings expect, until the core's `Hello` (which must carry the same).
        var schemaHash: UInt64 = 0
        var handshakeDone = false
        /// Closed for good: `shutdown()`, or a final failure.
        var isShutDown = false
        var reconnecting = false
        var pendingAttempt: (any ReconnectToken)? = nil
        var platform = "ios"
        var connectTimeout: Double = 10
    }

    /// The close code with which the dev server says it no longer holds this client's session.
    private static let sessionLostCode = 4001

    /// A reconnect attempt (connect and `Hello`) takes at most this long, whatever the blocking timeout is.
    private static let attemptCap: Double = 5

    private let url: URL?
    private let connector: (any SocketConnector)?
    private let reconnect: UndraReconnectPolicy?
    private let session: String?
    private let scheduler: any ReconnectScheduler
    private let sink: (@Sendable ([UInt8]) -> Void)?
    private let state = Guarded<State>(State())
    /// Delivers what the core is told about reconnecting in order, whichever thread noticed it.
    private let events = DispatchQueue(label: "dev.undra.runtime.reconnect-events")

    /// Creates a transport for a `ws://` or `wss://` URL.
    ///
    /// - Throws: `UndraLoadError.invalidURL`.
    convenience init(
        urlString: String,
        reconnect: UndraReconnectPolicy? = nil,
        session: String? = nil
    ) throws {
        guard let url = URL(string: urlString),
              let scheme = url.scheme?.lowercased(),
              scheme == "ws" || scheme == "wss",
              let host = url.host,
              !host.isEmpty
        else {
            throw UndraLoadError.invalidURL(urlString)
        }
        self.init(url: url, reconnect: reconnect, session: session)
    }

    /// Creates a transport for `url`, which must be a `ws://` or `wss://` URL.
    init(
        url: URL,
        connector: any SocketConnector = URLSessionSocketConnector(),
        reconnect: UndraReconnectPolicy? = nil,
        session: String? = nil,
        scheduler: any ReconnectScheduler = DispatchReconnectScheduler()
    ) {
        self.url = url
        self.connector = connector
        self.reconnect = reconnect
        self.session = session
        self.scheduler = scheduler
        self.sink = nil
    }

    /// Creates a transport that writes every outgoing frame to `sink` instead of a socket, for
    /// tests of the protocol logic. Incoming frames are fed to `handleFrame(_:)`.
    init(frameSink: @escaping @Sendable ([UInt8]) -> Void) {
        self.url = nil
        self.connector = nil
        self.reconnect = nil
        self.session = nil
        self.scheduler = DispatchReconnectScheduler()
        self.sink = frameSink
    }

    var mode: UndraMode {
        return .remote
    }

    var supportsDirectSync: Bool {
        return false
    }

    // MARK: Start and stop

    func start(inbound: any UndraInbound, options: TransportStartOptions) throws -> TransportInfo {
        state.withLock { (current: inout State) -> Void in
            current.inbound = inbound
            current.schemaHash = options.expectedSchemaHash
            current.platform = options.platform
            current.connectTimeout = options.connectTimeout
        }
        switch openConnection(resume: false, timeout: options.connectTimeout) {
        case .success(let info):
            return info
        case .failure(let error):
            closeSocket()
            throw error
        }
    }

    /// Opens a connection, sends `Hello` and waits for the core's. Blocks for up to `timeout`
    /// seconds: call it from a thread that may wait (`start` is one; so is a reconnect attempt).
    /// A failure leaves no connection open.
    private func openConnection(resume: Bool, timeout: Double) -> Result<TransportInfo, any Error> {
        let waiter = OneShot<Result<TransportInfo, any Error>>()
        let opened = state.withLock { (current: inout State) -> (connection: (any SocketConnection)?, generation: Int, platform: String, hash: UInt64) in
            current.generation += 1
            current.handshake = waiter
            current.handshakeDone = false
            current.isShutDown = false
            current.nextSeq = 0
            let connection = self.url.flatMap { target in
                self.connector?.makeConnection(url: self.urlFor(target, resume: resume))
            }
            current.connection = connection
            return (connection, current.generation, current.platform, current.schemaHash)
        }
        if let connection = opened.connection {
            connection.resume()
            receiveNext(from: connection, generation: opened.generation)
        }
        let hello = Wire.Hello(
            undraVersion: UndraCore.undraVersion,
            schemaHash: opened.hash,
            platform: opened.platform,
            mode: "dev"
        )
        sendFrame(kind: .hello, payload: hello.encode())
        guard let outcome = waiter.wait(timeoutSeconds: timeout) else {
            dropConnection(generation: opened.generation)
            return .failure(UndraLoadError.handshakeTimedOut(seconds: timeout))
        }
        if case .failure = outcome {
            dropConnection(generation: opened.generation)
        }
        return outcome
    }

    /// The URL of one connection: the core's, with the session token and, when objects are to be found again, the resume flag.
    private func urlFor(_ target: URL, resume: Bool) -> URL {
        guard let session = session, var components = URLComponents(url: target, resolvingAgainstBaseURL: false) else {
            return target
        }
        var items = components.queryItems ?? []
        items.append(URLQueryItem(name: "undra_session", value: session))
        if resume {
            items.append(URLQueryItem(name: "undra_resume", value: "1"))
        }
        components.queryItems = items
        if components.path.isEmpty {
            components.path = "/"
        }
        return components.url ?? target
    }

    func shutdown() {
        closeSocket()
    }

    /// Closes for good: no connection, no attempt scheduled, nothing more is sent.
    private func closeSocket() {
        let (connection, pending) = state.withLock { (current: inout State) -> ((any SocketConnection)?, (any ReconnectToken)?) in
            current.isShutDown = true
            current.reconnecting = false
            let existing = current.connection
            current.connection = nil
            let attempt = current.pendingAttempt
            current.pendingAttempt = nil
            return (existing, attempt)
        }
        pending?.cancel()
        connection?.cancel()
    }

    /// Lets go of connection `generation` (a failed attempt) without closing the transport.
    private func dropConnection(generation: Int) {
        let connection = state.withLock { (current: inout State) -> (any SocketConnection)? in
            guard current.generation == generation else {
                return nil
            }
            let existing = current.connection
            current.connection = nil
            current.handshake = nil
            current.handshakeDone = false
            return existing
        }
        connection?.cancel()
    }

    // MARK: Outgoing

    func send(call payload: [UInt8]) -> Bool {
        return sendFrame(kind: .call, payload: payload)
    }

    func callSync(_ payload: [UInt8]) throws -> [UInt8] {
        throw UndraModeError(operation: "inline callSync", mode: .remote)
    }

    func cancel(callId: UInt32) {
        sendFrame(kind: .cancel, payload: Wire.Cancel(callId: callId).encode())
    }

    func streamCredit(callId: UInt32, credit: UInt32) {
        sendFrame(kind: .streamCredit, payload: Wire.StreamCredit(callId: callId, credit: credit).encode())
    }

    func observe(handle: UndraHandle, signal: UInt32, on: Bool) {
        sendFrame(kind: .observe, payload: Wire.Observe(handle: handle, signalId: signal, on: on).encode())
    }

    func release(handle: UndraHandle) {
        sendFrame(kind: .release, payload: Wire.Release(handle: handle).encode())
    }

    func registerPort(_ portId: UInt32) {
        // The protocol has no registration message: the core calls the port and the host answers
        // `unavailable` for ports it does not implement.
    }

    func portReply(_ payload: [UInt8]) {
        sendFrame(kind: .portReply, payload: payload)
    }

    func event(portId: UInt32, methodId: UInt32, payload: [UInt8]) {
        let message = Wire.Event(portId: portId, methodId: methodId, payload: ArraySlice(payload))
        sendFrame(kind: .event, payload: message.encode())
    }

    func timerFired(_ timerId: UInt32) {
        sendFrame(kind: .timerFired, payload: Wire.TimerFired(timerId: timerId).encode())
    }

    func snapshot() throws -> [UInt8] {
        throw UndraModeError(operation: "snapshot", mode: .remote)
    }

    func restore(_ payload: [UInt8]) throws {
        throw UndraModeError(operation: "restore", mode: .remote)
    }

    func statsJSON() -> String? {
        return nil
    }

    /// Wraps `payload` in an envelope and writes it. Returns `false` if there is no connection
    /// to write to (closed, or reconnecting). The sequence number is assigned and the frame
    /// queued under one lock, so the wire order is the sequence order.
    @discardableResult
    func sendFrame(kind: Envelope.Kind, payload: [UInt8]) -> Bool {
        let sent = state.withLock { (current: inout State) -> Bool in
            if current.isShutDown {
                return false
            }
            if self.sink == nil && current.connection == nil {
                return false
            }
            current.nextSeq &+= 1
            let frame = encodeEnvelope(
                kind: kind,
                seq: current.nextSeq,
                schemaHash: current.schemaHash,
                payload: payload
            )
            if let sink = self.sink {
                sink(frame)
                return true
            }
            guard let connection = current.connection else {
                return false
            }
            let generation = current.generation
            connection.send(frame) { [weak self] error in
                if let error = error {
                    self?.connectionLost(error, generation: generation)
                }
            }
            return true
        }
        return sent
    }

    // MARK: Incoming

    private func receiveNext(from connection: any SocketConnection, generation: Int) {
        connection.receive { [weak self] result in
            guard let self = self else {
                return
            }
            switch result {
            case .failure(let error):
                self.connectionLost(error, generation: generation)
            case .success(let message):
                switch message {
                case .binary(let bytes):
                    self.handleFrame(bytes, generation: generation)
                case .text:
                    self.protocolFailure("the core sent a text frame; the protocol is binary")
                }
                self.receiveNext(from: connection, generation: generation)
            }
        }
    }

    /// Decodes one incoming envelope and routes its payload. Never throws: a malformed or
    /// unexpected frame disconnects with an `UndraProtocolError` or is dropped and logged.
    func handleFrame(_ bytes: [UInt8]) {
        handleFrame(bytes, generation: nil)
    }

    private func handleFrame(_ bytes: [UInt8], generation: Int?) {
        let frame: Envelope.Decoded
        do {
            frame = try decodeEnvelope(bytes)
        } catch let error as WireError {
            protocolFailure("malformed envelope: \(error)")
            return
        } catch {
            protocolFailure("malformed envelope: \(error)")
            return
        }
        let context = state.withLock { (current: inout State) -> (inbound: (any UndraInbound)?, done: Bool, hash: UInt64, stale: Bool) in
            let stale = generation.map { $0 != current.generation } ?? false
            return (inbound: current.inbound, done: current.handshakeDone, hash: current.schemaHash, stale: stale)
        }
        if context.stale {
            return
        }
        if frame.kind == .hello {
            handleHello(frame.payload)
            return
        }
        guard let inbound = context.inbound else {
            return
        }
        if !context.done {
            protocolFailure("the core sent \(frame.kind) before its Hello")
            return
        }
        if frame.schemaHash != context.hash {
            finalFailure(UndraSchemaMismatchError(expected: context.hash, got: frame.schemaHash))
            return
        }
        route(frame, to: inbound)
    }

    private func route(_ frame: Envelope.Decoded, to inbound: any UndraInbound) {
        let payload = Array(frame.payload)
        switch frame.kind {
        case .reply:
            guard let callId = peekU32(frame.payload) else {
                protocolFailure("a reply too short to carry a call id")
                return
            }
            inbound.onReply(callId: callId, payload: payload)
        case .changeSet:
            inbound.onChangeSet(payload)
        case .streamItem:
            guard let callId = peekU32(frame.payload) else {
                protocolFailure("a stream item too short to carry a call id")
                return
            }
            inbound.onStreamItem(callId: callId, payload: payload)
        case .portCall:
            routePortCall(frame.payload, to: inbound)
        case .log:
            do {
                let log = try Wire.Log.decode(slice: frame.payload)
                inbound.onLog(level: log.level, target: log.target, message: log.message)
            } catch {
                protocolFailure("malformed log message: \(error)")
            }
        case .snapshot:
            // Snapshots are requested through UndraCore.snapshot(), which the remote transport
            // does not support; an unsolicited one is ignored.
            break
        case .call, .portReply, .cancel, .streamCredit, .observe, .release, .event, .hello,
             .timerFired, .restore:
            protocolFailure("the core sent a host-to-core message: \(frame.kind)")
        }
    }

    private func routePortCall(_ payload: ArraySlice<UInt8>, to inbound: any UndraInbound) {
        let call: Wire.PortCall
        do {
            call = try Wire.PortCall.decode(slice: payload)
        } catch {
            protocolFailure("malformed port call: \(error)")
            return
        }
        let outcome = inbound.onPortCall(
            portId: call.portId,
            methodId: call.methodId,
            portCallId: call.portCallId,
            args: Array(call.args)
        )
        switch outcome {
        case .sync(let reply):
            sendFrame(kind: .portReply, payload: reply)
        case .unavailable:
            let reply = Wire.PortReply(portCallId: call.portCallId, status: .unavailable)
            sendFrame(kind: .portReply, payload: reply.encode())
        case .async:
            break
        }
    }

    private func handleHello(_ payload: ArraySlice<UInt8>) {
        let waiter = state.withLock { (current: inout State) -> OneShot<Result<TransportInfo, any Error>>? in
            return current.handshake
        }
        do {
            let hello = try Wire.Hello.decode(slice: payload)
            state.withLock { (current: inout State) -> Void in
                current.schemaHash = hello.schemaHash
                current.handshakeDone = true
                current.handshake = nil
            }
            waiter?.fulfill(.success(TransportInfo(schemaHash: hello.schemaHash)))
        } catch {
            state.withLock { (current: inout State) -> Void in
                current.handshake = nil
            }
            waiter?.fulfill(.failure(UndraLoadError.handshakeRejected("malformed Hello: \(error)")))
        }
    }

    private func peekU32(_ payload: ArraySlice<UInt8>) -> UInt32? {
        var reader = UndraReader(slice: payload)
        return try? reader.readU32()
    }

    // MARK: Failure

    private func protocolFailure(_ message: String) {
        UndraLog.error("remote transport: \(message)")
    }

    /// The connection `generation` ended: the socket failed, or the server closed it.
    private func connectionLost(_ error: any Error, generation: Int) {
        let peer = state.withLock { (current: inout State) -> (code: Int, reason: String)? in
            return current.generation == generation ? current.connection?.peerClose : nil
        }
        let context = state.withLock { (current: inout State) -> (waiter: OneShot<Result<TransportInfo, any Error>>?, wasUp: Bool, live: Bool) in
            guard current.generation == generation, !current.isShutDown, current.connection != nil else {
                return (nil, false, false)
            }
            current.connection = nil
            let waiter = current.handshake
            current.handshake = nil
            let wasUp = current.handshakeDone
            current.handshakeDone = false
            return (waiter, wasUp, true)
        }
        if !context.live {
            return
        }
        let reason = WebSocketTransport.describe(error, peer: peer)
        let lostSession = WebSocketTransport.isSessionLost(peer)
        if let waiter = context.waiter {
            // Before the handshake finished: the attempt that waits for it fails.
            waiter.fulfill(.failure(lostSession ? UndraSessionLostError(reason: peer?.reason ?? "") : UndraLoadError.connectionFailed(reason)))
            return
        }
        if lostSession {
            finalFailure(UndraSessionLostError(reason: peer?.reason ?? ""))
        } else if context.wasUp, reconnect != nil {
            beginReconnecting(UndraTransportError.connectionLost(reason: reason))
        } else {
            finalFailure(UndraTransportError.connectionLost(reason: reason))
        }
    }

    private static func isSessionLost(_ peer: (code: Int, reason: String)?) -> Bool {
        guard let peer = peer else {
            return false
        }
        // Foundation may not report a code outside its own list: the reason carries the same news.
        return peer.code == sessionLostCode || peer.reason.hasPrefix("session lost")
    }

    private static func describe(_ error: any Error, peer: (code: Int, reason: String)?) -> String {
        if let peer = peer, peer.code != 1005 {
            return "the dev server closed the connection (\(peer.code)\(peer.reason.isEmpty ? "" : ": \(peer.reason)"))"
        }
        return String(describing: error)
    }

    /// The transport is done for good: tells the core, once.
    private func finalFailure(_ error: any Error) {
        let (inbound, connection, pending) = state.withLock { (current: inout State) -> ((any UndraInbound)?, (any SocketConnection)?, (any ReconnectToken)?) in
            if current.isShutDown {
                return (nil, nil, nil)
            }
            current.isShutDown = true
            current.reconnecting = false
            let existing = current.connection
            current.connection = nil
            let attempt = current.pendingAttempt
            current.pendingAttempt = nil
            return (current.inbound, existing, attempt)
        }
        pending?.cancel()
        connection?.cancel()
        // Inline, not through `events`: a reconnect notice still queued there is ignored by a core that
        // is closed by then.
        inbound?.onDisconnect(error)
    }

    // MARK: Reconnecting

    /// The connection dropped: tell the core (attempt 1 is the loss) and schedule the first retry.
    private func beginReconnecting(_ cause: any Error) {
        let inbound = state.withLock { (current: inout State) -> (any UndraInbound)? in
            current.reconnecting = true
            return current.inbound
        }
        events.async {
            inbound?.onReconnecting(attempt: 1, error: cause)
        }
        scheduleAttempt(1)
    }

    private func scheduleAttempt(_ attempt: Int) {
        guard let policy = reconnect else {
            return
        }
        let delay = policy.delay(forAttempt: attempt)
        let token = scheduler.schedule(after: delay) { [weak self] in
            self?.attemptReconnect(attempt)
        }
        let superseded = state.withLock { (current: inout State) -> Bool in
            if current.isShutDown || !current.reconnecting {
                return true
            }
            current.pendingAttempt = token
            return false
        }
        if superseded {
            token.cancel()
        }
    }

    /// One reconnect attempt, on the scheduler's queue.
    private func attemptReconnect(_ attempt: Int) {
        let context = state.withLock { (current: inout State) -> (inbound: (any UndraInbound)?, live: Bool, timeout: Double, hash: UInt64) in
            current.pendingAttempt = nil
            return (current.inbound, !current.isShutDown && current.reconnecting, current.connectTimeout, current.schemaHash)
        }
        guard context.live, let inbound = context.inbound, let policy = reconnect else {
            return
        }
        let resume = inbound.holdsObjects()
        let patience = min(context.timeout, WebSocketTransport.attemptCap)
        switch openConnection(resume: resume, timeout: patience) {
        case .success(let info):
            if info.schemaHash != context.hash {
                finalFailure(UndraSchemaMismatchError(expected: context.hash, got: info.schemaHash))
                return
            }
            let generation = state.withLock { (current: inout State) -> Int in
                current.reconnecting = false
                return current.generation
            }
            events.async { [weak self] in
                // A connection that dropped again before the core heard of this one is not announced:
                // its loss started the next reconnect.
                guard let self = self, self.isUp(generation: generation) else {
                    return
                }
                inbound.onReconnected()
            }
        case .failure(let error):
            if error is UndraSessionLostError {
                finalFailure(error)
                return
            }
            let next = attempt + 1
            if let limit = policy.maxAttempts, next > limit {
                finalFailure(UndraTransportError.connectionLost(reason: "gave up reconnecting after \(attempt) attempts: \(error)"))
                return
            }
            let live = state.withLock { (current: inout State) -> Bool in
                return !current.isShutDown && current.reconnecting
            }
            guard live else {
                return
            }
            events.async {
                inbound.onReconnecting(attempt: next, error: error)
            }
            scheduleAttempt(next)
        }
    }

    private func isUp(generation: Int) -> Bool {
        return state.withLock { (current: inout State) -> Bool in
            return current.generation == generation && current.connection != nil && current.handshakeDone && !current.isShutDown
        }
    }
}
