// Executable stand-in for the `@undra/runtime` base API (see `index.d.ts` for the contract). It sits
// next to the *real* wire layer, compiled from `runtimes/ts/@undra/runtime/src` into `./dist`, so the
// execution tests (`tests/run_ts.rs`) run generated code against the real codecs. The base classes
// are just enough to construct generated objects and stores and to let a test-owned fake core
// answer their calls.

import { ALL_SIGNALS, UndraCallError } from "./dist/index.js";

// The real wire layer, errors (including the closed set of generated calls), adapters and standard types.
export * from "./dist/index.js";

export class Signal {
  #value;
  #subscribers = new Set();

  constructor(initial) {
    this.#value = initial;
  }

  get() {
    return this.#value;
  }

  peek() {
    return this.#value;
  }

  subscribe(fn) {
    this.#subscribers.add(fn);
    return () => this.#subscribers.delete(fn);
  }

  _set(value) {
    this.#value = value;
    for (const fn of this.#subscribers) fn(value);
  }
}

/** A core to subclass in tests; the real one is loaded from WebAssembly or over a socket. */
export class UndraCore {
  static get shared() {
    throw new Error("no shared core in this test");
  }

  static get current() {
    return null;
  }

  static load() {
    return Promise.reject(new Error("no core to load in this test"));
  }

  static attach() {
    return Promise.reject(new Error("no core to attach in this test"));
  }

  /** The closed placeholder (ADR-044): what a generated entry's `core` is while nothing is loaded. */
  static get unloaded() {
    throw new Error("no core is loaded in this test; pass one explicitly");
  }

  get closed() {
    return false;
  }

  /** The connection state (ADR-051); the callback registry (ADR-041) listens to it. */
  connection = new Signal({ kind: "connected" });

  /** The ports registered (a callback interface's bridge is registered at its first lend, ADR-041). */
  ports = new Map();

  registerPort(portId, impl) {
    this.ports.set(portId, impl);
  }

  /** ADR-040: one reference given back (a duplicate in a reply); `adopt` calls it. */
  _giveBack(handle) {
    (this.givenBack ??= []).push(handle);
  }

  /** ADR-040: a handle a new wrapper now holds; `adopt` calls it. */
  _held() {}
}

export class UndraObject {
  #closed = false;

  constructor(core, handle) {
    this.core = core;
    this.handle = handle;
  }

  get closed() {
    return this.#closed;
  }

  close() {
    if (this.#closed) return;
    this.#closed = true;
    this.core.release(this.handle);
  }
}

export class UndraStore extends UndraObject {
  constructor(core, handle, options = {}) {
    super(core, handle);
    this._signals = [];
    core.mirror.register(handle, (signalId, op, value) => this._apply(signalId, op, value), options);
  }

  async _observeAll() {
    try {
      await this.core.observe(this.handle, ALL_SIGNALS, true);
    } catch (error) {
      this.close();
      throw UndraCallError.mapped(error);
    }
  }
}
