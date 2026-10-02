import { UndraTransportError } from "./errors.js";
import { errorMessage } from "./platform.js";

/**
 * Runs the dynamic `import()` of a chunk of the runtime that loads on first use (ADR-057): what cannot be fetched (the network is
 * down, a deploy replaced the file, a Content-Security-Policy allows the entry script but not the chunks beside it) rejects
 * typed (constitution R6), as `UndraTransportError("closed")` with the failed import as its `cause`, which a generated call sees
 * as `UndraCallError.Unavailable`. Nothing is remembered: the next use tries the import again, as the lazy default ports do.
 *
 * @param what The chunk, for the message (`"stream support"`).
 * @param load The import, written where the bundler can see its specifier: `() => import("./stream-support.js")`.
 */
export function onDemand<T>(what: string, load: () => Promise<T>): Promise<T> {
  return load().catch((cause: unknown) => {
    throw new UndraTransportError("closed", `the runtime's ${what} could not be loaded: ${errorMessage(cause)}`, { cause });
  });
}
