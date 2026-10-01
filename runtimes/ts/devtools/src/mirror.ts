/** The page's own copy of the core's stores, kept by applying the change-sets the server forwards (SPEC 3.5, 3.8). */

import { ChangeOp, decodeChangeSet, type ChangeEntry } from "@undra/runtime/wire";
import type { ObjectDef, SchemaIndex, SignalDef } from "./schema.js";
import { applyPatch, decodePatchOps, decodeValue, PatchMismatch, type PatchOp, type Value } from "./value.js";

export interface SignalState {
  readonly def: SignalDef | undefined;
  /** The decoded value; `undefined` until the first value arrives. */
  value: Value | undefined;
  /** A lazy list that the core invalidated: its pages are the platform's to fetch. */
  invalidated: boolean;
  /** Why the page could not decode or apply the last entry, if it could not. */
  error: string | undefined;
}

export interface StoreState {
  readonly handle: bigint;
  readonly typeId: number;
  readonly def: ObjectDef | undefined;
  readonly signals: Map<number, SignalState>;
}

/** What one entry of a change-set did to one signal. */
export interface AppliedChange {
  readonly handle: bigint;
  readonly signalId: number;
  readonly store: string;
  readonly signal: string;
  readonly op: "full" | "patch" | "lazy";
  readonly before: Value | undefined;
  readonly after: Value | undefined;
  /** The operations of a keyed patch. */
  readonly patch: readonly PatchOp[] | undefined;
  readonly error: string | undefined;
}

/** Entries kept for a store the server has not announced yet. */
const MAX_ORPHANS = 256;

const message = (e: unknown): string => (e instanceof Error ? e.message : String(e));

export class Mirror {
  readonly stores = new Map<bigint, StoreState>();
  readonly #orphans: { txn: bigint; entry: ChangeEntry }[] = [];

  constructor(readonly schema: SchemaIndex) {}

  /** The set of stores the core holds: new ones appear, ones that are gone disappear. Returns what entries held back for a new store did. */
  setStores(refs: readonly { readonly handle: bigint; readonly typeId: number }[]): AppliedChange[] {
    const now = new Set(refs.map((r) => r.handle));
    for (const h of [...this.stores.keys()]) if (!now.has(h)) this.stores.delete(h);
    const fresh: bigint[] = [];
    for (const r of refs) {
      if (this.stores.has(r.handle)) continue;
      const def = this.schema.objectsByTypeId.get(r.typeId);
      const signals = new Map<number, SignalState>();
      for (const s of def?.store?.signals ?? []) signals.set(s.signal_id, { def: s, value: undefined, invalidated: false, error: undefined });
      this.stores.set(r.handle, { handle: r.handle, typeId: r.typeId, def, signals });
      fresh.push(r.handle);
    }
    const changes: AppliedChange[] = [];
    const held = this.#orphans.splice(0);
    for (const { entry } of held) {
      if (this.stores.has(entry.handle)) changes.push(this.#applyEntry(entry));
      else this.#orphans.push({ txn: 0n, entry });
    }
    return changes;
  }

  /** Applies one change-set whole. Throws when the payload itself is malformed (nothing is applied then). */
  apply(payload: Uint8Array): { readonly txn: bigint; readonly changes: AppliedChange[] } {
    const set = decodeChangeSet(payload);
    const changes: AppliedChange[] = [];
    for (const entry of set.entries) {
      if (!this.stores.has(entry.handle)) {
        // The server announces a store before it observes it, but a page that attaches late can see its values first.
        if (this.#orphans.length < MAX_ORPHANS) this.#orphans.push({ txn: set.txnId, entry: { ...entry, value: entry.value.slice() } });
        continue;
      }
      changes.push(this.#applyEntry(entry));
    }
    return { txn: set.txnId, changes };
  }

  #applyEntry(entry: ChangeEntry): AppliedChange {
    const store = this.stores.get(entry.handle) as StoreState;
    let signal = store.signals.get(entry.signalId);
    if (signal === undefined) {
      signal = { def: undefined, value: undefined, invalidated: false, error: undefined };
      store.signals.set(entry.signalId, signal);
    }
    const base = {
      handle: entry.handle,
      signalId: entry.signalId,
      store: store.def?.name ?? `store ${store.typeId}`,
      signal: signal.def?.name ?? `signal ${entry.signalId}`,
    };
    const before = signal.value;
    const fail = (op: AppliedChange["op"], e: unknown): AppliedChange => {
      signal.error = message(e);
      return { ...base, op, before, after: before, patch: undefined, error: signal.error };
    };
    signal.error = undefined;
    if (entry.op === ChangeOp.LazyInvalidated) {
      signal.invalidated = true;
      return { ...base, op: "lazy", before, after: before, patch: undefined, error: undefined };
    }
    const def = signal.def;
    if (def === undefined) {
      signal.value = entry.value.slice();
      return { ...base, op: "full", before, after: signal.value, patch: undefined, error: undefined };
    }
    try {
      if (entry.op === ChangeOp.FullValue) {
        signal.value = decodeValue(this.schema, def.ty, entry.value);
        signal.invalidated = false;
        return { ...base, op: "full", before, after: signal.value, patch: undefined, error: undefined };
      }
      if (def.ty.kind !== "vec") return fail("patch", `a keyed patch for ${def.name}, which is not a list`);
      const ops = decodePatchOps(this.schema, def.ty.of, entry.value);
      signal.value = applyPatch(Array.isArray(before) ? before : [], ops);
      return { ...base, op: "patch", before, after: signal.value, patch: ops, error: undefined };
    } catch (e) {
      // A patch that does not fit leaves the list as it was: the page is out of step with the core and says so.
      return fail(entry.op === ChangeOp.FullValue ? "full" : "patch", e instanceof PatchMismatch ? `out of step: ${e.message}` : e);
    }
  }
}
