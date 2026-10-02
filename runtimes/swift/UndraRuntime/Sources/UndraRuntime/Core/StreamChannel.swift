// Streams and their flow control (docs/SPEC.md section 3.7).
//
// The core sends at most `credit` items beyond what the host has credited, and the initial
// credit is zero. This runtime grants 16 when a stream opens and tops the window back up to 16
// whenever the items the consumer has not yet taken drop below 8, so a slow consumer bounds the
// core's memory to about 16 items per stream and a fast one never waits for credit.
//
// Consumption is measured where the consumer takes an item (`AsyncThrowingStream(unfolding:)`
// pulls one item per `next()`), not where the core delivers it, which is what makes the window a
// real backpressure signal.

/// The buffer between the core's stream callback (any thread, no calls back into the core) and
/// the consumer's `next()` (an ordinary task).
final class StreamChannel: @unchecked Sendable {
    /// Credit granted when the stream opens, and the level the window is topped up to.
    static let initialCredit: UInt32 = 16
    /// The window (items sendable or in the buffer, not yet consumed) below which credit is topped up.
    static let lowWater: UInt32 = 8

    enum Terminal {
        case ended
        case failed(any Error)
    }

    private struct Inner {
        var buffer: [[UInt8]] = []
        var terminal: Terminal? = nil
        var waiter: CheckedContinuation<[UInt8]?, any Error>? = nil
        var granted: UInt32 = 0
        var consumed: UInt32 = 0
        var closedByConsumer = false
    }

    private enum Pull {
        case resume(Result<[UInt8]?, any Error>)
        case parked
    }

    let callId: UInt32
    /// The callback instances the stream's arguments carry: given back when the core refuses the stream.
    let lent: LentInstances?
    private let inner = Guarded<Inner>(Inner())
    private let onCredit: @Sendable (UInt32, UInt32) -> Void
    private let onClose: @Sendable (UInt32) -> Void

    /// - Parameters:
    ///   - onCredit: sends `StreamCredit(callId, credit)` to the core; called from the consumer's task.
    ///   - onClose: tells the core to cancel the stream and forgets it; called when the consumer
    ///     stops before the stream ended.
    init(
        callId: UInt32,
        lent: LentInstances? = nil,
        onCredit: @escaping @Sendable (UInt32, UInt32) -> Void,
        onClose: @escaping @Sendable (UInt32) -> Void
    ) {
        self.callId = callId
        self.lent = lent
        self.onCredit = onCredit
        self.onClose = onClose
    }

    // MARK: Producer side (the core's callbacks)

    /// Records the initial grant. The caller then sends `StreamChannel.initialCredit` to the core.
    func noteInitialGrant() {
        inner.withLock { (state: inout Inner) -> Void in
            state.granted = StreamChannel.initialCredit
        }
    }

    /// Delivers one item.
    func push(_ item: [UInt8]) {
        let waiter = inner.withLock { (state: inout Inner) -> CheckedContinuation<[UInt8]?, any Error>? in
            if state.closedByConsumer || state.terminal != nil {
                return nil
            }
            if let parked = state.waiter {
                state.waiter = nil
                return parked
            }
            state.buffer.append(item)
            return nil
        }
        if let waiter = waiter {
            waiter.resume(returning: item)
        }
    }

    /// Ends the stream, normally or with an error. Items already buffered are still delivered
    /// first. Later calls are ignored.
    func finish(_ terminal: Terminal) {
        let parked = inner.withLock { (state: inout Inner) -> CheckedContinuation<[UInt8]?, any Error>? in
            if state.terminal != nil || state.closedByConsumer {
                return nil
            }
            state.terminal = terminal
            if state.buffer.isEmpty, let parked = state.waiter {
                state.waiter = nil
                if case .failed = terminal {
                    // The failure is delivered exactly once; after it the stream is over.
                    state.terminal = .ended
                }
                return parked
            }
            return nil
        }
        if let parked = parked {
            switch terminal {
            case .ended:
                parked.resume(returning: nil)
            case .failed(let error):
                parked.resume(throwing: error)
            }
        }
    }

    // MARK: Consumer side

    /// The next item, `nil` when the stream has ended (or the consumer stopped), or the error the
    /// stream failed with.
    func next() async throws -> [UInt8]? {
        let item = try await pull()
        if item != nil {
            noteConsumed()
        }
        return item
    }

    /// The consumer stopped (its task was cancelled or it dropped the stream). Cancels the stream
    /// in the core unless it already ended. Idempotent.
    func consumerGone() {
        let outcome = inner.withLock { (state: inout Inner) -> (CheckedContinuation<[UInt8]?, any Error>?, Bool) in
            if state.closedByConsumer {
                return (nil, false)
            }
            state.closedByConsumer = true
            let parked = state.waiter
            state.waiter = nil
            let ended = state.terminal != nil
            state.buffer.removeAll()
            return (parked, !ended)
        }
        if let parked = outcome.0 {
            parked.resume(returning: nil)
        }
        if outcome.1 {
            onClose(callId)
        }
    }

    // MARK: Internals

    private func pull() async throws -> [UInt8]? {
        return try await withTaskCancellationHandler(
            operation: {
                try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<[UInt8]?, any Error>) in
                    let decision = self.inner.withLock { (state: inout Inner) -> Pull in
                        return StreamChannel.decide(&state, continuation)
                    }
                    switch decision {
                    case .resume(let result):
                        continuation.resume(with: result)
                    case .parked:
                        break
                    }
                }
            },
            onCancel: {
                self.consumerGone()
            }
        )
    }

    private static func decide(_ state: inout Inner, _ continuation: CheckedContinuation<[UInt8]?, any Error>) -> Pull {
        if state.closedByConsumer {
            return .resume(.success(nil))
        }
        if !state.buffer.isEmpty {
            let item = state.buffer.removeFirst()
            return .resume(.success(item))
        }
        if let terminal = state.terminal {
            switch terminal {
            case .ended:
                return .resume(.success(nil))
            case .failed(let error):
                state.terminal = .ended
                return .resume(.failure(error))
            }
        }
        if state.waiter != nil {
            // Two concurrent `next()` calls on one iterator: not allowed by AsyncSequence.
            return .resume(.failure(UndraTransportError.closed))
        }
        state.waiter = continuation
        return .parked
    }

    /// Counts an item as consumed and tops the credit window up when it has run low.
    private func noteConsumed() {
        let grant = inner.withLock { (state: inout Inner) -> UInt32 in
            state.consumed &+= 1
            if state.terminal != nil || state.closedByConsumer {
                return 0
            }
            let window: UInt32 = state.granted >= state.consumed ? state.granted - state.consumed : 0
            if window >= StreamChannel.lowWater {
                return 0
            }
            let top = StreamChannel.initialCredit - window
            state.granted &+= top
            return top
        }
        if grant > 0 {
            onCredit(callId, grant)
        }
    }
}

/// Owned by the `AsyncThrowingStream` the consumer holds (its `unfolding` closure captures it).
/// When the stream (and every iterator over it) goes away, the deinitializer stops the stream in
/// the core.
final class StreamConsumer: @unchecked Sendable {
    private let channel: StreamChannel

    init(_ channel: StreamChannel) {
        self.channel = channel
    }

    func next() async throws -> [UInt8]? {
        return try await channel.next()
    }

    /// Stops the stream now instead of when the consumer is deallocated.
    func stop() {
        channel.consumerGone()
    }

    deinit {
        channel.consumerGone()
    }
}
