import type { ClockAdapter, LogAdapter, RngAdapter, TimerAdapter } from "./types.js";
import { msg } from "../messages.js";

/*
 * The adapters every JavaScript host has: clock, random numbers, log and
 * timers. In a wasm core they back the `now_ms`, `random`, `log` and
 * `timer_set` imports (SPEC 7); they are also what tests replace to make time
 * and randomness deterministic.
 */

/** What `getRandomValues` accepts (typed arrays over a non-shared buffer). */
type RandomTarget = Parameters<Crypto["getRandomValues"]>[0];

/** `crypto.getRandomValues` accepts at most this many bytes per call. */
const RANDOM_CHUNK = 65_536;

/** The wall clock (`Date.now`) and a monotonic clock (`performance.now`, else derived from `Date.now`). */
export function systemClock(): ClockAdapter {
  const perf = typeof performance === "object" && typeof performance.now === "function" ? performance : null;
  const origin = Date.now();
  return {
    nowMs: () => Date.now(),
    monotonicNs: () => BigInt(Math.round((perf ? perf.now() : Date.now() - origin) * 1e6)),
  };
}

/** The text a missing WebCrypto fails with: `UndraCore.load` rejects with it, and the `random` import reports it (ADR-049). */
export const WEB_CRYPTO_REQUIRED = msg(24);

/** Whether `crypto` (default the global one) can produce cryptographically secure random bytes: `crypto.getRandomValues` exists. */
export function hasCryptoRandom(crypto: unknown = (globalThis as { crypto?: unknown }).crypto): boolean {
  return typeof crypto === "object" && crypto !== null && typeof (crypto as { getRandomValues?: unknown }).getRandomValues === "function";
}

/**
 * Random bytes from WebCrypto (`crypto.getRandomValues`). Throws when the platform has no WebCrypto, and `fill`
 * throws when `getRandomValues` fails: randomness never degrades silently (ADR-049). Behind the wasm `random`
 * import, a throw leaves the core's buffer untouched, so the core answers its `Rng` as unavailable (a loud E0062
 * failure) instead of using predictable bytes.
 */
export function cryptoRng(crypto: Pick<Crypto, "getRandomValues"> | undefined = globalThis.crypto): RngAdapter {
  if (crypto === undefined || !hasCryptoRandom(crypto)) throw new TypeError(WEB_CRYPTO_REQUIRED);
  return {
    fill(out) {
      for (let at = 0; at < out.length; at += RANDOM_CHUNK) {
        crypto.getRandomValues(out.subarray(at, Math.min(at + RANDOM_CHUNK, out.length)) as RandomTarget);
      }
    },
  };
}

/** The part of `console` that {@link consoleLog} uses. */
export interface ConsoleLike {
  debug(...data: unknown[]): void;
  info(...data: unknown[]): void;
  warn(...data: unknown[]): void;
  error(...data: unknown[]): void;
}

/** Options of {@link consoleLog}. */
export interface ConsoleLogOptions {
  /** Records below this level are dropped (0 trace .. 5 fatal). Default 0. */
  readonly minLevel?: number;
  /** Where to write. Default the global `console`. */
  readonly console?: ConsoleLike;
}

/** Writes core log records to the console: trace and debug to `debug`, info to `info`, warn to `warn`, error and fatal to `error`, as `[target] message`. */
export function consoleLog(options: ConsoleLogOptions = {}): LogAdapter {
  const minLevel = options.minLevel ?? 0;
  return {
    log(level, target, message) {
      if (level < minLevel) return;
      const out = options.console ?? console;
      const line = `[${target}] ${message}`;
      if (level <= 1) out.debug(line);
      else if (level === 2) out.info(line);
      else if (level === 3) out.warn(line);
      else out.error(line);
    },
  };
}

/** The largest delay `setTimeout` accepts (a signed 32-bit millisecond count); longer delays are chained. */
const MAX_TIMEOUT_MS = 2_147_483_647;

/** Timers over `setTimeout`. Delays beyond 24.8 days are split into several timeouts instead of firing immediately. */
export function setTimeoutTimer(): TimerAdapter {
  return {
    set(timerId, delayMs, fire) {
      const arm = (remaining: number): void => {
        if (remaining > MAX_TIMEOUT_MS) {
          setTimeout(() => {
            arm(remaining - MAX_TIMEOUT_MS);
          }, MAX_TIMEOUT_MS);
        } else {
          setTimeout(() => {
            fire(timerId);
          }, remaining);
        }
      };
      arm(Number.isFinite(delayMs) ? Math.max(0, Math.ceil(delayMs)) : MAX_TIMEOUT_MS);
    },
  };
}
