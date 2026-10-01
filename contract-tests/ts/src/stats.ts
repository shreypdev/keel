import type { UndraCore } from "@undra/runtime";

/** The counters of `undra_stats_json` a scenario counts with (scenarios.md, "Statistics"). */
export interface CoreCounters {
  readonly liveHandles: number;
  readonly activeCalls: number;
  readonly openStreams: number;
  /** Change-sets delivered (one per committed transaction that somebody observes). */
  readonly transactions: number;
  readonly panics: number;
  readonly calls: number;
  readonly replies: number;
  readonly changeSets: number;
  readonly streamItems: number;
  readonly cancelled: number;
  readonly badRequests: number;
  /** `schema_hash` exactly as the core prints it (`0x` and sixteen hex digits). */
  readonly schemaHash: string;
}

function num(object: Readonly<Record<string, unknown>>, name: string): number {
  const value = object[name];
  if (typeof value !== "number") throw new Error(`undra_stats_json has no number \`${name}\`: ${JSON.stringify(object)}`);
  return value;
}

/** Reads the core's statistics (`core.stats()` parses `undra_stats_json`). Scenarios that count compare two readings: they never assume the counters start at zero. */
export async function counters(core: UndraCore): Promise<CoreCounters> {
  const stats = await core.stats();
  const c = stats.core;
  if (c === null) throw new Error("this core does not report statistics");
  const crossings = c["crossings"];
  if (typeof crossings !== "object" || crossings === null) throw new Error("undra_stats_json has no `crossings`");
  const x = crossings as Readonly<Record<string, unknown>>;
  const schemaHash = c["schema_hash"];
  if (typeof schemaHash !== "string") throw new Error("undra_stats_json has no `schema_hash`");
  return {
    liveHandles: num(c, "live_handles"),
    activeCalls: num(c, "active_calls"),
    openStreams: num(c, "open_streams"),
    transactions: num(c, "transactions"),
    panics: num(c, "panics"),
    calls: num(x, "calls"),
    replies: num(x, "replies"),
    changeSets: num(x, "change_sets"),
    streamItems: num(x, "stream_items"),
    cancelled: num(x, "cancelled"),
    badRequests: num(x, "bad_requests"),
    schemaHash,
  };
}
