# ADR-040: objects cross as parameters and returns; every handle the core hands out is one owned reference

Status: **Accepted** (2026-10-02, implemented in `wt/objects-callbacks`; see "Implementation notes" at the end for what
the code decided where this text left room, and the deviations). Proposed 2026-10-01 (`wt/boundary-adrs`; Amendment B
"boundary surface", catalogue M-3, gap audit TY-4). Touches SPEC 1.2 (handle layout), 2.1 (`TypeRef::Object`), 3.1, 4.1, 5.4 (the object table
counts host references), 5.9 (derived handles are transient; the snapshot floor widens), 10.1–10.3, 12
(E0064's text), 16.2/16.3; `undra-meta`, `undra-macros`, `undra-runtime`, `undra-transport`, `undra-bindgen`
and the three platform runtimes. **Schema: one new `TypeRef` variant (no existing hash moves). Wire: the
handle encoding of SPEC 3.1 is used in two more positions; the `Snapshot` floor widens to `u64` (decision 8,
in the pre-publication wire revision of Amendment C). No C ABI or wasm ABI change.** Constitution R1, R5
(nothing here makes a read cross), R6, R7 and R11.

## Context

An object (an `#[undra::api] impl`, SPEC 1) crosses only one way today: a host-called constructor returns its
handle. Everything else is refused:

* The macro maps by spelling, so `-> Mailbox` becomes `Named("Mailbox")` and a const assertion fails with
  **E0064** "`Mailbox` is an object and cannot be used as a value … return a record with the data the platform
  needs, or construct the object from the platform" (`crates/undra-macros/src/impl_/check.rs:352-397`).
  `Arc<T>` is E0001 "shared ownership does not survive a copy across the boundary" and `&T` E0001 "references
  have no wire representation" (`crates/undra-macros/src/impl_/types.rs:713-718`, `:402-429`).
* `undra-bindgen` rejects a `Named` object in every value position, "object handles cannot cross as values
  yet", except a constructor's own return (`crates/undra-bindgen/src/validate.rs:1032-1038`, `:842-864`); it
  also never type-checks free-function parameters (`:367-371`), a gap the macro's assertion happens to cover.
* The runtime has the pieces but no ownership model for a handle the *core* decides to issue: a handle is one
  slot entry, `release` removes it at once (`crates/undra-runtime/src/object_table.rs:492-511`), there is no
  count of host references and no map from an object to its handle (`:179-195`). Constructors insert and
  encode the handle (`crates/undra-macros/src/impl_/object.rs:640-687`; `crates/undra-runtime/src/runtime.rs:1523-1533`).
* No platform runtime maps a handle back to its wrapper. Swift and Kotlin let a second mirror registration for
  a handle replace the first, TypeScript throws ("two stores cannot mirror one handle",
  `runtimes/ts/@undra/runtime/src/mirror.ts:313-324`), and every `close()` unregisters and releases by handle.

The catalogue (`.10x/specs/2026-10-01-competitive-limitations.md`) calls this "the first wall a UniFFI migrant
hits (E0064, E0004)" (UNI-B1, quoting UniFFI: "Objects can be freely passed as arguments and returned as
values"), and matrix row 23 has Undra at "part." beside KMP, UniFFI and flutter_rust_bridge at "yes". The gap
audit (TY-4) names the shapes a domain model needs: factory methods (`account.mailbox(id) -> Mailbox`), child
objects, `Option<Child>`.

## Decision

1. **Spelling.** The object positions are written the way Rust already shares objects (SPEC 4.1: objects are
   `Arc<Type>`):
   * **returns**: `Arc<T>`, `Option<Arc<T>>`, `Vec<Arc<T>>`, and each of those as the `Ok` of a `Result`
     (sync or async methods, free functions; constructors may return `Arc<Self>`, see 6);
   * **parameters**: `&T`, `Arc<T>`, `Option<&T>`, `Option<Arc<T>>`, `Vec<Arc<T>>` (methods, constructors, free
     functions).
   The mapper turns `Arc<X>` and `&X` (a single-segment-or-path type that is not a built-in) into
   `KType::Object("X")`, and the identity check asserts `<X>::__UNDRA_IS_OBJECT` and the type id. `Arc<Todo>` or
   `&Todo` for a record fails that check with "`Todo` is a record; records cross by value: write `Todo`".
   `-> Mailbox` (an object by value) stays **E0064**, whose help now reads "return `Arc<Mailbox>`".
2. **Schema.** `TypeRef::Object(String)` (`{"kind":"object","of":"Mailbox"}`), naming an `ObjectDef`
   (stores included). `TypeRef::Named` no longer names objects; `undra-meta` validation rejects a `Named`
   object and an `Object` that is not one. No existing schema contains either, so no hash moves.
3. **Wire.** A handle is the `u64` of SPEC 3.1 ("Handle (object)"). The null handle is never encoded: absence is
   `Option`; a decoder reading `0` where an `Object` is expected fails (`BadRequest` in the core, `malformed` on
   a platform).
4. **Ownership: one crossing, one reference.**
   * **Core → host.** Every handle in a successful reply body is **one owned host reference**. The host gives
     each one back exactly once: by `close()`/finalizer of the wrapper that owns it, or immediately when it
     already holds a live wrapper for that handle (decision 7). A typed error (`E`) cannot contain an object
     (error variants hold values only), and statuses 2, 3, 4 and 5 transfer nothing.
   * **Host → core.** An object parameter is **borrowed**: the host keeps its reference. The dispatcher resolves
     each handle to an `Arc` **before** the method runs (before its first `await`); a stale or wrongly typed
     handle answers `BadRequest` naming the parameter and `Type.method`. The call holds its `Arc`s until it
     finishes, so a host `close()` racing an in-flight call never frees what the call uses. Generated code keeps
     the wrapper alive until the call is sent (`withExtendedLifetime` in Swift, `Reference.reachabilityFence` in
     Kotlin; in TypeScript the finalizer cannot run inside the synchronous send).
5. **The object table counts host references and interns handles.** `Entry` gains `host_refs: u32`; the table
   gains a map from the object's address (`Arc::as_ptr` as `usize`, valid because the entry holds the `Arc`) to
   its handle. `insert` (constructors, today) is `host_refs = 1`. `issue(arc)` (returns) finds the object's live
   handle and increments `host_refs`, or inserts it. `release(handle)` decrements and removes the entry at
   zero; with one reference, which is every flow that exists today, that is exactly today's behaviour. **An
   object therefore has at most one live handle**: returning the same `Arc` twice gives the same handle while
   the host holds it, and a fresh handle (fresh generation, ADR-022) after the host has let go. At removal a
   store's cell is unobserved (`observe(ALL_SIGNALS, false)`) and its handle set to `0`, so a later re-issue
   starts clean. `host_refs` saturates at `u32::MAX` with an ERROR log (never wraps).
6. **Issued handles are tied to the call that issues them.** Lowering `Arc<T>` to a handle happens in the
   dispatcher (a `Lower` step with the runtime in scope, before `Encode`), and each issue is recorded in the
   current call's **ledger** (a per-call list held by the runtime's call scope: the dispatch thread for sync
   calls, the task for async ones). A call that is answered with status 0 commits its ledger. A call that ends
   any other way — cancelled (status 3, including restore and shutdown), a panic after some handles were issued
   (status 2), a reply that could not be built — **rolls the ledger back** (one `release` per entry), so no
   reference is ever owned by nobody. Constructors may return `Arc<Self>` (the singleton pattern: an existing
   instance is interned) besides `Self`/`Result<Self, E>`.
7. **Platforms keep one wrapper per handle.** Each runtime gains an identity map (handle → weak wrapper) inside
   `UndraCore`: `adopt(handle, make)` returns the live wrapper and releases the extra reference at once, or
   makes, registers and returns a new one. Consequences that generated code relies on: `a.mailbox("inbox") ===
   a.mailbox("inbox")` while the first is alive; a store returned twice is mirrored once; closing a wrapper
   releases exactly the one reference it owns. Constructors go through `adopt` too. Swift's `UndraObject`
   becomes `Hashable` by identity (one wrapper per handle makes that equal to handle equality), so
   `ForEach(account.allMailboxes(), id: \.self)` works.
8. **Derived handles are transient, and generations widen.**
   * A handle first issued by a **return** (not by a host-called constructor) marks its entry transient, like
     query handles today (`crates/undra-query/src/handle.rs:284-324`, `transient() = true`): a snapshot leaves it
     out and it is stale after a restore. The host gets it again by calling the method again. Restore does not
     rebuild object graphs: an `Arc` held in a store's private field is whatever that store's `restore` hook
     makes it (SPEC 16.3), which the docs now say in so many words.
   * Returning objects freely raises handle churn, and ADR-022's counter "at 1,000/s [is] 50 days". The handle
     keeps its `u64` size but is repartitioned to **24 bits of slot (16.7 million live objects) and 40 bits of
     generation** (1.1 × 10^12 issues: 3.5 years at 10,000/s), and the `Snapshot` floor becomes `u64`. Hosts
     never interpret handles (wasm passes them as two `i32`s, JNI as `long`), so the only wire-visible change is
     the snapshot's floor word, which ships in the pre-publication wire revision with ADR-036/037.
9. **What stays forbidden, and why.**
   * **Record and variant fields, store signals, map keys and values** (E0064): a value is copied, compared,
     hashed, `Codable` and coalesced (ADR-031 drops superseded change-set values), and none of that can carry
     an owned reference. A child object belongs behind a method.
   * **Stream items** (E0064): credited items buffered on the host and dropped at cancellation would leak their
     references; revisit with per-item ledgers when a use case appears.
   * **Port and callback parameters and returns** (ADR-041 for callbacks): a port implementation would receive
     a reference it has no wrapper type for. Pass a record or an id.
   * **Query and mutation parameters and results** (E0064): cache values must be values (they are encoded,
     compared and persisted).
   * **Objects of another core** (ADR-044): a handle names an entry in one runtime only; generated types keep
     cores apart and a foreign handle fails as stale.

### Generated shapes

```rust
#[undra::api]
impl Account {
    pub fn new(ctx: Ctx, id: AccountId) -> Self { .. }
    /// The mailbox for `folder`, created on first use.
    pub fn mailbox(&self, folder: String) -> Arc<Mailbox> { .. }
    pub async fn open_thread(&self, id: ThreadId) -> Result<Arc<Thread>, MailError> { .. }
    pub fn move_to(&self, message: MessageId, target: &Mailbox) { .. }
    pub fn drafts(&self) -> Option<Arc<Mailbox>> { .. }
    pub fn mailboxes(&self) -> Vec<Arc<Mailbox>> { .. }
    pub fn chat(&self, peer: UserId) -> Arc<ChatStore> { .. }   // a store
}
```

```swift
public final class Account: UndraObject, @unchecked Sendable {
    /// The mailbox for `folder`, created on first use.
    /// - Throws: ``UndraCallError`` if the call fails in the core or cannot reach it.
    public func mailbox(folder: String) throws -> Mailbox
    /// - Throws: ``MailError``, `CancellationError` if the task is cancelled, or ``UndraCallError``.
    public func openThread(id: ThreadId) async throws -> Thread
    public func moveTo(message: MessageId, target: Mailbox)          // a command: reports, never throws (ADR-032)
    public func drafts() throws -> Mailbox?
    public func mailboxes() throws -> [Mailbox]
    @MainActor public func chat(peer: UserId) throws -> ChatStore    // stores are main-actor types
}
```

```kotlin
class Account private constructor(core: UndraCore, handle: Long) : UndraObject(core, handle) {
    fun mailbox(folder: String): Mailbox
    suspend fun openThread(id: ThreadId): Thread                     // throws MailError
    fun moveTo(message: MessageId, target: Mailbox)
    fun drafts(): Mailbox?
    fun mailboxes(): List<Mailbox>
    fun chat(peer: UserId): ChatStore
}
```

```ts
export class Account extends UndraObject {
  mailbox(folder: string): Promise<Mailbox>;
  openThread(id: ThreadId): Promise<Thread>;                          // rejects with MailError
  moveTo(message: MessageId, target: Mailbox): Promise<void>;
  drafts(): Promise<Mailbox | null>;
  mailboxes(): Promise<Mailbox[]>;
  chat(peer: UserId): Promise<ChatStore>;
}
```

A returned store observes its signals when its wrapper is made, exactly as a constructed one does.

## Alternatives considered

* **`Named` keeps naming objects; bindgen looks the kind up.** The SPEC already allows it, but a schema reader
  could not tell a handle from a value without resolving the name, validation could not refuse a `Named`
  object in a field by shape, and ownership — the thing that differs — would be invisible in the description.
* **A fresh handle per return (UniFFI's model: every crossing is a new reference and a new wrapper).** No
  interning map, but a store returned twice would have two handles while its cell can deliver to one, and two
  wrappers would mirror (or, in TypeScript, refuse to mirror) the same signals. Interning plus one wrapper per
  handle is what makes child stores work.
* **Accept `-> Mailbox` by value and wrap it in an `Arc` in the dispatcher.** The macro maps by spelling and
  cannot know `Mailbox` is an object at expansion time; type-level dispatch would need specialisation or a
  blanket impl that overlaps `Encode`. `Arc<Mailbox>` says "shared" in Rust, which is the truth.
* **Host-side reference counting only (the host dedups and never tells the core).** The core could then not
  free an object the host dropped while the core also holds it, and a race between a finalizer and a new reply
  of the same handle could release a reference the new wrapper needs. Counting in the core, one per crossing,
  is the only order-independent rule.
* **Snapshot derived objects too.** Would need the object graph (which store holds which `Arc`) in the
  snapshot; stores only have signals and a restore hook. Transient derived handles keep ADR-023's
  all-or-nothing restore honest.
* **Keep the 32/32 handle split.** ADR-022 calls widening "a wire break (major version)"; it is not, because
  hosts treat handles as opaque. Doing it now, in the one remaining wire revision, costs a snapshot word.

## Consequences

* Factory methods, child objects and child stores, optional and plural object results, and objects as
  arguments work on all three platforms with native shapes. Catalogue row 23 moves to "yes" with ADR-041.
* Every runtime gains an identity map and `adopt`; every generated constructor goes through it (no change in
  behaviour for a constructor, which always issues a new handle).
* A host that releases a handle twice can now drop a reference another wrapper owns. The runtimes' wrappers
  already release at most once (`close()` is guarded; the finalizer runs at most once); a C host must follow
  the rule (documented in `undra.h`).
* `stats()` reports `live_handles` as today plus `host_refs` (the sum), so a leak of references is visible.
* `undra dev` sessions release the references a disconnected client owned (decision 6's ledger records the
  origin: `Runtime::call` gains an origin id, and `undra-transport`'s `release_on_disconnect`
  (`crates/undra-transport/src/session.rs:297-339`) calls `Runtime::release_origin(session)` instead of
  releasing the handles it constructed).

## Risks

* **Reference leaks** if a platform decoder fails halfway through a `Vec<Arc<T>>` (each handle already owned).
  The decoders adopt each handle as it is read; a failure after some were adopted leaves wrappers whose
  finalizers release them. A malformed reply after a successful schema check is already "a bug in Undra"
  (`UndraCallError.malformed`).
* **Main-actor hops in Swift**: a nonisolated object method that returns a store is `@MainActor`; called from
  a background task it needs `await`. That is the honest cost of stores being main-actor types.
* **Generation churn** (decision 8) is bounded by the new split, but an app that returns a fresh object per list
  row per frame is still wrong; the cookbook says "records for data, objects for things with identity".

## Implementation brief

1. `crates/undra-meta`: `TypeRef::Object` + `TypeRefMeta::Object`, `Display` `object:<Name>`; validation (an
   `Object` names an object; a `Named` does not); unit and canonical-JSON tests (a schema without objects in
   signatures hashes as before).
2. `crates/undra-macros`: `types.rs` maps `Arc<X>`, `&X`, `Option<..>`/`Vec<..>` of them to `KType::Object` in
   the positions of decision 1 (`Pos::Param`, `Pos::Return`), keeps E0064/E0001 elsewhere with the decision 9
   reasons; `check.rs` asserts `__UNDRA_IS_OBJECT` (and the record-in-`Arc` message); `object.rs` and the
   free-function expansion resolve object parameters with `rt.object::<X>(h)` before the body (`BadRequest`
   text "argument `target` of `Account.move_to`: <BadHandle>") and lower returns through `rt.issue(..)`;
   constructors returning `Arc<Self>`. UI tests for every message; behaviour tests against the real runtime.
3. `crates/undra-runtime`: `object_table.rs` (`host_refs`, the address map, `issue`, decrementing `release`,
   `transient` for issued handles, the 24/40 split, `u64` floor), `runtime.rs` (`issue`, the call ledger in the
   call scope and its commit/rollback in `finish_call`, cancellation, restore, shutdown and the panic guard;
   `release_origin`; `stats_json` `host_refs`), `undra-wire` snapshot codec (`u64` floor). Property test:
   random sequences of issue/return/release/cancel/restore never leave `host_refs` different from the number of
   references the model host owns.
4. `crates/undra-transport`: origin ids per session, `release_origin` on disconnect.
5. `crates/undra-bindgen`: `Object` in `swift.rs` (`@MainActor` on methods returning stores; `adopt` in
   decoding), `kotlin.rs`, `ts.rs`; `validate.rs` accepts decision 1's positions, rejects decision 9's with the
   reason, and type-checks free-function parameters. Golden case `object-graph` (the `Account` example plus a
   child store and a free function returning an object).
6. Runtimes: Swift (`UndraCore.adopt`, weak identity map under `Guarded`, `UndraObject: Hashable`), Kotlin
   (`UndraCore.adopt`, a `WeakReference` map cleaned by the existing `HandleCleaner`), TS (`UndraCore.adopt`,
   `Map<bigint, WeakRef>` cleaned by the `FinalizationRegistry`; `mirror.register` keeps throwing on a second
   registration, now unreachable from generated code).
7. Contract scenario **S21 "objects cross"** (provisional number) (all three columns and the RN column of ADR-038): a parent returns
   a child; the same child twice is the same wrapper and one core reference (stats); a child is passed back as
   a parameter; closing the child makes it stale only when no reference remains; a child **store** delivers
   change-sets; a call cancelled after its body issued a handle leaves `host_refs` unchanged; after a restore the
   child is stale and the method returns a fresh one.
8. Bench: `dispatch/call_sync/return_object` (first issue) and `return_interned_object` (budget: within 2× the
   `add` row on the core half), host-side `adopt` in each runtime's bench; `bench/RESULTS.md` rows.
9. Docs: SPEC 1.2, 2.1, 3.1, 4.1, 5.4, 5.9, 10, 12 (E0064 text), 16; `undra.h` (the C host's ownership rule);
   cookbook "objects, children and identity".

## Dependencies

Independent of ADR-041…046 for its code, but ADR-041 reuses decision 6's ledger idea in the other direction, and
decision 8's snapshot word must ride the ADR-036/037 wire revision (if that revision ships first, its snapshot
codec should already carry a `u64` floor). Benefits from ADR-034 (`WeakCtx`) for objects that hold a context.

## Implementation notes (2026-10-02, `wt/objects-callbacks`)

Landed items 1 to 9 with ADR-041 in the same piece. No C ABI or wasm ABI change; the wire change is the one decision 8
named (the handle's partition and the snapshot's `u64` floor). What the code decided where the text left room, and the
deviations:

* **Scenario number.** The provisional S21 is **S27** (objects cross, every column): S21 and S22 were taken by ADR-049's
  web-only scenarios, S23 to S25 by ADR-047/048's ports. `contract-tests/scenarios.md` holds it.
* **Constructors keep `Named`** (decision 2). A constructor returning `Arc<Self>` is recorded as `Named(Self)`, like every
  constructor before it, so no existing hash moves and the platforms' constructor path is unchanged. `Schema::validate`
  lets a `Named` stand for an object in exactly that position (the return of the object's own constructors) and nowhere
  else; every other object position is `TypeRef::Object`.
* **The ledger is a scope, not a table** (decision 6). `IssueScope` is an RAII guard held where the call is (inside the
  dispatcher for a synchronous method; around the last poll for an asynchronous one, whose objects are lowered inside
  the poll that completes it, through a `WeakCtx`, so a call that is dropped before then issued nothing). `commit`
  says the reply carries the references; a scope dropped any other way gives them all back. The behaviours the ADR
  lists (cancelled, panic, a reply that could not be built) are the three ways a scope is dropped uncommitted.
* **Origins** (brief item 4). `Runtime::call_from(origin, ..)`, `release_from` and
  `release_origin` record what a remote session's calls committed; `undra-transport` names a session's origin (a hash of
  its resume token, so a resumed connection keeps it; the connection id for a session without one) and releases it when
  the session ends for good: a disconnect with no token to resume by, or the expiry or replacement of a retained
  session. In-process hosts never use them.
* **`Handle::new` masks.** `Handle::new(index: u32, generation: u64)` keeps the low 24 bits of the index and the low 40 of
  the generation; the object table never issues a larger value, and a hand-made handle cannot spill into its neighbour.
  The table's counter panics (contained at the boundary) at `2^40 - 1` issues, never wraps.
* **Foreign handles are refused on the host** (decision 9, last bullet). "A foreign handle fails as stale" was a gap: two
  cores hand out the same numbers, so a handle of core A is a *valid* handle of core B naming another object. Generated
  code now calls `core.requireOwn(object)` for every object argument; it throws `UndraCallError.refused(reason)` (reason
  names the class and says it belongs to another core) before anything is sent, and a command reports it through
  `onError` (ADR-032). The core's own checks stay the second line for a raw-API caller.
* **An object nobody constructs** (decision 1 and the generated shapes): a plain object (no `#[undra::api(store)]`) with
  no constructor that some method returns is legal; the generated class has no public initialiser. The brief's
  validation rule "an object needs a constructor" is relaxed accordingly.
* **Names**: the playground's example is `Workshop` / `Shelf` / `Watch` (`examples/playground/core/src/workshop.rs`), not
  `Account` / `Mailbox`; the SPEC's examples keep the ADR's names.
* **Bench rows** (item 8): `dispatch/call_sync/return_object` 84.0 ns, `return_interned_object` 84.0 ns (1.7x the `add`
  row of the same run, inside the ADR's 2x), `object_param` 48.7 ns (1.0x), with a ratio gate; budgets and ratio in
  `bench/budgets.toml`, text in `bench/RESULTS.md` finding 6. The platform side (`adopt`) is measured in each runtime's
  own suite and recorded in `.10x/decisions/sde/objects-callbacks.md`.
* **Docs** (item 9): `site/docs/objects.html` (a guide, next to Ports) instead of a cookbook recipe: the cookbook's
  recipes are checked against code the site builds, and this one is the playground's own `Workshop`. SPEC 1.2, 2.1,
  3.1, 4.1, 5.4, 5.9, 10.3a, 11, 12, 16 and 17 carry the rules.

