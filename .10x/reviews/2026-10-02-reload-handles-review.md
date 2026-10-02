# `reload-handles` (ADR-059: query handles across every restore) — adversarial review

**Date:** 2026-10-02 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:** `wt/reload-handles` at
`a9bd725` (38 commits from `main` `a309e9f`), reviewed against `main` `12dafe2`, then merged with `main` `fc326d6` (the CI piece),
`1e8f33c` (diagram-rn) and `ae362ac` (generics-fn-obj: S34, so the grid is 35 scenarios and 101 cells) · **Fixes:** one review-test
commit, eight `fix(reload-handles): review fixes` commits (two of them for CI red on the first pushed head), three merges, two `bench`
commits (the size record), this record · **Read:** `CLAUDE.md` (R1-R12),
ADR-059 with its implementation note, `.10x/decisions/architect/reload-handles.md`, `.10x/decisions/sde/reload-handles.md`, ADR-022,
023, 037, 040, 049, 053, 054 where they meet the piece, SPEC 1.1, 5.4, 5.9, 9, 16.2, 17.1, and the diff: `undra-wire` (the reserved
field), `undra-runtime` (`recreation.rs`, `object.rs`, `object_table.rs`, `runtime.rs` snapshot/restore/observe/object, `config.rs`),
`undra-query` (`revive.rs`, the record, `check_params`), `undra-transport` (the hub's `stores_of`, the displaced count), `undra-cli`
(runner protocol, `restore_state`, `take_snapshot`, `Outcome::describe`), the TypeScript runtime and `undra-bindgen` (the ADR-049 replay
removed), goldens, S22, S35 on three columns and the React Native model, `dev_reload.rs`, `dev_devtools.rs`, the playground, the site.

## Verdict

**Sound; merge once CI is green on the pushed head.** The mechanism does what ADR-059 says on every path the review attacked, and no
blocking defect was found. The in-band record degrades exactly as decided on a core without this branch (proved against `main`'s code:
the store comes back, the record is `dropped` with a WARN, the handle is stale). A crafted record never panics and never fails a restore
it should not: header damage gives a typed `RestoreError` that changes nothing, or a restore that keeps the stores; record-body damage is
refused (stale) or re-issued and builds on use. Dormant handles are built exactly once under concurrent first uses; host references
return to their baseline across restore, use, release and restore again; a restore makes no port call and arms no timer, on the same
runtime and on a fresh one. The first pushed head (`f7f31cb`) was red in two CI jobs, neither in the piece's code, and both are fixed
at the cause: a **High** pre-existing race in `undra-ffi`'s port registry that let a removal return while a callback of the port still
ran (the host may then free `user` under it; the hosted ASan job hit it) (H1), and a TypeScript Db test bounded by vitest's 5 s
default (L5); the local matrix found a third test of the same kind in React Native (L6). Two **Medium** findings of the piece: the
hello-world wasm grew **+1,541** bytes gzipped against `main` on the same toolchain (the note said +1,060 against a stale record;
decision 7 accepted +1.3 KB), accepted within the gate and recorded (M1); a schema-change reload test that took the first `Restarted:`
line (M2, fixed). The lows are fixed (L1, L2) or are the documented design of ADR-059 (L3). Every fix that changes behaviour has a
test that failed before it.

## Findings

| # | Sev | Where | Finding | Status |
|---|---|---|---|---|
| M1 | Medium | `crates/undra-runtime/src/runtime.rs:2497` (`object::<T>`), `:3222` (`restore_with_report`), `:2259` (`observe`), `:3079` (`snapshot`), `:3509` (`stats_json`) | `scripts/wasm-size.sh` on `main` `12dafe2` and on the branch, same machine, rustc 1.99.0, wasm-opt 133: **116,208 -> 117,749** gzipped (+1,541; raw module +3,284). ADR-059's note 8 reported +1,060 because it compared with the record (116,690), which is not `main` on this toolchain; the architect's decision 7 accepted "up to +1.3 KB". The link-by-use requirement itself holds: none of `recreation.rs`'s strings (`is not re-issued`, `could not be built again`) is in the hello-world module. The growth is the always-linked part (twiggy, raw, hashes stripped): `undra_restore` +795, `Runtime::object::<T>` +272 **per object type** (it stopped being inlined: a cost that grows with every object type of an app), `undra_observe` +248, `undra_snapshot` +204, `undra_stats_json` +118, `RestoreReport`'s drop +98. A non-generic loop for the miss path was tried and measured worse (+77), so it was reverted. | **Accepted and recorded.** The ADR's note 8 carries a dated correction (`ecdc465`); CI now pins rustc 1.99.0, so the record follows the piece as ADR-059's Risks asked (`9fa65ed`, then `7b5d1f7` after the merge with generics-fn-obj: **118,409** / 22,067 gzipped, gate 120,000 / 22,100; the site and README numbers regenerated). Two ways to take the per-type cost out of `object::<T>` were built and measured, and both were larger: a non-generic loop (+77 gzipped on the module) and a non-generic build handing back an `Arc<dyn Any>` for one downcast (`object::<Todos>` 813 raw bytes against 272); so the cost stays, measured: 272 raw bytes per object type that a core resolves. |
| M2 | Medium | `crates/undra-cli/tests/dev_reload.rs:1072` (`a_query_whose_parameter_type_the_edit_changes_is_not_carried_over_and_the_rest_is`) | The test edits the schema and takes the first `Restarted:` line. On a loaded machine the watcher can see the save in two bursts and rebuild once before the write (a restart that keeps the schema), so the test reads the wrong restart: `wt/ci-green` (`37af049`, `96bab2b`) replaced exactly this in the two older schema-edit tests with `Dev::wait_restart_changing_schema`. | **Fixed** (`ef67ab2`) after the merge brought `Dev::wait_restart_changing_schema` in. |
| L1 | Low | `crates/undra-cli/templates/runner/main.rs:379` (`take_snapshot`) | Counting stores and query handles now decodes the whole snapshot (every signal value copied into a `Vec`) where it read one word: up to 16 MiB decoded once more per reload in a debug runner. Correct, and dev-only. | **Fixed** (`f8a989f`): `Snapshot::count_records` reads the record headers with borrowed slices; its test agrees with decoding and fails at every cut length; the runner uses it. |
| L2 | Low | `crates/undra-runtime/src/recreation.rs:310` (`place`) | A record whose handle collides with another record at the same index (two records naming one slot under different generations) is skipped at DEBUG and counted nowhere. Only a forged snapshot has it (one slot holds one entry when a snapshot is taken); exact duplicates and a record on a store's handle are refused whole as `RestoreError::BadHandle` before anything is touched, like a forged store. | **Fixed** (`0dfcab9`): `place` refuses a record whose slot another entry of the snapshot holds (a store or an earlier record), with a WARN and a `refused` entry; a slot reused since the snapshot by a kept handle stays the DEBUG case. Test `a_record_that_names_the_slot_of_another_entry_of_the_snapshot_is_refused` failed before (nothing refused). |
| L3 | Low | `runtimes/ts/@undra/runtime/src/recovery.ts` | With the ADR-049 replay gone, a query handle created after the last kept crash-recovery snapshot (at most `snapshotEveryMs`, 1 s by default, before a trap) is stale after a restart, where the replay re-created it. ADR-059's path table says so ("as a store created then is lost") and SPEC 17.1 lists it under Lost. | A documented limit of ADR-059 (its path table and SPEC 17.1 "Lost"), not a defect. |
| L4 | Low | `crates/undra-macros` `tests/compile_fail.rs` | `diagnostics_render_as_documented` and `leaf_types_without_their_feature_name_it` failed locally under rustc 1.99.0 before the merge (the goldens were 1.98.1's). The crate has no diff in this piece. | Resolved by `main` `fc326d6` (the 1.99.0 bump regenerated the goldens). |

| H1 | High | `crates/undra-ffi/src/registry.rs` (`Registry::remove`, `install`, `retire_all`) | Pre-existing, not the piece's code; red on this branch's first push (CI run 37064975714, the `undra-ffi under ASan` job, `n1_two_removers_of_one_port_both_wait`). A removal took the registration out of the map, released the map's lock, and only then put it on the draining list. A second removal of the same port that ran in between found it in neither place and returned at once while a callback of that registration was still running: `undra_port_register`'s contract lets the host free its `user` pointer when the call returns, so the callback could use freed memory. | **Fixed** (`2c58dcc`): `Registry::take` retires and publishes under the map's write lock (order: the map, then the draining list; waits stay outside both). `n1_a_remover_that_finds_the_map_empty_while_the_winner_publishes_still_waits` forces the window with a test-only hook and failed before the fix (the loser returned with a callback running); 5 repeated runs and the release build pass. |
| L5 | Low | `runtimes/ts/@undra/runtime/test/support/db-suite.ts` ("carries a 2 MB row whole") | Pre-existing; red on the first push (the `TypeScript runtime` job): 5,188 ms on the hosted runner against vitest's default 5,000 ms. | **Fixed** (`ab52df4`): a 60 s hang guard; the test asserts what arrives. |
| L6 | Low | `runtimes/rn/@undra/react-native/test/transport.test.ts` (five tests) | Pre-existing: queue a record, sleep 5 or 10 ms, assert it was applied. Failed once in this review's matrix on a loaded machine (`expected [] to deeply equal [[0, 7]]`). | **Fixed** (`fcbd3ae`): each retries its check until it holds (`vi.waitFor`, 10 s). |

## The attacks, surface by surface

**1. The record in the snapshot (R7, data integrity).**
* *A core without this branch.* `main`'s tree (`git archive main`, its own target directory) restored a snapshot the branch wrote with a
  store (`Tally`) and a live `TodosQuery` handle: `RestoreReport { restored: 1, dropped: [DroppedStore { type_id: 0x54209c7c, handles:
  [the query's] }] }`, the WARN `restore: left out 1 store(s) of '0x54209c7c', a type this build does not have`, the store's value back
  (41), `refetch` on the query's handle status 5 (stale, as before ADR-059). P1 holds for real bytes, not only a hand-built record.
* *Crafted records* (`recreation.rs` `a_damaged_record_header_is_refused_typed_or_left_out_and_nothing_panics`, new): every byte of the
  record's header and body inside an encoded snapshot (handle, type, field count, field id, length, bytes), set to 0x00, 0xff and one bit
  flipped, restored into a runtime that holds a live, observed handle of its own: either `Decode`/`BadHandle`/`GenerationFloor` with the
  table and the next snapshot exactly as before, or success with the store restored and the runtime's own handle untouched; no panic, no
  build. `restore.rs` `every_damage_to_a_query_handles_record_is_refused_or_built_and_nothing_panics` (new): every byte of a real query
  handle's record (with a polling interval) changed three ways and cut at every length, plus one byte too many: the restore always
  succeeds, the record is refused (handle stale) or re-issued and then built by `observe` and `refetch`; no panic, no failed build (what
  passes `check` builds). Unknown query, fingerprint mismatch (ADR-037), parameters that do not decode: refused, counted and named in a
  WARN (the SDE's `a_record_that_cannot_be_honoured_is_refused_and_counted_and_the_rest_is_restored`, checked). A duplicate handle, a
  record on a store's handle, generation 0 or at the ceiling: refused whole, typed, nothing changed (`a_forged_record_fails_the_restore_
  like_a_forged_store_and_changes_nothing`, checked; it is the existing rule for stores, which ADR-059's brief asked for). A generation
  above the floor raises the counter (`the_generation_counter_rises_over_the_records_of_a_snapshot_too`).
* *The reserved id cannot come from a user schema.* A store's signal ids are its slots in order: `StoreCell::install`
  (`crates/undra-signals/src/store.rs:446`) refuses any `signal_id` other than the number attached so far (and `u32::MAX`), so a store
  would need 4,294,967,294 attached signals before one could carry `0xFFFF_FFFE`; the macros number fields from 0; a snapshot writes the
  cell's own ids. A store record is a recreation record only with **exactly one** field under that id
  (`only_exactly_one_reserved_field_makes_a_recreation_record`).

**2. Dormant entries and first use.**
* `first_uses_at_the_same_time_build_a_dormant_handle_exactly_once` (new): 16 rounds of eight threads behind a barrier, four observing
  and four calling `read` on one dormant handle: one build every round, every read and every observed value the built object's, one
  entry, one reference, no panic. (`observe` and dispatch both hold the core lock; `revive_handle` takes it when its caller does not.)
* `refetch` before any observe builds (SDE's tests, checked: `every_entry_point_..`, and the query test's `refetch` on a never-observed
  handle). `release` of a dormant handle forgets it unbuilt; `observe(.., false)` builds nothing.
* `host_references_are_exact_across_restore_use_release_and_restore_again` (new): `live_handles`/`host_refs`/`dormant_handles` from
  `[0,0,0]` through restore `[1,1,1]`, observe `[1,1,0]`, a same-runtime restore `[1,1,0]`, release `[0,0,0]`; a restore of a handle
  released since `[1,1,1]`, release `[0,0,0]`; a second reference on the live object kept by a restore and both releases needed.
* A restore of the runtime that holds the handle leaves it alone (`a_runtime_that_holds_a_handle_keeps_it_through_any_restore`,
  `a_restore_does_not_reset_the_polling_of_a_live_handle`, checked). **R12** (`a_restore_calls_no_port_and_arms_no_timer`, new): a
  counted `Clock` and `Timer` bound before start; a polling query, one with its own interval and one released after the snapshot;
  three same-runtime restores and one fresh one: zero clock reads, zero timers through the port or on the fake clock, the runtime's
  `pending_timers` unchanged (no second poll timer), no request; the ticker then fires once per interval, the dormant one never.

**3. Every restore path.** Dev reload (`dev_reload.rs` `a_query_handle_and_a_paged_list_keep_working_across_a_rebuild`: the ticker's
handle answers the reconnect's observe, `refetch` status 0, ticks from the new process, the `Library` pages through its new server, notice
`Reloaded, state kept`); a migratable schema change (`a_query_whose_parameter_type_..`: refused, counted in the notice, store kept);
devtools time travel to before the handle (`dev_devtools.rs`, `undra-transport` `a_time_travel_leaves_a_live_query_handle_alone_..`);
web crash restart (S22 step 4 asserts the handle after the trap **is** the handle before it); `core.restore` (S35 steps 2-8, three columns
and the RN model); cold start from a saved snapshot (S35 step 10 on a fresh core of build B; `undra-query` fresh-runtime tests). Each of
these fails on `main` (status 5 / a new handle before ADR-059). Infinite queries (all pages kept on the same runtime; first or persisted
pages on a fresh one), the observer's own polling interval (fake clock), the `Lazy<T>` server through op 0 (dev reload, S35 step 6/10):
covered by the SDE's tests, read and run.

**4. The TypeScript removal and the size.** `recreate`, `RecreateCall`, `_rebindObject`, `_recreatable`, `track` and `#recreatable` are
gone from `runtimes/`, `examples/`, `crates/undra-bindgen/`, `contract-tests/`, `site/reference/` and SPEC (grep); `#reattach` remains as
the re-observe step without its re-creation loop, as decided. The generated query-handle classes lost the `args` parameter and the
`recreate` block and nothing else (goldens: `decimal`, `full`, `infinite`, `polling`, `queries`, `stdlib`; `undra bindgen --check` on
the generated examples). Sizes: see M1 for the wasm; the up-front JS is **22,067** against `main`'s 22,100 (gate 22,100).

**5. Honesty of the surfaces.** The terminal line `state kept (2 stores, 1 query handle, ..)` (and `1 query handle` alone for a core
with no store), the notice `Reloaded, state kept` with `(N objects not carried over)` counting only what is stale, the WARN
`restore: Handle(..) (0x..) is not re-issued: <reason>`: as the ADR and its note say (`reload.rs` and `runner.rs` units, the dev tests).
The hub filters records out of `stores_of` and never observes one (`the_hub_of_a_reloaded_core_does_not_build_the_re_issued_query_
handles`). The post's dev-loop cell and list item, the roadmap move, `claims.md` M06-N5 / D10 / O35 point at ADR-059, DEV_LOOP and the
tests that prove them.

**6. Machine-speed independence.** No new test asserts an absolute time without a bound that only fails when the event never happens:
the waits for ticks, pages and notices return when the condition holds (20-60 s ceilings); absence checks are quiet windows that pass
vacuously on a slow machine; the two lock-ordering tests use 60 s deadlock detectors. The review's concurrent-build test races threads
against each other, never the test thread. Throttled run (`taskpolicy -b`, 16 `yes` burners, `--test-threads=4`; the machine also carried about 48 `yes` processes other
sessions had left running, so the load was far above the brief's): `undra-runtime` `recreation` 27/27 (418 s, the property tests
included), `undra-query` `restore`/`wire`/`hand_built` 17 + 22 + 4, `undra-ffi` `abi` (the refused-record case) pass. What failed under
that load failed in shared harnesses, not in the piece's assertions: the two new `undra-transport` devtools tests stopped at the test
client's 5 s reply wait in `open_query` (`tests/common/mod.rs:780`), as six older tests of the same file did in the same run; the new
`dev_devtools` test stopped at `devserver.rs`'s 600 s cold-build deadline, which `wt/ci-green` raises to 900 s and serialises; the
older `runner::tests::sixteen_mib_of_state_crosses_the_pipes_both_ways` hit its 60 s wait. Unthrottled, all of them pass. The one
fragility of the piece's own is M2.

## Counts

**On the final head** (after the merges with `main` `fc326d6`, `1e8f33c` and `ae362ac`; rustc 1.99.0, which CI now pins; Node 24):

* Rust workspace (`cargo test --workspace --no-fail-fast`): **3,689 passed, 0 failed**, 23 ignored. The review added 9 Rust tests
  (4 in `recreation.rs`, 2 in `restore.rs`, 1 in `registry.rs`, 1 unit and 1 doc test of `Snapshot::count_records`).
* TypeScript runtime **1,864**; React Native **110** (and its typecheck); Swift runtime **870**; Kotlin runtime **881** run (882 cases,
  1 skipped) + 32 (testkit). These are the numbers `site/data/tests.json` now carries.
* Contract grid (`contract-tests/run-all.sh`): **101/101** (TypeScript 35, Kotlin 33, Swift 33; S34 and S35 pass on all three). The
  Kotlin column's S30 failed once (`the cache entry of the list to be written within 100 ms of Background`) while about 60 `yes`
  processes other sessions had left behind were running on this Mac, and passed on the rerun once they had gone; the same 100 ms bound
  is in all three columns' S30 and passed on the hosted runners: recorded as an observation for the CI ledger, not a finding of this
  piece. fmt, clippy `-D warnings`, `cargo doc -D warnings`, `undra bindgen --check --docs` on five packages, the hello-world size
  gates and the site build pass; the registry tests pass under Miri.

**Before the merges** (the head the review attacked, `a9bd725` + the review's tests): Rust 3,603 passed (2 `undra-macros` goldens
failed under 1.99.0, L4); fmt, clippy (host and wasm32 `undra-ffi`), `cargo doc -D warnings` clean; the grid 98/98 (Swift's S33 failed
once, the flake `wt/ci-green` `000eed4` fixed, and passed on the rerun; the Kotlin column again under CI's Kotlin 2.0.21: 32/32); TS
1,862 + typecheck; RN 110, typecheck, contract model 25 passed / 2 skipped, `cpp/test/run.sh`, the Android library's Java; Swift 870;
Kotlin 882 + 32 under brew's 2.4.20 and CI's 2.0.21; the wasm, C (ASan) and Swift ABI suites; `schema_retention` + `schema_docs`;
`undra bindgen --check --docs` on five packages; derived vectors; the playground web build; two-cores JVM and Node; `undra-bench`
budgets 6 passed; site `build-all` and `check-links --words` (347 words); `main`'s code restoring the branch's snapshot 1/1.
`scripts/ci-local.sh` (every CI, Bench, Two cores and Site job on a clone) was green on `f7f31cb`.

## Sizes

* `web/hello-wasm`: **118,409** gzipped on the final head (`main` `ae362ac` with this piece), recorded; this piece's own share,
  measured on `main` `12dafe2` (116,208) against the branch (117,749), is +1,541 (M1). Gate 120,000: 1,591 to spare.
* `web/hello-runtime-js`: **22,067** (the ADR-049 replay is gone; `main` 22,100); gate 22,100.

## The merges with main, CI

* `fc326d6` (the CI piece): conflicts in the SDE index, `contract-tests/README.md` and `site/search-index.json` (regenerated); the
  dev tests inherit its turn-taking and 900 s build deadline; M2 fixed after it.
* `1e8f33c` (diagram-rn): no conflicts.
* `ae362ac` (generics-fn-obj, S34): S34 and S35 reconciled in `scenarios.md`, the README, `check.sh`, `run-all.sh`, the Kotlin and
  Swift runners and NOTES, the React Native include list and SPEC 14 (35 scenarios, 101 cells); generated bindings of the playground
  and two-cores a/b regenerated with `undra bindgen --docs` (schema hash `0x3c17cd5f59bb6f68`, which the testkit fixtures record);
  both open roadmap items closed on their sides; site rebuilt.
* First push `f7f31cb`: Site, Bench and Two cores green (runs 37064975649, 37064975588, 37064975573); CI red in two jobs (run
  37064975714): H1 and L5, both fixed. The landing gate is CI, Bench, Two cores and Site green on the final pushed head, whose run ids
  are in the reviewer's hand-off (this file is part of that head).

## Open items

None of the piece's. L3 is ADR-059's documented limit. `proto/reload-handles` (the architect's prototype branch) exists only as a local
branch of the main checkout (not on `origin`, no worktree) and can be deleted at the merge.
