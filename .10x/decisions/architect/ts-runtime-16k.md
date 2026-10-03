# Architect: the JavaScript runtime's first chunk from 22.1 KB to 16 KB (`wt/ts-runtime-16k`)

ADR: `.10x/adrs/ADR-057-js-runtime-16kb.md` (**Proposed**; nine decisions wait for the integrator, at its end). This piece
is design and measurement: nothing of the runtime changes on this branch. What lands: the ADR, this record, one tool
(`scripts/web-size-attribute.mjs` with its rules, `scripts/web-size-concerns.mjs`) and one switch of the gate's build script
(`UNDRA_SIZE_SOURCEMAP=1` writes source maps beside the chunks; the chunks are byte for byte the same, 71,052 / 22,100 with
and without). Thirteen prototype commits (`fe4dfcc`..`294fca2`) measured the levers and are reverted by `aaf5844`; they stay
in the branch's history for the implementer.

## What was measured, and how

* **The gate, reproduced**: `scripts/wasm-size.sh` on `a309e9f`: `web/hello-runtime-js` 71,052 bytes, **22,100** gzipped
  (budget 22,100); `web/hello-wasm` 116,471. Every JavaScript number below is that build (the template of `undra init`,
  the runtime's pinned Vite 8.3.1 / Rolldown 1.2.11, production mode, the `$initial` runtime chunk, zlib 9 through Python;
  Node's own zlib says 22,121 for the same chunk, so the tools shell out to Python like the gate).
* **Attribution**: `node scripts/web-size-attribute.mjs --symbols --strings` (after `scripts/wasm-size.sh` made the hello
  project). Per byte: the source map says which declaration printed it; the deflate stream, decoded symbol by symbol, says
  what it cost; the costs add up to the compressed size or the script throws. The ADR has the table per concern (24 rows)
  and per module; strings are 615, 8,304 bytes, 3,056 gzipped. Top of the per-declaration list: Vite's preload helper 563,
  `UndraCore.load` 504, the wire errors' `describe` 454, `WasmMainTransport.start` 396, `codecs.map` 320, `Mirror._fold` 295,
  `UndraCore._start` 280, `Mirror.flush` 277, `UndraCore.stats` 266, the host's `_imports` 259 and `send` 255.
* **Levers**: each a prototype commit, built and measured by the gate's build (`runtimes/ts/@undra/runtime/proto/measure.mjs`
  in `294fca2` is the gate's build with switches; `proto-plugins.mjs` reproduces the two publish-time passes inside it;
  `mangle-dist.mjs` is the rename pass over a real `tsc` output). The last state was also measured through `tsc` + the
  rename pass + `dist/index.js`: 15,980 and 15,386 against 15,958 and 15,384 through the sources.
* **The call path**: the stub-core bench of `call-path.test.ts`, printed, `main` and the last prototype alternating three
  times at host load 52: awaited call 333 / 355 / 355 ns (`main`) against 340 / 427 / 365; `callSync` 162 / 167 / 163 against
  170 / 171 / 183; budgets 1,600 and 800. No lever touches the call path's code; the implementer repeats this on a quiet host.
* **The all-features page**: the playground's generated bindings with recovery, a panic handler, `stats`, `snapshot`,
  `restore` and a background run (`proto/all-features-entry.ts`): 32,472 up front + 9,913 on demand = **42,385** on `main`;
  27,371 + 12,284 = **39,655** on the last prototype (the Worker script, apart: 12,537 and 11,197).

## The levers

Bytes are gzipped, measured in this order, each on top of the rows above. "Shape" is whether a public or generated shape
changes (so: an ADR decision). No row changes what the `[web."id"]` rows measure; R6 is "every error a typed value".

| # | Lever | Δ | after | How measured | Risk | Touches | Shape | R6 |
|---|---|---|---|---|---|---|---|---|
| 1 | session payloads re-exported by the barrel only | −129 | 21,971 | `fe4dfcc` | none | `wire/payloads.ts`, `wire/index.ts` | no | same |
| 2 | nine literal port ids up front; lazy ports forward any method id | −206 | 21,765 | `fe4dfcc` | low: a literal drifts from its name (a test pins each) | `adapters/{port-literals,default-ports,events}.ts`, `core.ts` | no | same |
| 3 | `codecs` a module namespace | −874 | 20,891 | `a4ac0e3` | low: `codecs` is no longer a frozen plain object | `wire/codec.ts`, new `wire/codecs*.ts` | the type of `codecs` (D9) | same |
| 4 | streams a feature of the generated entry | −747 | 20,144 | `83005e6` | medium: a generated shape; the fallback's first stream waits for a chunk | `core.ts`, new `stream-support.ts`, `ts.rs`, goldens | **yes** (D4) | a failed chunk fails the stream typed |
| 5 | typed channel; host without `send`; payloads split | −621 | 19,523 | `cc5efe3` | medium: the recovery wrapper and every transport double meet a new path | `transport/*`, `core.ts`, `wire/*`, `recovery.ts`, `worker.ts` | optional `Transport` methods (D3) | same |
| 6 | observe waiters with the transports that answer later | −213 | 19,310 | `8828a07` | low | `mirror.ts`, `transport/framed.ts`, `core.ts` | no | same |
| 7 | outside-thread core code with those transports | −333 | 18,977 | `07d8583` | medium: ordering at start (the Timer and Diagnostics ports before the first message) | `core.ts`, `transport/framed.ts` | no | same |
| 8 | `stats`, `snapshot`, `restore`, `runInBackground` on first call | −343 | 18,634 | `ae63df5` | low: first call waits for a chunk | `core.ts`, new `core-extras.ts`, `transport/wasm-snapshot.ts`, `recovery.ts` | no (D5) | a failed chunk rejects typed |
| 9 | each lazy transport maps `load()`'s options | −244 | 18,390 | `00faaef` | low | `core.ts`, `transport/{wasm-worker,remote,wasm-main}.ts` | no | same errors, thrown later in `load` |
| 10 | refusal text with the worker; per-export check a development check; one import site | −249 | 18,141 | `ee4bfa7` | low | `port-dispatch.ts`, `transport/wasm-*.ts`, `core.ts` | no | a missing export fails as a trap in production |
| 11 | pure barrels: rare codecs (−96), rare error classes (−159) | −255 | 17,886 | `ac8d8d0` | low | `wire/codecs*.ts`, new `errors-rare.ts`, `call-error.ts` | no | same classes, told by `kind` |
| 12 | trims (internal members, placeholder, UUID codec, no waiter fallback) | −208 | 17,678 | `2c681ef` | low; the UUID codec only if not slower | `core.ts`, `wire/types.ts`, `mirror.ts` | no | same |
| 13 | production flavour: messages are codes (wire text −292, the rest −886) | −1,178 | 16,500 | `294fca2` + the `messages()` transform (every string or template with two or more spaces becomes `__m(n, ..values)`: 219 sites in the package) | medium: two flavours; production text changes | every module that throws or logs, new `messages*.ts`, the build, `package.json` | production `message` text (D1) | classes, `kind`, fields identical |
| 14 | production flavour: private properties renamed | −542 | 15,958 | the `mangle()` transform over the sources, and `mangle-dist.mjs` over `tsc`'s output (15,980) | medium: the published code is no longer `tsc`'s | the build, `package.json`, `tsconfig.build.json` | no (D2) | same |
| 15 | the gate reports Vite's helper beside the number | −574 | **15,384** | the gate's build with the helper in a chunk of its own | none | `scripts/web-size-runtime.mjs`, `scripts/wasm-size.sh` | the gate (D6) | n/a |

Alone on `main`: row 13 is −1,426, row 14 −656 (−668 through `dist`). Not taken, with numbers, in the ADR ("Levers measured
and not taken"): host events by use (−530, needs the core to say it listens: the next lever), numeric reader/writer
methods (about −380, R3), compaction on first use (about −250, ADR-031's bound), one-letter names (−114, collision with
generated names), a second codecs module without the barrel rule (0: measured, which is how the module rule was found).

**The path**: 22,100 → 17,678 by structure (rows 1 to 12, none removes behaviour) → 16,500 with production messages →
15,958 with renamed private names → 15,384 as the gate would count it. Counted as today the plan ends 42 bytes under
16,000, which is no margin: the margin is D6, or the next lever (host events by use) with its ABI decision.

## Implementation brief (for the piece that builds it)

Read first: ADR-057, ADR-052 with its amendments, ADR-056, SPEC 17.1, `test/up-front.test.ts`, and the prototype of each
step (`git show <sha>`; the prototypes do not maintain tests and take shortcuts named below). One commit per step, in this
order; each commit's message carries the gate's number before and after and re-records it (`scripts/wasm-size.sh --record`;
the budget in `bench/budgets.toml` stays 22,100 until step 15 sets 16,000, the 5% over the record is the ratchet meanwhile).
**Green at every step**: in `runtimes/ts/@undra/runtime`, `npm test` (it includes `call-path.test.ts`) and
`npm run typecheck`; `contract-tests/ts/run.sh` (the TypeScript column); `crates/undra-ffi/tests/wasm/run.sh`;
`runtimes/rn/@undra/react-native` and `runtimes/ts/@undra/testkit` tests and typechecks; `cargo test -p undra-bindgen` and
`undra bindgen --check --docs` on the examples when a golden moves; `examples/playground/web` tests and build. Do not rename
or remove anything SPEC 17.1 lists. Update the SPEC in the commit that changes what it says (17.1, 10.3, 12, 13, 14).

0. **Guard first.** `test/up-front.test.ts` learns the module rule: besides "`core.ts` must not statically reach X", build
   nothing, but assert for each module that must stay out (`wire/session`, `wire/kind`, `wire/envelope`, and each new one
   below) that no module with code of its own re-exports it. The gate gets the stronger check in step 15.
1. **Row 1.** Move the `export { .. } from "./session.js"` block from `wire/payloads.ts` to `wire/index.ts`.
2. **Row 2.** `adapters/port-literals.ts`: `HTTP_PORT`, `KV_PORT`, `SECURE_STORE_PORT`, `FS_PORT`, `CONNECTIVITY_PORT`,
   `CONNECTIVITY_CHANGED`, `LIFECYCLE_PORT`, `LIFECYCLE_CHANGED`, `TIMER_PORT` as hex literals; `port-literals.test.ts` pins
   each to `PortIds`. `default-ports.ts`, `events.ts` and `core.ts` use them; no module of the first chunk imports
   `adapters/ids.ts`. `lazyPort` forwards any method id to the loaded port (the prototype uses a `Proxy` for `methods`; a
   method the port lacks answers as today). Tests: `default-ports.test.ts` (a method id never listed up front reaches the
   real port; ordering per port as before).
3. **Rows 3 and 11a.** `wire/codec.ts` keeps `Codec`, `encodeValue`, `decodeValue`, `WireResult` and ends with
   `export * as codecs from "./codecs.js"`; `wire/codecs.ts` is a pure barrel over `codecs-core.ts` (bool, u8, u16, u32,
   u64, string, uuid, unit, handle, vec) and `codecs-more.ts` (the other fourteen). Tests: the 24 names and their types are
   what they were (`codec.test.ts`, `package.test.ts`); replace any "is frozen" assertion by "assignment throws".
4. **Row 4.** `AttachOptions.features?: readonly UndraFeature[]`, `export interface UndraFeature`, `export const streams`
   (`stream-support.ts`: `open` and `item`, from `_openStream` and `_onStreamItem`). A stream's pending entry carries
   `reply(status, body)` and `reject(error)`; `_onReply` hands a stream's reply to its entry before anything else;
   `_failInFlight` calls `reject` on every entry. `UndraCore.stream` uses the installed support, else loads the module on
   the first iteration (through the typed import helper of step 8). Generator: `crates/undra-bindgen/src/ts.rs`, the entry
   (the two `UndraCore.load(` / `UndraCore.attach(` lines): for a schema with any stream method or function, import
   `streams`, pass `features: [streams]`, and omit `"features"` from the two option types; goldens `callbacks`, `decimal`,
   `full`, `newtypes`, `object_graph`, `objects`, `stdlib`, `stores` move, others must not. Tests: every stream test runs
   with the feature and without it; with it, the `Call` is sent synchronously when iteration starts (as today); without
   it, a second stream opened while the module loads is opened in order; a failed chunk fails the stream
   `UndraTransportError` (a generated stream: `Unavailable`) and the next stream tries again; `stats().openStreams` counts.
5. **Row 5.** `transport/transport.ts`: the seven optional methods on `Transport` (documented as ADR-056 documented
   `sendCall`: a transport that has them must copy what it is given before it returns). `transport/wasm-main.ts`: the host
   class (the prototype names it `WasmHost`) with the typed methods and without `send`; `transport/wasm-main-transport.ts`:
   `WasmMainTransport extends` it and adds `send(kind, payload)` exactly as today (and, after step 8, `snapshot`,
   `takeSnapshot`, `restore`, `twin` as methods over the functions); `index.ts` and `worker.ts` import it from there.
   `transport/framed.ts`: `framed(transport)` returns a wrapper (do not assign onto the transport, as the prototype does)
   whose typed methods encode and call `send`; `UndraCore._attach` wraps a transport that lacks `observe`. `core.ts` calls
   only typed methods; `encodeTarget` writes the page call's 21 bytes and `encodeConstructor` the constructor's header, so
   `encodeCall` leaves the first chunk. Split `wire/payloads.ts` (the prototype's `proto/split.mjs` call lists the names):
   the first chunk keeps `CallTarget`, `ReplyStatus`, `ChangeOp`, `ChangeEntry`, `decodeChangeSet`, `PortStatus`,
   `encodePortReply`, `PatchOp`, `decodePatch`, `applyPatch`, `ALL_SIGNALS`; the rest is `wire/framed-payloads.ts`, and
   `kind.ts`, `session.ts`, `envelope.ts` join it in one module if their public re-exports allow (each separate on-demand
   chunk costs the first chunk about 20 bytes). **`recovery.ts`**: its wrapper intercepts `send` today (a `Release` during
   a restart is remembered, a `PortReply` of the old instance dropped, anything else refused): it must implement the typed
   methods with the same rules, and forward `sendCall` / `callSyncParts`. Tests: `framed.test.ts` (each typed method yields
   the bytes `main`'s encoder yields, against `contract-tests/wire-vectors.json` where it has the payload); a transport
   with only `send` attaches and passes the core suite; `recovery.test.ts`, S21, S22 and the React Native contract
   column unchanged.
6. **Row 6.** `Mirror` keeps `whenObserved` and `failWaiters` as methods over an installable `MirrorWaiters`
   (`applied(handle, signalId)`, `drained()`, `gone(handle)`, `when(..)`, `fail(error)`, `waiting`); `mirrorWaiters(mirror)`
   lives in the framed module and `_attach` installs it for a transport that is not synchronous. A `Mirror` used on its own
   (tests) installs it through an exported helper; `whenObserved` without waiters throws `UndraError("state")`.
7. **Row 7.** The extension object of the framed module (`starting`, `down`, `held`, `logged`, `reconnecting`,
   `reconnected`): `_start` awaits `starting` before `transport.start` (the Diagnostics port and, for an explicit Timer
   adapter on `remote`, the Timer port must be registered before the first message after the Hello:
   `timer-port-start.test.ts` holds it) and the worker's ignored-adapters warning moves there. The remote and worker
   transports import the framed module statically, so no mode gains a round trip.
8. **Rows 8 and 10c.** One helper for every `import()` of the runtime: it maps a rejection to
   `UndraTransportError("closed", .., { cause })` and lets the next use try again. `core-extras.ts`: `stats`, `snapshot`,
   `restore`, `runInBackground` (`background.ts` folds in); `transport/wasm-snapshot.ts`: `takeSnapshot(host)`,
   `restoreInto(host, bytes)`, `twin(host)`, which recovery uses. `_backgroundWindow` reads
   `JSON.parse(await transport.stats()).background?.pending`. Tests: a hide with nothing pending imports nothing; with
   something pending it runs; `snapshot.test.ts` and `recovery.test.ts` unchanged; a failed chunk rejects typed.
9. **Row 9.** `workerTransport(options, recovery)` and `remoteTransport(options)` exported by their modules, with the
   checks `load` made for them (the same errors, in the same order relative to anything observable).
10. **Row 10a.** `syncPortRefusal` moves to `transport/wasm-worker.ts`. (The per-export check waits for step 13.)
11. **Row 11b.** `errors-rare.ts` holds `UndraRestoreError` and `UndraSessionLostError`, re-exported by `index.ts` only;
    `call-error.ts` and `core.ts` tell them by `kind`.
12. **Row 12.** `@internal` members where the prototype used an accessor object; the placeholder's transport is the
    minimum a closed core touches; the UUID codec of `2c681ef` **only if** an alternating bench on a quiet host shows it
    no slower than today's (on a loaded host: encode 47 to 77 ns against 42 to 51, decode 79 to 88 against 80 to 83:
    inconclusive); otherwise keep today's loops and merge their four messages into one.
13. **Row 13 (D1, D8).** `messages.ts`: `msg(code, ...values)` over a table `{ code: template }` whose templates are
    today's sentences with `{0}`, `{1}`; every sentence the runtime throws or logs becomes a `msg(<literal>, ..)` call
    (literal numbers at the call sites, a comment naming them; a test asserts every literal has a row, no code is used
    for two sentences, and no code is ever reused). `messages.prod.ts`: the same exports, the production text. The wire
    errors' `describe` is the development table's; production prints the code and fields. The per-export check of the wasm
    host goes through a development-only hook of the same module. The build (`scripts/build.mjs` of the package, behind
    `npm run build`): `tsc` into `dist/dev` (with `stripInternal`), copy to `dist`, swap `messages.js`, copy the
    declarations. `package.json`: every subpath `{ types, development, "react-native", default }`. SPEC 12 gets the T
    table; `site/scripts/build-errors.mjs` renders it. Tests: the whole suite on the sources (sentences unchanged, so
    message assertions hold); a flavour test that for a sample of every error class the production and development
    builds throw the same class, `name`, `kind` and fields and differ only in `message`; `package.test.ts` compares the
    exported names of every entry of the two flavours.
14. **Row 14 (D2).** The build's rename pass (start from `proto/mangle-dist.mjs`): oxc `minifySync` per file with
    `compress: false, mangle: false, mangleProps: { include: /^_[A-Za-z]/, reserved: ["_set", "_signals", "_apply",
    "_observeAll"], cache }`, names `_a`, `_b`, .. by frequency, the cache written to `dist/mangle-cache.json` (exported);
    maps composed with `tsc`'s (`@jridgewell/remapping`; `rolldown` and it become explicit devDependencies, both already
    in the lockfile through Vite). The build fails on a string literal equal to a renamed name. CI runs the suite a second
    time against `dist` (`UNDRA_TEST_DIST=1`: a Vitest alias from `../src/` to `../dist/`; the three tests that read `_era`
    or `_giveBack` skip there), and `crates/undra-ffi/tests/wasm/run.sh` and the contract column with `UNDRA_TS_DIST`
    pointing at it. A test in `@undra/react-native` and `@undra/testkit` asserts they name no underscore member of a
    runtime object besides the four.
15. **Row 15 and the gate (D6, D7).** `scripts/web-size-runtime.mjs`: build the package, link it into
    `<project>/web/node_modules/@undra/runtime`, drop the alias, match the runtime's group on its `dist`, give
    `vite/preload-helper` a group of its own, print `bundler` and the first chunk's module list; fail when the chunk holds
    a development sentence. `scripts/wasm-size.sh`: `bundler_gzipped` and `modules` on the JavaScript line (a list that
    differs from the committed record fails, naming the module); a third line `web/all-features-runtime-js` from the
    playground's bindings and a committed entry (`proto/all-features-entry.ts` is the draft), its value the first chunk
    plus every on-demand runtime chunk but the Worker script. `bench/budgets.toml`: 16,000 and 42,400 with their records
    and 5%. `bench.yml`, the README's sentence and `site/scripts/build-numbers.mjs` follow. Update ADR-057's status and
    numbers to what was built.

**Acceptance.** `scripts/wasm-size.sh` exits 0 with `web/hello-runtime-js` at most 16,000 (and within 5% of its record),
`web/all-features-runtime-js` at most 42,400, `web/hello-wasm` as it was; `npm test` green on the sources and on `dist`;
the contract grid, the wasm harness, the React Native column; `scripts/bench-device.sh --device web --runs 3` within every
`[web."id"]` row; `undra bindgen --check --docs` clean on every example.

## Decisions the integrator owes (ADR-057, end)

D1 production messages as codes; D2 the renamed production `dist` behind export conditions; D3 the typed `Transport`
methods and `WasmMainTransport` as a subclass of the host; D4 `features` and the generated `features: [streams]`; D5 `stats`,
`snapshot`, `restore`, `runInBackground` on first call; D6 Vite's helper beside the number (616 bytes of room) or inside it
(42); D7 the all-features budget; D8 the T-code family and its page; D9 `codecs` as a module namespace object.

## Deviations from the brief

1. **(f), a table-driven reader and writer, was not prototyped again.** `ts-size-e4` measured −19 for the reader; a
   string-keyed method table would also defeat the rename of row 14 and put a keyed lookup on the call path. What the
   codecs had to give came from row 3 instead (−874).
2. **(g), class to function shapes, is measured where it paid**: the observe waiters as a closure (row 6), snapshot and
   restore as functions over the host (row 8), the stream support as an object (row 4). `Slot`, `Single` and the error
   classes were left: under 50 bytes each, and the errors are public classes.
3. **Property renaming was measured with oxc** (the pinned Vite's minifier, `mangleProps` with a cache), not esbuild: the
   same operation, no new tool. Rolldown's own `output.minify.mangleProps` refuses a build with more than one chunk, which
   is why the pass runs per file at publish time.
4. **The gate's build script has one new switch** (`UNDRA_SIZE_SOURCEMAP`), for the attribution tool. Nothing else outside
   `.10x/` and the two new scripts changed on the branch's tip.
5. **The host was loaded** (load average 50) while the call-path rows were compared; they are a sanity check, not a result.
6. The wasm line measured 116,471 here with rustc 1.99.0 (the record's 116,690 is CI's 1.98.1); nothing in this piece
   touches the wasm.

## How to reproduce

```bash
source scripts/env.sh && (cd runtimes/ts/@undra/runtime && npm ci)
bash scripts/wasm-size.sh                                   # the gate; makes target/wasm-size/hello
node scripts/web-size-attribute.mjs --symbols=60 --strings=40   # the attribution tables of ADR-057
git checkout 294fca2                                        # the last prototype
R=runtimes/ts/@undra/runtime; H=target/wasm-size/hello
node $R/proto/measure.mjs $H $R /tmp/out                    # rows 1 to 12 and the generic wire text: 17,386
node $R/proto/measure.mjs $H $R /tmp/out --messages --mangle                  # row 14: 15,958 (± 5: the codes' numbering)
node $R/proto/measure.mjs $H $R /tmp/out --messages --mangle --split-helper   # row 15: 15,384
```

(`measure.mjs` reads `proto-plugins.mjs` from its own directory and `src/msg.ts` of the runtime it measures.)
