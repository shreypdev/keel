// The transport behind `UndraCore.shared` (and a generated entry's `core`) when no core is loaded
// (ADR-032, decision 7; ADR-044).
//
// It reaches nothing and calls no core's table. The core built over it starts out shut down,
// so every call fails before it gets here; the transport still answers every requirement with the
// same "closed" outcome, so a future caller cannot reach a core that is not there.

/// A transport that is permanently closed.
final class UnloadedTransport: UndraTransport, @unchecked Sendable {
    init() {}

    var mode: UndraMode {
        return .inproc
    }

    var supportsDirectSync: Bool {
        return true
    }

    func start(inbound: any UndraInbound, options: TransportStartOptions) throws -> TransportInfo {
        throw UndraTransportError.closed
    }

    func send(call payload: [UInt8]) -> Bool {
        return false
    }

    func callSync(_ payload: [UInt8]) throws -> [UInt8] {
        throw UndraTransportError.closed
    }

    func cancel(callId: UInt32) {}

    func streamCredit(callId: UInt32, credit: UInt32) {}

    func observe(handle: UndraHandle, signal: UInt32, on: Bool) {}

    func release(handle: UndraHandle) {}

    func registerPort(_ portId: UInt32) {}

    func portReply(_ payload: [UInt8]) {}

    func event(portId: UInt32, methodId: UInt32, payload: [UInt8]) {}

    func timerFired(_ timerId: UInt32) {}

    func snapshot() throws -> [UInt8] {
        throw UndraTransportError.closed
    }

    func restore(_ payload: [UInt8]) throws {
        throw UndraTransportError.closed
    }

    func statsJSON() -> String? {
        return nil
    }

    func shutdown() {}
}
