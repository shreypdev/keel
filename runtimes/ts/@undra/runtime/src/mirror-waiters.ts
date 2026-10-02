import { UndraError } from "./errors.js";
import type { Mirror, MirrorWaiters } from "./mirror.js";
import { ALL_SIGNALS, type Handle } from "./wire/index.js";
import { msg } from "./messages.js";

/*
 * The "initial change-set" barrier behind `UndraCore.observe` on a core that answers later (a worker, a socket): a promise per
 * `observe` that settles when an entry of its signal was applied by a drain (`Mirror.whenObserved`). A core in this thread
 * delivers the values inside `observe` and has none of this (ADR-057): the core installs it for a transport that is not
 * synchronous (`transport/framed.ts` re-exports `mirrorWaiters`, so it arrives with the transport that needs it), and a
 * `Mirror` used on its own (or an in-process core's) installs it by calling `mirrorWaiters(mirror)`, which the package root
 * re-exports from its pure barrel (so a page that does not import it does not load it).
 */

interface Waiter {
  readonly signalId: number;
  readonly resolve: () => void;
  readonly reject: (error: unknown) => void;
  timer: ReturnType<typeof setTimeout> | undefined;
}

/** Installs the observe waiters into `mirror` and returns them: call it once per mirror. */
export function mirrorWaiters(mirror: Mirror): MirrorWaiters {
  const waiters = new Map<Handle, Waiter[]>();
  /** What the drain in progress applied an entry for: settled when its rounds are done. */
  let satisfied: Waiter[] = [];
  const settle = (waiter: Waiter, failure?: { readonly error: unknown }): void => {
    if (waiter.timer !== undefined) clearTimeout(waiter.timer);
    waiter.timer = undefined;
    if (failure === undefined) waiter.resolve();
    else waiter.reject(failure.error);
  };
  const remove = (handle: Handle, waiter: Waiter): void => {
    const rest = (waiters.get(handle) ?? []).filter((w) => w !== waiter);
    if (rest.length === 0) waiters.delete(handle);
    else waiters.set(handle, rest);
  };
  const installed: MirrorWaiters = {
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
      for (const waiter of done) settle(waiter);
    },
    gone(handle) {
      const list = waiters.get(handle);
      if (list === undefined) return;
      waiters.delete(handle);
      for (const w of list) settle(w);
    },
    when(handle, signalId, timeoutMs) {
      return new Promise<void>((resolve, reject) => {
        const waiter: Waiter = { signalId, resolve, reject, timer: undefined };
        if (timeoutMs > 0) {
          waiter.timer = setTimeout(() => {
            remove(handle, waiter);
            reject(
              new UndraError(
                "observe",
                msg(104, signalId === ALL_SIGNALS ? "*" : String(signalId), String(handle), timeoutMs),
              ),
            );
          }, timeoutMs);
        }
        const list = waiters.get(handle);
        if (list === undefined) waiters.set(handle, [waiter]);
        else list.push(waiter);
        // Entries that arrived before the promise was made are drained now, not at the next frame.
        mirror.queueFlush();
      });
    },
    fail(error) {
      const all = [...waiters.values()].flat();
      waiters.clear();
      for (const w of all) settle(w, { error });
    },
  };
  mirror._install(installed);
  return installed;
}
