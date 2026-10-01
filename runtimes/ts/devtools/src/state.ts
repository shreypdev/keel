/** What the page knows: the core's stores, the timeline, the ports, the query cache, the counters. No DOM in here. */

import { type AppliedChange, Mirror } from "./mirror.js";
import type { Cause, PortRecord, ServerMsg, StepInfo, Traveled, Welcome } from "./proto.js";
import { parseSchema, type SchemaIndex } from "./schema.js";
import { decodeValue, type Value } from "./value.js";

export type ConnState = "connecting" | "open" | "reconnecting" | "closed";

export interface CommitEntry {
  readonly kind: "commit";
  readonly id: number;
  readonly atMs: number;
  readonly cause: Cause;
  readonly label: string;
  readonly txn: bigint;
  readonly changes: readonly AppliedChange[];
  /** The step that recorded the state after this commit; `undefined` until the next one is taken. */
  step: number | undefined;
  /** Which core process it came from: steps of an earlier one cannot be restored. */
  readonly epoch: number;
}

export interface DividerEntry {
  readonly kind: "divider";
  readonly id: number;
  readonly text: string;
}

export type TimelineEntry = CommitEntry | DividerEntry;

export interface PortEntry {
  readonly id: number;
  readonly name: string;
  readonly portId: number;
  readonly methodId: number;
  readonly startMs: number;
  readonly args: Uint8Array;
  endMs: number | undefined;
  /** 0 ok, 1 typed error, 2 unavailable; `undefined` while the call is open. */
  status: number | undefined;
  latencyUs: number | undefined;
  reply: Uint8Array | undefined;
}

export interface QueryRow {
  readonly queryId: number;
  readonly name: string;
  readonly key: string;
  readonly status: string;
  readonly fetching: boolean;
  readonly observers: number;
  readonly invalidated: boolean;
  readonly failed: boolean;
  readonly layers: number;
  readonly updatedAt: number | null;
  readonly dataLen: number;
  readonly data: Value | undefined;
  readonly error: Value | undefined;
}

export interface QueryLogEntry {
  readonly atMs: number;
  readonly event: "new" | "fetching" | "success" | "error" | "invalidated" | "removed";
  readonly name: string;
  readonly key: string;
}

export interface QueriesView {
  readonly online: boolean;
  readonly pendingMutations: number;
  readonly rows: readonly QueryRow[];
}

/** The counters of one sample, as the server sent them. */
export interface StatsSample {
  readonly atMs: number;
  readonly core: Readonly<Record<string, unknown>>;
  readonly server: Readonly<Record<string, number | boolean>>;
}

export interface TravelNote {
  readonly ok: boolean;
  readonly step: number;
  readonly message: string;
}

const MAX_TIMELINE = 1000;
const MAX_PORTS = 300;
const MAX_QUERY_LOG = 300;
const MAX_STATS = 30;

const num = (v: unknown): number => (typeof v === "number" ? v : 0);

export class DevtoolsState {
  conn: ConnState = "connecting";
  welcome: Welcome | undefined;
  schema: SchemaIndex | undefined;
  mirror: Mirror | undefined;
  app: { readonly connected: boolean; readonly platform: string } = { connected: false, platform: "" };
  readonly timeline: TimelineEntry[] = [];
  steps: StepInfo[] = [];
  /** The oldest step the server still holds. */
  evictedBelow = 1;
  readonly ports: PortEntry[] = [];
  queries: QueriesView = { online: true, pendingMutations: 0, rows: [] };
  readonly queryLog: QueryLogEntry[] = [];
  readonly stats: StatsSample[] = [];
  travel: TravelNote | undefined;
  /** Signals that changed since the view last painted, as `handle:signal`. */
  readonly dirty = new Set<string>();
  /** Problems the page ran into (a change-set it could not decode); shown, never fatal. */
  readonly notices: string[] = [];

  #epoch = 0;
  #coreEpoch: bigint | undefined;
  #nextId = 1;
  #requestId = 0;
  #pendingTravel: number | undefined;
  readonly #open = new Map<number, PortEntry>();
  #lastQueries = new Map<string, QueryRow>();
  readonly #listeners = new Set<(what: Set<string>) => void>();
  readonly #changed = new Set<string>();

  /** Called with what changed (`stores`, `timeline`, `steps`, `ports`, `queries`, `stats`, `conn`, `app`, `travel`) after each message. */
  onChange(listener: (what: Set<string>) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  #touch(...what: string[]): void {
    for (const w of what) this.#changed.add(w);
  }

  #flush(): void {
    if (this.#changed.size === 0) return;
    const what = new Set(this.#changed);
    this.#changed.clear();
    for (const l of this.#listeners) l(what);
  }

  setConn(state: ConnState): void {
    this.conn = state;
    this.#touch("conn");
    this.#flush();
  }

  /** Starts a restore request: returns its id, to send with the step. */
  nextTravel(): number {
    this.#requestId += 1;
    this.#pendingTravel = this.#requestId;
    return this.#requestId;
  }

  /** Says something to the person at the page (a problem the page ran into); never fatal. */
  notify(text: string): void {
    this.#notice(text);
    this.#flush();
  }

  /** The step the core is at now: the newest one the server took. */
  get currentStep(): number {
    return this.steps.at(-1)?.step ?? 0;
  }

  /** Steps that can be travelled to. */
  get restorableSteps(): StepInfo[] {
    return this.steps.filter((s) => s.restorable && s.step >= this.evictedBelow);
  }

  get epoch(): number {
    return this.#epoch;
  }

  onMessage(msg: ServerMsg): void {
    switch (msg.t) {
      case "welcome":
        this.#onWelcome(msg.welcome);
        break;
      case "stores":
        this.#onStores(msg.stores);
        break;
      case "changeSet":
        this.#onChangeSet(msg);
        break;
      case "step":
        this.#onStep(msg.step);
        break;
      case "evicted":
        this.evictedBelow = Math.max(this.evictedBelow, msg.belowStep);
        this.steps = this.steps.filter((s) => s.step >= this.evictedBelow);
        this.#touch("steps");
        break;
      case "port":
        this.#onPort(msg.record);
        break;
      case "stats":
        this.#onStats(msg.json);
        break;
      case "queries":
        this.#onQueries(msg.atMs, msg.json);
        break;
      case "traveled":
        this.#onTraveled(msg.result);
        break;
      case "app":
        this.app = { connected: msg.connected, platform: msg.platform };
        this.#touch("app");
        break;
    }
    this.#flush();
  }

  #divider(text: string): void {
    this.timeline.unshift({ kind: "divider", id: this.#nextId++, text });
    this.#touch("timeline");
  }

  #onWelcome(w: Welcome): void {
    const reloaded = this.#coreEpoch !== undefined && this.#coreEpoch !== w.coreEpoch;
    const returned = this.#coreEpoch !== undefined && !reloaded;
    if (reloaded) {
      this.#epoch += 1;
      this.#divider("core reloaded: the history starts again, the steps above cannot be travelled to");
    } else if (returned && this.timeline.length > 0) {
      this.#divider("reconnected");
    }
    this.#coreEpoch = w.coreEpoch;
    this.welcome = w;
    this.schema = parseSchema(w.schemaJson);
    this.mirror = new Mirror(this.schema);
    // The server sends its ring and the stores again: nothing from before is kept but the timeline.
    this.steps = [];
    this.evictedBelow = 1;
    this.#open.clear();
    this.#lastQueries = new Map();
    this.#touch("stores", "steps", "ports", "queries", "stats", "conn");
  }

  #onStores(stores: readonly { handle: bigint; typeId: number }[]): void {
    if (this.mirror === undefined) return;
    this.mirror.setStores(stores);
    this.#touch("stores");
  }

  #labelOf(cause: Cause): string {
    switch (cause.kind) {
      case "call":
        return this.schema?.methodName(cause.methodId) ?? `method ${cause.methodId}`;
      case "restore":
        return `restore to step ${cause.step}`;
      case "other":
        return "task, timer or stream";
    }
  }

  #onChangeSet(msg: Extract<ServerMsg, { t: "changeSet" }>): void {
    if (this.mirror === undefined) return;
    let applied: ReturnType<Mirror["apply"]>;
    try {
      applied = this.mirror.apply(msg.payload);
    } catch (e) {
      this.#notice(`a change-set could not be decoded: ${e instanceof Error ? e.message : String(e)}`);
      return;
    }
    for (const c of applied.changes) {
      if (c.error !== undefined) this.#notice(`${c.store}.${c.signal}: ${c.error}`);
      if (msg.delivery === "commit") this.dirty.add(`${c.handle}:${c.signalId}`);
    }
    this.#touch("stores");
    if (msg.delivery !== "commit") return;
    this.timeline.unshift({
      kind: "commit",
      id: msg.seq,
      atMs: msg.atMs,
      cause: msg.cause,
      label: this.#labelOf(msg.cause),
      txn: applied.txn,
      changes: applied.changes,
      step: undefined,
      epoch: this.#epoch,
    });
    if (this.timeline.length > MAX_TIMELINE) this.timeline.length = MAX_TIMELINE;
    this.#touch("timeline");
  }

  #notice(text: string): void {
    this.notices.unshift(text);
    if (this.notices.length > 20) this.notices.length = 20;
    this.#touch("notices");
  }

  #onStep(step: StepInfo): void {
    const known = this.steps.findIndex((s) => s.step === step.step);
    if (known >= 0) this.steps[known] = step;
    else this.steps.push(step);
    this.steps.sort((a, b) => a.step - b.step);
    // The change-sets since the previous step are the ones this step recorded.
    for (const e of this.timeline) {
      if (e.kind === "commit" && e.epoch === this.#epoch && e.step === undefined && e.id <= step.throughSeq) e.step = step.step;
    }
    this.#touch("steps", "timeline");
  }

  #onPort(r: PortRecord): void {
    const name = this.schema?.portMethodName(r.portId, r.methodId) ?? `${r.portId}.${r.methodId}`;
    if (r.phase === "start") {
      const entry: PortEntry = {
        id: r.id,
        name,
        portId: r.portId,
        methodId: r.methodId,
        startMs: r.atMs,
        args: r.args,
        endMs: undefined,
        status: undefined,
        latencyUs: undefined,
        reply: undefined,
      };
      this.#open.set(r.id, entry);
      this.ports.unshift(entry);
      if (this.ports.length > MAX_PORTS) this.ports.length = MAX_PORTS;
    } else {
      let entry = this.#open.get(r.id);
      if (entry === undefined) {
        // The start was before this page attached.
        entry = { id: r.id, name, portId: r.portId, methodId: r.methodId, startMs: r.atMs, args: new Uint8Array(0), endMs: undefined, status: undefined, latencyUs: undefined, reply: undefined };
        this.ports.unshift(entry);
      }
      entry.endMs = r.atMs;
      entry.status = r.status;
      entry.latencyUs = r.latencyUs;
      entry.reply = r.reply;
      this.#open.delete(r.id);
    }
    this.#touch("ports");
  }

  #onStats(json: string): void {
    try {
      const raw = JSON.parse(json) as { at_ms: number; core: Record<string, unknown>; server: Record<string, number | boolean> };
      this.stats.push({ atMs: num(raw.at_ms), core: raw.core, server: raw.server });
      if (this.stats.length > MAX_STATS) this.stats.shift();
      this.#touch("stats");
    } catch {
      this.#notice("the counters did not parse");
    }
  }

  #onQueries(atMs: number, json: string): void {
    let doc: { online?: boolean; pending_mutations?: number; entries?: Record<string, unknown>[] };
    try {
      doc = JSON.parse(json) as typeof doc;
    } catch {
      return;
    }
    const rows: QueryRow[] = [];
    for (const e of doc.entries ?? []) {
      const queryId = num(e["query_id"]);
      const def = this.schema?.queries.get(queryId);
      const decode = (hex: unknown, which: "data" | "error"): Value | undefined => {
        if (typeof hex !== "string" || this.schema === undefined || def === undefined) return undefined;
        try {
          const bytes = Uint8Array.from(hex.match(/../g) ?? [], (h) => Number.parseInt(h, 16));
          // `returns` is `Result<T, E>` for a query with an error type: data is the T, error the E.
          const ty = def.returns;
          const part = ty.kind === "result" ? (which === "data" ? ty.of[0] : ty.of[1]) : which === "data" ? ty : undefined;
          return part === undefined ? bytes : decodeValue(this.schema, part, bytes);
        } catch {
          return undefined;
        }
      };
      rows.push({
        queryId,
        name: def?.name ?? `query ${queryId}`,
        key: String(e["key"] ?? ""),
        status: String(e["status"] ?? "idle"),
        fetching: e["fetching"] === true,
        observers: num(e["observers"]),
        invalidated: e["invalidated"] === true,
        failed: e["failed"] === true,
        layers: num(e["layers"]),
        updatedAt: typeof e["updated_at"] === "number" ? e["updated_at"] : null,
        dataLen: num(e["data_len"]),
        data: decode(e["data"], "data"),
        error: decode(e["error"], "error"),
      });
    }
    this.queries = { online: doc.online !== false, pendingMutations: num(doc.pending_mutations), rows };
    this.#logQueryChanges(atMs, rows);
    this.#touch("queries");
  }

  #logQueryChanges(atMs: number, rows: readonly QueryRow[]): void {
    const next = new Map<string, QueryRow>();
    const log = (event: QueryLogEntry["event"], r: QueryRow): void => {
      this.queryLog.unshift({ atMs, event, name: r.name, key: r.key });
    };
    for (const r of rows) {
      const id = `${r.queryId}/${r.key}`;
      next.set(id, r);
      const was = this.#lastQueries.get(id);
      if (was === undefined) log("new", r);
      if (r.fetching && was?.fetching !== true) log("fetching", r);
      if (was !== undefined) {
        if (r.invalidated && !was.invalidated) log("invalidated", r);
        if (r.status === "error" && was.status !== "error") log("error", r);
        else if (!r.fetching && was.fetching && r.status === "success") log("success", r);
      }
    }
    for (const [id, r] of this.#lastQueries) if (!next.has(id)) log("removed", r);
    this.#lastQueries = next;
    if (this.queryLog.length > MAX_QUERY_LOG) this.queryLog.length = MAX_QUERY_LOG;
  }

  #onTraveled(t: Traveled): void {
    if (this.#pendingTravel === t.requestId) this.#pendingTravel = undefined;
    this.travel = { ok: t.ok, step: t.step, message: t.message };
    this.#touch("travel");
  }

  /** Whether a restore this page asked for is still being carried out. */
  get traveling(): boolean {
    return this.#pendingTravel !== undefined;
  }
}
