# Architect: the web bundle size (E5, `wt/wasm-size`)

ADR: `.10x/adrs/ADR-052-web-bundle-size.md` (Proposed; the founder accepts it by reply). Brief: piece E5 of
`.10x/specs/2026-10-01-v1x-default-choice-design.md`, Amendment D.

## What was measured

* The published "135 KB" is the `undra init` template (to-do store, no queries) built by
  `undra build --platform web` (release-wasm + `wasm-opt -Oz`, binaryen 133), gzip level 9. Reproduced on
  `main` a0d638f: 353,446 bytes, **136,243 gzipped** (zlib level 9; Apple `gzip -9 -n` agrees, Node's zlib
  says 136,683, GNU gzip differs too: the gate names its compressor). "Web runtime" in the row is the Rust
  runtime inside the module: the row is the wasm alone.
* The JavaScript runtime is a separate number (the blueprint budgets it at ≤ 8 KB gz): a hello app ships
  72,659 bytes / **22,521 gzipped** of `@undra/runtime` (the runtime's own Vite 8; the template's Vite 6 app
  build says 84,512 / 25,882).
* Attribution with twiggy (names kept, `wasm-opt -Oz -g --strip-dwarf`): the top 20 are in the ADR. The two
  that mattered: sixteen driftsort monomorphs from the schema sorts (58 KB raw) and `undra-query`, linked into
  every core by its own `inventory` registrations (29 KB raw plus its generics; the only float formatting).
  Absent, verified: the diagnostics catalogue and compile-time E-code strings (only E0062, a runtime message
  by design), debug names and custom sections, std randomness.

## Decisions

* **Budget**: the wasm module alone, ≤ 120,000 bytes gzipped (zlib 9). Kept at 120, not tightened: the v1.x
  wire revision and ADR-044's function table land in this module; the 5% ratchet catches creep.
* **Lever A** (`perf(meta)`, two commits): one stable merge sort on indices per key type replaces sixteen
  `sort_by` instantiations, and lists of up to 16 items take an in-place insertion sort. Identical output
  (exhaustive and pseudo-random tests against `sort_by`), hash unchanged. 136,243 → 129,589 gz. A pure
  insertion sort saved 3 KB more but was quadratic (9 ms at 2,000 records; the merge path is 239 µs, `sort_by`
  205 µs). The insertion path for short lists was added after the budgets test showed the merge path's
  allocations cost ~4 µs of cold start; with it, cold start equals `main`'s (75.8 vs 75.3 µs).
* **Lever B** (`perf(query)`): `#[undra::query]` / `#[undra::mutation]` submit `undra-query`'s init hook and
  dispatch layer (`::undra::query::__private::{HYDRATE, LAYER}`); `undra-query` no longer submits them; the
  runtime runs a hook and consults a layer once per name. 129,589 → **95,838** gz. Chosen over a cargo
  feature: same bytes, no build matrix, nothing for `undra build` to detect. Side effect: a core without
  queries no longer reads `Kv` at start-up.
* **Not taken** (ADR lists the numbers): docs stripping (2.2 KB, ADR-050 stands), `opt-level = "s"` (+14 KB),
  `--converge` (0.3 KB), `--low-memory-unused` (unsound with the stack-first layout), a build-time hash
  (breaks R1/R7), a hand-written JSON writer, a smaller allocator, `--remap-path-prefix` (follow-up below).
* **R6 on the web**: panic → level-5 `log` import → trap → `UndraTransportError("trap")` in the TS runtime;
  pinned by `raw.test.mjs` and `ts-runtime.test.mjs`, which pass on the dev and the release-wasm build.
* **Gate**: `scripts/wasm-size.sh` (+ `scripts/web-size-runtime.mjs`), `[size."web/hello-wasm"]` in
  `bench/budgets.toml` (budget 120,000, record 95,712, tolerance 0.05 → ceiling 100,497), a `size` job in
  `bench.yml` with binaryen version_133 pinned, the record in `bench/results/web-size.jsonl`. The budgets
  parser learned the table; a unit test fails when the record and the table disagree.
* **Published number**: `site/scripts/build-numbers.mjs` writes the `web-size` row's value from the record
  and fills `<!--measured:NAME-->` slots in the site and README.md; the site workflow checks README.md too.

## Deviations from the brief

* **Commit trailer.** The brief asked for `Co-Authored-By: Claude Fable 5.1`; the session's attribution
  rule (and the model that wrote the commits) is `Claude Opus 5.5`, which the commits carry.
* **The record is 95,712, the per-lever numbers end at 95,838.** The levers were measured on a template in a
  scratch directory, the record on the one `scripts/wasm-size.sh` creates under `target/wasm-size/` (panic
  locations embed the build path: 70 bytes), and after the short-list insertion sort (another 56). On `main` (a different checkout path) and
  on the runner the number will move by tens of bytes again; well inside the 5%.
* **The roadmap entry is left to the merge.** On this branch's base no roadmap entry said 135 KB; `main` has
  since rewritten `site/data/roadmap.json` (afa4bfd) with a v1.x item "The web bundle under its budget
  (ADR-052): 135 KB gzipped today against a 120 KB budget; the levers, and a size gate so it never creeps
  back." Any edit here would conflict with that rewrite, so this branch does not touch the roadmap (open
  item 3 has the replacement text).
* **`doctor.rs` untouched.** Nothing in this piece needs a new diagnosis: the gate's own error names the
  missing `wasm-opt`.

## Open items

1. ADR-052 open decisions 1-3 (the JS runtime's budget: 22.5 KB against 8 KB; the 5% tolerance; no second
   landing card).
2. `--remap-path-prefix` for web builds: removes the builder's home directory from shipped binaries (privacy)
   and makes the size identical across machines; worth about 1-2 KB raw. Needs RUSTFLAGS for every crate of
   the web build, which `Build.rustc_args` (shim only) cannot do.
3. Merge notes for the integrator. (a) `git merge-tree` of this branch with `main` (7f1080c) is clean, and
   `node site/scripts/build-all.mjs` on the merged tree changes nothing.
   (b) Then move the roadmap item to shipped (or reword it in place), for example: "The web bundle under its
   budget (ADR-052): the hello-world wasm is down from 136 KB to 96 KB gzipped against its 120 KB budget,
   and CI measures it on every change so it stays there." (c) `wt/runtime-lifecycle` edits the body of
   `undra_query::init` right above the removed `inventory::submit!` and touches
   `undra-runtime/src/runtime.rs`: keep its `init` body, this branch's `__private` module and
   `run_init_hooks`'s once-per-name loop. (d) After each merge that touches the core, `scripts/wasm-size.sh`
   (the checkout path moves the number by tens of bytes; re-record with `--record` only if the gate asks).
   (e) The size job pins Rust 1.98.1 like the rest of CI on `main`; the deliberate bump re-records the size.
   (f) `crates/undra-cli/tests/schema_docs.rs` pinned a stale playground hash on this branch's base (it fails
   on a0d638f too); `main` fixed it independently in 3a2ff1f, so nothing to do.
4. `undra build` prints its gzip size with the system `gzip -9` (GNU on Linux, filename in the header), so its
   line can differ from the record by up to 1%; left as is (a CLI dependency on a deflate crate is not worth
   it).
