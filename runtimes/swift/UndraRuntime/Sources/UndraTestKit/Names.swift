import UndraRuntime

/// `(trait, methods)` of the ten standard ports (docs/SPEC.md section 8).
private let standardPorts: [(String, [String])] = [
    ("Clock", ["now_ms", "monotonic_ns"]),
    ("Rng", ["fill"]),
    ("Log", ["log"]),
    ("Http", ["request"]),
    ("Kv", ["get", "set", "delete", "list"]),
    ("SecureStore", ["get", "set", "delete", "list"]),
    ("Fs", ["read", "write", "delete", "list"]),
    ("Timer", ["set"]),
    ("Connectivity", ["changed"]),
    ("Lifecycle", ["changed"]),
]

/// The port id of `Trait`: `fnv1a32("port.<Trait>")`.
public func undraPortId(_ trait: String) -> UInt32 {
    return fnv1a32("port.\(trait)")
}

/// The method id of `Trait.method`: `fnv1a32("<Trait>.<method>")`.
public func undraMethodId(_ trait: String, _ method: String) -> UInt32 {
    return fnv1a32("\(trait).\(method)")
}

/// `"Http.request"` for the standard port method (`port`, `method`), `nil` for anything else. The ids stay authoritative; the name is for
/// the person reading a recording.
public func undraStandardName(port: UInt32, method: UInt32) -> String? {
    for (name, methods) in standardPorts where undraPortId(name) == port {
        for m in methods where undraMethodId(name, m) == method {
            return "\(name).\(m)"
        }
    }
    return nil
}

/// The method ids of the standard port `port`, if it is one.
func standardMethodIds(port: UInt32) -> [UInt32] {
    for (name, methods) in standardPorts where undraPortId(name) == port {
        return methods.map { undraMethodId(name, $0) }
    }
    return []
}
