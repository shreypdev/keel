// WebSocket (ADR-047) on URLSessionWebSocketTask.

import Foundation

// TEMPORARY DIAGNOSTIC (ci-green): a timeline of one connection on stderr.
func wsdbg(_ message: @autoclosure () -> String) {
    let now = Date().timeIntervalSince1970.truncatingRemainder(dividingBy: 1000)
    FileHandle.standardError.write(Data(String(format: "WSDBG %.3f %@\n", now, message()).utf8))
}

/// `WebSocket` on `URLSessionWebSocketTask`.
///
/// Each connection has a session of its own, whose delegate learns when the handshake succeeded
/// (and which subprotocol the server chose) and the code and reason of the peer's close frame.
/// The subprotocols go out as `Sec-WebSocket-Protocol` on a `URLRequest` that carries the headers.
///
/// Inbound messages are pulled: `messages` calls `receive()` once per message the binding asks
/// for, and the binding asks only while the core's window has room, so a core that stops reading
/// stops the socket and TCP pushes back on the server.
///
/// How a connection ends: the first `receive` or `send` that fails after the peer's close frame is
/// ``WsError/closed(code:reason:)`` with the frame's code and reason; a failed upgrade is
/// ``WsError/refused(status:message:)`` with the HTTP status; a frame that breaks RFC 6455 (a
/// text frame that is not UTF-8) is ``WsError/protocol(_:)``; any other failure is
/// ``WsError/network(_:)``.
///
/// It is also the `WebSocket` adapter of ``Adapters/platformDefault`` (served by the binding of
/// ``WebSocketPortAdapter``).
public final class URLSessionWebSocketAdapter: WebSocketAdapter, UndraAdapter, @unchecked Sendable {
    private let configuration: URLSessionConfiguration
    private let maximumMessageSize: Int
    private let bindings = BindingSet<WebSocketBinding>()

    /// Opens connections with sessions made from `configuration` (copied), accepting messages up
    /// to `maximumMessageSize` bytes (URLSession's default is 1 MiB).
    public init(configuration: URLSessionConfiguration = .default, maximumMessageSize: Int = 16 * 1024 * 1024) {
        self.configuration = configuration.copy() as? URLSessionConfiguration ?? .default
        self.maximumMessageSize = maximumMessageSize
    }

    /// Opens a `URLSessionWebSocketTask` to `url` with `headers` and `protocols` and returns once
    /// the handshake succeeded (the delegate's `didOpenWithProtocol`).
    public func connect(url: String, protocols: [String], headers: [Header]) async throws(WsError) -> any WebSocketConnection {
        guard let target = URL(string: url), let scheme = target.scheme?.lowercased(), scheme == "ws" || scheme == "wss",
              target.host != nil
        else {
            throw WsError.refused(status: nil, message: "invalid URL: \(url)")
        }
        var request = URLRequest(url: target)
        for header in headers {
            request.addValue(header.value, forHTTPHeaderField: header.name)
        }
        if !protocols.isEmpty {
            request.setValue(protocols.joined(separator: ", "), forHTTPHeaderField: "Sec-WebSocket-Protocol")
        }
        let configuration = self.configuration.copy() as? URLSessionConfiguration ?? .default
        let connection = URLSessionWebSocketConnection(
            request: request,
            configuration: configuration,
            maximumMessageSize: maximumMessageSize
        )
        try await connection.open()
        return connection
    }

    // MARK: UndraAdapter

    public var portId: UInt32 {
        return StandardPorts.WebSocket.portId
    }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return bindings.add(WebSocketBinding(adapter: self)).portImpl()
    }

    public func detach() {
        bindings.detachAll()
    }
}

/// One `URLSessionWebSocketTask` and the delegate of its session.
final class URLSessionWebSocketConnection: NSObject, WebSocketConnection, URLSessionWebSocketDelegate, @unchecked Sendable {
    private struct State {
        var opening: CheckedContinuation<Result<String, WsError>, Never>?
        var opened = false
        var negotiated = ""
        /// The peer's close frame, as the delegate reported it.
        var peerClose: (code: UInt16, reason: String)?
        /// The task completed (the delegate's last word).
        var completed = false
        var closing = false
    }

    private let state = Guarded(State())
    private var session: URLSession!
    private var task: URLSessionWebSocketTask!

    init(request: URLRequest, configuration: URLSessionConfiguration, maximumMessageSize: Int) {
        super.init()
        let queue = OperationQueue()
        queue.maxConcurrentOperationCount = 1
        // The session keeps its delegate (this object) until it is invalidated, which `close`
        // and the end of the task do.
        session = URLSession(configuration: configuration, delegate: self, delegateQueue: queue)
        task = session.webSocketTask(with: request)
        task.maximumMessageSize = maximumMessageSize
    }

    /// Starts the handshake and waits for its outcome.
    func open() async throws(WsError) {
        let outcome = await withCheckedContinuation { (continuation: CheckedContinuation<Result<String, WsError>, Never>) in
            state.withLock { (current: inout State) -> Void in
                current.opening = continuation
            }
            task.resume()
        }
        switch outcome {
        case .success:
            return
        case .failure(let error):
            session.invalidateAndCancel()
            throw error
        }
    }

    // MARK: WebSocketConnection

    var negotiatedProtocol: String {
        return state.withLock { (current: inout State) -> String in
            return current.negotiated
        }
    }

    var messages: AsyncThrowingStream<WsMessage, any Error> {
        return AsyncThrowingStream(unfolding: { [self] () async throws -> WsMessage? in
            return try await self.receiveNext()
        })
    }

    func send(_ message: WsMessage) async throws(WsError) {
        let frame: URLSessionWebSocketTask.Message
        switch message {
        case .text(let text):
            frame = .string(text)
        case .binary(let bytes):
            frame = .data(Data(bytes))
        }
        do {
            // As `receive`: the caller's cancellation does not cancel the socket.
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, any Error>) in
                task.send(frame) { error in
                    if let error = error {
                        continuation.resume(throwing: error)
                    } else {
                        continuation.resume()
                    }
                }
            }
        } catch {
            throw await failure(after: error)
        }
    }

    func close(code: UInt16, reason: String) async {
        wsdbg("close(code: \(code), reason: \(reason)) entered; task.state=\(task.state.rawValue)")
        let first = state.withLock { (current: inout State) -> Bool in
            if current.closing {
                return false
            }
            current.closing = true
            return true
        }
        guard first else {
            wsdbg("close: already closing, nothing done")
            return
        }
        let closeCode = URLSessionWebSocketTask.CloseCode(rawValue: Int(code)) ?? .normalClosure
        let variant = ProcessInfo.processInfo.environment["UNDRA_WS_VARIANT"] ?? "base"
        if variant.contains("predelay") {
            try? await Task.sleep(nanoseconds: 30_000_000)
        }
        if variant.contains("ping") {
            await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
                let once = Guarded(false)
                task.sendPing { _ in
                    if once.withLock({ (done: inout Bool) -> Bool in let was = done; done = true; return was }) == false { continuation.resume() }
                }
                DispatchQueue.global().asyncAfter(deadline: .now() + 1) {
                    if once.withLock({ (done: inout Bool) -> Bool in let was = done; done = true; return was }) == false { continuation.resume() }
                }
            }
        }
        wsdbg("cancel(with: \(closeCode.rawValue)) now; task.state=\(task.state.rawValue) variant=\(variant)")
        task.cancel(with: closeCode, reason: reason.isEmpty ? nil : Data(reason.utf8))
        wsdbg("cancel(with:) returned; task.state=\(task.state.rawValue) closeCode=\(task.closeCode.rawValue)")
        if variant.contains("delayinv") {
            try? await Task.sleep(nanoseconds: 1_000_000_000)
        }
        // Lets the close frame go out, then releases the delegate.
        session.finishTasksAndInvalidate()
    }

    // MARK: Receiving

    /// The next message, `nil` after `close`, or the typed end.
    private func receiveNext() async throws -> WsMessage? {
        if isClosing {
            return nil
        }
        let message: URLSessionWebSocketTask.Message
        wsdbg("receive() starting")
        do {
            // The completion-handler `receive`, in a continuation: the awaiting Swift task's cancellation (the binding
            // cancels its pump when the core closes the connection) must not reach the socket; only `close` does.
            message = try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<URLSessionWebSocketTask.Message, any Error>) in
                task.receive { result in
                    continuation.resume(with: result)
                }
            }
            wsdbg("receive() returned a message")
        } catch {
            wsdbg("receive() failed: \(error); isClosing=\(isClosing)")
            if isClosing {
                return nil
            }
            throw await failure(after: error)
        }
        switch message {
        case .string(let text):
            return .text(text)
        case .data(let data):
            return .binary([UInt8](data))
        @unknown default:
            throw WsError.protocol("an unknown kind of WebSocket message")
        }
    }

    private var isClosing: Bool {
        return state.withLock { (current: inout State) -> Bool in
            return current.closing
        }
    }

    /// What a failed `receive` or `send` means. The delegate may hear of the peer's close frame
    /// just after the call failed, so this waits (briefly) for its word first.
    private func failure(after error: any Error) async -> WsError {
        for _ in 0 ..< 100 {
            let settled = state.withLock { (current: inout State) -> Bool in
                return current.peerClose != nil || current.completed
            }
            if settled {
                break
            }
            try? await Task.sleep(nanoseconds: 10_000_000)
        }
        if let close = state.withLock({ (current: inout State) -> (code: UInt16, reason: String)? in current.peerClose }) {
            return .closed(code: close.code, reason: close.reason)
        }
        let code = task.closeCode.rawValue
        if code != 0 {
            let reason = task.closeReason.map { String(decoding: $0, as: UTF8.self) } ?? ""
            return .closed(code: UInt16(clamping: code), reason: reason)
        }
        let nsError = error as NSError
        if nsError.domain == NSPOSIXErrorDomain && nsError.code == Int(EPROTO) {
            // Network.framework's verdict on a frame that breaks RFC 6455, a text frame that is
            // not UTF-8 among them.
            return .protocol("the server sent a frame that breaks RFC 6455 (\(nsError.localizedDescription))")
        }
        return .network(nsError.localizedDescription)
    }

    // MARK: URLSessionWebSocketDelegate

    func urlSession(_ session: URLSession, webSocketTask: URLSessionWebSocketTask, didOpenWithProtocol negotiated: String?) {
        wsdbg("delegate didOpen")
        let opening = state.withLock { (current: inout State) -> CheckedContinuation<Result<String, WsError>, Never>? in
            current.opened = true
            current.negotiated = negotiated ?? ""
            let taken = current.opening
            current.opening = nil
            return taken
        }
        opening?.resume(returning: .success(negotiated ?? ""))
    }

    func urlSession(
        _ session: URLSession,
        webSocketTask: URLSessionWebSocketTask,
        didCloseWith closeCode: URLSessionWebSocketTask.CloseCode,
        reason: Data?
    ) {
        wsdbg("delegate didCloseWith \(closeCode.rawValue)")
        state.withLock { (current: inout State) -> Void in
            if current.peerClose == nil {
                current.peerClose = (
                    UInt16(clamping: closeCode.rawValue),
                    reason.map { String(decoding: $0, as: UTF8.self) } ?? ""
                )
            }
        }
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: (any Error)?) {
        wsdbg("delegate didCompleteWithError \(String(describing: error))")
        let opening = state.withLock { (current: inout State) -> CheckedContinuation<Result<String, WsError>, Never>? in
            current.completed = true
            let taken = current.opening
            current.opening = nil
            return taken
        }
        if let opening = opening {
            opening.resume(returning: .failure(URLSessionWebSocketConnection.refusal(task: task, error: error)))
        }
        // The task is over: let the session (and with it this delegate) go.
        if !(ProcessInfo.processInfo.environment["UNDRA_WS_VARIANT"] ?? "").contains("delayinv") {
            session.finishTasksAndInvalidate()
        }
    }

    /// Why a handshake failed: the HTTP status of a refused upgrade, else a network failure.
    static func refusal(task: URLSessionTask, error: (any Error)?) -> WsError {
        if let response = task.response as? HTTPURLResponse, response.statusCode != 101 {
            let status = response.statusCode
            let text = HTTPURLResponse.localizedString(forStatusCode: status)
            return .refused(status: UInt16(clamping: status), message: "the server answered the upgrade with HTTP \(status) (\(text))")
        }
        if let urlError = error as? URLError, urlError.code == .badURL || urlError.code == .unsupportedURL {
            return .refused(status: nil, message: "invalid URL: \(task.originalRequest?.url?.absoluteString ?? "")")
        }
        if let error = error {
            return .network((error as NSError).localizedDescription)
        }
        return .network("the connection ended before the handshake")
    }
}
