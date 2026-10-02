import { afterEach } from "vitest";
import type { LogAdapter } from "../../src/adapters/types.js";
import { UndraCore } from "../../src/core.js";
import { UndraWriter } from "../../src/wire/index.js";

/** Cores created through `track` are closed after each test so `UndraCore.shared` never leaks between tests. */
const cores: UndraCore[] = [];

/** Registers `core` for closing after the current test and returns it. */
export function track<C extends UndraCore>(core: C): C {
  cores.push(core);
  return core;
}

afterEach(() => {
  for (const core of cores.splice(0)) core.close();
});

/** A log adapter that keeps what it is given. */
export interface CapturedLog extends LogAdapter {
  readonly records: Array<{ level: number; target: string; message: string }>;
}

/** Captures log records for assertions. */
export function captureLog(): CapturedLog {
  const records: CapturedLog["records"] = [];
  return {
    records,
    log(level, target, message) {
      records.push({ level, target, message });
    },
  };
}

/** Waits for pending microtasks and one macrotask. */
export function macrotask(): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, 0);
  });
}

/**
 * Waits until `probe` holds. The deadline only detects a hang: it is not a bound on how fast anything runs, and it is
 * kept inside the 5 s a test is given so that this message, not the test's timeout, says what was waited for.
 */
export async function waitFor(what: string, probe: () => boolean | Promise<boolean>, ms = 4_000): Promise<void> {
  const deadline = Date.now() + ms;
  while (!(await probe())) {
    if (Date.now() > deadline) throw new Error(`timed out after ${ms} ms waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 1));
  }
}

/** Waits for the microtask queue to drain a few times. */
export async function microtasks(rounds = 4): Promise<void> {
  for (let i = 0; i < rounds; i++) await Promise.resolve();
}

/** Encodes `values` as u32 followed by a string, a convenient argument payload. */
export function u32Bytes(value: number): Uint8Array {
  const w = new UndraWriter(4);
  w.writeU32(value);
  return w.finish();
}

/** The bytes of a UTF-8 string. */
export function utf8(text: string): Uint8Array {
  return new TextEncoder().encode(text);
}

/** `[1, 2, 3]` style byte arrays with a name that reads well in tests. */
export function bytesOf(...values: number[]): Uint8Array {
  return Uint8Array.from(values);
}

/** A promise with its resolvers, for tests that need to release something at a chosen moment. */
export function deferred<T = void>(): { promise: Promise<T>; resolve(value: T): void; reject(error: unknown): void } {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}
