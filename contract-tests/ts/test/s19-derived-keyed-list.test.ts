import { expect, test } from "vitest";
import {
  ChangeOp,
  Mirror,
  type PatchOp,
  UndraReader,
  applyPatch,
  codecs,
  decodeChangeSet,
  decodePatch,
  decodeValue,
} from "@undra/runtime";
import { type Filter, FilterCodec, type Todo, TodoCodec, Todos, UndraIds } from "@playground/core";
import { boot } from "../src/harness.js";
import { RawStore, type SignalUpdate, args, valueOf } from "../src/raw-store.js";
import { counters } from "../src/stats.js";
import { step } from "../src/wait.js";
import { VECTORS_HANDLE, VIEW_CODECS, hashOf, readVectors, splitmix } from "../src/derived-vectors.js";

// S19 derived keyed list: `Todos.visible` is a derived list (ADR-039): it reaches the platform as
// keyed patches, never as a whole list after the first; and 60,000 recorded operations over three
// derived views replay through this runtime's patch decoder, applier and mirror to exactly the views
// the core computed.

const Ids = UndraIds.Objects.Todos;
const VISIBLE = 2;
const REMAINING = 3;
const vecTodo = codecs.vec(TodoCodec);

function only(entries: readonly SignalUpdate[], signalId: number): SignalUpdate {
  const found = entries.filter((e) => e.signalId === signalId);
  expect(found, `entries for signal ${signalId}`).toHaveLength(1);
  return found[0] as SignalUpdate;
}

function patchOf(entry: SignalUpdate): PatchOp<Todo>[] {
  expect(entry.op, "the entry is a keyed patch").toBe(ChangeOp.KeyedPatch);
  const r = new UndraReader(entry.value);
  const ops = decodePatch(r, TodoCodec);
  r.finish();
  return ops;
}

test("S19 derived keyed list", async () => {
  const { core } = await boot();
  const raw = await RawStore.open(core, Ids);
  let host: Todo[] = [];
  const apply = (entry: SignalUpdate): void => {
    host = entry.op === ChangeOp.FullValue ? decodeValue(vecTodo, entry.value) : applyPatch(host, patchOf(entry));
  };
  const add = async (title: string): Promise<Todo> =>
    decodeValue(TodoCodec, await raw.call(Ids.add, args((w) => w.writeStr(title))));
  const toggle = (todo: Todo) => raw.callWith(Ids.toggle, codecs.uuid, todo.id);
  const setFilter = (filter: Filter) => raw.callWith(Ids.setFilter, FilterCodec, filter);
  const todos: Todo[] = [];

  await step("1. the initial visible entry is a full value, []", () => {
    const entry = only(raw.take(), VISIBLE);
    expect(entry.op).toBe(ChangeOp.FullValue);
    apply(entry);
    expect(host).toEqual([]);
  });

  await step("2. add a, b, c: each is one Insert at 0, 1, 2; remaining 1, 2, 3", async () => {
    for (const [index, title] of ["a", "b", "c"].entries()) {
      const todo = await add(title);
      todos.push(todo);
      const entries = raw.take();
      const visible = only(entries, VISIBLE);
      expect(patchOf(visible)).toEqual([{ op: "insert", index, item: todo }]);
      apply(visible);
      expect(valueOf(entries, REMAINING, codecs.u32)).toBe(index + 1);
    }
  });
  const [a, b] = todos as [Todo, Todo, Todo];

  await step("3. toggle(b) under All is one Update at 1; remaining 2", async () => {
    await toggle(b);
    const entries = raw.take();
    const visible = only(entries, VISIBLE);
    expect(patchOf(visible)).toEqual([{ op: "update", index: 1, item: { ...b, done: true } }]);
    apply(visible);
    expect(valueOf(entries, REMAINING, codecs.u32)).toBe(2);
  });

  await step("4. set_filter(Active) is one Remove at 1; set_filter(All) one Insert of b at 1", async () => {
    await setFilter("active");
    let visible = only(raw.take(), VISIBLE);
    expect(patchOf(visible)).toEqual([{ op: "remove", index: 1 }]);
    apply(visible);
    await setFilter("all");
    visible = only(raw.take(), VISIBLE);
    expect(patchOf(visible)).toEqual([{ op: "insert", index: 1, item: { ...b, done: true } }]);
    apply(visible);
  });

  await step("5. under Active, toggle(a) is one Remove at 0, and back one Insert of a at 0", async () => {
    await setFilter("active");
    apply(only(raw.take(), VISIBLE));
    await toggle(a);
    let visible = only(raw.take(), VISIBLE);
    expect(patchOf(visible)).toEqual([{ op: "remove", index: 0 }]);
    apply(visible);
    await toggle(a);
    visible = only(raw.take(), VISIBLE);
    expect(patchOf(visible)).toEqual([{ op: "insert", index: 0, item: a }]);
    apply(visible);
    await setFilter("all");
    apply(only(raw.take(), VISIBLE));
  });

  await step("6. fill(10000), then a toggle of a visible item: one op, under 100 bytes", async () => {
    await raw.callWith(Ids.fill, codecs.u32, 10_000);
    for (const entry of raw.take().filter((e) => e.signalId === VISIBLE)) apply(entry);
    expect(host).toHaveLength(10_003);
    const target = host[5_000] as Todo;
    expect(target.done).toBe(false);
    await toggle(target);
    const visible = only(raw.take(), VISIBLE);
    const ops = patchOf(visible);
    expect(ops).toEqual([{ op: "update", index: 5_000, item: { ...target, done: true } }]);
    expect(visible.value.length, "one op, not the view").toBeLessThan(100);
    apply(visible);
  });
  raw.close();

  await step("7. through the generated class, visible equals the model after every step", async () => {
    const store = await Todos.create(core);
    const model: Todo[] = [];
    const visibleOf = (filter: Filter): Todo[] =>
      model.filter((t) => filter === "all" || (filter === "active" ? !t.done : t.done));
    let filter: Filter = "all";
    const check = () => {
      expect(store.visible.peek()).toEqual(visibleOf(filter));
      expect(store.remaining.peek()).toBe(model.filter((t) => !t.done).length);
    };
    for (const title of ["a", "b", "c"]) {
      model.push(await store.add(title));
      check();
    }
    const flip = async (i: number) => {
      const t = model[i] as Todo;
      await store.toggle(t.id);
      model[i] = { ...t, done: !t.done };
      check();
    };
    await flip(1);
    for (const next of ["active", "all", "active"] as Filter[]) {
      await store.setFilter(next);
      filter = next;
      check();
    }
    await flip(0);
    await flip(0);
    await store.setFilter("all");
    filter = "all";
    check();
    await store.fill(10_000);
    // Identities count up from the store's counter (the `n`th is `n` in the first eight bytes).
    const idOf = (n: number): string => {
      const hex = n.toString(16).padStart(16, "0");
      return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-0000-000000000000`;
    };
    expect((model[2] as Todo).id).toBe(idOf(3));
    for (let n = 0; n < 10_000; n++) model.push({ id: idOf(4 + n), title: `Item ${n + 1}`, done: n % 4 === 3 });
    check();
    await flip(5_003);
    await store.remove((model[0] as Todo).id);
    model.shift();
    check();
    await store.clearDone();
    model.splice(0, model.length, ...model.filter((t) => !t.done));
    check();
    store.close();
  });

  await step("8. reads never cross: 1,000 reads of visible leave crossings.calls unchanged", async () => {
    const store = await Todos.create(core);
    await store.add("x");
    const before = await counters(core);
    let sink = 0;
    for (let i = 0; i < 1_000; i++) sink += store.visible.peek().length + store.visible.get().length;
    const after = await counters(core);
    expect(after.calls - before.calls).toBe(0);
    expect(sink).toBe(2_000);
    store.close();
  });

  await step("9. 60,000 recorded operations over three views replay to the core's views", () => {
    const records = readVectors();
    expect(records.length).toBeGreaterThan(20_000);
    // a. Applied change-set by change-set with this runtime's decoder and applier.
    const views: unknown[][] = [[], [], []];
    let patches = 0;
    for (const [i, record] of records.entries()) {
      for (const entry of decodeChangeSet(record.payload).entries) {
        const codec = VIEW_CODECS[entry.signalId] as (typeof VIEW_CODECS)[number];
        if (entry.op === ChangeOp.FullValue) {
          views[entry.signalId] = decodeValue(codecs.vec(codec), entry.value);
        } else {
          const r = new UndraReader(entry.value);
          views[entry.signalId] = applyPatch(views[entry.signalId] as unknown[], decodePatch(r, codec));
          r.finish();
          patches++;
        }
      }
      for (let v = 0; v < 3; v++) {
        if (hashOf(views[v] as unknown[], VIEW_CODECS[v] as (typeof VIEW_CODECS)[number]) !== record.hashes[v]) {
          throw new Error(`view ${v} differs from the core's after change-set ${i}`);
        }
      }
    }
    expect(patches).toBeGreaterThan(50_000);
    // b. Through a mirror that merges what arrives between drains (ADR-031), drained at seeded points.
    const merged: unknown[][] = [[], [], []];
    const mirror = new Mirror({
      schedule: () => {},
      onError: (error) => {
        throw error;
      },
    });
    mirror.register(VECTORS_HANDLE, (signalId, op, value) => {
      const codec = VIEW_CODECS[signalId] as (typeof VIEW_CODECS)[number];
      if (op === ChangeOp.FullValue) {
        merged[signalId] = decodeValue(codecs.vec(codec), value);
      } else {
        const r = new UndraReader(value);
        merged[signalId] = applyPatch(merged[signalId] as unknown[], decodePatch(r, codec));
        r.finish();
      }
    });
    const next = splitmix(19n);
    let drains = 0;
    // The recording's scripted prologue (seeded_views.rs): records 2-3 are a row's Move + Update in
    // the sorted views and then its Remove, records 4-5 a rebuild's full values and then patches.
    // Each pair is drained as one. Records 2-3: one merged patch per view (six entries received,
    // three applied). Records 4-5: each view's full value, then its patch (six applied).
    const scriptedDrains = new Map([
      [0, 0],
      [1, 0],
      [3, 3],
      [5, 6],
    ]);
    let applied = 0;
    for (const [i, record] of records.entries()) {
      mirror.enqueue(record.payload);
      const scripted = i <= 5;
      if (scripted ? scriptedDrains.has(i) : next() % 12 === 0 || i === records.length - 1) {
        mirror.flush();
        drains++;
        const appliedInDrain = scriptedDrains.get(i);
        if (scripted && appliedInDrain !== undefined && appliedInDrain > 0) {
          expect(mirror.stats().entriesApplied - applied, `entries the drain of records ${i - 1}-${i} applies`).toBe(
            appliedInDrain,
          );
        }
        applied = mirror.stats().entriesApplied;
        for (let v = 0; v < 3; v++) {
          if (hashOf(merged[v] as unknown[], VIEW_CODECS[v] as (typeof VIEW_CODECS)[number]) !== record.hashes[v]) {
            throw new Error(`merged view ${v} differs from the core's at change-set ${i}`);
          }
        }
      }
    }
    expect(drains).toBeGreaterThan(1_000);
    expect(mirror.stats().entriesApplied).toBeLessThan(mirror.stats().entriesReceived);
  });
});
