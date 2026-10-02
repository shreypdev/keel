# types-paging / tp-swift: `UndraLazyList` and `UndraLazyListObject` (ADR-043 decision 3, Swift half)

Branch `wt/tp-swift`, owner of `runtimes/swift/**` and `contract-tests/swift/**`. The contract is
`.10x/decisions/sde/types-paging.md` ("Lazy lists on the platforms"). Nothing outside `runtimes/swift/` and this record changed.

## What landed

| Where | What |
|---|---|
| `Sources/UndraRuntime/Wire/LazyPayloads.swift` | `UndraLazyValue { handle, len, version }`, `UndraLazyInvalidated { len, version }`, `UndraLazyPageHeader { version, total, count }`: public, `UndraPayload` (so `encode()`/`decode(_:)` exist), the layouts of `undra_wire::payload`. The page call is the existing target 3 (`CallTarget.lazyListPage(handle:offset:limit:)`, `method: 0`, `args: []`); its reply body is `UndraLazyPageHeader` + `count` items, read from `UndraCore.callSync` / `call`. |
| `Sources/UndraRuntime/Lazy/UndraLazyListEngine.swift` | `UndraLazyListEngine<Item>` (internal, `@MainActor`): the page cache, the requests, the version rules, the window, the hostile-input checks; `UndraLazyListError` (public, the typed errors). |
| `Sources/UndraRuntime/Lazy/UndraLazyList.swift` | `UndraLazyList<Item: UndraCodec & Sendable>` (`@MainActor @Observable`, `@available(iOS 17, macOS 14, *)`) and its twin `UndraLazyListObject<Item>` (`@MainActor ObservableObject`, every floor): thin shells over one engine. |
| `Tests/UndraRuntimeTests/LazyList{Support,Tests,HostileTests,ObservationTests}.swift`, `WireVectorTests.swift` | 74 new tests; the three `lazy *` wire vectors are real checks now (the `return true` placeholder is gone). |
| `README.md` | layout rows and a short "Lazy lists" section. |

## The API (what `tp-bindgen` generates against)

```swift
public final class UndraLazyList<Item: UndraCodec & Sendable>      // and UndraLazyListObject<Item>
    public init(core: UndraCore)
    public func applyFull(_ reader: inout UndraReader) throws            // op 0 (a LazyValue); reads and finishes the reader
    public func applyInvalidated(_ reader: inout UndraReader) throws     // op 2 (a LazyInvalidated)
    public private(set) var count: Int
    public subscript(position: Int) -> Item?                              // RandomAccessCollection: Element == Item?, Index == Int
    public func prefetch(_ range: Range<Int>)
    public var pageSize: Int            // 50, clamped to 1...1,048,576; a change drops the cache
    public var maxCachedPages: Int      // 24, at least 1
```

`init(core:)` is main-actor isolated (a store's initializer is). Generated stores call `try books.applyFull(&reader)` for
`.fullValue`, `try books.applyInvalidated(&reader)` for `.lazyListInvalidated`, and ignore `.keyedPatch`. The conformance to
`RandomAccessCollection` is `@preconcurrency` (the witnesses of a `@MainActor` class cannot satisfy the protocol's nonisolated
requirements in Swift 6 mode otherwise, and isolated conformances need a newer runtime than the iOS 15 floor): using the
collection off the main actor fails the runtime's isolation check, like any main-actor API used from the wrong place.

## Behaviour, and the decisions the contract left open

* **Reads queue, flushes send.** `list[i]` registers its observation dependencies, then (inside `0..<count`) *wants* the page and
  its two neighbours: touches them (LRU tick) and queues those that are not cached-and-current, in flight or queued. A flush,
  scheduled once per main-actor turn (`Task { @MainActor }`; tests drive it by hand through an internal `schedule:` seam), sends
  the batch in page order. No request is made, and no observable property changes, from inside a read (SwiftUI forbids it).
* **One call per page**, not one ranged call per run of pages: the call count of a batch is its page count, whatever the
  runtime. `callSync` when `transport.supportsDirectSync` (in process), else one `Task` per page awaiting `core.call` (so the
  order the calls leave in over a remote core is the runtime's, not the page order).
* **Window = pages touched since the previous invalidation** (reads, prefetch, and the neighbours a read prefetches; the pages an
  invalidation re-pages are *not* re-touched, the re-render that follows their arrival does it). An invalidation queues the window
  pages that are not already in flight; stale pages outside the window are kept (shown stale) and asked for again when read. The
  window cannot exceed the cache: eviction (LRU by touch tick, outside-window pages first) keeps `pages.count <= maxCachedPages`
  as a hard bound, so "never evicted while in the window" holds as far as the bound allows. A flush asks for at most
  `maxCachedPages` pages (the most recently touched), so `for row in list` over 50,000 rows asks for 24 pages, not 1,000.
* **Version rules.** Reply `version < list.version`: dropped, page queued again (at most 3 times in a row, then
  `UndraLazyListError.staleReplies` is reported; any later read asks again). Reply `version > list.version`: the engine does what an
  invalidation would (`adopt(total, version)`: new length, stale pages, window queued) and then files the page; the op 2 that
  follows carries a version that is not newer and changes nothing. A reply at the current version whose `total` differs from the
  list's length is refused. An op 0 with the same handle is a refresh (same rules), with a new handle a **restart**: cache,
  in-flight set and queue are dropped, replies of the old epoch are discarded when they arrive. Changing `pageSize` bumps the
  epoch the same way. An op 2 before the first op 0 is ignored (there is no page server yet).
* **Hostile replies** (never a trap, always `UndraCore.report(_, operation: "UndraLazyList.page(<n>)")`, so `LoadOptions.onError`
  receives an `UndraUnhandledError` carrying `UndraCallError.malformed`): `count > limit`; `count != min(limit, max(0, total -
  offset))` (truncated or padded items, "wrong total"); a total that disagrees with the list at the same version; an item that does
  not decode, or trailing bytes; a header that does not fit. `count` is checked against `limit` before anything is allocated.
  A null handle in op 0 and op 0/2 values that do not decode throw to the caller (the generated store reports them), leaving the
  list as it was. A failed page call (shut-down core, bad request) is reported and the page is asked for again at the next read.
* **Observation.** `UndraLazyList`: stored tracked `count` and a private tracked `revision`; the subscript reads both, so a row
  depends on the length and on pages arriving or being replaced; `revision` is bumped once per commit that touched pages. Changes
  are committed once per operation: a flush (all the pages of its batch, and any change-set its calls' drains applied) commits at
  its end; a reply that arrives on its own (async) commits itself. The twin sends `objectWillChange` **exactly once per commit**: a
  commit that changes the length sets `count` only (`@Published`), one that only changes pages bumps a private `@Published
  revision`, so a view never sees two sends for one change.
* **Lifetime.** The list holds the core and the handle number; it never releases anything (the page server belongs to the core's
  store: `transport.releases` stays empty when a list is dropped, tested). Nothing retains a list but its owner (the engine's hook
  and the scheduled flush hold it weakly), so there is no `close()`.
* **Re-entrancy.** `callSync` drains the mirror before it returns, so an op 2 (or op 0) can be applied *inside* a batch's call.
  The engine keeps no local copy of its state across a call: after each call it re-checks the epoch, handle and whether the page is
  still wanted (`needsRequest`), so a page that an invalidation queued and the batch has since loaded is not asked for twice
  (tested through a store fed by real change-sets: `testAnInvalidationInsideACallsDrainMakesItsReplyStaleAndTheRowsArrive`).

## Counts and commands

* `cd runtimes/swift/UndraRuntime && swift test`: **782 tests, 0 failures** (708 before: the 553 in `.10x/status.md` is the
  checkpoint-17 figure; this tree's baseline is 782 - 74 = 708); my suites alone: `swift test --filter LazyList` 75 (74 + the
  existing `PayloadTests.testCallLazyListPageCarriesNoArguments`).
* `scripts/ios-floor.sh runtime`: the package builds for the iOS 15.0 simulator and for macOS 12, no warnings.
* `bash runtimes/swift/scripts/sync-vectors.sh --check`: up to date.

## Notes for the Swift scenario columns (S29 to S31, to do after the integrator's merge)

* The runner reads rows through the generated store's list property; flushes are on the main actor, so a scenario `await`s a
  main-actor turn (`Task.yield()` loop or `waitUntil`) after reading, as the contract tests already do for frame delivery.
* Page calls are visible to a harness only through a port-level counter if there is one on the core (the engine's `callSync` calls
  go straight through `UndraCore`): count them from the Rust side (`LazySource` calls) or wrap the transport; `UndraStats` has no
  page counter (the contract does not ask for one).
* In process every page call is `callSync` and so applies the change-sets that arrived before its reply (the S29 "version race"
  step needs a core that changes the list while a page call runs; with the playground's single thread, assert it through the
  invalidation-after-reply order instead).
* `contract-tests/swift/run.sh --floor` must also run S29-S31 against the `ObservableObject` bindings: the twin is
  `UndraLazyListObject`; its `count`, subscript and `objectWillChange` behave as `UndraLazyList`'s.

## Deviations from the ADR and the contract

None of substance. Differences of spelling: the ADR sketches `UndraLazyList<Element>` with `Element == Element?`; the contract names
the generic `Item` and says `Element == Item?`, which is what is built. `pageSize` is clamped (a zero or negative page size would
divide by zero; an absurd one cannot fit the wire's `u32` `limit` and a reply this host would decode). The conformance is
`@preconcurrency` (above).

## Open items

* The batch order over a remote transport is not the page order (one task per page); nothing depends on it.
* No Swift benchmark row: the page call is the existing boundary and `lazy/page_50_of_100k` is the Rust half's; a cached read costs
  about 5 µs in a debug build (`testReadingCachedRowsIsCheap` has a 5 s budget for 100,000 reads).
* Row identity across a re-page is by index (ADR-043's risk); keyed identity can follow.

## Addendum: the mirror's lazy rule (ADR-031 amendment, requested by the integrator)

A full value (op 0) supersedes everything queued before it for its signal; a lazy invalidation (op 2) supersedes only earlier lazy
invalidations of the same signal, never the op 0 (which carries the page-server handle). `Core/Mirror.swift`: a fold `Slot` now holds
`full`, `invalidated` and `patches`; `setFull` clears the other two, op 2 replaces `invalidated`. A drain applies a signal's last
full value, then its merged patch, then its last invalidation (a signal is a keyed list or a lazy list, so in practice at most two
applies); the compaction keeps the same three. The waiting rule (a signal whose patch could not be merged waits for a full value)
now ends on op 0 only, in `fold`, `enqueue` and `markAwaiting`: an op 2 for a waiting signal is dropped like a patch (the full value
that ends the wait supersedes it anyway). Tests (`CoalesceTests`, replacing `testALazyInvalidationSupersedesWhatCameBeforeIt`):
`[Full, Inv]`, `[Full, Inv, Inv]`, `[Inv, Inv]`, `[Inv, Full]`, `[Inv, Full, Inv]`, `[Full, Full, Inv]`, a restore's new handle then an
invalidation, a keyed signal beside a lazy one, mixed streams, `no_coalesce`, a compaction at a bound of 4 entries (the handle
survives), a waiting signal, and a 60-seed property test (random drain and compaction points converge to the state of applying every
entry in order); `LazyListObservationTests`: a drain folding `[Full(handle)]` + two invalidations ends with the handle and the newest
length/version, and a restored handle followed by an invalidation pages the new server. The README's frame-coalescing bullet and the
`Mirror` header comments say the same. `docs/SPEC.md` section 11 (line 973) is the integrator's.

Counts after the addendum: `swift test` 792 tests, 0 failures (782 + 10); `scripts/ios-floor.sh runtime` builds for the iOS 15.0
simulator and macOS 12.
