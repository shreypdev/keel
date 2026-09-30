// Timer: the core arms timers with `Timer.set(timer_id, delay_ms)` and the host calls
// `keel_timer_fired(timer_id)` when they come due (docs/SPEC.md section 5.8).

import Foundation

/// `Timer` on `DispatchQueue`: one `asyncAfter` per armed timer.
///
/// Registering it replaces the core's own timer thread with the platform's timers.
public final class TimerAdapter: KeelAdapter, @unchecked Sendable {
    private struct Owner {
        weak var core: KeelCore?
    }

    private let owner = Guarded<Owner>(Owner())
    private let queue = DispatchQueue(label: "dev.keel.timer", qos: .userInitiated)

    /// The longest delay honoured, in milliseconds (about 100 years): far beyond it the
    /// conversion to a dispatch time could overflow.
    static let maxDelayMs: UInt64 = 100 * 365 * 24 * 3600 * 1000

    /// Creates the adapter.
    public init() {}

    public var portId: UInt32 {
        return StandardPorts.Timer.portId
    }

    public func makePortImpl(core: KeelCore) -> PortImpl? {
        return .sync([
            StandardPorts.Timer.set: { [weak self] args in
                var reader = KeelReader(args)
                let timerId = try reader.readU32()
                let delayMs = try reader.readU64()
                try reader.finish()
                self?.arm(timerId: timerId, delayMs: delayMs)
                return []
            },
        ])
    }

    public func attach(to core: KeelCore) {
        owner.withLock { (current: inout Owner) -> Void in
            current.core = core
        }
    }

    public func detach() {
        owner.withLock { (current: inout Owner) -> Void in
            current.core = nil
        }
    }

    /// Fires `timerFired(timerId)` on the core after `delayMs` milliseconds.
    func arm(timerId: UInt32, delayMs: UInt64) {
        let capped = Swift.min(delayMs, TimerAdapter.maxDelayMs)
        let deadline = DispatchTime.now() + .milliseconds(Int(capped))
        queue.asyncAfter(deadline: deadline) { [weak self] in
            guard let self = self else {
                return
            }
            let core = self.owner.withLock { (current: inout Owner) -> KeelCore? in
                return current.core
            }
            core?.timerFired(timerId)
        }
    }
}
