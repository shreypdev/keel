import { UndraCallError } from "./call-error.js";
import type { UndraCore } from "./core.js";
import { UndraTransportError } from "./errors.js";
import type { UndraObject } from "./object.js";
import { type Handle, UndraReader } from "./wire/index.js";
import { msg } from "./messages.js";

/*
 * One wrapper per handle (ADR-040 decision 7). Every handle in a successful reply is one reference the host owns; a
 * reply that carries an object the host already wraps gives its reference back at once, so `a.shelf("x") ===
 * a.shelf("x")` while the first wrapper lives, a store returned twice is mirrored once, and closing a wrapper releases
 * exactly the one reference it owns. Generated constructors, and every generated method that returns objects, go
 * through here. The map is per core (two cores issue the same handle numbers), weak (it never keeps a wrapper alive)
 * and swept as it grows: an entry whose wrapper was closed or collected is dropped, or replaced by the next wrapper of
 * its handle.
 */

/**
 * A generated object or store class as {@link adopt} takes it: `Shelf`, `Todos`. Its constructor is private to apps;
 * `adopt` is how the bindings make a wrapper of a handle the core issued.
 */
export interface UndraObjectClass<T extends UndraObject> {
  /** The class's prototype: what makes `adopt(core, handle, Shelf)` a `Shelf`. */
  readonly prototype: T;
}

/** The wrappers of each core, by handle. */
const tables = new WeakMap<UndraCore, Map<Handle, WeakRef<UndraObject>>>();

/** A store's `_observeAll` (protected; reached by name, as the generated `create()` reaches it inside the class). */
interface Observing {
  _observeAll(): Promise<void>;
}

/** The first observation of each store a reply made, so that the same store adopted again waits for its values too. */
const observed = new WeakMap<UndraObject, Promise<void>>();

/**
 * The wrapper of `handle` in `core`, if there is one (closed or not).
 *
 * @internal Crash recovery asks it before it replays a release.
 */
export function wrapperOf(core: UndraCore, handle: Handle): UndraObject | undefined {
  return tables.get(core)?.get(handle)?.deref();
}

/**
 * The wrapper of `handle`, a reference the core just handed to the host (a constructor's or a method's result): the
 * live wrapper of that handle, after giving the extra reference back, or a new `type` that owns it. Generated code
 * calls it; an app never holds a raw handle.
 */
export function adopt<T extends UndraObject>(core: UndraCore, handle: Handle, type: UndraObjectClass<T>): T {
  let live = tables.get(core);
  if (live === undefined) tables.set(core, (live = new Map()));
  const ref = live.get(handle);
  const found = ref?.deref();
  if (found !== undefined && !found.closed) {
    // The reply's reference: the live wrapper keeps the one it owns.
    core._giveBack(handle);
    return found as T;
  }
  // A wrapper that was collected before its finalizer ran still has its mirror registration (whether or not a sweep
  // dropped its entry since); the finalizer only gives its reference back once a newer wrapper holds the handle
  // (`collected`). Without a live wrapper, any registration of the handle is that dead wrapper's.
  if (found === undefined) core.mirror.unregister(handle);
  const made = new (type as unknown as new (core: UndraCore, handle: Handle) => T)(core, handle);
  core._held(handle);
  live.set(handle, new WeakRef(made));
  // As the map doubles past 64, the entries of wrappers that were closed or collected go: O(1) per adoption.
  const size = live.size;
  if (size >= 64 && (size & (size - 1)) === 0) {
    for (const [key, entry] of live) if (entry.deref()?.closed !== false) live.delete(key);
  }
  return made;
}

/** Reads a handle that must not be null (ADR-040 decision 3: absence is an `Option`). */
function readHandle(r: UndraReader): Handle {
  const handle = r.readU64();
  if (handle === 0n) throw new UndraTransportError("protocol", msg(96));
  return handle;
}

/** Adopts `handle` and, for a store that is new, observes it (once), so its signals hold the core's values. */
async function ready<T extends UndraObject>(core: UndraCore, handle: Handle, type: UndraObjectClass<T>): Promise<T> {
  const before = wrapperOf(core, handle);
  const object = adopt(core, handle, type);
  if (object !== before && typeof (object as Partial<Observing>)._observeAll === "function") {
    observed.set(object, (object as unknown as Observing)._observeAll());
  }
  await observed.get(object);
  return object;
}

/**
 * The object a reply `body` carries (`Arc<T>` in Rust), adopted ({@link adopt}); a store is observed before it is
 * returned, exactly as a constructed one is. Rejects with `UndraCallError` when the store cannot be observed, and with
 * a transport `protocol` error (mapped to `Malformed`) for a body that does not hold one handle.
 */
export async function adoptObject<T extends UndraObject>(core: UndraCore, body: Uint8Array, type: UndraObjectClass<T>): Promise<T> {
  const r = new UndraReader(body);
  const handle = readHandle(r);
  r.finish();
  return ready(core, handle, type);
}

/** The optional object a reply `body` carries (`Option<Arc<T>>`): `null`, or the object as {@link adoptObject} returns it. */
export async function adoptOptional<T extends UndraObject>(core: UndraCore, body: Uint8Array, type: UndraObjectClass<T>): Promise<T | null> {
  const r = new UndraReader(body);
  const tag = r.readU8();
  if (tag === 0) {
    r.finish();
    return null;
  }
  if (tag !== 1) throw new UndraTransportError("protocol", msg(97, tag));
  const handle = readHandle(r);
  r.finish();
  return ready(core, handle, type);
}

/**
 * The objects a reply `body` carries (`Vec<Arc<T>>`), in order, each adopted as it is read (a body that breaks off
 * leaves the wrappers made so far to their finalizers, which release them).
 */
export async function adoptList<T extends UndraObject>(core: UndraCore, body: Uint8Array, type: UndraObjectClass<T>): Promise<T[]> {
  const r = new UndraReader(body);
  const count = r.readLen(8);
  const objects: Array<Promise<T>> = [];
  for (let i = 0; i < count; i++) objects.push(ready(core, readHandle(r), type));
  r.finish();
  return Promise.all(objects);
}

/**
 * What `UndraCore.call` hands the reply of a call whose caller aborted after the core had answered (`orphan`), for a
 * method that returns objects: one reference per handle in the body (`shape` 0: one object, 1: an optional one, 2: a
 * list), given back. Without it the reply would be dropped and its references owned by nobody until the core closes.
 *
 * @internal Generated code passes it for an `async` method that returns objects.
 */
export function reclaim(core: UndraCore, shape: 0 | 1 | 2): (body: Uint8Array) => void {
  return (body) => {
    try {
      const r = new UndraReader(body);
      const count = shape === 2 ? r.readLen(8) : shape === 1 ? (r.readU8() === 1 ? 1 : 0) : 1;
      for (let i = 0; i < count; i++) core._giveBack(readHandle(r));
    } catch {
      // A body that does not read as handles carried none.
    }
  };
}

/**
 * The handle of `object` for a call into `core`: an object parameter is borrowed (the wrapper keeps its reference), and
 * an object of another core is refused before anything is sent (two cores issue the same handle numbers, so its handle
 * would name something else there). Throws `UndraCallError.Refused` naming the object's class.
 */
export function requireOwn(core: UndraCore, object: UndraObject): Handle {
  if (object.core === core) return object.handle;
  throw new UndraCallError.Refused(msg(98, object.constructor.name));
}

/**
 * Releases the reference of a wrapper that was garbage-collected without `close()`: its handle too (mirror, observed
 * signals), unless a newer wrapper adopted the handle meanwhile, which keeps them. `era` is the number of restarts
 * (crash recovery) the core had made when the wrapper was made: a newer wrapper of the handle keeps what a wrapper from
 * before a restart owned, because the restored core may not count that reference at all, and giving it back could
 * release the newer wrapper's only one (a leak until the core closes is the lesser harm).
 *
 * @internal The finalizer of `UndraObject`.
 */
export function collected(core: UndraCore, handle: Handle, era: number): void {
  if (wrapperOf(core, handle) === undefined) core.release(handle);
  else if (era === core._era) core._giveBack(handle);
}
