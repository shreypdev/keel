# keel-signals adversarial review (2026-09-30)

Scope: all of `crates/keel-signals/src` at `f5a2731`, plus the patch producer it calls
(`keel-wire/src/patch.rs`), its consumers (`keel-macros` store codegen, `keel-runtime` observe/sink
routing) and the three platform patch appliers. Method: read line by line; every finding below was
reproduced in an external scratch crate that depends on `keel-signals`/`keel-wire` by path. No repo
source was modified. Baseline: `cargo test -p keel-signals` (216 tests) green, `clippy -D warnings`
clean. Severity: nothing Critical, 1 High, 3 Medium, 4 Low, 7 Notes.

## Attacks that failed (verified sound)

- **Keyed diff vs. platforms.** `Move` is remove(from) then insert(to) in keel-wire `apply`, TS
  `applyPatch` (payloads.ts:888-894), Kotlin `applyPatch` (KeyedPatch.kt:147-152) and Swift
  (KeyedPatch.swift:174-176). `Update` replaces the whole item everywhere. I ran a property test
  (80k cases: distinct keys, random permutations with drops, inserts and updates, duplicate keys,
  and u64 key collisions forced with `key % 5`), applying each patch with an independent
  re-implementation of the platform semantics. All 23k `Some` patches (63k `Move`s) reproduce `new`
  exactly. Every `None` has a documented reason (duplicate key, >50% removed, no key overlap).
  A u64 collision between old and new becomes an `Update` that carries the full item, so the list
  value stays correct and only identity is lost. Inside one list a collision counts as a duplicate
  and falls back to the full value.
- **Diamonds.** Observed `sum` at id 1 with its inputs at ids 2 and 3: each node is recomputed
  exactly once per commit. The recompute is pull-based, so signal_id order does not matter.
- **Nested panics.** A `txn` body panics, the commit runs while unwinding, an effect panics too:
  caught, no abort.
- **Locks and races.** The observe/commit race on `dirty` is safe. No crate lock is held across
  `deliver`; walks lock parent before child, so no deadlock. Attach misuse returns typed errors,
  cancelled queued effects do not run, and there is no reference cycle.

## Findings

### H1 (High): a panic while building a change-set leaves the host silently diverged
- **Where:** `store.rs:436-451`, `store.rs:629-667`, `store.rs:335-343`, `txn.rs:290-295`.
- **What happens:**
  - `store.rs:436-451` clears `dirty` on every claimed slot.
  - `store.rs:471` calls `KeyedList::diff` (`store.rs:629-667`), which replaces the host baseline.
  - A later slot's encoder then panics at `store.rs:478`. A `Computed` closure is the realistic
    source.
  - `run_round` (`txn.rs:290-295`) catches the panic and drops the payload. Nothing is re-marked.
  - The runtime only logs and poisons the store (`runtime.rs:875-879`), and SPEC 5.5 says the store
    "keeps working".
- **Repro, keyed list (confirmed):**
  - Setup: keyed `rows=[(1,1),(2,2)]` at id 0, plus a `Computed` at id 1 that panics once.
  - `rows.update(|r| r[0].1 = 99)` panics and no change-set is sent.
  - `rows.update(|r| r.push((3,3)))` then sends `Insert{2,(3,3)}`.
  - Host ends at `[(1,1),(2,2),(3,3)]`, core at `[(1,99),(2,2),(3,3)]`. No op is out of bounds and
    the host never resyncs.
- **Repro, plain signal (confirmed):** `a` at id 0, a panicking computed at id 1, `b` at id 2.
  After `a.set(5)` panics, 10 later writes to `b` are delivered normally, but the host keeps
  `a=1` and `c=1` indefinitely (core: 5 and 5).
- **Repro, observe (confirmed):** if a computed panics during `observe(ALL)`, the runtime discards
  the entries (`runtime.rs:1006-1009`). Slot 0 is still marked `observed` with a resynced baseline,
  so the next commit sends a `KeyedPatch` for a list the host never received.
- **Fix direction:** make the claim transactional.
  - Encode first, stage the replayed baselines, and swap them in only after `builder.finish()`.
  - On unwind, set `dirty` again on every claimed slot and `forget()` every keyed baseline touched,
    so the next delivery is a full value.
  - In `observe`, set `observed` and the baseline only after every entry has encoded, or have the
    runtime re-observe on failure.

### M1 (Medium): writes made during `observe` reach the host before the observe entries
- **Where:** `store.rs:335-343` with `runtime.rs:1006-1007`.
- **What happens:**
  - `observe` encodes computeds, and their closures may write signals (allowed by
    `computed.rs:27-30`).
  - With no transaction open (depth 0), such a write commits and reaches the sink immediately.
  - The runtime delivers the observe entries only after `cell.observe` returns.
  - This breaks SPEC 3.5 commit order on a single thread without any misuse.
- **Repro (confirmed):**
  - Setup: `hits` at id 0, `items` at id 1, and a `Computed` at id 2 whose closure runs
    `hits.update(+1)`.
  - During `observe(ALL)` the sink receives `hits=1`; the observe payload with `hits=0` arrives
    after it.
  - The host shows 0 while the core has 1, until `hits` is written again.
  - `encode_signal` and restore's re-observe go through the same path.
- **Fix direction:** either have the caller hold a `txn` across observe and delivery, and write that
  into the SPEC 16.1 `observe` contract, or give `observe` a delivery callback that runs before its
  own guard closes.

### M2 (Medium): writes from two threads can reach the host out of order
- **Where:** `graph.rs:103-111`, `store.rs:446-483`.
- **What happens:**
  - Ownership of `dirty` is global (`graph.rs:103-111`), but commits are per thread.
  - Claiming and diffing (`store.rs:446` and `store.rs:471`) and `deliver` (`store.rs:483`) are not
    one atomic step per store.
- **Repro, out-of-order delivery (confirmed, deterministic):**
  - Thread A removes item 2: the diff is `Remove{1}` and the baseline advances. A is then
    preempted before `deliver`.
  - Thread B updates item 3 (`Update{1}`) and delivers first.
  - The sink sees txn ids `[2, 1]`. Host ends at `[(1,0),(3,0)]`, core at `[(1,0),(3,7)]`, with
    no out-of-bounds op.
- **Repro, absorption (confirmed):** A writes `x` inside a `txn`; B's bare `x.set(2)` delivers
  nothing (docs: a bare `set` commits before returning) and ships later under A's txn_id and sink.
- **Why this is reachable:** `lib.rs:7-11` disclaims ordering, and runtime-internals §11 forbids
  signal writes on blocking threads. Nothing enforces either. `Signal` is `Send + Sync + Clone`,
  and `Ctx::current()` works on blocking threads.
- **Fix direction:** add a debug-build check in `record`/commit that the committing thread is in a
  core scope. Alternatively, stamp a per-cell sequence number at claim time and serialize claim
  through delivery per cell. At minimum, document the rule on `Signal::set`.

### M3 (Medium): the 1000-round cap leaves slots and effects frozen for every other thread
- **Where:** `txn.rs:220-223`, `graph.rs:104`, `effect.rs:101`.
- **What happens:**
  - `txn.rs:220-223` stops with work still queued in the thread-local `writes` and `effects`.
  - The `dirty` flags of that work stay true (`graph.rs:104`, `effect.rs:101`).
  - Writes from any other thread see `dirty` already set and skip recording.
- **Repro (confirmed):**
  - Run an x↔y effect ping-pong on a short-lived thread, then drop the effects.
  - 10 later `x.set` calls on the main thread produce 0 change-sets for x (host x=0, core 109).
  - An `Effect` watching x runs 0 times for those writes.
  - Only `observe(on)` unsticks the slots. A stranded effect never runs again once the capping
    thread exits, and host threads (JNI, Swift tasks) do commit on the core.
- **Fix direction:** when the cap trips, drain the leftovers instead of leaving them queued: reset
  `dirty` on those slots and effects and report through a log hook. Or move the leftovers to a
  shared queue that any thread's next commit drains.

### L1 (Low): a computed cycle through an untracked read overflows the stack (R6)
- **Where:** `computed.rs:22-23`, `computed.rs:140-150`.
- **What happens:** the doc says cycles are impossible, but a closure can read a computed created
  later if it captures it through a `OnceLock`.
- **Repro (confirmed):**
  - `c1 = Computed(&a, |v| v + later.get().map_or(0, |c2| c2.get()))`, `c2 = Computed(&c1, ..)`,
    then fill `later` with `c2`.
  - `c2.get()` fails with "has overflowed its stack" and SIGABRT, which cannot be caught.
  - `recompute` clears `dirty` (`computed.rs:150`), but an empty cache still recurses (`140-146`).
- **Fix direction:** keep a thread-local "recomputing" mark per node and panic (catchable) on
  re-entry. Correct the doc.

### L2 (Low): `observe` debug-asserts on host-controlled input
- **Where:** `store.rs:319-322`.
- **What happens:** an unknown `signal_id` panics in debug builds and silently returns 0 in
  release.
- **Repro (confirmed):** `observe(7)` on a store with one signal panics in debug. Through
  `runtime.rs:1006`, a host bug becomes a caught panic, a poisoned store and a level-5 log.
- **Fix direction:** return 0 and log in every build (keep the assert for tests only).

### L3 (Low): an `update` closure that reads a computed of the same signal deadlocks forever
- **Where:** `signal.rs:118-120`, `signal.rs:146-147`.
- **What happens:** the value stays write-locked while `f` runs. `c.get()` recomputes and calls
  `read_recursive` on that same lock.
- **Repro (confirmed):** `a.update(|v| *v = c.get())` with `c = Computed(&a, ..)` hangs (2 s
  watchdog). The docs forbid reading "this signal", not values derived from it.
- **Fix direction:** document that the rule is transitive, and add a debug-only thread-local
  "updating" check in `snapshot()` that panics instead of hanging.

### L4 (Low): keyed `no_coalesce` slots keep a list copy while unobserved
- **Where:** `store.rs:660-663`, `store.rs:330-332`, `store.rs:79-81`.
- **What happens:** `store.rs:660-663` stores a baseline on every full value, and `forget` runs only
  on `observe(off)` (`store.rs:330-332`).
- **Repro (confirmed):** an unobserved `no_coalesce` keyed list sends `[Full, KeyedPatch]`, so a
  full list clone lives as long as the store, contradicting `store.rs:79-81`. Not a divergence:
  the TS mirror applies every entry of a registered handle.
- **Fix direction:** correct the doc, or send full values while unobserved.

## Notes

- **N1 (unconfirmed, code path cited): encoders run under a lock.** Encoders and the generated key
  function run while `KeyedList.baseline` is locked (`store.rs:631-666`). SPEC 16.1 says no lock is
  held while an encoder runs. A hand-written `Encode` that observes or writes the same keyed slot
  would deadlock on this non-reentrant mutex.
- **N2: invalidation never short-circuits.** `Computed::invalidate` (`computed.rs:187-196`) walks
  the whole downstream graph on every write, even when the node is already dirty. N writes to a
  signal with D dependents inside one txn cost O(N·D).
- **N3: allocations on every commit.** `group_by_store` allocates a `Vec<Group>` plus one `Vec`
  per store (`txn.rs:240-268`); `commit_slots` allocates `claimed` and `scratch` (`store.rs:436`,
  `465`); the keyed diff re-hashes the whole baseline, re-encodes every surviving item on both
  sides (`store.rs:687-698`, `patch.rs:248-269`) and clones changed items twice; `observe` builds
  a `Vec` per slot, then copies it (`store.rs:339-350`). Caching baseline keys and item hashes
  would cut most of this.
- **N4: determinism (R12).** `TXN_ID` and `PASS` are process-global (`txn.rs:47`, `graph.rs:33`),
  so one runtime's txn ids depend on other runtimes in the process. The default `RandomState`
  `HashMap` (`txn.rs:243`, `patch.rs:252`) reads OS entropy; no output depends on it.
- **N5: change-sets consumed with nowhere to go.** Claimed slots are consumed even when
  `RuntimeSink` has no current runtime (`runtime.rs:70-75`); a keyed baseline then advances for a
  change-set nobody received.
- **N6/N7:** snapshots in an open txn include uncommitted writes (by design, ADR-017); `attach`
  after `set_handle` is silent; a computed's old `Arc<T>` drops under its cache lock (`computed.rs:160-163`).

## Verdict

The happy path is solid. The keyed diff is correct and matches all three platforms bit for bit, so
there is no Critical finding. Invalidation, diamonds, effects and nesting behave as specified, and
lock discipline is sound.

**Not yet sound for v1 under failure or ordering stress.** Commits change delivery state (dirty bits,
keyed baselines, observed flags) before they know the change-set will be delivered. "Commit order"
holds only when every write is on one thread and none happens inside `observe`. Every divergence
found is silent: the host never learns it should resync.

Fix first:
1. **H1:** make the claim transactional, restoring dirty bits and dropping keyed baselines when a
   change-set or observe payload is abandoned.
2. **M1:** hold a transaction across `observe` and delivery so writes from closures cannot overtake
   the initial values, and write that into the SPEC 16.1 contract.
3. **M3 and M2:** drain leftovers at the round cap instead of stranding them, and detect writes made
   outside the core in debug builds.

Repros: a temporary scratch crate outside the repo; each finding's steps are enough to recreate it.

## Resolution (branch `wt/signals-fixes`)

Every finding above was closed with a regression test that reproduces the review's steps
(`crates/keel-signals/tests/delivery_integrity.rs`, `tests/write_checker.rs`, and the runtime tests
named below). Contract changes are in SPEC 16.1 and ADR-019/019/020 (`.10x/adrs/`).

| Finding | Fix | Regression tests |
|---|---|---|
| H1 | The commit claim is transactional: abandoned slots go to a per-store *unsent* set that the next commit of the store re-sends in full; keyed baselines are dropped; `observe(on)` rolls back on unwind | `h1_*` |
| M1 | `observe(on)` and `encode_signal` run inside a transaction; targets are re-encoded (at most 8 passes) until no closure write dirtied them | `m1_*` |
| M2 | Per-store delivery lock from claim to sink return, strictly ascending `txn_id` per store; `set_write_checker` (debug builds) installed by the runtime, refusing blocking-pool workers | `m2_*` (signals, `write_checker.rs`, runtime `threads.rs`) |
| M3 | At the round cap: cancel queued effects, deliver the dirty changes, release the rest as unsent, report via `ChangeSink::round_cap_hit` (runtime logs an error) | `m3_*` (signals, runtime `signals.rs`) |
| L1 | Thread-local recompute stack: a re-entrant recompute panics "computed cycle detected" | `l1_*` |
| L2 | Unknown `observe` id ignored in every build; the runtime does not record it either | `l2_*` (signals, runtime) |
| L3 | Documented as transitive; a re-entered `update` panics instead of deadlocking | `l3_*` |
| L4 | An unobserved `no_coalesce` keyed list sends full values and keeps no baseline | `l4_*` |

Left as they were: notes N1 to N7. Documented trade-offs: a computed that panics on every evaluation
holds back its store's change-sets until it recovers (ADR-019); a write to a slot that another thread
has dirty in an open transaction is absorbed into that transaction (ADR-020).

## Re-review (fix round merged at `5a887de`)

Method: the original scratch repros rebuilt against the merged checkout, plus new probes.
`keel-signals`, `keel-query`, `keel-runtime`: 634 tests green; clippy clean.

| # | Verdict | Evidence (original repro, re-run) |
|---|---|---|
| H1 | CLOSED | Keyed drift: next commit resends a full value, host = core `[(1,99),(2,2),(3,3)]`. Plain: host `a=5, c=5` = core. Failed `observe(ALL)`: `is_observed(0)=false`, no patch to an unbased host. |
| M1 | CLOSED* | `hits` repro: host 1 = core 1. *Except at the pass cap (R2). |
| M2 | CLOSED | A held 300 ms inside `deliver` while B commits the same store: arrival `[txn 1, txn 2]`, host = core. Cross-thread absorption remains, now documented in SPEC 16.1. The write checker is debug-only and flags blocking-pool threads only. |
| M3 | CLOSED | Capped thread exits, then host `x=y=109` = core; watcher effect runs 10/10. |
| L1 | CLOSED | Catchable panic "computed cycle detected" instead of SIGABRT. |
| L2 | CLOSED | `observe(7)` does not panic in debug; the runtime's `Observed::record` ignores unknown ids. |
| L3 | CLOSED | Panics at once (0.01 s) instead of hanging. |
| L4 | CLOSED | Unobserved `no_coalesce` keyed list sends `[Full, Full]`; no baseline kept. |

Your questions:
- **(a) Deadlocks:** no crate-internal deadlock. 4 threads × 3000 writes across two stores with
  cross-reading computeds and sinks that `observe`/`encode_signal` the other store finish in
  0.07 s under a 20 s watchdog. Only `run_round` takes the delivery lock and commits never nest.
  One user-level deadlock is new: R1.
- **(b) Unsent drain vs. concurrent commits:** no regression; the drain runs under the delivery
  lock. Stress (200 rounds × 2 threads × 400 writes, ~20% of change-sets abandoned by a panicking
  computed): after a quiescent commit host = core, no out-of-bounds op, txn ids strictly ascend
  (debug and release).
- **(c) Observe pass loop:** always terminates (≤ 8 passes); what ships at the cap is R2.
- **(d) keel-query handles:** no break found. Handle cells hold only plain signals, writes go
  through `ctx.txn` on the core, and release is serialized with commits by the core lock. A commit
  after release sees handle 0 and consumes its claims and unsent list; release-mid-fetch wire
  tests pass.

New findings (all confirmed):
- **R1 (Low): deadlock under the delivery lock.** Computed closures now run under it. Repro: a
  computed (id 2) runs `thread::scope(|s| { s.spawn(|| log.set(7)); })` with `log` at id 1 of the
  same store; `src.set(2)` never returns (3 s watchdog), where it completed before the fix round.
  SPEC 16.1 states the rule for sinks only; `computed.rs:35` and the `store.rs:537` comment still
  say closures run with no lock held. Fix: evaluate claimed computeds before taking the lock, or
  document the rule for closures.
- **R2 (Low): `observe` ships stale entries at the pass cap.** Writes still pending after pass 8
  commit at observe's own `TxnGuard` drop, before the runtime delivers the entries
  (`runtime.rs:1089-1090`). Repro: a computed over `n` doing `if v < 50 { n.set(v + 1) }`;
  `observe(ALL)` sends 43 change-sets, then entries with `n=7`. Host ends at `n=7`, core `n=50`,
  stale until the next write. Fix: the runtime holds `keel_signals::txn` across `cell.observe`
  and `deliver_entries`.
- **R3 (Note): a computed that keeps panicking blocks its whole store.** ADR-019 accepts this;
  SPEC 5.5 still says a poisoned store "keeps working". Repro: `divisor=0`, then 5 writes to an
  unrelated `title` give 5 re-raised panics, 0 change-sets, host `title=0` vs core 5 (before the
  fix round: 5 deliveries). Fix: catch per slot and keep only the failing one unsent.

Verdict: every original finding is closed and nothing new is above Low. R2 is a one-line runtime
change and should land with v1; R1 and R3 are a doc fix or a v1.x refinement.

## Integrator resolution of the re-review (same day)

* **R2 fixed**: `Runtime::observe` holds `keel_signals::txn` across entry building *and*
  delivery, so pass-cap leftovers commit as follow-up change-sets after the entries; the host
  converges. Regression: `keel-runtime/tests/signals.rs::r2_observe_past_the_pass_cap_converges_through_follow_up_change_sets`.
* **R1 documented**: `Computed`'s docs now state the closure runs under its store's delivery
  lock during a commit and must not wait on another thread writing the same store (ADR-020).
* **R3 documented**: SPEC §5.5 now matches ADR-019 (a panicking computed holds back its
  store's deliveries loudly; per-signal isolation is a v1.x refinement).
