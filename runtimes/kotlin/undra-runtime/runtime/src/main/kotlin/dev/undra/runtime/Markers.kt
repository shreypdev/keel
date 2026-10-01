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
 *   typed error; any other exception answers `unavailable` (the core sees `PortError::Unavailable`).
 * @property detach called once when the implementation stops serving its core: the core is closed, or another
 *   implementation was registered for the same port id. Ports that hold platform resources for the core (the
 *   connections of the `WebSocket` and `Sse` bindings, the databases of `Db`) release them here. It must return
 *   quickly (start slow work on a thread of its own) and must not call into Undra. `null` for ports that hold nothing.
 */
public class PortImpl(
    public val sync: Boolean,
    public val methods: Map<UInt, suspend (ByteArray) -> ByteArray>,
    public val detach: (() -> Unit)? = null,
)
