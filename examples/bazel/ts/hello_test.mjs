// The TypeScript consumer of the Bazel example (ADR-061): the generated bindings, compiled by `undra_ts_library` against the
// compiled runtime, load the wasm core `undra_core` built for the web and call it. Every check prints one `bazel ts:` line; any
// failure exits 1, which is what makes `bazel test //ts:hello_test` fail.
import { readFile } from "node:fs/promises";
import { Todos, TodoError, UndraHelloCore, greeting } from "@bazel-hello/core";

let failed = false;
function check(what, ok) {
  console.log(`bazel ts: ${ok ? "ok  " : "FAIL"} ${what}`);
  if (!ok) failed = true;
}

const wasm = await WebAssembly.compile(await readFile(process.argv[2]));
const core = await UndraHelloCore.load({ mode: "wasm-main", wasm, shared: false });
check(`the core loaded: ${UndraHelloCore.namespace}`, UndraHelloCore.core === core);

const hello = await greeting("Bazel");
check(`a function crosses the boundary: "${hello}"`, hello === "Hello, Bazel, from the bazel-hello core");

const todos = await Todos.create();
await todos.add("Write the rules");
await todos.add("Run bazel test");
check(`a store's signals mirror the core: 2 items remain, the mirror says ${todos.remaining.peek()}`, todos.remaining.peek() === 2);

const error = await todos.add("   ").then(
  () => undefined,
  (e) => e,
);
check(`a typed error from Rust arrives as a TypeScript class: ${error?.kind}`, error instanceof TodoError.EmptyTitle);

await todos.toggle(todos.todos.peek()[0].id);
check(`a command changes the signals the UI observes: ${todos.remaining.peek()} remaining`, todos.remaining.peek() === 1);

core.close();
console.log(`bazel ts: ${failed ? "FAILED" : "passed"}`);
process.exit(failed ? 1 : 0);
