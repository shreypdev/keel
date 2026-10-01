import type { WorkerLike } from "../../src/transport/wasm-worker.js";
import { runWorker, type WorkerScope } from "../../src/worker.js";

/**
 * A worker served in this thread: the two ends of a real `MessageChannel`, the host end shaped as the
 * `WorkerLike` that `WasmWorkerTransport` takes and the other end running `runWorker`. Messages cross the
 * channel with the structured clone and the transfer list a real worker would see. `close` stops the worker
 * and closes both ends.
 */
export function channelWorker(): { readonly host: WorkerLike; close(): void } {
  const channel = new MessageChannel();
  channel.port1.start();
  channel.port2.start();
  const host: WorkerLike = {
    addEventListener: (type, fn) => {
      channel.port2.addEventListener(type as "message", fn as (event: MessageEvent) => void);
    },
    removeEventListener: (type, fn) => {
      channel.port2.removeEventListener(type as "message", fn as (event: MessageEvent) => void);
    },
    postMessage: (message, transfer) => {
      channel.port2.postMessage(message, transfer ?? []);
    },
    close: () => {
      channel.port2.close();
    },
  };
  const stop = runWorker(channel.port1 as unknown as WorkerScope);
  return {
    host,
    close() {
      stop();
      channel.port1.close();
      channel.port2.close();
    },
  };
}
