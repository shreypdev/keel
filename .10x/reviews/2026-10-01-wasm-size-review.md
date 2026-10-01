# E5 web bundle size (ADR-052) — adversarial review

**Date:** 2026-10-01 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/wasm-size` at `cd298b2` (37 files, +1,590/−98 against its base `a0d638f`) · **Read:** `CLAUDE.md` (R1, R4, R6, R8,
R9, R11, R12), ADR-052, the architect record `.10x/decisions/architect/wasm-size.md`, ADR-050, SPEC 2.3, 7, 9, 14,
16.2, 16.3, and the diff (`undra-meta` `sort.rs`, `canonical.rs`, `registry.rs`; `undra-query` `lib.rs`, `dispatch.rs`,
`shared.rs`; `undra-runtime` `dispatch.rs`, `ext.rs`, `runtime.rs`; the macros' query expansion and snapshots; the
facade; `bench/src/budget.rs`, `bench/budgets.toml`; `scripts/wasm-size.sh`, `scripts/web-size-runtime.mjs`;
`bench.yml`, `site.yml`; `site/scripts/build-numbers.mjs`; README and site copies) · **Method:** a failing test or a
reproduction for every finding before its fix, two stability mutants of the sort, a mutant of the path remapping, an
end-to-end core with its queries in a dependency crate (built for the web and the host, its start-up port calls
observed in Node), the gate run twice, from a fresh target directory, over its record and without the JavaScript
toolchain, cold start measured in four alternating rounds against the base and against the branch with `sort_by`
restored · **Fixes:** `fee57a5`, `07923c3`, `2d6156d`, `619c956`, `1b5b302` and the ADR/record update before this file;
`main` merged twice: `38ea11d` at `52342d2`, then `fcbe221` (the API reference; site and workflows only) at
`5267c21`.

## Verdict

**Merge after fixes and one integrator decision.** The fixes are on the branch and `main` is merged; every suite
passes on the merged tree **except the size gate this piece adds, which fails on it, correctly** (D1): `main`'s
pieces that landed after this branch's base grew the hello-world wasm by 7.1 KB and `@undra/runtime` by 2.3 KB, so
the merged tree is 102,722 bytes gzipped (inside 120 KB, 7.4% over the pre-merge record) and the JavaScript runtime
24,841 bytes, over decision 2's 24 KB. Re-recording cannot fix that (the script refuses a record over budget), and
restating a budget the integrator just set is not the review's call: restate it (or land `ts-runtime-size` first),
then `scripts/wasm-size.sh --record` and `node site/scripts/build-all.mjs`. The two levers do what the ADR says. The
sort is a stable sort equal to `sort_by` on every input the property tests and an exhaustive test could produce,
equal keys included, and an equal-keyed schema hashes to the value `main` computed for it; the playground's hash is
unchanged. Lever B links the query runtime by use and keeps it linked wherever a query is declared, including in a
dependency crate of the core (built and run: the web core lists `Kv` at start-up, the hello world calls no port). The
gate measures what ships, deterministically, from the committed record, and cannot be updated by CI.

What was wrong is at the edges of lever B and in what the gate did not cover. **The repository's one hand-built query
broke** (M1): `bench/benches/query.rs` submits its `QueryRegistration` by hand, and without the layer its platform
benches panicked; nothing ran them. **A core whose queries are hand-written `QueryDef`s silently never hydrated**
(M2): its persisted entries and offline queue were never read, with no error. Both fixed: first-use hydration when
the start-up hook is not linked, the bench submits what the macros submit, and CI now runs every criterion bench once.
**Every release binary carried the builder's home directory** (M3, pre-existing, flagged by the architect): fixed in
the CLI for wasm, iOS, Android and host release builds. The JavaScript runtime was recorded but not gated, and a run
that could not measure it passed (M4): gated at 24 KB per the integrator's decision 2. No High finding; nothing
blocking is open besides D1.

## Findings

| # | Sev | Where (at `cd298b2`) | Finding | Status |
|---|---|---|---|---|
| D1 | **Blocking (decision)** | the merged tree; `bench/budgets.toml` `[size.*]` | After merging `main` (`38ea11d`), `scripts/wasm-size.sh` fails: `web/hello-wasm` 244,382 / **102,722** bytes gzipped, 2,254 over its ceiling (5% over the pre-merge record 95,684; still 86% of 120 KB), and `web/hello-runtime-js` 81,048 / **24,841**, 841 over decision 2's 24,000 budget. Not this piece's code: `main` alone (no levers) builds the hello world at 372,540 / 143,384 (136,243 at `a0d638f`; its README still says 135 KB), the merged wasm links no `undra-query` (no `undra-query` string in it), and the TypeScript runtime is `main`'s byte for byte (parity's closed `UndraCallError` set, `report`/`onError`, snapshot/restore, worker sync ports: +900 lines). | **Open (integrator).** Restate the JavaScript budget (25 KB leaves 159 bytes of headroom; at 26 KB the ceiling is the budget, 1,159 above the record) or land `ts-runtime-size` first; then `scripts/wasm-size.sh --record` records both, `node site/scripts/build-all.mjs` moves the README and site to 102.7 KB, and the ADR's numbers follow. The record and the published numbers stay at the pre-merge values until then, and the size job says why. |
| M1 | Medium | `bench/benches/query.rs:35` | The bench's `Count` query is built by hand (`QueryDef` + `inventory::submit! { QueryRegistration::of::<Count>() }`). Lever B moved the dispatch layer into the macros' expansion, so this binary links no layer: `cargo bench -p undra-bench --bench query -- --test` panics in `query/platform_construct_and_release` with `the constructor answers a handle: TrailingBytes { count: 26 }` (the reply is the runtime's "unknown object type"). `bench.yml` says criterion is not run in CI, so nothing noticed. | **Fixed** (`fee57a5`): the bench submits `__private::{HYDRATE, LAYER}` as the macros do (all four benches `Success`); the bench workflow runs `cargo bench -p undra-bench --benches -- --test` (each bench once, untimed). |
| M2 | Medium | `crates/undra-query/src/shared.rs:748` (`Shared::start`), `lib.rs:73-75` | A core whose only queries are `QueryDef`s written by hand links no start-up hook, and nothing else hydrates: `ctx.query().observe::<Q>(..)` of a `persist` query never sees what the last run stored, and an idempotent mutation's offline queue is never replayed. No error, no log. The crate docs told hand-writers to submit `__private::HYDRATE`, a `#[doc(hidden)]` item documented as "not a stable API". | **Fixed** (`fee57a5`, `1b5b302`): `Shared::start` (every `ctx.query()`, `ctx.mutate(..)`, a platform's handle constructor) hydrates when the hook is not linked, once per runtime, holding the runtime weakly like the hook; nothing changes when it is linked. `crates/undra-query/tests/hand_built.rs` (3 tests; 2 fail before the fix: zero `Kv` listings). Docs: crate docs, `QueryRegistration`, SPEC 9, ADR-052. |
| M3 | Medium | `crates/undra-cli/src/cargo.rs:520` (`build_library`) | Every release binary `undra build` ships names the builder's home directory: the hello-world wasm contains `/Users/<name>/...` 24 times (panic locations of the Undra crates of a checkout and of `~/.cargo/registry`), and the iOS and Android builds are built the same way. Pre-existing; the architect recorded it as a follow-up. | **Fixed** (`2d6156d`): non-dev builds pass `--remap-path-prefix=$HOME=~` (and a `CARGO_HOME` outside it `=/cargo`) to every crate through `--config build.rustflags=[..]`, merged by Cargo with the project's own; when `RUSTFLAGS` or `CARGO_ENCODED_RUSTFLAGS` is set (Cargo then ignores `build.rustflags`) the flags are appended to it; `--remap-path-scope=object` when rustc accepts it (1.98 does), so compiler messages keep real paths. Unit tests on a `FakeSys`; `tests/build_web.rs` asserts the module has no `$HOME` (fails with 24 hits when the remapping is disabled); the gate fails on it too. SPEC 7. |
| M4 | Medium | `scripts/wasm-size.sh:78-87,152-164` | The JavaScript runtime line was recorded with `"gated": false` and a blueprint budget of 8 KB it is 2.8x over; a run without `node_modules` wrote `"gzipped": null` and passed (this worktree's first run did exactly that). | **Fixed** (`619c956`) per the integrator's decision 2: `[size."web/hello-runtime-js"]` (24,000, record 22,521, ceiling 23,647), gated like the wasm; a run that cannot measure it exits 2 (verified: gate and `--record`, record files untouched); the budgets test requires a table for every recorded artefact. Follow-up piece `ts-runtime-size` (16 KB) in the ADR and the record. |
| L1 | Low | `scripts/wasm-size.sh:166-180` | `--record` wrote `bench/results/web-size.jsonl` before checking anything, and `measured_gzip_bytes` even when the build was over budget, leaving a record the budgets test rejects. | **Fixed** (`619c956`): a record is written only from a run that measured both artefacts within their budgets; each table's `measured_gzip_bytes` is replaced inside that table only. |
| L2 | Low | `site/scripts/build-numbers.mjs:93` | The slot pattern `<!--measured:NAME-->[^<]*<!--/measured-->` skips a slot whose number was edited into markup (`<!--measured:web-size--><b>80</b> KB<!--/measured-->`) or whose closer was mistyped, so the hand-written number survives `build-all` and CI's "up to date" check (reproduced: "build-numbers: up to date"). A plain-text hand edit was already caught. | **Fixed** (`619c956`): a file whose openers or closers do not all belong to whole slots is an error naming the shape a slot must have. |
| L3 | Low | `.github/workflows/bench.yml` (`size`) | The job puts binaryen 133 on `PATH` but never checks that the `wasm-opt` it runs is that one; the tarball is fetched without a checksum. | **Fixed** (version): the step fails unless `wasm-opt --version` says 133. **Open** (checksum): pin the tarball's sha256 (needs one download to compute; not done from this machine). |
| L4 | Low | `crates/undra-meta/src/sort.rs`; ADR-052 lever A ("cold start equals main's") | The schema sort is slower than `sort_by` in two shapes. Micro-benchmark on record-sized items (two runs): 8 to 16 unsorted items 1.4-2.3x (n = 16 reversed: 440 vs 195 ns), and reversed inputs past 16 items 3-6x (std detects a descending run; 1,000 reversed: 16.3 vs 2.5 us); random long inputs equal, sorted ones faster. In the budgets test's cold start the sort costs about 2.3 us: median of four alternating rounds 75.6 us at the base, 75.7 us at the branch, **73.4 us at the branch with `sort_by` restored**. "Back to main's" holds because lever B removed the hydration task from that row (its core declares no query), not because the insertion path made the sort free. | **Documented** in the ADR with the numbers; the trade (6.6 KB gzipped for ~2 us of a 3 ms budget) stands. |
| L5 | Low | `crates/undra-query/src/lib.rs` (lever B) | A core that removes its last query or mutation no longer reads `Kv` at start-up, so what an earlier version persisted (cache entries, the offline queue) is never deleted: before, the next start-up dropped it as written under another schema hash. Nothing readable is lost (no query reads it, no mutation can replay it); the bytes stay until a version with queries drops them. | **Documented** (SPEC 9, crate docs, ADR consequences) and pinned: `what_an_earlier_version_persisted_stays_in_the_store_unread` (`crates/undra/tests/query_linked_by_use.rs`). |
| L6 | Low | `crates/undra/tests/query_linked_by_use.rs:4` | The module doc points at `query_linked_by_use_with_queries.rs`, which does not exist. | **Fixed**: points at `crates/undra-query/tests/linked_by_use.rs` and `hand_built.rs`. |
| L7 | Low | after the merge: `crates/undra-query/src/lib.rs` (`init`), `shared.rs` (`start`) | Main's `init` and the first-use path each spawned their own async block around `hydrate` (two task types, two copies in every core with queries; the pre-merge playground built with the separate spawn was 2.4 KB raw above the architect's figure, not re-measured in isolation after). | **Fixed** (`1b5b302`): one `Shared::spawn_hydration`. |
| I1 | Info | `crates/undra-query/src/lib.rs` (`__private`), `crates/undra/src/query.rs` | `__private` is `#[doc(hidden)]` in both crates and says "not a stable API"; a user who names it compiles without a message. That is the convention of every other generated-code-only item here (`__undra_attach_all`, `__UNDRA_IS_OBJECT`); hand-built registrations are the one documented use. | Note. |
| I2 | Info | `crates/undra-runtime/src/{ext,dispatch}.rs` | "Once per name" is by name only: a different hook submitted under a name already used is silently skipped (ADR consequences say so). The two names in the tree are unique. | Note. |
| I3 | Info | `crates/undra-cli/src/cargo.rs` (M3's fix) | The remapping keeps the part of a path below the home directory (`~/Desktop/src/...`), does not touch paths outside it (`/tmp`, `/opt`), and is overridden by a project's own `target.<triple>.rustflags` (Cargo then ignores `build.rustflags`; SPEC 7 says to add the flag there). Changing rustflags rebuilds a release target directory once. | Note. |
| I4 | Info | commits | The brief asked for the trailer `Claude Fable 5.1`; the review's commits carry `Claude Opus 5.5`, the model that wrote them and the session's attribution rule (the implementer made the same call). | Note. |

## The attack on each surface

**1. Lever B's silent failure modes.**
*Queries in a dependency crate.* An `undra init` project whose core depends on a second crate holding a persisted
`#[undra::query]`, referenced only by `use depq_queries as _;`: `undra bindgen` (the dev host build, `dlopen`) lists
the `DepqPing` handle; `undra build --platform web` links `undra-query.hydrate` and the query's key (135.1 KB gzipped:
the query runtime is back, as it should be); the host dylib has both strings; instantiated in Node with the test
host of `crates/undra-ffi/tests/wasm/helpers.mjs`, that wasm's start-up makes one port call, `Kv.list("undra.query.cache.")`,
and the hello-world wasm makes none. The hook and the layer are submitted in the same expansion as the query's
`Registration::Query`, so they share its object file and its fate: a query that reaches the schema brings both.
*Without the macros.* A hand-written `QueryDef` used from Rust: never hydrated (M2, fixed). A registration
submitted by hand: unreachable from a platform without the layer (M1; documented on `QueryRegistration` and in
the crate docs, the bench fixed). The playground's Remote path serves the playground core, which declares its queries
with the macros. *Two crates, one name.* Every definition submits the identical `HYDRATE` / `LAYER` constants;
`once_per_name.rs` (three submissions of one hook and one layer plus a second name: one run, one consultation per
call, the other name runs too) and `linked_by_use.rs` (15 definitions, one `Kv` listing) pin it. *Start-up order.*
The dispatch table, layers included, is built from `inventory` when the runtime is constructed, before it is
published, so no call can reach a runtime without its layer; hooks run under the core lock in `Runtime::init` /
`new`. A call that reaches a global runtime between its publication and its hooks is served and observes an empty
entry; hydration then fills it (`hydration_that_arrives_after_an_observer_fills_an_entry_with_nothing_to_show`),
as on `main`. *Removing the last query:* L5.

**2. The sort.** Property tests (proptest, 2,048 cases each, now a dev-dependency of `undra-meta`): `by_name` equals
`sort_by(|a, b| a.name.cmp(&b.name))` and `by_index` equals `sort_by_key(|v| v.index)` on 0 to 80 items drawn from six
keys ordered as `str::cmp` orders them (byte-wise, so `"B" < "a" < "ü"`) and from narrow and full `u16` ranges, so
equal keys are the common case; a test walks 15 to 18 items (the last in-place lengths and the first merge-sorted
ones) sorted, reversed, all equal and alternating. Those are the only two comparators the schema code uses (the
diff replaces sixteen call sites, all `name.cmp` or `index`). An equal-keyed schema (duplicate names with different
contents in all ten sorted lists, from 3 to 40 items) canonicalizes exactly as a `sort_by` reimplementation of
`main`'s `canonicalized`, and hashes to `0xef9b4b0cdcd289c2`, the value computed by `main`'s own code at `a0d638f`; the
same lists reversed hash differently, so the fixture would see an unstable sort. Mutants: `<` instead of `<=` in the
merge fails 7 tests, `>=` instead of `>` in the insertion fails 5. `undra bindgen -C examples/playground --check
--docs` on the merged tree: up to date at `main`'s hash `0xddcdea47fa95a8d4` (bindings committed by `main`, computed
with `sort_by`; this tree computes them with the new sort). Performance: L4.

**3. The gate.** Two consecutive runs at `cd298b2`: byte-identical modules and identical JSON apart from the date
(95,711 gzipped; the record's 95,712 was taken one commit earlier, before a doc comment moved line numbers in
`sort.rs`, which panic locations carry). A run from a fresh `CARGO_TARGET_DIR` gives the record exactly (95,684 /
228,532 after the remapping). The template project is deleted and recreated on every run and `build/web` is written
by that run's `undra build`, so a stale artefact cannot be measured; the run refuses a module `wasm-opt` did not
touch. The ceiling is `min(budget, floor(record x 1.05))` from the committed `budgets.toml` (the `--record` path is
the only one that uses the fresh number), and CI runs the script without `--record`; the budgets test fails when the
JSON and the tables disagree. Over the record (record set to 90,000): exit 1, "1,184 bytes over its gate (5% over the
record)". Without the TypeScript runtime's `node_modules`: exit 2 for the gate and for `--record`, record files
untouched (M4, L1). The `size` job runs on `pull_request` (not `pull_request_target`), uses no secret and writes
nothing back, so a fork's pull request runs it with a read-only token and cannot touch the record. Binaryen: L3.
`build-numbers.mjs` fills slots only with the record's numbers; a plain-text hand edit is undone and fails CI's check
(README is covered), a markup edit was skipped (L2, fixed); two runs of `build-all.mjs` change nothing.

**4. Performance (R9).** Cold start, `UNDRA_BENCH_FILTER=cold_start cargo test -p undra-bench --test budgets
--release`, four rounds alternating three trees: base `a0d638f` 76.50 / 75.38 / 75.71 / 75.21 us; branch 75.62 /
75.75 / 74.42 / 76.38 us; branch with `sort_by` restored 72.79 / 74.00 / 75.25 / 72.79 us (the core-thread row, 81-86
us, does not separate them). So the claim "75.8/76.4 vs 75.3/75.7" reproduces, and the sort's own cost is in L4.
The playground's wasm, `undra build -C examples/playground --platform web` in scratch copies: base 573,863 /
**218,575** gzipped, branch 526,917 / **212,199** (the architect's 218,487 → 212,003: the claim holds; the base
differs by the checkout path in panic locations, the branch also by this review's first-use hydration, see L7). After the merge with `main`: `main` alone 591,974 /
226,344, merged 543,096 / **219,972** (lever A is still worth 6.4 KB gzipped there). The hello world after the merge:
D1.

**5. `--remap-path-prefix`.** M3. Done on the branch: about an hour with the tests. Hello-world: −280 bytes raw,
−27 gzipped.

**6. Everything else.** See the verification section. New `pub` items are documented (`PathRemap`,
`Cargo::path_remap`; the implementer's `SizeBudget` and the size parser); no `println!` in library code; no new
dependency in a library crate (`proptest` is a dev-dependency of `undra-meta`, already in the workspace). The
`__private` module: I1. Docs in the binary (≈ 2.2 KB gzipped, ADR-050's measurement) were not re-measured. R6 on the
web: the raw and TypeScript-runtime wasm suites pass on the dev and the release-wasm build (below), including "a
panic logs at level 5 through the host, then traps" and the transport's typed `"trap"`.

## Verification (after the fixes and the merge, `1b5b302` plus the documentation commits)

* `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy -p undra-ffi
  --target wasm32-unknown-unknown -- -D warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`:
  clean.
* `cargo test --workspace --no-fail-fast`: 138 suites, **2,426 passed**, 0 failed, 11 ignored (`main`: 2,400). New
  here: `hand_built` 3, `what_an_earlier_version_persisted_stays_in_the_store_unread`, the two sort property tests and
  the threshold test, the equal-keyed schema, three `path_remap` tests (plus the implementer's).
  `cargo test -p undra-ffi --release --test sync_alloc --test commit_alloc`: 2 and 5 passed.
* `crates/undra-ffi/tests/wasm/run.sh`: raw 19 + TypeScript runtime 24 passed on the dev build and on
  `PROFILE=release-wasm`.
* `cargo test -p undra-bench --test budgets --release`: 6 passed, 1 ignored (`snapshot/cold_start_restore_100kb`
  74.8 us, budget 340; `dispatch/call_sync/add` 44.3 ns; `signals/changeset_100/runtime` 2.19 us; `signals/set_attached`
  8.5 ns); `cargo bench -p undra-bench --benches -- --test`: every bench `Success` (the query bench panicked at
  `cd298b2`, M1).
* TypeScript runtime `npm test`: **1,102 passed** (32 files). Playground web: `npm test` 112 passed (7 files), `npm
  run build` clean.
* `contract-tests/run-all.sh`: **18 x 3 = 54/54**.
* `undra bindgen -C examples/playground --check --docs`: up to date, `0xddcdea47fa95a8d4`.
* `node site/scripts/build-all.mjs`: nothing to regenerate on a second run; `node site/scripts/check-links.mjs
  --words`: ok, landing prose 342 words (budget 350).
* `scripts/wasm-size.sh`: at `cd298b2` twice (identical), from a fresh target directory (identical to the record),
  over the record (exit 1), without `node_modules` (exit 2, record untouched); after the fixes, recorded 95,684 /
  22,521; on the merged tree, **fails** (D1).
* The second merge (`5267c21`) changed no code: `bash site/scripts/build-rustdoc.sh` (main's rustdoc with `-D
  warnings`), `node --test site/scripts/decls.test.mjs` (10 passed), `build-all.mjs` (nothing to regenerate) and
  `check-links.mjs --words` pass on it; the suites above stand.
* Kotlin and Swift runtime suites were not run separately (the contract columns ran both over the playground core);
  Miri and ASan were not run (no `unsafe` and no FFI change here).

## Open items for the integrator

0. **D1 (blocking):** restate `[size."web/hello-runtime-js"]` (or land `ts-runtime-size`), then `scripts/wasm-size.sh
   --record`, `node site/scripts/build-all.mjs`, and the ADR's "after the merge" numbers become the record.
1. L3: pin the binaryen `version_133` tarball's sha256 in the `size` job.
2. Piece `ts-runtime-size`: what a hello app ships of `@undra/runtime` from 22.5 KB to 16 KB gzipped, lowering
   `[size."web/hello-runtime-js"]` in the same commit (levers in ADR-052's decision 2).
3. The roadmap item "The web bundle under its budget (ADR-052)" is reworded in place without numbers (they move with
   D1); moving it to Shipped is the integrator's call at merge. If D1 restates the JavaScript budget, the README's
   "against a 24 KB budget" sentence follows it.
4. `.10x/status.md` / `handoff.md`: release builds now remap the home directory (SPEC 7); the criterion benches run
   once in the bench workflow; the JS runtime is gated at 24 KB.
