// The seam between `UndraCore` and the two ways of reaching a core: the C ABI in process
// (`InprocTransport`) and a WebSocket (`WebSocketTransport`). Tests plug in a scripted fake.
//
// A transport moves already-encoded payloads. `UndraCore` owns everything above that: call ids,
// continuations, streams, the mirror, ports.

/// What the host answers to a port call (docs/SPEC.md sections 3.6 and 6.3).
package enum PortCallOutcome: Sendable, Equatable {
    /// The host answered inline; `reply` is a complete `Wire.PortReply` payload.
    case sync(reply: [UInt8])
    /// The host will answer later through `UndraTransport.portReply(_:)`.
    case async
    /// The port is not implemented, or the method is unknown, or it failed.
    case unavailable
}

/// What a transport reports to `UndraCore`.
///
/// Every method may be called on any thread, possibly while the core holds its own lock. An
/// implementation must therefore copy what it needs, hand the work to another thread, and never
/// call back into the transport synchronously (the one exception is `onPortCall`, which returns
/// the answer for a sync port and has no other way to give it).
package protocol UndraInbound: AnyObject, Sendable {
    /// A `Wire.Reply` payload for call `callId`.
    func onReply(callId: UInt32, payload: [UInt8])
    /// A `Wire.ChangeSet` payload.
    func onChangeSet(_ payload: [UInt8])
    /// A `Wire.StreamItem` payload for the stream opened by call `callId`.
    func onStreamItem(callId: UInt32, payload: [UInt8])
    /// The core calls a host-implemented port.
    func onPortCall(portId: UInt32, methodId: UInt32, portCallId: UInt32, args: [UInt8]) -> PortCallOutcome
    /// A log record from the core (remote transport; in process the core logs through the Log port).
    func onLog(level: UInt8, target: String, message: String)
    /// The connection is gone for good; every call in flight fails with `error`.
    func onDisconnect(_ error: any Error)
    /// Only for a transport that reconnects (remote, ADR-051): the connection dropped, or a retry
    /// failed, and the transport will try again. `attempt` counts from 1, and attempt 1 is the
    /// loss itself: whatever was in flight has failed for good. `onDisconnect` follows only if the
    /// transport gives up.
    func onReconnecting(attempt: Int, error: any Error)
    /// Only for a transport that reconnects: the connection is back and the core's `Hello` was
    /// checked. The core observes its stores again (the callback must not call into the transport).
    func onReconnected()
    /// Only for a transport that reconnects: whether the core holds objects it expects the server
    /// to still have (it asks the server to resume them).
    func holdsObjects() -> Bool
}

extension UndraInbound {
    package func onReconnecting(attempt: Int, error: any Error) {}

    package func onReconnected() {}

    package func holdsObjects() -> Bool {
        return false
    }
}

/// What a transport learns while starting.
package struct TransportInfo: Sendable, Equatable {
    /// The schema hash of the core.
    package var schemaHash: UInt64

    /// Reports a core with this schema hash.
    package init(schemaHash: UInt64) {
        self.schemaHash = schemaHash
    }
}

/// What a transport needs to start.
package struct TransportStartOptions: Sendable, Equatable {
    /// The host platform, sent to the core (`"ios"`, `"macos"`).
    package var platform: String
    /// Records below this level are not forwarded by the core (Log port numbering).
    package var logLevel: UInt8
    /// Seconds to wait for a remote handshake.
    package var connectTimeout: Double
    /// The schema hash the bindings were generated for; a remote transport announces it in its
    /// `Hello`.
    package var expectedSchemaHash: UInt64
}

/// Moves encoded payloads between `UndraCore` and a core.
///
/// All methods are thread-safe. None of them may be called from inside an `UndraInbound` callback
/// (the core's re-entrancy rule, docs/SPEC.md section 5.1).
package protocol UndraTransport: AnyObject, Sendable {
    /// How this transport reaches the core.
    var mode: UndraMode { get }

    /// Whether `callSync` runs a call inline on the calling thread (in process). When it does
    /// not, `UndraCore.callSync` blocks on an ordinary call instead.
    var supportsDirectSync: Bool { get }

    /// Attaches to the core (blocking until the handshake is done) and reports its schema hash.
    func start(inbound: any UndraInbound, options: TransportStartOptions) throws -> TransportInfo

    /// Sends a `Wire.Call` payload. Returns `false` if the core rejected it without a reply.
    func send(call payload: [UInt8]) -> Bool

    /// Runs a `Wire.Call` payload inline and returns the `Wire.Reply` payload. Only called when
    /// `supportsDirectSync` is true.
    func callSync(_ payload: [UInt8]) throws -> [UInt8]

    func cancel(callId: UInt32)
    func streamCredit(callId: UInt32, credit: UInt32)
    func observe(handle: UndraHandle, signal: UInt32, on: Bool)
    func release(handle: UndraHandle)

    /// Tells the core that the host implements `portId`.
    func registerPort(_ portId: UInt32)
    /// Sends a `Wire.PortReply` payload (the answer of an async port).
    func portReply(_ payload: [UInt8])
    func event(portId: UInt32, methodId: UInt32, payload: [UInt8])
    func timerFired(_ timerId: UInt32)

    func snapshot() throws -> [UInt8]
    func restore(_ payload: [UInt8]) throws

    /// The core's statistics document, if the transport can ask for it.
    func statsJSON() -> String?

    /// Detaches from the core. Idempotent.
    func shutdown()
}
