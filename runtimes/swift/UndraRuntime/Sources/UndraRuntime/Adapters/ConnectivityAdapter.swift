// Connectivity and Lifecycle: the two event ports (docs/SPEC.md section 8). The host reports;
// the core listens.
//
//   Connectivity.changed(online: bool, kind: NetKind)     from NWPathMonitor (this adapter)
//   Lifecycle.changed(state: AppState)                    from the app (`UndraLifecycle`)

import Foundation
import Network

/// `Connectivity`: reports network reachability to the core with `NWPathMonitor`.
///
/// The monitor delivers the current path as soon as it starts, so the core learns the initial
/// state without asking. It has no methods the core calls, hence no `PortImpl`.
public final class ConnectivityAdapter: UndraAdapter, @unchecked Sendable {
    private let monitor = Guarded<NWPathMonitor?>(nil)
    private let queue = DispatchQueue(label: "dev.undra.connectivity", qos: .utility)

    /// Creates the adapter.
    public init() {}

    public var portId: UInt32 {
        return StandardPorts.Connectivity.portId
    }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return nil
    }

    public func attach(to core: UndraCore) {
        let pathMonitor = NWPathMonitor()
        pathMonitor.pathUpdateHandler = { [weak core] path in
            guard let core = core else {
                return
            }
            let report = ConnectivityAdapter.classify(status: path.status, usesWifi: path.usesInterfaceType(.wifi),
                                                      usesCellular: path.usesInterfaceType(.cellular),
                                                      usesWired: path.usesInterfaceType(.wiredEthernet))
            core.event(
                port: StandardPorts.Connectivity.portId,
                method: StandardPorts.Connectivity.changed,
                payload: ConnectivityAdapter.encodeChanged(online: report.online, kind: report.kind)
            )
        }
        let previous = monitor.withLock { (current: inout NWPathMonitor?) -> NWPathMonitor? in
            let old = current
            current = pathMonitor
            return old
        }
        previous?.cancel()
        pathMonitor.start(queue: queue)
    }

    public func detach() {
        let existing = monitor.withLock { (current: inout NWPathMonitor?) -> NWPathMonitor? in
            let old = current
            current = nil
            return old
        }
        existing?.cancel()
    }

    /// Turns a path into what the core is told: online only when the path is satisfied, and the
    /// most specific interface it uses.
    static func classify(
        status: NWPath.Status,
        usesWifi: Bool,
        usesCellular: Bool,
        usesWired: Bool
    ) -> (online: Bool, kind: PortNetKind) {
        if status != .satisfied {
            return (online: false, kind: .disconnected)
        }
        if usesWifi {
            return (online: true, kind: .wifi)
        }
        if usesCellular {
            return (online: true, kind: .cellular)
        }
        if usesWired {
            return (online: true, kind: .wired)
        }
        return (online: true, kind: .unknown)
    }

    /// The parameters of `Connectivity.changed(online, kind)`, encoded.
    static func encodeChanged(online: Bool, kind: PortNetKind) -> [UInt8] {
        var writer = UndraWriter()
        writer.writeBool(online)
        kind.undraEncode(&writer)
        return writer.finish()
    }
}

/// Reports the app's lifecycle to the core (`Lifecycle.changed`). The core refetches stale
/// queries when the app becomes active, so call it from wherever the app learns its phase:
///
/// ```swift
/// .onChange(of: scenePhase) { _, phase in
///     switch phase {
///     case .active: UndraLifecycle().changed(.active)
///     case .inactive: UndraLifecycle().changed(.inactive)
///     case .background: UndraLifecycle().changed(.background)
///     @unknown default: break
///     }
/// }
/// ```
public struct UndraLifecycle: Sendable {
    private let core: UndraCore

    /// A reporter for `core` (the shared core by default).
    public init(core: UndraCore = .shared) {
        self.core = core
    }

    /// Tells the core the app is now in `state`.
    public func changed(_ state: UndraAppState) {
        core.event(
            port: StandardPorts.Lifecycle.portId,
            method: StandardPorts.Lifecycle.changed,
            payload: UndraLifecycle.encodeChanged(state)
        )
    }

    /// The parameters of `Lifecycle.changed(state)`, encoded.
    static func encodeChanged(_ state: UndraAppState) -> [UInt8] {
        var writer = UndraWriter()
        state.undraEncode(&writer)
        return writer.finish()
    }
}
