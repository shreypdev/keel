// Executable stand-in for the `@keel/runtime` base API (see `index.d.ts` for the contract). It sits
// next to the *real* wire layer, compiled from `runtimes/ts/@keel/runtime/src` into `./dist`, so the
// execution tests (`tests/run_ts.rs`) run generated code against the real codecs. The base classes
// are just enough to construct generated objects and stores and to let a test-owned fake core
// answer their calls.

export * from "./dist/index.js";

export class KeelError extends Error {
  constructor(kind, message, options) {
    super(message, options);
    this.kind = kind;
  }
}

export class KeelReplyError extends KeelError {
  constructor(status, body) {
    super("reply", `keel reply with status ${status}`);
    this.status = status;
    this.body = body;
  }
}

export class KeelPortError extends KeelError {
  constructor(body) {
    super("port", "typed port failure");
    this.body = body;
  }
}

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
export class KeelCore {
  static get shared() {
    throw new Error("no shared core in this test");
  }
}

export class KeelObject {
  constructor(core, handle) {
    this.core = core;
    this.handle = handle;
  }

  close() {
    this.core.release(this.handle);
  }
}

export class KeelStore extends KeelObject {
  constructor(core, handle) {
    super(core, handle);
    this._signals = [];
    core.mirror.register(handle, (signalId, op, value) => this._apply(signalId, op, value));
  }
}
