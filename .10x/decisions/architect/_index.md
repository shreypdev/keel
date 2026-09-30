# Architect — index

Cross-cutting decisions are recorded in `docs/SPEC.md` (binding) and were made before the
takeover; treat these notes as [DISCOVERED] context, confirmed by reading the code.

- Schema-first (R1): keel-meta describes every public shape; macros emit it, bindgen
  consumes it, ffi exports it. Canonical JSON + fnv1a64 hash gates attach (§2.3, §7).
- One-mutator core: parking_lot Mutex on native (caller's thread for sync calls, one
  `keel-core` thread polls async), RefCell + host-driven `keel_poll` on wasm (§5.1).
- Reads never cross the boundary (R5): platforms hold a mirror; the core pushes
  change-sets per transaction; keyed lists cross as patches (§3.5, §3.8).
- Boundary: hand-rolled C ABI + JNI RegisterNatives + raw wasm exports; no bindgen
  frameworks (uniffi/wasm-bindgen rejected in blueprint).
- Lost ADRs: ADR-014 (Kv foreign port), ADR-017 (panic poisoning, no CoW overlay) are
  referenced by the SPEC; re-write them if those decisions are reopened.
