# ADR-019: keel-signals delivery is transactional

Status: accepted (2026-09-30). Touches SPEC 16.1 (commit algorithm, `observe`).
Origin: `.10x/reviews/2026-09-30-keel-signals-review.md`, findings H1 and M1.

## H1: an abandoned change-set must not leave the host diverged

Context. A commit claims its slots (clears their dirty bits) and advances the keyed-list
baselines while it builds the change-set. If a computed, an encoder or the sink then panics, the
claim was already spent: nothing is re-marked, and the next keyed patch is computed against a
baseline the host never saw. The host stays wrong and nothing tells it to resynchronise.

Decision. The claim is transactional. While a change-set is being built and delivered, an
`Abandon` guard is armed; if it unwinds, every claimed slot is recorded in a per-store *unsent*
set and every claimed keyed baseline is dropped. The next commit that touches the store adds the
unsent slots to its own and claims them even though their dirty bit is clear, so they go out as
full values (op 0). `observe(on)` of a slot removes it from the set (the host has just been given
its current value). `observe(on)` is transactional in the same way: entries are built in a
private buffer, and if anything panics the targets return to their previous observed state, their
baselines are dropped and previously observed targets are marked unsent.
Consequence, accepted and documented: a computed that panics on every evaluation holds back its
store's change-sets (each commit re-raises its panic) until it recovers. That is loud; the
alternative was silent drift.

Alternative rejected: re-set the dirty bits themselves. A set dirty bit tells every writer to skip
recording, so with nobody holding the slot in a queue the next write to it would trigger no
commit at all (the stranded state of finding M3). The unsent set is owned by the store, so any
thread's next commit can drain it. Also rejected: retrying immediately inside the same commit
loop (a deterministic panic would burn all 1000 rounds).

## M1: writes made during `observe` must be part of what the host receives first

Context. `observe` runs computed closures to encode them, and closures may write signals. With no
transaction open such a write committed at once and reached the sink before the observe entries,
which carried the older values.

Decision. `observe(on)` runs inside a transaction. It marks the targets observed, then encodes
every target, and repeats while a write made by a closure dirtied one of the targets (bounded, at
most eight passes); each pass clears the targets' dirty bits before it encodes, so the writes are
absorbed into the entries and their own commit, which happens when the transaction ends, finds
nothing to send for them. The entries therefore hold the post-write values. Writes to slots that
were not targeted commit normally after the entries are built. `encode_signal` runs in a
transaction for the same reason.

Alternative rejected: have the caller hold a transaction across `observe` and delivery. It moves
the burden to every caller (the runtime, restore, tests) and leaves the bare `StoreCell::observe`
contract unsafe.
