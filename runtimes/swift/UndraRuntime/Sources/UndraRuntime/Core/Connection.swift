// What the connection to a remote core is doing, and how it reconnects (ADR-034).
//
// Only a remote core (`undra dev`) ever leaves `.connected`; an in-process core is `.connected`
// from `load` until `shutdown()`.

import Observation

/// Why a core is ``UndraConnectionState/closed(_:)``.
public enum UndraClosedReason: Sendable, Equatable, CustomStringConvertible {
    /// The app called `shutdown()`.
    case requested
    /// The core was rebuilt with another schema than the bindings were generated for.
    case schemaMismatch(expected: UInt64, got: UInt64)
    /// The dev server no longer holds this core's objects: it was restarted (`undra dev` rebuilt
    /// the core), or the session's grace period passed while the app was away. Every store and
    /// object of this core is dead; load a new core and create them again.
    case sessionLost
    /// The connection failed for good: the reconnect policy gave up, or the server broke the
    /// protocol.
    case failed(String)

    public var description: String {
        switch self {
        case .requested:
            return "the app shut the core down"
        case .schemaMismatch(let expected, let got):
            return "schema mismatch (the bindings expect 0x\(hex16(expected)), the core reports 0x\(hex16(got)))"
        case .sessionLost:
            return "the dev server lost this core's session"
        case .failed(let reason):
            return "the connection failed: \(reason)"
        }
    }
}

/// What the connection to the core is doing (``UndraCore/connectionState``).
///
/// After a drop a remote core is `reconnecting`, then `connected` again once its stores are
/// observed again, or `closed` for good (see ``UndraClosedReason``). While it is `reconnecting`
/// every call fails at once with ``UndraCallError/unavailable(_:)``; what was in flight when the
/// connection dropped failed with it.
public enum UndraConnectionState: Sendable, Equatable, CustomStringConvertible {
    /// `UndraCore.load` is connecting. Only `LoadOptions.onConnectionChange` can see this state.
    case connecting
    /// The core is reachable.
    case connected
    /// The connection dropped and the runtime is trying again; `attempt` counts from 1.
    case reconnecting(attempt: Int)
    /// The core is closed for good; a new `UndraCore.load` is the way back.
    case closed(UndraClosedReason)

    public var description: String {
        switch self {
        case .connecting:
            return "connecting"
        case .connected:
            return "connected"
        case .reconnecting(let attempt):
            return "reconnecting (attempt \(attempt))"
        case .closed(let reason):
            return "closed: \(reason)"
        }
    }
}

/// The connection state of a core, for SwiftUI: an `@Observable` object, updated on the main
/// actor.
///
/// ```swift
/// struct DevBanner: View {
///     var body: some View {
///         if case .reconnecting(let attempt) = UndraCore.shared.connection.state {
///             Text("Reconnecting to the dev server (attempt \(attempt))")
///         }
///     }
/// }
/// ```
@MainActor
@Observable
public final class UndraConnection {
    /// The state the main actor has been told of. It trails ``UndraCore/connectionState`` by one
    /// hop to the main queue.
    public internal(set) var state: UndraConnectionState = .connecting

    nonisolated init() {}
}

/// How a remote core reconnects after its connection drops (ADR-034). Attempt `n` (from 1) waits
/// `min(maxDelay, initialDelay * 2^(n-1))` seconds, less a random share of up to `jitter` of that,
/// so many clients of one server do not retry in step. The same schedule in the TypeScript and
/// Kotlin runtimes.
public struct UndraReconnectPolicy: Sendable {
    /// The wait before the first retry, in seconds.
    public var initialDelay: Double
    /// The longest wait, in seconds.
    public var maxDelay: Double
    /// The share of the wait that is randomised away, from 0 (none) to 1 (down to nothing).
    public var jitter: Double
    /// Give up (and close the core) after this many failed attempts; `nil` (the default) never does.
    public var maxAttempts: Int?
    /// Random numbers in `[0, 1)` for the jitter; tests pass a fixed one.
    public var random: @Sendable () -> Double

    /// 250 ms doubling to 5 s, half of it jittered, for ever.
    public static let `default` = UndraReconnectPolicy()

    /// Creates a policy.
    public init(
        initialDelay: Double = 0.25,
        maxDelay: Double = 5,
        jitter: Double = 0.5,
        maxAttempts: Int? = nil,
        random: @escaping @Sendable () -> Double = { Double.random(in: 0..<1) }
    ) {
        self.initialDelay = initialDelay
        self.maxDelay = maxDelay
        self.jitter = min(1, max(0, jitter))
        self.maxAttempts = maxAttempts
        self.random = random
    }

    /// How long reconnect attempt `attempt` (from 1) waits, in seconds.
    public func delay(forAttempt attempt: Int) -> Double {
        let doublings = Double(min(30, max(0, attempt - 1)))
        let base = min(maxDelay, initialDelay * pow2(doublings))
        return base * (1 - jitter * random())
    }

    private func pow2(_ exponent: Double) -> Double {
        var result = 1.0
        var remaining = Int(exponent)
        while remaining > 0 {
            result *= 2
            remaining -= 1
        }
        return result
    }
}

/// The dev server no longer holds the objects of this core (ADR-034): it was restarted (`undra
/// dev` rebuilt the core) or the session's grace period passed while the app was away. The
/// handles of every store and object of this core are dead; load a new core and create them
/// again. Calls made on the core fail with ``UndraCallError/unavailable(_:)``.
public struct UndraSessionLostError: Error, Sendable, Equatable, CustomStringConvertible {
    /// The server's reason, when it gave one.
    public let reason: String

    /// Creates the error.
    public init(reason: String = "") {
        self.reason = reason
    }

    public var description: String {
        let tail = reason.isEmpty ? "" : ": \(reason)"
        return "the dev server no longer has this core's objects (it was restarted, or the session expired); load a new core\(tail)"
    }
}
