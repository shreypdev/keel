// A small mutex-protected value. The runtime's shared state (call tables, the mirror's queue,
// stream buffers) is touched from the core's threads, the caller's threads and the main actor,
// and none of those sections may suspend, so a plain lock is the right tool: nothing here ever
// awaits while holding it, and no closure passed to `withLock` may call back into the core.

#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif

/// A value protected by a `pthread_mutex_t`.
///
/// `withLock` runs `body` with exclusive access to the value. Keep bodies short and never call
/// out of them (no callbacks, no `resume` of a continuation, no `undra_*` function): compute what
/// to do inside, do it after the lock is released.
final class Guarded<Value>: @unchecked Sendable {
    private let mutex: UnsafeMutablePointer<pthread_mutex_t>
    private var value: Value

    init(_ initial: Value) {
        let raw = UnsafeMutablePointer<pthread_mutex_t>.allocate(capacity: 1)
        raw.initialize(to: pthread_mutex_t())
        pthread_mutex_init(raw, nil)
        self.mutex = raw
        self.value = initial
    }

    deinit {
        pthread_mutex_destroy(mutex)
        mutex.deinitialize(count: 1)
        mutex.deallocate()
    }

    /// Runs `body` while holding the lock and returns its result.
    func withLock<Output>(_ body: (inout Value) throws -> Output) rethrows -> Output {
        pthread_mutex_lock(mutex)
        defer { pthread_mutex_unlock(mutex) }
        return try body(&value)
    }
}
