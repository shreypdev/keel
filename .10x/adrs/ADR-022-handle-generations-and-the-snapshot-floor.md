# ADR-022: handle generations come from one counter, and a snapshot carries its floor

Status: accepted (2026-09-30). Changes the `Snapshot` payload (SPEC 5.9) and the object table
(SPEC 1.2, 5.4). Origin: `.10x/reviews/2026-09-30-keel-runtime-review.md`, finding H1 (and its
edges T8 and L8).

## Context

A handle is `(slot index u32, generation u32)`. The generation is what makes a stale handle fail
(`stale_handle`, status 5) instead of naming whatever object reuses the slot. The table kept one
generation per slot and bumped it on release. `restore` rebuilt the table and set each restored
slot back to the generation *in the snapshot*, which can be lower than generations the core had
already issued at that index; the floor it raised (`max snapshot generation + 1`) protected only
vacant and fresh slots.

The review's repro (T1): `A = (0,1)`; snapshot; release A; `B = (0,2)` (the host keeps B's
wrapper); restore puts A back at `(0,1)` and B is correctly stale; release A again; a new `C`
becomes `(0,2)`, which is B. The stale wrapper now reads C's values, and when the host finalises
it, `keel_release(B)` releases C. Restoring into a fresh process has the same hole for any slot the
old core had cycled past the snapshot's generations, and a snapshot whose handle carries generation
`u32::MAX` saturated the floor so the first release wrapped the slot to 1 (T8). A per-slot counter
also wraps after 2^32 - 1 releases of one slot, and the LIFO free list concentrates churn on the
lowest slot (L8).

## Decision

1. **One counter.** Every handle the object table issues takes a fresh generation from a single,
   monotonically increasing `u32` counter (`fetch_add`), not from its slot. Generation `0` is never
   issued. Consequently a `(slot, generation)` pair is never issued twice, anywhere in the process,
   for as long as the process lives. The counter is **process-wide** for real runtimes
   (`Runtime::init`, `Runtime::new`): it also keeps a handle from one runtime, or from before a
   `keel_shutdown`/`keel_init` cycle (host finalisers run late), from naming an object of the next.
   `TestRuntime` owns its counter instead, so the handles a test sees (and may put in a golden
   file) are identical on every run however many tests run in parallel.
2. **The snapshot carries the counter.** `Snapshot` gains `generation_floor u32`: the highest
   generation issued when the snapshot was taken (`0` if none). Layout:
   `count u32, generation_floor u32, stores × { ... }`, all little-endian; the floor sits right
   after `count`, so the first four bytes stay the store count.
3. **Restore resumes above the floor.** `restore` raises the counter to
   `max(current, generation_floor, every generation in the snapshot)`; it never lowers it (a floor
   below the current counter is fine). Restored stores are placed at their recorded handles, but
   every handle issued later, in this process or a fresh one, is above everything issued before
   the snapshot and everything issued between the snapshot and the restore, so none of those can
   alias a post-restore object. A released slot is simply vacant; the next object in it gets a fresh
   generation.
4. **Exhaustion.** The counter never wraps. After `u32::MAX` issues it is spent for good: the next
   insert logs FATAL ("handle generations exhausted") and panics with a clear message, which the
   runtime's entry points contain like any panic (status 2 for a call). Nothing already created is
   affected, and releasing does not free a generation. This is a **v1 limit**: at 100 handles/s it
   is 1.3 years, at 1,000/s 50 days, at 10,000/s 5 days, per *process* (every runtime in it shares
   the counter). A core that long-lived and that busy restarts and restores from a snapshot.
   Widening the generation needs a handle-layout change, which is a wire break (major version).
5. **Refusals.** `restore` rejects a handle whose generation is `u32::MAX` (`BadHandle`) and a
   floor of `u32::MAX` (`RestoreError::GenerationFloor`): either would leave the counter nothing to
   issue, so obeying a corrupt or hostile snapshot would brick the runtime. `insert_at` refuses
   generation `u32::MAX` too. Decoding an older-layout snapshot (no floor) fails with
   `unexpected_eof` (an empty one is four bytes where eight are needed); there are no persisted
   snapshots in the wild before v1, so there is no fallback decoder.

The platforms' typed `Snapshot` codecs (Swift, Kotlin, TypeScript) and their tests follow the new
layout; the floor is opaque to hosts, which pass snapshots through as bytes.

## Alternatives rejected

* **Per-slot high-water marks** carried per store in the snapshot (`insert_at` never lowers a
  slot's generation; release jumps to `max(hwm, gen) + 1`). It fixes the T1 alias for stores but not
  for non-store handles (lazy lists, query handles, every other object), which have no snapshot
  record to carry a mark, and it grows the payload by a word per slot the core ever used. One
  number covers every handle.
* **A per-runtime counter** (one per object table). It fixes H1 and keeps generations
  deterministic, but a host that outlives `keel_shutdown` and calls `keel_init` again in the same
  process would again meet `(0,1)` issued twice. Only `TestRuntime`, where determinism matters
  more, keeps a private counter.
* **Retire a slot at wrap** (the reviewer's L8 suggestion): needs per-slot state this design no
  longer has.
* **Wrap and skip live generations**: makes exhaustion silent and aliasing possible again.

## Consequences

* Wire: the `Snapshot`/`Restore` payload is 4 bytes longer; `keel-wire`, the three platform codecs,
  SPEC 5.9 and `keel_snapshot`'s pre-init answer (eight zero bytes) change together.
* Generations are no longer small per-slot numbers: they count handles issued process-wide, so a
  handle's generation says how many objects the process had created, not how often its slot was
  reused. Nothing relies on the old meaning.
* Residual: a runtime restored from a *different* process's snapshot, after it already issued
  handles of its own, can hold a pre-restore handle equal to a restored `(slot, generation)`. The
  table is cleared by the restore, so such a handle resolves to the restored object; hosts restore
  before creating objects (the documented crash-recovery flow), and every handle created afterwards
  is above the floor.
