/** How long anything asynchronous may take before a scenario gives up (scenarios.md, "Waiting"). */
export const WAIT_TIMEOUT_MS = 5_000;

/** How often {@link waitFor} looks again. */
export const POLL_INTERVAL_MS = 10;

/** Resolves after `ms` milliseconds. Only for the quiet windows scenarios name ("for 200 ms nothing happens"); everything else uses {@link waitFor}. */
export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

/** Options of {@link waitFor}. */
export interface WaitOptions {
  /** Give up after this many milliseconds. Default {@link WAIT_TIMEOUT_MS}. */
  readonly timeoutMs?: number;
  /** Look again this often. Default {@link POLL_INTERVAL_MS}. */
  readonly intervalMs?: number;
}

/**
 * Polls `probe` until it returns something other than `undefined`, `null` or `false`, and
 * returns that. Fails with a message naming `what` after the timeout; an exception thrown by
 * `probe` counts as "not yet" and is reported if it is the last thing seen.
 *
 * ```ts
 * const item = await waitFor("the first item", () => store.items.peek()[0]);
 * ```
 */
export async function waitFor<T>(
  what: string,
  probe: () => T | undefined | null | false | Promise<T | undefined | null | false>,
  options: WaitOptions = {},
): Promise<T> {
  const timeoutMs = options.timeoutMs ?? WAIT_TIMEOUT_MS;
  const intervalMs = options.intervalMs ?? POLL_INTERVAL_MS;
  const deadline = Date.now() + timeoutMs;
  let lastError: unknown;
  for (;;) {
    try {
      const value = await probe();
      if (value !== undefined && value !== null && value !== false) return value;
      lastError = undefined;
    } catch (error) {
      lastError = error;
    }
    if (Date.now() >= deadline) {
      const cause = lastError === undefined ? "" : ` (last error: ${lastError instanceof Error ? lastError.message : String(lastError)})`;
      throw new Error(`timed out after ${timeoutMs} ms waiting for ${what}${cause}`);
    }
    await sleep(intervalMs);
  }
}

/**
 * Runs one numbered step of a scenario. A failure is re-thrown with the step's label in front of
 * its message, so a report says which of the scenario's numbered points broke.
 *
 * ```ts
 * await step("2. area of a rect", async () => { ... });
 * ```
 */
export async function step<T>(label: string, run: () => T | Promise<T>): Promise<T> {
  try {
    return await run();
  } catch (error) {
    if (error instanceof Error) error.message = `[step ${label}] ${error.message}`;
    throw error;
  }
}
