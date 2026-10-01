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
* [react-native](react-native.md) — ADR-038: React Native is a fourth host of the C ABI (a C++ TurboModule over JSI, the TS runtime's transport on top; ADR-044 transitional notes) (2026-10-01).
