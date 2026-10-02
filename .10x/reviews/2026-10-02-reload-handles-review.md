# `reload-handles` (ADR-059: query handles across every restore) — adversarial review

**Date:** 2026-10-02 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:** `wt/reload-handles` at
`a9bd725` (38 commits from `main` `a309e9f`), reviewed against `main` `12dafe2`, then merged with `main` `fc326d6` (the CI piece) and again with `1e8f33c` (diagram-rn; no conflicts, the generated site files unchanged by a rebuild) ·
**Fixes:** one review-test commit, two `fix(reload-handles): review fixes` commits, the merge, one `bench` commit (the size record),
then this record · **Read:** `CLAUDE.md` (R1-R12),
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
runtime and on a fresh one. The review added five tests, a dated correction of the size figure in the ADR's implementation note,
and one test fix. Two **Medium** findings, neither blocking: the hello-world wasm grew **+1,541** bytes gzipped against `main` on the
same toolchain (the implementation note said +1,060 against a stale record; the architect's decision accepted +1.3 KB; the 120,000
gate holds with 2,251 to spare), accepted and now recorded (M1); and the new schema-change reload test took the first `Restarted:`
line, which `wt/ci-green` had just replaced in the two older schema-edit tests because a loaded runner can rebuild twice (M2, fixed
after the merge with `main`). Low items are open follow-ups.

## Findings

| # | Sev | Where | Finding | Status |
|---|---|---|---|---|
| M1 | Medium | `crates/undra-runtime/src/runtime.rs:2497` (`object::<T>`), `:3222` (`restore_with_report`), `:2259` (`observe`), `:3079` (`snapshot`), `:3509` (`stats_json`) | `scripts/wasm-size.sh` on `main` `12dafe2` and on the branch, same machine, rustc 1.99.0, wasm-opt 133: **116,208 -> 117,749** gzipped (+1,541; raw module +3,284). ADR-059's note 8 reported +1,060 because it compared with the record (116,690), which is not `main` on this toolchain; the architect's decision 7 accepted "up to +1.3 KB". The link-by-use requirement itself holds: none of `recreation.rs`'s strings (`is not re-issued`, `could not be built again`) is in the hello-world module. The growth is the always-linked part (twiggy, raw, hashes stripped): `undra_restore` +795, `Runtime::object::<T>` +272 **per object type** (it stopped being inlined: a cost that grows with every object type of an app), `undra_observe` +248, `undra_snapshot` +204, `undra_stats_json` +118, `RestoreReport`'s drop +98. A non-generic loop for the miss path was tried and measured worse (+77), so it was reverted. | Accepted (within the gate); the ADR's note 8 carries a dated correction (`ecdc465`). Now that CI pins rustc 1.99.0 (`main` `fc326d6`), the record follows the piece as ADR-059's Risks asked: `scripts/wasm-size.sh --record` after the merge, 117,749 / 22,067 against `main`'s record 116,181 / 22,100 (`9fa65ed`; the site and README numbers regenerated). Open: make the miss path of `object::<T>` cost nothing per type (a non-generic build that hands back the built `Arc<dyn Any>` for one downcast), and re-measure. |
| M2 | Medium | `crates/undra-cli/tests/dev_reload.rs:1072` (`a_query_whose_parameter_type_the_edit_changes_is_not_carried_over_and_the_rest_is`) | The test edits the schema and takes the first `Restarted:` line. On a loaded machine the watcher can see the save in two bursts and rebuild once before the write (a restart that keeps the schema), so the test reads the wrong restart: `wt/ci-green` (`37af049`, `96bab2b`) replaced exactly this in the two older schema-edit tests with `Dev::wait_restart_changing_schema`. | **Fixed** (`ef67ab2`) after the merge brought `Dev::wait_restart_changing_schema` in. |
| L1 | Low | `crates/undra-cli/templates/runner/main.rs:379` (`take_snapshot`) | Counting stores and query handles now decodes the whole snapshot (every signal value copied into a `Vec`) where it read one word: up to 16 MiB decoded once more per reload in a debug runner. Correct, and dev-only. | Open: count records by walking the record headers without copying values. |
| L2 | Low | `crates/undra-runtime/src/recreation.rs:310` (`place`) | A record whose handle collides with another record at the same index (two records naming one slot under different generations) is skipped at DEBUG and counted nowhere. Only a forged snapshot has it (one slot holds one entry when a snapshot is taken); exact duplicates and a record on a store's handle are refused whole as `RestoreError::BadHandle` before anything is touched, like a forged store. | Open (cosmetic): count it in `refused`. |
| L3 | Low | `runtimes/ts/@undra/runtime/src/recovery.ts` | With the ADR-049 replay gone, a query handle created after the last kept crash-recovery snapshot (at most `snapshotEveryMs`, 1 s by default, before a trap) is stale after a restart, where the replay re-created it. ADR-059's path table says so ("as a store created then is lost") and SPEC 17.1 lists it under Lost. | Accepted as decided; noted because it is the one case the web loses that it used to keep. |
| L4 | Low | `crates/undra-macros` `tests/compile_fail.rs` | `diagnostics_render_as_documented` and `leaf_types_without_their_feature_name_it` failed locally under rustc 1.99.0 before the merge (the goldens were 1.98.1's). The crate has no diff in this piece. | Resolved by `main` `fc326d6` (the 1.99.0 bump regenerated the goldens). |

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

All on this machine (rustc 1.99.0; CI pins 1.98.1), on the branch head before the merge with `main`:

* Rust workspace (`cargo test --workspace --no-fail-fast`): **3,603 passed**, 21 ignored, 2 failed: L4 (`undra-macros` compile_fail
  goldens under 1.99.0; no diff in the piece). The review added 5 (3 in `recreation.rs`, 2 in `restore.rs`).
* `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo clippy -p undra-ffi --target wasm32-unknown-unknown
  -D warnings`, `cargo doc --workspace --no-deps` with `-D warnings`: clean.
* Contract grid (`contract-tests/run-all.sh`): **98/98** (TypeScript 34, Kotlin 32, Swift 32; S35 passes on all three). Swift's S33
  failed once (`the gap between the first two ticks was 0.0 s`), the flake `wt/ci-green` `000eed4` fixes, and passed on the rerun.
  Kotlin column again under CI's Kotlin 2.0.21 (own build directory): 32/32.
* TypeScript runtime 1,862 tests (66 files) + typecheck; React Native 110 tests, typecheck, the contract model 25 passed / 2 skipped,
  `cpp/test/run.sh`, the Android library's Java; Swift runtime 870 XCTest; Kotlin runtime 882 + 32 (testkit) under brew's 2.4.20 and
  CI's 2.0.21.
* wasm ABI (`crates/undra-ffi/tests/wasm/run.sh`), C ABI under ASan, Swift over the C ABI, `schema_retention` + `schema_docs`,
  `undra bindgen --check --docs` on the playground, cookbook, fieldbook and two-cores a/b, the derived vectors, the playground web
  build, two-cores JVM and Node (interop), `undra-bench` budgets (6 passed, 1 ignored), `node site/scripts/build-all.mjs` (up to date)
  and `check-links.mjs --words` (347 words).
* `main`'s code restoring the branch's snapshot: 1/1 (attack 1).

## Sizes

* `web/hello-wasm`: 117,749 gzipped (branch) / 116,208 (`main`, same machine and toolchain); gate 120,000.
* `web/hello-runtime-js`: 22,067 / 22,100; gate 22,100.

## The merge with main, CI

`main` `fc326d6` (the CI piece: Rust 1.99.0 pinned, `wt/**` push triggers, `scripts/ci-local.sh`) merged as `880418d`. Conflicts:
`.10x/decisions/sde/_index.md` (both lines kept), `contract-tests/README.md` (34 scenarios, 98 cells, with `main`'s wording),
`site/search-index.json` (regenerated). Auto-merged and re-run: the hub (`ring.rs` pacing beside this piece's `stores_of` filter),
`tests/devtools.rs`, `devserver.rs` (the dev tests now take turns and get 900 s for a cold build, which the new dev tests inherit),
`dev_reload.rs` (then M2's fix). After the merge: `scripts/ci-local.sh` on a clone of the head (see the final report for its
result), then one push of `wt/reload-handles`; the landing gate is CI, Bench, Two cores and Site green on that exact head, whose run
ids are in the reviewer's hand-off (this file is part of the head they test).

## Open items

* M1's follow-up: a per-type-free miss path in `Runtime::object::<T>`, then re-record the wasm size under the pinned toolchain.
* L1, L2: small, dev-only or forged-input cosmetics.
* `proto/reload-handles` (the architect's prototype branch, local only) can be deleted.
