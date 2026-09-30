// The marker protocols generated code conforms to (docs/SPEC.md section 17.3).

/// A generated record: a struct that crosses the boundary by value.
public protocol KeelRecord: KeelCodec, Sendable, Hashable {}

/// A generated enum, unit or with data, that crosses the boundary by value.
public protocol KeelEnum: KeelCodec, Sendable, Hashable {}

/// A generated error enum (`#[keel::error]`): an enum that is also a Swift `Error`.
public protocol KeelError: KeelCodec, Error, Sendable, Hashable {}

/// A generated port protocol: a trait the platform implements and the core calls.
///
/// Generated port protocols additionally require `Sendable`; the adapter that registers an
/// implementation (`<name>PortImpl(_:)`) returns a `PortImpl` for `KeelCore.registerPort`.
public protocol KeelPort {}
