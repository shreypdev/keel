import { encodeChangeSet, ChangeOp, UndraWriter } from "@undra/runtime/wire";
import { SchemaIndex, type Schema } from "../src/schema.js";

/** A schema with a `Todo` record, a `Filter` enum, a `Counter` store and a `Todos` store with a keyed list. */
export const SCHEMA: Schema = {
  crate_name: "test-core",
  records: [
    {
      name: "Todo",
      type_id: 1,
      fields: [
        { name: "id", ty: { kind: "u32" } },
        { name: "title", ty: { kind: "string" } },
        { name: "done", ty: { kind: "bool" } },
      ],
    },
    { name: "Header", type_id: 2, fields: [{ name: "name", ty: { kind: "string" } }, { name: "value", ty: { kind: "string" } }] },
  ],
  enums: [
    {
      name: "Filter",
      type_id: 3,
      is_error: false,
      variants: [
        { name: "All", index: 0, fields: [], tuple: false },
        { name: "Tag", index: 1, fields: [{ name: "0", ty: { kind: "string" } }], tuple: true },
        { name: "Range", index: 2, fields: [{ name: "from", ty: { kind: "i32" } }, { name: "to", ty: { kind: "i32" } }], tuple: false },
      ],
    },
  ],
  objects: [
    {
      name: "Counter",
      type_id: 10,
      constructors: [{ name: "new", method_id: 100, params: [{ name: "initial", ty: { kind: "i32" } }], returns: { kind: "named", of: "Counter" }, is_async: false }],
      methods: [{ name: "add", method_id: 101, params: [{ name: "n", ty: { kind: "i32" } }], returns: { kind: "i32" }, is_async: false }],
      store: {
        signals: [
          { name: "count", signal_id: 0, ty: { kind: "i32" }, computed: false, key: null },
          { name: "label", signal_id: 1, ty: { kind: "string" }, computed: true, key: null },
        ],
      },
    },
    {
      name: "Todos",
      type_id: 11,
      constructors: [],
      methods: [],
      store: {
        signals: [
          { name: "items", signal_id: 0, ty: { kind: "vec", of: { kind: "named", of: "Todo" } }, computed: false, key: "id" },
          { name: "filter", signal_id: 1, ty: { kind: "named", of: "Filter" }, computed: false, key: null },
        ],
      },
    },
  ],
  functions: [{ name: "sum", method_id: 200, params: [], returns: { kind: "i32" }, is_async: false }],
  ports: [
    {
      name: "Http",
      port_id: 300,
      kind: "async",
      methods: [
        {
          name: "send",
          method_id: 301,
          params: [{ name: "url", ty: { kind: "string" } }, { name: "retries", ty: { kind: "u8" } }],
          returns: { kind: "result", of: [{ kind: "u16" }, { kind: "string" }] },
          is_async: true,
        },
      ],
    },
  ],
  queries: [
    { name: "todos_page", query_id: 400, kind: "query", key: "todos/{page}", params: [{ name: "page", ty: { kind: "u32" } }], returns: { kind: "vec", of: { kind: "named", of: "Todo" } } },
  ],
};

export const index = (): SchemaIndex => new SchemaIndex(SCHEMA);

/** Bytes written by `f`. */
export function bytes(f: (w: UndraWriter) => void): Uint8Array {
  const w = new UndraWriter();
  f(w);
  return w.finish();
}

export function todo(w: UndraWriter, id: number, title: string, done = false): void {
  w.writeU32(id);
  w.writeStr(title);
  w.writeBool(done);
}

export const todoBytes = (id: number, title: string, done = false): Uint8Array => bytes((w) => todo(w, id, title, done));

export const listBytes = (items: readonly [number, string, boolean?][]): Uint8Array =>
  bytes((w) => {
    w.writeLen(items.length);
    for (const [id, title, done] of items) todo(w, id, title, done ?? false);
  });

/** A keyed patch of the `Todo` list. */
export type Op = ["insert", number, number, string] | ["remove", number] | ["update", number, number, string] | ["move", number, number] | ["clear"];

export function patchBytes(ops: readonly Op[]): Uint8Array {
  return bytes((w) => {
    w.writeLen(ops.length);
    for (const op of ops) {
      switch (op[0]) {
        case "insert":
          w.writeU8(0);
          w.writeU32(op[1]);
          todo(w, op[2], op[3]);
          break;
        case "remove":
          w.writeU8(1);
          w.writeU32(op[1]);
          break;
        case "update":
          w.writeU8(2);
          w.writeU32(op[1]);
          todo(w, op[2], op[3]);
          break;
        case "move":
          w.writeU8(3);
          w.writeU32(op[1]);
          w.writeU32(op[2]);
          break;
        case "clear":
          w.writeU8(4);
          break;
      }
    }
  });
}

export const i32Bytes = (n: number): Uint8Array => bytes((w) => w.writeI32(n));

export interface Entry {
  handle: bigint;
  signalId: number;
  op: ChangeOp;
  value: Uint8Array;
}

export const changeSet = (txn: bigint, entries: readonly Entry[]): Uint8Array => encodeChangeSet({ txnId: txn, entries });

export const full = (handle: bigint, signalId: number, value: Uint8Array): Entry => ({ handle, signalId, op: ChangeOp.FullValue, value });
export const patch = (handle: bigint, signalId: number, ops: readonly Op[]): Entry => ({ handle, signalId, op: ChangeOp.KeyedPatch, value: patchBytes(ops) });

export const unhex = (hex: string): Uint8Array => Uint8Array.from(hex.match(/../g) ?? [], (h) => Number.parseInt(h, 16));
export const hexOf = (b: Uint8Array): string => [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
