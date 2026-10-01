import type { TransportFailure } from "../errors.js";
import type { HelloPayload } from "../wire/index.js";

/*
 * The messages between the main thread and the worker of the `wasm-worker`
 * mode. The data path is the Undra envelope (SPEC 3.2): every core-bound and
 * host-bound message is a framed envelope whose `ArrayBuffer` is transferred,
 * not copied. Around it sit four control messages for what an envelope cannot
 * carry: loading the module, the startup verdict, statistics and shutdown.
 */

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
  | { readonly t: "stats"; readonly id: number; readonly json: string | null }
  | { readonly t: "closed"; readonly failure: WorkerFailure };
