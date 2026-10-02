import type { UndraBackgroundReport } from "./adapters/types.js";
import type { UndraCore } from "./core.js";
import { CallTarget, UndraReader, codecs, encodeValue } from "./wire/index.js";

/*
 * `UndraCore.runInBackground` (ADR-046 decision 3), loaded on demand (ADR-052): a page reaches it only when its core has
 * background work to drain (`stats().background.pending > 0`, which a core without a background task, a hello world, never
 * has) or when the app calls it, so it is not in the chunk every page loads. The page window fetches it when the page is
 * hidden, which comes before it is left (`pagehide`) or frozen.
 */

/** The standard function `run_background` (`fnv1a32("fn.run_background")`, ADR-046): in every schema, called like any free async function. */
export const RUN_BACKGROUND = 0x0e5b14ff;

/**
 * Calls `run_background(deadlineMs)` on `core` and decodes its `BackgroundReport`: what `UndraCore.runInBackground` does (see
 * there; it maps the failures). The deadline is clamped to a whole, non-negative, safe number of milliseconds.
 */
export async function runInBackground(core: UndraCore, deadlineMs: number, signal: AbortSignal | undefined): Promise<UndraBackgroundReport> {
  const ms = Math.min(Math.max(0, Math.trunc(deadlineMs) || 0), Number.MAX_SAFE_INTEGER);
  const reader = new UndraReader(await core.call(CallTarget.FreeFunction, RUN_BACKGROUND, encodeValue(codecs.u64, BigInt(ms)), signal));
  // `BackgroundReportCodec.decode` (adapters/codecs.ts), spelled out: the codecs are a chunk of their own.
  const report = { finished: reader.readBool(), replayed: reader.readU32(), refetched: reader.readU32(), stillPending: reader.readU32() };
  reader.finish();
  return report;
}
