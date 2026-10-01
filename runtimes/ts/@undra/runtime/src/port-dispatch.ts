import { standardMethodName } from "./adapters/ids.js";
import { UndraPortError } from "./errors.js";
import type { PortImpl } from "./port.js";
import type { PortOutcome } from "./transport/transport.js";
import { type PortCallPayload, PortStatus, encodePortReply } from "./wire/index.js";

/*
 * Running a host port implementation for one `PortCall` (SPEC 6.3), the same way on the main thread
 * (`UndraCore`) and inside the worker of the `wasm-worker` mode (the ports of `LoadOptions.worker.ports`).
 */

const NO_BYTES = new Uint8Array(0);
const UNAVAILABLE: PortOutcome = { kind: "unavailable" };
const ASYNC: PortOutcome = { kind: "async" };

/** What {@link dispatchPortCall} needs from the side that runs it. */
export interface PortDispatchHooks {
  /** Sends the `PortReply` of a method that answered later (its promise settled). */
  later(reply: Uint8Array): void;
  /** A method failed with something that is not its port's typed error (a bug in the adapter): report it. The core sees "unavailable". */
  untyped(call: PortCallPayload, error: unknown): void;
}

function isThenable(value: unknown): value is PromiseLike<Uint8Array> {
  return typeof value === "object" && value !== null && typeof (value as { then?: unknown }).then === "function";
}

/**
 * Runs the method of `impl` that `call` names: a synchronous answer is returned as a complete `PortReply`, a
 * promise is answered later through `hooks.later`, a port or method that is not implemented is "unavailable".
 * An `UndraPortError` is the port's typed error (status 1); anything else is passed to `hooks.untyped` and
 * answered "unavailable" (status 2).
 */
export function dispatchPortCall(impl: PortImpl | undefined, call: PortCallPayload, hooks: PortDispatchHooks): PortOutcome {
  const method = impl?.methods[call.methodId];
  if (method === undefined) return UNAVAILABLE;
  let result: Uint8Array | PromiseLike<Uint8Array>;
  try {
    result = method(call.args);
  } catch (error) {
    return { kind: "sync", reply: portFailureReply(call, error, hooks) };
  }
  if (isThenable(result)) {
    result.then(
      (body) => {
        hooks.later(encodePortReply({ portCallId: call.portCallId, status: PortStatus.Ok, body }));
      },
      (error: unknown) => {
        hooks.later(portFailureReply(call, error, hooks));
      },
    );
    return ASYNC;
  }
  return { kind: "sync", reply: encodePortReply({ portCallId: call.portCallId, status: PortStatus.Ok, body: result }) };
}

/** The `PortReply` for a port method that threw: its typed error (status 1), or "unavailable" (status 2) after `hooks.untyped`. */
export function portFailureReply(call: PortCallPayload, error: unknown, hooks: Pick<PortDispatchHooks, "untyped">): Uint8Array {
  if (error instanceof UndraPortError) {
    return encodePortReply({ portCallId: call.portCallId, status: PortStatus.Error, body: error.body });
  }
  hooks.untyped(call, error);
  return encodePortReply({ portCallId: call.portCallId, status: PortStatus.Unavailable, body: NO_BYTES });
}

/**
 * How the runtime names the implementation of a port in what it logs: `Kv.get adapter (port 0x..., method 0x...,
 * ...)` for a standard port, `port 0x... method 0x...` for a custom one.
 */
export function portOperation(call: Pick<PortCallPayload, "portId" | "methodId">): string {
  const ids = `port 0x${call.portId.toString(16)} method 0x${call.methodId.toString(16)}`;
  const standard = standardMethodName(call.portId, call.methodId);
  return standard === undefined ? ids : `${standard} adapter (${ids}, answered as unavailable: an adapter must fail with its port's typed error)`;
}

/** The text of the error that refuses a synchronous port on a thread the core cannot wait for (`wasm-worker`, ADR-049). */
export function syncPortRefusal(portId: number): string {
  const standard = standardMethodName(portId, 0)?.split(" ")[0];
  const name = standard === undefined ? `port 0x${portId.toString(16)}` : `the ${standard} port (0x${portId.toString(16)})`;
  return `${name} is synchronous (#[undra::port(sync)]), and in wasm-worker mode the core cannot wait for the main thread to answer it: register it in the worker instead, in the module of LoadOptions.worker.ports (its default export maps port ids to implementations), or load the core with mode "wasm-main"`;
}
