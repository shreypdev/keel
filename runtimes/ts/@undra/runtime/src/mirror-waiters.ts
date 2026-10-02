// PROTOTYPE (ADR-057 lever d5): the `observe` barrier of a core that answers later, out of the first chunk.
import { UndraError } from "./errors.js";
import type { Mirror, MirrorWaiters } from "./mirror.js";
import { ALL_SIGNALS, type Handle } from "./wire/index.js";

interface Waiter {
  readonly signalId: number;
  readonly resolve: () => void;
  readonly reject: (error: unknown) => void;
  timer: ReturnType<typeof setTimeout> | undefined;
}

/** The waiters of `mirror`. */
export function mirrorWaiters(mirror: Mirror): MirrorWaiters {
  const waiters = new Map<Handle, Waiter[]>();
  let satisfied: Waiter[] = [];
  const settle = (waiter: Waiter, failure: { readonly error: unknown } | undefined): void => {
    if (waiter.timer !== undefined) clearTimeout(waiter.timer);
    waiter.timer = undefined;
    if (failure === undefined) waiter.resolve();
    else waiter.reject(failure.error);
  };
  return {
    get waiting() {
      return waiters.size > 0;
    },
    applied(handle, signalId) {
      const list = waiters.get(handle);
      if (list === undefined) return;
      const rest: Waiter[] = [];
      for (const w of list) {
        if (w.signalId === ALL_SIGNALS || w.signalId === signalId) satisfied.push(w);
        else rest.push(w);
      }
      if (rest.length === 0) waiters.delete(handle);
      else waiters.set(handle, rest);
    },
    drained() {
      const done = satisfied;
      satisfied = [];
      for (const waiter of done) settle(waiter, undefined);
    },
    gone(handle) {
      const list = waiters.get(handle);
      if (list === undefined) return;
      waiters.delete(handle);
      for (const w of list) settle(w, undefined);
    },
    when(handle, signalId, timeoutMs) {
      return new Promise<void>((resolve, reject) => {
        const waiter: Waiter = { signalId, resolve, reject, timer: undefined };
        if (timeoutMs > 0) {
          waiter.timer = setTimeout(() => {
            const list = waiters.get(handle);
            if (list !== undefined) {
              const rest = list.filter((w) => w !== waiter);
              if (rest.length === 0) waiters.delete(handle);
              else waiters.set(handle, rest);
            }
            reject(
              new UndraError(
                "observe",
                `no change-set arrived for signal ${signalId === ALL_SIGNALS ? "*" : String(signalId)} of handle ${String(handle)} within ${timeoutMs} ms; is the handle a live store?`,
              ),
            );
          }, timeoutMs);
        }
        const list = waiters.get(handle);
        if (list === undefined) waiters.set(handle, [waiter]);
        else list.push(waiter);
        mirror.queueFlush();
      });
    },
    fail(error) {
      const all = [...waiters.values()].flat();
      waiters.clear();
      for (const w of all) settle(w, { error });
    },
  };
}
