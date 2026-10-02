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

**Identity map.** `UndraCore.adopt(handle, make)` returns the live wrapper for `handle` (and gives the extra
reference back at once with `release`), or makes one with `make`, registers it weakly and returns it. Keyed by handle
number under the core's own lock. The wrapper's `close()`/finalizer releases the one reference it owns (at most once).
Constructors go through `adopt` too (an `Arc<Self>` constructor returns the same handle twice). Helpers generated code
calls (same names in all three): `adoptObject(body, make)`, `adoptOptional(body, make)`, `adoptList(body, make)` (decode
the reply body's handle(s), adopt each as it is read), `requireOwn(object)` (a typed refusal for an object of another
core, below). Swift's `UndraObject` is `Hashable` by identity.

**Foreign objects.** Two cores hand out the same handle numbers (a handle is a slot and a generation of one core's
table), so a handle means something only to the core that issued it. The generated code refuses to send an object of
another core: `core.requireOwn(target)` throws/rejects `UndraCallError.refused(reason)` (Swift `.refused`, Kotlin
`Refused`, TypeScript `Refused`) with a reason that names the object's class and says it belongs to another core, before
anything is sent; a command (a method that reports instead of throwing, ADR-032) reports it through `onError` and does
nothing. The core's own checks (stale, wrong type) remain the second line for a raw-API caller.

**Object parameters.** The wrapper's handle (`u64`) is written; `Option` writes a tag; a list writes a count. The
wrapper must stay reachable until the call is sent (`withExtendedLifetime` in Swift, `Reference.reachabilityFence`
in Kotlin; in TypeScript the finalizer cannot run inside the synchronous send).

**Callback registry.** `UndraCore.callbacks` (one per core): `lend(impl) -> UInt64` returns the instance handle the
host chose (non-zero, from a per-core counter, never reused), interning by object identity (a class instance in Swift
and Kotlin, an object in TypeScript) and counting one reference per crossing; `giveBack(instance)` takes one back (the
generated code does it when a call is refused, status 5, or never reached the core); `release(instance)` is what the
core's `__release` calls; the entry goes at zero. An over-release is logged as an error, never frees early.
`callbacks.liveCount` (and per-implementation `count(of:)`) is what the scenarios read.

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

**Weak wrappers** (generated per trait): Swift `WeakUploadListener`, Kotlin `UploadListener.weak(target)`, TypeScript
`weakUploadListener(target)`: forward while the target lives, then do nothing / answer unavailable.

**Stats.** `UndraStats` gains `hostRefs` (the core's `host_refs`); the mirror's stats `callbacksDelivered`; the registry's
live count.

**Generated shapes.** As in the ADRs, with the three names above; see each language's golden `object_graph` and
`callbacks` cases (`crates/undra-bindgen/tests/golden/`). Reserved ids a callback interface's id namespace carries:
`releaseInstance` and `cancelCall` (Swift/TS), `RELEASE_INSTANCE`/`CANCEL_CALL` (Kotlin). A plain object with no
constructor that some method returns is legal (no public constructor is generated).

## Contract scenarios

S27 (objects cross) and S28 (host callbacks), `contract-tests/scenarios.md`, on Swift, Kotlin and TypeScript, against the
playground core's `Workshop` / `Shelf` / `Watch` / `Reporter` (`examples/playground/core/src/workshop.rs`).

## Deviations, numbers, counts

(filled in at the end)
