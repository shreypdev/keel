import type { TransportFailure } from "../errors.js";
import type { HelloPayload } from "../wire/index.js";

/*
 * The messages between the main thread and the worker of the `wasm-worker`
 * mode. The data path is the Undra envelope (SPEC 3.2): every core-bound and
 * host-bound message is a framed envelope whose `ArrayBuffer` is transferred,
 * not copied. Around it sit four control messages for what an envelope cannot
 * carry: loading the module, the startup verdict, statistics and shutdown.
 *
 * Both ends ship in one package, so the protocol is internal; its version still
 * travels in `init` so a worker never sends a shape the host cannot read.
 */

/**
 * The version of this protocol. 2 (ADR-031): the worker sends the envelopes one
 * task produced as one `envelopes` message; 1 sent one `envelope` message each.
 * A host accepts both shapes; a worker batches only for a host that announced 2.
 */
export const WORKER_PROTOCOL_VERSION = 2;

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
  | { readonly t: "close" };

/** A failure, in a form that survives structured cloning. */
export type WorkerFailure =
  | { readonly kind: "schemaMismatch"; readonly expected: bigint; readonly got: bigint }
  | { readonly kind: "transport"; readonly reason: TransportFailure; readonly message: string }
  | { readonly kind: "error"; readonly message: string };

/** Worker to main thread. */
export type WorkerToHost =
  | { readonly t: "ready"; readonly hello: HelloPayload }
  | { readonly t: "failed"; readonly failure: WorkerFailure }
  | { readonly t: "envelope"; readonly data: ArrayBuffer }
  /** Protocol 2: every envelope the worker produced during one task, in order, all transferred. */
  | { readonly t: "envelopes"; readonly data: readonly ArrayBuffer[] }
  | { readonly t: "stats"; readonly id: number; readonly json: string | null }
  | { readonly t: "closed"; readonly failure: WorkerFailure };
