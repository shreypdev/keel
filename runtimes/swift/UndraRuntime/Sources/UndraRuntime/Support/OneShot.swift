import Dispatch

/// A value that is produced once by one thread and awaited, blocking, by another.
///
/// Used where the runtime must present a synchronous API over an asynchronous transport (the
/// remote transport's `callSync` and `construct`, and the WebSocket handshake).
final class OneShot<Value: Sendable>: @unchecked Sendable {
    private let semaphore = DispatchSemaphore(value: 0)
    private let slot = Guarded<Value?>(nil)

    init() {}

    /// Stores `value` and wakes the waiter. Returns `false` (and drops `value`) if a value was
    /// already stored.
    @discardableResult
    func fulfill(_ value: Value) -> Bool {
        let first = slot.withLock { (current: inout Value?) -> Bool in
            if current != nil {
                return false
            }
            current = value
            return true
        }
        if first {
            semaphore.signal()
        }
        return first
    }

    /// Blocks until a value is stored, or `timeoutSeconds` elapse (then returns `nil`).
    func wait(timeoutSeconds: Double) -> Value? {
        let deadline = DispatchTime.now() + timeoutSeconds
        _ = semaphore.wait(timeout: deadline)
        // Read the slot whatever the wait reported: a value stored just after a timeout is
        // still a value.
        return slot.withLock { (current: inout Value?) -> Value? in
            return current
        }
    }
}
