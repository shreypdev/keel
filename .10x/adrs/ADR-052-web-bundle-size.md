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
  code; the fix is to re-record (and to look at why). CI pins Rust (1.98.1, the version the record was
  measured with) and binaryen, so this happens only at a deliberate toolchain bump, which re-records in the
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
