# keel-signals

Reactive state for a Keel core (`docs/SPEC.md` sections 5.5 and 16.1).

`Signal`s hold values, `Computed`s derive values from them, `Effect`s react to changes, and a
`StoreCell` connects the signals of one store to the platform. Writes are grouped into
transactions (`txn`); when the outermost transaction ends, everything the host is *observing* is
encoded into a change-set (one per store) and handed to the `ChangeSink` the runtime installed
with `set_sink`.

```rust
use keel_signals::testing::CaptureSink;
use keel_signals::{txn, with_sink, Computed, Signal, StoreCell, ALL_SIGNALS};
use keel_wire::Writer;

// A store with a signal and a computed, wired up the way generated code does it.
let cell = StoreCell::new(0xC0DE);
let count = Signal::new(1_i32);
let double = Computed::new(&count, |n| n * 2);
cell.attach(&count, 0).unwrap();
cell.attach_computed(&double, 1).unwrap();

// The runtime gives the store a handle, and the host starts observing: it immediately
// receives the current values.
cell.set_handle(0x1_0000_0001);
let mut initial = Writer::new();
assert_eq!(cell.observe(ALL_SIGNALS, true, &mut initial), 2);

// Two writes in one transaction produce a single change-set.
let capture = CaptureSink::new();
with_sink(capture.clone(), || {
    txn(|| {
        count.set(10);
        count.update(|n| *n += 1);
    });
});
let sets = capture.take_decoded();
assert_eq!(sets.len(), 1);
assert_eq!(sets[0].entries.len(), 2); // `count` and its computed `double`
assert_eq!(double.get(), 22);
```

## What is in the crate

| Item | Purpose |
|---|---|
| `Signal<T>` | A shared value. `get`, `with`, `set`, `update`; `Clone` is another handle to the same signal. |
| `Computed<T>` | A cached value derived from signals and other computeds; lazy, recomputed eagerly at commit only while observed. |
| `Effect` | Runs after every commit that changed one of its inputs; dropping it cancels it. |
| `Deps` | The inputs of a computed or effect: one `&Signal` / `&Computed`, or a tuple of up to six. |
| `txn` | Batches writes; nested calls join the outer transaction; exception safe. |
| `StoreCell` | The signal table of one store: `attach`, `attach_keyed`, `attach_computed`, `observe`, `encode_signal`, `encode_snapshot`. |
| `ChangeSink`, `set_sink`, `with_sink` | Where committed change-sets go. |
| `next_txn_id`, `ALL_SIGNALS` | Transaction ids; the "every signal" id. |
| `testing::CaptureSink` | Records change-sets in tests. |

## Guarantees

* No `unsafe` (`#![forbid(unsafe_code)]`), no threads spawned, no clocks or randomness; builds
  for `wasm32-unknown-unknown`.
* One change-set per store per transaction, in commit order, never split; stores committed by
  one transaction share a `txn_id`.
* Commit cost is proportional to the number of *dirty* slots, not to the size of the store.
* Unobserved signals are never encoded. `observe(on)` always sends the current value (and
  re-observing resends it, which is how a host resynchronises).
* Keyed lists (`attach_keyed`) are sent as patches when a patch is possible and worthwhile, and
  as full values otherwise (SPEC 3.8). Cost: one clone of the list per *observed* keyed signal.
* No lock is held while user code (sink, effect, computed closure, encoder) runs. Writes made
  during a commit are queued and committed as a new transaction afterwards.
* A panic in a transaction, sink, effect or computed never corrupts the thread's transaction
  state; the first panic is re-raised once the commit has finished.

## Tests

```text
cargo test -p keel-signals
```

Unit tests next to the code; integration tests in `tests/` for stores, keyed lists,
re-entrancy and panics, multi-threaded writes through the global sink, and a proptest that
drives random edits, transactions and observation changes against a host-side mirror and checks
that the mirror always agrees with the core.
