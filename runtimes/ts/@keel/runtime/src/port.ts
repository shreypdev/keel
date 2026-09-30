/**
 * A host implementation of one core port, in the byte-level form the core
 * speaks (docs/SPEC.md sections 5.7, 6.3 and 17.1). Generated code and
 * `adapters/ports.ts` build these around typed implementations; register one
 * with `KeelCore.registerPort(portId, impl)`.
 *
 * `methods` maps a method id (`fnv1a32("<Trait>.<method>")`) to a function
 * from the encoded arguments to the encoded return value. A function may
 * throw `KeelPortError(body)` to fail with the port's typed error (the core
 * gets status 1 and `body`); any other exception is logged and answered as
 * "unavailable".
 *
 * `sync: true` promises that every method returns its bytes directly, which is
 * what a synchronous core call (a `#[keel::port(sync)]` port) needs. Sync
 * ports work in the `wasm-main` mode only, where the core waits inside the
 * call; in `wasm-worker` and `remote` mode the core cannot block on the main
 * thread, so it sees them as unavailable unless it built the answer itself
 * (Clock, Rng and Log have built-in bindings).
 */
export interface PortImpl {
  /** Whether every method answers synchronously. */
  readonly sync: boolean;
  /** Implementations by method id. */
  readonly methods: Readonly<Record<number, (args: Uint8Array) => Uint8Array | Promise<Uint8Array>>>;
}
