// Adapters: the platform's implementations of the standard ports (docs/SPEC.md sections 8 and 11).

/// An implementation of one port for this platform.
///
/// `KeelCore.load` registers every adapter of `LoadOptions.adapters`: it asks each for its
/// `PortImpl` (event-only adapters such as Connectivity have none) and then calls `attach(to:)`,
/// which is where an adapter that starts work of its own (a timer, a network monitor) gets the
/// core to report to. `KeelCore.shutdown()` calls `detach()`.
public protocol KeelAdapter: Sendable {
    /// The port id, `fnv1a32("port.<Trait>")`.
    var portId: UInt32 { get }

    /// The method table the core calls, or `nil` for a port the host only emits events for.
    func makePortImpl(core: KeelCore) -> PortImpl?

    /// Called once, after the port is registered, with the core that now owns the adapter.
    func attach(to core: KeelCore)

    /// Called when the core shuts down. Stop any work started in `attach(to:)`.
    func detach()
}

extension KeelAdapter {
    public func attach(to core: KeelCore) {}

    public func detach() {}
}

/// An adapter that serves a ready-made `PortImpl`, for example one produced by a generated
/// `<name>PortImpl(_:)` function around the app's own implementation.
public struct PortImplAdapter: KeelAdapter {
    public let portId: UInt32
    private let impl: PortImpl

    /// Serves `impl` for port `portId`.
    public init(portId: UInt32, impl: PortImpl) {
        self.portId = portId
        self.impl = impl
    }

    public func makePortImpl(core: KeelCore) -> PortImpl? {
        return impl
    }
}

/// The set of adapters a core is loaded with: at most one per port id.
///
/// ```swift
/// // The defaults, with the app's own Http implementation instead of URLSession:
/// let adapters = Adapters.platformDefault
///     .replacing(portId: KeelIds.Ports.Http.portId, with: httpPortImpl(MyHttp()))
/// let core = try KeelCore.load(.inproc(adapters: adapters, expectedSchemaHash: KeelIds.schemaHash))
/// ```
public struct Adapters: Sendable {
    /// The adapters, in registration order.
    public private(set) var all: [any KeelAdapter]

    /// Creates a set from `adapters`; a later adapter replaces an earlier one for the same port.
    public init(_ adapters: [any KeelAdapter] = []) {
        self.all = []
        for adapter in adapters {
            self = self.replacing(adapter)
        }
    }

    /// No adapters: the app registers every port the core needs itself.
    public static var none: Adapters {
        return Adapters([])
    }

    /// This set with `adapter` in place of any adapter for the same port (or appended).
    public func replacing(_ adapter: any KeelAdapter) -> Adapters {
        var copy = self
        var replaced = false
        var index = 0
        while index < copy.all.count {
            if copy.all[index].portId == adapter.portId {
                copy.all[index] = adapter
                replaced = true
            }
            index += 1
        }
        if !replaced {
            copy.all.append(adapter)
        }
        return copy
    }

    /// This set with `impl` serving `portId` in place of the default adapter.
    public func replacing(portId: UInt32, with impl: PortImpl) -> Adapters {
        return replacing(PortImplAdapter(portId: portId, impl: impl))
    }

    /// This set without the adapter for `portId`.
    public func removing(portId: UInt32) -> Adapters {
        var copy = self
        copy.all.removeAll { (adapter: any KeelAdapter) -> Bool in
            return adapter.portId == portId
        }
        return copy
    }
}
