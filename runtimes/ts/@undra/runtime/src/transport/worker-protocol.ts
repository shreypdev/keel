import type { TransportFailure } from "../errors.js";
import type { HelloPayload } from "../wire/index.js";

/*
 * The messages between the main thread and the worker of the `wasm-worker`
 * mode. The data path is the Undra envelope (SPEC 3.2): every core-bound and
 * host-bound message is a framed envelope whose `ArrayBuffer` is transferred,
 * not copied. Around it sit control messages for what an envelope cannot
 * carry: loading the module, the startup verdict, the host's ports, statistics,
 * snapshots, restores and shutdown.
 *
 * Both ends ship in one package, so the protocol is internal; its version still
 * travels in `init` so a worker never sends a shape the host cannot read.
 *
 * Ports (protocol 3, ADR-049). The core calls synchronous ports (Clock, Rng, Log, an app's
 * `#[undra::port(sync)]` port) while it runs and cannot wait for the main thread, so the worker
 * answers every port call where it can: a port implemented in the worker (the module named by
 * `portsModule`) is answered there; a port the host registered with asynchronous methods (the ids
 * in `asyncPorts`, kept current by `ports` messages) crosses to the host as a `PortCall` envelope;
 * any other port is "unavailable", which makes the core's built-in bindings answer Clock, Rng and
 * Log from the worker's own clock, `crypto` and `log` import.
 *
 * Snapshot and restore (SPEC 5.9) are request/answer pairs matched by `id`: `snapshot` to
 * `snapshot` and `restore` to `restored`. They are control messages and not envelopes because
 * `Kind.Restore` over the envelope path has no acknowledgement (the host could not tell when the
 * restored values reached it, or that the core refused the bytes). A worker announces them in
 * {@link WORKER_FEATURES} of its `ready` message, and a host never sends them to a worker that
 * did not. The worker posts `restored` after the envelopes the restore produced (`post` flushes
 * the batch first), so when the host reads the acknowledgement the change-sets that re-deliver
 * every observed signal (ADR-023) and the replies of the calls the restore cancelled have already
 * been handed to the host's handler.
 */

/**
 * The version of this protocol. 3 (ADR-049): `init` carries `asyncPorts` and
 * `portsModule`, `ports` keeps the set current, and the snapshot payloads are
 * `data`. 2 (ADR-031): the worker sends the envelopes one task produced as one
 * `envelopes` message; 1 sent one `envelope` message each. A host accepts every
 * shape; a worker batches only for a host that announced 2 or more, and answers
 * ports as protocol 3 says only for a host that announced 3 (before that, every
 * port but Clock, Rng and Log crosses to the host).
 */
export const WORKER_PROTOCOL_VERSION = 3;

/** What a worker can do beyond the base protocol, announced in its `ready` message (see the header). */
export const WORKER_FEATURES = Object.freeze(["snapshot", "ports"] as const);

/** One of {@link WORKER_FEATURES}. */
export type WorkerFeature = (typeof WORKER_FEATURES)[number];

/** How the module travels to the worker. A `URL` cannot be structured-cloned, so it goes as its text. */
export type WorkerWasm =
  | { readonly kind: "url"; readonly href: string }
  | { readonly kind: "bytes"; readonly bytes: ArrayBuffer }
  | { readonly kind: "module"; readonly module: WebAssembly.Module };

/** Main thread to worker. */
export type HostToWorker =
  | {
      readonly t: "init";
      readonly wasm: WorkerWasm;
      readonly expectedSchemaHash: bigint;
      readonly platform: string;
      readonly devtools: boolean;
      readonly logLevel: number;
      /** The host's {@link WORKER_PROTOCOL_VERSION}; absent from a version 1 host. */
      readonly protocol?: number;
      /** Protocol 3: the ids of the ports the host serves asynchronously; their calls cross to the host. */
      readonly asyncPorts?: readonly number[];
      /** Protocol 3: the URL of the module of `LoadOptions.worker.ports`, imported before `undra_init`. */
      readonly portsModule?: string;
    }
  | { readonly t: "envelope"; readonly data: ArrayBuffer }
  | { readonly t: "stats"; readonly id: number }
  /** Protocol 3: the host registered a port after load; `asyncPorts` replaces the set of `init`. */
  | { readonly t: "ports"; readonly asyncPorts: readonly number[] }
  /** Asks for `undra_snapshot`; answered by a `snapshot` message with the same `id`. */
  | { readonly t: "snapshot"; readonly id: number }
  /** Asks for `undra_restore(data)`; `data` is a private copy, transferred. Answered by `restored` with the same `id`. */
  | { readonly t: "restore"; readonly id: number; readonly data: ArrayBuffer }
  | { readonly t: "close" };

/** A failure, in a form that survives structured cloning. */
export type WorkerFailure =
  | { readonly kind: "schemaMismatch"; readonly expected: bigint; readonly got: bigint }
  /** `stack` is the engine's stack of a trap (`reason: "trap"`), for the panic report. */
  | { readonly kind: "transport"; readonly reason: TransportFailure; readonly message: string; readonly stack?: string }
  | { readonly kind: "error"; readonly message: string };

/** Worker to main thread. */
export type WorkerToHost =
  /** The core is up. `features` lists what this worker script can do beyond the base protocol; absent from a worker that predates it. */
  | { readonly t: "ready"; readonly hello: HelloPayload; readonly features?: readonly WorkerFeature[] }
  | { readonly t: "failed"; readonly failure: WorkerFailure }
  | { readonly t: "envelope"; readonly data: ArrayBuffer }
  /** Protocol 2: every envelope the worker produced during one task, in order, all transferred. */
  | { readonly t: "envelopes"; readonly data: readonly ArrayBuffer[] }
  | { readonly t: "stats"; readonly id: number; readonly json: string | null }
  /** The answer to `snapshot`: the snapshot `data` (transferred), or why there is none. */
  | { readonly t: "snapshot"; readonly id: number; readonly data?: ArrayBuffer; readonly failure?: WorkerFailure }
  /**
   * The answer to `restore`. `code` is what `undra_restore` returned (0: restored; anything else:
   * the core refused the bytes and is unchanged). `failure` is present only when `undra_restore`
   * could not be run at all (the core is closed, the module exports none, it trapped).
   */
  | { readonly t: "restored"; readonly id: number; readonly code: number; readonly failure?: WorkerFailure }
  | { readonly t: "closed"; readonly failure: WorkerFailure };
