# ADR-041: host callback interfaces are port instances: `#[undra::callback]` traits the host implements and passes in

Status: **Accepted** (2026-10-02, implemented in `wt/objects-callbacks`; see "Implementation notes" at the end for what
the code decided where this text left room, and the deviations). Proposed 2026-10-01 (`wt/boundary-adrs`; Amendment B
"boundary surface", catalogue M-4).
Touches SPEC 1 (a new concept), 2.1 (`TypeRef::Callback`), 2.2 (`PortKind::Callback`), 3.6 (two reserved
methods on a callback port), 4 (a seventh-plus attribute, `#[undra::callback]`), 5.7, 10.1–10.3, 11 (delivery),
12 (E0004's help, E0071) and 17 (the runtimes' callback registry); `undra-meta`, `undra-macros`,
`undra-runtime`, `undra-bindgen`, the three platform runtimes. **No C ABI, wasm ABI or envelope change: a
callback call is a port call (SPEC 3.6) whose arguments start with the instance's handle.** Schema: one new
`TypeRef` variant and one new `PortKind` value; no existing hash moves. Constitution R5, R6, R11, R12.

## Context

Host code reaches the core in exactly one way today: a **port**, a trait whose single implementation the host
registers globally by id (`undra_port_register(port_id, cb, user)`, SPEC 6; `PortTable` in
`crates/undra-runtime/src/ports.rs:109-199`). Anything per-call or per-object — a progress listener for one
upload, a delegate for one screen, a token provider for one account — has no shape:

* `Box<dyn Fn(..)>`, `fn(..)`, `impl Fn` and `dyn Trait` are **E0004** "trait object … cannot cross the
  boundary … callbacks into the platform are ports (`#[undra::port]`)" (`crates/undra-macros/src/impl_/types.rs:332-351`,
  `:439-447`); in a record field the same is E0012.
* The catalogue (`.10x/specs/2026-10-01-competitive-limitations.md`) puts it beside objects as "the first wall a
  UniFFI migrant hits": UniFFI's "traits marked `with_foreign` can be implemented in Swift or Kotlin and passed
  into Rust" (UNI-B1), KMP exports lambdas (KMP-B8), flutter_rust_bridge advertises that "Rust can also call
  Dart" (FL-B6). Matrix row 23 footnote: "Ut: … host code only through global ports (E0004)."

The port machinery already has every piece a per-instance callback needs: a dyn-compatible trait rewrite (the
port macro desugars `async fn` into boxed futures, `crates/undra-macros/src/impl_/port.rs:1-40`), a generated
proxy that encodes arguments and awaits a `PortFuture`, typed outcomes through `E: From<PortError>`
(ADR-025, E0033), a host callback that may answer later through `undra_port_reply`, and fire-and-forget calls
with `port_call_id 0` (the `Log` path, `crates/undra-ffi/src/session.rs:89-108`; replies to id 0 are dropped,
`crates/undra-ffi/src/api.rs:202-236`, ADR-026 §4). What it lacks is an **instance** and a **lifetime**.

The host contract fixes the threading: callbacks run "on the thread that produced the event … possibly with the
core lock or a store's delivery lock held … concurrently" and "must not call back into the core" (SPEC 6 host
contract 2 and 4, ADR-020, ADR-026). App code in a listener calls into the core constantly (a delegate that
refreshes a store). So app code must never run inside the port callback.

## Decision

1. **A callback interface is a trait marked `#[undra::callback]`.**

   ```rust
   #[undra::callback]
   pub trait UploadListener {
       /// Bytes sent so far.
       #[undra(coalesce)]
       fn progress(&self, sent: u64, total: u64);
       /// Asks the user whether to replace an existing file.
       async fn confirm_replace(&self, name: String) -> Result<bool, PromptError>;
   }

   #[undra::api]
   impl Uploader {
       pub async fn upload(&self, file: FileRef, listener: Arc<dyn UploadListener>) -> Result<UploadId, UploadError> { .. }
       pub fn watch(&self, listener: Arc<dyn UploadListener>) -> Arc<Watch> { .. }   // ADR-040: close the Watch to stop
   }
   ```

   Method shapes: **fire-and-forget** `fn m(&self, ..)` returning `()`, or **async** `async fn m(&self, ..) ->
   Result<T, E>` with `E` an `#[undra::error]` enum that implements `From<PortError>` (E0033, as for ports). A
   synchronous method that returns a value, and an async method without a `Result`, are **E0071**: "the host
   runs a callback outside the core's thread and lock, so the core can never wait for it synchronously, and a
   host implementation can always fail or be gone; make it `async` and return `Result<T, E>`". Parameters are
   value types (no objects, no other callbacks). `Send + Sync` supertraits are added and `async fn` is
   desugared exactly as `#[undra::port]` does, so `Arc<dyn UploadListener>` is a normal Rust trait object:
   Rust code and tests implement the trait directly and pass their own `Arc`.
2. **Positions.** `Arc<dyn Trait>` and `Option<Arc<dyn Trait>>` as **parameters** of methods, constructors and
   free functions. Not in returns, fields, signals, stream items, ports, queries or mutations (E0004 with a help
   text per position). E0004's help for closures and `dyn Trait` becomes "declare a `#[undra::callback]`
   trait and take `Arc<dyn Trait>`".
3. **Schema.** A `PortDef` with `kind: Callback` (`port_id = fnv1a32("port.<Trait>")`, method ids as for ports;
   a name is a port or a callback, never both), and `TypeRef::Callback(String)` naming it. The new values are
   written only where used, so no existing hash moves.
4. **Wire.** A callback value is a `u64` **instance handle** chosen by the host (non-zero, from a per-core
   counter, never reused). A call into an instance is an ordinary PortCall (SPEC 3.6) on the trait's
   `port_id` whose `args` are `instance u64` followed by the parameters:
   * fire-and-forget methods use `port_call_id 0` (no reply is expected or read);
   * async methods use a normal `port_call_id`, answered with `undra_port_reply` (status 0, 1 = `E`, 2 =
     unavailable) exactly like an async port;
   * two reserved methods per callback port, always fire-and-forget: **`__release`** (`fnv1a32("<Trait>.__release")`,
     args `instance u64`) and **`__cancel`** (`fnv1a32("<Trait>.__cancel")`, args `instance u64, port_call_id
     u32`). The macro rejects user methods whose name starts with `__`, and bindgen's id-collision check covers
     the reserved ids.
5. **Ownership: one crossing, one reference** (ADR-040's rule, in the other direction).
   * Every instance handle in a call's arguments is **one reference the core now owns**. The host's registry
     maps handle → (implementation, count) and **interns by object identity** (a class instance in Swift and
     Kotlin, an object in TypeScript), so the same listener passed twice is the same handle with count 2.
   * The core interns too: decoding a handle it already has a live proxy for returns that proxy (so
     `Arc::ptr_eq` holds for the same host listener) and gives the duplicate reference back at once with
     `__release`. A proxy's `Drop` sends one `__release`. The host frees its entry when the count reaches zero.
   * **A refused call transfers nothing.** A dispatcher decodes callback arguments into *pending* proxies that
     become owned only when every argument has decoded; a status-5 answer therefore means "the core holds no
     reference from this payload" and the generated host code gives its references back itself. Every other
     status means the core owns them (a panic in the body drops the proxies, which releases them).
   * Proxies hold a `WeakCtx` (ADR-034): a listener kept by a long-lived store does not keep the runtime alive;
     after shutdown a fire-and-forget call is a no-op and an async call is `E::from(PortError::Cancelled)`.
6. **Threading and delivery: never under the core lock, never inline.** The host's callback port (one per
   trait, registered by the runtime at load, before `undra_init`) only **enqueues** and returns `1`; the app's
   implementation runs later:
   * **`main` delivery (the default)** runs the implementation on the main thread **through the mirror's
     drain** (ADR-031): an invocation is queued with the change-sets in arrival order and applied in that
     order, so a listener sees the stores as they were when the core called it (the change-sets committed before
     the call are applied first). Invocations are never folded with change-set entries; a method marked
     `#[undra(coalesce)]` keeps only the newest pending invocation per (instance, method) in a drain (progress
     reporting). Latency: at most one display frame, like any delivery.
   * **`background` delivery** (`#[undra::callback(background)]`) runs the implementation on a serial executor
     per instance (Swift: a serial `DispatchQueue`-backed executor; Kotlin: `Dispatchers.Default.limitedParallelism(1)`
     per instance; TypeScript: a microtask), in call order, without waiting for a frame. For services that are
     not UI (token providers, analytics sinks).
   * Async invocations start in call order; their completions are unordered.
7. **Cancellation.** When the Rust future awaiting an async callback is dropped, the proxy sends `__cancel`
   with the call's `port_call_id` (and the runtime abandons the id as for any port call, SPEC 5.2). The host
   cancels the task running the implementation: Swift `Task.cancel()`, Kotlin `Job.cancel()`, TypeScript
   aborts the `AbortSignal` passed as the method's last argument. A reply that arrives anyway is discarded by
   the core, as today. This needs no new ABI entry (Amendment C item 6 defers *port* cancellation, which would
   add a host callback, to v1.2; that ADR should consider this reserved-method mechanism first).
8. **Errors from the host.** An async implementation that throws its `E` answers status 1 (`E` encoded); any
   other throw is reported on the host (`UndraCore.report` → `onError`, ADR-032 and its C4b analogue) and
   answered status 2, which the proxy turns into `E::from(PortError::Unavailable)`. A throw from a
   fire-and-forget implementation is reported the same way and goes nowhere else.
9. **Strong by default, weak on request.** The host registry holds the implementation strongly while the core
   holds a reference (a listener created inline must not vanish). That makes a cycle possible: a view model
   holds an `Uploader` wrapper, the core's `Uploader` holds the listener, the listener is the view model. The
   generated code offers the way out per trait — `WeakUploadListener(target)` (Swift, a forwarding class with a
   `weak` reference), `UploadListener.weak(target)` (Kotlin, `WeakReference`), `weakUploadListener(target)`
   (TypeScript, `WeakRef`) — whose methods do nothing (fire-and-forget) or answer unavailable (async) once the
   target is gone; and the cookbook teaches the Rust-side pattern of decision 1's `watch`: return an object
   (ADR-040) whose `close()` drops the listener.

### Generated shapes

```swift
/// (docs of the trait)
@MainActor
public protocol UploadListener: AnyObject {
    /// Bytes sent so far.
    func progress(sent: UInt64, total: UInt64)
    /// Asks the user whether to replace an existing file.
    func confirmReplace(name: String) async throws(PromptError) -> Bool
}
public final class WeakUploadListener: UploadListener { public init(_ target: any UploadListener) … }

// on Uploader:
public func upload(file: FileRef, listener: any UploadListener) async throws -> UploadId
```

With `background`: `public protocol UploadListener: AnyObject, Sendable` with nonisolated requirements. Typed
`throws(E)` follows `Generator::swift_typed_throws`, as for port requirements (ADR-032).

```kotlin
interface UploadListener {
    /** Bytes sent so far. */
    fun progress(sent: ULong, total: ULong)
    /** Asks the user whether to replace an existing file. Throws [PromptError]. */
    suspend fun confirmReplace(name: String): Boolean
    companion object { fun weak(target: UploadListener): UploadListener = … }
}
// on Uploader:
suspend fun upload(file: FileRef, listener: UploadListener): UploadId
```

```ts
export interface UploadListener {
  /** Bytes sent so far. */
  progress(sent: bigint, total: bigint): void;
  /** Asks the user whether to replace an existing file. Rejects with `PromptError`. */
  confirmReplace(name: string, signal: AbortSignal): Promise<boolean>;
}
export function weakUploadListener(target: UploadListener): UploadListener;
// on Uploader:
upload(file: FileRef, listener: UploadListener): Promise<UploadId>;
```

## Alternatives considered

* **Closures as parameters (`impl Fn(u64)`/`Box<dyn Fn>`).** A closure has no name to generate a protocol
  from, no documentation, no error type and no way to say "async with a typed error"; one-method traits cover
  the same ground with native shapes (a Kotlin `fun interface` could be added later for one-method traits).
* **Synchronous callbacks that block the core until the host answers (UniFFI's model).** The core calls with
  its lock held (SPEC 5.1); a host that runs app code there either deadlocks (the app calls the core) or blocks
  every other caller. ADR-038 rejected the same thing for JavaScript sync ports. Fire-and-forget plus async
  covers listeners, delegates and providers.
* **A new C ABI entry per callback (`undra_callback_register`, `undra_callback_release`).** Reopens the
  19-function ABI (ADR-026, ADR-038's "calls the 19 functions exactly") for something the port path already
  carries. Every host, including React Native's C++ module, gets callbacks by handling one more `port_id`.
* **Host-side dispatch on the calling thread for `background` callbacks.** Faster by a hop, but it runs app
  code under the core lock; one hop to a serial executor is the price of R6 and the host contract.
* **Callbacks as returns (the core handing a host object back).** No use case that a parameter or an ADR-040
  object does not serve better; it would need the reverse adoption path in every runtime.
* **Weak references by default.** Delegates are weak in UIKit, but a listener object created inline
  (`upload(file, listener: ProgressPrinter())`) would be freed immediately and the upload would report into
  nothing. Strong by default, weak on request, cycle documented.

## Consequences

* Listeners, delegates, providers and confirmation prompts cross with native shapes on every platform, with
  ordered delivery, typed errors and cancellation; catalogue row 23 moves to "yes" with ADR-040.
* The runtimes gain a callback registry (handle → implementation, count) and one generic callback port bridge
  per trait; the mirror's queue (ADR-031) gains a non-folding entry kind for `main` invocations, counted in
  `MirrorStats` (`callbacksDelivered`).
* React Native (ADR-038) needs nothing extra: its C++ module already queues every non-event port call for
  JavaScript, which is exactly the `main` delivery path there.
* `undra dev` remote sessions carry callback port calls as PortCall envelopes; a disconnect drops the host's
  registry, and the core's proxies then answer unavailable until they are dropped.

## Risks

* **Cross-language cycles** (decision 9): mitigated, not eliminated; documented with the weak wrappers and the
  subscription-object pattern.
* **Floods.** A core that calls a `main` fire-and-forget method in a tight loop grows the mirror queue; the
  queue's byte bound (ADR-031) counts callback entries, `coalesce` exists for the common case, and a drop past
  the bound is counted and reported, never silent.
* **Host bugs releasing twice** would free a listener the core still calls; the runtimes count per handle and
  log an over-release as an error instead of freeing early.

## Implementation brief

1. `crates/undra-meta`: `PortKind::Callback`, `TypeRef::Callback` (+ `TypeRefMeta`), validation (a callback
   port's methods are fire-and-forget or async-with-`Result`; `Callback` names a callback port; reserved method
   ids), canonical-JSON hash-stability tests.
2. `crates/undra-macros`: `#[undra::callback]` (and `(background)`) reusing `port.rs`'s trait rewrite, proxy
   and `PortDispatcher` generation with an instance handle; `#[undra(coalesce)]` on fire-and-forget methods;
   E0071; `types.rs` maps `Arc<dyn Trait>` in parameter position to `KType::Callback`; dispatchers decode
   callback arguments into pending proxies and commit them after the last argument; E0004 help texts. Facade
   re-export (`undra::callback`).
3. `crates/undra-runtime`: `CallbackProxy` support — the per-runtime proxy intern map (handle → `Weak`),
   `__release` on drop, `__cancel` on a dropped `PortFuture` of a callback call, fire-and-forget calls with id
   0 through `Host::port_call` (a `Runtime::port_notify` entry beside `port_call`), all holding a `WeakCtx`.
4. `crates/undra-bindgen`: protocols/interfaces (decision "Generated shapes"), weak wrappers, the callback port
   bridge registration list in `UndraIds`, parameters of callback type at call sites (`core.callbacks.lend(..)`
   returning the handle, and giving it back on a status-5 refusal). Golden case `callbacks` (main and
   background traits, a fire-and-forget, a coalesced and an async method, a callback parameter on a method, a
   constructor and a free function).
5. Runtimes: Swift `UndraCore.callbacks` (registry under `Guarded`, interning by `ObjectIdentifier`), the mirror
   entry kind for `main` delivery, a serial executor for `background`, `Task` bookkeeping per `port_call_id` for
   `__cancel`; Kotlin likewise (`IdentityHashMap` under a lock, `Job`s, the C4b `onError`); TypeScript
   (`Map`/`WeakMap` registry, `AbortController` per call, microtask delivery for `background`).
6. Contract scenario **S22 "host callbacks"** (provisional number): a listener receives fire-and-forget calls in order and only after
   the change-sets committed before them; an async callback returns a value, throws its typed `E`, and is
   cancelled when the core drops its future (the host task observes cancellation); the same listener passed
   twice is one handle; the host registry is empty after the core drops the last proxy; a `coalesce` method
   delivers only the newest value per drain; a refused call (stale receiver) leaves the registry unchanged.
7. Bench: `boundary/callback/notify` (core half of a fire-and-forget call; budget: within 1.5× the
   `boundary/port_call` row) and `callback/async_roundtrip` per runtime.
8. Docs: SPEC 1, 2, 3.6, 4, 5.7, 10, 11, 12, 17; the cookbook's "listeners and delegates" page (main vs
   background, cycles, the subscription-object pattern).

## Dependencies

ADR-034 (`WeakCtx` for proxies) should land first; ADR-040's subscription-object pattern is the recommended
way to unregister, so 040 before 041 is the natural order (not a hard dependency). ADR-031's mirror queue gains
an entry kind. The v1.2 port-cancellation ADR (Amendment C item 6) should reuse decision 7's mechanism or
explain why not.

## Implementation notes (2026-10-02, `wt/objects-callbacks`)

Landed items 1 to 7 with ADR-040 in the same piece. No C ABI, wasm ABI or envelope change. What the code decided where the
text left room, and the deviations:

* **Scenario number.** The provisional S22 is **S28** (host callbacks, every column); S27 is ADR-040's.
* **Schema fields, both written only when true** (decision 3): `PortDef.background` (the trait is
  `#[undra::callback(background)]`; the platforms read it to pick the executor) and `MethodDef.coalesce` (a method marked
  `#[undra(coalesce)]`; the platforms' drains read it). No existing hash moves. `Schema::validate` knows where a
  `Callback` may stand (a parameter, alone or in an `Option`: E0004 elsewhere) and what a callback port's methods may be
  (E0071).
* **E0071 is a little wider than decision 1**: besides a synchronous method that returns a value and an `async` method
  without a `Result`, it rejects a method whose name starts with `__` (reserved for `__release` and `__cancel`) and
  `#[undra(coalesce)]` on an `async` method (a coalesced call that is dropped would leave its caller waiting).
* **Reserved ids** are `ids::callback_release_id(trait)` and `callback_cancel_id(trait)`, `fnv1a32("<Trait>.__release")` and
  `fnv1a32("<Trait>.__cancel")`; the platform id tables carry them as `releaseInstance` / `cancelCall` (Kotlin
  `RELEASE_INSTANCE` / `CANCEL_CALL`).
* **The core's half** is `undra_runtime::callbacks`: `CallbackInterface` (what `#[undra::callback]` implements for
  `dyn Trait`: how a dispatcher makes a proxy from an instance handle), `CallbackHandle` (the port, the instance and a
  `WeakCtx`; its drop sends `__release`), `CallbackCall` (the future of an async method; its drop sends `__cancel` and
  abandons the id) and `Runtime::callback` (interning through a map of `Weak` proxies). Fire-and-forget methods go
  through `Runtime::port_notify` (`port_call_id 0`, no reply read, no id allocated). A dispatcher makes the proxies
  last, after every argument has decoded and every object parameter has resolved (everything that could refuse the call
  has run); a refused call (status 5) therefore owns nothing, which is what lets the generated host code give its references back.
* **Statistics.** `stats_json` reports `live_callbacks` (the proxies the core holds); the platforms report the registry's
  live count and `MirrorStats.callbacksDelivered`.
* **Testing in Rust**: a trait with `#[undra::callback]` is also a normal trait object, so a Rust test implements it with
  `#[undra::port] impl Reporter for Recorder` (the playground's `workshop.rs` tests do) and passes its own `Arc`; the
  `TestRuntime` records the callback port calls a proxy makes.
* **Names**: the playground's example is `Reporter` / `Workshop::run`, `burst`, `watch`, `announce` (a `Watch` subscription
  object, decision 9), not `UploadListener`; the SPEC and the generated-shape section above keep the ADR's names.
* **Bench rows** (item 6): `boundary/port_call/notify` 24.3 ns, `boundary/callback/notify` 43.2 ns and
  `boundary/callback/async_roundtrip` 182.7 ns, each with a budget in `bench/budgets.toml` (`bench/RESULTS.md`, finding 6).
* **Docs**: `site/docs/callbacks.html` (a guide, next to Ports), SPEC 5.7, 6, 10.3a, 11 and 17, the playground's
  `workshop.rs` for the example that runs.
* **Platform limits.** TypeScript: after a crash-recovery restart the callback registry's entries are not dropped (no restart
  hook yet). Swift: with typed throws (the default) a weak wrapper's `async` method called directly after its target is gone
  cannot return (there is no `E` value to throw); the core never calls it in that state, and with `swift_typed_throws =
  false` it throws `.unavailable(.closed)`. A callback over `undra dev` has unit tests but no end-to-end WebSocket test.

