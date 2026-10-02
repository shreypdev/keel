# Architect: reload-handles (query handles across every restore) design note

2026-10-02, `wt/reload-handles` (from `main` at `a309e9f`). Binding text: **ADR-059** (Proposed),
`.10x/adrs/ADR-059-transient-handles-across-restore.md`. Read: `CLAUDE.md`; SPEC 1.2, 5.4, 5.9, 5.10, 9, 10.3c, 11,
17; ADR-022, 023, 037, 040, 043, 049 (decision 3 and `recovery.ts`), 051, 053 (with its review amendments), 054;
`undra-runtime` (`object_table.rs`, `runtime.rs` snapshot / restore / observe / dispatch, `lazy.rs`), `undra-query`
(`handle.rs`, `dispatch.rs`, `shared.rs`, `poll.rs`, `paged.rs`), `undra-transport` (`resume.rs`, `session.rs`,
`devtools/hub.rs`), the dev runner template, the three runtimes' reconnect paths and lazy lists, `undra-bindgen`'s
`ts.rs`.

## The decision, short

1. **Core-side, one mechanism.** A snapshot keeps, per query handle, a record of what the handle is made of (the
   encoded parameters and the observer's own polling interval). A restore re-issues the handle's **value** from the
   record without building anything (a *dormant* handle). The object is built again when the host first uses the
   handle (`Observe`, a call, an object parameter). Hosts run no code: the wrapper, the identity map and the mirror
   registration never notice.
2. **A query handle that is alive in the runtime being restored is left alone** (time travel, an app's own
   `restore`): no change-set, no refetch, polling and pages untouched, also when the step is older than the handle.
3. **The record rides in the record shape layout 2 already has** (one field under the reserved id `0xFFFF_FFFE`),
   so no layout, envelope, ABI or schema change, and a runtime older than this ADR drops the record as "a store type
   this build does not have" instead of refusing the snapshot.
4. **A record that cannot be honoured** (the query is gone, its parameter types changed, the record does not
   decode) is refused and counted; the restore still succeeds (ADR-037 decision 7's exception, argued in the ADR).
5. **The TypeScript replay of ADR-049 goes** (generated `recreate`, `_rebindObject`, `recovery.ts`'s loop): the web
   gets the same mechanism as everyone, and a wrapper's `handle` never changes.
6. `Lazy<T>` page servers already come back through the store's op 0; pinned by tests, nothing new.
7. User objects are deferred; the cache's data is not in the snapshot.

## Decisions needed from the integrator

Each with my recommendation. 1 to 3 are the design; 4 to 9 can be answered independently.

1. **The record in-band (reserved field id) or a "layout 3" trailer behind a tag?** The brief expected a layout
   change. *Recommend in-band.* P1 ran both against `main` unmodified: an old runtime restores the in-band snapshot
   and drops the record (WARN, `RestoreReport::dropped`); it refuses a trailer whole. With a trailer, a rollback to an
   older build loses all restored state whenever a query screen was open at snapshot time, and four codecs and their
   vectors change. The cost of in-band is naming: SPEC 5.9's `count x { store }` becomes `count x { record }`, a
   record being a store's or a recreation record.
2. **Build at first use (dormant handles), not at restore.** *Recommend first use.* A restore then calls no port
   (R12), starts no fetch for a handle the host released after the snapshot (the core cannot tell those apart on a
   same-runtime restore), and does not fetch on a dev reload before a client and its ports are attached. Every
   runtime already observes every handle again after a reconnect or a restart, so the build happens at once where
   it matters. The cost is one more object kind in the table (dormant) and three resolution points that must call
   one function.
3. **Same-runtime restores leave live query handles alone, including a time travel to a step older than the
   handle.** The brief suggested "dropped, as stores are". *Recommend kept.* A store built since the step has no
   values in the snapshot; a query handle needs none, and dropping it breaks a screen for nothing. The one case that
   still goes stale: a store of the snapshot needs the handle's slot (the slot was reused); it is reported as
   `displaced` and added to the devtools count.
4. **Warm reload: should a non-persisted query keep its rows on screen across a dev reload?** With this ADR it
   shows "loading" once and then the data fetched by the new code (the old process's cache is gone). *Recommend:
   not in this piece, and never in the snapshot* (a query without `persist` is one the developer chose not to write
   down, apps persist snapshots, and a seeded cache makes every poll a new devtools step). If wanted, it is its own
   small ADR: `undra dev` hands the observed cache entries to the new runner beside the snapshot, in memory only,
   marked stale so the new code refetches while the old rows stay.
5. **User objects: `#[undra::api(recreate)]` now, later, or never?** *Recommend deferred*, with the documented
   answer "make it a store". The seam is general (`UndraObjectDyn::recreation`, `Reviver`), but an object rebuilt
   from its constructor arguments silently loses what its methods changed, re-runs constructor side effects, and
   cannot take objects or callbacks. Revisit with a real use case and its diagnostics.
6. **Remove the TypeScript replay in the same piece?** *Recommend yes, as the piece's step 7.* Both orders work
   (P4: all 33 TypeScript scenarios pass against the new core with the old runtime, and again with the replay
   removed). It is a generated-shape change (query handle constructors lose `args` and the `recreate` option;
   goldens and four generated examples move) and it overlaps `ts-runtime-16k` in `core.ts` and `object.ts` (deletions
   only). Measured: 33 bytes gzipped off the hello-world runtime (22,100 to 22,067), about 530 gzipped off an app
   that imports `crashRecovery()` (1,979 raw), a seven-line block off every generated query handle class.
7. **The hello-world wasm gate.** The baseline on this machine is 116,416 bytes gzipped (budget 120,000). The
   prototype written inline measured 119,910 (+3,494, 90 bytes under the gate); with the recreation code reachable
   only through `Runtime::add_reviver` (the pattern the lazy page servers use, ADR-052) it measures 117,718
   (+1,302). *Recommend: make "reachable only through `add_reviver`" a requirement of the piece (it is in the
   ADR), and accept up to +1.3 KB on the hello-world core, re-recorded in the same commit.* A core with no reviver
   then handles a record as a store type it does not have, which is also what an old runtime does.
8. **Wording.** Terminal: `state kept (2 stores, 1 query handle, 1 KiB, restored in ..)`. The notice's
   `(N objects not carried over)` keeps its words and stops counting query handles. An infinite query after a
   reload shows its persisted pages or its first page, and loads the rest on demand (not back to its old depth).
   *Recommend as written*; restoring the depth is a follow-up if someone asks.
9. **Scenario number and shape.** S35, ten steps: same-runtime steps through the generated bindings on three columns
   and the React Native model; the fresh-runtime step in the second process (build B) that S14 and S15 already
   use on Swift and Kotlin, with one query whose parameter type build B changes (the refused case). *Recommend as
   written*; it needs one query added to `examples/playground/core/src/updates.rs`, so the playground's schema hash
   and generated bindings move once.

## What the prototype proved (and what it is not)

Branch `proto/reload-handles` in this repository (local, never pushed, **not for merging**), head `364ccb3`: three
`proto:` commits on top of `a309e9f`. It implements decisions 1 to 4 in `undra-wire`, `undra-runtime` and `undra-query`
without the final details the ADR's brief marks, and removes the TypeScript replay.

| | What ran | Result |
|---|---|---|
| P1 | `crates/undra-runtime/tests/proto_inband.rs` against `main` unmodified | the in-band record is left out and reported, the store restored; a trailer is refused whole |
| P2 | `crates/undra-query/tests/proto_reload.rs` (four cases, a platform's raw calls) | fresh runtime: no HTTP until the `Observe`, then the answer, the fetch, `refetch` accepted, polling at the observer's recorded interval; same runtime: three restores, no change-set, no fetch, same `live_handles`; released since the snapshot: dormant, nothing fetched in two minutes of fake time, carried into the next snapshot, forgotten by `release`; refused records counted, the rest re-issued |
| | `cargo test -p undra-runtime -p undra-query -p undra-wire -p undra-transport` | green except the one test that pins the old behaviour (`undra-query/tests/wire.rs`, `a_snapshot_leaves_handles_out_and_restore_still_works`) |
| P3 | `crates/undra-cli/tests/dev_reload.rs`, `proto_a_query_handle_keeps_working_across_a_rebuild`: the real `undra dev`, a raw client holding the `ticker` query's handle | after the rebuild the same handle answers the `Observe`, `refetch` is status 0, the new process's ticks arrive (`[1, 2, 3]`), the notice is `Reloaded, state kept`; the nine existing reload tests and the three devtools tests pass. The runner template was not changed. |
| P4 | `contract-tests/ts` on the real wasm core | 33/33 with the TypeScript runtime unchanged; 33/33 with the replay removed, S22 asserting the query handle **is** the handle it was before the trap |
| | `contract-tests/ts/test/proto-s35.test.ts` (generated bindings, `core.restore` on the live core) | the same wrappers; nothing delivered for the query handles, no request, the feed's 100 rows stay; `refetch` accepted; the ticker keeps ticking; the third page loads; a second restore changes nothing |
| | `contract-tests/kotlin` (JNI) and `contract-tests/swift` (the C ABI table) | 31/31 each against the prototype core, no runtime change |
| | `runtimes/ts/@undra/runtime` `test/recovery.test.ts` | 28 pass; the 2 tests of the replay fail, as they should (rewritten in step 7) |
| | `scripts/wasm-size.sh` | decision 7's numbers |

Found by the prototype, fixed in the ADR's brief: the dev runner counts records as stores (`state kept (2 stores`
for one store and one handle); the devtools hub's `stores_of` would list records as stores and, by observing them,
build dormant handles; `observe(.., false)` must not build; the fingerprint must be cached (the prototype computes a
closure per handle per snapshot).

Not done by the prototype: the property test, the bench rows, S35's fresh-runtime step on Swift and Kotlin and
the React Native model, `displaced`, the runner's counts, the hub's filter, the device proof. Those are the
piece's.

## For the implementer

The ADR's "Implementation brief" is the order of work. Start from `main`, not from the prototype branch; read its
diff for the shape of steps 1 to 3 and 7 (`git diff a309e9f proto/reload-handles`).
