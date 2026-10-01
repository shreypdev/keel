# Boundary surface, data-layer completion, multiple cores, iOS floor, production operations: implementation plan

**Date:** 2026-10-01 · **Status:** proposed with ADR-040…046 and ADR-049 (all Proposed) · **Author:** principal
architect, `wt/boundary-adrs` · **Inputs:** Amendments B and C of `.10x/specs/2026-10-01-v1x-default-choice-design.md`,
`.10x/specs/2026-10-01-competitive-limitations.md` (findings 2–5, M-1…M-12), `.10x/specs/2026-10-01-v1x-gaps.md`
(TY-2…TY-6, RX-2, PO-3, PO-4, PO-11, PC-2, PA-5), ADR-034…039 as merged on `main` (`7d9b1ea`).

## 1. What the ADRs decide

| ADR | Decision in one line | Size |
|---|---|---|
| 040 | Objects cross as parameters (`&T`, `Arc<T>`, borrowed) and returns (`Arc<T>`, `Option`/`Vec` of it); every handle the core hands out is one owned host reference, interned so an object has one live handle and one wrapper; issued handles roll back with a failed call; derived handles are transient; handles repartition to 24-bit slot / 40-bit generation | L |
| 041 | `#[undra::callback]` traits are port instances: the host passes `Arc<dyn Trait>`, calls are port calls prefixed by an instance handle, fire-and-forget or async-with-`Result`, delivered off the core lock (main thread through the mirror's drain by default, or a serial background executor), released and cancelled by reserved fire-and-forget methods — no ABI change | L |
| 042 | One-field tuple structs are transparent newtypes (Swift `RawRepresentable` struct, Kotlin `value class`, TS branded type); generic structs/enums are templates instantiated by `#[undra::api] pub type X = T<A>;` into plain named records; a `Decimal` wire leaf; opt-in `uuid`/`chrono`/`time`/`rust_decimal`/`bytes` leaf types | M + M + S |
| 043 | Interval polling (end-to-start, paused in background and offline, per-observer override); infinite queries return `Page<T, C>` and expose a keyed list that grows by recorded appends; `Lazy<T>` becomes a store signal the host pages through, with op 2 carrying length and version | S + M + M |
| 044 | Each core is a self-contained image (cdylib, or a prelinked static object on iOS) exporting one `<namespace>_undra_api()` function table; JNI natives on a per-core class; artefacts named by namespace; one `UndraCore` per core; `-force_load` goes away; ADR-038 adopts the table | L |
| 045 | Below an iOS 17 deployment target, generated stores are `ObservableObject` + `@Published`; below 16, the wire `Duration` maps to `UndraDuration`; the runtime's floor drops to iOS 15 / macOS 12 | M |
| 046 | Release builds keep line tables and write symbol files (app dSYM covers Rust; Android unstripped twins + Play zip; wasm debug module + function map); a tested debugger path per platform; `run_background(deadline)` as a standard function with BGTaskScheduler / WorkManager helpers; structured panic reports to `onPanic` through a `Diagnostics` standard port | M + S + M |
| 049 | `Kv`/`SecureStore` return `Result<_, StorageError>` and `undra-query` never panics on storage nor overwrites an unreadable queue; worker mode answers sync ports in the worker (built-ins, plus a `worker.ports` module); opt-in web recovery restarts a trapped module from its last snapshot with a typed `onCoreRestarted` | S + M + M |

Sizes use the catalogue's scale: S = under a week for one implementer; M = one worktree with a
cross-platform surface; L = several pieces or a wire/ABI change.

## 2. Dependencies

```text
ADR-034 WeakCtx ─────────────┬──> 043a polling ─┐
(runtime-lifecycle, Track A) ├──> 040 objects ──┼──> 041 callbacks
                             ├──> 046c background runs
ADR-036+037 wire revision ───┼──> 040 (snapshot floor) ; 043c lazy (three encodings)
ADR-037 persistence-v2 ──────┼──> 043b infinite (persisted pages) ; 046c (per-item queue) ; 049c recovery
ADR-039 derived lists impl ──┴──> 043c `Lazy::over(&DerivedList)` (the rest of 043c does not wait)
A3 (ADR-019 amendment) ──────────> 046b panic reports (new containment sites)
044a ABI table ──> ADR-038 RN host code ; 044b artefacts ──> 046a symbols/debugging
C4a android-adapters ──> 046c (Android Lifecycle) ; 049b (Android storage errors from day one)
C4c TS snapshot parity == 049c's first step
045, 042 ── independent
```

Answers to the questions the brief asked:

* **044 before 038 lands?** Yes — before 038's *code*. The RN C++ module must call through the table and key
  its host object by namespace (ADR-044 §8); building it on global `undra_*` symbols would be rewritten weeks
  later. Only the small enabling half (044a: table, `export_core!`, `undra.h` v2, Swift/Kotlin transports and the
  schema loader through the table) must precede G1; 044b (artefact names, prelink, per-core JNI class, generated
  entries) can run alongside.
* **040 before 041?** Yes, softly: 041 copies 040's ownership rule in the other direction and recommends 040's
  subscription-object pattern for unregistering; the identity-map machinery is shared. 041 also needs ADR-034.
* **043 after 037/039?** Partly: polling (043a) needs only ADR-034 and can land early; infinite queries (043b)
  wait for ADR-037's persisted format; lazy lists (043c) need the wire revision's encodings, and only
  `Lazy::over(&DerivedList)` waits for ADR-039's implementation.

## 3. Order and pieces

| Wave | Piece (worktree) | ADRs | Size | Needs first | Implementer / reviewer |
|---|---|---|---|---|---|
| 0 (now) | `po4-worker-sync` (inside the `parity` piece, as Amendment C allows) | 049 §2.1, §2.5 | S | — | sonnet / opus |
| 0 | `abi-table` (`wt/abi-table`) | 044a | M | — | opus / opus |
| 0 | `ios-floor` (`wt/ios-floor`) | 045 | M | — | sonnet / opus |
| 0 | `newtypes` (`wt/newtypes`; C3's ADR-backed half) | 042 §1 + §4 | M | — | sonnet / opus |
| 1 | `multi-core` (`wt/multi-core`) | 044b | L | abi-table | opus / opus |
| 1 | `generics-decimal` (`wt/generics`) | 042 §2 + §3 | M | newtypes (same files) | sonnet / opus |
| 1 | `stdlib-v2` (`wt/stdlib-v2`; piece A7) | 049 §1 + ADR-046's standard items + PO-6/PO-8 standard changes | M | — (C4a adopts `StorageError` when it lands) | opus / opus |
| 1 | `polling` (`wt/polling`) | 043 §1 | S | ADR-034 merged | sonnet / opus |
| 1 | `symbols-debugging` (`wt/symbols`; Track I, shares D3's build plumbing) | 046 §1 + §2 | M | multi-core | sonnet / opus |
| 2 | wire revision (already owned by `persistence-v2`) | + 040 §8 snapshot floor and handle split, + 043 §3.2 lazy encodings (codecs and vectors only) | +S | — | opus (persistence-v2's implementer) |
| 2 | `objects` (`wt/objects`) | 040 | L | runtime-lifecycle merged (same `runtime.rs`); wire revision | opus / opus |
| 2 | `infinite-lazy` (`wt/paging`; re-scoped E3) | 043 §2 + §3 | M + M | persistence-v2; wire revision; `Lazy::over` after derived-lists | opus / opus |
| 2 | `panic-reports` (`wt/panic-reports`) | 046 §4 | S | A3; stdlib-v2 | sonnet / opus |
| 2 | `background` (`wt/background`) | 046 §3 | M | ADR-034, persistence-v2, C4a, stdlib-v2 | sonnet / opus |
| 2 | `web-recovery` (`wt/web-recovery`; piece A6) | 049 §3 (with C4c) | M | persistence-v2, panic-reports | opus / opus |
| 3 | `callbacks` (`wt/callbacks`) | 041 | L | objects, ADR-034 | opus / opus |

Machine load (Amendment A: no more than seven concurrent pieces that build Rust): wave 0 adds three Rust
pieces; the rest are serialised behind Track A and persistence-v2 as the table shows.

## 4. Shared revisions (do each once)

* **The last pre-publication wire revision** (Amendment C item 1: ADR-036 + ADR-037) also carries **ADR-040 §8**
  (`Snapshot` floor `u64`; handles repartitioned 24/40 — invisible to hosts) and **ADR-043 §3.2** (the `Lazy<T>`
  value, op 2's value and the lazy page reply gain a length and a version; nothing emits them today). If the
  revision lands before 040/043 are implemented, its implementer lands these as codec changes with vectors.
  Nothing else in this set changes the wire: 041 rides SPEC 3.6, 042's `Decimal` is a new, additive leaf, 044
  is an ABI change, 046/049 use calls and ports.
* **Standard surface revision 2** (ADR-024: every core's hash moves once): `StorageError`, the `Kv`/`SecureStore`
  signatures and `FsError::{Full, Unavailable}` (049), `Diagnostics`, `PanicReport`, `PanicFrame`,
  `run_background`, `BackgroundReport` (046), and the gap audit's pending standard changes (PO-6
  `HttpError::TooLarge` + `max_body_bytes`, PO-8 `Fs::delete_dir`). One piece (`stdlib-v2`) changes
  `undra-ports`, `undra_bindgen::stdlib` and the three runtimes' standard types together.
* **C ABI version 2** (044): the function table. Because the table carries its `size` and only ever appends,
  the v1.2 port-cancellation ADR (Amendment C item 6) can add a host callback later **without another break** —
  though ADR-041 §7 shows a reserved-method mechanism that needs no ABI entry at all.

## 5. Contract scenarios (the grid grows past 19)

ADR-039 holds S19. Numbers below are provisional, assigned in planned landing order; the integrator renumbers at
merge if the order changes.

| # | Scenario | ADR | Columns |
|---|---|---|---|
| S20 | newtypes and generic instantiations (`UserId` in and out, `HashMap<UserId, _>`, `TodoPage`, exact `Decimal`) | 042 | Swift, Kotlin, TS |
| S21 | objects cross (child returned, interned twice, passed back, released, child store delivers, cancelled-after-issue leaves `host_refs`, stale after restore) | 040 | Swift, Kotlin, TS (+ RN) |
| S22 | host callbacks (ordered after prior change-sets, async value / typed error / cancellation, interning, registry empties, `coalesce`) | 041 | Swift, Kotlin, TS (+ RN) |
| S23 | lazy list (page a window, op 2 carries length and version, re-page) | 043 | Swift, Kotlin, TS |
| S24 | infinite query (append as a patch, refetch re-chains cursors, first page persisted) | 043 | Swift, Kotlin, TS |
| S25 | polling (two polls at 1 s, paused by `Background`, resumed by `Active`, per-observer override) | 043 | Swift, Kotlin, TS |
| S26 | two cores in one process (independent calls, observations, stats, shutdown) | 044 | Swift, Kotlin (JVM), TS |
| S27 | panic report (`onPanic` gets message, `file:line`, operation, namespace, hash; detached task too) | 046 | Swift, Kotlin, TS |
| S28 | background run (offline-queued mutation replayed by `runInBackground`; deadline cancel keeps the item) | 046 | Swift, Kotlin, TS |
| S29 | storage failures are typed (no panic or trap; counters; unreadable queue never overwritten) | 049 | Swift, Kotlin, TS |
| S30 | worker sync ports (built-ins and `worker.ports` in `wasm-worker` mode) | 049 | TS |
| S31 | web recovery (trap → `restarted` rejection, snapshot values, query handle re-created, budget) | 049 | TS |

ADR-045 adds no scenario: the Swift column runs the whole grid a second time with stores generated in
`ObservableObject` mode. ADR-044's two-core test apps (iOS simulator, Android emulator) and ADR-046's
`symbols.rs`/`debugging.rs` are CI tests outside the grid.

## 6. Budgets (R9)

| Row | Budget | ADR |
|---|---|---|
| `dispatch/call_sync/return_object`, `return_interned_object` | ≤ 2× `add` (core half) | 040 |
| `boundary/callback/notify` | ≤ 1.5× `boundary/port_call` | 041 |
| `lazy/page_50_of_100k` / `lazy/invalidate` | ≤ 20 µs / ≤ 1 µs and 12 bytes | 043 |
| `query/infinite_append_page_50` | ≤ 2× a 50-op keyed patch | 043 |
| `boundary/call_sync/add` through the table | no regression beyond 2 ns | 044 |
| release artefact sizes (`bench/RESULTS.md` size table) | unchanged by symbol work | 046 |
| `ts/snapshot_take_100kb` / `ts/recovery_restart_100kb` | ≤ 2 ms / ≤ 50 ms (desktop Chromium) | 049 |

## 7. Diagnostics

These ADRs use the block **E0070–E0079** (ADR-035 holds E0065, ADR-037 E0066): E0070 generic-instantiation alias
rules (042), E0071 callback method shapes (041), E0073 infinite-query shape (043); E0072 and E0074–E0079 are
free. E0001 (`Lazy`), E0002, E0004, E0007 and E0064 change their texts; D1's error-codes page regenerates.

## 8. Effects on other pieces

* **G1 / ADR-038**: amend decisions 1, 11 and 13 per ADR-044 §8 before code (table, per-namespace host object,
  no `-force_load`, the "one core per process" limit lifted).
* **C3 (parity)**: 042 is C3's ADR-backed half; the recursive-record bug and TY-7…TY-13 stay in C3.
* **C4a (android-adapters, in flight)**: return `StorageError` once `stdlib-v2` lands; report Lifecycle from
  `ProcessLifecycleOwner` (046 §3.3 relies on it).
* **E3**: re-scoped to ADR-043 §2–§3 (`wt/paging`).
* **D3**: shares 046's build plumbing; the `-force_load` removal (044) changes the templates D3 emits.
* **H1/H4**: cookbook pages per ADR (objects and identity, listeners, modelling the domain, pagination, polling,
  one core or several, iOS 15/16, debugging, crash reporting, background, worker mode, recovery). The post may
  claim each row only after its piece merges; matrix rows 8–10, 13, 17, 22–24, 30 and 33 move.

## 9. Open decisions for the integrator

1. **040**: the `Arc<T>`/`&T` spelling; interning (one handle and one wrapper per object) rather than UniFFI's
   fresh-reference-per-crossing; folding the 24/40 handle split and the `u64` floor into the wire revision.
2. **041**: `main` delivery through the mirror's drain as the default (vs `background`); cancellation by the
   reserved `__cancel` method, and whether the v1.2 port-cancel ADR should use the same mechanism instead of an
   ABI entry.
3. **042**: instantiations only in the template's crate (E0070); `Decimal` as `i128` + scale ≤ 38 and not a map
   key; no literal conformances on Swift newtypes.
4. **043**: the three lazy encodings in the wire revision; `UndraLazyList`'s subscript requesting pages; no
   bidirectional paging or `max_pages` in v1.x.
5. **044**: prelinked static objects on iOS (vs dynamic frameworks); the namespace default (crate name); dropping
   the `Java_*` JNI exports.
6. **045**: floor at iOS 15 (vs 16, which needs no runtime change); mode chosen automatically by the deployment
   target.
7. **046**: `debug = "line-tables-only"` in release; the `backtrace` dependency (subject to the wasm32/iOS/
   Android check); `android-work` as a separate module; a `Diagnostics` standard port rather than log parsing.
8. **049**: the `StorageError` variants; recovery opt-in; `worker.ports` as the way to provide sync ports in
   worker mode.
9. **Bundling**: one wire revision (036 + 037 + 040 §8 + 043 §3.2), one standard-surface revision (`stdlib-v2`),
   one ABI version (044).
