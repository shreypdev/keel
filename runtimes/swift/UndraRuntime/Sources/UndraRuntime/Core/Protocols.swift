// The marker protocols generated code conforms to (docs/SPEC.md section 17.3).

/// A generated record: a struct that crosses the boundary by value.
public protocol UndraRecord: UndraCodec, Sendable, Hashable {}

/// A generated enum, unit or with data, that crosses the boundary by value.
public protocol UndraEnum: UndraCodec, Sendable, Hashable {}

/// A generated error enum (`#[undra::error]`): an enum that is also a Swift `Error`.
public protocol UndraError: UndraCodec, Error, Sendable, Hashable {}

/// A generated port protocol: a trait the platform implements and the core calls.
///
/// Generated port protocols additionally require `Sendable`; the adapter that registers an
/// implementation (`<name>PortImpl(_:)`) returns a `PortImpl` for `UndraCore.registerPort`.
public protocol UndraPort {}
