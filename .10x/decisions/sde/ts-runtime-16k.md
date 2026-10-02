# `ts-runtime-16k`: the JavaScript runtime's first chunk at 15.7 KB (ADR-057)

Worktree `wt/ts-runtime-16k`, from `main` `a309e9f` (the architect's tip `23ebf45`, prototypes reverted), 17 commits, merged with `main` at the
end. Author: the implementer of the piece. Reviews: none yet (the integrator's adversarial review is next). One ADR: ADR-057 (Accepted, with an
implementation note) and an amendment at the end of ADR-052. The brief is `.10x/decisions/architect/ts-runtime-16k.md`; the decisions D1 to D9 are
the integrator's, recorded in ADR-057. One generated shape changed (the entry of a schema with a stream: `features: [streams]`, eight goldens, the
playground and two-cores bindings); no wire, ABI or schema change, no schema hash moved.

## What landed

Each lever its own commit, or one or two together, with its measured gate number in the message (ADR-057's implementation note has the table):
**22,100 -> 15,680 gzipped** for what a hello-world page loads up front of the production build of `@undra/runtime`, with Vite's preload helper
(691) beside it; 16,191 with the helper counted in; the page that uses everything 42,385 -> **40,100**. No behaviour is removed and no public name
changed: every class, method, `kind` and typed field of SPEC 17.1 is the same in both flavours of the package.

* Steps 0 to 12: the module rule as a test (`up-front.test.ts`, `test/support/module-graph.ts`); the session payloads, nine port ids, the codecs
  (`codecs` a namespace, ten and fourteen) and the stream support (`features`) out of the first chunk; the typed channel (`CoreTransport`,
  `framed()`) so the in-process host has no `send` and no payload decoder; the observe waiters and what only a core outside this thread needs
  load with its transport; `stats`/`snapshot`/`restore`/`runInBackground` on first call; per-transport `load()` mapping; two rare error classes;
  trims.
* Step 13: every sentence the runtime throws or logs is `msg(<code>, ...values)`: a table of 244 rows (`src/messages.ts`), the production
  module (`messages.prod.ts`) says `T<code>: <values> — <link>`, the package is built twice (`scripts/build.mjs`: `dist` production, `dist/dev`
  development, the export conditions `types`/`development`/`react-native`/`default`), the errors page has a "Runtime messages" section (an anchor
  per code and per wire failure).
* Step 14: `scripts/mangle.mjs` renames the 169 private properties of the production build (`_pending` -> `_c`); the suites run against that build
  (`npm run test:dist`, `UNDRA_TS_DIST`), each a second CI run.
* Step 15: the gate (`scripts/web-size-runtime.mjs`, `scripts/wasm-size.sh`): the package as an app installs it, the helper beside the number,
  three `[size]` rows (16,000 / 16,600 / 42,400), the module list in the record, a changed list fails.

## Numbers

* **Gate** (`scripts/wasm-size.sh --record`, zlib 9): `web/hello-runtime-js` 15,680 (budget 16,000, 320 under), `web/hello-runtime-js-with-helper`
  16,191 (16,600), `web/all-features-runtime-js` 40,100 = 27,355 + 12,745 on demand (42,400); lazy chunks and the Worker script 27,224. The
  record is of the last code commit; the integrator's next `--record` on CI's toolchain refreshes it. The development flavour's first chunk: 21,151 (ungated). The wasm line is main's (116,690): this host's
  rustc 1.99.0 measures 116,471, CI's 1.98.1 gave the record; not re-recorded.
* **Call path, Node** (`call-path.test.ts`'s measurement, best of five attempts of 20 batches, alternating the base `23ebf45` and this tree, eight
  rounds in two orders, load 6.5 to 8.8 from other agents' builds): awaited call 300 to 366 ns before, 330 to 399 after (medians 321 and 335:
  +4%); `callSync` 159 to 181 before, 164 to 189 after (+3%). Budgets are 1,600 and 800 (`[web."node/..."]`). The prototype's reading was the same
  size (340 and 170 against 333 and 162).
* **Chromium 153, headless, the device bench** (`examples/playground/web`, `npm run bench`, base and this tree alternating, three runs each, p50 in
  us, load 6.6 to 7.9): `sync_call` 0.45 0.45 0.43 -> 0.45 0.43 0.44; `sync_call_runtime` 0.30 0.30 0.29 -> 0.30 0.30 0.30; `record_1kb` 1.28 1.48
  1.39 -> 1.44 1.51 1.33; `keyed_insert_10k` 13.4 14.0 13.8 -> 13.5 13.8 13.5; `changeset_100` 17.0 17.8 17.2 -> 16.4 16.9 16.3; merged frame 1.75 1.48
  1.30 ms -> 1.50 1.32 1.25 ms; cold `load` 5.8 5.9 6.2 ms -> 5.9 5.7 5.7 ms. Every `[web."id"]` row holds; nothing moved outside the noise.
* **Live, in a browser** (the Browser pane): the playground's stress screen at 100,000 updates a second, firehose: generated 99,956/s, received
  100,121/s, applied 169/s, 85 drains/s, drain p50 under 0.1 ms and p99 200 us, 45 ns per change-set, 0 dropped frames, 16.1 MB heap, no console
  error (that app aliases the sources, so it runs the readable flavour). A scratch page built with Vite against the **production** package (the
  playground's bindings, `@undra/runtime` resolved through `exports`): 169 private names on `UndraCore.prototype` read `_ck _ae _bU ..`, an
  out-of-range write says `T0233: u8, 300 — https://…/errors.html#T0233`, a wire failure `wire: code=unexpected_eof needed=3 at=0 — …#wire-unexpected_eof`,
  a typed error of the core arrives with its own text (`emptyTitle`, "the title cannot be empty"), a malformed command reports `Todos.remove
  malformed` to `onError`, `stats`, `snapshot` (688 bytes) and `restore` run from their lazy chunk, and the same stress at 100,000/s generated
  99,762/s and applied 73/s. The same package in `wasm-worker` mode on a real Worker (the Worker script bundled by Vite from `dist/worker.js`): a store created and a call
  answered through the worker, a typed `emptyTitle` rejection across it, `callSync` refused with `UndraModeError` saying `T0093: callSync, wasm-worker — …`,
  `snapshot` (687 bytes) and `restore`. The playground's Playwright smoke: 9 of 9.
* **Counts**, on the tree before the merge with `main` (after it: the runtime's suite 2,025 pass + 1 skipped, `test:dist` 1,983, `main`'s two new tests): the
  runtime's suite **2,023 pass, 1 skipped** in 75 files (1,646 on `main`'s count in
  `docs/ONBOARDING.md`) and `npm run test:dist` **1,981 pass** in 73 (the two source-only files excluded); its three typechecks; the wasm
  harness 22 + 36 pass on the sources and on the production build; the TypeScript contract column 33/33 on both; the React Native package 111 pass,
  its contract column 24 pass + 2 skipped, on both; the testkit 38, the devtools page 71, on both; the playground web 135 and its build, the
  fieldbook web 13 and its build; `contract-tests/run-all.sh` (TypeScript, Kotlin, Swift) every scenario of every column; the interop run; the
  two-cores Node app; `undra bindgen --check --docs` clean on the playground, `two-cores/{a,b}`, the cookbook and the fieldbook (`--check` on
  `ios15-sample`); `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test -p undra-bindgen`, `-p undra-cli`,
  `-p undra-bench` (the size record against the tables) pass; `cargo test --workspace --no-fail-fast`: 3,534 pass, 3 fail, 21 ignored, and the 3
  are not this piece's: `undra-macros`'s two trybuild suites (this host's rustc is 1.99.0, CI pinned 1.98.1 then: the rustc 1.99 golden drift the
  handoff names, which `main`'s `e462c3a` regenerated and the merge brought in) and `undra-transport`'s `a_commit_storm_costs_steps_by_time_not_by_commit` (a timing test of the devtools ring, 13,550 of 20,002
  on a loaded host; no change of mine touches that crate); `bash runtimes/ts/devtools/build.sh --check`, `node --test
  scripts/bench-device-report.test.mjs`, `node site/scripts/build-all.mjs` (current) and `check-links --words` (347 of 350) pass.

## Deviations from the brief

1. **The rename pass is not oxc's `minifySync`** and has no map composition (ADR-057's note says why): oxc's printer drops `/* @__PURE__ */`;
   `scripts/mangle.mjs` parses with oxc and splices names into `tsc`'s text, and shifts each map's columns by what an edit changed.
2. **The tests that read `_era`, `_giveBack`, `_install` run on the production build** (they ask `test/support/internals.ts` for the name) instead of
   skipping there; `UNDRA_TS_DIST` also runs the RN, testkit, devtools and contract suites, not only the wasm harness and the contract column.
3. **A suite against `dist` reads the development messages** (`scripts/test-dist-plugin.mjs`): tests across the packages assert
   sentences (the first run of the other packages' suites against `dist` failed eight tests in the React Native package, three in the contract column
   and three in the wasm acceptance on a sentence each). What the production table says is held by `flavours.test.ts` and `dist-flavour.test.ts`.
4. **`UndraFeature` gained `readonly name`**, so that `stripInternal` leaves a real type; **`stream-feature.ts`** exists (below).
5. **Final landed size 15,680, not the prototype's 15,384**: 296 bytes of code the prototypes did not write; the margin is 320 under the budget, not 616.
6. **The development flavour's first chunk is 21,151, not 17.8 KB** (the whole table is one module); ungated.
7. **The commit trailer** of the last six commits names the model this session ran as (the brief named another).

## Findings

* **A build warning I introduced and then found by running an app**: with row 4, `stream-support.ts` was both a static import (the generated
  entry) and the `import()` target of a core without the feature, and Rolldown prints INEFFECTIVE_DYNAMIC_IMPORT at every build of an app with a
  stream (the playground's did). Fixed by a one-line loader module nothing imports statically (`a50bc9a`).
* **A timing test of mine**: `stats().openStreams` right after opening a one-item stream passed on a fast host and failed four of four on a
  loaded one, because the first `stats()` waits for a chunk. It loads the chunk first now. The suite passes three times in a row under twelve CPU
  burners.
* **`build.sh --check` of the devtools page** was stale after the wire module changed (the page bundles it from source): the per-lever passes did not
  run it, the final pass did; rebuilt and committed (`e0ebfe4`).
* **Earlier levers had not been through `cargo fmt`**: one commit of formatting (`56287c5`).
* **Close reasons of the realtime ports are messages too**: `reactNativeWebSocket`'s "the core did not keep up" and the Node client's protocol
  failures are `WsError.Closed(1008, reason)` fields and WebSocket close-frame reasons sent to the peer; in the production build they read
  `T0115 — https://…` (66 bytes, inside a close frame's 123). D1 listed `reason` among the fields that do not change meaning; this one is the
  runtime's own text. If the integrator wants wire-visible reasons to stay sentences in both flavours, `HEADERS_REFUSED`, `DID_NOT_KEEP_UP` and
  the `#fail(code, msg)` sites of `node-websocket.ts` are the places (all in opt-in modules, so no gated byte).
* **`vi.doMock` of a module and `vi.resetModules`**: a class imported before the reset is not the one the fresh registry builds; tests that
  compare classes import after it (several of the new test files say so).

## What the integrator owns

* Merge, the `state(ts-runtime-16k)` commit in `.10x/status.md` and `handoff.md`, and the re-record of the JavaScript rows by the next
  `scripts/wasm-size.sh --record` on CI's toolchain (the wasm line then also comes from 1.98.1).
* The Rust 1.99 drift and the devtools timing test above are main's, not this piece's (the CI-green piece owns them; the merge with `fc326d6` brought its fixes).
* The merge with `fc326d6` (2026-10-02, late): one new sentence of `main` (`node:sqlite` U+0000 refusal, `db/node-sqlite.ts`) became code 245; the devtools
  bundle and the generated site pages were rebuilt; `bench/results/web-size.jsonl` keeps `main`'s wasm line (116,181 under 1.99.0; this host measures 116,471,
  a path-dependent difference of a core this piece does not touch) and this piece's three JavaScript rows.
* Helper branches: none. The scratch directories, servers and the base-commit worktree this piece used are removed.
