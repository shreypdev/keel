import Foundation
import UndraRuntime

/// The `Connectivity` port's event (docs/SPEC.md section 8), raised by the app: the core is told
/// whether the network is reachable, and queues or replays idempotent mutations accordingly.
///
/// The ids are those of the standard port, `fnv1a32("port.Connectivity")` and
/// `fnv1a32("Connectivity.changed")`; the parameters are `online: bool, kind: NetKind`, with
/// `NetKind` the runtime's public type.
enum WireConnectivity {
    /// `fnv1a32("port.Connectivity")`.
    static let portId = fnv1a32("port.Connectivity")
    /// `fnv1a32("Connectivity.changed")`.
    static let changedMethod = fnv1a32("Connectivity.changed")

    /// The parameters of `Connectivity.changed(online, kind)`: Wi-Fi while online, none otherwise.
    static func changed(online: Bool) -> [UInt8] {
        var writer = UndraWriter()
        online.undraEncode(&writer)
        (online ? NetKind.wifi : NetKind.disconnected).undraEncode(&writer)
        return writer.finish()
    }
}
