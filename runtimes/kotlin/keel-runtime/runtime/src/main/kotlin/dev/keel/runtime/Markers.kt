package dev.keel.runtime

/** Marker of generated records (`#[keel::api]` structs); their companion is a `KeelCodec`. */
public interface KeelRecord

/** Marker of generated enums (`#[keel::api]` enums, unit or with data). */
public interface KeelEnum

/** Marker of generated port interfaces (`#[keel::port]` traits). */
public interface KeelPort

/**
 * What generated code registers with [KeelCore.registerPort]: the platform's implementation of one
 * port, adapted to bytes.
 *
 * @property sync `true` for a `#[keel::port(sync)]` port. The runtime answers such calls inline, on the
 *   thread the core called from, with the core lock possibly held, so a sync implementation must
 *   return without suspending and must never call back into Keel.
 * @property methods maps a port method id (`fnv1a32("<Trait>.<method>")`) to a function from the
 *   encoded arguments to the encoded reply body. Throwing [KeelPortException] answers with the port's
 *   typed error; any other exception answers `unavailable` (the core sees `PortError::Unavailable`).
 */
public class PortImpl(
    public val sync: Boolean,
    public val methods: Map<UInt, suspend (ByteArray) -> ByteArray>,
)
