# Architect: boundary surface, paged queries, multiple cores, iOS floor, production operations (2026-10-01)

**Problem.** The competitive catalogue's findings 2–5 and the gap audit's blocking ports/web gaps: the boundary
refused objects as parameters or returns (E0064), host callbacks (E0004), newtypes (E0007) and generics (E0002);
the data layer had no polling (SPEC 9 promised it), no infinite queries and no `Lazy<T>`; only one Undra core
could exist per process; the floor was iOS 17; nothing produced crash symbols, documented debugging into Rust,
drained queues in the background or reported panics to a crash reporter; storage ports panicked on failure,
worker mode trapped on its first sync port and a trapped web core stayed dead. Plan:
`.10x/specs/2026-10-01-boundary-surface-plan.md`.

**Probes (throwaway, session scratch directory).**
* Two Rust static libraries sharing a crate with an `inventory` registry and a global: a plain link **silently
  merges** their registries and statics; with `-force_load` 2,179 duplicate symbols (2,178 on the iOS slice);
  fat-LTO staticlibs 2 duplicates. Prelinked (`ld -r`, only `_<ns>_…` global) they are independent, and from the
  LTO staticlib with `-u` the two-core program is 712 KB, the size of two cdylibs. With line tables, `atos` on the
  app's dSYM resolves the Rust frame through the prelinked object (`lib.rs:3`) at no binary-size cost.
* The Swift runtime builds unchanged at an iOS 16 / macOS 13 floor; only generated `@Observable` stores fail. At
  iOS 15 / macOS 12 the only runtime errors are `Swift.Duration`/`ContinuousClock` in three files. Stores
  rewritten as `ObservableObject` + `@Published` build in Swift 6 mode with no warnings; typed throws in async
  protocol requirements type-check for iOS 15.

**Decisions (all Proposed).**
* ADR-040 - objects cross as `&T`/`Arc<T>` parameters (borrowed) and `Arc<T>` returns (one owned reference per
  crossing, interned: one handle and one wrapper per object, rolled back with a failed call); derived handles
  are transient; handles repartition 24/40 with a `u64` snapshot floor in the wire revision.
* ADR-041 - `#[undra::callback]` traits are port instances (`Arc<dyn Trait>` parameters; instance handle in the
  port call; fire-and-forget or async-with-`Result`; `__release`/`__cancel` reserved methods); app code runs off
  the core lock, on the main thread through the mirror's drain by default; no ABI change.
* ADR-042 - transparent newtypes; generic templates instantiated by named aliases into plain records; a
  `Decimal` leaf; opt-in `uuid`/`chrono`/`time`/`rust_decimal`/`bytes` leaf types.
* ADR-043 - end-to-start interval polling; infinite queries as keyed lists grown by recorded appends; `Lazy<T>`
  store signals paged by the host, op 2 carrying length and version.
* ADR-044 - one `<namespace>_undra_api()` function table per self-contained core image (cdylib or prelinked
  static object); per-core JNI class and artefact names; one `UndraCore` per core; `-force_load` removed;
  ADR-038 adopts the table before its code.
* ADR-045 - `ObservableObject` stores below an iOS 17 deployment target, `UndraDuration` below 16; runtime floor
  iOS 15 / macOS 12.
* ADR-046 - release builds keep line tables and write symbol files; a tested debugger path per platform;
  `run_background(deadline)` with BGTaskScheduler/WorkManager helpers; structured panic reports to `onPanic`.
* ADR-049 - `Kv`/`SecureStore` return `StorageError`; `undra-query` never panics on storage nor overwrites an
  unreadable queue; worker mode answers sync ports in the worker; opt-in web recovery from the last snapshot.

**Rejected.** `Named` for objects; a fresh handle per crossing; synchronous host callbacks under the core lock;
new ABI entries for callbacks; generic platform types (schema type parameters); prefixing all 19 symbols (the
Rust symbols still collide or merge); a shared JNI library; hand-rolled dual-mode observation; a separate
debug build for symbols; a Lifecycle event for background windows; `SharedArrayBuffer` for worker sync ports;
always-on recovery.

**Plan.** Wave 0: PO-4 fix, `abi-table` (044a), `ios-floor`, `newtypes`. Wave 1: `multi-core`, `generics-decimal`,
`stdlib-v2` (049 §1 + 046's standard items + PO-6/PO-8), `polling`, `symbols-debugging`. Wave 2 (after
runtime-lifecycle and persistence-v2): the wire revision gains 040 §8 and 043 §3.2; `objects`, `infinite-lazy`,
`panic-reports`, `background`, `web-recovery`. Wave 3: `callbacks`. Scenarios S20–S31 (provisional); E-codes
E0070–E0079.

**Open for the integrator.** The nine items of the plan's §9 (spellings, interning, callback delivery default and
cancellation mechanism, generic-alias crate limit, lazy encodings in the wire revision, prelink vs dynamic
frameworks, iOS 15 vs 16, release line tables and the `backtrace` dependency, `StorageError` variants and opt-in
recovery, one wire / one standard-surface / one ABI revision).
