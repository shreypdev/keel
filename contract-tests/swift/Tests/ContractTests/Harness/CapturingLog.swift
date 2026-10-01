import Foundation
@testable import UndraRuntime

/// The `Log` port of the harness: keeps every record `(level, target, message)` the core writes.
/// Levels are 0 trace, 1 debug, 2 info, 3 warn, 4 error, 5 fatal.
final class CapturingLog: UndraAdapter, @unchecked Sendable {
    /// One log record.
    struct Record: Equatable, Sendable {
        let level: UInt8
        let target: String
        let message: String
    }

    private let records = Locked<[Record]>([])

    /// A one-shot hook for S17.5: `run` is called, once, from inside the Log port (on the thread the
    /// core calls it on, which holds the core's lock) for the first record `matching` accepts.
    private struct Trigger {
        let matching: @Sendable (UInt8, String, String) -> Bool
        let run: @Sendable () -> Void
    }

    private let trigger = Locked<Trigger?>(nil)

    /// Arms the hook; the next record that `matching` (level, target, message) accepts calls `run`
    /// once, and the hook is cleared. Replaces an armed hook.
    func onNextRecord(
        where matching: @escaping @Sendable (UInt8, String, String) -> Bool,
        run: @escaping @Sendable () -> Void
    ) {
        trigger.withLock { (current: inout Trigger?) -> Void in
            current = Trigger(matching: matching, run: run)
        }
    }

    var portId: UInt32 {
        return StandardPorts.Log.portId
    }

    /// Every record so far, oldest first.
    var all: [Record] {
        return records.snapshot
    }

    func makePortImpl(core: UndraCore) -> PortImpl? {
        let records = self.records
        let trigger = self.trigger
        return .sync([
            StandardPorts.Log.log: { args in
                var reader = UndraReader(args)
                let level = try reader.readU8()
                let target = try reader.readString()
                let message = try reader.readString()
                try reader.finish()
                records.withLock { (current: inout [Record]) -> Void in
                    current.append(Record(level: level, target: target, message: message))
                }
                // Taken out under the lock, run outside it: the hook calls into the core.
                let armed = trigger.withLock { (current: inout Trigger?) -> Trigger? in
                    guard let hook = current, hook.matching(level, target, message) else {
                        return nil
                    }
                    current = nil
                    return hook
                }
                armed?.run()
                return []
            },
        ])
    }
}
