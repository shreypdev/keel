// Shared helpers of the execution tests: loading the compiled package and a fake core.

import { join } from "node:path";
import { pathToFileURL } from "node:url";

export const hex = (bytes) => Buffer.from(bytes).toString("hex");
export const bytes = (text) => new Uint8Array(Buffer.from(text, "hex"));

/** Imports `path` (relative to the generated package directory `pkg`). */
export function loader(pkg) {
  return (path) => import(pathToFileURL(join(pkg, path)).href);
}

/** Loads the runtime and every generated module of the package at `pkg`. */
export async function setup(pkg) {
  const load = loader(pkg);
  const rt = await load("node_modules/@undra/runtime/index.js");
  const modules = {};
  for (const name of ["types", "errors", "objects", "stores", "ports", "queries", "ids"]) {
    modules[name] = await load(`dist/${name}.js`);
  }
  return { rt, ...modules, UndraIds: modules.ids.UndraIds };
}

/** A core that records what generated code asks of it and answers from queues. */
export function fakeCoreClass(rt) {
  return class FakeCore extends rt.UndraCore {
    calls = [];
    constructed = [];
    observed = [];
    events = [];
    replies = [];
    streams = [];
    mirrorFns = new Map();
    nextHandle = 7n;
    mirror = {
      register: (handle, fn) => this.mirrorFns.set(handle, fn),
      unregister: (handle) => this.mirrorFns.delete(handle),
    };

    async call(target, methodId, args, signal) {
      this.calls.push({ target, methodId, args: hex(args), signal });
      const reply = this.replies.shift();
      if (reply instanceof Error) throw reply;
      return reply ?? new Uint8Array(0);
    }

    stream(target, methodId, args) {
      this.calls.push({ target, methodId, args: hex(args) });
      const items = this.streams.shift() ?? [];
      return (async function* () {
        for (const item of items) {
          if (item instanceof Error) throw item;
          yield item;
        }
      })();
    }

    async construct(typeId, methodId, args) {
      this.constructed.push({ typeId, methodId, args: hex(args) });
      const reply = this.replies.shift();
      if (reply instanceof Error) throw reply;
      return this.nextHandle++;
    }

    async observe(handle, signalId, on) {
      this.observed.push({ handle, signalId, on });
    }

    event(portId, methodId, payload) {
      this.events.push({ portId, methodId, payload: hex(payload) });
    }

    release() {}

    /** Delivers a change-set entry the way the mirror would. */
    deliver(handle, signalId, op, value) {
      this.mirrorFns.get(handle)(signalId, op, value);
    }
  };
}
