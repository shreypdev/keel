// The read-ahead of one inbound stream of the WebSocket and Sse bindings (ADR-047 §3): a pump
// that pulls a platform stream into a buffer only while the buffer is under the core's window, and
// the pull (`receive` / `next`) that hands the buffer to the core.
//
// The platform streams are lazy (`AsyncThrowingStream(unfolding:)` over `URLSessionWebSocketTask
// .receive()`, or over the events a data task's delegate queued: the Sse adapter suspends the task
// while enough of them wait), so a full buffer stops the pump, the pump stops asking the socket,
// and TCP pushes back on the server.
//
// A burst is one crossing (ADR-047 §3). URLSession hands a burst over one message per `receive()`
// call, tens of microseconds apart, which is as fast as the core pulls; answering every pull with
// the one message that happened to be there would turn a burst of 16 into 16 crossings. So a pull
// that finds fewer than `max` items while they are still arriving waits for the burst: it is
// answered once `max` items are there, once no item arrived for `burstGap`, at the latest
// `burstCap` after its first item, or when the stream ends. A lone message waits `burstGap`.

import Dispatch

/// How many items the pump reads ahead before the core's first pull (SPEC 3.7's initial grant).
let initialReadAhead = 16

/// A pending pull holding some items is answered when no other item arrived for this long.
let burstGap: DispatchTimeInterval = .milliseconds(2)

/// ... and at the latest this long after its first item arrived.
let burstCap: UInt64 = 8_000_000 // nanoseconds

/// One inbound stream: its pump, its buffer and the core's pending pull.
///
/// Every state change happens under one lock; continuations are resumed after it is released.
final class PulledInbox<Item: Sendable, Failure: Error & Sendable>: @unchecked Sendable {
    private typealias Pull = CheckedContinuation<Result<[Item], Failure>, Never>

    private struct State {
        var buffer: [Item] = []
        var window = initialReadAhead
        var closed = false
        var terminal: Failure?
        var pending: Pull?
        var pendingMax = 0
        /// When the pending pull's first item arrived (`DispatchTime.uptimeNanoseconds`).
        var firstReadyAt: UInt64?
        /// Bumped by every item a pending pull receives: a burst timer that finds another value
        /// was overtaken by a newer item.
        var burst: UInt64 = 0
        var roomWaiter: CheckedContinuation<Bool, Never>?
        var pump: Task<Void, Never>?
    }

    private let state = Guarded(State())

    /// Starts pumping `source`. A thrown error is mapped by `mapError`; a source that finishes
    /// without one (and was not closed by the core) ends with `finished`.
    init(
        source: AsyncThrowingStream<Item, any Error>,
        mapError: @escaping @Sendable (any Error) -> Failure,
        finished: Failure
    ) {
        let task = Task { [weak self] in
            var iterator = source.makeAsyncIterator()
            while true {
                guard let inbox = self, await inbox.waitForRoom() else {
                    return
                }
                do {
                    guard let item = try await iterator.next() else {
                        self?.end(finished)
                        return
                    }
                    self?.deliver(item)
                } catch {
                    self?.end(mapError(error))
                    return
                }
            }
        }
        state.withLock { (current: inout State) -> Void in
            if current.closed {
                task.cancel()
            } else {
                current.pump = task
            }
        }
    }

    /// Whether the core closed the stream.
    var isClosed: Bool {
        return state.withLock { (current: inout State) -> Bool in
            return current.closed
        }
    }

    /// How the stream ended on the platform side, once it did.
    var terminal: Failure? {
        return state.withLock { (current: inout State) -> Failure? in
            return current.terminal
        }
    }

    /// How many items wait for the core.
    var buffered: Int {
        return state.withLock { (current: inout State) -> Int in
            return current.buffer.count
        }
    }

    /// The core's pull: up to `max` buffered items at once (a burst still arriving is waited for,
    /// see the top of this file); `[]` once the core closed the stream; else the terminal error
    /// (sticky); else waits for the first item. A second pull while one is pending fails with
    /// `alreadyPending`. `max` becomes the pump's window.
    func pull(max: UInt32, alreadyPending: Failure) async -> Result<[Item], Failure> {
        return await withCheckedContinuation { (continuation: Pull) in
            var timer: UInt64?
            let (answer, wake) = state.withLock { (current: inout State) -> (Result<[Item], Failure>?, CheckedContinuation<Bool, Never>?) in
                if current.closed {
                    return (.success([]), nil)
                }
                if current.pending != nil {
                    return (.failure(alreadyPending), nil)
                }
                let count = Swift.max(1, Int(clamping: max))
                current.window = count
                if current.buffer.count >= count || (!current.buffer.isEmpty && (current.terminal != nil || current.pump == nil)) {
                    // A full batch, or all there will be.
                    return (.success(PulledInbox.take(count, from: &current)), PulledInbox.room(&current))
                }
                if current.buffer.isEmpty, let terminal = current.terminal {
                    return (.failure(terminal), nil)
                }
                current.pending = continuation
                current.pendingMax = count
                current.firstReadyAt = nil
                if !current.buffer.isEmpty {
                    current.firstReadyAt = DispatchTime.now().uptimeNanoseconds
                    current.burst &+= 1
                    timer = current.burst
                }
                return (nil, PulledInbox.room(&current))
            }
            wake?.resume(returning: true)
            if let answer = answer {
                continuation.resume(returning: answer)
            }
            if let timer = timer {
                armBurstTimer(timer)
            }
        }
    }

    /// The core closed the stream: a pending pull answers `[]`, the buffer is dropped and the pump
    /// stops. Returns `false` when it was already closed.
    @discardableResult
    func close() -> Bool {
        let taken = state.withLock { (current: inout State) -> (Pull?, CheckedContinuation<Bool, Never>?, Task<Void, Never>?)? in
            if current.closed {
                return nil
            }
            current.closed = true
            current.buffer = []
            let pending = current.pending
            let waiter = current.roomWaiter
            let pump = current.pump
            current.pending = nil
            current.roomWaiter = nil
            current.pump = nil
            return (pending, waiter, pump)
        }
        guard let (pending, waiter, pump) = taken else {
            return false
        }
        pending?.resume(returning: .success([]))
        waiter?.resume(returning: false)
        pump?.cancel()
        return true
    }

    // MARK: The pump's side

    /// Waits until the buffer is under the window; `false` once the stream is closed.
    private func waitForRoom() async -> Bool {
        let ready = state.withLock { (current: inout State) -> Bool? in
            if current.closed {
                return false
            }
            return current.buffer.count < current.window ? true : nil
        }
        if let ready = ready {
            return ready
        }
        return await withCheckedContinuation { (continuation: CheckedContinuation<Bool, Never>) in
            let answer = state.withLock { (current: inout State) -> Bool? in
                if current.closed {
                    return false
                }
                if current.buffer.count < current.window {
                    return true
                }
                current.roomWaiter = continuation
                return nil
            }
            if let answer = answer {
                continuation.resume(returning: answer)
            }
        }
    }

    /// One item from the platform: into the buffer, and to the pending pull once its batch is
    /// full or its burst is over.
    private func deliver(_ item: Item) {
        var timer: UInt64?
        let answer = state.withLock { (current: inout State) -> (Pull, [Item])? in
            if current.closed {
                return nil
            }
            current.buffer.append(item)
            guard let pending = current.pending else {
                return nil
            }
            let now = DispatchTime.now().uptimeNanoseconds
            let first = current.firstReadyAt ?? now
            current.firstReadyAt = first
            if current.buffer.count >= current.pendingMax || now &- first >= burstCap {
                current.pending = nil
                return (pending, PulledInbox.take(current.pendingMax, from: &current))
            }
            current.burst &+= 1
            timer = current.burst
            return nil
        }
        if let (continuation, items) = answer {
            continuation.resume(returning: .success(items))
        }
        if let timer = timer {
            armBurstTimer(timer)
        }
    }

    /// Answers the pending pull with what it has once `burstGap` passed without another item.
    private func armBurstTimer(_ burst: UInt64) {
        DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + burstGap) { [weak self] in
            self?.burstOver(burst)
        }
    }

    private func burstOver(_ burst: UInt64) {
        let answer = state.withLock { (current: inout State) -> (Pull, [Item])? in
            guard current.burst == burst, let pending = current.pending, !current.buffer.isEmpty else {
                return nil
            }
            current.pending = nil
            return (pending, PulledInbox.take(current.pendingMax, from: &current))
        }
        if let (continuation, items) = answer {
            continuation.resume(returning: .success(items))
        }
    }

    /// The platform stream ended: the terminal is recorded (unless the core closed it first). A
    /// pending pull gets what is buffered now, or the terminal when nothing is.
    private func end(_ failure: Failure) {
        let answer = state.withLock { (current: inout State) -> (Pull, Result<[Item], Failure>)? in
            if current.closed || current.terminal != nil {
                return nil
            }
            current.terminal = failure
            current.pump = nil
            guard let pending = current.pending else {
                return nil
            }
            current.pending = nil
            if current.buffer.isEmpty {
                return (pending, .failure(failure))
            }
            return (pending, .success(PulledInbox.take(current.pendingMax, from: &current)))
        }
        if let (continuation, result) = answer {
            continuation.resume(returning: result)
        }
    }

    /// Takes up to `count` items from the front of the buffer.
    private static func take(_ count: Int, from current: inout State) -> [Item] {
        let taken = Array(current.buffer.prefix(count))
        current.buffer.removeFirst(taken.count)
        current.firstReadyAt = nil
        return taken
    }

    /// The pump's waiter, taken when the buffer is under the window again.
    private static func room(_ current: inout State) -> CheckedContinuation<Bool, Never>? {
        guard current.buffer.count < current.window, let waiter = current.roomWaiter else {
            return nil
        }
        current.roomWaiter = nil
        return waiter
    }
}
