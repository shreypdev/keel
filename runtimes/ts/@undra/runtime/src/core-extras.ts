import type { UndraBackgroundReport } from "./adapters/types.js";
import type { UndraCore, UndraStats } from "./core.js";
import { UndraModeError } from "./errors.js";
import type { CoreTransport } from "./transport/transport.js";
import { WasmHost } from "./transport/wasm-main.js";
import { restoreInto, takeSnapshot } from "./transport/wasm-snapshot.js";
import { CallTarget, UndraReader, codecs, encodeValue } from "./wire/index.js";

/*
 * What `UndraCore` does only when asked, loaded on the first call (ADR-057, ADR-052): `stats`, `snapshot`, `restore` and
 * `runInBackground`. All four answer with a promise already, so a first call that waits for this chunk is an ordinary
 * asynchronous call (SPEC 17.1 is unchanged). A hello page that never asks does not carry them: its background window reads the
 * one number it needs (`background.pending`) from the core's own JSON.
 */

/** Statistics counters exposed by `undra_stats_json` that this runtime reads. */
type CoreStatsJson = Readonly<Record<string, unknown>>;

/** Live counters of `core`: what `UndraCore.stats` answers (see {@link UndraStats}). */
export async function stats(core: UndraCore): Promise<UndraStats> {
  let own: CoreStatsJson | null = null;
  const json = core.closed ? null : await core._transport.stats?.().catch(() => null);
  if (typeof json === "string") {
    try {
      const parsed: unknown = JSON.parse(json);
      if (typeof parsed === "object" && parsed !== null) own = parsed as CoreStatsJson;
    } catch {
      own = null;
    }
  }
  let calls = 0;
  let streams = 0;
  for (const p of core._pending.values()) {
    if (p.kind === "call") calls++;
    else streams++;
  }
  const coreHandles = own?.live_handles;
  const count = (value: unknown): number => (typeof value === "number" ? value : 0);
  const background = (own?.background ?? {}) as CoreStatsJson;
  const hostRefs = own?.host_refs;
  return {
    liveHandles: typeof coreHandles === "number" ? coreHandles : core._handles.size,
    hostRefs: typeof hostRefs === "number" ? hostRefs : core._handles.size,
    pendingCalls: calls,
    openStreams: streams,
    mirroredStores: core.mirror.size,
    droppedEntries: core.mirror.dropped,
    mirror: core.mirror.stats(),
    core: own,
    panicReports: count(own?.panic_reports),
    background: {
      tasks: count(background.tasks),
      pending: count(background.pending),
      runs: count(background.runs),
      finished: count(background.finished),
      replayed: count(background.replayed),
      refetched: count(background.refetched),
    },
  };
}

/** The persisted state of every store: what `UndraCore.snapshot` answers (the core is open; see there). */
export async function snapshot(transport: CoreTransport): Promise<Uint8Array> {
  if (transport.snapshot !== undefined) return transport.snapshot();
  if (transport instanceof WasmHost) return takeSnapshot(transport);
  throw new UndraModeError("snapshot", transport.mode);
}

/** Rebuilds the stores from `bytes`: what `UndraCore.restore` does before it flushes the mirror (the core is open; see there). */
export async function restore(transport: CoreTransport, bytes: Uint8Array): Promise<void> {
  if (transport.restore !== undefined) return transport.restore(bytes);
  if (transport instanceof WasmHost) return restoreInto(transport, bytes);
  throw new UndraModeError("restore", transport.mode);
}

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
