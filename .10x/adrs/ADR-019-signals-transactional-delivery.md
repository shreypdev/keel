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

## Amendment (2026-10-01): a panicking computed is isolated per signal

Status: accepted (2026-10-01; implemented on `wt/runtime-lifecycle`, Track A, piece A3). Origin: the v1.x gap
audit `.10x/specs/2026-10-01-v1x-gaps.md`, gap PC-1, which recommended recording this as an amendment before
code because it changes when a store's change-sets are delivered. Touches SPEC 5.5 and 16.1 (the commit
algorithm, `StoreCell::failed_signals`, `ChangeSink::computed_failed`). No wire, ABI, schema or generated
platform code change.

Context. The consequence H1 accepted above ("a computed that panics on every evaluation holds back its store's
change-sets until it recovers") turned out to be far wider than "loud": the audit's probe wrote an unrelated
signal of the store five times and got five panics and zero change-sets. One derived value that cannot be
computed for the current inputs (a division by zero, an index out of range) froze every signal of its store,
failed every write to it with status 2 and marked it poisoned. The retry of the unsent set made it worse: every
commit of the store re-evaluated the computed and re-raised its panic into whichever call committed.

Decision.

1. **A computed's evaluation is isolated per slot.** Wherever a delivery evaluates a computed (a commit
   building a change-set, `observe(on)`, `observe_and_deliver`), it does so under `catch_unwind`, into a scratch
   buffer. A computed that panics is left out of the change-set; every other slot of the store is delivered;
   the write that triggered the commit succeeds (the panic is not re-raised). A delivery that ends up with no
   entry at all sends nothing.
2. **The slot is held back, not retried per commit.** The failed slot is *not* put in the unsent set: a
   computed is deterministic in its inputs, so retrying it on every commit of its store would turn one bad
   value into a panic per write. Its dirty bit is claimed like any other; when its inputs change, the
   invalidation dirties it again and the next delivery evaluates it again. The host keeps the last value it
   received (an observe that finds it failing sends no entry for it, and leaves it observed). The first
   evaluation that succeeds sends the full value, as every computed is sent.
3. **The typed poisoned state is the signal's, not the store's.** The cell records the failed slots with the
   panic message (`StoreCell::failed_signals() -> Vec<(u32, String)>`, `StoreCell::is_failed(id)`), and reports
   each transition, after the store's delivery lock is released: `ChangeSink::computed_failed(owner, handle,
   signal_id, message)` once when a slot starts failing, `computed_recovered(..)` once when it evaluates again.
   The runtime logs the failure at error level through the owning runtime (naming the store type, the handle,
   the signal and the message), counts it with the panics, marks the store poisoned and lists the signal in
   `stats_json` (`poisoned_signals`, the number held back now); the recovery is logged at info level.
4. **Computeds that read a failing one fail with it** (their closure re-reads it and panics) and are held back
   the same way, each reported once.
5. **What the platforms see is unchanged.** No new message: a change-set without the failing signal's entry, and
   for a call that reads the computed (a method returning it) the ordinary panic reply, status 2, which every
   runtime already maps (Swift `UndraCallError.panicked`, Kotlin `UndraReplyException(PANIC)`, TypeScript
   `UndraReplyError` status 2).
6. **What still abandons a change-set (H1 above, unchanged):** a panicking encoder of a plain or keyed slot (the
   value's `Encode`), and a panicking sink. Those are bugs in a type or in the embedder, not app logic over the
   current inputs, and the abandon-and-resend path stays the right answer for them.
7. **wasm** (`panic = "abort"`): nothing can be caught; a panicking computed still traps the core. Recovery is
   piece A6 (gap PC-2). Effects (gap RX-3) are not part of this amendment.

Alternatives rejected. *Retry the failed slot on every commit of the store* (the unsent set): a panic per write
for a computed that cannot be evaluated on its inputs, which is the probe's problem in a quieter form. *Send a
"poisoned" marker to the host*: a wire change for a state the host cannot act on; the existing status 2 of a
reading call and the core-side state are enough. *Keep the panic re-raised to the writer as well*: the writer's
write succeeded and was delivered; failing its call would misreport it.
