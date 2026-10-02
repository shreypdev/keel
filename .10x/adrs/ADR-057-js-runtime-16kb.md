# ADR-057: The JavaScript runtime's first chunk at 16 KB: where 22.1 KB is, what leaves it, and the gate

Status: **Proposed** (2026-10-02, piece `ts-runtime-16k`: design and measurement; the implementation is a later piece, from
the brief in `.10x/decisions/architect/ts-runtime-16k.md`). It would touch `@undra/runtime`'s module layout, its package
`exports` (a production and a development flavour), one optional field of `AttachOptions` (`features`), optional methods of
`Transport` (the control messages as calls), the TypeScript generator's entry for a schema that has a stream
(`features: [streams]`: a generated shape, R3 and R11), the JavaScript size gate (`scripts/web-size-runtime.mjs`,
`scripts/wasm-size.sh`, `bench/budgets.toml`) and SPEC 10.3, 12, 13, 14 and 17.1. It does **not** touch the wire, the C or wasm
ABI, the schema, the schema hash, the threading model, or any name SPEC 17.1 lists: every class, method and function there
keeps its name, its signature and its behaviour. Constitution R9 (budgets are tests), R11 (decided before the code), R3
(generated code), R6 (every error a typed value).

## Context

ADR-052's decision 2 asked for 16 KB of JavaScript runtime in what a hello-world page loads up front. `ts-size-e4` reached
21.2 KB and wrote that 16 KB "would mean removing behaviour"; five pieces then added 0.9 KB of required behaviour and the gate
is **22,100 bytes** with no room (`[size."web/hello-runtime-js"]`, record 22,100). The founder did not accept the conclusion
without the levers being measured. This ADR measures them. All numbers are the gate's own: the `undra init` template, the
runtime's pinned Vite 8.3.1 (Rolldown 1.2.11, oxc), production mode, the chunk of runtime modules the entry reaches by static
imports, zlib level 9 through Python (`scripts/web-size-runtime.mjs`; `main` at `a309e9f`: 71,052 bytes, 22,100 gzipped).

### Where the 22,100 bytes are

`scripts/web-size-attribute.mjs` (new with this ADR) builds the gate's chunk with a source map and charges every byte of the
minified chunk, and every bit of its deflate stream, to the declaration it was printed from: a literal costs its Huffman
code, a match its length and distance codes spread over the bytes it produces, the tables are spread in proportion, and the
shares add up to 22,100 exactly. A share is what code costs in this chunk, not what removing it saves (a match is charged
where it lands, not where it copies from): the levers below are measured by building, never read off this table.

| Concern (across modules) | min. bytes | gz | of which strings |
|---|---|---|---|
| wire: reader, writer, codecs, value types | 12,357 | 3,702 | 204 |
| mirror: queue, fold, drain, compaction, observe waiters, counters | 9,545 | 2,867 | 183 |
| transport: the in-process wasm host (`WasmMainTransport`) | 7,767 | 2,521 | 382 |
| error classes and messages (the `UndraError` family, the `UndraCallError` mapping) | 6,314 | 2,038 | 874 |
| call path (`call`, `callSync`, the direct call, the header, pending calls, replies) | 6,245 | 1,889 | 194 |
| ports: dispatch, the lazy default ports, port ids (fnv), host events (Connectivity, Lifecycle) | 4,871 | 1,834 | 264 |
| core lifecycle (`load`, options, start, the placeholder, close) | 6,234 | 1,805 | 361 |
| streams (`StreamCall`, stream items, credit, failures) | 3,646 | 945 | 96 |
| bundler: Vite's preload helper for dynamic imports (563) and the chunk's table of on-demand chunks (153) | 1,437 | 717 | 189 |
| mirror: keyed patches (`decodePatch`, `applyPatch`, `PatchError`) | 1,398 | 406 | 71 |
| system adapters (console log, clock, random, timers, the WebCrypto check) | 896 | 394 | 42 |
| store base class, and the chunk's `export { .. }` list | 771 | 334 | 0 |
| object identity (`adopt`, `collected`, the finalizer, the object base class) | 964 | 332 | 4 |
| signals (`Signal`, `batch`) | 850 | 320 | 0 |
| stats (`UndraStats`, the background and panic counters) | 904 | 299 | 9 |
| what only a core outside this thread needs, in `core.ts` (reconnect, connection-down, dev notice, timer port) | 1,195 | 250 | 41 |
| observe and release bookkeeping | 1,035 | 233 | 8 |
| snapshot, restore and the recovery hooks | 971 | 211 | 18 |
| panic-report trigger | 665 | 193 | 23 |
| error channel (`report`, `onError`, log) | 610 | 190 | 49 |
| wire: the framed transports' session payloads (`Hello`, `Log`, `PortCall`) | 566 | 172 | 0 |
| wire: control payloads encoded, then decoded again in process | 957 | 156 | 0 |
| background window (the run at `pagehide` and `freeze`) | 542 | 152 | 16 |
| namespace validation | 312 | 140 | 29 |
| **the chunk** | **71,052** | **22,100** | **3,056** |

Per module (gz): `core.ts` 5,141, `mirror.ts` 2,713, `transport/wasm-main.ts` 2,447, `wire/writer.ts` 1,125,
`wire/payloads.ts` 1,073, `wire/codec.ts` 1,010, `wire/reader.ts` 823, `call-error.ts` 761, `wire/types.ts` 716, `errors.ts`
664, `wire/errors.ts` 600, Vite's helper 563, `stream.ts` 512, `object.ts` 502, `adapters/browser-events.ts` 447,
`adapters/system.ts` 394, `signal.ts` 320, `port-dispatch.ts` 312, `adapters/ids.ts` 254, `adapters/events.ts` 253,
`adapters/default-ports.ts` 246, `wire/kind.ts` 200, `wire/session.ts` 172, `fnv.ts` 171, `identity.ts` 164.
**Strings: 615** (literals and the text parts of templates), **8,304 bytes, 3,056 gzipped**: the sentences of errors and
logs are about 1,900 of that, the rest names (`"UndraCallError.CancelledByCore"`, wasm export names, wire error codes, chunk
file names).

Four things this shows that `ts-size-e4`'s table (per module, estimated in proportion) could not:

1. **A hello page ships code no `wasm-main` hello page can run**: 945 bytes of streams (the template has none), 250 of
   reconnect, 172 of session payloads, the `Kind` enum and six control payloads that `UndraCore` encodes only for
   `WasmMainTransport.send` to decode them again in the same thread (156 + 255 + 200), the waiters behind `observe` on a
   core that answers later, 20 codecs out of 24 (`codecs` is one frozen object: `map` alone is 320), the name of every
   standard port and method with the hash function that turns them into ids at load (425).
2. **Where a module is emitted is decided by re-exports, not by use.** Rolldown emits a module in the first chunk when
   the entry reaches it statically through any module that has code of its own, even if only an on-demand chunk uses it;
   it emits the module with its user only when every static path to it is a pure re-export barrel. `wire/session.ts` is in
   the first chunk because `wire/payloads.ts` re-exports it (ADR-052's amendment thought it had left). This is the rule
   behind half of the levers below, and the gate should hold it (decision 5).
3. **The sentences are a tenth of the chunk** (the 874 above, plus those in other rows), and R8, which `ts-size-e4` cited
   for keeping them, is about macro diagnostics: nothing requires a production page to carry the prose of an error whose
   class, `kind`, fields and code it already has.
4. **717 bytes are the bundler's**, and they grow with every lever that makes something lazy: each on-demand chunk costs 15
   to 30 bytes of file name in the first chunk, twice when it is also an import site.

## Decision

### 1. The budget

**`[size."web/hello-runtime-js"]` is 16,000 bytes gzipped** (tolerance 5% over the record, as today), for what a hello-world
page loads up front of `@undra/runtime`: the runtime's own modules in the first chunk of a **production build as an app
installs the package** (sections 4 and 5 say what that changes in the measurement). The measured end of the plan is
**15,384** (record) with Vite's preload helper reported next to the number, and **15,958** with it inside the number: the
target holds under either reading, the margin (616 bytes) only under the first. Decision D6 at the end.

### 2. The levers, measured one after the other

Each row is a prototype commit of `wt/ts-runtime-16k` (reverted at the branch's tip; the runtime's own suite, without its
tests being adapted, still passes 1,435 of 1,555 on the last of them: what fails is where a test imports a moved name),
measured by the gate's build on that commit. gzip is not additive: a row's delta is what it saved on top of the rows above.

| # | Lever | Commit | gz after | Δ |
|---|---|---|---|---|
| | `main` `a309e9f` | | 22,100 | |
| 1 | the session payloads are re-exported by the wire barrel, not by `payloads.ts` | `fe4dfcc` | 21,971 | −129 |
| 2 | the nine port ids the first chunk needs are literals pinned by a test; `PortIds` and `fnv1a32` leave it; a lazy default port answers any method id | `fe4dfcc` | 21,765 | −206 |
| 3 | `codecs` is a module namespace (`export * as codecs`): a bundle keeps the codecs it names | `a4ac0e3` | 20,891 | −874 |
| 4 | stream support is a feature the generated entry passes when the schema has a stream; loaded on the first stream otherwise | `83005e6` | 20,144 | −747 |
| 5 | `UndraCore` speaks a typed channel (`observe(handle, id, on)`, `cancel(callId)`, ..) to the in-process host; `send(kind, payload)`, the payload codecs and `Kind` stay with the transports that frame | `cc5efe3` | 19,523 | −621 |
| 6 | the waiters behind `observe` load with a transport that answers later | `8828a07` | 19,310 | −213 |
| 7 | what only a core outside this thread needs (reconnect, connection-down, dev notice, its ports at start) loads with its transport | `07d8583` | 18,977 | −333 |
| 8 | `stats()`, `snapshot()`, `restore()` and `runInBackground()` are one module loaded on the first call; the page window reads `background.pending` itself | `ae63df5` | 18,634 | −343 |
| 9 | each on-demand transport maps `load()`'s options itself | `00faaef` | 18,390 | −244 |
| 10 | small ones: the sync-port refusal text with the worker transport, the per-export check a development check, one import site for the four of row 8 | `ee4bfa7` | 18,141 | −249 |
| 11 | pure barrels: the codecs a hello schema does not name (−96) and the two error classes only a mode throws (−159) are modules of their own | `ac8d8d0` | 17,886 | −255 |
| 12 | trims: internal members instead of an accessor object, a placeholder without stubs, a compact UUID codec, no import fallback in `whenObserved` | `2c681ef` | 17,678 | −208 |
| 13 | **the production flavour says codes**: a wire error's text is its code and values (−292), every other sentence is `T<code>` with its values and a link (−886); the development flavour keeps the sentences | `294fca2` + transform | 16,500 | −1,178 |
| 14 | **the published production flavour renames private properties** (`_pending` → `_a`), one cache for the package | transform | 15,958 | −542 |
| 15 | the gate reports Vite's preload helper next to the number (`bundler_gzipped`), not in it | build flag | **15,384** | −574 |

The same tree compiled by `tsc` to a `dist`, renamed by the publish-time pass and measured through `dist/index.js` (what a
registry app resolves): **15,980** at row 14 and **15,386** at row 15. Rows 13 and 14 measured alone on `main`: −1,426 and
−656 (−668 through `dist`, whose baseline is 22,134 because `tsc` and oxc lower TypeScript a little differently). Of the
798 bytes ADR-056 paid for `private _x` over `#x`, 656 to 668 come back; names of one letter would bring 114 more and are
not taken (below).

### 3. What each structural lever is (rows 1 to 12)

**The module rule (rows 1, 2, 11).** A module that only an on-demand chunk or an optional feature uses is re-exported from a
pure barrel (`index.ts`, `wire/index.ts`, a new `wire/codecs.ts`) and from nothing that has code of its own; a module of the
first chunk never imports it by value. `payloads.ts` is split along that line: what the first chunk runs (`CallTarget`,
`ReplyStatus`, `ChangeOp`, the change-set reader, `encodePortReply`, the patch reader and applier) stays; the framed
transports' codecs (`Call`/`Reply`/`StreamItem` decode and encode, observe, release, cancel, credit, event, timer, snapshot,
lazy pages, `encodePatch`) become `wire/framed-payloads.ts`, and with `kind.ts`, `session.ts` and `envelope.ts` one coarse
module where it can be (each separate on-demand chunk costs the first chunk a file name). `UndraRestoreError` and
`UndraSessionLostError` move to `errors-rare.ts`; the first chunk tells them by `kind` (`"restore"`, `"sessionLost"`), not by
`instanceof`. The first chunk's port ids are nine literals in `adapters/port-literals.ts`, each pinned to `PortIds` by a
unit test (R1: the name stays the source, the test is the derivation); a lazy default port no longer enumerates its
methods' ids up front, it forwards any method id to the port it loads.

**`codecs` (row 3).** `export const codecs = Object.freeze({ .. })` becomes `export * as codecs from "./codecs.js"`, one
`export const` per codec. Generated code is unchanged byte for byte (`codecs.vec(TodoCodec)`), and so is hand-written
code; a bundler resolves `codecs.vec` to the one binding and drops the rest. What changes: `codecs` is a module namespace
object, not a frozen plain object (`Object.isFrozen(codecs)` is false although assigning to it still throws), and a bundler
that does not follow a re-exported namespace keeps all 24 (today's size, never more).

**Streams as a feature (row 4, generated shape).** `AttachOptions.features?: readonly UndraFeature[]`, and
`export const streams: UndraFeature`. The generated entry of a schema that has a stream method or function passes it:

```ts
import { UndraCore, UndraError, streams, type AttachOptions, type LoadOptions, type Transport } from "@undra/runtime";
// ...
return started(UndraCore.load({ ...options, expectedSchemaHash: UndraIds.schemaHash, namespace: UndraIds.namespace, features: [streams] }));
```

`UndraCore.stream(target, methodId, args)` keeps its signature and its semantics: with the feature the stream opens as it
does today, synchronously when iteration starts; without it (a core loaded by hand with `UndraCore.load`, or bindings older
than this ADR) the first stream's iteration loads the module first (one chunk, 1.2 KB, once per page), so nothing that works
today stops working. A pending stream is an entry of the pending map that carries its own reply and failure handling, so
`UndraCore` has no stream code beyond `stream()` itself and one line that hands a stream item to the feature. `UndraCore`
stays feature-agnostic: a feature is `{ _install(core) }`, and the same hook is where later schema-driven features go (below:
callbacks' drain entries, lazy-list page calls, patch merging for a schema with no keyed list are each under 250 bytes and
not taken now). R3: a TypeScript engineer reads `features: [streams]` the way they read `plugins: [..]`.

**The typed channel (rows 5 to 7, runtime model).** `Transport` gains optional methods beside `sendCall` and `callSyncParts`
(ADR-056's): `observe(handle, signalId, on)`, `release(handle)`, `cancel(callId)`, `streamCredit(callId, credit)`,
`event(portId, methodId, payload)`, `timerFired(timerId)`, `portReply(reply)`. `UndraCore` calls only these. The in-process
host implements them straight onto the wasm exports (it decoded the payloads into exactly these arguments); a transport
that has only `send(kind, payload)` (remote, worker, React Native's, a test double) is wrapped once, in `attach`, by
`framed(transport)` from `transport/framed.ts`, which encodes each call into its `Kind` payload: the same bytes reach the
same `send` as today. The remote and worker transports import that module statically, so it arrives in their own fetch
wave; a custom transport's `attach` awaits one more small chunk. The same module carries what else only a core outside this
thread needs: the mirror's waiters (`UndraCore.observe` resolves at once on an in-process core; `Mirror.whenObserved` stays a
method and uses the installed waiters), reconnect and the releases held while the connection is down, the connection-down
test of `report`, the dev notice, the Diagnostics and Timer ports of a native core. `WasmMainTransport` keeps every member
it has (`send`, `snapshot`, `takeSnapshot`, `restore`, `twin`): it becomes a subclass of the host class `UndraCore.load`
runs, in a module of its own that the worker script, recovery and tests import, so a `wasm-main` page ships the host
without the framed `send`.

**On first use (rows 8 to 10).** `stats()`, `snapshot()`, `restore()` and `runInBackground()` are asynchronous already;
their bodies (and the host's snapshot and restore, as functions over the host) are one module, `core-extras.ts`, loaded by
the first call of any of them. The page's background window no longer builds an `UndraStats` to read one number: it
parses `background.pending` out of the core's own JSON, so the window at `pagehide` still needs no chunk when nothing is
pending (ADR-052's prod-ops review, item by item, is unchanged: the listeners, the window, the trap-report trigger stay up
front). `load()` keeps the `wasm-main` branch; the worker and remote transports each export a function that builds them
from `LoadOptions` (their checks and option mapping were 244 bytes of the first chunk).

**Every on-demand import fails typed (R6).** One helper wraps the runtime's `import()`s: a chunk that cannot be fetched
rejects the operation with `UndraTransportError("closed", .., { cause })` (a generated call sees
`UndraCallError.Unavailable`), is reported once through `onError` where there is no caller, and is tried again by the next
use, as the lazy default ports already do (ADR-052's amendment).

### 4. The production flavour (rows 13 and 14)

The package publishes two builds of the same sources and the same declarations:

```jsonc
"exports": { ".": { "types": "./dist/index.d.ts", "development": "./dist/dev/index.js", "react-native": "./dist/dev/index.js", "default": "./dist/index.js" }, /* every subpath the same way */ }
```

* **`dist/dev`** is today's output: sentences, every check, readable private names. Vite's dev server, Vitest, webpack in
  development mode and (through its own condition) React Native resolve it.
* **`dist`** is what a production build ships: Vite's `build`, webpack in production mode and anything that asks for
  neither condition (esbuild, Rollup unless told, plain Node) resolve `default`. It differs in exactly three
  ways, and a test holds each.

**(b) Messages are codes.** Every sentence the runtime throws or logs is `msg(17, operation, mode)`: a literal code and the
values. `messages.ts` (development) holds the table and formats today's sentence, character for character; the production
build swaps in `messages.prod.ts`, whose `msg` returns
`undra T0017: callSync, remote (https://shreypdev.github.io/undra/docs/errors.html#T0017)`. A wire error's production text
is its code and fields (`wire: code=unexpected_eof at=12 needed=3`). What is **synchronous and unchanged in both flavours**
is everything R6 names: the class (`instanceof`), `name`, `kind`, `code`, `detail`, `status`, `reason`, `operation`,
`cause`, and the core's own text (a panic message, a refusal reason travel as values). What a production page loses is the
prose, one click away. The T-codes are a second catalogue of SPEC 12 (never reused, generated into the same errors page);
the Swift and Kotlin runtimes keep their sentences, which no gate measures. React's production builds have worked this way
for a decade.

**(c) Development checks.** One today: the list of seventeen wasm exports a module must have, checked one by one to say
which is missing. Production asks whether `undra_abi_version` is there and lets the ABI version check say the rest (a
missing export then fails typed, as a trap of the first call that needs it). Range checks of the writer, the memory bounds
of the host, the namespace check and the mirror's guards are behaviour, not development aids, and stay in both.

**(a) Private properties are renamed.** After `tsc`, the publish build renames every property whose name starts with an
underscore (SPEC 17.1: "a name that starts with an underscore is not API") to `_a`, `_b`, .. through one cache for the
whole package (oxc's `mangleProps`, the minifier the pinned Vite already ships; 184 names today), except the four that are
the runtime's contract with generated code: `_set`, `_signals`, `_apply`, `_observeAll`. Properties stay plain properties,
so ADR-056's speed is kept on every engine. The names start with an underscore because a generated member never does
(SPEC 17.1), so a renamed runtime member cannot collide with a signal called `a`; bare single letters would save 114 bytes
more and could. What it means elsewhere:

* **Declarations**: unchanged for the public surface. A `private _pending;` line names a property the production flavour
  calls `_c`; nothing can reference it. `@internal` members that cross modules are stripped from the declarations
  (`stripInternal`), so the two flavours' types agree with their code.
* **Other packages**: `@undra/react-native`, `@undra/testkit`, the contract tests and every generated golden use no
  underscore member of a runtime object besides those four (checked by search; a test keeps it so). Should one ever need an
  internal, the cache ships as `@undra/runtime/mangle-cache.json` and their build applies it.
* **Debugging**: a production page shows `_c` in the inspector and the right source lines (the rename pass composes its map
  with `tsc`'s); a development build shows today's names. An app that aliases the checkout's sources (the examples) gets
  the development code whatever its mode.
* **Honesty of the gate**: the template is a Vite app, and `vite build` resolves `default`. The gate must therefore resolve
  the package the way an installed app does (decision 5), not alias a file.

### 5. The gate

* `scripts/web-size-runtime.mjs` builds the runtime package (`npm run build`), links it into the hello project's
  `node_modules` and lets Vite resolve `@undra/runtime` through `exports` with its default conditions, in production mode.
  A build that resolved the development flavour fails the run (the chunk contains a sentence of the table).
* The number is the chunk of the runtime's modules, as today. Vite's preload helper, a virtual module the build adds to
  whichever chunk has an `import()`, gets a chunk of its own and is reported as `bundler_gzipped` (691 bytes alone), like
  `lazy_gzipped`: it is not the runtime's code, an app with one `import()` of its own has it anyway, and a Vite upgrade that
  changes it should not fail a runtime gate (ADR-052's risk). The chunk's own table of on-demand chunks stays in the number.
* The record line lists the runtime modules of the first chunk; a run whose list differs from the committed record fails
  and names the module (fact 2 of the context: a re-export can move a module in without any import of it).
* **A second row, `web/all-features-runtime-js`**: the playground's bindings (stores, keyed, derived and lazy lists, queries
  and mutations, objects, callbacks, streams, ports) with crash recovery, a panic handler, `stats`, `snapshot`, `restore` and
  a background run, in `wasm-main` mode: its first chunk plus every on-demand runtime chunk except the Worker script.
  **42,385 bytes today** (32,472 up front + 9,913 on demand); **39,655 after** (27,371 + 12,284), 6.4% less. Its budget is
  today's number (42,400): the plan may move bytes out of the first chunk, it may not make the page that uses everything
  load more. With the structural levers alone it is 42,926 (+1.3%: the price of the chunk boundaries); rows 13 and 14 pay
  for that and more.
* `bench/budgets.toml`: `[size."web/hello-runtime-js"]` `budget_gzip_bytes = 16000`, the record, `tolerance = 0.05`;
  `[size."web/all-features-runtime-js"]` `budget_gzip_bytes = 42400`, the record, `tolerance = 0.05`.

### 6. What a page loads over its life, and what it costs in time

A hello page that calls a default port loads **21,378** bytes of runtime over its life (15,960 + 2,444 + 2,974), against
27,015 today. `lazy_gzipped` (every on-demand chunk and the Worker script, which no one page loads) goes from 24,303 to
28,031. No lever adds a round trip to `load` in any built-in mode; the first call of `stats`, `snapshot`, `restore` or
`runInBackground` loads one chunk (857 bytes); a core loaded without its generated entry loads the stream support at its
first stream. The call path's code is the same code: `call`, `callSync`, the direct call, the writer and the reader are not
touched, and on the last prototype the two Node rows measured 340 and 170 ns against `main`'s 333 and 162 (the best of three
alternating runs each, on one host at load 52; budgets 1,600 and 800, `[web."node/.."]`). Each lever lands with the `[web."id"]` rows green.

## Levers measured and not taken

| Lever | Worth | Why not |
|---|---|---|
| Host events by use: the Connectivity and Lifecycle sources, their adapters and encoders out of the first chunk | −530 (ablation, on top of row 14) | A hello core listens to neither, but nothing tells the host so: loading them after `load` regardless would move the bytes past the gate, not off the page. It needs the core to say when it starts listening (an import or a stats field): an ABI decision, its own ADR. **This is the next lever.** |
| `UndraWriter` / `UndraReader` numeric methods a schema does not use (`writeI16`, `readF64`, ..) | about −380 (attribution) | Methods of a class cannot be dropped; free functions would change every generated codec (`writeI16(w, v)`), which fails R3. |
| The mirror's compaction on first use | about −250 | The backlog bound of ADR-031 would hold only once a chunk arrived, and not at all offline. |
| Keyed-patch merging loaded on the first keyed entry | 0 for the gate, about −250 for a schema without keyed lists | The hello template has a keyed list. And it could not wait for a chunk: read-your-writes drains are synchronous (`callSync`, `observe`, the microtask before a reply's continuation), so it would need an unmerged path beside the merged one. |
| Object identity on the first `create()` | −132 | ADR-052's note measured and rejected it (every app's first screen waits for it). |
| Renamed names of one letter (`a`, `b`) | −114 more | A generated signal may be called `a`. Uppercase-first names measured no better than `_a` (−4). |
| Renaming internal properties that do not start with an underscore | −43 (fourteen names) | Not worth a second naming rule. |
| One table-driven reader, writer or codec | −19 (`ts-size-e4`) | gzip already folds the repetition; a string-keyed method lookup would slow the call path. Superseded by row 3. |
| `UndraCallError.mapped` on first failure | about −750 | Generated code throws its result synchronously; the mapping is R6's own code. |
| `build.modulePreload: false` | −193 | An app's own setting; the helper stays for CSS. |
| A pre-bundled `dist` with designed chunks | not measured | It would trade the per-module tree-shaking the levers above rely on. |
| TypeScript enums as plain objects | about −150 | Changes their declared types (`ts-size-e4` rejected it for that; still true). |

## Alternatives considered

* **Stop at 21 or 22 KB and call the rest required behaviour** (ADR-052's amendment). Two thirds of what leaves the chunk
  here (rows 1 to 12: 4,422 of 6,716 bytes) is code a `wasm-main` hello page cannot run or does not run until asked; none
  of it is removed from the package.
* **`process.env.NODE_ENV` checks instead of an export condition.** Works in fewer places without a bundler (a browser
  loading the package from a CDN has no `process`), and leaves the choice of flavour to each call site instead of to one
  module swap and one rename pass. The condition is what Lit, Solid and Vue publish.
* **A subpath import (`#messages`) with conditions, and one `dist`.** One copy of everything but one module, which is
  attractive; but a development build would then carry renamed names (or none would), and the sources (`files` ships `src`)
  could not resolve the same specifier to TypeScript and to JavaScript. Two directories from one script are simpler to
  verify.
* **Renaming in the Undra Vite plugin instead of in the published `dist`.** Only Vite apps would get it.
* **A TypeScript transformer instead of oxc for the rename.** Exact source maps for free, but it is our code to keep
  complete; oxc's pass is the pinned toolchain's and its map composes with `tsc`'s. The dist-run of the suite catches a
  missed rename either way.
* **Free functions instead of features** (`openStream(core, ..)`): `UndraCore.stream` is in SPEC 17.1 and stays.

## Consequences

* The first chunk goes from 22,100 to about 15.4 KB as the gate will count it (16.0 KB counted as today). The development
  flavour's first chunk is about 17.8 KB, ungated.
* Generated code changes in one place: the entry of a schema that has a stream gains `streams` in an import and
  `features: [streams]` in `load` and `attach` (and `"features"` in the options it omits); the goldens of such schemas move.
  Bindings generated before this ADR keep working on a runtime after it (the stream support loads on first use); bindings
  generated after it need a runtime that exports `streams`. No schema hash moves.
* `Transport` has seven more optional methods; a custom transport sees no change (`attach` adapts it). `PortImpl`, `Mirror`
  and every error class keep their public members. `codecs` is a namespace object.
* The package's `dist` is no longer what `tsc` wrote: `npm run build` is a script (compile, swap one module, rename, compose
  maps), and the suite runs twice in CI, on the sources and on the production `dist` (the wasm harness and the contract
  column already take a `dist` path).
* A production page's errors say `undra T0017: ..` and link to the errors page, which has a T section generated from the
  development table. Logs, crash reports and `onError` carry the same codes with their typed fields.
* More, smaller on-demand chunks: 13 files in a hello build (10 once the framed modules are one) instead of 8. An app that precaches (a service worker)
  lists them like the others.

## Risks

* **Another bundler follows the rules less well.** The numbers are Rolldown's. webpack and esbuild resolve the `default`
  flavour too; a bundler that does not follow a re-exported namespace keeps every codec, and one that places modules
  differently may emit more in its first chunk. Nothing breaks; the saving is smaller. The gate measures the template's
  bundler, as it always has.
* **Two flavours drift.** They differ by one module, one rename pass and one check; the suite runs on both; a test compares
  the exported names of every entry of the two.
* **A property is reached by a string** (`obj["_x"]`, `"_x" in obj`) and the rename misses it: the build fails on any string
  literal equal to a renamed name, and the dist-run of the suite exercises the result.
* **The prototype numbers are prototypes.** Tests will ask for code the prototypes did not write; 616 bytes of margin is for
  that. If the helper is counted (D6), there is none, and the first chunk needs the next lever to have room.

## Decisions needed (the integrator)

* **D1.** Production messages are a code, the values and a link; the sentences live in the development flavour and on the
  errors page (row 13: −1,178). Without it the plan ends at about 16.6 KB (helper apart) or 17.1 KB.
* **D2.** The published `dist` is the production flavour with private properties renamed; `development` and `react-native`
  resolve the readable build (row 14: −542).
* **D3.** `Transport` gains the typed control methods, and `WasmMainTransport` becomes a subclass of the host class the core
  runs (rows 5 to 7: −1,167).
* **D4.** `AttachOptions.features` and the generated `features: [streams]` (row 4: −747; a generated shape).
* **D5.** `stats()`, `snapshot()`, `restore()` and `runInBackground()` load a chunk on their first call (row 8: −343).
* **D6.** The gate reports Vite's preload helper beside the number (15,384, 616 bytes under 16,000) or keeps it inside
  (15,958, 42 under). Recommended: beside, with both printed.
* **D7.** The all-features row's budget: today's 42,400, or informational only.
* **D8.** The code family (`T0001`..) and its page (`docs/errors.html`, a second table), or another scheme.
* **D9.** `codecs` becomes a module namespace object instead of a frozen plain object (row 3: −874; generated code and its
  call sites unchanged).
