# Architect: v1.x gap audit and ADR drafts 034–037 (2026-10-01)

**Problem.** The v1.x program ("no engineer declines Undra because of something it cannot do") needed a
ranked list of what a real app team would hit, from the code rather than from memory, and the ADRs for the
Track A runtime-model changes. Spec: `.10x/specs/2026-10-01-v1x-gaps.md`.

**Findings.** 71 gaps: 7 block adoption, 40 hurt, 24 polish. The blocking ones: Android ships without Http,
Kv, SecureStore, Fs, Connectivity and Lifecycle adapters (PO-1); Android has no `undra dev` (DV-1, B1 in
flight); Kv/SecureStore have no error channel, so a storage failure traps a web core (PO-3); `wasm-worker`
mode traps on its first sync port call (PO-4); a web panic kills the core with no restart (PC-2); the offline
queue of the previous build is deleted on an app update (PS-1); a snapshot of the previous build restores
wrong values silently (PS-3, reproduced). Verified clean: `unsafe` confinement (R2) and no printing in library
crates.

**Decisions (all Proposed).**
* ADR-034 — anything that outlives a call holds a `WeakCtx`; the runtime's own call/stream tasks and
  `undra-query`'s tasks stop pinning it; subscribers receive the `Ctx`; `WeakCtx::upgrade() -> Result<Ctx,
  Gone>`, `Ctx::closed()` and `WeakCtx::sleep` give a typed end; a dropped owner frees the runtime (and `Drop`
  answers in-flight work like shutdown); Kotlin `close()` shuts the in-process core down. `Ctx` stays strong.
* ADR-035 — a signal write from a thread that does not hold the owning runtime's core lock is refused in every
  build (E0065 panic, or `WriteError` from `try_set`/`try_update`); change-sets are routed by the store's owner;
  `Ctx::with_core` is the sanctioned path.
* ADR-036 — flag 2 carries only the stream's `E`; new flag 3 `failed` = `status, message, detail` in the reply
  vocabulary; `impl Stream<Item = Result<T, E>>` ends a stream with a typed error. Pre-publication wire change.
* ADR-037 — persisted artefacts record a schema hash and per-item type fingerprints with an embedded closure;
  equal loads, different migrates structurally by name, then through `#[undra::migrate]` hooks in Rust;
  refusals are typed (restore) or dead letters (queue), never silent; the persisted cache is bounded.

**Rejected.** A weak `Ctx` (breaks `&Runtime`/`&T` accessors and costs every call); auto-shutdown on the last
external reference (uncountable); lenient release writes with a log (still diverges); marshalling off-core
writes onto the core (breaks read-after-write); a discriminator byte inside flag 2 (two meanings under one
flag); a whole-schema "hash changed" hook (cannot decode types that no longer exist); serde/JSON for persisted
values (a second encoding, R1).

**Plan changes proposed.** New A6 (web-core crash recovery) and A7 (storage ports with an error channel;
no standard port can trap); C4 split into C4a adapters (start in phase 1, Android first), C4b failure model
(an ADR-032 analogue for Kotlin and TS) and C4c snapshot parity; A3 recorded as an ADR-019 amendment; C3 needs
ADRs for newtypes, generic aliases, objects as values and a decimal type; port cancellation needs an ADR that
revisits "the C ABI stays at 19 functions"; ADR-036 and ADR-037's snapshot layout ship as one wire revision.

**Open for the integrator.** Accept or amend 034–037; whether A7 joins `wt/runtime-lifecycle`; whether C4a
starts now; E-code numbers E0065/E0066; `max_persisted_entries` default.
