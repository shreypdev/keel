/**
 * A host implementation of one core port, in the byte-level form the core
 * speaks (docs/SPEC.md sections 5.7, 6.3 and 17.1). Generated code and
 * `adapters/ports.ts` build these around typed implementations; register one
 * with `UndraCore.registerPort(portId, impl)`.
 *
 * `methods` maps a method id (`fnv1a32("<Trait>.<method>")`) to a function
 * from the encoded arguments to the encoded return value. A function may
 * throw `UndraPortError(body)` to fail with the port's typed error (the core
 * gets status 1 and `body`); any other exception is logged and answered as
 * "unavailable".
 *
 * `sync: true` promises that every method returns its bytes directly, which is
 * what a synchronous core call (a `#[undra::port(sync)]` port) needs: the core
 * waits inside the call. In `wasm-main` mode that is the main thread. In
 * `wasm-worker` mode the core runs in the worker and cannot wait for the main
 * thread, so a sync port must be implemented in the worker, in the module of
 * `LoadOptions.worker.ports` (ADR-049); registering one on the main thread is
 * an error (at load, or from `registerPort`). Clock, Rng and Log have built-in
 * bindings in the core, which answer them wherever it runs unless a port of
 * that id is implemented there.
 */
export interface PortImpl {
  /**
   * The port's name (`"Locale"`), for what the runtime says about it: the refusal of a synchronous port in
   * `wasm-worker` mode, a failure of the implementation. Generated adapters set it; without it the runtime names
   * the port by its id.
   */
  readonly name?: string;
  /** Whether every method answers synchronously. */
  readonly sync: boolean;
  /** Implementations by method id. */
  readonly methods: Readonly<Record<number, (args: Uint8Array) => Uint8Array | Promise<Uint8Array>>>;
  /**
   * Called when the core that registered this port closes, and when a wasm core restarts after a
   * trap (`crashRecovery`, ADR-049): a port that holds platform resources for the core (the
   * WebSocket connections of `webSocketPort`, the databases and open transactions of `dbPort`)
   * releases them here. After a restart the same instance serves the new core instance, so a call
   * that arrives after `dispose` starts afresh. `registerPort` does not dispose the port it
   * replaces: call its `dispose` when it holds something. Register one instance per core.
   * Optional; it must not throw (the main entry carries no handler for it, ADR-052).
   */
  readonly dispose?: () => void;
}

/**
 * The base of every generated port interface (SPEC 17.1): `export interface Locale extends UndraPort { hello(): string }`.
 * It has no members; it marks a type as the host side of a core port, which the generated `<name>PortImpl` adapter
 * turns into a {@link PortImpl}.
 */
export interface UndraPort {}
