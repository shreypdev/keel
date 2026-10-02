// PROTOTYPE (ADR-057 lever d7): `UndraCore.stats`, `snapshot` and `restore`, loaded on their first call.
import type { UndraStats } from "./core.js";
import { UndraModeError } from "./errors.js";
import type { Mirror } from "./mirror.js";
import type { Transport } from "./transport/transport.js";
import { WasmHost } from "./transport/wasm-main.js";
import { restoreInto, takeSnapshot } from "./transport/wasm-snapshot.js";

type CoreStatsJson = Readonly<Record<string, unknown>>;

export async function stats(core: { readonly closed: boolean; readonly mirror: Mirror }, transport: Transport, pending: ReadonlyMap<number, { readonly kind: string }>, handles: ReadonlySet<bigint>): Promise<UndraStats> {
  let own: CoreStatsJson | null = null;
  const json = core.closed ? null : await transport.stats?.().catch(() => null);
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
  for (const p of pending.values()) {
    if (p.kind === "call") calls++;
    else streams++;
  }
  const coreHandles = own?.live_handles;
  const count = (value: unknown): number => (typeof value === "number" ? value : 0);
  const background = (own?.background ?? {}) as CoreStatsJson;
  const hostRefs = own?.host_refs;
  return {
    liveHandles: typeof coreHandles === "number" ? coreHandles : handles.size,
    hostRefs: typeof hostRefs === "number" ? hostRefs : handles.size,
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

export async function snapshot(transport: Transport): Promise<Uint8Array> {
  if (transport instanceof WasmHost) return takeSnapshot(transport);
  if (transport.snapshot === undefined) throw new UndraModeError("snapshot", transport.mode);
  return transport.snapshot();
}

export async function restore(transport: Transport, bytes: Uint8Array): Promise<void> {
  if (transport instanceof WasmHost) return restoreInto(transport, bytes);
  if (transport.restore === undefined) throw new UndraModeError("restore", transport.mode);
  await transport.restore(bytes);
}

export { runInBackground } from "./background.js";
