import { afterEach, describe, expect, it, vi } from "vitest";
import { UndraCore } from "../src/core.js";
import {
  DEFAULT_MAX_PENDING_BYTES,
  DEFAULT_MAX_PENDING_ENTRIES,
  type DrainStats,
  MAX_MERGED_PATCH_BYTES,
  MAX_MERGED_PATCH_OPS,
  Mirror,
  type MirrorOptions,
} from "../src/mirror.js";
import { Signal } from "../src/signal.js";
import { runWorker, type WorkerScope } from "../src/worker.js";
import { WasmWorkerTransport, type WorkerLike } from "../src/transport/wasm-worker.js";
import {
  ALL_SIGNALS,
  CallTarget,
  ChangeOp,
  Kind,
  PatchError,
  type PatchOp,
  UndraReader,
  UndraWriter,
  applyPatch,
  codecs,
  decodeEnvelope,
  decodePatch,
  decodeValue,
  encodeCall,
  encodeChangeSet,
  encodeEnvelope,
  encodePatch,
  encodeValue,
  type Codec,
  type Handle,
} from "../src/wire/index.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { STUB, compileStub } from "./support/stub-core.js";
import { track } from "./support/harness.js";
import { CounterStore, u32 } from "./support/store.js";

/*
 * Frame-coalesced delivery (ADR-031, docs/SPEC.md section 11): the merge rules of a drain, its
 * equivalence with applying every change-set in order (property test), the bounded backlog,
 * read-your-writes for replies and synchronous calls, the frame scheduler, the worker's batching,
 * the drain listener and the counters, and the `no_coalesce` opt-out.
 */

// ----- helpers ---------------------------------------------------------------------------------

let txn = 0n;

/** One change-set payload. */
function cs(...entries: Array<{ handle?: Handle; signalId: number; op?: ChangeOp; value: Uint8Array }>): Uint8Array {
  return encodeChangeSet({
    txnId: ++txn,
    entries: entries.map((e) => ({ handle: e.handle ?? 1n, signalId: e.signalId, op: e.op ?? ChangeOp.FullValue, value: e.value })),
  });
}

/** A keyed patch of `u32` items. */
function patchBytes(ops: readonly PatchOp<number>[], item: Codec<number> = codecs.u32): Uint8Array {
  const w = new UndraWriter();
  encodePatch(w, ops, item);
  return w.finish();
}

const listCodec = codecs.vec(codecs.u32);

/** A manual scheduler: the drain runs only when the test says so. */
function manual(): { readonly schedule: (fn: () => void) => void; readonly pending: Array<() => void>; run(): void } {
  const pending: Array<() => void> = [];
  return {
    schedule: (fn) => {
      pending.push(fn);
    },
    pending,
    run() {
      while (pending.length > 0) pending.shift()?.();
    },
  };
}

/** A tiny deterministic PRNG (xorshift32), so a failing case can be replayed from its seed. */
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

/**
 * A host-side store of keyed `u32` lists and scalars, applying entries the way generated code
 * does: a full value decodes, a keyed patch goes through `applyPatch` (one copy of the list per
 * entry) and an out-of-bounds patch asks for a resync.
 */
class ListHost {
  readonly lists = new Map<number, Signal<number[]>>();
  readonly scalars = new Map<number, Signal<number>>();
  applies = 0;
  readonly notified: number[] = [];

  constructor(
    readonly listIds: readonly number[],
    readonly scalarIds: readonly number[],
    readonly onResync: (signalId: number) => void,
  ) {
    for (const id of listIds) {
      const s = new Signal<number[]>([]);
      s.subscribe(() => this.notified.push(id));
      this.lists.set(id, s);
    }
    for (const id of scalarIds) {
      const s = new Signal<number>(0);
      s.subscribe(() => this.notified.push(id));
      this.scalars.set(id, s);
    }
  }

  readonly apply = (signalId: number, op: ChangeOp, value: Uint8Array): void => {
    this.applies++;
    const list = this.lists.get(signalId);
    if (list !== undefined) {
      if (op === ChangeOp.FullValue) {
        list._set(decodeValue(listCodec, value));
      } else if (op === ChangeOp.KeyedPatch) {
        const r = new UndraReader(value);
        const ops = decodePatch(r, codecs.u32);
        r.finish();
        try {
          list._set(applyPatch(list.peek(), ops));
        } catch (error) {
          if (!(error instanceof PatchError)) throw error;
          this.onResync(signalId);
        }
      }
      return;
    }
    const scalar = this.scalars.get(signalId);
    if (scalar !== undefined && op === ChangeOp.FullValue) scalar._set(decodeValue(codecs.u32, value));
  };

  snapshot(): { lists: Record<number, number[]>; scalars: Record<number, number> } {
    const lists: Record<number, number[]> = {};
    const scalars: Record<number, number> = {};
    for (const [id, s] of this.lists) lists[id] = s.peek();
    for (const [id, s] of this.scalars) scalars[id] = s.peek();
    return { lists, scalars };
  }
}

/** The core's side of a {@link ListHost}: the truth, and the change-sets that move it. */
class ListCore {
  readonly lists = new Map<number, number[]>();
  readonly scalars = new Map<number, number>();
  #nextItem = 1;

  constructor(listIds: readonly number[], scalarIds: readonly number[]) {
    for (const id of listIds) this.lists.set(id, []);
    for (const id of scalarIds) this.scalars.set(id, 0);
  }

  full(signalId: number): Uint8Array {
    const list = this.lists.get(signalId);
    if (list !== undefined) return cs({ signalId, value: encodeValue(listCodec, list) });
    return cs({ signalId, value: encodeValue(codecs.u32, this.scalars.get(signalId) ?? 0) });
  }

  /** Random valid ops on list `signalId` (applied to the truth); `corrupt` appends one out-of-bounds op the core never made. */
  patch(signalId: number, rand: (n: number) => number, count: number, corrupt: boolean): Uint8Array {
    const ops: PatchOp<number>[] = [];
    for (let i = 0; i < count; i++) {
      const list = this.lists.get(signalId) as number[];
      const kind = list.length === 0 ? 0 : rand(5);
      let op: PatchOp<number>;
      switch (kind) {
        case 0:
          op = { op: "insert", index: rand(list.length + 1), item: this.#nextItem++ };
          break;
        case 1:
          op = { op: "remove", index: rand(list.length) };
          break;
        case 2:
          op = { op: "update", index: rand(list.length), item: this.#nextItem++ };
          break;
        case 3:
          op = { op: "move", from: rand(list.length), to: rand(list.length) };
          break;
        default:
          op = rand(10) === 0 ? { op: "clear" } : { op: "insert", index: list.length, item: this.#nextItem++ };
          break;
      }
      ops.push(op);
      this.lists.set(signalId, applyPatch(this.lists.get(signalId) as number[], [op]));
    }
    const current = this.lists.get(signalId) as number[];
    if (corrupt) ops.push({ op: "remove", index: current.length + 3 });
    return cs({ signalId, op: ChangeOp.KeyedPatch, value: patchBytes(ops) });
  }

  /** A new full value of list `signalId`. */
  replace(signalId: number, rand: (n: number) => number): Uint8Array {
    const list = Array.from({ length: rand(6) }, () => this.#nextItem++);
    this.lists.set(signalId, list);
    return cs({ signalId, value: encodeValue(listCodec, list) });
  }

  scalar(signalId: number, value: number): Uint8Array {
    this.scalars.set(signalId, value);
    return cs({ signalId, value: encodeValue(codecs.u32, value) });
  }

  snapshot(): { lists: Record<number, number[]>; scalars: Record<number, number> } {
    const lists: Record<number, number[]> = {};
    const scalars: Record<number, number> = {};
    for (const [id, l] of this.lists) lists[id] = l;
    for (const [id, v] of this.scalars) scalars[id] = v;
    return { lists, scalars };
  }
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

// ----- the merge rules -------------------------------------------------------------------------

describe("a drain merges per signal", () => {
  it("applies only the last full value of a signal, once", () => {
    const m = manual();
    const mirror = new Mirror({ schedule: m.schedule });
    const seen: number[] = [];
    mirror.register(1n, (_id, _op, value) => seen.push(decodeValue(codecs.u32, value)));
    for (let i = 1; i <= 1000; i++) mirror.enqueue(cs({ signalId: 0, value: u32(i) }));
    expect(m.pending).toHaveLength(1);
    m.run();
    expect(seen).toEqual([1000]);
    expect(mirror.stats()).toMatchObject({ changeSetsReceived: 1000, entriesReceived: 1000, entriesApplied: 1, drains: 1 });
  });

  it("concatenates consecutive keyed patches into one: counts add up, ops keep their order", () => {
    const mirror = new Mirror({ schedule: () => {} });
    const values: Uint8Array[] = [];
    mirror.register(1n, (_id, op, value) => {
      expect(op).toBe(ChangeOp.KeyedPatch);
      values.push(value.slice());
    });
    const a: PatchOp<number>[] = [{ op: "insert", index: 0, item: 7 }];
    const b: PatchOp<number>[] = [
      { op: "move", from: 0, to: 0 },
      { op: "update", index: 0, item: 8 },
    ];
    const c: PatchOp<number>[] = [{ op: "clear" }];
    for (const ops of [a, b, c]) mirror.enqueue(cs({ signalId: 3, op: ChangeOp.KeyedPatch, value: patchBytes(ops) }));
    mirror.flush();
    expect(values).toHaveLength(1);
    expect([...(values[0] as Uint8Array)]).toEqual([...patchBytes([...a, ...b, ...c])]);
    const r = new UndraReader(values[0] as Uint8Array);
    expect(decodePatch(r, codecs.u32)).toEqual([...a, ...b, ...c]);
  });

  it("applies a signal at most twice: its last full value, then the patches after it, merged", () => {
    const mirror = new Mirror({ schedule: () => {} });
    const seen: Array<[ChangeOp, number[]]> = [];
    mirror.register(1n, (_id, op, value) => {
      if (op === ChangeOp.FullValue) seen.push([op, decodeValue(listCodec, value)]);
      else seen.push([op, decodePatch(new UndraReader(value), codecs.u32).map((o) => ("item" in o ? o.item : -1))]);
    });
    mirror.enqueue(cs({ signalId: 0, op: ChangeOp.KeyedPatch, value: patchBytes([{ op: "insert", index: 0, item: 1 }]) }));
    mirror.enqueue(cs({ signalId: 0, value: encodeValue(listCodec, [5]) }));
    mirror.enqueue(cs({ signalId: 0, op: ChangeOp.KeyedPatch, value: patchBytes([{ op: "insert", index: 1, item: 6 }]) }));
    mirror.enqueue(cs({ signalId: 0, op: ChangeOp.KeyedPatch, value: patchBytes([{ op: "insert", index: 2, item: 7 }]) }));
    mirror.flush();
    expect(seen).toEqual([
      [ChangeOp.FullValue, [5]],
      [ChangeOp.KeyedPatch, [6, 7]],
    ]);
  });

  describe("a lazy list's signal (ADR-043: an invalidation carries a length and version, the full value the page server's handle)", () => {
    /** What a drain delivers for the signal of the given entries, as `op:value` (the value is a `u32`, or empty for none). */
    function delivered(entries: Array<[ChangeOp, number]>, other: { maxPendingEntries?: number } = {}): string[] {
      const mirror = new Mirror({ schedule: () => {}, ...other });
      const seen: string[] = [];
      mirror.register(1n, (_id, op, value) => seen.push(`${ChangeOp[op]}:${value.length === 0 ? "" : decodeValue(codecs.u32, value)}`));
      for (const [op, n] of entries) mirror.enqueue(cs({ signalId: 0, op, value: u32(n) }));
      mirror.flush();
      return seen;
    }
    const FULL = ChangeOp.FullValue;
    const INV = ChangeOp.LazyInvalidated;

    it("keeps the full value an invalidation follows: [Full, Inv] delivers both, in order", () => {
      expect(delivered([[FULL, 1], [INV, 2]])).toEqual(["FullValue:1", "LazyInvalidated:2"]);
    });

    it("an invalidation supersedes only earlier invalidations: [Full, Inv, Inv] delivers the full value and the last one", () => {
      expect(delivered([[FULL, 1], [INV, 2], [INV, 3]])).toEqual(["FullValue:1", "LazyInvalidated:3"]);
      expect(delivered([[INV, 2], [INV, 3]])).toEqual(["LazyInvalidated:3"]);
      expect(delivered([[INV, 2]])).toEqual(["LazyInvalidated:2"]);
    });

    it("a full value supersedes everything before it: [Inv, Full] delivers the full value; [Full, Inv, Full] the last", () => {
      expect(delivered([[INV, 1], [FULL, 2]])).toEqual(["FullValue:2"]);
      expect(delivered([[FULL, 1], [INV, 2], [FULL, 3]])).toEqual(["FullValue:3"]);
      expect(delivered([[FULL, 1], [INV, 2], [FULL, 3], [INV, 4]])).toEqual(["FullValue:3", "LazyInvalidated:4"]);
    });

    it("keeps the pair through a compaction of the backlog", () => {
      const mirror = new Mirror({ schedule: () => {}, maxPendingEntries: 10 });
      const seen: string[] = [];
      mirror.register(1n, (id, op, value) => seen.push(`${id}:${ChangeOp[op]}:${decodeValue(codecs.u32, value)}`));
      mirror.enqueue(cs({ signalId: 0, value: u32(1) }, { signalId: 0, op: INV, value: u32(2) }));
      for (let i = 0; i < 40; i++) mirror.enqueue(cs({ signalId: 1, value: u32(i) }));
      mirror.enqueue(cs({ signalId: 0, op: INV, value: u32(3) }));
      mirror.flush();
      expect(mirror.stats().compactions).toBeGreaterThan(0);
      expect(seen).toEqual(["0:FullValue:1", "0:LazyInvalidated:3", "1:FullValue:39"]);
    });

    it("an ordinary signal is unaffected: the last full value wins, and keyed patches fold as before", () => {
      expect(delivered([[FULL, 1], [FULL, 2], [FULL, 3]])).toEqual(["FullValue:3"]);
      const mirror = new Mirror({ schedule: () => {} });
      const seen: string[] = [];
      mirror.register(1n, (_id, op, value) => seen.push(op === FULL ? `full ${decodeValue(listCodec, value)}` : `patch ${decodePatch(new UndraReader(value), codecs.u32).length}`));
      mirror.enqueue(cs({ signalId: 0, value: encodeValue(listCodec, [1]) }));
      mirror.enqueue(cs({ signalId: 0, op: ChangeOp.KeyedPatch, value: patchBytes([{ op: "insert", index: 1, item: 2 }]) }));
      mirror.enqueue(cs({ signalId: 0, op: ChangeOp.KeyedPatch, value: patchBytes([{ op: "insert", index: 2, item: 3 }]) }));
      mirror.flush();
      expect(seen).toEqual(["full 1", "patch 2"]);
    });

    it("counts each delivery: the pair is two applied entries", () => {
      const mirror = new Mirror({ schedule: () => {} });
      mirror.register(1n, () => {});
      mirror.enqueue(cs({ signalId: 0, value: u32(1) }, { signalId: 0, op: INV, value: u32(2) }));
      mirror.flush();
      expect(mirror.stats().entriesApplied).toBe(2);
    });
  });

  it("applies signals in the order of their first entry, and announces each once at the end", () => {
    const mirror = new Mirror({ schedule: () => {} });
    const order: string[] = [];
    const a = new Signal(0);
    const b = new Signal(0);
    a.subscribe((v) => order.push(`notify a=${v} (b=${b.peek()})`));
    b.subscribe((v) => order.push(`notify b=${v} (a=${a.peek()})`));
    mirror.register(1n, (_id, _op, value) => {
      order.push("apply a");
      a._set(decodeValue(codecs.u32, value));
    });
    mirror.register(2n, (_id, _op, value) => {
      order.push("apply b");
      b._set(decodeValue(codecs.u32, value));
    });
    mirror.enqueue(cs({ handle: 2n, signalId: 0, value: u32(1) }));
    mirror.enqueue(cs({ handle: 1n, signalId: 0, value: u32(1) }));
    mirror.enqueue(cs({ handle: 2n, signalId: 0, value: u32(2) }, { handle: 1n, signalId: 0, value: u32(2) }));
    mirror.flush();
    expect(order).toEqual(["apply b", "apply a", "notify b=2 (a=2)", "notify a=2 (b=2)"]);
  });

  it("drops the entries of a handle nobody registered, counting every one", () => {
    const mirror = new Mirror({ schedule: () => {} });
    for (let i = 0; i < 5; i++) mirror.enqueue(cs({ handle: 9n, signalId: 0, value: u32(i) }));
    mirror.flush();
    expect(mirror.dropped).toBe(5);
    expect(mirror.stats().entriesApplied).toBe(0);
  });
});

describe("no_coalesce signals", () => {
  it("are applied entry by entry, in order, each announced on its own", () => {
    const mirror = new Mirror({ schedule: () => {} });
    const progress = new Signal(0);
    const other = new Signal(0);
    const heard: string[] = [];
    progress.subscribe((v) => heard.push(`progress ${v}`));
    other.subscribe((v) => heard.push(`other ${v}`));
    mirror.register(
      1n,
      (id, _op, value) => {
        (id === 1 ? progress : other)._set(decodeValue(codecs.u32, value));
      },
      { noCoalesce: [1] },
    );
    for (let i = 1; i <= 3; i++) mirror.enqueue(cs({ signalId: 0, value: u32(10 * i) }, { signalId: 1, value: u32(i) }));
    mirror.flush();
    expect(heard).toEqual(["other 30", "progress 1", "progress 2", "progress 3"]);
    expect(mirror.stats().entriesApplied).toBe(4);
  });

  it("are folded like any other signal when the backlog passes its bound (the bound wins)", () => {
    const mirror = new Mirror({ schedule: () => {}, maxPendingEntries: 10 });
    const seen: number[] = [];
    mirror.register(1n, (_id, _op, value) => seen.push(decodeValue(codecs.u32, value)), { noCoalesce: [0] });
    for (let i = 1; i <= 25; i++) mirror.enqueue(cs({ signalId: 0, value: u32(i) }));
    mirror.flush();
    expect(mirror.stats().compactions).toBeGreaterThan(0);
    expect(seen.at(-1)).toBe(25);
    expect(seen.length).toBeLessThan(25);
  });
});

// ----- equivalence with sequential application (property test) --------------------------------

describe("a merged drain equals applying every change-set in order", () => {
  const LISTS = [0, 2];
  const SCALARS = [1];

  /** A host whose resync is answered at once with the core's state as it is then (wasm-main is synchronous). */
  function host(core: ListCore): { host: ListHost; mirror: Mirror } {
    let mirror: Mirror | undefined;
    const h = new ListHost(LISTS, SCALARS, (signalId) => mirror?.enqueue(core.full(signalId)));
    mirror = new Mirror({ schedule: () => {} });
    mirror.register(1n, h.apply);
    return { host: h, mirror };
  }

  it("for 600 random histories of full values and keyed patches, including out-of-bounds ones", () => {
    let corrupted = 0;
    for (let seed = 1; seed <= 600; seed++) {
      const rand = rng(seed);
      const core = new ListCore(LISTS, SCALARS);
      // Sequential: one drain per change-set, as the core commits them.
      const sequential = host(core);
      const merged = host(core);
      const deliver = (payload: Uint8Array): void => {
        sequential.mirror.enqueue(payload);
        sequential.mirror.flush();
        merged.mirror.enqueue(payload);
      };
      for (const id of [...LISTS, ...SCALARS]) deliver(core.full(id));
      let corrupt = false;
      const steps = 1 + rand(40);
      for (let i = 0; i < steps; i++) {
        const pick = rand(100);
        const list = LISTS[rand(LISTS.length)] as number;
        if (pick < 10) deliver(core.replace(list, rand));
        else if (pick < 25) deliver(core.scalar(1, rand(1000)));
        else {
          const bad = rand(25) === 0;
          corrupt ||= bad;
          deliver(core.patch(list, rand, 1 + rand(4), bad));
        }
      }
      // Merged: every change-set committed before one drain (a resync gets the final state).
      merged.mirror.flush();

      const truth = core.snapshot();
      const context = `seed ${seed}`;
      if (corrupt) corrupted++;
      expect(sequential.host.snapshot(), context).toEqual(truth);
      expect(merged.host.snapshot(), context).toEqual(truth);
      expect(merged.mirror.pending, context).toBe(0);
      if (!corrupt) {
        // Merging applies a signal at most twice and announces each signal once.
        expect(merged.host.applies, context).toBeLessThanOrEqual(2 * (LISTS.length + SCALARS.length));
        expect(new Set(merged.host.notified).size, context).toBe(merged.host.notified.length);
      }
    }
    expect(corrupted, "some histories carried an out-of-bounds patch").toBeGreaterThan(10);
  });
});

// ----- the bounded backlog ---------------------------------------------------------------------

describe("the backlog is bounded", () => {
  it("1,000,000 change-sets with no drain stay under the bound, and one drain converges", () => {
    const LISTS = [0, 1];
    const SCALARS = [2, 3, 4, 5];
    const rand = rng(31337);
    const core = new ListCore(LISTS, SCALARS);
    let mirror: Mirror;
    const host = new ListHost(LISTS, SCALARS, (signalId) => mirror.enqueue(core.full(signalId)));
    const resyncs: number[] = [];
    mirror = new Mirror({
      schedule: () => {}, // the main thread is blocked: nothing drains until the end
      resync: (_handle, signalId) => {
        resyncs.push(signalId);
        mirror.enqueue(core.full(signalId));
      },
    });
    mirror.register(1n, host.apply);
    for (const id of [...LISTS, ...SCALARS]) mirror.enqueue(core.full(id));
    let maxEntries = 0;
    let maxBytes = 0;
    for (let i = 0; i < 1_000_000; i++) {
      const pick = rand(10);
      if (pick < 6) mirror.enqueue(core.patch(LISTS[i & 1] as number, rand, 1, false));
      else mirror.enqueue(core.scalar(SCALARS[i % SCALARS.length] as number, i));
      if ((i & 1023) === 0) {
        const s = mirror.stats();
        maxEntries = Math.max(maxEntries, s.pendingEntries);
        maxBytes = Math.max(maxBytes, s.pendingBytes);
      }
    }
    const before = mirror.stats();
    expect(maxEntries).toBeLessThanOrEqual(DEFAULT_MAX_PENDING_ENTRIES);
    expect(maxBytes).toBeLessThanOrEqual(DEFAULT_MAX_PENDING_BYTES);
    expect(before.compactions).toBeGreaterThan(5);
    expect(before.changeSetsReceived).toBe(1_000_006);

    mirror.flush();
    expect(host.snapshot()).toEqual(core.snapshot());
    expect(mirror.pending).toBe(0);
    // Each list's patches passed the operations bound (about 300,000 ops of 9 bytes each), so they were dropped and re-observed.
    expect(resyncs.sort()).toEqual([0, 1]);
    expect(mirror.stats().resyncs).toBe(2);
  }, 60_000);

  it("drops a merged patch that passes the patch bounds and re-observes its signal once", () => {
    const big = codecs.string;
    let truth: string[] = [];
    const resyncs: number[] = [];
    let mirror: Mirror;
    const list = new Signal<string[]>([]);
    const full = (): Uint8Array => cs({ signalId: 0, value: encodeValue(codecs.vec(big), truth) });
    mirror = new Mirror({
      schedule: () => {},
      maxPendingEntries: 1000,
      resync: (_h, signalId) => {
        resyncs.push(signalId);
        mirror.enqueue(full());
      },
    });
    mirror.register(1n, (_id, op, value) => {
      if (op === ChangeOp.FullValue) list._set(decodeValue(codecs.vec(big), value));
      else {
        const r = new UndraReader(value);
        list._set(applyPatch(list.peek(), decodePatch(r, big)));
      }
    });
    const text = "x".repeat(300);
    // Compactions run every 1,000 entries; the one after the 1 MiB-th byte (about 3,400 ops) finds the merged patch past a bound.
    for (let i = 0; i < MAX_MERGED_PATCH_OPS + 1500; i++) {
      const op: PatchOp<string> = { op: "insert", index: truth.length, item: `${i}${text}` };
      truth = [...truth, op.item];
      const w = new UndraWriter();
      encodePatch(w, [op], big);
      mirror.enqueue(cs({ signalId: 0, op: ChangeOp.KeyedPatch, value: w.finish() }));
    }
    expect(mirror.stats().compactions).toBeGreaterThan(0);
    // Patches that arrive while the signal waits for its full value are discarded.
    expect(mirror.pending).toBeLessThan(1000);
    mirror.flush();
    expect(resyncs).toEqual([0]);
    expect(list.peek()).toEqual(truth);
  });

  it("drops a merged patch past either bound: a few large items (bytes), or many small operations (count)", () => {
    // ADR-031 decision 3 promises O(observed keys x (value + 1 MiB)): the byte bound must drop a
    // patch on its own, however few operations it has (40 inserts of 64 KiB here), and the
    // operation bound caps what a drain replays (5,000 inserts of a one-letter item, 50 KiB).
    for (const [shape, count, item, maxPendingEntries] of [
      ["bytes", 40, "z".repeat(64 * 1024), 8],
      ["operations", MAX_MERGED_PATCH_OPS + 904, "a", 64],
    ] as const) {
      let truth: string[] = [];
      const resyncs: number[] = [];
      const list = new Signal<string[]>([]);
      const strings = codecs.vec(codecs.string);
      let mirror: Mirror;
      mirror = new Mirror({
        schedule: () => {},
        maxPendingEntries,
        resync: (_h, signalId) => {
          resyncs.push(signalId);
          mirror.enqueue(cs({ signalId: 0, value: encodeValue(strings, truth) }));
        },
      });
      mirror.register(1n, (_id, op, value) => {
        if (op === ChangeOp.FullValue) list._set(decodeValue(strings, value));
        else list._set(applyPatch(list.peek(), decodePatch(new UndraReader(value), codecs.string)));
      });
      let maxBytes = 0;
      for (let i = 0; i < count; i++) {
        const op: PatchOp<string> = { op: "insert", index: truth.length, item };
        truth = [...truth, item];
        const w = new UndraWriter();
        encodePatch(w, [op], codecs.string);
        mirror.enqueue(cs({ signalId: 0, op: ChangeOp.KeyedPatch, value: w.finish() }));
        maxBytes = Math.max(maxBytes, mirror.stats().pendingBytes);
      }
      expect(maxBytes, shape).toBeLessThan(MAX_MERGED_PATCH_BYTES + 2 * 64 * 1024 * maxPendingEntries);
      mirror.flush();
      expect(resyncs, shape).toEqual([0]);
      expect(list.peek(), shape).toEqual(truth);
    }
  });

  it("folds when the bytes pass their bound too", () => {
    const mirror = new Mirror({ schedule: () => {}, maxPendingBytes: 64 * 1024 });
    const seen: number[] = [];
    mirror.register(1n, (_id, _op, value) => seen.push(value.length));
    const blob = new Uint8Array(4096);
    for (let i = 0; i < 100; i++) mirror.enqueue(cs({ signalId: 0, value: blob }));
    expect(mirror.stats().compactions).toBeGreaterThan(0);
    expect(mirror.stats().pendingBytes).toBeLessThanOrEqual(64 * 1024);
    mirror.flush();
    expect(seen).toEqual([4096]);
  });
});

// ----- read-your-writes ------------------------------------------------------------------------

describe("read-your-writes", () => {
  const SET = 0x51;
  const HANDLE = 0x1_0000_0001n;

  async function setup(synchronous: boolean) {
    const fake = new FakeCoreTransport({ synchronous });
    const core = track(
      await UndraCore.attach(fake, {
        expectedSchemaHash: SCHEMA,
        shared: false,
        adapters: { log: { log() {} }, http: null, timer: null },
        // No frame ever comes: only a reply (or a synchronous call) can apply what the core sent.
        mirror: { schedule: () => {} },
      }),
    );
    fake.store(HANDLE, new Map([[0, u32(1)]]));
    // The method writes the counter in a change-set that arrives before its reply.
    fake.on(SET, (call, r) => {
      const value = decodeValue(codecs.u32, "args" in call ? call.args : new Uint8Array(0));
      fake.setSignal(HANDLE, 0, u32(value));
      r.ok();
    });
    const store = await CounterStore.create(core, HANDLE);
    return { fake, core, store };
  }

  it("holds after `await` of an asynchronous reply", async () => {
    const { core, store } = await setup(false);
    expect(store.count.peek()).toBe(1);
    await core.call({ target: CallTarget.ObjectMethod, handle: HANDLE }, SET, u32(7));
    expect(store.count.peek()).toBe(7);
    expect(core.mirror.pending).toBe(0);
  });

  it("holds after `await` of a reply delivered inside the call (wasm-main)", async () => {
    const { core, store } = await setup(true);
    await core.call({ target: CallTarget.ObjectMethod, handle: HANDLE }, SET, u32(8));
    expect(store.count.peek()).toBe(8);
  });

  it("holds when a synchronous call returns", async () => {
    const { core, store } = await setup(true);
    core.callSync({ target: CallTarget.ObjectMethod, handle: HANDLE }, SET, u32(9));
    expect(store.count.peek()).toBe(9);
  });

  it("holds for a rejected call too", async () => {
    const { fake, core, store } = await setup(false);
    fake.on(SET + 1, (_call, r) => {
      fake.setSignal(HANDLE, 0, u32(13));
      r.error(u32(0));
    });
    await expect(core.call({ target: CallTarget.ObjectMethod, handle: HANDLE }, SET + 1, new Uint8Array(0))).rejects.toThrow();
    expect(store.count.peek()).toBe(13);
  });

  it("leaves a change the core makes on its own to the next frame", async () => {
    const { fake, core, store } = await setup(false);
    fake.setSignal(HANDLE, 0, u32(21));
    await fake.settle();
    expect(store.count.peek(), "not applied before a frame").toBe(1);
    expect(core.mirror.pending).toBe(1);
    core.mirror.flush();
    expect(store.count.peek()).toBe(21);
  });
});

// ----- the frame scheduler ---------------------------------------------------------------------

describe("the default schedule is frame-aligned", () => {
  function fakeFrames(visibility: string) {
    const frames: Array<(t: number) => void> = [];
    vi.stubGlobal("document", { visibilityState: visibility });
    vi.stubGlobal("requestAnimationFrame", (fn: (t: number) => void) => frames.push(fn));
    vi.stubGlobal("cancelAnimationFrame", (id: number) => {
      frames[id - 1] = () => {};
    });
    return {
      frames,
      tick() {
        const due = frames.splice(0);
        for (const fn of due) fn(0);
      },
    };
  }

  it("drains once per animation frame while the document is visible", async () => {
    vi.useFakeTimers();
    const raf = fakeFrames("visible");
    const mirror = new Mirror();
    const seen: number[] = [];
    mirror.register(1n, (_id, _op, value) => seen.push(decodeValue(codecs.u32, value)));
    for (let i = 1; i <= 50; i++) mirror.enqueue(cs({ signalId: 0, value: u32(i) }));
    await Promise.resolve();
    expect(raf.frames).toHaveLength(1);
    expect(seen).toEqual([]);
    raf.tick();
    expect(seen).toEqual([50]);
    // The 100 ms backstop was cleared: no second drain.
    mirror.enqueue(cs({ signalId: 0, value: u32(51) }));
    vi.advanceTimersByTime(99);
    expect(seen).toEqual([50]);
    raf.tick();
    expect(seen).toEqual([50, 51]);
    vi.advanceTimersByTime(1000);
    expect(mirror.stats().drains).toBe(2);
  });

  it("drains from a timer when a requested frame never comes", () => {
    vi.useFakeTimers();
    const raf = fakeFrames("visible");
    const mirror = new Mirror();
    const seen: number[] = [];
    mirror.register(1n, (_id, _op, value) => seen.push(decodeValue(codecs.u32, value)));
    mirror.enqueue(cs({ signalId: 0, value: u32(1) }));
    vi.advanceTimersByTime(100);
    expect(seen).toEqual([1]);
    raf.tick(); // the late frame does nothing
    expect(mirror.stats().drains).toBe(1);
  });

  it("drains in a zero-delay task while the document is hidden", () => {
    vi.useFakeTimers();
    const raf = fakeFrames("hidden");
    const mirror = new Mirror();
    const seen: number[] = [];
    mirror.register(1n, (_id, _op, value) => seen.push(decodeValue(codecs.u32, value)));
    mirror.enqueue(cs({ signalId: 0, value: u32(3) }));
    expect(raf.frames).toHaveLength(0);
    vi.advanceTimersByTime(0);
    expect(seen).toEqual([3]);
  });

  it("drains in a microtask where there is no document (Node)", async () => {
    const mirror = new Mirror();
    const seen: number[] = [];
    mirror.register(1n, (_id, _op, value) => seen.push(decodeValue(codecs.u32, value)));
    mirror.enqueue(cs({ signalId: 0, value: u32(1) }));
    mirror.enqueue(cs({ signalId: 0, value: u32(2) }));
    await Promise.resolve();
    expect(seen).toEqual([2]);
  });

  it("an observe waiter drains without waiting for the frame", async () => {
    const raf = fakeFrames("visible");
    const mirror = new Mirror();
    mirror.register(1n, () => {});
    const observed = mirror.whenObserved(1n, ALL_SIGNALS);
    mirror.enqueue(cs({ signalId: 0, value: u32(1) }));
    await observed; // no frame ever runs here: the waiter's microtask drained it
    expect(raf.frames).toHaveLength(0);
    expect(mirror.stats().entriesApplied).toBe(1);
  });
});

// ----- drain listener and counters -------------------------------------------------------------

describe("the drain listener and the counters", () => {
  it("reports each drain and stops when removed", () => {
    const mirror = new Mirror({ schedule: () => {} });
    mirror.register(1n, () => {});
    const drains: DrainStats[] = [];
    const remove = mirror.addDrainListener((s) => drains.push(s));
    mirror.enqueue(cs({ signalId: 0, value: u32(1) }, { signalId: 1, value: u32(1) }));
    mirror.enqueue(cs({ signalId: 0, value: u32(2) }));
    mirror.flush();
    expect(drains).toHaveLength(1);
    expect(drains[0]).toMatchObject({ changeSets: 2, entries: 3, appliedEntries: 2 });
    expect(drains[0]?.durationMs).toBeGreaterThanOrEqual(0);
    remove();
    remove();
    mirror.enqueue(cs({ signalId: 0, value: u32(3) }));
    mirror.flush();
    expect(drains).toHaveLength(1);
    expect(mirror.stats()).toMatchObject({
      changeSetsReceived: 3,
      entriesReceived: 4,
      entriesApplied: 3,
      drains: 2,
      compactions: 0,
      resyncs: 0,
      pendingEntries: 0,
      pendingBytes: 0,
    });
  });

  it("a listener that throws is reported and does not stop the others", () => {
    const errors: unknown[] = [];
    const mirror = new Mirror({ schedule: () => {}, onError: (e) => errors.push(e) });
    mirror.register(1n, () => {});
    const boom = new Error("listener failed");
    mirror.addDrainListener(() => {
      throw boom;
    });
    const second = vi.fn();
    mirror.addDrainListener(second);
    mirror.enqueue(cs({ signalId: 0, value: u32(1) }));
    mirror.flush();
    expect(errors).toEqual([boom]);
    expect(second).toHaveBeenCalledOnce();
  });

  it("UndraCore.stats() carries the mirror's counters", async () => {
    const fake = new FakeCoreTransport({ synchronous: true });
    const core = track(await UndraCore.attach(fake, { expectedSchemaHash: SCHEMA, shared: false, adapters: { log: { log() {} }, http: null, timer: null } }));
    fake.store(5n, new Map([[0, u32(1)]]));
    await CounterStore.create(core, 5n);
    const stats = await core.stats();
    expect(stats.mirror).toMatchObject({ changeSetsReceived: 1, entriesReceived: 1, entriesApplied: 1, drains: 1 });
  });

  it("options reach the core's mirror", async () => {
    const fake = new FakeCoreTransport({ synchronous: false });
    const schedule = vi.fn();
    const options: MirrorOptions["schedule"] = schedule;
    const core = track(
      await UndraCore.attach(fake, {
        expectedSchemaHash: SCHEMA,
        shared: false,
        adapters: { log: { log() {} }, http: null, timer: null },
        mirror: { schedule: options, maxPendingEntries: 4 },
      }),
    );
    for (let i = 0; i < 10; i++) fake.emitChangeSet([{ handle: 1n, signalId: 0, op: ChangeOp.FullValue, value: u32(i) }]);
    await fake.settle();
    expect(schedule).toHaveBeenCalledOnce();
    expect((await core.stats()).mirror.compactions).toBeGreaterThan(0);
  });
});

// ----- the worker batches ----------------------------------------------------------------------

describe("the worker sends one task's envelopes as one message", () => {
  /** A worker scope driven by hand: the test delivers messages and reads what the worker posted. */
  function scope(): { scope: WorkerScope; posted: Array<{ message: { t: string; data?: unknown }; transfer: Transferable[] }>; deliver(message: unknown): void } {
    const listeners: Array<(event: Event) => void> = [];
    const posted: Array<{ message: { t: string; data?: unknown }; transfer: Transferable[] }> = [];
    return {
      posted,
      scope: {
        addEventListener: (_type: string, fn: EventListenerOrEventListenerObject | null) => {
          listeners.push(fn as (event: Event) => void);
        },
        removeEventListener: () => {},
        postMessage: (message: unknown, transfer?: Transferable[]) => {
          posted.push({ message: message as { t: string }, transfer: transfer ?? [] });
        },
      },
      deliver(message) {
        for (const fn of listeners) fn({ data: message } as MessageEvent);
      },
    };
  }

  async function startWorker(protocol: number | undefined) {
    const module = await WebAssembly.compile((await compileStub()) as Uint8Array<ArrayBuffer>);
    const s = scope();
    const stop = runWorker(s.scope);
    s.deliver({
      t: "init",
      wasm: { kind: "module", module },
      expectedSchemaHash: STUB.SCHEMA_HASH,
      platform: "test",
      devtools: false,
      logLevel: 2,
      ...(protocol === undefined ? {} : { protocol }),
    });
    await vi.waitFor(() => {
      expect(s.posted.some((p) => p.message.t === "ready")).toBe(true);
    });
    s.posted.length = 0;
    return { ...s, stop };
  }

  const call = (method: number) =>
    encodeEnvelope(Kind.Call, 0, STUB.SCHEMA_HASH, encodeCall({ target: CallTarget.FreeFunction, methodId: method, callId: 9, args: u32(5) })).buffer;

  it("batches a stream's reply, item and end into one transferred message", async () => {
    const w = await startWorker(2);
    w.deliver({ t: "envelope", data: call(STUB.STREAM) });
    expect(w.posted).toHaveLength(0); // flushed from a microtask
    await Promise.resolve();
    expect(w.posted).toHaveLength(1);
    const { message, transfer } = w.posted[0] as (typeof w.posted)[0];
    expect(message.t).toBe("envelopes");
    const data = message.data as ArrayBuffer[];
    expect(data).toHaveLength(3);
    expect(transfer).toEqual(data);
    expect(data.map((b) => decodeEnvelope(new Uint8Array(b), STUB.SCHEMA_HASH).kind)).toEqual([Kind.Reply, Kind.StreamItem, Kind.StreamItem]);
    w.stop();
  });

  it("sends one envelope per message to a host that did not announce protocol 2", async () => {
    const w = await startWorker(undefined);
    w.deliver({ t: "envelope", data: call(STUB.STREAM) });
    await Promise.resolve();
    expect(w.posted.map((p) => p.message.t)).toEqual(["envelope", "envelope", "envelope"]);
    w.stop();
  });

  /**
   * A real `MessageChannel` between the main-thread transport and a worker serving on the other
   * end. `downgrade` strips the protocol version from `init`, as a version 1 host would send it.
   */
  async function overChannel(downgrade: boolean) {
    const module = await WebAssembly.compile((await compileStub()) as Uint8Array<ArrayBuffer>);
    const channel = new MessageChannel();
    channel.port1.start();
    channel.port2.start();
    let messages = 0;
    channel.port2.addEventListener("message", () => {
      messages++;
    });
    const host: WorkerLike = {
      addEventListener: (type, fn) => {
        channel.port2.addEventListener(type as "message", fn as (event: MessageEvent) => void);
      },
      removeEventListener: (type, fn) => {
        channel.port2.removeEventListener(type as "message", fn as (event: MessageEvent) => void);
      },
      postMessage: (message, transfer) => {
        const m = message as { t: string; protocol?: number };
        const sent = downgrade && m.t === "init" ? { ...m, protocol: undefined } : m;
        channel.port2.postMessage(sent, transfer ?? []);
      },
      close: () => {
        channel.port2.close();
      },
    };
    const stop = runWorker(channel.port1 as unknown as WorkerScope);
    const transport = new WasmWorkerTransport({ wasm: module, expectedSchemaHash: STUB.SCHEMA_HASH, worker: host });
    const core = track(await UndraCore.attach(transport, { expectedSchemaHash: STUB.SCHEMA_HASH, shared: false, adapters: { log: { log() {} }, http: null, timer: null } }));
    return {
      core,
      messages: () => messages,
      close() {
        core.close();
        stop();
        channel.port1.close();
      },
    };
  }

  it("the main thread reads a batch in order: a stream's reply, item and end cost it one message", async () => {
    const w = await overChannel(false);
    const before = w.messages();
    const items: number[] = [];
    for await (const item of w.core.stream(CallTarget.FreeFunction, STUB.STREAM, u32(5))) items.push(decodeValue(codecs.u32, item));
    expect(items).toEqual([5]);
    expect(w.messages() - before).toBe(1);
    w.close();
  });

  it("a batch that is not a list of envelopes fails the transport as a protocol error", async () => {
    // A hand-rolled worker: answers `init` with `ready`, then sends a malformed protocol 2 batch.
    const listeners: Array<(event: Event) => void> = [];
    const worker: WorkerLike = {
      addEventListener: (type, fn) => {
        if (type === "message") listeners.push(fn as (event: Event) => void);
      },
      removeEventListener: () => {},
      postMessage: (message) => {
        if ((message as { t: string }).t !== "init") return;
        const hello = { undraVersion: "test", schemaHash: STUB.SCHEMA_HASH, platform: "test", mode: "dev" };
        queueMicrotask(() => {
          for (const fn of listeners) fn({ data: { t: "ready", hello } } as MessageEvent);
        });
      },
      close: () => {},
    };
    const transport = new WasmWorkerTransport({ wasm: new Uint8Array(8), expectedSchemaHash: STUB.SCHEMA_HASH, worker });
    const closed: unknown[] = [];
    await transport.start({
      reply: () => {},
      changeSet: () => {},
      streamItem: () => {},
      portCall: () => ({ kind: "unavailable" }),
      log: () => {},
      closed: (error) => closed.push(error),
    });
    for (const fn of listeners) fn({ data: { t: "envelopes", data: 7 } } as MessageEvent);
    expect(closed).toHaveLength(1);
    expect(String(closed[0])).toContain("without a list of envelopes");
  });

  it("the main thread still reads one envelope per message (a version 1 worker's shape)", async () => {
    const w = await overChannel(true);
    const before = w.messages();
    const items: number[] = [];
    for await (const item of w.core.stream(CallTarget.FreeFunction, STUB.STREAM, u32(6))) items.push(decodeValue(codecs.u32, item));
    expect(items).toEqual([6]);
    expect(w.messages() - before).toBe(3);
    w.close();
  });
});
