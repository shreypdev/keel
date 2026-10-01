# Frame-coalesced delivery (ADR-031) — adversarial review

**Date:** 2026-10-01 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/coalesce` at `9d09055` (`main` fully merged) · **Read:** `CLAUDE.md`, ADR-031 (accepted, conditions (a)-(e)),
ADR-019/020/023/027/028, `docs/SPEC.md` 3.5, 3.8, 5.5, 11.1, 17.1-17.3, `docs/HIGH_FREQUENCY.md`, the SDE record
`.10x/decisions/sde/frame-coalesced-delivery.md`, the three mirrors and their reply paths, the TS worker protocol, the
`android-adapters` module, `undra-meta` / `undra-macros` / `undra-bindgen` changes and `tests/schema_hash.rs`,
`examples/playground/core/src/stress.rs`, `contract-tests/scenarios.md` S18 and the three runners · **Fixes:** `8b6e13c`.

## Verdict

The mirror-equals-core claim holds under coalescing. Per key, a drain applies the last full value (or lazy
invalidation) and then the concatenation of the patches that followed it; because SPEC 3.8 applies ops sequentially,
each index relative to the list the previous op left, and because every platform's `applyPatch` validates the whole
patch before it mutates anything (TS copies, Kotlin copies, Swift pre-validates), the merged patch is the same change
as the sequence, and an out-of-bounds op anywhere in it takes the same resynchronisation path as before. I could not
construct a sequence where concatenation and sequential application differ for a well-formed core (only a malformed
patch whose count disagrees with its bytes can be "healed" by its neighbour, I4). Model-based tests added in all three
runtimes (two stores, multi-entry change-sets, a wire that delivers random prefixes, drains at random points so
transactions straddle them, backlog bounds of 2-25 entries so compactions run mid-history, a `no_coalesce` signal,
out-of-bounds patches, resyncs answered asynchronously behind what the core already sent, bulk patches that a
compaction must drop) converge to the core's state for 30,000 (TS), 20,000 (Kotlin) and 20,000 (Swift) histories, and
between settles never show a value the signal never had; they fail when the awaiting-a-full-value discard is removed or
the full-before-patch order is swapped (mutation-checked). Two Medium findings were real and are fixed here: the
backlog's memory bound did not hold for patches of large items (the drop rule required both bounds, M1), and a Kotlin
drain chased a producer on another thread for up to 1,000 rounds, undoing the merge (M2). No High finding; nothing
High or Medium is open.

## Findings

| # | Sev | Where (at `9d09055`) | Finding | Status |
|---|---|---|---|---|
| M1 | Medium | `runtimes/ts/@undra/runtime/src/mirror.ts:196-198`, `runtimes/kotlin/.../Mirror.kt:659`, `runtimes/swift/.../Core/Mirror.swift:655-657`, `docs/SPEC.md:715` | A compaction dropped a merged patch only when it had **more than 4,096 ops and more than 1 MiB**. A patch of at most 4,096 ops has no byte bound then (4,096 updates of 64 KiB items is 256 MiB for one key), so SPEC 11.1's "memory is O(observed keys x (value size + 1 MiB))" was false. Decision below: **either** bound drops. TS test before the fix: 40 inserts of 64 KiB kept 2.6 MB pending against a 2 MiB budget. | **Fixed** (all three runtimes, SPEC, HIGH_FREQUENCY, Swift README; new test per runtime) |
| M2 | Medium | `runtimes/kotlin/.../Mirror.kt:450-471` (`drain`) | The drain took a further round whenever the queue was non-empty, including change-sets other threads delivered while it ran. With a producer on the core thread and a 20 us apply (a list copy), one drain ran 8 and 84 rounds in two runs (one apply and one list copy per round, 2.1 ms), and up to the 1,000-round cap: the per-frame merge degenerates to per-batch application exactly under the firehose ADR-031 targets, and the main thread can be held for 1,000 x apply. Swift already limited rounds to work the drain caused on the main thread. | **Fixed**: `moreRounds` set only by a main-thread submit inside a drain (or a due resync); other threads' change-sets wait for the frame they requested. Regression test `CoalesceModelTests` "a drain does not chase ..." |
| L1 | Low | `crates/undra-bindgen/src/model.rs:454-469` | `no_coalesce_ids` was inserted between `is_unit_enum`'s doc comment / `#[must_use]` and the function: `no_coalesce_ids` carried "Whether an enum's variants all carry no fields", `is_unit_enum` lost its doc (R4). | **Fixed** |
| L2 | Low | `runtimes/ts/@undra/runtime/src/transport/wasm-worker.ts:153` | The protocol 2 `envelopes` message was not validated: a non-list `data` threw a `TypeError` out of the message listener instead of failing the transport (each element is validated by `decodeEnvelope`). | **Fixed** (`protocol` transport failure) + test |
| L3 | Low | `crates/undra-macros/src/impl_/attrs.rs:233-240` | The teaching error for `#[undra(no_coalesce)]` on a non-signal store field exists (E0008, why, fix, docs link) but nothing pinned it, now that the flag reaches the schema (R8). | **Fixed** (unit test `no_coalesce_needs_a_signal_field`) |
| L4 | Low | `runtimes/swift/.../Core/UndraCore.swift:544`, `runtimes/kotlin/.../ConnectedCore.kt:317` | `restore` is a synchronous call but did not drain on the main thread: its values showed up a frame later (condition (b) says synchronous calls from the main thread drain before they return). | **Fixed** (`withImmediateDrain` / `drainIfOnMainThread`, SPEC 11.1 lists it) + a test each |
| L5 | Low | `runtimes/ts/@undra/runtime/src/core.ts:353`, `runtimes/kotlin/.../UndraCore.kt` `callSync` docs | Read-your-writes for a synchronous call made from inside a drain (a subscriber, a store's `apply`) is deferred to the drain's next round; Swift documents it, TS and Kotlin did not. | **Fixed** (docs) |
| L6 | Low | `contract-tests/kotlin/src/.../Check.kt:65,99,103` | Every Kotlin wait drains the mirror before it looks. That is right for S12 (see attack 9) but left the Kotlin column with no end-to-end check that frame-paced delivery happens on its own. | **Fixed**: a Kotlin-only last check in S18 (a burst made off the main thread reaches the store at a frame with no drain), noted in `contract-tests/kotlin/NOTES.md` |
| L7 | Low | `docs/SPEC.md` 11.1 "Immediately" | "`observe(on)` drains ... as soon as its initial change-set arrives over a worker or a socket" is TS only; Swift and Kotlin `observe` do not wait over a socket and apply it at the next frame (as before ADR-031, which posted it). | **Fixed** (SPEC wording) |
| L8 | Low | `docs/HIGH_FREQUENCY.md:76` | "If the main thread falls 65,536 entries behind" ignored the byte bound. | **Fixed** |
| L9 | Low | the three property tests (`coalesce.test.ts`, `CoalesceTests.kt`, `CoalesceTests.swift`) | They deliver single-entry change-sets of one store and drain once at the end: no straddling, no compaction, no `no_coalesce`, synchronous resyncs only. | **Fixed**: `coalesce-model.test.ts`, `CoalesceModelTests.kt`, `CoalesceModelTests.swift` (see the verdict) |
| I1 | Info | `runtimes/kotlin/undra-runtime/build.gradle.kts:5-6` | The root declares AGP `apply false`, so a JVM-only Gradle build resolves the Android plugin (network on first use) even without an SDK. `test-local.sh` and `typecheck_kotlin` are unaffected. | Open (note) |
| I2 | Info | design | Any drop rule, under an asynchronous transport at extreme rates (a compaction every frame and one key with more than a bound's worth of ops between its resync answer and the next compaction), can drop the answer again before a drain applies it, so that key lags until the rate falls. In process the resync is answered inside the drain. | Open (note) |
| I3 | Info | ADR-031 decision 2 | Read-your-writes covers replies and synchronous calls, not stream items: a consumer of a stream that writes a store and then yields sees the store at the next frame (before ADR-031 TS and Kotlin applied it before the item). The ADR lists streams among what is frame-aligned. | Open (note) |
| I4 | Info | the merge | Merging does not decode: a malformed patch whose count disagrees with its bytes can be completed by its neighbour into a valid-looking patch. Only a core bug produces one; sequential application would fail on it. | Open (note) |
| I5 | Info | Swift `UndraStore.init(core:handle:noCoalesce:)` | No longer an `override` of `UndraObject.init(core:handle:)`: generated code is unaffected (the default argument), a hand-written subclass that overrides `init(core:handle:)` no longer compiles. | Open (note) |

## Decision on the patch-size bound (decision 1 of the SDE record)

**Either bound drops**: a compaction drops a key whose merged patch has more than 4,096 operations **or** more than 1 MiB
of operation bytes. Memory is what decision 3 bounds, and only the byte bound bounds memory; under "both", a patch of up to
4,096 ops is never dropped whatever its size, so the per-key backlog is 4,096 x item size, not 1 MiB. With "either", what
survives a compaction is at most one full value plus 1 MiB per key, the next compaction comes at twice that, so the
backlog is O(observed keys x (value + 1 MiB)) as SPEC 11.1 says. The operations arm costs nothing in memory and caps what
a drain replays for one key, as the core's own op log does at 4,096; its price is a resync (one full value, at most one per
key per drain) for a key that collected more than 4,096 small ops while the main thread was more than 65,536 entries or
16 MiB behind. SPEC 11.1, `docs/HIGH_FREQUENCY.md`, the three runtimes and their tests now say "or".

## The attacks

**1. Merge soundness.** (i) Full value drops everything earlier for the key, patches included: `Slot.setFull`
(`mirror.ts:178-183`), `Slot.setFull` (`Mirror.kt:661-666`), `Slot.setFull` (`Mirror.swift:620-627`). (ii) `op 2` is
treated as a full value: it supersedes and is superseded, clears "awaiting", and patches after it are applied after it;
sequentially identical. Generated `apply` functions ignore `op 2` and no core path emits it for a keyed list. (iii)
Concatenation is sound by SPEC 3.8; the cases tried by hand and by the models: an op whose index exists only because of
a prior op, `Move` after `Insert`, `Clear` mid-sequence, an out-of-bounds op at a random position followed by more
patches (both orders resync and converge; `applyPatch` leaves the list unchanged on error on all three, so the
merged path never shows a half-applied patch), a patch for a key awaiting a full value (discarded: TS/Kotlin at the
fold, Swift on arrival plus `markAwaiting` for what was queued after the round was taken). Count overflow is refused
(Kotlin/Swift `addPatch` false past `u32`). (iv) Slots are created in first-arrival order and applied in that order;
per-key order is arrival order; across keys only the final state is observable inside a drain. (v) `no_coalesce`
entries are `Single` units at their arrival position, one `batch` each on TS; a compaction folds them (the bound wins),
documented in SPEC 11.1 and `HIGH_FREQUENCY.md` and tested on all three; the models check that the `no_coalesce` signal
sees a subsequence of its committed values ending with the last, and all of them when no compaction ran.

**2. Ordering.** The original property tests were too narrow (L9); the model tests cover multi-store transactions
straddling drains, realistic mixes (30 % scalars, 8 % list replacements, the rest 1-4-op patches, 1 in 30 corrupt) and
compactions. Runs: TS 30,000 seeds (85,041 compactions, 149,461 settled checks), Kotlin 20,000, Swift 20,000; the
original 600-seed properties run three times each with the suites.

**3. Read-your-writes.** TS: a reply queues the flush microtask *before* `resolve` (`core.ts:722`), so it runs before the
awaiting continuation (and before the generated method's own continuation); in a batched worker message, the one flush
microtask runs after the whole batch has been read, so a later reply in the batch sees earlier change-sets. Only
microtasks queued before the reply was handled can see the old state, and they are not the caller. `callSync` flushes
after the transport returns (`core.ts:361`); in `wasm-main` the change-sets arrive synchronously through the wasm import
during the call and are queued, the flush runs after the import returned, outside the core. Swift: `drainBeforeResuming`
(`Mirror.swift:355`) is called before the continuation is resumed (`UndraCore.swift:796`) and posts to
`DispatchQueue.main`; main-actor jobs are enqueued on the same serial main queue, so the drain runs first whether the
caller is on the main actor or hops to it later; a nested run loop services both alike. `callSync`, `construct`,
`observe` and now `restore` drain through `withImmediateDrain` (`Mirror.swift:381`). Kotlin: `drainSoon` posts before
`complete` (`ConnectedCore.kt:413`); with `Main.immediate` both go through the same `Handler`; a dispatcher that does not
run in posting order (Compose's frame-driven one) is covered by the drain in `call`'s `finally` (`ConnectedCore.kt:154`),
which runs inside the resumed continuation before the caller's code. Sync calls drain at `:121`, `:128`, restore at
`:321`. The nested case (a sync call from inside a drain) is deferred to the drain's next round on all three (L5).

**4. Backlog bound.** Each fold is O(n) and the next waits for `max(bound, 2 x size after the fold)`, so a queue that stays
near the bound (many distinct keys) folds every ~bound entries at O(2 x bound): O(1) amortised; no quadratic path
(TS 1,000,000 change-sets in ~1 s). Drop rule: see the decision above. Resync storms: a dropped key is re-observed once
per drain (the mark goes from `true` to `false` when the request is sent; patches stay discarded until a full value), so
re-observes are bounded by drains, not by producer rate; with the main thread blocked nothing is re-observed until it
drains (I2 for the extreme async case). Memory, with M1 fixed: per key one full value plus at most 1 MiB after a fold,
twice that before the next one; payload buffers kept alive by small values are released by the copies a fold makes
(`own`, `addOwned`, Swift copies every survivor).

**5. Frame alignment.** TS `scheduleFrame` (`mirror.ts:115-138`): rAF plus a 100 ms backstop, cleared by whichever
runs first; hidden documents use `setTimeout(0)` (throttled by the browser to 1/s or 1/min, harmless with the bound);
no document means a microtask. Swift: `CADisplayLink` in common modes, paused after each tick, unpaused through one
main-actor hop from other threads, never drains from inside `resume` (a core callback may be on the stack); main-actor
hop on macOS. Kotlin: `FramePacer` is a `fun interface` in `:runtime`; `PacedFramePacer` is one shared `undra-frame`
daemon posting on a 16.67 ms grid; `ChoreographerFramePacer` lives in `android-adapters`; `:runtime` has no `android.*`
import (grep) and stays stdlib + coroutines (condition (a)). Long background then foreground: frames stop (display link,
Choreographer for an invisible window, hidden tab), the queue folds in place, replies still drain immediately, and the
first frame after foregrounding drains once; the model's "settle" is that drain.

**6. Worker protocol.** Version 2 is announced in `init` (`wasm-worker.ts:209`); a worker batches only for a host that
announced it (`worker.ts:140`), so an old host gets single envelopes; a new host reads both shapes, so an old worker
works. Each envelope is a fresh `ArrayBuffer` (`encodeEnvelope`), the batch array is replaced before posting and
nothing touches a buffer after transfer. Control messages flush pending envelopes first, so order is kept. L2 fixed the
one unvalidated shape.

**7. Counters and listener.** `entriesApplied` counts calls of apply functions after merging (once per slot part, once
per `Single`); `drains` counts drains that found work; listeners run once per drain, after it, with `durationMs` /
`duration` from a monotonic clock, only when a listener is registered at the start; removal is idempotent on all three;
a throwing listener is reported (TS `onError`, Kotlin log) and the others still run; Swift listeners cannot throw.

**8. Schema.** `SignalDef.no_coalesce` has `#[serde(default, skip_serializing_if = "is_false")]`; `schema_hash.rs` pins
the nine golden hashes from `main` (checked against `main`'s generated `UndraIds`: all nine match) and that a set flag
changes the hash. Bindgen passes the ids in signal order, only for stores that have some, so every other generated file is
byte-identical; the macro records the flag in `SignalMeta` and rejects it off a signal field with E0008 (L3 pins it).

**9. Contracts.** S18 proves the claim on each platform: the raw callback ran once with the final value 1,000 when the
call returned, `changeSetsReceived` grew by 1,000 and `entriesApplied` by 1 (so the core really sent 1,000 change-sets
and the mirror merged them), `transactions` grew by 1,000, the generated store reads 1,000 on return, and the
`no_coalesce` signal was applied ten times. The Kotlin harness change is sound: S12 polled a store as if it were the core
and refetched while the first fetch was still in flight because `fetching = true` had not been applied yet; that is the
documented frame lag for what the core does on its own (the scenario thread is not the main thread, so read-your-writes
does not apply), not a staleness bug, and draining before each look reads the store as a UI reads it at a frame. It did
remove the Kotlin column's end-to-end coverage of the frame path, restored by L6.

**10. Native review (R3).** Generated: TS `super(core, handle, { noCoalesce: [0] })`, Kotlin
`UndraStore(core, handle, noCoalesce = setOf(0u))`, Swift `super.init(core: core, handle: handle, noCoalesce: [0])`: each
is what an engineer of that language would write. Mirror APIs: TS `addDrainListener(fn): () => void` like `subscribe`;
Kotlin `addDrainListener {}: AutoCloseable` (`use {}`), `MirrorStats`/`DrainStats` plain classes like `UndraStats`;
Swift `addDrainListener {} -> DrainListenerRegistration` with `remove()`, like a `NotificationCenter` token (it does not
remove on deinit, which is documented), `MirrorStats: Sendable, Equatable`. `MirrorOptions` (Kotlin) and the
`LoadOptions` fields (Swift) read naturally. I5 is the one source-compatibility note.

## The SDE record's open choices, judged

1. "Both bounds" — **overturned** (M1, decision above).
2. The bound wins over `no_coalesce` — agreed; documented and tested on all three.
3. Compaction amortisation by doubling — agreed (attack 4).
4. A patch too short for its count is dropped and re-observed — agreed (better than handing it to a decoder).
5. TS rAF plus a 100 ms backstop, a zero-delay task while hidden — agreed.
6. TS read-your-writes from a microtask queued before `resolve` — agreed; draining inside the wasm import would run
   subscribers inside a core callback.
7. Observe waiters drain from a microtask — agreed.
8. Resync sends only `Observe(on)` — agreed: `observed` is a flag, not a count (`store.rs` `start_observing`), and
   re-observing re-baselines the keyed list (SPEC 16.1).
9. Worker protocol version in `init` — agreed.
10. Two Kotlin `register` overloads — agreed.
11. Extra rounds — agreed for Swift and TS (one thread), **overturned for Kotlin** (M2).
12. Swift reply drain on `DispatchQueue.main` — agreed (attack 3).

## Verification after the fixes

| Check | Result |
|---|---|
| `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` (with `GRADLE_HOME`, so `typecheck_kotlin` compiles the goldens against the fixed runtime) | 2,168 passed, 0 failed, 10 ignored |
| TS runtime `npm test` + `npm run typecheck` | 931 passed (927 + 4); typecheck clean |
| Kotlin `scripts/test-local.sh` | 500 cases, 0 failed, 2 skipped (495 + 5) |
| Swift `swift test` | 425 passed (421 + 4), no warnings |
| `crates/undra-ffi/tests/{wasm,c,swift}/run.sh` | 29 passed / ok / passed |
| `contract-tests/run-all.sh` | ts 18/18, kotlin 18/18 (S18 with the new frame-path check), swift 18/18: 54/54 |
| playground web `npm test` | 64 passed |
| Model tests at scale | TS 30,000, Kotlin 20,000, Swift 20,000 histories: pass |
