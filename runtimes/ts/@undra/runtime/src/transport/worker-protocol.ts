import type { TransportFailure } from "../errors.js";
import type { HelloPayload } from "../wire/index.js";

/*
 * The messages between the main thread and the worker of the `wasm-worker`
 * mode. The data path is the Undra envelope (SPEC 3.2): every core-bound and
 * host-bound message is a framed envelope whose `ArrayBuffer` is transferred,
 * not copied. Around it sit four control messages for what an envelope cannot
 * carry: loading the module, the startup verdict, statistics, snapshots, restores
 * and shutdown.
 *
 * Both ends ship in one package, so the protocol is internal; its version still
 * travels in `init` so a worker never sends a shape the host cannot read.
 *
 * Snapshot and restore (SPEC 5.9) are request/answer pairs matched by `id`: `snapshot` to
 * `snapshot` and `restore` to `restored`. They are control messages and not envelopes because
 * `Kind.Restore` over the envelope path has no acknowledgement (the host could not tell when the
 * restored values reached it, or that the core refused the bytes). They are additive, so
 * {@link WORKER_PROTOCOL_VERSION} did not change: a worker that understands them says so in
 * {@link WORKER_FEATURES} of its `ready` message, and a host never sends them to a worker that
 * did not (a script left over from an older build would ignore them and the request would wait
 * forever). The worker posts `restored` after the envelopes the restore produced (`post` flushes
 * the batch first), so when the host reads the acknowledgement the change-sets that re-deliver
 * every observed signal (ADR-023) and the replies of the calls the restore cancelled have already
 * been handed to the host's handler.
 */

/**
 * The version of this protocol. 2 (ADR-031): the worker sends the envelopes one
 * task produced as one `envelopes` message; 1 sent one `envelope` message each.
 * A host accepts both shapes; a worker batches only for a host that announced 2.
 */
export const WORKER_PROTOCOL_VERSION = 2;

/** What a worker can do beyond the base protocol, announced in its `ready` message (see the header). */
export const WORKER_FEATURES = Object.freeze(["snapshot"] as const);

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
    }
  | { readonly t: "envelope"; readonly data: ArrayBuffer }
  | { readonly t: "stats"; readonly id: number }
  /** Asks for `undra_snapshot`; answered by a `snapshot` message with the same `id`. */
  | { readonly t: "snapshot"; readonly id: number }
  /** Asks for `undra_restore(bytes)`; `bytes` is a private copy, transferred. Answered by `restored` with the same `id`. */
  | { readonly t: "restore"; readonly id: number; readonly bytes: ArrayBuffer }
  | { readonly t: "close" };

/** A failure, in a form that survives structured cloning. */
export type WorkerFailure =
  | { readonly kind: "schemaMismatch"; readonly expected: bigint; readonly got: bigint }
  | { readonly kind: "transport"; readonly reason: TransportFailure; readonly message: string }
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
  /** The answer to `snapshot`: the snapshot (transferred), or why there is none. */
  | { readonly t: "snapshot"; readonly id: number; readonly bytes?: ArrayBuffer; readonly failure?: WorkerFailure }
  /**
   * The answer to `restore`. `code` is what `undra_restore` returned (0: restored; anything else:
   * the core refused the bytes and is unchanged). `failure` is present only when `undra_restore`
   * could not be run at all (the core is closed, the module exports none, it trapped).
   */
  | { readonly t: "restored"; readonly id: number; readonly code: number; readonly failure?: WorkerFailure }
  | { readonly t: "closed"; readonly failure: WorkerFailure };
