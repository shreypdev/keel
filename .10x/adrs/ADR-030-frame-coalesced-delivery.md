# ADR-030: the platform mirror applies change-sets once per frame, merged, with a bounded backlog

Status: **proposed** (draft, 2026-09-30, from the harsh-conditions benchmark design,
`.10x/specs/2026-09-30-stress-bench-design.md`). Not accepted, not implemented. Touches SPEC 11 (the
"per-frame coalescing" sentence becomes exact rules), SPEC 17.1-17.3 (mirror options and counters), the
three platform runtimes' `Mirror` and reply paths, the TypeScript worker protocol (internal to the runtime
package), and, for decision 6 only, `keel-meta` (`SignalDef.no_coalesce`) and `keel-bindgen` (one argument
of the generated store registration). **No wire change** (SPEC 3.5 and 3.8 bytes are unchanged), **no C ABI
or wasm ABI change**, no change to `keel-signals` or `keel-runtime`. Constitution R11: the runtime model of
delivery (when and how the platform applies what the core commits) and, with decision 6, a generated shape
change, so it is decided here before code.

## Context

The core commits one change-set per transaction per store and hands it to the host synchronously, on the
committing thread; nothing in the core or the FFI batches across transactions or across time:

* `keel-signals`: "for each store, the slots that are observed (or `no_coalesce`) are encoded into **one**
  change-set and handed to the sink" (`crates/keel-signals/src/txn.rs:8-10`), once per outermost
  transaction (`txn.rs:227-247`, `commit_stores` `txn.rs:310-327`, `StoreCell::commit_slots`
  `crates/keel-signals/src/store.rs:649-745`, `sink.deliver` at `store.rs:741`). The only merging in the
  core is inside one transaction (a slot written many times is one entry: the dirty bit, `store.rs:673-692`)
  and for unobserved slots (never encoded, `store.rs:79-80`).
* `keel-runtime`: `RuntimeSink::deliver` (`crates/keel-runtime/src/runtime.rs:136-143`) calls
  `deliver_change_set`, which calls `Host::change_set` (`runtime.rs:846-866`).
* `keel-ffi`: one C callback per change-set (`crates/keel-ffi/src/native.rs:142-146`), one JNI up-call
  (`jni_shim.rs:135-137`), one wasm import call (`wasm.rs:127-132`).

The platform runtimes coalesce the **hop** to the main thread, but not the **work**:

| Runtime | Hop | Work per change-set on the main thread | Queue |
|---|---|---|---|
| Swift | at most one `Task { @MainActor }` scheduled (`runtimes/swift/KeelRuntime/Sources/KeelRuntime/Core/Mirror.swift:101-115`), which drains everything queued (`Mirror.swift:67-95`) | every change-set decoded and every entry applied (`Mirror.swift:91-93, 125-141`, a lock per entry to find the handler): one `@Observable` mutation per entry; patches applied through `inout` (`generated/swift/.../Stores.swift:1666`) | `pending: [[UInt8]]`, unbounded (`Mirror.swift:23`); each payload copied on the core thread (`InprocTransport.swift:339-348`) |
| Kotlin | at most one `main.post(drainTask)` (`runtimes/kotlin/.../Mirror.kt:80-84`), up to 32 batches per hop (`Mirror.kt:118-141, 239`) | a full value superseded by a later full value of the same signal in the same batch is skipped (`Mirror.kt:158-186`); **patches are never merged**, and each one copies the list (`KeyedPatch.applyPatch`, `wire/KeyedPatch.kt:131-132`, called per entry from generated code, `examples/playground/generated/kotlin/.../Stores.kt:1286`) | `ConcurrentLinkedQueue`, unbounded (`Mirror.kt:43`) |
| TypeScript, `wasm-main` | one `flush()` per microtask checkpoint (`runtimes/ts/@keel/runtime/src/mirror.ts:139-150`, default `queueMicrotask` `mirror.ts:69-73`) | every change-set decoded on arrival (`mirror.ts:128-141`), every entry applied (`mirror.ts:181-187, 197-207`); subscribers notified once per round (`batch`, `signal.ts:48`); each keyed patch copies the list (`applyPatch` = `list.slice()`, `wire/payloads.ts:880-881`, from generated `_apply`, `examples/playground/generated/ts/src/stores.ts:1175`) | `#queue: ChangeEntry[]`, unbounded (`mirror.ts:51`) |
| TypeScript, `wasm-worker` | **one `postMessage` per change-set** (`src/worker.ts:65-68, 75-77`), so one main-thread task, one microtask checkpoint and one `flush()` per change-set (`transport/wasm-worker.ts:284-285`, `core.ts:624-626`) | as above, but a flush holds one change-set, and React re-renders per flush (`useSignal` is `useSyncExternalStore`, `react.ts:42`) | as above |

Measured (throwaway probes, Apple M5 Pro, shared machine; see the spec for the method):

* The core is not the bottleneck: one observed single-signal transaction, committed and delivered, costs
  **82 ns** (12 M transactions/s on one core); through `Runtime::call_sync`, 153-176 ns.
* TypeScript mirror (V8, Node 24, the runtime's own `Mirror` and `applyPatch`): **~175 ns per full-value
  change-set** whatever the batch size (decode plus `_set`), 226 ns when each change-set gets its own flush;
  a keyed patch on 10,000 rows costs **1.38 us** (the list copy) however small it is, so 1,667 patches (100 k/s
  at 60 Hz) cost **2.3 ms per frame**, where applying the same 1,667 ops to one copy costs **5 us**.
* Worker mode: a `postMessage` per change-set costs **1.25 us** on the receiving thread before any apply
  (Node `worker_threads` proxy; 0.36 us each when 100 are sent in one message); in a browser each is also a
  task with a microtask checkpoint and, through `useSyncExternalStore`, a React render.

So a core-side firehose of 100 k transactions/s becomes 100 k copies, decodes and applies per second on the
main thread, a keyed list copied 100 k times per second on TS and Kotlin, and a backlog with no bound when the
main thread falls behind (or is suspended: a backgrounded iOS app, a hidden tab). The hops merge only while the
main thread is busy: Swift schedules the next hop as soon as the previous one starts draining
(`Mirror.swift:117-123`) and Kotlin as soon as it ends with work queued (`Mirror.kt:135-140`), so an idle main
thread is woken for nearly every change-set (read from the code, not measured); TS `wasm-main` flushes once per
task of the producer, which is once per change-set when each update arrives in its own task (a socket
message); and TS `wasm-worker` mode **is** 100 k main-thread tasks per second. The blueprint promises the opposite ("Coalescing is on by default.
Change-sets committed before the platform's next main-thread hop are merged, so a burst of transactions from
a network response becomes one render. Apps that need every intermediate state (progress bars) opt out per
signal", `docs/blueprint.html:703`), SPEC 11 says "apply change-sets on the main thread with per-frame
coalescing", and the opt-out cannot be honoured: `#[keel(no_coalesce)]` never reaches the platforms
(`SignalMeta` has no such field, `crates/keel-meta/src/meta.rs:345-356`).

## Decision

1. **Merge per drain.** A *drain* is one application of the mirror's queue on the main thread. Within a
   drain the mirror folds the queued entries per key `(handle, signal_id)`, in arrival order, without
   decoding any value:
   * a full value (`op 0`) or a lazy invalidation (`op 2`) replaces everything queued earlier for the key;
   * keyed patches (`op 1`) that follow each other for the key are **concatenated** into one patch:
     `count = sum of counts`, `ops = the ops' bytes in arrival order`. SPEC 3.8 applies ops sequentially,
     each index relative to the list after the previous op, so the concatenation is the same change; no
     knowledge of `T` is needed.

   Each key is then applied at most twice per drain (its last full value, then its merged patch), keys in
   the order of their first entry in the drain, and subscribers are notified once at the end of the drain
   (TS `batch`; Swift and Kotlin already defer rendering to the frame). The generated `apply` functions are
   unchanged: they receive a valid entry, and a merged patch that goes out of bounds takes the existing
   resynchronisation path (re-observe, SPEC 3.8). The drain parses each change-set's entry table once
   (as `Mirror.kt:203-224` already does) and, on TS, stops decoding values on arrival.
2. **Drains are frame-aligned, replies are not delayed.** The mirror drains at most once per display frame:
   TS `requestAnimationFrame` while the document is visible and a zero-delay task otherwise (Node keeps
   `queueMicrotask`); Swift a `CADisplayLink` on iOS, tvOS and visionOS (paused while the queue is empty) and
   the existing main-actor hop elsewhere; Kotlin `Choreographer.postFrameCallback` on Android and a paced
   single-thread executor on a plain JVM. Two events drain **immediately**, before anything else is
   delivered: `observe` (as today, so a store has its values before its initializer returns) and **a reply**
   that arrives while entries are queued (TS drains synchronously before it resolves the call's promise, and
   at the end of a `callSync`; Swift and Kotlin enqueue an immediate main-thread drain before they resume the
   continuation or complete the deferred, so a caller on the main thread runs after it, FIFO). Read-your-writes
   therefore holds for UI code: after `await store.increment()` the mirror shows the change, as it does today
   by accident of that same FIFO ordering (TS microtasks, Swift main-actor jobs, Kotlin `Main.immediate`
   posts). Frame alignment only delays what the core produced on its own (timers, streams, events, port
   completions, background tasks): the firehose.
3. **The backlog is bounded.** The queue holds parsed entries, not payloads. When it passes
   `maxPendingEntries` (default 65,536) or `maxPendingBytes` (default 16 MiB), the enqueuing thread folds
   it in place with the rules of decision 1 (O(n) once per n enqueues). A key whose merged patch passes
   `max(4096 ops, 1 MiB)` is dropped and marked *awaiting a full value*: entries for it are discarded until
   an `op 0` for it arrives, and the next drain re-observes it (`observe(handle, signal, true)` from the main
   thread; the core answers with the current full value, SPEC 5.5). Memory is then O(observed keys x (value
   size + 1 MiB)) however far the main thread falls behind, and a suspended app catches up in one drain.
4. **The worker batches.** `runWorker` sends the envelopes produced during one worker task as one message
   (`{ t: "envelopes", data: ArrayBuffer[] }`, all buffers transferred), flushed from a microtask in the
   worker; `WasmWorkerTransport` accepts both shapes. The worker protocol is internal to the runtime package
   (both ends ship together); its version constant is bumped.
5. **Observable.** Each runtime counts `changeSetsReceived`, `entriesReceived`, `entriesApplied`
   (after merging), `drains`, `compactions`, `resyncs`, and offers a drain listener (TS
   `mirror.addDrainListener(fn): () => void`, called after each drain with `{ changeSets, entries,
   appliedEntries, durationMs }`; Swift and Kotlin equivalents in the device phase). `KeelCore.stats()`
   reports the counters. This is what the playground's stress screen and the landing page's live numbers
   read.
6. **The per-signal opt-out reaches the platforms** (separable: may land after 1-5). `SignalDef` gains
   `no_coalesce: bool`, serialised only when `true` (canonical JSON, and so the schema hash, of every schema
   without such a signal is unchanged); `#[keel::store]` records it in `SignalMeta`; bindgen passes the
   store's no-coalesce signal ids to its mirror registration. For those keys the mirror applies every entry,
   in order, and notifies after each (TS: one `batch` per entry). Whether the UI can observe each value is
   still up to the platform's reactive primitive (a Kotlin `StateFlow` conflates by design); the docs say so.

## Alternatives considered

* **Core-side coalescing per delivery tick** (commits leave slots dirty; a host-driven `keel_flush()` or a
  `RuntimeConfig.delivery = "frame"` mode emits one merged change-set per store per frame). It would also
  save the core's per-transaction encode and the FFI copy. Rejected for now: it adds an ABI entry point
  (R7/R11), it separates commit from delivery and so reopens ADR-019 (the transactional claim), ADR-020 (claim
  order, the delivery lock, strictly increasing `txn_id`s per store), ADR-023 (observe and restore delivering
  under the lock) and the op-log lifetime of ADR-027; it removes the per-transaction change-sets that the
  devtools protocol (SPEC 5.10) and the blueprint's time-travel log record; and the core is not the
  bottleneck (82 ns per transaction). Revisit, as its own ADR, if device measurements show the per-change-set
  crossing (the copy in the FFI callback), not the apply, dominating a real app.
* **Merge without frame alignment** (fold only what accumulated when the hop runs). An idle main thread hops
  within microseconds of each change-set and merges almost nothing, so the work stays per change-set, and
  React renders once per flush. Kept only as the test configuration (the `schedule` option TS already has).
* **Backpressure to the producer** (credit for change-sets, as SPEC 3.7 does for streams). State is
  last-writer-wins, so the right backpressure for state is to drop superseded values, which is decision 1.
  Blocking the committing thread would stall the whole core loop and, for an off-core commit, would wait
  inside the store's delivery lock, which a sink must never do (ADR-020). Streams keep their credit.
* **Status quo plus documentation** ("batch your writes in `ctx.txn`"). The producers that flood are
  often not the app's own loops (event ports, hundreds of port completions, a stream the core consumes), and
  the blueprint promises coalescing by default.
* **Merging in generated code** (a batch `apply` per signal). The mirror has every byte it needs; keyed
  patch concatenation needs no type knowledge. Generated code stays as it is (R3).

## Consequences

* **Ordering (ADR-019, ADR-020).** Unchanged in the core and on the wire: one change-set per transaction per
  store, commit order, strictly increasing `txn_id` per store. On the platform, the state after a drain equals
  the state after applying every queued change-set in commit order; intermediate states inside one drain are
  not observable (they were not on TS already, whose subscribers hear once per round, and only partly on
  Kotlin). A transaction that touched several stores still arrives as consecutive change-sets and may, as
  today, straddle two drains; frame alignment makes that rarer, not impossible.
* **Keyed lists (ADR-027).** A signal's patches in one drain cost one decode and one list copy (TS, Kotlin)
  plus O(ops); measured on TS for 1,667 patches on 10,000 rows: 2.3 ms per frame becomes about 5 us. Swift
  mutates through `inout` (`generated/swift/.../Stores.swift:1666`); whether that copies the array depends on
  the `@Observable` accessors and on a view holding a reference, which the device phase measures; either way
  it saves the per-entry work. The out-of-bounds rule is unchanged.
* **Observe and restore (ADR-023).** Their change-sets go through the same queue; `observe` keeps its
  immediate drain; a restore's per-store change-sets may merge with nothing and apply in one drain.
* **Latency.** A change the core makes on its own reaches the UI at the next frame (at most one frame
  interval later than the next hop would have applied it, and rendering happened at the next frame anyway).
  A reply is never delayed.
* **Memory.** Bounded by decision 3 instead of by the main thread's speed; a backgrounded app or a hidden tab
  no longer accumulates every change-set it did not render.
* **Tests.** Each runtime: a property test that a random sequence of full values and patches (including
  out-of-bounds ones) applied through a merged drain equals sequential application; a backlog test (1 M
  change-sets with the main thread blocked stay under the bound and converge after one drain); a
  read-your-writes test (reply after change-set, state visible after the await); TS: worker batching and
  frame scheduling with a fake `requestAnimationFrame`. A new contract scenario **S18 coalesced burst**:
  `Stress.burst(Firehose, 1000)` is applied as one entry for the signal (the raw mirror callback runs once),
  the final value is exact, and `transactions` grew by 1,000.
* **Benchmarks.** The Rust harness (`bench/`) measures the core and is unaffected. The playground's stress
  screen measures the web mirror before and after this ADR with the same numbers; the device phase does the
  same for Swift and Kotlin.
* **Docs.** SPEC 11's sentence becomes the rules above; SPEC 17 lists the options, counters and the drain
  listener; `docs/` gains a "high-frequency data" page (what coalescing does, when to use `no_coalesce`, how
  to batch in `ctx.txn`).
