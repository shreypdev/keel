// The remote transport: a core running elsewhere (`undra dev`), reached over a WebSocket that
// carries the envelope of docs/SPEC.md section 3.2 (one envelope per binary message).
//
// This file imports Foundation for `URLSessionWebSocketTask`; it is the only transport file that
// does. The protocol logic (`handleFrame`, `sendFrame`) is separate from the socket so tests can
// drive it with a frame sink.

import Foundation

/// Speaks the envelope protocol to a dev core over a WebSocket.
///
/// The handshake: the host sends `Hello` (announcing the schema hash the bindings expect), the
/// core answers with its own `Hello`, and `start` returns the core's hash for `UndraCore` to
/// compare. Every later envelope carries the core's hash, and one that does not disconnects.
///
/// There is no automatic reconnect: when the socket closes, everything in flight fails with
/// `UndraTransportError.connectionLost` and the app loads a new core.
final class WebSocketTransport: UndraTransport, @unchecked Sendable {
    private struct State {
        var inbound: (any UndraInbound)? = nil
        var task: URLSessionWebSocketTask? = nil
        var handshake: OneShot<Result<TransportInfo, any Error>>? = nil
        var nextSeq: UInt32 = 0
        var schemaHash: UInt64 = 0
        var handshakeDone = false
        var isShutDown = false
    }

    private let url: URL?
    private let session: URLSession?
    private let sink: (@Sendable ([UInt8]) -> Void)?
    private let state = Guarded<State>(State())

    /// Creates a transport for a `ws://` or `wss://` URL.
    ///
    /// - Throws: `UndraLoadError.invalidURL`.
    convenience init(urlString: String) throws {
        guard let url = URL(string: urlString),
              let scheme = url.scheme?.lowercased(),
              scheme == "ws" || scheme == "wss",
              let host = url.host,
              !host.isEmpty
        else {
            throw UndraLoadError.invalidURL(urlString)
        }
        self.init(url: url)
    }

    /// Creates a transport for `url`, which must be a `ws://` or `wss://` URL.
    init(url: URL) {
        self.url = url
        self.session = URLSession(configuration: .default)
        self.sink = nil
    }

    /// Creates a transport that writes every outgoing frame to `sink` instead of a socket, for
    /// tests of the protocol logic. Incoming frames are fed to `handleFrame(_:)`.
    init(frameSink: @escaping @Sendable ([UInt8]) -> Void) {
        self.url = nil
        self.session = nil
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
        let waiter = OneShot<Result<TransportInfo, any Error>>()
        var task: URLSessionWebSocketTask? = nil
        if let session = session, let url = url {
            task = session.webSocketTask(with: url)
        }
        state.withLock { (current: inout State) -> Void in
            current.inbound = inbound
            current.task = task
            current.handshake = waiter
            current.schemaHash = options.expectedSchemaHash
            current.isShutDown = false
            current.handshakeDone = false
        }
        if let task = task {
            task.resume()
            receiveNext(from: task)
        }
        let hello = Wire.Hello(
            undraVersion: UndraCore.undraVersion,
            schemaHash: options.expectedSchemaHash,
            platform: options.platform,
            mode: "dev"
        )
        sendFrame(kind: .hello, payload: hello.encode())
        guard let outcome = waiter.wait(timeoutSeconds: options.connectTimeout) else {
            closeSocket()
            throw UndraLoadError.handshakeTimedOut(seconds: options.connectTimeout)
        }
        switch outcome {
        case .success(let info):
            return info
        case .failure(let error):
            closeSocket()
            throw error
        }
    }

    func shutdown() {
        closeSocket()
    }

    private func closeSocket() {
        let task = state.withLock { (current: inout State) -> URLSessionWebSocketTask? in
            current.isShutDown = true
            let existing = current.task
            current.task = nil
            return existing
        }
        task?.cancel(with: .goingAway, reason: nil)
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

    /// Wraps `payload` in an envelope and writes it. Returns `false` if the transport is shut
    /// down. The sequence number is assigned and the frame queued under one lock, so the wire
    /// order is the sequence order.
    @discardableResult
    func sendFrame(kind: Envelope.Kind, payload: [UInt8]) -> Bool {
        let sent = state.withLock { (current: inout State) -> Bool in
            if current.isShutDown {
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
            guard let task = current.task else {
                return false
            }
            task.send(.data(Data(frame))) { [weak self] error in
                if let error = error {
                    self?.connectionLost(error)
                }
            }
            return true
        }
        return sent
    }

    // MARK: Incoming

    private func receiveNext(from task: URLSessionWebSocketTask) {
        task.receive { [weak self] result in
            guard let self = self else {
                return
            }
            switch result {
            case .failure(let error):
                self.connectionLost(error)
            case .success(let message):
                switch message {
                case .data(let data):
                    self.handleFrame([UInt8](data))
                case .string:
                    self.protocolFailure("the core sent a text frame; the protocol is binary")
                @unknown default:
                    self.protocolFailure("the core sent an unknown kind of WebSocket message")
                }
                self.receiveNext(from: task)
            }
        }
    }

    /// Decodes one incoming envelope and routes its payload. Never throws: a malformed or
    /// unexpected frame disconnects with an `UndraProtocolError` or is dropped and logged.
    func handleFrame(_ bytes: [UInt8]) {
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
        let context = state.withLock { (current: inout State) -> (inbound: (any UndraInbound)?, done: Bool, hash: UInt64) in
            return (inbound: current.inbound, done: current.handshakeDone, hash: current.schemaHash)
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
            inbound.onDisconnect(UndraSchemaMismatchError(expected: context.hash, got: frame.schemaHash))
            closeSocket()
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

    private func connectionLost(_ error: any Error) {
        let context = state.withLock { (current: inout State) -> (inbound: (any UndraInbound)?, waiter: OneShot<Result<TransportInfo, any Error>>?, alreadyDown: Bool) in
            let result = (inbound: current.inbound, waiter: current.handshake, alreadyDown: current.isShutDown)
            current.handshake = nil
            current.isShutDown = true
            current.task = nil
            return result
        }
        if context.alreadyDown {
            return
        }
        let reason = String(describing: error)
        if let waiter = context.waiter {
            waiter.fulfill(.failure(UndraLoadError.connectionFailed(reason)))
            return
        }
        context.inbound?.onDisconnect(UndraTransportError.connectionLost(reason: reason))
    }
}
