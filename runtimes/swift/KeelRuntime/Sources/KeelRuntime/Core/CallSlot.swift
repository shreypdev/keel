// Bookkeeping for calls in flight.

/// The rendezvous between an `async` call and its reply.
///
/// A slot is completed at most once, by whichever of three events comes first: the reply, a
/// transport failure, or the caller's task being cancelled. The state machine also covers the two
/// orderings that make cancellation subtle: cancellation before the continuation is installed
/// (the call must then never be sent) and a reply before the continuation is installed (it is
/// held until `install`).
final class CallSlot: @unchecked Sendable {
    private enum State {
        case idle
        case waiting(CheckedContinuation<[UInt8], any Error>)
        case completedEarly(Result<[UInt8], any Error>)
        case cancelledEarly
        case finished
    }

    private let state = Guarded<State>(.idle)

    init() {}

    /// Installs the continuation of the awaiting task. Returns `true` if the call should now be
    /// sent. Returns `false` if the slot was already cancelled or completed; the continuation
    /// has then been resumed and nothing may be sent.
    func install(_ continuation: CheckedContinuation<[UInt8], any Error>) -> Bool {
        let early = state.withLock { (current: inout State) -> Result<[UInt8], any Error>? in
            switch current {
            case .idle:
                current = .waiting(continuation)
                return nil
            case .completedEarly(let result):
                current = .finished
                return result
            case .cancelledEarly:
                current = .finished
                return .failure(CancellationError())
            case .waiting, .finished:
                return .failure(KeelTransportError.closed)
            }
        }
        if let early = early {
            continuation.resume(with: early)
            return false
        }
        return true
    }

    /// Completes the call. Returns `false` if it was already completed or cancelled.
    @discardableResult
    func complete(_ result: Result<[UInt8], any Error>) -> Bool {
        let outcome = state.withLock { (current: inout State) -> (Bool, CheckedContinuation<[UInt8], any Error>?) in
            switch current {
            case .idle:
                current = .completedEarly(result)
                return (true, nil)
            case .waiting(let continuation):
                current = .finished
                return (true, continuation)
            case .completedEarly, .cancelledEarly, .finished:
                return (false, nil)
            }
        }
        if let continuation = outcome.1 {
            continuation.resume(with: result)
        }
        return outcome.0
    }

    /// Cancels the call on behalf of the caller's task. Returns `true` if the call had been sent
    /// and is still unanswered (so the core must be told); the awaiting task is resumed with
    /// `CancellationError`. Returns `false` if there is nothing to tell the core: the call was
    /// never sent, or it had already finished.
    func cancel() -> Bool {
        let outcome = state.withLock { (current: inout State) -> (Bool, CheckedContinuation<[UInt8], any Error>?) in
            switch current {
            case .idle:
                current = .cancelledEarly
                return (false, nil)
            case .waiting(let continuation):
                current = .finished
                return (true, continuation)
            case .completedEarly, .cancelledEarly, .finished:
                return (false, nil)
            }
        }
        if let continuation = outcome.1 {
            continuation.resume(throwing: CancellationError())
        }
        return outcome.0
    }
}

/// What `KeelCore` knows about one call id.
enum PendingCall: Sendable {
    /// The id is taken but nothing waits on it yet (or it is a `callSync` whose reply comes back
    /// through the transport's return value).
    case reserved
    /// An `async` call.
    case unary(CallSlot)
    /// A blocking call over a transport without inline sync calls.
    case blocking(OneShot<Result<[UInt8], any Error>>)
    /// A stream.
    case stream(StreamChannel)
}
