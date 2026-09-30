# ADR-027: keyed-list change-sets are O(change): list operations are recorded, the diff is the fallback

Status: accepted (2026-09-30). Touches SPEC 3.8 (wording only; the wire does not change) and 16.1 (the
`Signal<Vec<T>>` API and the keyed-list commit algorithm). Amends ADR-019 (what an abandoned keyed delivery
drops).
Origin: `bench/RESULTS.md` Finding 1. The blueprint says keyed patches must be "O(change), not O(list)"
and its section 14 row asks for one insert into 10,000 items in at most 20 us (iOS, A15). Measured: 536 us,
27x over, and the same 530 us for an in-place update. 85% of it is `KeyedPatch::diff` re-hashing and
re-scanning the whole list at every commit. Needs an ADR first because it adds a public API to
`keel-signals` and changes the runtime model of a keyed slot (R11).

## Context

A keyed list slot (`StoreCell::attach_keyed`) sends the host a keyed patch (SPEC 3.8). It used to find the
patch by comparing the list as it is now with a copy of the list as the host last saw it (the baseline):
`KeyedPatch::diff` hashes every key of both lists into two maps, builds position tables, compares the
encoded bytes of every surviving item, and only then knows that one row was inserted. The cost is linear
in the list and independent of the change, which is the opposite of the claim. The store code that made
the change knew exactly what it did (`rows.insert(i, row)`); the information was thrown away at the `Vec`
and rediscovered at the commit.

## Decision

`Signal<Vec<I>>` gains **recorded list operations**: `push`, `insert`, `remove -> I`, `update_at(i, f)`,
`move_item(from, to)`, `clear`, and `replace(Vec<I>)`. Each applies the mutation and, when the signal is
the list of an observed keyed slot, appends the SPEC 3.8 op it performed to a per-signal **op log**
(`Insert { index, item }`, `Remove { index }`, `Update { index, item }`, `Move { from, to }`, `Clear`;
`replace` is `set`, see below). At commit the slot sends the log as the patch: O(ops). The bytes are the
ones the diff would have encoded for the same change, so the wire and every host applier are untouched.

* **Raw writes** (`set`, `update`, `replace`) mark the log *stale* before they touch the list, inside the
  same write lock. A commit that finds the log stale **diffs**, exactly as before (with the more-than-50%
  removal and no-key-overlap fallbacks to the full value). A transaction that contains a raw write is
  diffed as a whole, whatever recorded operations came before or after it in the transaction. `replace` is
  the raw write with a list-flavoured name: a refresh of a whole list that shares keys with the old one is
  best sent as a patch (the host's list views keep the identity of the rows that stayed), and the diff
  produces that.
* **The baseline stays** as the fallback's input, and in the recorded path it is kept in step by
  replaying the same ops on it, moving the items out of the log (no second clone): O(ops) plus the
  `memmove` a `Vec::insert` in the middle needs, which the core's own list and the host pay as well. A
  `Move` is one `rotate` of the span between the two indices, not a removal and an insertion.
* **The log lives exactly as long as the baseline.** It is empty whenever a baseline is taken
  (`observe(on)`, the full value that a first or abandoned commit sends) and is dropped with it
  (`observe(off)`; an abandoned commit, ADR-019, which drops "the baselines among its claimed slots" and
  now the logs recorded against them). A slot nobody observes records nothing. An abandoned delivery
  therefore cannot leave ops that the next delivery, a full value, would apply a second time.
* **Sent as recorded.** The recorded path does not apply the diff's rules: no more-than-50%-removed
  fallback, no key-overlap fallback, no key-uniqueness check (it never calls the key function), and no
  minimality (three mutations of one row are three ops). O(ops) is the goal, not the smallest patch.
  Duplicate keys are therefore not noticed on this path; the list the host receives is the same either
  way, and the contract (`KeyFn`: keys are unique) is unchanged.
* **Bounded.** A log of more than `max(4096, list length)` ops goes stale: the commit diffs, which from
  that size is no worse than O(ops). This bounds memory when nothing commits (no sink, store without a
  handle): ops recorded then stay in the log and are sent by the next commit that delivers.
* **Nothing escapes as a panic.** Indices are checked before the list is touched and panic with a message
  as `Vec::insert`/`remove` do (these are Rust-side store methods, running under the dispatch guard). A
  panic inside `update_at`'s closure, after it changed the item and before its op was recorded, marks the
  log stale, so the next commit diffs and carries the change.

### How the commit stays consistent with concurrent writers

The design has one moving part that the diff did not: the log and the list have to be seen together.

* A recorded operation appends to the log **inside the value's write lock**, the same critical section
  that mutates the list. A raw write marks the log stale inside its write lock too.
* The commit takes the log and looks at the list under the value's **read lock** (`Signal::read_locked`):
  no writer can be between a mutation and its log entry, so the ops taken are exactly the ones the list
  seen includes. The same step re-arms the log (empty) for what comes next, which is what makes a full
  value, a diff and a recorded commit all leave "baseline + log = list".
* Lock order is baseline, then the value lock, then the log's own lock, which is a leaf (nothing is called
  and no other lock is taken under it). Writers take value then log; nothing takes the baseline lock
  from a write path.
* Before sending the log the commit checks, in O(ops), that every index fits the list as the ops replay
  on the baseline's length and that the result has the list's current length; if not it diffs instead.
  This is a safety net (and what turns a bug into a correct, slower commit), not a path the design
  relies on. Debug builds additionally compare the baseline with the list item by item (encoded bytes)
  after every recorded commit, so every test and property case checks the invariant, not only its length.

## Alternatives rejected

* **Per-row change stamps** (a version per item, scanned at commit). Still O(list) per commit and adds a
  word per row to every list; does not help inserts and moves, which are position changes.
* **Making `diff` cheaper** (keep the baseline's key vector, skip old-side hashing). A constant factor
  (RESULTS.md estimates 2x), not the order: 10,000 rows would still cost hundreds of microseconds.
* **Recording through `update`'s `&mut Vec`** (a wrapper that logs calls). The closure can do anything
  to the `Vec` (`retain`, `sort`, `swap`), and a wrapper that logs only what it understands is a log that
  is silently wrong. Explicit operations, with raw access invalidating the log, make correctness local.
* **Applying the diff's 50% and key-overlap fallbacks to recorded ops.** Would need the list to be
  inspected at commit, which is the O(list) this removes. A host applies any sequence of SPEC 3.8 ops;
  the fallbacks exist to keep a computed patch from being larger than the value.
* **Lazy baseline** (keep the last materialised baseline plus the ops sent since, materialise on the
  first diff). Saves one of the two `memmove`s per insert only until the pending ops have to be applied;
  the amortised cost is the same and the state machine is bigger.
* **A new wire op or a wire change.** Not needed; R7 would make it a major version.

## Consequences

* `keyed_10k/insert` goes from 536 us to 6.3 us, `update` from 532 us to 0.27 us, `move` from 703 us to
  9.7 us (Apple M5 Pro, `bench/RESULTS.md`), and the cost no longer depends on the list except for the
  `memmove`: 1,000 rows 0.77 us, 100 rows 0.36 us. The blueprint row (<= 20 us) is met on this host, at
  0.32x the target, with room for a slower core. The raw path keeps its cost and its budget
  (`keyed_10k/raw_update_diff`, about 530 us).
* Store code that wants the O(change) path must use the recorded operations. `update(|rows| ..)` keeps
  working and keeps being O(list). The macro that generates store methods, and the playground, have to be
  moved onto the recorded API by their owners; nothing in this ADR changes a generated shape.
* New public surface in `keel-signals`: seven inherent methods on `Signal<Vec<I>>`. No existing signature
  changes. Internally `SignalInner` carries a type-erased log handle set by `attach_keyed`; the recorded
  methods are plain mutations on a signal that is not a keyed slot, or whose slot is unobserved.
* SPEC 16.1 describes both paths; SPEC 3.8 gains a sentence saying the core sends recorded ops as they
  happened or, for raw writes, the diff, and that the host applies either the same way.
* Tests: the `keel-signals` model test (`tests/keyed_ops.rs`) runs arbitrary interleavings of recorded
  operations, raw writes, transactions, aborted commits (a computed that panics), observe and
  unobserve against a host mirror that applies every change-set, and asserts both equality with the core
  and that a healthy recorded transaction is sent as exactly its ops; `tests/concurrency.rs` does the same
  with several writer threads and an observer, and the existing suites, the delivery-integrity regressions
  especially, pass unchanged. The model test runs 1,500 cases by default (`KEEL_KEYED_OPS_CASES` raises
  it); 100,000 cases were run against this change in a debug build, where every recorded commit is also
  checked item by item against the baseline.
