# ADR-056: The web call path: what a call costs in JavaScript, where it went, and the budgets that hold it

Status: **Accepted** (2026-10-02, piece `ts-size-e4`, E4 of the v1.x default-choice design, Amendment D; the
size half is ADR-052's amendment of the same date). Touches: `@undra/runtime`'s `UndraWriter` and `UndraReader`,
`UndraCore.call` and `callSync`, `WasmMainTransport`, two **optional** `Transport` methods (`sendCall`,
`callSyncParts`), the Vite plugin's `config` hook (a default `build.target`), the playground's Vite config, a new
kind of table in `bench/budgets.toml` (`[web."id"]`) and the device bench's web label. It does **not** touch the
wire (SPEC 3), the C or wasm ABI, the schema, the schema hash, the threading model, or any generated file: the
bindgen goldens did not move, the generated stores call `core.call(target, methodId, args, signal?)` and
`new UndraWriter()` exactly as before. Constitution R9 (budgets are tests) and R11 (decided before the code, here:
the runtime-model part is the two optional transport methods).

## Context

The device bench (`bench/RESULTS.md`, "Device numbers") measured the generated TypeScript call at **3.2 to 3.5 µs**
in Chromium on an M5 Pro (3.5 to 3.9 for the runtime's own `callSync`) against the blueprint's 80 ns in-thread
target, and the device bench review's cause analysis (`.10x/reviews/2026-10-01-device-bench-review.md`, "E4 seed")
put the wasm export itself at 130 to 200 ns, the build target at 70 to 80% of the rest, and the remainder in
allocation: the args writer (about 280 ns), the call header with a `BigInt` handle (about 310 ns), copies in and
out. The React Native review measured the same JavaScript under Hermes: `encodeCall` 4.1 µs, `callSync` about 6 µs,
an awaited generated method 15 to 20 µs, and 13 to 16 ms of every 16.7 ms frame at 100,000 keyed patches a second
(`docs/REACT_NATIVE.md`, "Limits").

## Decision

### 1. Where the time went (measured first)

Node 24 microbench of the playground's `Bench` through the generated binding, V8, the same wasm core, min of seven
rounds; Chromium 153 headless, the device bench's own page. Allocation was the cause, and two V8 facts decided how:

* A typed array of **at most 64 bytes** lives on V8's heap (`new Uint8Array(64)`: 62 ns with a copied result); one of
  65 bytes or more gets a backing store (`new Uint8Array(65)`: 280 ns; 256 bytes, the writer's old default, 274 ns).
* Anything that needs the array's `ArrayBuffer` of a small one (`subarray`, `.buffer`, `new DataView(a.buffer)`)
  *materialises* it: a 9-byte copy then `subarray(5)` is 266 ns, `slice(5)` 105; a `DataView` over a 50-byte payload,
  267 ns. A promise is 8 to 15 ns, so the promise machinery the brief named is not where the time is.

### 2. The levers, each its own commit

| # | Commit | Lever | What it removes |
|---|---|---|---|
| 1 | `b1aa0e5` | `UndraWriter` and `UndraReader` store and load integers up to 32 bits as bytes, make the `DataView` on first use, default to 64 bytes, copy a result of up to 64 bytes out (`finish`), write short strings ASCII-first and read them by a loop | a `DataView` and an `ArrayBuffer` per writer and per reader; `TextDecoder` for names and keys |
| 2 | `ae53a8d` | `encodeTarget` lays the `Call` header and the arguments out in one array; the handle's two halves come from a four-entry cache of recent handles; a reply's call id is read from its bytes | the `CallPayload` object, a second `UndraWriter`, three `BigInt` operations, a `DataView` per reply |
| 3 | `8e3ece9` | **the direct call**: on a transport that answers inside `send` (`wasm-main`) and a call without a signal, `UndraCore.call` registers a plain `DirectCall` in the pending map, sends, and returns an already settled promise; a call the core answers later gets its promise after `send` (before anything can reply: `undra_poll` runs from a microtask) | a `Promise`, its executor and three closures built before the send |
| 4 | `484b67b` | a small reply's body is `slice(5)`, not `subarray(5)` | the materialisation of the reply's buffer |
| 5 | `1e29d25` | **no `#private` on the call path**: the writer, reader, mirror, `UndraCore`, `WasmMainTransport`, `StreamCall`, `Signal` and `UndraObject` use TypeScript `private` members | a `WeakMap` or helper call per field access wherever a build lowers `#private` (Vite 6's default target, Babel under Hermes) |
| 6 | `4f08e67` | the Undra Vite plugin sets `build.target` to `es2022` when the app set none; the playground sets it | the same lowering, for Vite apps |
| 7 | `92b8b7c` | 64-bit and floating-point values go through one module-level scratch `DataView` | a `DataView` (and an `ArrayBuffer`) per reader or writer that met a `u64`: every change-set entry has a handle |
| 8 | `9fdaf86` | **`sendCall` and `callSyncParts`** (optional on `Transport`): the core writes the 17-byte header into one array it reuses and the transport copies header and arguments into wasm memory one after the other | the joined payload's allocation and copy (for a 1 KB argument, 1 KB of each) |

Node microbench, `await bench.benchAdd` through the generated binding, ns per call (min), on the loaded host (load 15
to 60) with the variants run alternately, so each row is comparable with the one beside it and not with another
column's host:

| | before | 1 | 1+2 | 1-3 | 1-4 | 1-7 | 1-8 |
|---|---|---|---|---|---|---|---|
| `await bench.benchAdd` (generated) | 1,693 | 1,395 | 997 | 808 | 659 | 507 | 462 |
| `UndraCore.callSync`, pre-encoded arguments | 854 | 877 | 630 | 524 | 393 | 326 | 291 |
| `await core.call`, pre-encoded arguments | 1,089 | 1,448 | 973 | 576 | 616 | 435 | 392 |

(Rows 5 and 6 do not move V8 with native fields, which is why this table has no column for them; the Chromium table
below does.)

### 3. Chromium, the device bench

`scripts/bench-device.sh --device web --runs 3`, headless Chromium 153, cross-origin isolated (5 µs clock), release
core, load 10 (the earlier files: load 2 to 4), p50:

| Row | before, committed 2026-10-01 (Vite default target) | after, the same page at the default target | after, `es2022` (what the plugin and the playground build) |
|---|---|---|---|
| `await bench.benchAdd` | 3.16 to 3.48 µs | 1.08 µs | **0.44 to 0.48 µs** |
| `UndraCore.callSync` | 3.55 to 3.89 µs | 0.70 µs | **0.29 to 0.32 µs** |
| 1 KB round trip | 4.57 to 5.30 µs | 2.94 µs | **1.21 to 1.50 µs** |
| keyed insert, 10,000 rows | 20.6 to 22.6 µs | 23.0 µs | **13.35 to 14.30 µs** |
| 100-signal change-set | 87.7 to 95.9 µs | 47.9 µs | **16.7 to 18.5 µs** |
| the merged 1,667-patch frame | 4.03 to 4.28 ms | 2.25 ms | **1.26 to 1.54 ms** |

(The middle column is one run at load 28 to 31, from before the last two levers; it is there for the target, not for
the host.) The call is 7x cheaper, the change-set 5x, and the call is now within 2.2x to 3.7x of the core's own
130 to 200 ns export plus its copies, not 20x. The blueprint's 80 ns is still out of reach in JavaScript; what is left
of a call is the writer, the 17-byte header, two wasm calls (`undra_call_sync`, `undra_buf_free`), the reply copy and
the `await`.

### 4. React Native (Hermes)

The same runtime code runs under Hermes, so what transfers is what is engine-independent: no `#private` on the call
path (Babel lowers it, and Hermes pays a helper call for each access), no `DataView` or `ArrayBuffer` per reader and
writer (a `DataView` operation is a native call in Hermes), fewer allocations per call and per change-set.
`@undra/react-native`'s own `NativeTransport` is not changed.

Measured on the React Native playground (`scripts/rn-device-checks.sh ios`'s app: iPhone 17 Pro simulator on the M5
Pro, Release, Hermes bytecode, a dedicated simulator; 21 of 21 of the app's own checks pass with both runtimes), the
app built twice and run alternately, two runs of each, host load 28 to 40 for both (not a quiet host, the same host):

| Row of the app's bench screen | the runtime of `1801951` | this piece |
|---|---|---|
| `encodeCall`, a new writer each call | 6.0 to 6.7 µs | 1.5 µs |
| `mirror.flush()` with nothing queued | 278 to 319 ns | 26 ns |
| `UndraCore.callSync` (`bench_add`) | 9.65 to 9.85 µs | **1.69 to 1.72 µs** |
| `await bench.benchAdd(1, 2)` | 24.6 to 25.3 µs | 7.66 to 7.75 µs |
| 1 KB round trip through `callSync` | 15.5 µs | 3.4 to 3.7 µs |
| 1 KB `await bench.benchEchoBytes` | 30.0 to 31.1 µs | 9.7 to 10.2 µs |
| 1,667 change-sets parsed as they arrive (`mirror.enqueue`) | 12.9 to 13.1 ms | **2.4 to 2.6 ms** |
| the frame's drain of them | 8.6 to 8.9 ms | **2.6 to 2.8 ms** |
| JSI host function floor; the call straight to the native core | 32 ns; 190 to 345 ns | unchanged |

At 100,000 keyed patches a second the mirror's work per 16.7 ms frame was 21.5 to 22.0 ms (it did not fit, as the
React Native review said, 13 to 16 ms on a less loaded host) and is **5.0 to 5.4 ms**: `docs/REACT_NATIVE.md`'s limit
is restated. The Android emulator was not measured again (it was slower than the simulator before, and the levers
are the same JavaScript); this is a simulator, not a phone.

### 5. The budgets are tests (R9)

`bench/budgets.toml` has a fifth kind of table, `[web."id"]` (`budget_ns`, `measured_ns`, `what`), read by
`bench/src/budget.rs` and, through `scripts/web-budgets.mjs`, by the two harnesses that measure the rows:

* the playground's device bench (`bench.spec.ts`) fails a full run whose p50 of `sync_call`, `sync_call_runtime`,
  `record_1kb`, `keyed_insert_10k`, `changeset_100` or the merged `drain_frame` is over its row;
* `call-path.test.ts` of the TypeScript runtime fails `npm test` when an awaited call (the generated code's writer,
  call and decode) or `callSync` over the WebAssembly stub core (whose answer costs nothing, so the runtime's own cost is
  what is timed) is over `node/call_async` or `node/call_sync`.

The rule is the file's own: five times what the reference host measured, rounded up to two significant figures
(2,200, 1,500, 6,100, 67,000, 84,000 ns and 6.3 ms; 1,600 and 800 ns). The rows the web bench had before this piece
(`sync_call` 3.2 to 5.5 µs, `callSync` 3.5 to 5.9 µs, the change-set 88 to 110 µs) are over their budgets. Five times
is what a shared runner needs, so these catch a call path that went back to an order of magnitude more allocation, not
a 1.5x drift (the Node stub measured 760 to 780 ns per awaited call and 340 to 360 per `callSync` on the code before
this series, against 312 and 159 now: inside the 1,600 and 800 budgets).

## Alternatives considered

* **A generated call builder** (`core.callWriter(handle, methodId)`, the arguments written after a reserved header,
  `core.invoke(w, signal)`), which would save the arguments' own writer for a small call and one copy for a large one.
  It changes every generated call (a generated shape: every golden, three generators' worth of review, an ADR of its
  own, and a minimum runtime version for new bindings) for what the optional transport methods already take for a 1 KB
  call (1,634 to 1,114 ns) and a few tens of nanoseconds of a small one. Not taken; the numbers are the evidence if it
  is wanted later.
* **An arena for what the core copies out** (small replies and change-sets as views into a bump-allocated 4 KB
  `ArrayBuffer`, so `subarray` and `DataView` find a buffer already made). It saved 130 ns of 238 in a microbench and
  measured 5 to 10% on the 1,667-patch burst, inside the noise of the host, at the price of views that keep a 4 KB arena
  alive. Not taken.
* **A fused patch decode and apply** (no operation objects per patch), **a leaner `Mirror.enqueue`**: the burst
  profile puts 54% of the time in wasm (the core's own transactions) and 25% in the JavaScript enqueue; neither is worth
  its generated-shape or SPEC cost. Not taken.
* **`#private` kept, with `es2022` documented.** It leaves Hermes, a Vite 6 app that sets its own target, and any bundler
  that lowers, with the 3x to 5x. Native private members are not faster than properties in V8, so the only cost of the
  change is bytes: +798 gzipped at the gated target (property names are not mangled), 0 at es2020, where the helpers
  disappear from the first chunk. Taken.
* **A call-path budget at 3x** instead of the file's 5x: it would catch the old call path on a quiet reference host and
  fail on a shared runner. The file's rule stands (one rule for every row); the device bench files keep the ratchet.

## Consequences

* The hello page's first chunk grew by **1,510 bytes gzipped** (7.7%) over the same tree without the call-path levers
  (ADR-052's amendment has the commit-by-commit sizes); the gate is restated there.
* A generated binding sees no change. A custom transport sees none either: `sendCall` and `callSyncParts` are optional,
  `UndraCore` uses them only where they exist (`WasmMainTransport`; the recovery wrapper, the worker, remote and native
  transports and test doubles do not have them, and their calls are what they were), and a transport that has them must
  copy both parts before it returns and keep neither (documented on `Transport` and in SPEC 17).
* The direct call changes **when** a call's promise exists, not when its change-sets apply: they are still queued ahead
  of the caller's continuation (read-your-writes, SPEC 11), and a call that takes a signal, an `onError` handler's own
  call, a closed core and every transport but `wasm-main` take the path they always did.
* `UndraWriter`'s default capacity is 64 bytes (was 256) and `finish()` returns an exact copy when the result is at most
  64 bytes (still an exact-length view above): both are inside the documented contract (the view's `ArrayBuffer` may be
  larger, never overwritten).
* The Vite plugin builds for `es2022` when the app sets no target. Chrome 94, Firefox 93 and Safari 16.4 are the
  floor of that syntax; an app that needs older browsers sets `build.target` itself, and the call path is then
  as fast as it now is at es2020 (1.08 µs, not 5.5).
* Private members of the runtime's classes are no longer enforced at run time, only by the compiler (the declaration
  files hide them): code that reaches `core._pending` is wrong, and now possible.

## Risks

* The budgets are 5x: they do not see a 2x regression. `bench/results/device/` and the microbench in this ADR are where
  that is seen; a future piece can add a baseline gate for the JavaScript rows the way `bench/baselines` does for the
  Rust ones.
* The 64-byte threshold is V8's (`typed_array_max_size_in_heap`); a build of V8 that moves it moves the constant's
  value, not the correctness, of every lever that depends on it.
* Hermes was measured through a React Native app on the iOS simulator (section 4), not on a phone, and the Android
  emulator was not measured again.
