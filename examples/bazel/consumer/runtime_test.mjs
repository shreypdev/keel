import { readFile } from "node:fs/promises";
import { UndraCallError } from "@undra/runtime";
import { Todos, UndraHelloCore } from "@bazel-hello/core";

const wasm = await WebAssembly.compile(await readFile(process.argv[2]));
const core = await UndraHelloCore.load({ mode: "wasm-main", wasm, shared: false });
const todos = await Todos.create();
await todos.add("Ship it");
const ok = typeof UndraCallError === "function" && todos.remaining.peek() === 1;
console.log(`runtime test: ${ok ? "passed" : "FAILED"}`);
core.close();
process.exit(ok ? 0 : 1);
