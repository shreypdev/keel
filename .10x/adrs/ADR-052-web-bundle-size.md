# ADR-052: Web bundle size: the budget, the levers and the gate

Status: Accepted (2026-10-01, by the integrator, with the decisions at the end; `wt/wasm-size`, piece E5 of
`.10x/specs/2026-10-01-v1x-default-choice-design.md`, Amendment D). Touches how `undra-query` is linked (its init hook and dispatch layer are submitted by
`#[undra::query]` / `#[undra::mutation]` instead of by the crate, and the runtime keeps one hook and one layer
per name: SPEC 9 and 16.3), the order the schema code sorts in (same result, SPEC 2.3), a new size gate (SPEC
14, `bench/budgets.toml`, `scripts/wasm-size.sh`, `bench.yml`) and where the published web number comes from
(README, site), and, from the review, a JavaScript runtime gate and the path remapping of every release build
(`undra-cli`, `cargo.rs`). It does **not** touch the wire, the C or wasm ABI, the schema, the schema hash, the threading
model or any generated Swift, Kotlin or TypeScript. Constitution R9 (budgets are tests) and R11: decided here.

## Context

The README and the landing page publish "Web runtime + hello-world core: 135 KB gzipped wasm, over budget"
against the blueprint's row "Hello-world size added to the app: ≤ 120 KB gz wasm + ≤ 8 KB gz runtime"
(`docs/blueprint.html`, section 14). The number was typed by hand and nothing measured it after it was
written: it grew from 85 KB at the first launch to 135 KB without anyone being told.

**What the row measures** (reproduced exactly, `main` at `a0d638f`): `undra init hello --platforms web`, then
`undra build --platform web` (the `release-wasm` profile of the generated shim: `opt-level = "z"`,
`lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, `strip = "symbols"`; then `wasm-opt -Oz
--strip-debug --strip-producers`, binaryen 133), then gzip at level 9. "Web runtime" in the row is Undra's
*Rust* runtime inside the module (the blog post says "the wasm module of Undra's runtime plus a hello-world
core"): the row is the wasm module alone. The hello world is the `undra init` template core (a `Todos` store
with a keyed list and two computeds, a record, an enum, an error and a function; no query, no mutation).

| `main` a0d638f, hello world | bytes | gzip -9 |
|---|---|---|
| cargo output, before wasm-opt | 410,256 | 141,687 |
| after `wasm-opt -Oz` (what ships) | **353,446** | **136,243** |
| the same with brotli -q 11 (for reference) | | 110,369 |
| the playground core (queries, mutations, several stores), after wasm-opt | 573,795 | 218,487 |

gzip is zlib's deflate at level 9 (Python's `zlib`, which is also what Apple's `gzip -9 -n` produces);
GNU gzip and Node's zlib differ from it by up to 1% on the same file (Node: 136,683), which is why the gate
names its compressor. KB is 1,000 bytes, as `undra build` prints them. The module has no custom sections
at all (`strip = "symbols"` already removes the name section, so `--strip-debug` is a no-op today).

**The JavaScript runtime is a second number.** The blueprint budgets it separately (≤ 8 KB gz). What a hello
world app ships of `@undra/runtime` (tree-shaken and minified, the app's own `src/undra.ts` as the entry, the
lazily loaded worker script excluded): **72,659 bytes, 22,521 gzipped** with the runtime's own pinned Vite 8
(what the gate records), 84,512 / 25,882 with the template's Vite 6 app build. The generated bindings are
1.2 KB gzipped more, the app's loader 0.2 KB. It is far over the blueprint's 8 KB, which predates the
transports, reconnect, coalescing and worker mode: decision 2 at the end restates it at 24 KB and gates it.

### Where the 353 KB go

`twiggy top` over the same build with names kept (`strip = "none"`, `wasm-opt -Oz -g --strip-dwarf`, which
inflates code by about 4%), shallow bytes grouped by what they are, the 165 KB name section left out:

| # | Contributor | bytes | What it is |
|---|---|---|---|
| 1 | `core::slice::sort` monomorphs | 58,269 | sixteen driftsorts: `collect_schema` and the canonical form sort six lists twice and the methods/variants once, each `sort_by` closure its own copy |
| 2 | data segments | 39,737 | strings and tables: messages, panic locations (4.9 KB of `/rustc/..` and registry paths), schema names, doc comments (4.6 KB, ADR-050's measurement), std and serde_json error texts, digit tables |
| 3 | `undra-query` | 29,464 | the query cache, persistence, offline queue and dispatch layer, linked into every core by its own `inventory` registrations; plus its share of rows 4, 7, 8 and 10 (it alone formats floats) |
| 4 | `alloc` | 27,239 | `Vec`, `String`, `BTreeMap`, `VecDeque` instantiations |
| 5 | `undra-ffi` exports | 27,209 | the 20 ABI entry points with the runtime code inlined into them (`undra_init` 7.8 KB, `undra_restore` 6.3 KB) |
| 6 | `undra-runtime` | 25,345 | dispatch, executor, object table, ports, snapshot |
| 7 | `core`, other | 21,559 | `str`, iterators, `Option`/`Result` helpers |
| 8 | `core::fmt` with `flt2dec` | 19,406 | formatting; 5 KB of it is float formatting, reached only from the query layer |
| 9 | `undra-ports` | 18,347 | the ten standard ports' metas and dispatchers (part of every schema and its hash) |
| 10 | `hashbrown` | 16,873 | `HashMap` instantiations (std's `RandomState` reaches no import on this target) |
| 11 | `undra-signals` | 16,217 | signals, computeds, keyed lists, change-sets |
| 12 | `serde` / `serde_json` | 12,318 | the schema JSON export and the canonical form the hash is taken over |
| 13 | the hello core | 10,279 | its generated dispatch, store restore and metas |
| 14 | `undra-meta` | 10,157 | `collect_schema`, `Schema::hash`, meta-to-def conversion |
| 15 | `dlmalloc` | 7,153 | std's allocator on `wasm32-unknown-unknown` |
| 16 | `undra-wire` | 6,343 | the codec |
| 17 | `std` | 4,971 | panic machinery, thread-locals, the no-threads sync shims |
| 18 | `parking_lot` + `parking_lot_core` | 4,878 | the core lock and store locks |
| 19 | `undra-ffi` internals | 2,690 | buffers, the panic hook, the built-in Clock/Rng/Log |
| 20 | compiler builtins, wasm structure | 3,394 | `__multi3` and friends; types, imports, exports, the table |

Checked and **absent**: the diagnostics catalogue (`undra-meta`'s validation and its E-code texts are not
linked: none of their strings is in the module), compile-time E-code teaching strings (the only code in the
binary is E0062, which is a *runtime* message by design: a port proxy method without an error channel panics
with it, SPEC 12), debug names and custom sections (stripped already), `std` randomness (no `random` import is
reached from `HashMap`).

## Decision

### 1. The budget and what it covers

**The hello-world web core, the wasm module alone, is at most 120,000 bytes gzipped**: the `undra init`
template built by `undra build --platform web` (release-wasm, `wasm-opt -Oz --strip-debug --strip-producers`
with binaryen), compressed with zlib's deflate at level 9. The README row and the site card say exactly that
("Web core: Undra's runtime and a hello-world core, one wasm module, gzipped"). The 120 KB stays: it is the
blueprint's published promise, and the code coming next (the v1.x wire revision of ADR-036/037/040/043, the
function table of ADR-044) lands in this module; a tighter budget would be spent by the roadmap rather than
by regressions, which is what the tolerance below is for.

The JavaScript runtime is measured next to it and gated the same way at 24,000 bytes (decision 2 at the end).

### 2. The levers, in order of bytes per unit of risk

Measured on the hello world, each on top of the previous one, `wasm-opt -Oz`, gzip -9:

| Lever | bytes | gzip -9 | Δ gzip | Risk |
|---|---|---|---|---|
| baseline (`main` a0d638f) | 353,446 | 136,243 | | |
| **A. one stable sort per key type, on indices** (`undra-meta`) | 304,523 | 129,589 | −6,654 | none: identical output, pinned by the hash |
| **B. the query runtime linked by use** (`undra-query`, macros, runtime) | **227,266** | **95,838** | −33,751 | low: no change for a core with queries |
| total | −126,180 (−35.7%) | −40,405 (−29.7%) | | |

The playground (which declares queries, so lever B does not apply to it) goes from 573,795 / 218,487 to
522,954 / 212,106 (−6.4 KB gzipped, lever A); 524,488 / 212,003 with the short-list insertion sort.

The record `scripts/wasm-size.sh` wrote from the same tree was 227,227 / 95,768: the template sits at
another path there, and panic locations embed the build directory, so the number moves by tens of bytes
with where the checkout lives (the path remapping below shortens those paths but keeps the part under the
home directory). After the small-list insertion sort (below) the record was 228,812 / 95,712, and after
the review's path remapping it is 228,532 / **95,684**.

**A. Sorting.** `slice::sort_by` instantiates a full driftsort per element type *and per closure*. The schema
code now sorts through `undra_meta::sort`: a list of up to 16 items (most of them) takes a stable in-place
insertion sort; a longer one has its keys collected, one bottom-up merge sort per key type (`&str`, `u16`)
orders their indices stably, and a swap loop applies the permutation; a long list already in order costs one
pass. Only the insertion loop, the key collection and the swap loop are generic over the item type. The
insertion path came second: with the merge path alone, the budgets test's cold start (which collects and
hashes the schema) measured about 4 µs slower than `main` on the same machine, from three allocations per
short list; with it, 75.8 / 76.4 µs against `main`'s 75.3 / 75.7 µs in alternating runs. Its result equals
`sort_by` on every input (an exhaustive test over all 5,040 arrangements of a list with duplicates, and long
pseudo-random ones), so the canonical JSON and every schema hash are unchanged
(`undra bindgen -C examples/playground --check --docs` passes against the committed bindings, hash `0x04d2adf769c58b9f`). Time stays
O(n log n): hashing a schema whose 2,000 records arrive in reverse order takes 239 µs (`sort_by`: 205 µs);
100 records 12 µs (10 µs). A plain insertion sort saved 3 KB more and was rejected for being quadratic
(9 ms at 2,000).

**B. Link by use.** `undra-query` submitted its `InitHook` (hydration) and its `DispatchLayer` itself, so every
core linked the whole query runtime, and every core, queries or not, read `Kv` at start-up. Now
`#[undra::query]` and `#[undra::mutation]` submit both (`::undra::query::__private::{HYDRATE, LAYER}`, hidden
items of the facade) next to the registration they already emit, and the runtime runs an init hook and
consults a dispatch layer **once per name**, however many definitions submitted it. A core without queries
links none of it and starts without a port call; a core with queries behaves exactly as before (one
hydration, one layer: pinned by tests that submit each three times, and by the 15-definition test core of
`undra-query`), including one whose queries live in a dependency crate of the core (the hook and the layer
are submitted in the same expansion as the query's schema registration, so they link or not together; the
review built such a core for the web and the host and saw it list `Kv` at start-up). A `QueryDef` written by
hand, in a core without macro-declared queries, is hydrated on the client's first use (`ctx.query()`,
`ctx.mutate(..)`, a handle constructed by a platform) instead of at start-up (`Shared::start` checks whether the
hook is linked; `crates/undra-query/tests/hand_built.rs`); a registration submitted by hand reaches the
platforms only with `__private::LAYER` submitted next to it, as the macros do (the crate docs say so, and
`bench/benches/query.rs` does). This is the lever the brief called "feature-gating code the hello world does not link",
taken without cargo features: what links the query runtime is the core's own declarations, so there is no
build matrix at all and `undra build` chooses nothing.

**Levers considered and not taken**, with what each would save on top of A and B:

* **Docs out of the embedded schema JSON.** Docs are excluded from the hash (SPEC 2.3), so stripping them
  would keep it; but they are not in the JSON export, they are `&'static str` fields of the registrations
  (ADR-050), and the most stripping them could save is 2.2 KB gzipped (ADR-050's measurement on this
  template) at the price of a negative cargo feature, a macro change and two flavours of every library.
  ADR-050's rejection stands; nothing in this budget needs it.
* **`wasm-opt`.** Already run by `undra build` when binaryen is present, with a warning when it is absent; it
  is worth 5.4 KB gzipped here (141,687 → 136,243). The gate installs a pinned binaryen so it measures the
  optimised artefact. `--converge` saves 0.3 KB for a slower build; `--low-memory-unused` is unsound under
  Rust's stack-first layout (the shadow stack lives at the bottom of memory). Not taken.
* **`opt-level = "s"`** instead of `"z"`: about 150.5 KB gzipped, 14 KB worse. `"z"` stays the default.
* **`lto`, `codegen-units`, `panic = "abort"`** are already set (Cargo.toml and the shim template agree).
* **`std::fmt` avoidance in the hot wire paths.** The wire codec does not format; what formats is error and
  panic text, which R8 and R6 want. After lever B `core::fmt` is 10.5 KB raw. Not worth a rewrite.
* **A build-time schema hash and a host-only schema export** (the lever the README guessed at). The hash
  must be computed by the core from what it actually links (R1, R7); a hash injected by the CLI could
  disagree with the binary, and a plain `cargo build` would not have one. Lever A removed most of the code
  it would have saved, and the export is part of the wasm ABI (SPEC 7).
* **A hand-written JSON writer instead of `serde_json`** in `undra-meta` (12 KB raw), **a smaller
  allocator** than dlmalloc (7 KB raw, an `unsafe` global allocator).
* **`--remap-path-prefix`** was listed here as a follow-up (it shortens the 4.9 KB of panic-location paths)
  and the review took it for its own reason: every shipped binary embedded the builder's home directory
  (`/Users/<name>/...`, 24 times in the hello-world wasm). Every release build `undra build` runs (wasm,
  iOS, Android, host) now remaps `$HOME` to `~` (and a `CARGO_HOME` elsewhere to `/cargo`) for every crate,
  through `build.rustflags` (merged with the project's own), or appended to `RUSTFLAGS` /
  `CARGO_ENCODED_RUSTFLAGS` when the user sets one (Cargo then ignores `build.rustflags`), with
  `--remap-path-scope=object` where rustc knows it so compiler messages keep real paths. A project with
  `target.<triple>.rustflags` in its Cargo config overrides `build.rustflags` and adds the flag itself
  (SPEC 7). Worth 280 bytes raw, 27 gzipped here; checked by `crates/undra-cli/tests/build_web.rs` and by
  the gate.
* **`.expect()` on `serde_json` results** links `serde_json::Error`'s `Debug` and its message table (about
  1.5 KB raw) for an error that cannot happen. Left alone: small, and the message is the right panic text.

### 3. R6 on the web, where panics abort

`panic = "abort"` is not a lever; it is how `wasm32-unknown-unknown` works without wasm exception handling,
and SPEC 7 has required it from the start. R6 holds on the web this way: the shim's panic hook
(`crates/undra-ffi/src/wasm.rs`, `ensure_initialized`; the runtime's own hook once a runtime exists) reports
the panic message and location through the `log` import at level 5 (fatal) *before* the module traps; the
trap surfaces in JavaScript as a `WebAssembly.RuntimeError` out of the export that was running, and the
TypeScript runtime turns it into a typed error: `UndraTransportError` with reason `"trap"`, after which the
transport is closed and every caller gets that typed value, never an exception of unknown shape
(`runtimes/ts/@undra/runtime/src/transport/wasm-main.ts`, `#classify`). Tests:
`crates/undra-ffi/tests/wasm/raw.test.mjs` ("a panic logs at level 5 through the host, then traps") and
`crates/undra-ffi/tests/wasm/ts-runtime.test.mjs` ("a panic in the core logs at level 5, then the transport
reports a trap and closes"), both against the real module. Restarting from the last snapshot is piece A6.
Nothing in this ADR changes that path; both tests pass on the optimised build as well
(`PROFILE=release-wasm`).

### 4. The gate

* `scripts/wasm-size.sh` builds the CLI, creates the `undra init` template under `target/wasm-size/`, runs
  `undra build --platform web`, refuses to measure an unoptimised module (it fails when `wasm-opt` is
  missing instead of measuring something else), compresses with zlib level 9 (Python's `zlib`, the same on
  every machine), and writes one JSON line per artefact to `bench/results/web-size.jsonl` (or
  `$UNDRA_BENCH_RESULTS_DIR`): `{"artifact", "bytes", "gzipped", "budget", ...}` with the raw size, the
  ceiling, the toolchain and the commit. The second line is the JavaScript runtime's share, gated by its
  own table (decision 2); it needs the TypeScript runtime's `node_modules`, and a run that cannot measure it
  exits 2 like one without `wasm-opt`. A module that contains the builder's home directory fails the
  gate. `--record` writes the record only from a run that measured both artefacts within their budgets.
* `bench/budgets.toml` gets a `[size."web/hello-wasm"]` table: `budget_gzip_bytes = 120000`,
  `measured_gzip_bytes` (the record) and `tolerance = 0.05`. The gate fails when the gzipped size is over the
  budget **or** more than 5% over the record: `min(120,000, floor(record × 1.05))`, 100,497 bytes today. The
  tolerance absorbs what the runner does differently (its checkout path in panic locations, its libz) and
  makes any real growth a decision: the change that adds 5 KB re-records the number in the same commit,
  where a reviewer sees it. The ceiling is computed from the committed record, never from the build being
  measured, and CI never records. The budgets parser (`bench/src/budget.rs`) reads the table strictly like
  the others, rejects a record already over its own gate, and its test fails when a record line has no
  table or a table no record line. `[size."web/hello-runtime-js"]`: `budget_gzip_bytes = 24000`, the record
  and `tolerance = 0.05` (23,647 bytes today).
* CI: a `size` job in `bench.yml` (Rust 1.98.1 with the wasm target, as every CI job pins it; binaryen
  `version_133` from its GitHub release, the step failing unless `wasm-opt --version` says 133; Node and
  `npm ci` for the runtime line) runs the script and uploads the JSON as an artifact. It needs no secret
  and no write permission, so it runs the same on a pull request from a fork.
* The published number is generated: `site/scripts/build-numbers.mjs` reads `bench/results/web-size.jsonl`,
  writes the value of the `web-size` row of `site/data/bench.json` (the row keeps its shape and points at the
  record), and fills every `<!--measured:web-size-->` slot of the site and the README. The site workflow's
  "generated files are up to date" check covers README.md too, so a hand edit or a stale record fails CI;
  a slot whose number was edited into markup or whose closing marker was mistyped (which the slot pattern
  would skip) is an error of the script.

### 5. Finding what grew

When the gate fails, attribute the bytes the way the table above was made:

```bash
scripts/wasm-size.sh                                   # builds target/wasm-size/hello
SHIM=$(echo target/wasm-size/target/undra/*/shim/Cargo.toml)
CARGO_PROFILE_RELEASE_WASM_STRIP=none cargo build --manifest-path "$SHIM" \
  --target wasm32-unknown-unknown --profile release-wasm --lib --target-dir target/wasm-size/named
wasm-opt -Oz -g --strip-dwarf --strip-producers --enable-bulk-memory --enable-nontrapping-float-to-int \
  --enable-sign-ext --enable-mutable-globals --enable-multivalue --enable-reference-types \
  target/wasm-size/named/wasm32-unknown-unknown/release-wasm/undra_core_*.wasm -o named.wasm
twiggy top -n 40 named.wasm                            # cargo install twiggy (a tool, not a dependency)
```

Compare with the same recipe on the base commit; a lever's worth is its gzipped delta measured by the
script, not twiggy's shallow bytes (gzip is not additive).

## Alternatives considered

* **Raise the budget to 140 KB.** It would have made the row true by moving the promise. The data showed
  30% of the module was code a hello world does not use; moving the bar would have shipped it to every app.
* **Split the module into lazily loaded chunks.** Rust has no stable way to split one wasm module into
  several that share a memory without wasm-bindgen or the component model, and every byte of the core
  is needed before the first call (the schema hash is checked at load). Lazy loading belongs on the
  JavaScript side (open decision 1), not in the core.
* **Accept 135 KB and say so.** That was the state on `main`; it is what this ADR exists to end.
* **A cargo feature for the query layer** (`undra/query`, on by default, `undra build` turning it off for a
  core without queries). It saves what lever B saves, but adds a build flavour per platform, needs the CLI to
  detect whether a core uses queries, and fails when it guesses wrong. Lever B gets the same bytes from the
  linker, with no flavour.

## Consequences

* The hello-world web core is 95.7 KB gzipped (95,684 bytes; 228,532 raw), 80% of its budget; what the app
  ships of the JavaScript runtime is 22.5 KB (22,521 bytes), 94% of its 24 KB. A core without queries also
  starts faster and makes no `Kv` call at start-up.
* An app that removes its last query or mutation leaves what it persisted (cache entries, the offline
  queue) in `Kv` unread: before, its next start-up deleted them as written under another schema hash; now
  nothing reads them until a version that declares a query again deletes them the same way. Nothing a
  user could rely on is lost (no query can read an entry, no mutation can replay a queue item, of a schema
  that has none); the bytes stay (SPEC 9; `crates/undra/tests/query_linked_by_use.rs`).
* `InitHook` and `DispatchLayer` names are identities now: two different hooks under one name would run only
  one. The two names in the tree (`undra-query.hydrate`, `undra-query`) are unique.
* A core that builds queries by hand (implements `QueryDef` without the macro) hydrates on first use; one
  that also submits `QueryRegistration` / `MutationRegistration` by hand must submit `__private::LAYER`
  (and `HYDRATE`, for start-up hydration) next to them, or a platform's call is answered "unknown object
  type". The repository had one, `bench/benches/query.rs`, which this ADR broke and the review fixed (its
  platform benches panicked; CI now runs every criterion bench once with `--test`).
* A call on an object whose type has no dispatcher, in a core without queries, now fails with the runtime's
  own "no dispatcher is registered for `X`" instead of the query layer's handle-type error; nothing else
  observable changes.
* Every change that grows the module more than 5% re-records `bench/budgets.toml` and the JSON in the same
  commit. Merges that conflict on the record re-run the script.
* `wt/runtime-lifecycle` edits `undra-query/src/lib.rs` (the body of `init`, right above the removed
  `inventory::submit!`) and `undra-runtime/src/runtime.rs`; the integrator keeps both sides (its new `init`
  body and this ADR's `__private` module).

## Risks

* **A toolchain update moves the number by more than 5%.** Then the gate fails on a commit that changed no
  code; the fix is to re-record (and to look at why). CI pins Rust (1.98.1, the version the record was
  measured with) and binaryen, so this happens only at a deliberate toolchain bump, which re-records in the
  same commit.
* **The runtime-JS measurement depends on Vite's chunking and minifier.** A Vite update in the runtime's
  lockfile can move it by more than 5% with no runtime change; the fix is to re-record in the same commit
  (and to look at why), as for a toolchain update.

## Decisions (2026-10-01, the integrator)

1. **ADR-052 is accepted** as written above.
2. **The JavaScript runtime's budget is 24 KB gzipped** for what a hello app ships of `@undra/runtime`
   (the blueprint's 8 KB predates the transports, reconnect, coalescing and worker mode), recorded and
   gated exactly like the wasm: over the budget, or more than 5% over the record, fails
   (`[size."web/hello-runtime-js"]`, 22,521 bytes recorded, ceiling 23,647). A budget that fails forever
   is not a test (R9), so the cut to the blueprint's number is a piece of its own: **`ts-runtime-size`**,
   targeting **16 KB**, with the levers listed under "the options" below (the `wasm-worker` transport
   imported statically by `index.ts`, which defeats `core.ts`'s dynamic import; the `remote` transport
   bundled because the mode is a runtime string; `core.ts` + `mirror.ts`, 47.7 KB before minification).
   That piece lowers the budget in the same commit that reaches it.
3. **The 5% tolerance over the record stays**, alongside the budget, for both artefacts.
4. **No second landing card.** The landing row says plainly that it is the wasm alone ("Web core, hello
   world: the wasm alone, gzipped"); the README states the JavaScript number and its budget in prose.

The options as they were put (kept for the record):

1. **The JavaScript runtime's budget.** The blueprint says ≤ 8 KB gz; a hello world ships 22.5 KB of
   `@undra/runtime`. Options: (a) keep 8 KB and open a piece (E5b) to cut it; visible levers: `index.ts`
   statically imports the `wasm-worker` transport, which defeats the dynamic import in `core.ts` (Vite reports
   `INEFFECTIVE_DYNAMIC_IMPORT`), the `remote` transport is always bundled because the mode is a runtime
   string, and `core.ts` + `mirror.ts` are 47.7 KB of the bundle before minification; (b) restate it (24 KB);
   (c) one first-load budget of 128 KB for both (120 + 8), 118.2 KB today. Recommended: (a), with the JS line
   gated as a ratchet once E5b has set its own number.
2. **The tolerance.** 5% of the record (4.8 KB today), or a budget-only gate (catches nothing until 120 KB).
   Recommended: 5%.
3. **The landing page card** shows the wasm only, labelled as such. A second card for the JavaScript runtime
   would cost words of the 350-word landing budget and show an over-budget number; not added.
