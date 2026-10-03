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
transports, reconnect, coalescing and worker mode: decision 2 at the end restates it (24 KB, then 26 KB after the
merge with the parity failure model) and gates it.

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

The JavaScript runtime is measured next to it and gated the same way at 26,000 bytes (decision 2 at the end, as
restated on 2026-10-01 after the merge).

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
short list; with it, 75.8 / 76.4 µs against `main`'s 75.3 / 75.7 µs in alternating runs. (The review separated
the two levers: with `sort_by` restored on the branch the same row is about 2.3 µs faster, median 73.4 against
75.7 µs over four alternating rounds, so the sort still costs that much; the row matches `main` because lever B
took the hydration task out of it, its core declaring no query. The sort alone, on record-sized items: 8 to 16
unsorted items 1.4-2.3x `sort_by`'s time, reversed inputs past 16 items 3-6x, since the standard sort detects a
descending run; random long inputs equal. Kept: 6.6 KB gzipped for about 2 µs of a 3 ms budget.) Its result equals
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
  table or a table no record line. `[size."web/hello-runtime-js"]`: `budget_gzip_bytes = 26000`, the record
  and `tolerance = 0.05` (the ceiling is the budget today: 26,000 bytes against a 24,841 record).
* CI: a `size` job in `bench.yml` (Rust 1.99.0 with the wasm target, as every CI job pins it; binaryen
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

* At the branch's base the hello-world web core was 95.7 KB gzipped (95,684 bytes; 228,532 raw) and the
  JavaScript runtime 22.5 KB (22,521 bytes). The record, after the merges below: **102,722** bytes gzipped
  (244,382 raw), 86% of the budget, and **24,841** for the JavaScript runtime, 96% of its 26 KB. A core without
  queries also starts faster and makes no `Kv` call at start-up.
* **After the merge with `main` at `38ea11d`** (Track A's WeakCtx, write checks and typed stream failures,
  the parity failure model, React Native), measured by the review: `main` alone builds the hello world at
  372,540 / 143,384 bytes gzipped (it was 136,243 at `a0d638f`); with this ADR's levers 244,382 / **102,722**
  (86% of the budget, but 7.4% over the pre-merge record, so the gate asks for a re-record); the JavaScript
  runtime is **24,841** bytes, over decision 2's 24 KB (`main`'s parity work grew `@undra/runtime` by 2.3 KB).
  The playground: `main` alone 591,974 / 226,344, merged 543,096 / 219,972 (lever A, −6.4 KB gzipped).
  The integrator restated the JavaScript budget at 26 KB (decision 2, below), and both were re-recorded with
  `scripts/wasm-size.sh --record` on the merged tree (with `main` at `7d5b73c`, the tooling piece: unchanged).
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
  code; the fix is to re-record (and to look at why). CI pins Rust (1.99.0, the version the record was
  measured with since the bump of 2026-10-02: 116,690 gzipped bytes at 1.98.1, 116,181 at 1.99.0) and binaryen, so this happens only at a deliberate toolchain bump, which re-records in the
  same commit.
* **The runtime-JS measurement depends on Vite's chunking and minifier.** A Vite update in the runtime's
  lockfile can move it by more than 5% with no runtime change; the fix is to re-record in the same commit
  (and to look at why), as for a toolchain update.

## Decisions (2026-10-01, the integrator)

1. **ADR-052 is accepted** as written above.
2. **The JavaScript runtime's budget is 24 KB gzipped** (restated at 26 KB after the merge, below) for what a hello app ships of `@undra/runtime`
   (the blueprint's 8 KB predates the transports, reconnect, coalescing and worker mode), recorded and
   gated exactly like the wasm: over the budget, or more than 5% over the record, fails
   (`[size."web/hello-runtime-js"]`, 22,521 bytes recorded, ceiling 23,647). A budget that fails forever
   is not a test (R9), so the cut to the blueprint's number is a piece of its own: **`ts-runtime-size`**,
   targeting **16 KB**, with the levers listed under "the options" below (the `wasm-worker` transport
   imported statically by `index.ts`, which defeats `core.ts`'s dynamic import; the `remote` transport
   bundled because the mode is a runtime string; `core.ts` + `mirror.ts`, 47.7 KB before minification).
   That piece lowers the budget in the same commit that reaches it.

   *Restated 2026-10-01, after the merge with `main` at `38ea11d`:* the budget is **26 KB** (26,000 bytes; the
   ceiling is the budget). The parity failure model's error channel (the closed `UndraCallError` set,
   `report` / `onError`, snapshot and restore, worker sync ports) added 2.3 KB to what the hello app ships, so the
   record is 24,841 bytes; 24 KB would have failed from the day it was set, which is not a test (R9). The
   record is the honest number; `ts-runtime-size` still targets 16 KB.

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

## Note (2026-10-01, the persistence-v2 review): the standard ports' dispatchers are linked by use

After ADR-037 and ADR-049 (persistence-v2) and ADR-044 (the ABI table) met on one tree, the hello-world core
measured 120,188 bytes gzipped, 188 over the budget. The lever the persistence-v2 record named is applied, with
section 2's "linked by use" pattern: the Rust-side dispatchers of the eight standard request/reply ports
(`Clock`, `Rng`, `Log`, `Http`, `Kv`, `SecureStore`, `Fs`, `Timer`) are no longer `inventory`-submitted by
`undra-ports` (every core links those ports, and fat LTO keeps every submission). `#[undra::port(dispatcher_by_use)]`
(a hidden flag only `undra-ports` uses) emits `pub static <TRAIT_SNAKE>_DISPATCHER: PortDispatcher` instead,
and `Runtime::bind_dyn_port_with(port_id, imp, &undra_ports::KV_DISPATCHER)` binds a Rust implementation together
with the dispatcher a raw port call on it runs through (`fakes::install` and the dev runner's native `Clock`,
`Rng`, `Log` do). The typed accessors (`undra_ports::kv(&ctx)`) never needed a dispatcher. App ports are unchanged
(their dispatchers are submitted as before). Measured: 116,677 bytes gzipped (−3,511).

## Amendment (2026-10-02, `ts-size-e4`): the JavaScript gate counts what the page loads up front, and the cut

Decision 2 above set the JavaScript runtime's budget at 26,000 bytes, with a follow-up (`ts-runtime-size`) aiming at
16 KB. That piece landed together with E4, the binding call path (ADR-056), as `ts-size-e4`. It reached **21,159
bytes gzipped, 21.2 KB**, not 16 KB; the gate is restated at 21,500 (below), and this section says what was measured,
what was cut, what was not, and why the number stops here.

### What the gate measures

`scripts/web-size-runtime.mjs` put every runtime module in one chunk (a `codeSplitting` group matching the runtime's
directory), including the modules that only a dynamic `import()` reaches. `UndraCore.load` imports the `wasm-worker`
transport that way, so the transport rode in the measured chunk although a `wasm-main` page never loads it. The group
now takes Rolldown's `$initial` modules only (reachable from the page's entry by static imports); a runtime module only a
dynamic import reaches is a chunk of its own, loaded when an app asks for that mode, and the record reports the sum of
those chunks as `lazy_gzipped` instead of gating it. Both numbers, on the same tree (`1801951`, Vite 8.3.1 / Rolldown
1.2.11, hello template, zlib level 9):

| | bytes | gzip -9 |
|---|---|---|
| the gate as it was: one chunk, the `wasm-worker` transport folded in | 83,958 | **25,996** |
| the same tree, the chunk the page loads up front | 77,348 | **24,335** |
| the `wasm-worker` transport on its own, loaded in that mode only | | 2,614 |
| the Worker script (`worker-*.js`), the other thing that mode loads | | 11,886 |

The difference is the measurement (-1,661), not a saving; every line below starts from the up-front 24,335.
`UNDRA_SIZE_MODULES=1` makes the script print each module's rendered bytes per chunk (`=exports` adds the exports each
keeps), `UNDRA_SIZE_TARGET=es2020` builds for another target. The gated build is the pinned Vite's default (native
`#private` fields); an app on Vite 6's default target (es2020) ships 21,615 of the same code, 462 more, and the Undra
Vite plugin builds such an app for `es2022` anyway (ADR-056).

### Where the bytes were

esbuild's per-input minified bytes (`--metafile`, es2022) on the same tree, the share of the up-front gzipped size
estimated in proportion (gzip is not additive; the lever table below is what each change measured), 25,931 gzipped for
esbuild's chunking, which also loads the remote transport:

| Module | min. bytes | ~gz | |
|---|---|---|---|
| `core.ts` (`UndraCore`) | 12,649 | 4,180 | |
| `mirror.ts` | 7,193 | 2,377 | |
| `transport/wasm-main.ts` | 6,924 | 2,288 | |
| `transport/remote.ts` | 5,216 | 1,724 | the mode is a runtime string: bundled by every page |
| `wire/payloads.ts` | 4,183 | 1,382 | includes `decodeHello`, `decodeLog`, `decodePortCall` only the framed transports read |
| `wire/writer.ts`, `reader.ts`, `codec.ts` | 3,779 / 2,813 / 3,120 | 1,249 / 930 / 1,031 | |
| `call-error.ts`, `errors.ts`, `wire/errors.ts` | 2,878 / 1,944 / 1,386 | 951 / 642 / 458 | the public error classes and messages |
| the four default ports: `adapters/{ports, types, secure, fs, codecs, kv, http, idb}.ts` | 11,894 | ~3,900 | built by `browserAdapters()` for ports a hello core never calls |
| `adapters/browser.ts`, `system.ts`, `ids.ts` | 1,132 / 903 / 561 | 374 / 298 / 185 | |
| `wire/types.ts`, `envelope.ts`, `stream.ts`, `port-dispatch.ts`, `object.ts`, `signal.ts` | 1,688 / 1,590 / 1,602 / 883 / 875 / 743 | 558 / 525 / 529 / 292 / 289 / 246 | |

Two things this shows. About a quarter of the first chunk was code a `wasm-main` page with a hello core never runs (the
remote transport, the four default ports with their codecs, error types and browser adapters, the framed transports'
wire code). And the rest is the runtime itself: `UndraCore`, the mirror, the in-process transport, the wire and the
error classes.

### The levers, each its own commit

Up-front gzipped bytes of the hello template, measured at each commit (`scripts/web-size-runtime.mjs`):

| Commit | Lever | gzip -9 | Δ |
|---|---|---|---|
| `feaba72` | the gate counts the up-front chunk (baseline) | 24,335 | |
| `67835f5` | **the remote transport** is a dynamic import (`UndraCore.load` fetches it for mode `remote`) | 22,795 | -1,540 |
| `14eddde` | **the four default ports load on their first call**: `UndraCore` starts from `lightAdapters()` (timer, console, `Connectivity`, `Lifecycle`) and registers Http, Kv, SecureStore and Fs as lazy ports (`adapters/default-ports.ts`); the first call loads `adapters/standard.js` (the port builders, their codecs and error types, the browser adapters). All four are asynchronous ports: a call that waits for the module is an ordinary asynchronous port call, and a failed load is tried again by the next call | 20,481 | -2,314 |
| `e69c1a8` | `NetKind` and `AppState` live with the host events: a module two chunks import is emitted whole in the first one, so `types.ts` (every adapter error class) rode along for two unit-enum lists | 19,890 | -591 |
| `8444262` | the framed transports' wire code (`Kind` apart from the envelope codec, `wire/session.ts` for `Hello`, `Log` and `PortCall`) leaves the first chunk, for the same reason | 19,649 | -241 |
| | *tried and dropped:* one helper for `UndraReader`'s numeric reads (gzip already folds repeated code) | -19 | |
| `b1aa0e5`..`9fdaf86` | the E4 call-path levers (ADR-056): byte-wise integers (+150), the call payload in one allocation (+208), the direct call (+127), the small-reply copy (+16), **no `#private` on the call path's classes** (+798: property names are not mangled), a shared scratch `DataView` (+42), `sendCall` / `callSyncParts` (+163) | 21,153 | +1,504 |
| `dc502f9` | the record after the merge of main and the base object's renamed private flag | **21,159** | +6 |

The up-front chunk went from 24,335 to **21,159** (-3,176, -13%); the call path's speed cost 1,510 bytes of it
(7.7%). The levers the brief named and what happened to them: tree-shakeable module shape (the lazy modules above are
what that gained; every top-level registration was already pure or absent, `sideEffects: false` holds), lazy imports
(remote, default ports, framed wire code; the worker transport already was; recovery is the app's own import), the
error hierarchy (public classes and messages, kept: R8), `const enum`-free numeric tags (`Kind`, `CallTarget`,
`ReplyStatus` are public enums; turning them into objects changes their types, so no), class hierarchies (the errors
are the public API, SPEC 17), dedupe of the wire codecs (the one measured gave -19).

### Why it stops at 21 KB

What a `wasm-main` hello page runs: `UndraCore` (call routing, the error channel, observe and release, snapshot and
restore, the connection signal), the mirror (ADR-031's coalescing and compaction), the in-process transport (the wasm
host and its imports), the wire (writer, reader, the codecs, the payloads the host sends and the change-set it reads),
the error classes, `Signal`, `StreamCall`, the port dispatch and the host events. Each of them is behaviour the
constitution or the SPEC requires, and none is reached only by a mode or a port. Attributed after the levers (esbuild
per-input minified bytes, the up-front closure): `core.ts` 16,100, `mirror.ts` 8,618, `wasm-main.ts` 7,973, `writer.ts`
4,050, `payloads.ts` 3,634, `reader.ts` 3,343, `codec.ts` 3,072, `call-error.ts` 2,876, `errors.ts` 1,942, `stream.ts`
1,853. Strings are 9 KB of the 61 KB minified, almost all of them the messages R8 wants. Getting to 16 KB would mean
removing behaviour (a mirror without compaction, no worker or remote mode in the first chunk of an app that picks one,
no snapshot API) or a different public surface, and that is a decision for a later ADR, not for a size piece. The 8 KB of the
blueprint is out of reach for the same reason.

### What a page loads over its life (review, same tree)

The gate is the up-front chunk, so it does not say what an app loads in all. gzip -9, measured on the hello app at the
gated target: the up-front chunk **21,159** (the bindings and the loader, 1,457 and 197 more, are outside the gate as
before); the first call to Http, Kv, SecureStore or Fs loads `standard` (2,471) and the
`ports` chunk it imports (1,688), **4,159** in all for the four, one request each, in parallel (Vite's preload of the dynamic
import's dependencies). A hello app that calls any of those ports therefore loads **25,318** bytes of runtime over its
life (21,159 + 4,159), against 25,996 before this change: the saving for such an app is 678 bytes and one more round trip at
its first port call, not the 4,686 the up-front chunk shows; an app that never calls them keeps the whole saving. The remote
transport (2,276 + 649 for the envelope chunk it shares with the worker transport), the worker transport (2,610 + 649) and the
Worker script (12,393) load only in the mode that needs them. `lazy_gzipped` (22,159) is the sum of all of them, which no one
page loads. If a port's chunk cannot be fetched (the network is down, a deploy replaced the file, a Content-Security-Policy
that allows the entry script but not the chunks beside it, which a policy by origin or by nonce with `strict-dynamic` does not
do), the call is answered "unavailable" to the
core, `onError` receives an `UndraUnhandledError` naming the port (`Kv port 0x... method 0x...`) with the failed import as its
cause, and the next call tries the load again (`default-ports.test.ts`). The four default ports are asynchronous, whatever
adapter backs them (`sync: false` in the lazy wrapper), so `wasm-worker`'s refusal of a synchronous main-thread port still
happens at load and never concerns a lazily loaded chunk. Calls made before a port's code arrives run in the order they were
made, per port; across two ports the calls of the port whose code arrived first run first.

### The gate

Decision 2 is restated: **`[size."web/hello-runtime-js"]` is 21,500 bytes** (the budget; record 21,159, ceiling
min(21,500, floor(21,159 x 1.05)) = 21,500), the tolerance stays 5%, and the gate is the up-front chunk. The record line
carries `lazy_gzipped` (22,159: the remote transport, the worker transport, the default ports, the framed wire code, the
Worker script) so that growth in what loads on demand is visible in review, though ungated. The wasm line of the record was refreshed by the same
`--record` run (116,575 -> 116,966 bytes gzipped, 117.0 KB: `main` had drifted by 391 bytes since the last record, with no wasm
change in this piece; still within the 120,000 budget and 5% of itself). The README's number is
generated from the record as before. A run still fails when the runtime's `node_modules` are missing.

## Amendment (2026-10-01, `prod-ops`): the up-front gate is 22,000

ADR-046 puts a panic report and a background run on every platform, and the web column pays for part of it in the page's
first chunk. Measured on the merged tree with the gate's own script (`scripts/web-size-runtime.mjs`, the `$initial`
chunk, zlib level 9): **21,756** bytes gzipped, against 21,336 for `main` alone (+420), so the budget `web/hello-runtime-js`
becomes **22,000** (the record is 21,756; the 5% tolerance stays). The hello wasm is 119,227 gzipped (+2,146 on `main`'s
117,081; budget 120,000, unchanged).

What the 420 bytes are (an ablation on the final tree, gzipped, the pieces overlap): loading the trap-report builder
when the app set `onPanic` or `crashRecovery` (130), `runInBackground` (112), the page's background window and its callback
(80), the `pagehide` and `freeze` listeners (80), the `panicReports` and `background` stats fields (76), the Diagnostics
registration (56). What is lazy and costs the hello world nothing: the report builder for the wasm trap path
(`panic-report.js`, 853 bytes, loaded only for an app with `onPanic` or `crashRecovery`), and the `Diagnostics` port with its
codecs (in the `ports` chunk, for native cores only). Tried and rejected: decoding the background report from a lazy
`codecs.js` (+30 bytes: the inline four-field read is cheaper); a lazy `runInBackground` (-45 bytes, but the page window
must run at `pagehide` or `freeze`, when a chunk fetch is unreliable, so it needs a second code path); all of that plus
no stats fields reaches about 21,617, still over the old gate, and drops specified behaviour. `up-front.test.ts` fails if
`core.ts` statically reaches `ports`, `codecs`, `standard`, `panic-report`, `remote`, `wasm-worker` or `recovery`.

One behaviour change on `main`'s lazy design: a page that loads a native core over `remote` (`undra dev`, React Native's
stand-in) now fetches the `ports` chunk (about 2.3 KB gzipped) at load, because the `Diagnostics` port must be registered
before the transport starts; `main` fetched it there only for an explicit Timer adapter. A failed fetch rejects `load`, as
it already did in that case.

## Note (2026-10-02, the objects-callbacks review): the JavaScript gate is restated at 21,800 bytes

ADR-040 and ADR-041 put code in the chunk a page loads up front, because every generated constructor now goes
through it: `[size."web/hello-runtime-js"]`'s `budget_gzip_bytes` is **21,800** (was 21,500), the tolerance stays 5%,
and the record is re-measured in the same commit. What the bytes are, measured on the hello app (zlib level 9, the
gate's own build):

* `main` at `b800994` already measured **21,336**: ns-storage's namespace check had added 163 bytes to the record of
  21,173 without a re-record (its review states 21,336 of 21,500). That is not this piece's growth.
* This piece adds **341** (21,336 -> 21,677 before the review's fixes): the identity map (`adopt`, `collected`:
  one wrapper per handle, a reply's extra reference given back at once, a finalizer that releases a collected
  wrapper's reference exactly once without touching a newer wrapper of the handle), about **132** of them (the
  chunk with `adopt` reduced to `new type(core, handle)` and `collected` to `release` measures 21,533); the
  mirror's callback entries (a main-delivered invocation is a queue entry kind and a fold barrier, ADR-041
  decision 6), the core's `_giveBack`, `_held` and `hostRefs`, and the wire's handle layout of 24 and 40 bits
  make the rest.
* **The alternative, measured and rejected: load the identity map on the first `create()`.** It would take at most
  the 132 bytes out of the chunk (less the cost of the dynamic import), leaving the chunk above 21,500 anyway, and
  the first `create()` of a page would wait for a chunk that is not loaded yet: a dynamic import of a 5 to 7 KB
  chunk the page has not fetched measured **2.6 to 3.8 ms** in headless Chromium against `vite preview` on the
  loopback (three runs), one round trip more on a real network, against 0.1 ms for a chunk already loaded and
  0.000 ms for later imports. Prefetching it during `load` would keep the bytes on the startup path while the gate
  stopped counting them. Later creates would have been unchanged (the module cached), but the first is every
  app's first screen.

The README's and the site's numbers come from the record as before.

## Review note (2026-10-02, `prod-ops` adversarial review): the JavaScript gate is 22,100; the wasm is back under 120,000

Measured on `prod-ops` merged with `main` at `f9a37a8` (objects-callbacks in), with the gate's own script.

**D1, the JavaScript up front.** The rule (R9): growth of `web/hello-runtime-js` is allowed only for behaviour a hello
app gets at load. The amendment above, item by item (its ablation's numbers, gzipped, overlapping):

| Item | Bytes | Verdict |
|---|---|---|
| The `pagehide` and `freeze` listeners | 80 | **Kept up front.** Registered at load for every page; at `pagehide`/`freeze` a chunk fetch is unreliable. |
| The page's background window (`_backgroundWindow`, its `stats()` read) | 80 | **Kept up front.** Runs at every hide, for every page. |
| The `panicReports` and `background` stats fields | 76 | **Kept up front.** Specified surface of `stats()` on every platform; the window reads `background.pending`. |
| The trap-report loader (`_loadPanics`) | 130 | **Kept up front.** The trigger must run at load: the builder and the module's SHA-256 (`imageId`) have to be there before a trap, and with `recovery` the report is built synchronously before the restart. The builder itself (`panic-report.js`) stays lazy. |
| `runInBackground` | 112 | **Lazy** (`background.js`, 281 bytes on demand). A hello core has no background task, so `background.pending` is 0 and the window never calls it. The window fetches the chunk at the first hide with work pending; `visibilitychange` to hidden precedes `pagehide` and `freeze` (a page is frozen only when hidden), and the debounced persistence is flushed by the core on `Lifecycle.Background` itself, not by the run. The amendment's objection (a fetch at `pagehide` is unreliable) holds only for a browser that fires `pagehide` without hiding first, where the page is going away and the replay's network calls could not finish either. A failed chunk fetch rejects the call with an `UndraCallError` (to `onError` in the window), like the other lazy chunks. |
| The `Diagnostics` registration | 56 | **Lazy** (`serveDiagnostics` in the `ports` chunk). Only a native core runs it, and that chunk is already fetched before the transport starts; a wasm page ships none of it. |

On `prod-ops` alone that took the chunk from 21,756 to 21,666 (-90). On the merged tree: **22,005** bytes gzipped, against
**21,672** for `main` (objects-callbacks' record): this piece is **+333**, the four kept items, and 4 bytes for `load` waiting for the module's hash when the app set `onPanic` (the review's fix of
S29: a trap right after load had no `imageId`). The budget is **22,100**, the
record rounded up to the next hundred (the 5% tolerance stays); 22,000 would already fail. Both pieces' bytes, for the
integrator: `main` before either 21,336; objects-callbacks +336 (21,672, its note above: the identity map, the mirror's
callback entries, the handle layout); prod-ops +333 (22,005: the trap-report trigger, the page window with
`pagehide`/`freeze`, the stats fields). `up-front.test.ts` also fails if `core.ts` statically reaches `background.ts`.

**The wasm (a finding of the review, fixed).** Merged with `main`, the hello wasm measured **121,164** gzipped, 1,164 over
the 120,000 budget: `main` was at 119,055 and this piece added 2,109. Where (named builds, twiggy, then the script's
gzipped deltas): the standard function `run_background`, generated as an `async fn`, linked its future, its reply encoding
and its dispatcher into every core (-745 when removed); the wasm hook's `in <operation>` naming (-78); and, the most, the
report paths of a *caught* panic (`report_from`, the frames, the reporting at every guard), which a wasm core can never
run: with `panic = "abort"` nothing catches a panic there. Two changes, no behaviour lost:

1. `guarded` is `Ok(f())` on wasm (the hook still logs the FATAL record before the trap), so no reporting path of a caught
   panic is linked into a wasm core: -1,137 bytes gzipped (part of it `main`'s own dead paths).
2. `run_background`'s dispatcher is written by hand in `undra-ports` (the same declaration, so the schema and its hash are
   unchanged): a runtime with no background task answers an idle report synchronously
   (`Runtime::background_call`), and the asynchronous run is reachable only through `Runtime::add_background_task`: -378.

The hello wasm is **119,654** gzipped (+599 on `main`: the standard surface every schema carries, R1 and ADR-024, the idle
dispatcher, the FATAL record's `at`/`in` trailer); the budget stays 120,000.

## Review note (2026-10-02, `types-paging` adversarial review): the JavaScript up front is 22,068 of 22,100; the gate stays

Measured on `wt/types-paging` after the review's fixes, with the gate's own script (`scripts/wasm-size.sh`, zlib level 9):
**web/hello-runtime-js 22,068** gzipped (record 22,005, budget 22,100: 32 bytes of headroom) and **web/hello-wasm 116,480**
(record 119,654, budget 120,000). The JavaScript number does not depend on the checkout's path (the ~240-byte path effect the
piece's record mentions is the wasm's panic locations), so `main` will measure the same after the merge.

What the +63 bytes up front are, each something a hello page runs at load (R9's rule): ADR-031's amended fold in the mirror
(a slot keeps a full value and the last lazy invalidation after it; +36 as the amendment records), the page call's target in
`UndraCore.call`/`callSync` (`CallTarget.LazyListPage`: the request encoder every transport shares), and the review's fix of
the mirror's wait-for-a-full-value rule (an invalidation is dropped like a patch). `LazyList`, `useLazyList`, `useLoadMore`
and `Decimal` are not in the hello chunk (a store without a `Lazy<T>` signal links none of them).

The budget stays **22,100** and the record is re-measured on `main` by the integrator. The next piece that adds to the chunk
loaded up front either makes room or restates the budget here with its own measured items; 32 bytes is not room for a feature.

## Note 2026-10-02 (objects-followups review): the gate no longer moves with the checkout's path

**Finding.** The hello wasm measured 120,031 gzipped in `.work/objects-followups` and 119,856 in a sibling checkout with a
shorter name, for the same source: the gate (and the record the README and the site publish) moved with the directory the
build ran in, by more than the piece under review had changed. Cause: the remapping above covered `$HOME` only, so what
stayed was the rest of the checkout's path (`~/Desktop/src/.work/objects-followups/crates/undra-runtime/src/…`) in every
panic location of the Undra crates, and the project's own path in those of the core.

**Decision.** `undra build` (every release profile: wasm, iOS, Android, host) names the directories of the build by fixed
labels with `--remap-path-prefix`, widest first (rustc applies the last prefix that matches): `$HOME` to `~`, a `CARGO_HOME`
outside it to `/cargo`, Cargo's registry sources and git checkouts to `/undra/deps`, the Undra checkout the core depends on
by path to `/undra/src`, and the project's Cargo workspace (the project itself when the core is a workspace of its own) to
`/undra/app`. The project's *workspace*, not its own directory, so that two cores of one workspace built into one target
directory carry the same flags (Cargo fingerprints rustflags: a flag per project directory would rebuild every dependency
each time the project changes). `crates/undra-cli/src/cargo.rs` (`RemapRoots`, `Cargo::path_remap`), `Session::remap_roots`.
`scripts/wasm-size.sh` fails when the module names `$HOME`, the checkout or the hello project.

**What this does not make identical.** Not every byte: a `TypeId` is a hash of its crate's identity, Cargo derives that
from the absolute path of a path dependency outside the shim's workspace (the core, the Undra checkout), and no rustc flag
changes it. The `TypeId` constants (about ten, i64 literals in code and 16-byte statics) differ in value and, as LEB128,
by a byte or two in width: two copies of the hello project at paths of different length differ by 4 bytes of 278,794 raw,
and every string of 16 printable bytes or more is the same in both (`build_web.rs`,
`the_web_module_does_not_depend_on_where_the_project_lives`). The gate's number can still move by a few bytes with the path
the repository is checked out at (the Undra crates' ids); it no longer moves by hundreds. Debuggers: the DWARF paths are the
labels too (`--remap-path-scope=object`), and `.lldbinit` does not map them back (it did not map `~` either).

**Measured (review, same source at four checkout paths, before the merge with `types-paging`):** 119,927 (`.work/objects-followups`),
119,931 (`.work/s1`), 119,818 (a 68-character name), and `main` + this fix 119,842 against this piece 119,931 at one path: the
piece's own cost is **+89 bytes**; before the fix the same source moved 120,031 / 119,856 between two paths, in order of the
path's length. What is left (up to 113 bytes, in no order) is the crate identities above. **After the merge** (`main` `94b87ba`,
recorded 116,628 at its own path without the remap): wasm **116,864** gzipped at `.work/objects-followups` (gate 120,000), and the
JavaScript up front **22,105**, 5 over its 22,100 (`types-paging`'s 22,068 plus this piece's +39): trimmed to **22,100** by letting an
aborted call settle its promise before the cancel goes out (no no-op `reject` on the abandoned entry) and by the shorter name of
the restart counter (`_era`). That is no headroom: the next change to the chunk makes room or restates the budget here.

## Amendment (2026-10-02, `ts-runtime-16k`, ADR-057): the JavaScript gate is 16,000, and what it measures

ADR-057 measured the levers the `ts-size-e4` amendment declined ("16 KB would mean removing behaviour") and the founder accepted
the plan: `web/hello-runtime-js` is **15,680** bytes gzipped against a budget of **16,000** (was 22,100), and no behaviour is removed.
What this ADR said about the number changes in four places.

* **What is measured.** The runtime as an app installs it: the gate builds the package (`npm run build`, the production flavour in
  `dist`, the readable one in `dist/dev`), links it into the project's `node_modules` and lets Vite resolve `@undra/runtime`
  through `exports`, which selects the production flavour (messages as `T<code>` with their values and a link, private properties
  renamed). A chunk that holds a sentence of the development table fails the run.
* **The preload helper is not the runtime.** Vite's preload helper, the virtual module the build adds to whichever chunk has an
  `import()`, is a chunk of its own and is reported as `bundler_gzipped` (691) beside the number. The other reading is gated too:
  `web/hello-runtime-js-with-helper` (the helper left in the chunk, 16,191) at 16,600.
* **The modules are part of the record.** `web-size.jsonl` lists the runtime modules that hold code in the first chunk; a run whose
  list differs from the committed one fails and names the module, because a re-export from a module with code of its own can bring a
  module in without any import of it (ADR-057, the module rule; `up-front.test.ts` is the cheap guard of the same rule).
* **A page that uses everything is gated.** `web/all-features-runtime-js` (the playground's bindings with recovery, a panic handler,
  `stats`, `snapshot`, `restore` and a background run; the first chunk plus every chunk of the runtime it loads on demand, the Worker
  script excepted) is 40,100 against 42,400 (was 42,385): the plan may move bytes out of the first chunk, it may not make that
  page load more.

The "Why it stops at 21 KB" section of the `ts-size-e4` amendment is superseded; its levers (the on-demand transports and default
ports) stand, and ADR-057's rows 1 to 15 are what came after. `scripts/wasm-size.sh`, `scripts/web-size-runtime.mjs`,
`scripts/web-size-all-features.ts`, `bench/budgets.toml` (three `[size]` tables for the JavaScript, comments with the history) and the
size job of `bench.yml` follow. The wasm line is unchanged by this piece.
The piece's review moved 131 bytes back into the first chunk (`snapshot`/`restore` at the call, the background window without a
fetch: ADR-057, "Review"): recorded **15,811**, with the helper 16,333, all features 40,221; after its lows and the merge of ADR-059,
**15,774**, 16,285 and 39,922.

## Amendment: native size gates (2026-10-02)

Piece `android-size` (`wt/android-size`), from user feedback U4: a user measured their core at 1.6 MB per Android ABI against a 1.2 MB
budget. This ADR gated the web only; nothing gated an Android or an iOS core, and every native release build used the profile tuned for
speed. This section is the profile, the levers measured on it, what the bytes are, the gates, the speed check and where the user's number
lands. It does **not** touch the wire, the C ABI, the schema, the threading model or any generated code, and no crate's code changed: R6 and
ADR-046 (a native panic is contained by `catch_unwind`, so `panic = "unwind"`) are a constraint the profile keeps.

### What was true

`undra build --release` printed the library's size next to "[budget 1.2 MB per ABI (hello world, release)]" for any core; the design's budget is
for the hello world, and a core with queries, persistence and several stores is bigger by what it does. On `main` at `b909739` the `undra init`
template measured **987,720** bytes for arm64-v8a and **1,054,336** for x86_64, under 1.2 MB, and the iOS device slice added **859,845** bytes to
an app (95% of the design's 900 KB); the playground core (queries, mutations, several stores) measured **2,860,992** / **3,026,936** and added
**2,637,348**. The Android record (`bench/results/android-size.jsonl`) said 978,552 and was "measured, not gated" (it had not moved with
`main`); iOS had no record. The shim's `release` profile (`opt-level = 3`, fat LTO, one codegen unit, `panic = "unwind"`; `strip` is done by `undra build`,
ADR-046) was the one every native release build used, and the only size-tuned profile was `release-wasm`. The workspace's root `Cargo.toml` is not what
an app builds (an app builds the shim `undra build` generates, a workspace of its own), so the profile lives in the shim's template
(`crates/undra-cli/templates/shim/Cargo.toml.tmpl`) and nowhere else: nothing in this repository builds with it, so the root `Cargo.toml` does not repeat it.

### The profile

```toml
[profile.release-mobile]
inherits = "release"
opt-level = "s"
panic = "unwind"

# the call path stays at the speed profile's optimiser: undra-wire, undra-signals, undra-runtime, undra-ffi
[profile.release-mobile.package.undra-wire]
opt-level = 3
```

`undra build --release` builds it for iOS and for Android (`Profile::ReleaseMobile`, `Profile::mobile(release)`, `crates/undra-cli/src/cargo.rs`; the
Android build is `cargo ndk .. rustc --profile release-mobile`, the iOS one `cargo rustc --profile release-mobile`, output directory
`target/<triple>/release-mobile`). The host build (the JVM tests, `undra bindgen`) stays on `release`. What the profile keeps, and why:

* **`panic = "unwind"`**: R6 and ADR-046 need `catch_unwind` on native (a contained panic is a typed reply and a `PanicReport`; `abort` would make it a
  crash). It is spelled out in the profile and a unit test (`shim.rs`) fails if it becomes `abort`. `abort` is the lever that would have saved most,
  and SPEC 7 keeps it for wasm.
* **`lto = "fat"`, `codegen-units = 1`** are inherited. **`debug = "line-tables-only"` and no `strip`** are inherited, so ADR-046's symbol files are
  unchanged: the CLI strips the copy that ships (`llvm-strip`) and keeps the unstripped twin, and `--no-symbols` reaches the profile through
  `profile.release.*` as it did (a profile that inherits `release` sees that override, as `release-wasm` always has).
* **The call path stays at `opt-level = 3`** (`undra-wire`, `undra-signals`, `undra-runtime`, `undra-ffi`: the codec, the signals and change-sets, the dispatcher
  and the entry points). `opt-level = "s"` for everything was the first design and the speed check (below) is why it is not the profile: the device rows
  moved by up to 8%, and the core's own operations measured on the host were 17% slower (median over 88 rows, up to 5x for two loops the vectoriser no longer
  takes up). With these four crates at 3 the device rows are inside their noise and the host rows are 3% slower (median), for about 6 points of the size. A unit
  test (`shim.rs`) fails if one of the four drops out of the profile. What the override keeps at 3, precisely (the review checked it with `CARGO_TERM_VERBOSE=true undra
  build --platform android --release`: rustc gets `-C opt-level=3` for those four crates and `-C opt-level=s` for every other one, the app's core, `serde_json` and the shim
  included): the code each of the four compiles itself, optimised at 3 before the link and without the `optsize` attribute. Two things stay at `s` whatever the list says:
  the link-time optimisation of the whole library runs at the shim's level (`s`), and a generic function of those crates instantiated for the app's own types (a store's
  signal of the app's record, the codec of the app's record) is compiled in the app's crate, at `s`. The device rows and the host rows below are measured with both, so
  the numbers include them.
* **`opt_level` in `[ios]` and in `[android]` of `undra.toml`** (added by the review, "The knob" below): this profile is the default, `"s"`. A team with a
  hard byte budget sets `"z"`, which builds `release-mobile-z` (`opt-level = "z"` for every crate, the call path included; `panic = "unwind"` kept); a team
  whose own hot code should not be optimised for size sets `"3"`, which builds `release`, the speed profile every native release build used before. It is a
  setting of the project, like `[web] opt_level`, so the Xcode and Gradle builds that run `undra build --release` get it without a flag. The gates measure the default.

### The levers

Hello world (`undra init hello`, no query) and the playground core, `undra build --release` on `main`'s CLI with `CARGO_PROFILE_RELEASE_OPT_LEVEL`
and `RUSTFLAGS` as the row says (one lever at a time unless the row says "on top"), the profile's own row built by this piece's CLI, rustc 1.99.0,
NDK r27.2.12479018, Xcode 26.6, Apple M5 Pro. Android: the bytes of the shipped (stripped) `lib<ns>.so`. iOS: the **linked** column is what is gated: the
device slice linked with `-force_load -dead_strip`, local symbols stripped (`strip -x`), the sections of its `__TEXT`, `__DATA_CONST` and `__DATA`; the
archive is the `.a` the XCFramework holds.

| Lever | arm64-v8a | Δ | x86_64 | Δ | iOS linked | Δ | iOS archive |
|---|---|---|---|---|---|---|---|
| baseline: `release`, `opt-level = 3` (`main` b909739) | 987,720 | | 1,054,336 | | 859,845 | | 1,865,888 |
| A. `opt-level = "s"` everywhere | 849,760 | −137,960 (−14.0%) | 898,024 | −156,312 (−14.8%) | 744,149 | −115,696 (−13.5%) | 1,746,192 |
| **A′. `"s"`, the call path at 3 (the profile)** | **905,520** | −82,200 (−8.3%) | **971,464** | −82,872 (−7.9%) | **793,517** | −66,328 (−7.7%) | 1,806,544 |
| B. `opt-level = "z"` everywhere | 774,480 | −213,240 (−21.6%) | 841,240 | −213,096 (−20.2%) | 608,574 | −251,271 (−29.2%) | 1,966,976 |
| on top of A: `-Wl,--gc-sections` spelled out | 849,760 | 0 | 898,024 | 0 | | | |
| on top of A: `-Wl,--no-gc-sections` | 853,376 | +3,616 | 901,368 | +3,344 | | | |
| on top of A: `-Wl,--icf=safe` | 849,760 | 0 | 898,024 | 0 | | | |
| on top of A: `-Wl,--icf=all` | 845,152 | −4,608 | 893,816 | −4,208 | | | |
| on top of A: `-Wl,--pack-dyn-relocs=android` | 823,440 | −26,320 | 870,648 | −27,376 | | | |
| on top of A: `-Wl,-O2` | 849,728 | −32 | 897,992 | −32 | | | |
| on top of A: `-C llvm-args=-enable-machine-outliner=always` | 845,744 | −4,016 | 910,696 | +12,672 | | | |
| on top of A: `--icf=all` and `--pack-dyn-relocs=android` | 818,848 | −30,912 | 866,472 | −31,552 | | | |
| B with `--icf=all` and `--pack-dyn-relocs=android` | 741,744 | −245,976 | 807,720 | −246,616 | | | |
| the playground, baseline | 2,860,992 | | 3,026,936 | | 2,637,348 | | 5,518,848 |
| the playground, A | 2,398,920 | −462,072 (−16.2%) | 2,477,576 | −549,360 (−18.1%) | 2,223,980 | −413,368 (−15.7%) | 5,095,552 |
| **the playground, A′ (the profile)** | **2,498,856** | −362,136 (−12.7%) | **2,599,592** | −427,344 (−14.1%) | **2,317,056** | −320,292 (−12.1%) | 5,222,584 |
| the playground, B | 2,024,336 | −836,656 (−29.2%) | 2,250,640 | −776,296 (−25.7%) | 1,653,480 | −983,868 (−37.3%) | 5,699,792 |
| the playground, A with `--pack-dyn-relocs=android` | 2,294,264 | −104,656 vs A (−4.4%) | 2,371,832 | −105,744 vs A | | | |

What the table says:

* **`opt-level = "s"` is the lever**: uniformly it takes 14% off the arm64 library of a hello world, 16% off the playground's, and 13.5% off what the iOS slice adds
  to an app. Keeping the call path at 3 gives back 6 points on the hello world and 3.5 on the playground (the four crates are most of a hello world and little of a
  large core), and keeps the speed (below). `"z"` takes 8 to 13 points more than `s` and costs the call path up to half again, so it is not the default.
* **The iOS `.a` is no measure of what an app gets.** At `"z"` the archive *grows* (1,746,192 to 1,966,976 bytes; the playground's 5.1 to 5.7 MB: the
  outlined functions' local symbols and the debug map) while what the app links *shrinks* by a fifth. That is why the gated iOS number is the linked one,
  and why the `.a` and the slice object's sections are in the record but not in the gate. The design's 900 KB is that number (859,845 before).
* **`--gc-sections` is already on**: rustc passes it, spelling it again changes nothing, and turning it off costs 3.6 KB, which is how little fat LTO leaves
  for the linker to drop. `-Wl,-O2` (string tail merging) changes 32 bytes.
* **`--icf=safe` changes nothing** (rustc emits no address-significance table, so lld finds no function it may merge); **`--icf=all` saves 4.6 KB (0.5%)** and was
  not taken: it merges functions with equal bodies, which changes the address a function-pointer comparison or a debugger test would see, for half a percent.
* **`-Wl,--pack-dyn-relocs=android` saves 26 KB (3.1%; 105 KB, 4.4%, on the playground)**: the `.rela.dyn` of 30 KB becomes 4 KB. **Not taken**: the dynamic
  loader of every Android version an app supports must read the packed format, and `min_sdk` is the app's to set (21 or more, `undra.toml`); the CLI cannot promise
  it, and a library that does not load is not a size win. It is a documented option for an app that can take it (`RUSTFLAGS`).
* **The machine outliner** takes 65 KB off arm64 `.text` and gives 61 KB back: each outlined function has an unwind entry (`.eh_frame` +38 KB,
  `.eh_frame_hdr` +15 KB, `.gcc_except_table` +8 KB). Net 4 KB on arm64 and 12.7 KB *worse* on x86_64. Not taken.

### What the bytes are

`llvm-size -A` and `llvm-nm --print-size --size-sort -C` on the unstripped twin `undra build --release` keeps in `build/symbols/android/<abi>/` (the NDK's own tools:
no `cargo-bloat`, no dependency), symbols grouped by crate (LTO inlines across crates, so a crate's share is what stayed a function of its own), the hello world at
uniform `opt-level = "s"` (617 KB of named code and data of the 850 KB file; the rest is the sections below):

| What | bytes | share | Ours to cut? |
|---|---|---|---|
| `undra-runtime` (dispatch, executor, objects, persistence, the guard) | 97,988 | 15.9% | the product |
| `core` (fmt, iterators, slices) | 80,522 | 13.0% | monomorphs of our types |
| the standard library's backtrace symboliser (`gimli`, `addr2line`, `object`, `rustc_demangle`, `miniz_oxide`) | 67,936 | 11.0% | **not on stable**, see below |
| `undra-ffi` (the 20 entry points, the JNI shim) | 59,804 | 9.7% | the ABI |
| the standard library's panic and thread glue | 46,533 | 7.5% | no |
| `alloc` | 43,368 | 7.0% | |
| `undra-meta` (`collect_schema`, the canonical JSON the hash is taken over, closures) | 42,872 | 6.9% | R7 needs the hash at load |
| `undra-signals` | 38,756 | 6.3% | the product |
| `std` | 35,266 | 5.7% | |
| `hashbrown`, `libunwind` (C++), the core itself, `serde`/`serde_json`, `parking_lot`, `jni`, `undra-wire` | 20,940 / 15,556 / 13,184 / 10,324 / 7,900 / 5,252 / 4,988 | 3.4 / 2.5 / 2.1 / 1.7 / 1.3 / 0.9 / 0.8% | |

By section (the same build): `.text` 607,208 (71%); `.eh_frame` and `.eh_frame_hdr` 96,916 (11%) and `.gcc_except_table` 32,184 (3.8%), the unwind tables a core that
unwinds needs; `.rodata` 46,252 (5.4%); `.rela.dyn` 30,096 (3.5%); `.data.rel.ro` 23,864 (2.8%). The playground at the same profile: the core itself 324 KB (19%), `core` 222 KB,
`undra-runtime` 186 KB, `undra-query` 185 KB, `undra-signals` 160 KB, `undra-ports` 87 KB (the ports' metas and dispatchers, part of every schema), `alloc` 83 KB; the same 68 KB of symboliser.

**The floor.** A bare Rust `cdylib` with one `catch_unwind` and one `panic!`, and none of Undra, is **260,272 bytes stripped at `opt-level = "s"`, with 127
`gimli`/`addr2line` symbols in it**: the standard library's default panic hook prints a backtrace and links the symboliser whether or not anything asks for one, so about 260 KB of
a hello world is the price of a Rust library that unwinds and of nothing Undra does. Removing it takes `-Zbuild-std` with `panic_immediate_abort` (an abort) or nightly-only options
(`-Zlocation-detail`, `-Zfmt-debug`); a stable, no-build-std toolchain, which is what the constitution's toolchain section asks for, has none.

**Findings.** Nothing debug-only is linked into a release core, so there is no feature to put it behind. The devtools hub is in the CLI (`undra dev`); a core carries the runtime's
inspector registry (`undra-runtime::ext`, a few hundred bytes). `undra-testkit` links nothing into a core that does not use it. No `regex`, no second JSON stack, no float
formatting outside `serde_json`'s (`zmij`, 17.7 KB, in the playground only). `Backtrace::force_capture` in `guard.rs` (the status 2 body) does not link the symboliser: the
standard hook already did (the review measured it: with `capture_backtrace` returning an empty string the hello world's arm64 library is 6,616 bytes smaller, 0.7%, and all 127 of
its `gimli` and `addr2line` symbols are still there; those bytes capture and format the report's `backtrace` text, which ADR-046 defines, so it stays). This piece changes no crate's code; it is a profile, a gate and a record.

### The gates

`scripts/native-size.sh` builds the hello world as an app does (`undra init`, `undra build --release`) and gates three rows of `bench/budgets.toml`: at most the design's
number and at most 5% over the record, whichever is lower, the web rows' shape (`budget_bytes`, `measured_bytes`, `tolerance`; the parser reads them as raw bytes, `bench/src/budget.rs`,
`SizeUnit`, and a size table is one unit or the other):

| Row | Measures | Record | Budget (the design's) | Gate (what fails CI: record + 5%) |
|---|---|---|---|---|
| `android/hello-arm64-v8a` | `libhello_core.so`, stripped, 16 KB aligned | 898,176 | 1,200,000 | 943,084 |
| `android/hello-x86_64` | the same | 960,776 | 1,200,000 | 1,008,814 |
| `ios/hello-arm64` | the device slice of the XCFramework, linked and stripped (above) | 784,086 | 900,000 | 823,290 |

The record is the review's, `scripts/native-size.sh --record` on the branch merged with `main` 773054f and the pieces that land before it (the piece first recorded 905,520,
971,464 and 793,517; `main`'s cold-restore piece took `serde`'s serializer out of every core, 7 to 11 KB of a hello world). The other tables of this amendment are the piece's
measurements on its own tree, `main` b909739, except "The knob", which is the review's on the merged tree.

**Is the iOS row what an app gains?** The review linked the same slice into a minimal iOS executable whose `main` takes the address of
`hello_core_undra_api`, against the same executable without the core (`clang -Wl,-dead_strip`, `strip -S -x`, Xcode 26.6): its `__TEXT`, `__DATA_CONST` and `__DATA` sections
grew by **786,457** bytes, and `__LINKEDIT` (chained fixups, the function starts, the indirect symbols) by 6,904 (on the piece's tree, where the row measured 793,517). The row is 0.9% above the sections an app gains (the dylib
the script links keeps a few more bytes of `__TEXT`) and leaves out the 0.9% of `__LINKEDIT`: within a percent of the file growth either way, so the row is an honest
measure of the delta; the page padding of the app's segments is the app's.

The script fails when a library's LOAD segments are not 16 KB aligned (Google Play), when one contains the builder's home directory, the checkout or the project (the remapping of this ADR),
and, like the web gate, when it cannot measure (no NDK, no cargo-ndk, no Xcode: exit 2). The record is `bench/results/native-size.jsonl` (it replaces `android-size.jsonl`; the README and the site
read it through the same `<!--measured:android-size-->` slot) and `--record` re-records it and the tables together, from the committed tree. The ceiling of a row is the committed record's, never the build's.
CI: the `size` job of `bench.yml` installs cargo-ndk and the NDK r27 (the version the record was measured with) and runs the Android half beside the web one; the iOS slice needs Apple's
linker, so `size-ios` is a `macos-15` job of its own. Neither needs a secret or a write permission.

### Speed

The rule: a size win may not slow the call path. The rows are the device bench's (`scripts/bench-device.sh`: the playground through the generated binding, JNI or Swift,
and the mirror): a synchronous call, a 1 KB record, a keyed insert into 10,000 rows, a change-set of 100 signals, the merged drain frame (1,667 one-update patches) and the cold
load. `bench/budgets.toml` has no `[device."..."]` tables (the device bench fails nothing but the web rows), so the tolerance is stated here: **10%**, the widest spread between the
repository's own repeated runs of one build (the committed iOS simulator runs of 2026-10-01: keyed insert 8,000 to 8,792 ns; Android emulator, 251 to 266 ns for a call), and a row
is also read against the noise it was measured with.

**Method.** Playground cores and apps built from a clone of `main` by `main`'s CLI with `CARGO_PROFILE_RELEASE_OPT_LEVEL` (`base` is `release`, `opt-level = 3`; `s`; `z`) and by a
CLI whose template adds the four call-path crates at 3 (`m`, the profile; its arm64 library is byte-identical to the one this piece's `release-mobile` builds: 905,520 bytes for the hello
world). Each variant is installed once and the runs are **interleaved**, the variants in rotating order round after round, so drift cancels: on the shared AVD `emulator-5554` (arm64-v8a,
API 35, hardware-virtualized; never restarted) and on the iPhone 17 Pro simulator (iOS 26.5), the Android runs of the first series 26 per variant (`base`, `s`, `z`) and of the second 14 (`base`, `s`, `m`),
the iOS ones 8 and 8. The Mac was shared with other agents (load average 3 to 35 during the Android runs), which makes the emulator's medians meaningless (the two halves of one variant's runs differ
2x) and its slowdowns one-sided, so each cell is the **best of the runs**, and the last column is that estimator's noise: the ratio of the best odd-numbered base run to the best even-numbered one.
The `s` and `m` columns are against the base of their own series, `z` against the base of its own.

Android emulator (arm64-v8a):

| row | base | s | s / base | m (the profile) | m / base | z | z / base | noise |
|---|---|---|---|---|---|---|---|---|
| sync call | 370 ns | 358 ns | 0.97x | 391 ns | 1.06x | 544 ns | 1.50x | 1.11x |
| 1 KB record | 1,667 ns | 1,625 ns | 0.97x | 1,625 ns | 0.97x | 1,958 ns | 1.18x | 1.07x |
| keyed insert, 10k | 39.1 us | 34.5 us | 0.88x | 35.2 us | 0.90x | 36.1 us | 1.09x | 1.02x |
| change-set, 100 signals | 29.3 us | 28.2 us | 0.96x | 28.7 us | 0.98x | 33.7 us | 1.26x | 1.04x |
| drain frame | 312.8 us | 314.8 us | 1.01x | 264.4 us | 0.85x | 282.3 us | 1.14x | 1.18x |
| cold load | 3.13 ms | 2.62 ms | 0.84x | 2.59 ms | 0.83x | 3.08 ms | 0.79x | 1.07x |

iPhone 17 Pro simulator:

| row | base | s | s / base | m (the profile) | m / base | z | z / base | noise |
|---|---|---|---|---|---|---|---|---|
| sync call | 306 ns | 310 ns | 1.01x | 302 ns | 0.99x | 396 ns | 1.34x | 1.05x |
| 1 KB record | 459 ns | 500 ns | 1.09x | 458 ns | 1.00x | 542 ns | 1.18x | 1.09x |
| keyed insert, 10k | 8,000 ns | 8,375 ns | 1.05x | 8,292 ns | 1.04x | 8,541 ns | 1.05x | 1.05x |
| change-set, 100 signals | 27.4 us | 28.3 us | 1.03x | 27.5 us | 1.00x | 31.6 us | 1.17x | 1.03x |
| drain frame | 474.5 us | 474.5 us | 1.00x | 490.9 us | 1.03x | 458.2 us | 0.95x | 1.03x |
| cold load | 21.65 ms | 22.38 ms | 1.03x | 21.97 ms | 1.02x | 22.57 ms | 1.04x | 1.04x |

The core's own operations, which the device rows dilute (the platform's side of a call dominates them): the host budgets test (`cargo test -p undra-bench --test budgets --release`, 88 rows) built
under each profile, the best of 8 interleaved runs, noise 1.00x to 1.07x, ratios to `opt-level = 3`:

| | median of 88 rows | 90th percentile | `call_sync/add` | `record1k/roundtrip` | `changeset_100/runtime` | `vec_u32_1k/roundtrip` | `cold_start_restore_100kb` |
|---|---|---|---|---|---|---|---|
| `s` everywhere | 1.17x | 1.32x | 1.26x | 1.11x | 1.47x | 3.02x | 1.23x |
| **`m`, the profile** | **1.03x** | 1.15x | 1.06x | 1.05x | 1.15x | 1.04x | 1.24x |
| `z` everywhere | 1.84x | 2.83x | 2.60x | 1.35x | 2.56x | 7.40x | 1.72x |

**Reading it.** `z` fails on every boundary row it can be seen on: a sync call 1.50x on the emulator and 1.34x on the simulator, a change-set 1.26x and 1.17x, a 1 KB record 1.18x on both, the core's own
operations 1.84x (median), so it is not the default. `s` everywhere is inside the tolerance on every device row (the emulator cannot tell it from the base; the simulator has it up to 9% slower on one
row and 5% on another, against noise of 3% to 9%), but the core's own operations are **17% slower (median)** and `wire/vec_u32_1k/roundtrip` 3x, because the vectoriser no longer takes the loop up:
that is the work an app does inside its core, and the device rows hide it, so a profile that passes them is not yet one that does not slow the core. **The profile (`m`)** has the host rows at 1.03x (median),
the simulator's device rows between 0.99x and 1.04x and the emulator's between 0.83x and 1.06x (its noise, 1.02x to 1.18x), for 6 points less of size on a hello world than `s` everywhere. What it leaves: `cold_start_restore_100kb`
at 1.24x (the restore runs in `undra-meta`'s closures and the app's generated restore code, which stay at `s`: 30 µs of a start-up that takes milliseconds), and `signals/changeset_100/decode` at 2.2x (the Rust decoder,
which dev tooling and tests run; a shipped core encodes change-sets and the platform decodes them). What is not claimed: a physical device (these are the emulator and the simulator on a shared Mac), and the
emulator's cold load being faster for every variant (0.8x) is its noise, not a win.

### The knob (review, 2026-10-02)

The brief's question was whether to leave 1.2 MB to a follow-up. The knob is a few lines (a profile in the shim's template, a key in two tables of `undra.toml`,
`Profile::mobile(release, level)`), so it is in this piece: `[android] opt_level` and `[ios] opt_level`, `"s"` (the default), `"z"` or `"3"` (also the integer `3`), any other
value a `C0002` that names the three. `undra init` writes the line commented out. Measured through the knob itself (`undra build --release` with each value, the `undra init`
template and the playground, on the branch merged with `main` 773054f, rustc 1.99.0, NDK r27, Xcode 26.6; on the piece's own tree every number was 0.2% to 2.3% higher, the hello
world's at `"3"` most, so its percentages were a point or two larger):

| `opt_level` | profile | hello arm64-v8a | hello x86_64 | hello, iOS linked | playground arm64-v8a | playground x86_64 | playground, iOS linked |
|---|---|---|---|---|---|---|---|
| `"3"` | `release` | 965,968 | 1,028,184 | 838,126 | 2,842,448 | 3,003,640 | 2,618,037 |
| `"s"` (default) | `release-mobile` | 898,176 (−7.0%) | 960,776 (−6.6%) | 784,086 (−6.4%) | 2,493,376 (−12.3%) | 2,590,752 (−13.7%) | 2,309,589 (−11.8%) |
| `"z"` | `release-mobile-z` | 766,528 (−20.6%) | 831,688 (−19.1%) | 601,203 (−28.3%) | 2,018,792 (−29.0%) | 2,243,528 (−25.3%) | 1,648,597 (−37.0%) |

The middle ground, `z` with the call path kept at 3 (`CARGO_PROFILE_RELEASE_MOBILE_OPT_LEVEL=z` on the default profile; the piece's tree), measures 883,472 bytes for the hello
world's arm64-v8a library (2.4% under the default there: the four call-path crates are most of a hello world) and 2,197,976 for the playground's (12.0% under the default, 8.6%
over `"z"`). It is not a value of the knob: its speed was
not measured (the app's own code and every generic instantiated for its types would be at `z`), and the knob's job is the smallest library; it stays the follow-up below.

**Speed, re-measured by the review** (the playground's device bench, the three variants built through the knob, installed in turn and run interleaved in rotating order; on
the emulator a warm-up run after each install is discarded and the device rests 10 s first, because the package broadcasts of an install wake system apps). Each cell is the median of
the variant's runs; "noise" is the ratio of the `"3"` variant's odd-round median to its even-round one. The shared emulator ran 3 to 4 times slower in absolute terms than the
committed results of 2026-10-01 (a call 1,068 ns against 266), steadily so (noise 1.00x to 1.03x on the four call rows), so the ratios are the reading:

| row | Android emulator, 8 runs each: `"s"` / `"3"` | `"z"` / `"3"` | iPhone 17 Pro simulator, 5 runs each: `"s"` / `"3"` | `"z"` / `"3"` | noise (emulator / simulator) |
|---|---|---|---|---|---|
| sync call | 0.99x | **1.58x** | 0.99x | **1.33x** | 1.02x / 1.01x |
| 1 KB record | 1.03x | 1.17x | 1.00x | 1.18x | 1.00x / 1.00x |
| keyed insert, 10k | 1.00x | 0.97x | 0.99x | 1.04x | 1.03x / 1.01x |
| change-set, 100 signals | 1.01x | 1.15x | 0.99x | 1.13x | 1.02x / 1.03x |
| drain frame | 1.02x | 0.95x | 0.95x | 0.77x | 1.00x / 1.19x |
| cold load (first in the run) | 1.08x | 1.10x | 0.94x | 0.92x | 1.18x / 1.09x |

This reproduces the piece's reading: the default profile is inside the noise on every row of both targets (the implementer's 0.83x was the emulator's cold load and its 0.85x the
drain frame, both rows whose noise was 1.07x and 1.18x: noise, not a win), and `"z"` costs a synchronous call 1.58x on the emulator and 1.33x on the simulator, a record or a
change-set 13% to 18%. A team that sets `"z"` pays that, and the host rows above (1.84x median for the core's own operations) say what it costs inside the core.

### Where the user's number lands

U4's 1.6 MB core is not the hello world, and it was measured with the speed profile. Split it as the hello world's 987,720 bytes of that time (the runtime, the standard
library, a template's worth of core) plus about 612 KB of the user's own; take the hello world's bytes as they are now at each setting, and scale the user's part as the
playground's own part scales (what the playground adds over the hello world: 1,876,480 bytes at `"3"`, −15.0% at the default, −33.3% at `"z"`): the core becomes **about 1.42 MB**
per ABI with no setting changed and **about 1.18 MB with `opt_level = "z"` in `[android]`** (the CLI prints "1.6 MB" for 1.55 to 1.65 million bytes: 1.38 to 1.46 MB, and 1.14
to 1.21 MB). The default does not reach 1.2 MB; `"z"` reaches it, narrowly and not for every core of that size, for the price above (a synchronous call 1.58x on the emulator). Relocation packing would take another 4% (`-Wl,--pack-dyn-relocs=android` through `RUSTFLAGS`, for an
app whose `min_sdk` loads it). So the answer to U4 is a setting, measured and documented, not a promise: the team sets `opt_level = "z"`, measures its own core with `undra build
--release` (it prints the size) and reads the speed rows above against its own hot paths.

The follow-ups, in bytes per risk: `z` on the cold crates only with the call path at 3 (measured for bytes above: −12.0% of the playground under the default; its speed is the open
question); relocation packing applied by the CLI when `[android] min_sdk` is one whose loader reads the packed format (−3% to −4%; the review's reading is that the packed
format is read from API 23, under the default `min_sdk` of 26, which wants a load test on the oldest emulator before the CLI turns it on); and the runtime's own size
(`undra-runtime::runtime` is 65 KB of the 98 KB, `undra-meta`'s `Schema::canonical_json` 12 KB).

### Consequences

* A mobile release build changes bytes and the profile's name: `target/<triple>/release-mobile/` replaces `release/` for iOS and Android (the CLI's debug-size hint reads the new directory); the generated Xcode and
  Gradle integrations call `undra build --release` and are unaffected. A project that built with its own `CARGO_PROFILE_RELEASE_*` variables must set the `release-mobile` ones (or `RUSTFLAGS`) now.
* The next change that grows a hello-world core by 5% re-records in the same commit, with the measured cause. A toolchain bump (rustc, the NDK, Xcode) can move the numbers by a few percent: re-record after a look at why.
* The Android record was measured on macOS; the CI job measures on Ubuntu with the same NDK and rustc. The first hosted run (the pull request's merge commit, 2026-10-03) measured 905,896 bytes for
  arm64-v8a (+376, 0.04%) and 971,168 for x86_64 (−296): a few hundred bytes of path- and host-dependent constants (16 bytes move between two checkouts on this Mac, the `TypeId` constants of the
  note above); the 5% tolerance absorbs that a hundred times over. The `size-ios` job of the same run used `macos-15`'s Xcode 16.4 (the record: Xcode 26.6) and
  measured 793,533 bytes against 793,517; it ran for 2.5 minutes after waiting 36 for a macOS runner, which is the latency a macOS job adds to `Bench / All green`.
* `scripts/bench-device.sh --device android` fails on macOS when exactly one emulator is running (`[ "$(... | wc -l)" = 1 ]` is false because `wc -l` pads its count): `--target <serial>` is the way until that
  script is fixed (not this piece's file).
