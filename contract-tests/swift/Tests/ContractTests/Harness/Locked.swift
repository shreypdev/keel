import Foundation

/// A value behind an `NSLock`: what the fakes use, because the core calls ports from its own
/// threads while the test reads them from the main actor.
final class Locked<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Value

    init(_ value: Value) {
        self.value = value
    }

    /// Runs `body` with exclusive access to the value.
    func withLock<Result>(_ body: (inout Value) throws -> Result) rethrows -> Result {
        lock.lock()
        defer { lock.unlock() }
        return try body(&value)
    }

    /// A copy of the current value.
    var snapshot: Value {
        return withLock { (current: inout Value) -> Value in current }
    }
}
