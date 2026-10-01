package dev.undra.runtime

/** Marker of generated records (`#[undra::api]` structs); their companion is an `UndraCodec`. */
public interface UndraRecord

/** Marker of generated enums (`#[undra::api]` enums, unit or with data). */
public interface UndraEnum

/** Marker of generated port interfaces (`#[undra::port]` traits). */
public interface UndraPort

/**
 * What generated code registers with [UndraCore.registerPort]: the platform's implementation of one
 * port, adapted to bytes.
 *
 * @property sync `true` for a `#[undra::port(sync)]` port. The runtime answers such calls inline, on the
 *   thread the core called from, with the core lock possibly held, so a sync implementation must
 *   return without suspending and must never call back into Undra.
 * @property methods maps a port method id (`fnv1a32("<Trait>.<method>")`) to a function from the
 *   encoded arguments to the encoded reply body. Throwing [UndraPortException] answers with the port's
 *   typed error (status 1), and so does a standard port's own error type thrown by one of its methods
 *   (`StorageError` from `Kv` and `SecureStore`, `FsError` from `Fs`, `HttpError` from `Http`). Any other
 *   exception is logged at error level, passed to `LoadOptions.onError` and answers `unavailable` (the core
 *   sees `PortError::Unavailable`).
 */
public class PortImpl(
    public val sync: Boolean,
    public val methods: Map<UInt, suspend (ByteArray) -> ByteArray>,
)
