import { describe, expect, it } from "vitest";
import { DEFAULT_MAX_PENDING_BYTES, DEFAULT_MAX_PENDING_ENTRIES, MAX_MERGED_PATCH_BYTES, MAX_MERGED_PATCH_OPS, Mirror } from "../src/mirror.js";
import {
  ChangeOp,
  PatchError,
  type PatchOp,
  UndraReader,
  UndraWriter,
  applyPatch,
  codecs,
  decodePatch,
  decodeLazyInvalidated,
  decodeLazyValue,
  decodeValue,
  encodeChangeSet,
  encodeLazyInvalidated,
  encodeLazyValue,
  encodePatch,
  encodeValue,
  type Codec,
  type Handle,
} from "../src/wire/index.js";

/*
 * A model-based check of frame-coalesced delivery (ADR-031, docs/SPEC.md section 11), written by the
 * review of the piece: the property tests next to the implementation deliver single-entry change-sets of
 * one store and drain once at the end. Here a model core commits transactions over two stores (one
 * change-set per store, entries ordered by signal id, as `commit_stores` does), the "wire" delivers a
 * random prefix of what it holds at random points, drains run at random points (so a transaction can
 * straddle two drains), the backlog bound is small (so compactions run mid-history), one signal is
 * `no_coalesce`, out-of-bounds patches ask for a resync, and resyncs are answered asynchronously (the
 * core sends the full value later, behind whatever it already sent, as a worker or a socket does). A
 * rare bulk patch passes both patch bounds on its own, so a compaction drops it and the mirror re-observes.
 * One signal of each store is a lazy list (ADR-043): a commit sends an invalidation (length, version), and now and then
 * a restart of its page server sends a full value with a new handle; a drain that folds the two must never lose the handle.
 *
 * Invariant: whenever the wire is empty, no resync is outstanding and a drain left nothing queued, every
 * mirrored value equals the core's. The `no_coalesce` signal sees a subsequence of the committed values
 * (every one of them while no compaction ran), ending with the last.
 */

const HANDLES: readonly Handle[] = [1n, 2n];
const U32_LISTS = [0, 2];
const STRING_LIST = 4;
const SCALARS = [1, 3];
/** A `Lazy<T>` signal: its value is (page server handle, length, version). */
const LAZY = 5;
/** Signal 3 of handle 2 is declared `no_coalesce`. */
const NO_COALESCE_HANDLE: Handle = 2n;
const NO_COALESCE_SIGNAL = 3;

const u32List = codecs.vec(codecs.u32);
const strList = codecs.vec(codecs.string);

function rng(seed: number): (n: number) => number {
  let x = seed >>> 0 || 0x9e3779b9;
  return (n) => {
    x ^= x << 13;
    x >>>= 0;
    x ^= x >>> 17;
    x ^= x << 5;
    x >>>= 0;
    return x % n;
  };
}

function patchBytes<T>(ops: readonly PatchOp<T>[], item: Codec<T>): Uint8Array {
  const w = new UndraWriter();
  encodePatch(w, ops, item);
  return w.finish();
}

interface StoreState {
  lists: Map<number, number[]>;
  strings: string[];
  scalars: Map<number, number>;
  lazy: { handle: number; len: number; version: number };
}

function emptyState(): StoreState {
  return {
    lists: new Map(U32_LISTS.map((id) => [id, [] as number[]])),
    strings: [],
    scalars: new Map(SCALARS.map((id) => [id, 0])),
    lazy: { handle: 1, len: 0, version: 0 },
  };
}

/** One signal's value, as text. */
function renderKey(s: StoreState, signalId: number): string {
  if (signalId === LAZY) return `${s.lazy.handle}/${s.lazy.len}/${s.lazy.version}`;
  if (signalId === STRING_LIST) return `#${s.strings.length}/${s.strings.map((x) => x.slice(0, 12)).join(",")}`;
  if (U32_LISTS.includes(signalId)) return `[${(s.lists.get(signalId) ?? []).join(",")}]`;
  return String(s.scalars.get(signalId) ?? 0);
}

const ALL_KEYS = [...U32_LISTS, STRING_LIST, ...SCALARS, LAZY];

function render(s: StoreState): string {
  const lists = U32_LISTS.map((id) => `${id}:[${(s.lists.get(id) ?? []).join(",")}]`).join(" ");
  const scalars = SCALARS.map((id) => `${id}=${s.scalars.get(id) ?? 0}`).join(" ");
  return `${lists} ${STRING_LIST}:${renderKey(s, STRING_LIST)} ${scalars} lazy ${renderKey(s, LAZY)}`;
}

type Entry = { handle: Handle; signalId: number; op: ChangeOp; value: Uint8Array };

class ModelCore {
  readonly truth = new Map<Handle, StoreState>(HANDLES.map((h) => [h, emptyState()]));
  readonly wire: Uint8Array[] = [];
  readonly resyncRequests: Array<[Handle, number]> = [];
  readonly progressCommitted: number[] = [];
  /** Every value each signal has had, per handle: a drained mirror never shows anything else (without corrupt patches). */
  readonly history = new Map<string, Set<string>>();
  #txn = 0n;
  #item = 1;

  constructor(
    readonly rand: (n: number) => number,
    readonly bulk: boolean,
    readonly corrupt: boolean,
  ) {
    for (const h of HANDLES) for (const id of ALL_KEYS) this.#remember(h, id);
  }

  #remember(h: Handle, id: number): void {
    const key = `${String(h)}/${id}`;
    let seen = this.history.get(key);
    if (seen === undefined) {
      seen = new Set();
      this.history.set(key, seen);
    }
    seen.add(renderKey(this.state(h), id));
  }

  state(h: Handle): StoreState {
    return this.truth.get(h) as StoreState;
  }

  full(h: Handle, signalId: number): Entry {
    const s = this.state(h);
    let value: Uint8Array;
    if (signalId === LAZY) value = encodeLazyValue({ handle: BigInt(s.lazy.handle), len: s.lazy.len, version: BigInt(s.lazy.version) });
    else if (signalId === STRING_LIST) value = encodeValue(strList, s.strings);
    else if (U32_LISTS.includes(signalId)) value = encodeValue(u32List, s.lists.get(signalId) ?? []);
    else value = encodeValue(codecs.u32, s.scalars.get(signalId) ?? 0);
    return { handle: h, signalId, op: ChangeOp.FullValue, value };
  }

  /** One transaction: one change-set per store it touched, entries in signal-id order, one per signal. */
  commit(): void {
    const rand = this.rand;
    this.#txn++;
    const stores = rand(3) === 0 ? [...HANDLES] : [HANDLES[rand(HANDLES.length)] as Handle];
    for (const h of stores) {
      const touched = new Map<number, Entry>();
      const signals = 1 + rand(3);
      for (let i = 0; i < signals; i++) {
        const pick = rand(100);
        if (pick >= 92) {
          // The lazy list changes (an invalidation), or its page server restarts (a full value with a new handle).
          const lazy = this.state(h).lazy;
          const restart = pick >= 97 || touched.get(LAZY)?.op === ChangeOp.FullValue;
          if (pick >= 97) lazy.handle++;
          lazy.len = rand(500);
          lazy.version++;
          touched.set(
            LAZY,
            restart
              ? this.full(h, LAZY)
              : { handle: h, signalId: LAZY, op: ChangeOp.LazyInvalidated, value: encodeLazyInvalidated({ len: lazy.len, version: BigInt(lazy.version) }) },
          );
        } else if (pick < 30) {
          const id = SCALARS[rand(SCALARS.length)] as number;
          const v = this.#item++;
          this.state(h).scalars.set(id, v);
          if (h === NO_COALESCE_HANDLE && id === NO_COALESCE_SIGNAL) {
            // Two writes of one signal in one transaction are one entry: only the last is committed.
            if (touched.has(id)) this.progressCommitted.pop();
            this.progressCommitted.push(v);
          }
          touched.set(id, this.full(h, id));
        } else if (pick < 38) {
          const id = U32_LISTS[rand(U32_LISTS.length)] as number;
          this.state(h).lists.set(id, Array.from({ length: rand(6) }, () => this.#item++));
          touched.set(id, this.full(h, id));
        } else if (pick < 39 && this.bulk) {
          touched.set(STRING_LIST, this.#bulk(h, touched.get(STRING_LIST)));
        } else if (this.bulk && rand(3) === 0) {
          touched.set(STRING_LIST, this.#patchStrings(h, touched.get(STRING_LIST)));
        } else {
          const id = U32_LISTS[rand(U32_LISTS.length)] as number;
          touched.set(id, this.#patch(h, id, touched.get(id)));
        }
      }
      for (const id of touched.keys()) this.#remember(h, id);
      const entries = [...touched.values()].sort((a, b) => a.signalId - b.signalId);
      this.wire.push(encodeChangeSet({ txnId: this.#txn, entries }));
    }
  }

  /** Random ops on a `u32` list; rarely an out-of-bounds op the core never made. Within one transaction a second patch of the signal becomes a full value. */
  #patch(h: Handle, id: number, earlier: Entry | undefined): Entry {
    const rand = this.rand;
    const ops: PatchOp<number>[] = [];
    const count = 1 + rand(4);
    for (let i = 0; i < count; i++) {
      const list = this.state(h).lists.get(id) as number[];
      const kind = list.length === 0 ? 0 : rand(6);
      let op: PatchOp<number>;
      switch (kind) {
        case 0:
          op = { op: "insert", index: rand(list.length + 1), item: this.#item++ };
          break;
        case 1:
          op = { op: "remove", index: rand(list.length) };
          break;
        case 2:
          op = { op: "update", index: rand(list.length), item: this.#item++ };
          break;
        case 3:
          op = { op: "move", from: rand(list.length), to: rand(list.length) };
          break;
        case 4:
          op = rand(8) === 0 ? { op: "clear" } : { op: "remove", index: list.length - 1 };
          break;
        default:
          op = { op: "insert", index: list.length, item: this.#item++ };
          break;
      }
      ops.push(op);
      this.state(h).lists.set(id, applyPatch(list, [op]));
    }
    if (earlier !== undefined) return this.full(h, id);
    if (this.corrupt && rand(30) === 0) ops.splice(rand(ops.length + 1), 0, { op: "remove", index: 1_000_000 });
    return { handle: h, signalId: id, op: ChangeOp.KeyedPatch, value: patchBytes(ops, codecs.u32) };
  }

  /** Small position-relative ops on the string list (what follows a bulk patch the mirror may have dropped). */
  #patchStrings(h: Handle, earlier: Entry | undefined): Entry {
    const rand = this.rand;
    const s = this.state(h);
    const ops: PatchOp<string>[] = [];
    for (let i = 0; i < 1 + rand(3); i++) {
      const n = s.strings.length;
      const kind = n === 0 ? 0 : rand(4);
      let op: PatchOp<string>;
      if (kind === 0) op = { op: "insert", index: rand(n + 1), item: `s${this.#item++}` };
      else if (kind === 1) op = { op: "remove", index: rand(n) };
      else if (kind === 2) op = { op: "update", index: rand(n), item: `u${this.#item++}` };
      else op = { op: "move", from: rand(n), to: rand(n) };
      ops.push(op);
      s.strings = applyPatch(s.strings, [op]);
    }
    if (earlier !== undefined) return this.full(h, STRING_LIST);
    return { handle: h, signalId: STRING_LIST, op: ChangeOp.KeyedPatch, value: patchBytes(ops, codecs.string) };
  }

  /** A patch on the string list that passes both patch bounds by itself: clear, then 4,200 inserts of 270 bytes. */
  #bulk(h: Handle, earlier: Entry | undefined): Entry {
    const s = this.state(h);
    const ops: PatchOp<string>[] = [{ op: "clear" }];
    const items: string[] = [];
    const tag = this.#item++;
    for (let i = 0; i < MAX_MERGED_PATCH_OPS + 104; i++) {
      const item = `${tag}:${i}:${"y".repeat(256)}`;
      ops.push({ op: "insert", index: i, item });
      items.push(item);
    }
    s.strings = items;
    if (earlier !== undefined) return this.full(h, STRING_LIST);
    return { handle: h, signalId: STRING_LIST, op: ChangeOp.KeyedPatch, value: patchBytes(ops, codecs.string) };
  }

  /** The core handles resync requests: each re-observe sends the current value, behind what is already on the wire. */
  answerResyncs(max: number): void {
    for (let i = 0; i < max && this.resyncRequests.length > 0; i++) {
      const [h, id] = this.resyncRequests.shift() as [Handle, number];
      this.#txn++;
      this.wire.push(encodeChangeSet({ txnId: this.#txn, entries: [this.full(h, id)] }));
    }
  }
}

class ModelHost {
  readonly state = new Map<Handle, StoreState>(HANDLES.map((h) => [h, emptyState()]));
  readonly progressSeen: number[] = [];

  constructor(readonly core: ModelCore) {}

  apply(h: Handle): (signalId: number, op: ChangeOp, value: Uint8Array) => void {
    return (signalId, op, value) => {
      const s = this.state.get(h) as StoreState;
      if (signalId === LAZY) {
        if (op === ChangeOp.FullValue) {
          const v = decodeLazyValue(value);
          s.lazy = { handle: Number(v.handle), len: v.len, version: Number(v.version) };
        } else {
          // An invalidation is relative to the page server a full value named: it is never applied to one the host never saw.
          const v = decodeLazyInvalidated(value);
          s.lazy = { ...s.lazy, len: v.len, version: Number(v.version) };
        }
        return;
      }
      if (signalId === STRING_LIST) {
        if (op === ChangeOp.FullValue) s.strings = decodeValue(strList, value);
        else {
          const r = new UndraReader(value);
          const ops = decodePatch(r, codecs.string);
          r.finish();
          try {
            s.strings = applyPatch(s.strings, ops);
          } catch (error) {
            if (!(error instanceof PatchError)) throw error;
            this.core.resyncRequests.push([h, signalId]);
          }
        }
        return;
      }
      if (U32_LISTS.includes(signalId)) {
        if (op === ChangeOp.FullValue) s.lists.set(signalId, decodeValue(u32List, value));
        else {
          const r = new UndraReader(value);
          const ops = decodePatch(r, codecs.u32);
          r.finish();
          try {
            s.lists.set(signalId, applyPatch(s.lists.get(signalId) ?? [], ops));
          } catch (error) {
            if (!(error instanceof PatchError)) throw error;
            // What generated code does: re-observe (here: the core answers later).
            this.core.resyncRequests.push([h, signalId]);
          }
        }
        return;
      }
      const v = decodeValue(codecs.u32, value);
      s.scalars.set(signalId, v);
      if (h === NO_COALESCE_HANDLE && signalId === NO_COALESCE_SIGNAL) this.progressSeen.push(v);
    };
  }
}

function isSubsequence(sub: readonly number[], of: readonly number[]): boolean {
  let at = 0;
  for (const v of sub) {
    while (at < of.length && of[at] !== v) at++;
    if (at === of.length) return false;
    at++;
  }
  return true;
}

function runHistory(seed: number, bulk: boolean): { compactions: number; resyncs: number; checks: number } {
  const rand = rng(seed);
  const corrupt = rand(2) === 0;
  const core = new ModelCore(rand, bulk, corrupt);
  const host = new ModelHost(core);
  const small = rand(2) === 0;
  const mirror = new Mirror({
    schedule: () => {},
    onError: (error) => {
      throw error;
    },
    maxPendingEntries: small ? 2 + rand(24) : DEFAULT_MAX_PENDING_ENTRIES,
    maxPendingBytes: small ? 64 + rand(4096) : DEFAULT_MAX_PENDING_BYTES,
    resync: (h, id) => core.resyncRequests.push([h, id]),
  });
  for (const h of HANDLES) {
    mirror.register(h, host.apply(h), h === NO_COALESCE_HANDLE ? { noCoalesce: [NO_COALESCE_SIGNAL] } : {});
  }
  const deliver = (n: number): void => {
    for (let i = 0; i < n && core.wire.length > 0; i++) mirror.enqueue(core.wire.shift() as Uint8Array);
  };
  let checks = 0;
  const settle = (context: string): void => {
    // Deliver everything, answer every resync, drain; repeat until nothing is outstanding.
    for (let guard = 0; guard < 100; guard++) {
      core.answerResyncs(Number.MAX_SAFE_INTEGER);
      deliver(Number.MAX_SAFE_INTEGER);
      mirror.flush();
      if (core.wire.length === 0 && core.resyncRequests.length === 0 && mirror.pending === 0) break;
    }
    checks++;
    for (const h of HANDLES) {
      expect(render(host.state.get(h) as StoreState), `${context} handle ${String(h)}`).toBe(render(core.state(h)));
    }
  };
  const steps = 20 + rand(120);
  for (let step = 0; step < steps; step++) {
    const pick = rand(100);
    if (pick < 55) core.commit();
    else if (pick < 75) deliver(1 + rand(6));
    else if (pick < 87) {
      mirror.flush();
      if (!corrupt) {
        // Between settles a signal may lag the core, never show a state the core never had.
        for (const h of HANDLES) {
          for (const id of ALL_KEYS) {
            const shown = renderKey(host.state.get(h) as StoreState, id);
            expect(core.history.get(`${String(h)}/${id}`)?.has(shown), `seed ${seed} step ${step}: ${String(h)}/${id} shows ${shown.slice(0, 80)}`).toBe(true);
          }
        }
      }
    }
    else if (pick < 95) core.answerResyncs(1 + rand(2));
    else settle(`seed ${seed} step ${step}`);
  }
  settle(`seed ${seed} end`);
  const stats = mirror.stats();
  const committed = core.progressCommitted;
  if (committed.length > 0) {
    expect(host.progressSeen.at(-1), `seed ${seed}: the no_coalesce signal ends on the last value`).toBe(committed.at(-1));
    expect(isSubsequence(host.progressSeen, committed), `seed ${seed}: no_coalesce values in commit order`).toBe(true);
    if (stats.compactions === 0) expect(host.progressSeen, `seed ${seed}: every no_coalesce value applied`).toEqual(committed);
  }
  return { compactions: stats.compactions, resyncs: stats.resyncs, checks };
}

describe("review: a merged drain equals the core under random delivery, drains, compactions and asynchronous resyncs", () => {
  it("2,000 histories without bulk patches", () => {
    const seeds = Number(process.env["UNDRA_MODEL_SEEDS"] ?? 2000);
    let compactions = 0;
    let checks = 0;
    for (let seed = 1; seed <= seeds; seed++) {
      const r = runHistory(seed, false);
      compactions += r.compactions;
      checks += r.checks;
    }
    expect(compactions).toBeGreaterThan(100);
    expect(checks).toBeGreaterThan(seeds);
  }, 120_000);

  it("150 histories with patches that pass both bounds (dropped by a compaction, re-observed)", () => {
    expect(MAX_MERGED_PATCH_BYTES).toBe(1024 * 1024);
    let resyncs = 0;
    for (let seed = 10_001; seed <= 10_150; seed++) resyncs += runHistory(seed, true).resyncs;
    expect(resyncs).toBeGreaterThan(0);
  }, 300_000);
});
