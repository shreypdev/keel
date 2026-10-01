# High-frequency data

A socket that pushes prices, a sensor at 1 kHz, a simulation, a list that churns: the core commits
state far faster than a screen can show it. This page says what Undra does with such a firehose, what
you can rely on, and the two tools you have: `#[undra(no_coalesce)]` and `ctx.txn`. The binding rules
are in `docs/SPEC.md` §11.1; the decision and its measurements are ADR-031.

## What happens to a firehose

The core is fast: one observed transaction costs about 80 ns to commit and hand to the platform
(about 12 million per second on one core). Every transaction becomes one **change-set**, delivered
in commit order, and nothing on the wire is merged or dropped.

The platform side is where it would hurt, so the **mirror** (the part of each runtime that applies
change-sets to your stores) merges before it applies:

* **Once per frame.** Change-sets the core produced on its own (a timer, a stream, an event, a port
  completion, a background task) are applied at most once per display frame: on the next
  `requestAnimationFrame` in a browser (a zero-delay task while the tab is hidden, a microtask under
  Node), from a `CADisplayLink` on iOS, through `Choreographer` on Android.
* **Merged per signal.** Within one drain, a full value replaces everything queued before it for
  that signal, and the keyed patches of a list are concatenated into one patch. So each signal is
  applied **at most twice per frame** (its last full value, then one merged patch) however many
  transactions touched it, and a keyed list is copied once per frame instead of once per patch.
* **Exact.** The state after a drain is exactly the state after applying every change-set in commit
  order. Only the intermediate states inside one frame are skipped, and you could not have seen
  them: they would never have been rendered.
* **Bounded.** The backlog holds at most 65,536 entries or 16 MiB (tunable). Past that it is folded in
  place, so a blocked main thread, a backgrounded app or a hidden tab costs memory proportional to the
  number of signals you observe, not to the number of transactions, and catches up in one drain. A
  list whose merged patch grows past 4,096 operations and 1 MiB is dropped and re-observed instead:
  one full value is cheaper than that patch.

On the web, 1,667 one-row patches per frame on a 10,000-row list (100,000 per second) went from about
4 ms of main-thread work per frame to about 0.4 ms with this merge (ADR-031, "Consequences").

## What is never delayed: your own calls

Frame alignment applies only to what the core does on its own. Your calls see their own writes:

* `await store.method()` (TypeScript, Swift `async`, Kotlin `suspend`): the change-sets the call
  produced are applied before your code after the `await` runs.
* A synchronous method called on the main thread (Swift and Kotlin `callSync`-backed methods, TS
  `callSync` in `wasm-main`): applied before it returns.
* Creating a store: its initial values are there when `create()` / the initializer returns.

```ts
await todos.add("milk");
console.log(todos.remaining.get()); // already counts "milk"
```

## When to opt out: `no_coalesce`

Some signals are about the steps, not the state: a progress bar that should visibly walk from 0 to
100, a counter whose every increment drives an animation. Declare them `no_coalesce`:

```rust
#[undra::store]
pub struct Upload {
    #[undra(no_coalesce)]
    progress: Signal<u32>,   // every value reaches the platform and is applied
    bytes_sent: Signal<u64>, // the last value per frame is enough
}
```

The core then delivers every commit of the signal (even while nothing observes it), the schema
records the flag, and the generated store tells its mirror, which applies every entry of that
signal, in order (TypeScript announces each one to subscribers separately).

Know what you are asking for:

* **The UI framework still decides what it shows.** A Kotlin `StateFlow` conflates by design, and
  SwiftUI and React render once per frame. If you need to *react* to each value in code, subscribe to
  the signal (TS `signal.subscribe`) or, better, make the steps a **stream** (`Stream<T>` return
  type): streams have credit-based backpressure and deliver every item.
* **The backlog bound wins.** If the main thread falls 65,536 entries behind, a `no_coalesce` signal
  is folded like any other.
* **It costs what coalescing saves.** A `no_coalesce` signal written 100,000 times per second is
  100,000 applies per second on the main thread. Keep it for low-rate step semantics.

## Batch in the core: `ctx.txn`

Coalescing happens on the platform, after the core has encoded and handed over one change-set per
transaction. When **your** code makes many writes in one go, batch them in a transaction: the core
then encodes one change-set, crosses the boundary once, and the platform has nothing to merge.

```rust
pub fn apply_quotes(&self, quotes: Vec<Quote>, at: Timestamp) {
    self.ctx.txn(|| {
        for q in &quotes {
            self.board.update_at(q.row as usize, |row| row.cents = q.cents);
        }
        self.updated_at.set(at);
    });
}
```

Without `ctx.txn`, every write outside a transaction is its own transaction (SPEC §5.5): a loop of
1,000 writes is 1,000 change-sets. Coalescing makes that cheap on the platform; `ctx.txn` makes it
free. Use it whenever one logical change touches many signals or many rows, and in loops that ingest
batches from a socket or a port. It also keeps a multi-signal change atomic for the UI: a transaction
that touches one store is one change-set, never split across frames.

Two more rules for producers inside the core:

* A push source you own (a channel fed by a port, a socket reader) is your buffer: bound it, or
  expose it as a stream so the consumer's credit paces it.
* A transaction that touches several stores arrives as several change-sets; a frame can fall between
  them. Keep state that must change together in one store.

## Measuring it

Every runtime counts what its mirror did: change-sets and entries received, entries applied after
merging, drains, compactions and resyncs. `UndraCore.stats()` reports them, and a drain listener
reports each drain as it happens.

```ts
const stop = core.mirror.addDrainListener(({ changeSets, entries, appliedEntries, durationMs }) => {
  console.log(`${changeSets} change-sets, ${entries} entries, applied ${appliedEntries} in ${durationMs} ms`);
});
const { mirror } = await core.stats(); // changeSetsReceived, entriesApplied, drains, compactions, ...
```

```swift
let registration = core.mirror.addDrainListener { stats in
    print(stats.changeSets, stats.appliedEntries, stats.duration)
}
let counters = core.stats().mirror
```

```kotlin
val handle = core.mirror.addDrainListener { stats -> log(stats.changeSets, stats.appliedEntries, stats.duration) }
val counters = core.stats().mirror
```

`entriesApplied` far below `entriesReceived` means coalescing is doing its job; `compactions` above
zero means the main thread fell behind the bound; `resyncs` above zero means a list was re-observed
instead of patched.

## Tuning

The defaults suit an app; change them only with a measurement in hand.

| | TypeScript | Swift | Kotlin |
|---|---|---|---|
| Frame source | `UndraCore.load({ mirror: { schedule } })` (default `scheduleFrame`) | automatic (`CADisplayLink` on iOS, tvOS, visionOS) | `LoadOptions(mirror = MirrorOptions(framePacer = ChoreographerFramePacer()))` from `android-adapters` |
| Backlog bound | `mirror: { maxPendingEntries, maxPendingBytes }` | `LoadOptions.maxPendingEntries`, `.maxPendingBytes` | `MirrorOptions(maxPendingEntries, maxPendingBytes)` |
