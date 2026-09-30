# ADR-021: keel-signals turns three misuse hangs and aborts into contained panics

Status: accepted (2026-09-30). Touches SPEC 16.1 (`Signal::update`, `Computed::get`, keyed lists).
Origin: `.10x/reviews/2026-09-30-keel-signals-review.md`, findings L1, L3 and L4 (L2, the unknown
`observe` id, is covered by ADR-019).

## L1: a computed cycle overflowed the stack (R6)

Context. Dependencies are fixed at construction, so the tracked graph is acyclic, but a closure can
read a computed it did not declare (a handle captured through a `OnceLock` filled in later). Such a
cycle recursed until the stack overflowed, which aborts the process and cannot be caught at the
dispatch boundary.

Decision. Each thread keeps a stack of the computeds it is recomputing. Recomputing a node that is
already on the stack panics with "computed cycle detected". The nodes stay stale. The doc sentence
"cycles are impossible" now says what is and is not possible.

Alternative rejected: a per-node recursion flag. Concurrent recomputes of one node from different
threads are legitimate (the ticket scheme exists for them), so the flag would misreport contention as
a cycle.

## L3: `Signal::update` deadlocked when its closure read a computed of the same signal

Context. `update` holds the value write-locked while the closure runs. Reading a computed
derived from the signal recomputes it, which read-locks the same value: the thread waits for
itself forever. The docs forbade reading "this signal" only.

Decision. The restriction is documented as transitive. The value lock is now tried first, and only
when it is found taken does the code look at a thread-local list of signals whose `update` closure is
running on this thread: a hit panics with a clear message instead of waiting. Contention with other
threads is unaffected (they wait), and the uncontended path pays nothing beyond the `try_` call.

Alternative rejected: a debug-only check on every snapshot. It would put a thread-local access on the
hottest read path, and release builds would still hang.

## L4: an unobserved `no_coalesce` keyed list kept a full copy forever

Context. Keyed diffing stored a baseline on every delivery, and only `observe(off)` dropped it, so a
`no_coalesce` keyed list that was never observed held a clone of its list for the life of the store,
contradicting the documented memory cost.

Decision. A keyed slot delivered while unobserved (only because it is `no_coalesce`) sends its full
value and keeps no baseline; observing sends a full value and starts the baseline, as before.
