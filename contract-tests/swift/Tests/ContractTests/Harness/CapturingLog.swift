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

    var portId: UInt32 {
        return StandardPorts.Log.portId
    }

    /// Every record so far, oldest first.
    var all: [Record] {
        return records.snapshot
    }

    func makePortImpl(core: UndraCore) -> PortImpl? {
        let records = self.records
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
                return []
            },
        ])
    }
}
