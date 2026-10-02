# objects-callbacks: ADR-040 (objects as parameters and returns) and ADR-041 (host callback interfaces)

Piece `objects-callbacks`, wave 1 of `.10x/specs/2026-10-01-boundary-surface-plan.md`. Worktree
`wt/objects-callbacks`. The decisions are the ADRs' (their Decision sections are the specification); this record
holds what the implementation settled on, the contract the three platform runtimes implement, and (at the end)
the deviations, numbers and counts.

## What is built, by layer

### The core (Rust) — done first, the platforms build on it

* `undra-wire`: `Handle` is 24 bits of slot and 40 bits of generation (`Handle::new(index: u32, generation: u64)`,
  `MAX_INDEX`, `MAX_GENERATION`); the `Snapshot`'s `generation_floor` is a `u64`; `contract-tests/wire-vectors.json`
  moved with it (`handle`, `call_method`, `changeset_one`, `snapshot_v2`; the snapshot vector's floor is `4294967299`).
  Platform codecs: every runtime reads the floor as a `u64` and treats handles as opaque.
* `undra-meta`: `TypeRef::Object(name)` and `TypeRef::Callback(name)` (+ `TypeRefMeta`), `PortKind::Callback`,
  `PortDef.background` (written only when true), `MethodDef.coalesce` (written only when true); `Schema::validate`
  knows where an object (a method or function parameter or return, alone, `Option` or `Vec`; E0064) and a callback
  (a parameter, alone or `Option`; E0004) may stand, that a `Named` never names an object **except as the return type
  of the object's own constructors** (kept so no existing hash moves; a constructor returning `Arc<Self>` is recorded
  the same), and that a callback port's methods report or are `async` with a `Result` (E0071). Reserved callback
  method ids: `ids::callback_release_id(trait)` = `fnv1a32("<Trait>.__release")`, `callback_cancel_id`.
* `undra-macros`: `Arc<T>`, `&T`, `Option<..>`, `Vec<Arc<T>>` in object positions; `Arc<dyn Trait>` in callback
  positions; constructors returning `Arc<Self>`; `#[undra::callback]` / `(background)`; `#[undra(coalesce)]`; E0071;
  texts of E0001, E0004, E0064 changed; identity assertions for objects (E0061/E0064) and callbacks (E0004 bound).
* `undra-runtime`: the object table counts host references and interns by address (`issue_with`, `release` decrements,
  transient entries), `IssueScope` (the ledger: gives references back unless the reply carries them), origins
  (`call_from`, `release_from`, `release_origin`), `CallbackInterface`/`CallbackHandle`/`CallbackCall`, `Runtime::callback`
  (the proxy intern map), `Runtime::port_notify` (fire-and-forget port calls), `stats_json` `host_refs`,
  `live_callbacks`, `origin_refs`.
* `undra-transport`: a session's origin; its disconnect (or an expired/superseded retained session) releases what its
  calls returned.

### The platform contract (what Swift, Kotlin and TypeScript implement, with the same names)

**Identity map.** `adopt(handle, make)` returns the live wrapper for `handle` (and gives the extra reference back at once
with `release`), or makes one with `make`, registers it weakly and returns it. Keyed by handle number under the core's own
lock. The wrapper's `close()`/finalizer releases the one reference it owns (at most once). Named constructors go through
`adopt` too (an `Arc<Self>` constructor returns the same handle twice). Helpers generated code calls: `adoptObject(body,
make)`, `adoptOptional(body, make)`, `adoptList(body, make)` (decode the reply body's handle(s), adopt each as it is read),
`requireOwn(object)` (a typed refusal for an object of another core, below). Swift's `UndraObject` is `Hashable` by
identity. In Swift and Kotlin these are methods of `UndraCore`; **in TypeScript they are free functions that take the core
first** (`adopt(core, handle, type)`, `adoptObject(core, body, type)`, `requireOwn(core, object)`), so an app that uses none
of it ships none of it (ADR-052's gate on `web/hello-runtime-js`).

**Foreign objects.** Two cores hand out the same handle numbers (a handle is a slot and a generation of one core's
table), so a handle means something only to the core that issued it. The generated code refuses to send an object of
another core: `core.requireOwn(target)` throws/rejects `UndraCallError.refused(reason)` (Swift `.refused`, Kotlin
`Refused`, TypeScript `Refused`) with a reason that names the object's class and says it belongs to another core, before
anything is sent; a command (a method that reports instead of throwing, ADR-032) reports it through `onError` and does
nothing. The core's own checks (stale, wrong type) remain the second line for a raw-API caller.

**Object parameters.** The wrapper's handle (`u64`) is written; `Option` writes a tag; a list writes a count. The
wrapper must stay reachable until the call is sent (`withExtendedLifetime` in Swift, `Reference.reachabilityFence`
in Kotlin; in TypeScript the finalizer cannot run inside the synchronous send).

**Callback registry.** One per core: `lend(impl) -> instance` returns the instance handle the host chose (non-zero, from a
per-core counter, never reused), interning by object identity (a class instance in Swift and Kotlin, an object in
TypeScript) and counting one reference per crossing; `giveBack(instance)` takes one back (the generated code does it when a
call is refused, status 5, or never reached the core: Kotlin's `giveBackIfRefused(error, instance)`, TypeScript's
`lending(core, send, signal)`); `release(instance)` is what the core's `__release` calls; the entry goes at zero. An
over-release is logged (TypeScript: reported to `onError`), never frees early. `liveCount` and `count(of:)` are what the
scenarios read. Swift and Kotlin: `core.callbacks`; TypeScript: `callbacks(core)` and the free functions `lend(core, impl,
callback)`, `giveBack(core, instance)`. Swift and Kotlin install the interfaces at load (the generated entry passes them:
`UndraCallbackInterface` list, `CoreEntry(callbacks = ..)`); TypeScript registers an interface's port with the core the
first time an instance is lent (messages stay in order on every transport, so this is equivalent).

**Callback delivery.** The runtime registers one port per `#[undra::callback]` trait before `init` (the generated
entry passes the list, `UndraIds`' callback ports). The registered port callback **only enqueues** and returns `1`
(never runs app code inside the port callback: SPEC 6 host contract 2 and 4):
* `main` (default): the invocation joins the mirror's queue (ADR-031) in arrival order with the change-sets, as an entry
  kind that is never folded with change-set entries; at the drain it is applied in that order (so a listener sees the
  stores as they were when the core called it). `coalesce` methods keep only the newest pending invocation per
  (instance, method) in a drain. `MirrorStats.callbacksDelivered`. `__release` takes effect when the queue reaches it
  (after the invocations queued before it); `__cancel` takes effect at once (cancels the running task, or drops the
  invocation if it has not started).
* `background` (`PortDef.background`): a serial executor per instance runs the invocations in call order without
  waiting for a frame (Swift a serial executor, Kotlin `Dispatchers.Default.limitedParallelism(1)`, TypeScript a
  microtask).
* async methods start in call order and answer with `port_reply` (status 0 value, 1 `E`, 2 unavailable);
  `port_call_id 0` calls are fire-and-forget (nothing is replied).

**Errors.** An implementation that throws its own `E` answers status 1; any other throw is reported
(`UndraCore.report` → `onError`, operation = `Interface.method`) and answered status 2; a throw from a
fire-and-forget method is only reported.

**Cancellation.** `__cancel(instance, port_call_id)` cancels the task running the implementation: Swift `Task.cancel()`,
Kotlin `Job.cancel()`, TypeScript aborts the `AbortSignal` that is the method's last argument. A reply that arrives
anyway is discarded by the core.

**Weak wrappers** (generated per trait): Swift `WeakReporter`, Kotlin `Reporter.weak(target)`, TypeScript
`weakReporter(target)`: forward while the target lives, then do nothing / answer unavailable.

**Stats.** `UndraStats` gains `hostRefs` (the core's `host_refs`); the mirror's stats `callbacksDelivered`; the registry's
live count.

**Generated shapes.** As in the ADRs, with the three names above; see each language's golden `object_graph` and
`callbacks` cases (`crates/undra-bindgen/tests/golden/`). Reserved ids a callback interface's id namespace carries:
`releaseInstance` and `cancelCall` (Swift/TS), `RELEASE_INSTANCE`/`CANCEL_CALL` (Kotlin). A plain object with no
constructor that some method returns is legal (no public constructor is generated).

## Contract scenarios

S27 (objects cross) and S28 (host callbacks), `contract-tests/scenarios.md`, on Swift, Kotlin and TypeScript, against the
playground core's `Workshop` / `Shelf` / `Watch` / `Reporter` (`examples/playground/core/src/workshop.rs`).

## Platform shapes that differ from the draft contract above

The three runtimes were built by three implementers from the contract; where they differ from it, the code is right and
SPEC 17 says so:

* **TypeScript's helpers are free functions** that take the core first (`adopt(core, handle, type)`, `adoptObject(core,
  body, type)`, `requireOwn(core, object)`, `callbacks(core)`, `lend(core, impl, callback)`, `giveBack(core, instance)`,
  `lending(core, send, signal)`), not methods of `UndraCore`: `UndraCore` is the main entry's class, and a hello-world bundle
  that uses none of it must not ship it (ADR-052). Swift and Kotlin have them as methods. A TypeScript callback interface
  registers its port with the core the first time an instance is lent (the generated entry does not), and `lending` gives
  the references back when the call was refused or never sent but leaves them with the core when the caller aborts after
  the send; the registry drops its entries when the connection is lost or the core closes.
* **Kotlin**: `X(ctx)` is a companion `operator fun invoke` calling `X.create(ctx)` (a Kotlin constructor cannot return an
  existing wrapper, and constructors must go through `adopt`); the wrapper's own constructor is `internal`; generated
  code calls the runtime's `reachabilityFence` (the JDK's exists from API 28, the playground's minimum is 26); a stream
  that takes objects or callbacks fences and lends once per collection inside its `flow {}`, because a stream is sent when
  it is collected. `GoldenFullTests`' reflection calls `Companion.create`.
* **Swift**: a `new` constructor is a convenience initialiser and cannot return an existing object, so an `Arc<Self>`
  constructor that returns an interned object while its wrapper is alive gives a second wrapper that owns its own
  reference (the count is right, `===` does not hold; `UndraObject.init` registers itself so later returns of the handle
  find it); named constructors go through `adopt`. Generated callback protocols refine `Sendable`. The entry registers the
  callback ports after the core starts and before `load` returns. A stream method with object or callback parameters
  cannot throw, so it has no `requireOwn` and no give-back on refusal. A weak wrapper's async method called directly
  after its target is gone cannot return under typed throws (there is no `E` value to throw); the core never calls it in
  that state (the runtime resolves the target first and answers unavailable), and with `swift_typed_throws = false` it
  throws `.unavailable(.closed)`.
* **Delivery detail all three share**: a callback invocation is also a fold barrier in the mirror's queue (change-sets
  that arrive after a call are not folded into ones before it), so a listener never sees a newer store value than the one
  at the time of the call. Background-delivered async methods start in call order; their first steps on a cooperative
  pool are not strictly ordered.

## Deviations, numbers, counts

(Deviations from the two ADRs are in each ADR's "Implementation notes"; the ones below are the piece's.)

**Deviations from the brief and the ADRs (summary).**

1. Constructors keep `Named(Self)` in the schema (no hash moves). 2. The ledger is an RAII `IssueScope`. 3. Foreign handles
are refused on the host (`requireOwn` -> `UndraCallError.refused`), a gap the ADR text left (it said "fails as stale").
4. `MethodDef.coalesce` and `PortDef.background` are schema fields written only when true. 5. E0071 also rejects `__` names
and a coalesced async method. 6. The scenario numbers are S27 and S28 (S21..S25 were taken). 7. The docs are two guides
(`objects.html`, `callbacks.html`), not cookbook recipes. 8. The platform differences listed above. 9. S27 step 3 asserts two
change-sets (one per shelf; `transactions` counts change-sets), S28 step 2 asserts the last progress report is `3/3` and
reports increase (`progress` is `coalesce`), and Swift's column cannot write S28 step 3 against typed-throws protocols (the
runtime tests cover the mapping; the column prints a NOTE). 10. The Kotlin callback registry's `giveBackIfRefused` and
TypeScript's `lending` are how generated code gives references back on a refused call; the ADR only said "the generated host
code gives its references back itself".

**Known limits (not fixed here).**

* Kotlin: a caller's coroutine cancelled at the very moment a successful reply carrying an object arrives drops that reply,
  and its one reference stays with the core until the core closes (fixing it needs a "reply dropped" hook on
  `UndraCore.call`, an API change).
* TypeScript: after a crash-recovery restart (a wasm trap) the callback registry's entries are not dropped; the runtime has
  no restart hook for it yet.
* A callback over `undra dev` (remote) has unit tests (a dropped connection clears the registry) but no end-to-end WebSocket
  test; the remote path is the same `onPortCall` code.
* The playground web app's Stress tab fails its Playwright smoke ("Stress.stop failed: stale handle" on leaving the tab)
  with this branch's TS runtime and also with the pre-change one on this branch's core, so it comes from the core's
  changes (the wider handle), not the TypeScript work; recorded, not diagnosed.

**Numbers.**

* Bench rows (`bench/RESULTS.md`, finding 6; budgets in `bench/budgets.toml`): `dispatch/call_sync/object_param` 48.7 ns
  (the `add` row 49.4 ns: ratio gate `object_param_vs_bare_call` max 2.0, measured 1.0), `return_object` 84.0 ns,
  `return_interned_object` 84.0 ns (ADR-040's target: within 2x of `add`: 1.7x), `boundary/port_call/notify` 24.3 ns,
  `boundary/callback/notify` 43.2 ns, `boundary/callback/async_roundtrip` 182.7 ns. Host side (each runtime's own suite,
  release or JIT'd, one machine): Swift `adopt` 489 ns/op for a first adoption and 141 ns/op for one that finds a live wrapper
  (and releases the extra reference); Kotlin 15.5 to 22.5 ns/op (live wrapper), 190 to 204 ns/op (a new wrapper and its
  close); TypeScript 671 to 891 ns/op (a new wrapper), about 220 ns/op for the identity lookup alone and 1.0 to 1.3 us/op with its
  `Release` through the in-process fake transport.
* **Size** (`scripts/wasm-size.sh`): `web/hello-wasm` 118,929 gzipped (gate 120,000: ok; no wasm change, the build only
  grew by what the playground-independent hello template links). `web/hello-runtime-js` **26,313 gzipped against a gate of
  26,000 (record 25,996): 317 bytes of growth, 313 over**. About 97 bytes are the mirror's callback entries (the `Mirror`
  class always ships) and about 220 are `adopt` in every constructor, the late-finalizer handling, `_giveBack` / `_held` /
  `hostRefs` and the dispatch change; hello does not include `callbacks.ts`, `adoptObject` or `requireOwn`. Two trims gave
  nothing measurable. **Proposal**: raise `budget_gzip_bytes` of `web/hello-runtime-js` to 26,500 and re-record at 26,313 in
  the integrator's commit (`scripts/wasm-size.sh --record`), or let ts-size-e4 land first and re-measure; the budget is not
  changed on this branch.

**Counts.** (see the report that merged this piece; the matrix was run once at the end)

COUNTS_PLACEHOLDER
