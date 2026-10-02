// A wasm host for a built Undra core, for the symbol test (tests/symbols.rs; ADR-046). It
// instantiates the module with every import stubbed, makes the playground core panic and prints
// what V8 says about the trap, one line per thing:
//
//     FATAL <message of the level-5 record the core logs before it traps>
//     TRAP <the RuntimeError message>
//     FRAME wasm-function[<index>]:0x<module offset>      one per wasm frame of the trap's stack, innermost first
//
// usage: node web.mjs <module.wasm> explode | detached
//   explode   the free function `explode("kaboom")`, a synchronous call: the module traps inside it
//   detached  `explode_detached("task")`, which panics in a task of its own, on the next poll
import { readFileSync } from "node:fs";

const [, , path, scenario = "explode"] = process.argv;
Error.stackTraceLimit = 200;

const module = new WebAssembly.Module(readFileSync(path));
let memory;
const view = () => new Uint8Array(memory.buffer);
const fatal = [];
let schedules = 0;
const imports = { undra: {} };
for (const i of WebAssembly.Module.imports(module)) imports[i.module][i.name] = () => 0;
// The level-5 record: `target String, message String` after the level.
imports.undra.log = (level, ptr, len) => {
  if (level !== 5) return;
  const bytes = view().slice(ptr >>> 0, (ptr >>> 0) + (len >>> 0));
  const dv = new DataView(bytes.buffer);
  const targetLen = dv.getUint32(0, true);
  const messageLen = dv.getUint32(4 + targetLen, true);
  fatal.push(new TextDecoder().decode(bytes.slice(8 + targetLen, 8 + targetLen + messageLen)));
};
imports.undra.schedule = () => {
  schedules++;
};
imports.undra.now_ms = () => 1_700_000_000_000;
imports.undra.random = (ptr, len) => {
  for (let i = 0; i < (len >>> 0); i++) view()[(ptr >>> 0) + i] = (i * 7 + 1) & 0xff;
};

const instance = new WebAssembly.Instance(module, imports);
const x = instance.exports;
memory = x.memory;
x._initialize?.();

const fnv1a32 = (text) => {
  let h = 0x811c9dc5;
  for (const b of new TextEncoder().encode(text)) {
    h ^= b;
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h >>> 0;
};
const u32 = (n) => Uint8Array.of(n & 0xff, (n >>> 8) & 0xff, (n >>> 16) & 0xff, (n >>> 24) & 0xff);
const str = (s) => {
  const b = new TextEncoder().encode(s);
  return Uint8Array.from([...u32(b.length), ...b]);
};
const concat = (...parts) => Uint8Array.from(parts.flatMap((p) => [...p]));
const withBytes = (bytes, fn) => {
  const ptr = x.undra_alloc(Math.max(bytes.length, 1));
  view().set(bytes, ptr);
  try {
    return fn(ptr, bytes.length);
  } finally {
    x.undra_free(ptr, Math.max(bytes.length, 1));
  }
};
// RuntimeConfig: platform String, mode String, core_threads u8, blocking_threads u8, log_level u8.
withBytes(concat(str("node"), str("inproc"), Uint8Array.of(0, 0, 2)), (p, n) => x.undra_init(p, n));
// A Call payload for a free function (SPEC 3.3): target 0, handle 0, method id, call id, args.
const call = (name, callId, args) => concat(Uint8Array.of(0), new Uint8Array(8), u32(fnv1a32(`fn.${name}`)), u32(callId), args);

const report = (error) => {
  for (const message of fatal) console.log(`FATAL ${message.replaceAll("\n", "\\n")}`);
  console.log(`TRAP ${error.message}`);
  for (const line of String(error.stack).split("\n")) {
    const found = /wasm-function\[(\d+)\]:(0x[0-9a-f]+)/.exec(line);
    if (found) console.log(`FRAME wasm-function[${found[1]}]:${found[2]}`);
  }
};

try {
  if (scenario === "explode") {
    withBytes(call("explode", 7, str("kaboom")), (p, n) => x.undra_call_sync(p, n));
  } else {
    withBytes(call("explode_detached", 8, str("task")), (p, n) => x.undra_call(p, n));
    for (let i = 0; i < 100 && schedules > 0; i++) {
      schedules = 0;
      x.undra_poll();
    }
  }
  console.log("NOTRAP");
} catch (error) {
  report(error);
}
