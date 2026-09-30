# ADR-019: keel-signals orders delivery per store and polices the write context

Status: accepted (2026-09-30). Touches SPEC 3.5, 5.1, 16.1 (`ChangeSink`, commit algorithm,
`set_write_checker`). Origin: `.10x/reviews/2026-09-30-keel-signals-review.md`, findings M2 and
M3 (M3 is added by the commit that fixes it).

## M2: writes from two threads reached the host out of order

Context. Dirty-bit ownership is global but commits are per thread, and claiming, diffing and
`deliver` were three separate steps. Thread A could claim and diff (advancing the keyed
baseline), be preempted before `deliver`, and thread B could then deliver a patch computed
against A's baseline first: the host applied B's patch to a list A's patch had not yet changed
and diverged, with no out-of-bounds op to reveal it. The runtime's core lock hides this, but
nothing enforced that writes happen under it (the pool's `spawn_blocking` closures have a
runtime installed and can write).

Decision.
1. Every `StoreCell` has a delivery mutex that `commit_slots` holds from the claim until the sink
   returns. Change-sets of one store therefore reach the sink one at a time, in claim order.
   `txn_id`s a store sees strictly ascend: a `txn_id` shared by the stores of one transaction is
   replaced by a fresh one for a store that has already seen a newer id (the store's `last_txn`).
   The mutex is held while the sink and computed closures run, which is a deliberate exception to
   "no lock is held while user code runs" in SPEC 16.1; it is documented on `ChangeSink`: a sink
   must not wait for another thread that writes the same store. Same-thread writes from a sink
   are queued by the commit loop and cannot re-enter, and effects run after the mutex is released.
   The runtime's sink only hands the payload to the host.
2. `keel_signals::set_write_checker(f: fn() -> bool)` (a global like `set_sink`). In debug
   builds every write to a signal that is attached to a store or has dependents asserts `f()`
   before anything changes; release builds never evaluate it (no cost). A purely local signal
   (unattached, no dependents) is exempt: it delivers nothing and races with nobody.
   `keel-runtime` installs a checker that refuses blocking-pool worker threads (SPEC 5.1 already
   forbids their writing signals) and allows every other thread, including threads inside no
   runtime, whose writes reach the global runtime by design (runtime-internals section 12, and the
   `the_global_runtime_lifecycle` test).

Not fixed, documented instead: a write to a slot that another thread has dirty inside an open
transaction is absorbed into that transaction (dirty-bit ownership is global). The checker is what
keeps runtime users from getting there.

Alternatives rejected. (a) Stamp a per-cell sequence number at claim time and let the host
reorder: needs a wire change and host state. (b) Take the runtime's core lock inside the sink: the
sink must never take the core lock (a thread holding a keel-signals lock while waiting for the
core would deadlock against the core). (c) A strict checker that also refuses threads with no
runtime: it contradicts the documented global-runtime routing and breaks existing tests that
write store signals from the test thread; a blocking-pool denylist catches the documented
violation without that.
