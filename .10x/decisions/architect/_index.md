# Architect — index

Cross-cutting decisions are recorded in `docs/SPEC.md` (binding) and were made before the
takeover; treat these notes as [DISCOVERED] context, confirmed by reading the code.

- Schema-first (R1): undra-meta describes every public shape; macros emit it, bindgen
  consumes it, ffi exports it. Canonical JSON + fnv1a64 hash gates attach (§2.3, §7).
- One-mutator core: parking_lot Mutex on native (caller's thread for sync calls, one
  `undra-core` thread polls async), RefCell + host-driven `undra_poll` on wasm (§5.1).
- Reads never cross the boundary (R5): platforms hold a mirror; the core pushes
  change-sets per transaction; keyed lists cross as patches (§3.5, §3.8).
- Boundary: hand-rolled C ABI + JNI RegisterNatives + raw wasm exports; no bindgen
  frameworks (uniffi/wasm-bindgen rejected in blueprint).
- Lost ADRs: ADR-014 (Kv foreign port), ADR-017 (panic poisoning, no CoW overlay) are
  referenced by the SPEC; re-write them if those decisions are reopened.
- [stress-bench](stress-bench.md) (2026-09-30): harsh-conditions benchmark design; the core is
  not the bottleneck, platform delivery does not bound work or memory; ADR-031 (frame-coalesced
  delivery) proposed.
- 2026-09-30 [swift-error-channel.md](swift-error-channel.md): ADR-032 (proposed). Generated
  Swift never traps; calls throw `E` / `CancellationError` / `UndraCallError`, sync `()` commands
  report to `LoadOptions.onError`; typed throws only on port requirements.
* [v1x-default-choice](v1x-default-choice.md) — the v1.1 / v1.2 program: no reason to say no (2026-10-01).
* [v1x-gaps](v1x-gaps.md) — gap audit (71 gaps: 7 block, 40 hurt, 24 polish) and ADR-034…037 proposed: WeakCtx, off-core writes refused in every build, typed stream failures, persisted-state migrations (2026-10-01).
* 2026-10-01 [derived-keyed-lists.md](derived-keyed-lists.md): ADR-039 (proposed). `DerivedList<T>` (filter, map, sort_by_key, parameters, count) kept from the source's recorded ops on an order-statistic index: O(log n) per row change, one patch per transaction, no schema or generated-shape change; waits for Track A.
* 2026-10-01 [boundary-surface.md](boundary-surface.md): ADR-040…046 and ADR-049 (proposed). Objects cross with interned owned references; host callbacks are port instances; newtypes, generic instantiations, `Decimal`; polling, infinite queries, `Lazy<T>`; one namespaced function table per self-contained core (two cores per app); iOS 15/16 `ObservableObject` mode; symbol files, debugging, background runs, panic reports; typed storage errors, worker sync ports, web recovery. Plan: `.10x/specs/2026-10-01-boundary-surface-plan.md`.
* 2026-10-01 [wasm-size.md](wasm-size.md): ADR-052 (accepted). The hello-world web core from 136.2 KB to 95.7 KB gzipped at the branch's base, 102.7 KB after merging Track A and parity (`main` alone: 143.4 KB; budget 120 KB, the wasm alone): the schema sorts share one merge sort, the query runtime is linked only by cores that declare queries; `scripts/wasm-size.sh` gates it in CI (120 KB and 5% over the record) and the README/site number is generated from the record. The JS runtime (24.8 KB) is gated the same way at 26 KB (restated from 24 KB after the merge); piece `ts-runtime-size` targets 16 KB. Release builds remap the home directory out of every binary.
* [react-native](react-native.md) — ADR-038: React Native is a fourth host of the C ABI (a C++ TurboModule over JSI, the TS runtime's transport on top; ADR-044 transitional notes) (2026-10-01).
* 2026-10-01 [devtools.md](devtools.md): ADR-054 (accepted). `undra dev` serves a devtools page on its own listener: a second endpoint (`/devtools`, `/devtools/ws`) with its own messages, no envelope kind; the server's hub observes every store while a page is open and the bridge keeps the app client's view unchanged; the ring of snapshots lives in the dev server (200 steps, 32 MiB, 4 MiB a step); time travel is `Runtime::restore` of a step through the app's own session; a per-run token on every request, `404` for anything else; one runtime seam, `register_inspector`.
* 2026-10-02 [reload-handles.md](reload-handles.md): ADR-059 (proposed). Query handles across every restore, on every platform, by one mechanism in the core: a snapshot keeps a record of what each query handle is made of (in the record shape of layout 2, under the reserved field id `0xFFFF_FFFE`: no layout, ABI or schema change, and an old runtime drops the record instead of refusing the snapshot), a restore re-issues the handle's value without building anything, and the object is built when the host first uses the handle; a handle alive in the runtime being restored is left alone. The TypeScript replay of ADR-049 goes. Prototyped end to end on the local branch `proto/reload-handles` (the real `undra dev`, the three contract columns); nine decisions for the integrator.
