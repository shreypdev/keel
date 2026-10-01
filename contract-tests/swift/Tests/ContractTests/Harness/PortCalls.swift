import Foundation
@testable import UndraRuntime

/// Counts the calls the core makes to the harness's ports, per port, across reloads of the core.
/// S17.7 reads it to show that a shut-down core calls none of them.
final class PortCallCounter: @unchecked Sendable {
    private let counts = Locked<[UInt32: Int]>([:])

    /// Notes one call to `portId`.
    func note(_ portId: UInt32) {
        counts.withLock { (current: inout [UInt32: Int]) -> Void in
            current[portId, default: 0] += 1
        }
    }

    /// The calls to `portId` so far.
    func count(_ portId: UInt32) -> Int {
        return counts.withLock { (current: inout [UInt32: Int]) -> Int in current[portId] ?? 0 }
    }

    /// The calls so far, per port.
    var all: [UInt32: Int] {
        return counts.snapshot
    }
}

/// An adapter that serves its port with `inner`'s implementation and counts every call the core
/// makes to it in `counter`, before the implementation runs.
struct CountingAdapter: UndraAdapter {
    let inner: any UndraAdapter
    let counter: PortCallCounter

    var portId: UInt32 {
        return inner.portId
    }

    func makePortImpl(core: UndraCore) -> PortImpl? {
        guard let impl = inner.makePortImpl(core: core) else {
            return nil
        }
        let portId = inner.portId
        let counter = self.counter
        switch impl {
        case .sync(let methods):
            return .sync(methods.mapValues { (method: @escaping SyncPortMethod) -> SyncPortMethod in
                return { (args: [UInt8]) throws -> [UInt8] in
                    counter.note(portId)
                    return try method(args)
                }
            })
        case .async(let methods):
            return .async(methods.mapValues { (method: @escaping AsyncPortMethod) -> AsyncPortMethod in
                return { (args: [UInt8]) async throws -> [UInt8] in
                    counter.note(portId)
                    return try await method(args)
                }
            })
        }
    }

    func attach(to core: UndraCore) {
        inner.attach(to: core)
    }

    func detach() {
        inner.detach()
    }
}
