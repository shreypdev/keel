# Derived keyed lists: implementation brief (ADR-039)

**Date:** 2026-10-01 · **Status:** ready once ADR-039 is accepted **and Track A has merged** (ADR-034
`WeakCtx`, ADR-035 off-runtime writes, the A3 amendment to ADR-019: they rewrite `write_with`,
`StoreCell::commit_slots` and the sink, which this piece also touches). Open decisions 1-5 of the ADR are
assumed at their recommended values; if the integrator picks otherwise, sections 2.3, 2.4 and 3 change. ·
**Roles:** implementer opus (`sde`), reviewer opus (adversarial, `docs/AGENT_WORKFLOW.md` section 3).

Read first, in this order: `CLAUDE.md` (R1, R3, R4, R5, R6, R9, R11, R12); `.10x/adrs/ADR-039-derived-keyed-lists.md`
(the decision; this brief does not repeat its reasons); ADR-027 (the op log you generalise), ADR-019 and
ADR-020 (claims, abandon, the delivery lock), ADR-021 (contained misuse), ADR-031 (platform merging: your
patches are ordinary SPEC 3.8 patches); `docs/SPEC.md` 2.2, 3.8, 4.3, 5.5, 5.9, 16.1, 16.3; then the code:
`crates/undra-signals/src/{signal.rs, signal/list.rs, oplog.rs, store.rs, computed.rs, deps.rs, graph.rs,
txn.rs, context.rs}`, `crates/undra-macros/src/impl_/store.rs`, `examples/playground/core/src/todos.rs`,
`bench/common/{stress.rs, fixtures.rs, host.rs, workloads.rs}`, `bench/budgets.toml`. The throwaway prototype
the ADR measured (arena AVL trees, filter / sort / ties, checked against a `Vec` model) is described in
section 2.1; rebuild it properly, do not copy it.

## 1. Scope

In scope:

* `crates/undra-signals`: the new `derived` module (index, taps, pipeline, node), the tap hooks in
  `signal.rs` / `signal/list.rs` / `oplog.rs`, the derived slot in `store.rs`, a crate-private
  `Computed::from_compute`, `OwnedDep::snapshot`, the "deriving" thread-local, exports, README, tests.
* `crates/undra` (the prelude exports `DerivedList`).
* `crates/undra-macros` (`DerivedList<T>` store fields, E0008 texts, expansion and diagnostics tests).
* `crates/undra-meta` (tests only: the pins), `crates/undra-bindgen` (the `stores` golden case gains a derived
  signal; regenerated goldens; no generator code change is expected; if one is needed, stop and ask: it would
  contradict ADR-039 decision 7).
* `examples/playground/core/src/todos.rs` and `examples/playground/generated/` (regenerated, never edited).
* `contract-tests/` (S19, the three runners, `check.sh`'s count).
* `bench/` (fixtures, workloads, the stress scenario, `budgets.toml`, `RESULTS.md`).
* Docs: `docs/SPEC.md`, `site/docs/concepts.html`, `crates/undra-signals/README.md`, `docs/HIGH_FREQUENCY.md`.

Out of scope (do not touch): `undra-wire`, `undra-ffi`, `undra-runtime`, `undra-query`, the platform runtimes
(`runtimes/`), the bindgen generators' code, lazy lists, `.10x/status.md` and `.10x/handoff.md`. The wire,
the ABI and the schema hash of every existing core stay byte-identical.

## 2. `undra-signals`

New module tree `src/derived/` (`mod.rs` public API and docs, `index.rs`, `tap.rs`, `pipeline.rs`,
`node.rs`, `slot.rs`). Constants in `derived/mod.rs`, documented:

```rust
/// Pending source operations a derived list keeps before it stops recording and rebuilds at its next
/// read (ADR-039 section 4). Fixed: it does not grow with the list.
pub(crate) const TAP_LIMIT: usize = 4096;
/// Derived operations an observed list keeps for its next commit before it sends the full value instead.
pub(crate) const OUT_LIMIT: usize = 4096;
/// Most operations a parameter change is sent as; beyond, the full value (measured break-even, ADR-039).
pub(crate) const PARAM_WALK_LIMIT: usize = 256;
```

### 2.1 The index (`derived/index.rs`)

One generic arena AVL tree with parent pointers and a summable aggregate, instantiated twice.

```rust
pub(crate) const NIL: u32 = u32::MAX;
pub(crate) trait Aggregate: Copy + Default { fn add(self, other: Self) -> Self; }
pub(crate) struct OsTree<A: Aggregate> { nodes: Vec<OsNode<A>>, free: Vec<u32>, root: u32, len: usize }
struct OsNode<A> { l: u32, r: u32, p: u32, h: u8, own: A, sum: A }   // `own`: this node's contribution
```

Operations (all O(log n) unless stated; node ids are stable for the life of the node):

| Fn | Contract |
|---|---|
| `alloc(own) -> u32` / `free(id)` | arena slot; a freed id is reused by the next `alloc` |
| `attach_at(id, index)` | link a detached node so it ends at in-order position `index` (`<= len`); fix up to the root |
| `attach_by(id, less: impl FnMut(u32) -> bool)` | link by descending: `less(existing)` true means "the new node goes before `existing`"; equal goes right |
| `detach(id)` | unlink (successor relinking when it has two children, as the prototype did), fix up from the lowest touched node; the node keeps its id and `own` |
| `select(index) -> u32` | the node at in-order position `index` (`< len`) |
| `before(id) -> A` | sum of `own` over every node before `id` in order (walk up the parents) |
| `rank(id) -> usize` | `before(id)`'s size component, for trees whose aggregate counts nodes |
| `set_own(id, own)` | change a node's contribution and re-sum its ancestors |
| `sum() -> A` / `len()` | the root's sum (O(1)) / the node count |
| `build(ids_in_order: &[u32])` | O(n) perfectly balanced build from already allocated nodes (used by rebuilds) |
| `for_each_in_order(f)` | O(n), iterative (no recursion: 100,000 rows would be fine, but a degenerate tree must not overflow the stack) |
| `clear()` | O(n) to drop, keeps capacity |
| `#[cfg(test)] validate()` | heights, balance, parent links, sums: called by the tests after every operation |

The two trees, as `DerivedIndex<K>`:

* **positional**: aggregate `Pos { size: u32, pass: u32 }`; one node per source row in source order; the
  row's **id is its positional node id** (everything else is keyed by it). Per row, in parallel `Vec`s indexed by
  that id: `sorted: u32` (its node in the sorted tree, `NIL` when it does not pass or the view is unsorted).
* **sorted** (only when the pipeline has a sort stage): aggregate `Size(u32)`; one node per passing row; per
  sorted node, in parallel `Vec`s: `row: u32` (back to the positional id) and `key: Option<K>` (`Some` while
  the node is live). Comparison for insertion of row `x` with key `kx` at source position `px` (computed once)
  against existing node `y`: `kx.cmp(key[y])`, and on `Equal` `px < pos.rank(row[y])`.

Rank in the view: unsorted `pos.before(row).pass`; sorted `sorted.rank(sorted[row])`.

Tests (`index.rs`, unit + `proptest`): random sequences of `attach_at` / `detach` / reattach (a move) /
`set_own` / `free` against a `Vec<(id, own)>` model: in-order ids, `select`, `before`, `rank` and `len` agree
after every step, `validate()` holds; `attach_by` with a comparator builds the same order as a stable sort of
the model; `build` gives height `<= ceil(log2(n + 1))`; ids are reused. Heavy-tie case: 10,000 nodes with 4
keys, compare with `slice::sort_by_key` (stable).

### 2.2 Source taps (`derived/tap.rs`, `signal.rs`, `signal/list.rs`, `oplog.rs`)

* `SignalInner` gains `pub(crate) taps: OnceLock<Arc<dyn ListLog>>` (beside `log`, `signal.rs:98-100`),
  holding a `TapList<I>` created by the first `derive()` that builds a node. `TapList<I>` is
  `RwLock<Vec<Weak<SourceTap<I>>>>`; its `ListLog::invalidate` marks every live tap stale; `as_any` downcasts.
  Registration takes the `TapList` write lock and never runs under the source's value lock; dead `Weak`s
  are pruned when the list is about to grow (as `graph::add_dependent` does).
* `SourceTap<I>`: `Mutex<TapState<I> { armed: bool, stale: bool, ops: Vec<PatchOp<I>>, spare: Vec<PatchOp<I>> }>`,
  a leaf lock. `is_recording()`, `push(op)` (dropped when not recording; at `TAP_LIMIT` it goes stale and frees
  `ops`), `invalidate()`, `disarm()`, and

  ```rust
  /// Under the source's value READ lock: the ops recorded since the last take, oldest first, and the list
  /// they lead to; re-arms empty. `Fresh` when the tap was disarmed or stale (the caller rebuilds).
  pub(crate) fn take(&self, current: &Arc<Vec<I>>) -> TapTaken<I>;   // Ops { ops, current } | Fresh { current }
  pub(crate) fn take_ops(&self) -> Option<Vec<PatchOp<I>>>;           // without a snapshot; None = stale/disarmed
  pub(crate) fn recycle(&self, ops: Vec<PatchOp<I>>);
  ```

  A tap starts **disarmed**: a new derived list records nothing until its first drain arms it together with a
  snapshot (so building a view costs nothing until it is read or observed).
* **Recording** (`signal/list.rs`). Keep the slot log path exactly as it is (its op is still built before the
  mutation for `push`/`insert`, and recorded after it). Taps get their ops **after** the mutation, cloned from
  the list itself, under a guard that marks every tap stale if a clone panics:

  ```rust
  // inside write_with, after the list was changed and the slot log's op recorded, value still write-locked:
  if let Some(taps) = self.tap_list() {           // None: no derived list was ever built on this signal
      let guard = taps.stale_on_unwind();
      taps.for_each_recording(|tap| tap.push(make_op()));   // make_op clones list[index] for Insert/Update
      guard.disarm();
  }
  ```

  No allocation per op beyond the item clones; with no taps the cost is the `OnceLock::get` that returns
  `None`. One behaviour changes, and only for a list that has a derived view: a `Clone` that panics while a
  tap's op is built leaves the item inserted or updated (the list, the slot log and the host agree; the taps go
  stale and their views rebuild), where today `push`/`insert` leave the list untouched. Say so in those
  methods' `# Panics` sections. (Pre-cloning for the taps before the mutation keeps the old guarantee but needs
  storage for k clones per op; choose it only if review asks, with an inline buffer for two taps.) `invalidate_log` (`signal.rs:223-227`) also invalidates the tap list, so every raw write marks every
  tap stale before it touches the list.
* Index overflow: a source index that does not fit `u32` makes the taps stale (as `wire_index` does for the log).

Tests: `signal.rs` / `list.rs` unit tests mirroring `a_raw_write_invalidates_the_op_log_and_a_recorded_one_does_not`
for taps; a panicking `Clone` inside `push` / `insert` / `update_at` leaves every tap stale; two taps on one
source both see every op; a dropped derived list's tap is pruned.

### 2.3 The pipeline and the builder (`derived/pipeline.rs`, `derived/mod.rs`)

Public surface (every item documented with a one-line summary and an example on `derive`, `DerivedList`,
`filter_with`, `sort_by_key` and `count`):

```rust
impl<T: SignalValue> Signal<Vec<T>> {
    pub fn derive(&self) -> Derive<T>;
}
#[must_use = "a pipeline does nothing until `build()` or `count()`"]
pub struct Derive<T, U = T, O: Order = Unsorted> { .. }
pub struct Unsorted;
pub struct Sorted<K>(PhantomData<fn() -> K>);
pub trait Order: sealed::Sealed + 'static { #[doc(hidden)] type Key: Ord + Clone + Send + Sync + 'static; }
impl Order for Unsorted { type Key = (); }
impl<K: Ord + Clone + Send + Sync + 'static> Order for Sorted<K> { type Key = K; }

impl<T: SignalValue, U: SignalValue, O: Order> Derive<T, U, O> {
    pub fn filter(self, f: impl Fn(&U) -> bool + Send + Sync + 'static) -> Self;
    pub fn filter_with<D: Dep>(self, param: D, f: impl Fn(&D::Value, &U) -> bool + Send + Sync + 'static) -> Self;
    pub fn map<V: SignalValue>(self, f: impl Fn(&U) -> V + Send + Sync + 'static) -> Derive<T, V, O>;
    pub fn build(self) -> DerivedList<U>;
    pub fn count(self) -> Computed<u32>;
}
impl<T: SignalValue, U: SignalValue> Derive<T, U, Unsorted> {
    pub fn sort_by_key<K: Ord + Clone + Send + Sync + 'static>(self, f: impl Fn(&U) -> K + Send + Sync + 'static) -> Derive<T, U, Sorted<K>>;
    pub fn sort_by_key_with<D: Dep, K: Ord + Clone + Send + Sync + 'static>(self, param: D, f: impl Fn(&D::Value, &U) -> K + Send + Sync + 'static) -> Derive<T, U, Sorted<K>>;
}
```

Internals:

* The stages compose into one `Arc<dyn Fn(&Params, &T) -> Option<(O::Key, U)> + Send + Sync>` (`Eval<T, U, K>`).
  A filter before or after the sort is applied in the order written; a map after the sort transforms the
  value the key was taken from (the key is taken at the sort stage). `count()` drops the output and keeps
  only the passing test (a node with `U = ()`, unsorted, no `out`).
* `Params` is the per-drain snapshot of the parameter values: `Vec<Arc<dyn Any + Send + Sync>>`; each
  parameterised stage knows its slot and downcasts (`expect` is fine: the builder made both sides).
* `ParamSlot` (type-erased per parameter): `snapshot(&self) -> Arc<dyn Any + Send + Sync>`, `same(&self, a, b) -> bool`
  (`Arc::ptr_eq`, else equal encoded bytes: `P: Encode`), `subscribe(&self, Weak<dyn Reactive>)`. Built from
  `D: Dep` through `D::Owned`; add `#[doc(hidden)] fn snapshot(&self) -> Arc<Self::Value>` to `OwnedDep`
  (`deps.rs:42-46`): `Signal` returns its snapshot, `Computed` its `current()`, `DerivedList` its materialised value.
* Pipelines without a `map` may move the tap's owned item into the emitted op instead of cloning it a second
  time (an evaluation API that takes `Cow<T>`); an optimisation, measured by the allocation test (section 7).

### 2.4 The node (`derived/node.rs`)

```rust
pub struct DerivedList<T> { inner: Arc<dyn DerivedNode<T>> }        // Clone, Send + Sync, Debug (never drains from Debug)
pub struct DerivedStats { pub rebuilds: u64, pub full_values: u64, pub patches: u64, pub ops_emitted: u64 }

struct DerivedInner<S, U, K> {
    source: Signal<Vec<S>>,
    tap: Arc<SourceTap<S>>,
    eval: Eval<S, U, K>,
    params: Vec<Box<dyn ParamSlot>>,
    sorted: bool,
    state: Mutex<State<U, K>>,                    // the drain lock: held while pipeline closures run
    dirty: AtomicBool, visited: AtomicU64,        // Reactive bookkeeping, as ComputedInner
    dependents: Dependents, binding: OnceLock<Binding>,
    stats: [AtomicU64; 4],
}
struct State<U, K> {
    index: DerivedIndex<K>,
    params: Params,                               // the values the index was built with
    needs_rebuild: bool,                          // true at birth, after a stale tap, after an unwind mid-drain
    out: Out<U>,                                  // { armed, stale, ops: Vec<PatchOp<U>> }: kept only while observed
    cache: Option<Arc<Vec<U>>>,                   // materialised for Rust reads; dropped by any change
}
```

`Reactive for DerivedInner` (`graph.rs:22-28`): as `ComputedInner::invalidate` (`computed.rs:238-249`): visited
check, `dirty`, `record(binding)`, `propagate`. It subscribes to the source and to each parameter at build time
(only once fully built, as `Computed::new` does).

**The drain** (one routine; every read, commit and observe goes through it):

```text
drain(need_snapshot) -> guard over State:
  check re-entrancy: this node on the thread-local DRAINING stack -> panic "derived list cycle" (ADR-021 L1 wording)
  source.assert_not_updating("read")             // ADR-021 L3: inside the source's update/update_at closure
  lock state; push DRAINING; guard: on unwind set needs_rebuild = true, out.stale = true, pop DRAINING
  new_params = snapshot every ParamSlot (outside the deriving scope)
  if needs_rebuild:
      (current) = under source.read_locked: tap.take(current) (re-arms)            // ops discarded
      rebuild(current, new_params); out.stale = true (if armed); cache = None; stats.rebuilds++
      return
  if need_snapshot || params changed:
      (ops, current) = under source.read_locked: tap.take(current)
      on Fresh -> rebuild as above
  else:
      ops = tap.take_ops(); on None (stale) -> needs_rebuild = true; restart
  for op in ops: apply_source_op(op, &state.params)                                 // old parameter values
  if params changed: param_walk(current, new_params)                               // section below
  state.params = new_params
  tap.recycle(ops); if any op changed the view, or a rebuild or a walk ran: cache = None
  (invalidate() never touches State: it runs on the writer's thread and takes no lock but the graph's)
```

`apply_source_op` follows ADR-039 section 2's table; the prototype's `insert` / `remove` / `update` /
`move_item` are the reference (rank computed before a removal and after an insertion; a sorted row whose key
changed is detached from the sorted tree, re-keyed and reattached, and emits `Move` when the rank changed, then
`Update`). The shared **row transition** (used by `Update` and by the parameter walk):

```text
transition(row, was: Option<K>, now: Option<(K, U)>):          // None = filtered out
  (None, None)                 -> nothing
  (Some, None)                 -> r = rank(row); detach from sorted (if sorted); pos.set_own(pass 0); emit Remove{r}
  (None, Some((k, u)))         -> pos.set_own(pass 1); if sorted: key = k; attach_by; r = rank(row); emit Insert{r, u}
  (Some(old), Some((k, u)))    -> if !sorted || old == k: emit Update{rank(row), u}
                                  else: from = rank; detach; key = k; attach_by; to = rank; if from != to emit Move{from, to}; emit Update{to, u}
```

In the parameter walk the same routine runs with one difference: a row that passes before and after emits
no `Update` (no stage maps with a parameter in v1, so its value did not change); only its `Move`, when a sort
parameter moved it.

Every emitted op goes to `out` only while it is armed (observed); `out` beyond `OUT_LIMIT` goes stale
(drops its ops). Item evaluation runs inside a **deriving scope** (section 2.6). A `Move` of the source
detaches and reattaches the same positional node; a sorted row additionally detaches and reattaches its sorted
node (its position among equal keys may change).

**Parameter walk**: `for_each_in_order` over the positional tree zipped with `current` (they have the same
length and order after the ops were applied; `debug_assert` it), evaluate each row with `new_params`, call
`transition` for rows whose membership or key changed, count emitted ops; past `PARAM_WALK_LIMIT` set
`out.stale` (keep updating the index, emit nothing more). O(n) evaluations plus O(k log n).

**Rebuild** (`current`, params): evaluate every row in source order; allocate positional nodes, `build` them
balanced (O(n)); stable-sort the passing rows by key (`sort_by` on `(key)` keeps source order on ties because
the input is in source order) and `build` the sorted tree. O(n log n). Free everything first (`clear`).

**Materialise** (`get`, `with`, `Dep`, `observe`'s full value, `encode_signal`): `drain(need_snapshot = true)`,
then, if `cache` is `None`: unsorted: walk the positional tree with `current`, evaluate passing rows, collect
`U`; sorted: one pass assigns each positional id its position (`Vec<u32>`), then walk the sorted tree and
evaluate `current[pos[row]]`. A row the index says passes but whose evaluation now says `None` is an impure
closure: `debug_assert!` with the purity message; in release set `needs_rebuild` and redo. Store the result
in `cache` (an `Arc`), return it. Debug builds also check the keys of the materialised view are unique with
the store's key function when the list is attached (`debug_assert!` naming the duplicate's position).

**`count()`**: a node with `U = ()` and no `out`, wrapped in a `Computed<u32>` through a crate-private
`Computed::from_compute(Box<dyn Compute<u32>>)` (refactor `Computed::new` onto it, `computed.rs:132-153`):
`run()` drains (no snapshot needed) and returns `index.pos.sum().pass` saturated to `u32`; `subscribe` adds the
computed to the node's dependents.

**Public reads**: `len`/`is_empty` drain without a snapshot; `get`/`with` materialise; `stats` reads atomics.

### 2.5 The derived slot (`derived/slot.rs`, `store.rs`)

```rust
pub(crate) enum Emitted { Patch, Full, Nothing }
pub(crate) trait DerivedSlot: Send + Sync {
    fn commit(&self, w: &mut Writer, retain: bool) -> Emitted;   // retain = observed (false: no_coalesce while unobserved)
    fn resync(&self, w: &mut Writer);                             // full value; arm `out` empty from this instant
    fn forget(&self);                                             // disarm `out` (drop pending ops); the index stays
}
```

* `commit`: `retain == false` -> materialise, encode the full value, `Full` (no `out`). Otherwise drain; then
  `out` stale (or a rebuild happened) -> materialise, encode full, re-arm empty, `Full`; `out` empty -> `Nothing`;
  else encode `KeyedPatch { ops }` from `out`, recycle, `Patch`. Debug builds: after a `Patch`, materialise and
  compare against a shadow copy of "what the host has" kept only under `cfg(debug_assertions)` (the
  `same_encoding` check of `store.rs:1118-1129`), so every test checks the invariant, not only the end state.
* `StoreCell::attach_derived<T: SignalValue>(self: &Arc<Self>, list: &DerivedList<T>, signal_id: u32, key: fn(&T) -> u64)`
  installs `SlotKind::Derived(Box<dyn DerivedSlot>)` (`store.rs:54-58`) with an encoder that materialises.
  Errors as `attach` (a list attaches to one store for life: `AlreadyAttached`).
* `commit_slots` (`store.rs:649-744`): the derived branch pushes nothing on `Nothing`; when the builder holds
  no entry at the end, the change-set is not delivered (recycle the buffer, return; `last_txn` already advanced
  is harmless, ids stay strictly increasing). Re-check this against A3's per-slot isolation once Track A has
  landed: a derived slot that panics is isolated exactly like a computed slot.
* `encode_settled` (`store.rs:530-564`): `SlotKind::Derived(state) => state.resync(&mut value)`.
* `stop_observing`, `Abandon` (`store.rs:795-813`), `ObserveRollback`: call `forget()` for derived slots, as
  they do for keyed ones.
* `encode_snapshot` (`store.rs:586-604`): skip `Derived` as it skips `Computed`.

### 2.6 Purity and re-entrancy checks

* `context.rs` (or `derived/mod.rs`): `thread_local! DERIVING: Cell<u32>`; `pub(crate) struct DerivingScope`
  (increments, decrements on drop); `pub(crate) fn assert_not_deriving(what: &str)` compiled only with
  `debug_assertions`, message: "undra-signals: a derived list's closure {what} a signal. Filter, sort-key and
  map closures must be pure functions of their arguments: the list re-evaluates a row only when the row changes,
  so state read any other way leaves rows where an older answer put them. Pass the value as a parameter
  (`filter_with`, `sort_by_key_with`)."
* Call it in `Signal::snapshot` / `read_locked` (`signal.rs:199-219`), `write_with` (`signal.rs:250-272`),
  `ComputedInner::current` (`computed.rs:189-196`) and every public `DerivedList` read. The node's own reads of
  its source and parameters happen outside the scope.
* The DRAINING stack: `thread_local! DRAINING: RefCell<Vec<usize>>` keyed by the node's address, as
  `RECOMPUTING` (`computed.rs:72-110`).

### 2.7 Exports

`lib.rs`: `pub use derived::{Derive, DerivedList, DerivedStats, Order, Sorted, Unsorted};`.
`crates/undra/src/lib.rs:47`: add `DerivedList` to the prelude. `#![forbid(unsafe_code)]` stays; no new
dependency (the crate must still build for `wasm32-unknown-unknown`, iOS and Android: run
`cargo build -p undra-signals --target wasm32-unknown-unknown`).

## 3. `undra-macros` (`src/impl_/store.rs`)

* `SigKind::Derived`; `signal_wrapper` (`:118-126`) recognises the last segment `DerivedList` (one type
  argument `T`). The schema type and every type check use `Vec<T>` (`syn::parse_quote!(::std::vec::Vec<#t>)`
  for `map_type` and `checks.ty`).
* The key is **required**: without `#[undra(key = "..")]`, E0008 "`visible` is a `DerivedList<Todo>` without
  `#[undra(key = \"..\")]`" / why: "a derived list reaches the platforms as keyed patches; the key names the
  field that identifies a row" / help: "add `#[undra(key = \"id\")]` naming a field of `Todo`". The key
  function is generated as for a keyed `Signal` (`:344-361`, item type `T`) and passed to
  `attach_derived(&self.visible, id, __undra_key_visible)?`.
* `#[undra(key)]` on a `Computed<..>` keeps E0008 (`:214-230`) with a new help: "a computed list is sent whole;
  to send it as keyed patches, build it as a `DerivedList<T>` (`source.derive().filter(..).build()`) and key
  that".
* `SignalMeta { computed: true, key: Some(..), no_coalesce }` for a derived field; `no_coalesce` allowed.
* Restore: a derived field is not a plain signal (left out of the snapshot and of the hook's parameters) and
  counts as computed for E0013 (`:284-302`; the message lists it).
* Tests: expansion tests next to `keyed_lists_and_no_coalesce` (`:865-895`): the attach line, the key fn, the
  meta; diagnostics: missing key, key on a computed (new help text), `DerivedList` without a type argument,
  E0013 naming a derived field. Update `tests/snapshots.rs` / `tests/diagnostics.rs` the way their existing
  cases are written.

## 4. `undra-meta` and `undra-bindgen`

* `canonical.rs` tests: `derived_signals_add_no_field`: a store signal with `computed: true, key: Some("id")`
  serialises as `...,"computed":true,"key":"id"}` (exactly; no other key), and
  `hash_golden_for_representative_schema` (`:385-392`) is untouched by this piece (do not edit the golden).
* `crates/undra-bindgen/tests/common/mod.rs` `stores()` (`:808`): add a derived signal (computed, keyed `Vec`
  of a record) after the existing ones; `UPDATE_GOLDEN=1 cargo test -p undra-bindgen --test golden`; review the
  diff: the new property is declared exactly like a computed list, and its `apply` has the keyed-patch case.
  Add an assertion test in `tests/generators.rs` that, for each language, the derived signal's apply contains
  the patch call and its declaration equals the computed one's modulo the name. The `typecheck_ts` and
  `typecheck_kotlin` suites must pass on the new golden.

## 5. Playground (`examples/playground/core/src/todos.rs`)

* Exactly the rewrite in ADR-039 section 9: `visible: DerivedList<Todo>` (keyed `id`) built with
  `filter_with(&filter, ..)`, `remaining` from `.count()`, `add` with `push`, `toggle` with `update_at`,
  `remove` with `remove`, `clear_done` as descending `remove`s in one `txn`, a private `position(id)`.
* New public method `fill(&self, count: u32)`: appends `count` items titled `Item {n}`, every fourth one done,
  in one transaction (ids continue from `next`); documented as a demo and test helper (S19, the docs).
* Tests in `todos.rs` (they decode change-sets): `add` is a one-op `Insert` on `VISIBLE`; `toggle` under
  `All` is one `Update`; under `Active` one `Remove` and back one `Insert`; `set_filter` sends the membership
  patch; `fill(10_000)` then `toggle` sends a `VISIBLE` entry under 100 bytes and a `REMAINING` entry; restore
  rebuilds `visible` (observe after restore: full value equal to the model). Keep the existing value tests.
* Regenerate: `undra bindgen -C examples/playground --docs`; `--check` must then pass. Expected diff: the
  `Todos` apply of signal 2 gains the keyed-patch case in all three languages, and `fill` appears. No app source
  changes (the apps read `visible` as a list).

## 6. Contract scenario S19 (`contract-tests/`)

Add to `scenarios.md` after S18, and make `check.sh` require nineteen ids:

> ### S19 derived keyed list
>
> `Todos.visible` is a derived list (ADR-039): it reaches the platform as keyed patches, never as a whole list
> after the first.
>
> 1. `Todos` observed through a raw mirror: the initial `visible` (signal 2) entry is a full value (`op = 0`), `[]`.
> 2. `add("a")`, `add("b")`, `add("c")`: each change-set's `visible` entry is `op = 1` with exactly one
>    `Insert` at index 0, 1, 2; `remaining` is 1, 2, 3.
> 3. `toggle(b)` with filter `All`: `visible` is one `Update{index=1}` (b, done); `remaining == 2`.
> 4. `set_filter(Active)`: the change-set's `visible` entry is one `Remove{index=1}`; `set_filter(All)`: one
>    `Insert{index=1, b}`.
> 5. Under `Active`, `toggle(a)`: one `Remove{index=0}`; `toggle(a)` again: one `Insert{index=0, a}`.
> 6. `fill(10000)` (one full value or one patch for `visible`: either is allowed), then `toggle` of a visible
>    item: the `visible` entry is `op = 1`, one op, **under 100 bytes**.
> 7. Through the generated class (`Todos.create()` / `Todos()`), the same steps: `visible` equals the model
>    (a local list mutated by the same steps) after every step, read-your-writes as in S18.
> 8. Reads never cross: reading `visible` 1,000 times leaves `crossings.calls` unchanged.

Runners: `contract-tests/swift/Tests/ContractTests/`, `contract-tests/kotlin/src/dev/undra/contract/`,
`contract-tests/ts/test/`, each following its S10 implementation for the raw mirror and patch decoding.

## 7. Tests in `undra-signals` (the core of the review)

* **`tests/derived.rs`, the model test** (as `tests/keyed_ops.rs`, `UNDRA_DERIVED_CASES`, default 1,500; run
  100,000 once in a debug build and record the count in the decision record). One store with a keyed source
  and four attached derived lists (filter only; filter + `sort_by_key` with many equal keys; `map` to another
  record + `sort_by_key` on a `String`; `filter_with` a parameter signal) plus a `count()` computed. Random
  sequences of: every recorded operation, `set` / `update` / `replace`, transactions mixing them, parameter
  writes (same value and new value), `observe(on/off)` per slot, Rust reads (`len`, `get`, `with`) inside and
  outside transactions, a computed in the same store that panics on demand (abandoned commits), a sink that
  panics on demand. A host mirror applies every change-set (patches by SPEC 3.8). After every commit, for every
  observed derived list: **host == `view(source)` == `DerivedList::get()`**, computed with the reference
  `stable_sort_by_key(filter_map(source))`; `count == view(source).len()`; and a transaction that contained only
  recorded operations and no parameter change sent each observed view as a patch (`stats().full_values`
  unchanged) unless it overflowed.
* `tests/derived_delivery.rs`: the ADR-019/020 cases for a derived slot (abandon then full value; observe
  rollback; `no_coalesce` unobserved = full value; a view whose drain emits nothing adds no entry and a
  change-set with no entry is not delivered; entries ordered by `signal_id`; `txn_id`s strictly increase), and a
  concurrency test modelled on `tests/concurrency.rs` (writer threads on the source, an observer re-observing).
* Bounds: exactly `TAP_LIMIT - 1`, `TAP_LIMIT`, `TAP_LIMIT + 1` pending ops on an unobserved view (rebuild only
  past the limit, `stats().rebuilds`); `OUT_LIMIT` on an observed one; a parameter walk of 256 and 257 changed
  rows (patch, then full value).
* Re-entrancy (`tests/reentrancy.rs`): a closure that reads its own list panics with the cycle message (no
  hang); a source `update_at` closure that reads the view panics with the L3 message; debug builds: a closure
  that reads another signal, or writes one, panics with the purity message; `#[cfg(not(debug_assertions))]`
  twin: no panic.
* Panics: a filter that panics on a value -> the commit is abandoned, the next commit after the value changes
  sends the full value and the index is consistent (rebuilt).
* An inconsistent `Ord` key (random `cmp`) never loses or duplicates a row in the index (`len` stays right,
  `validate()` holds) and never hangs; a rebuild may panic (std sort), which is contained.
* **Allocation discipline** (`tests/derived_alloc.rs`, a counting global allocator as
  `crates/undra-ffi/tests/sync_alloc.rs` does): after warm-up, an `update_at` on a source of `u64` rows with one
  observed unsorted view and no `map` makes zero allocations in the core beyond the change-set buffer reuse
  (assert an exact count; record it).
* Doc tests on every public item; `cargo test -p undra-signals --release` too (the release twin of the purity
  tests).

## 8. Bench (`bench/`)

* Fixtures (`common/fixtures.rs`): a `Views` store: `#[undra(key = "id")] rows: Signal<Vec<Item>>`, `show:
  Signal<bool>`, `#[undra(key = "id")] open: DerivedList<Item>` (filter `!done`), `#[undra(key = "id")] by_title:
  DerivedList<Item>` (filter `!done`, `sort_by_key(title)`), `#[undra(key = "id")] shown: DerivedList<Item>`
  (`filter_with(&show, ..)`), `open_count: Computed<u32>` (`count()`), with `seed(count)`, `rename(at, title)`,
  `toggle(at)`, `insert_at`, `remove_at`, `flip()` (writes `show`), `reset()` (`replace`). Extend `Churn` (or a
  sibling `ChurnViews`) with `open: DerivedList<Item>` = `rows.derive().filter(|r| !r.done).sort_by_key(|r|
  r.title.clone())` for the stress scenario.
* Workloads (`common/workloads.rs`, group `signals`): the nine rows of ADR-039 section 11, each observing only
  what its definition says, each asserting its shape before timing (entry count, op count, bytes), as the
  `keyed` rows do (`workloads.rs:512-...`).
* `budgets.toml`: the rows and the two `[ratio]` tables, set with the file's own rules after measuring
  (`cargo test -p undra-bench --test budgets --release -- --ignored --nocapture baseline`, best of three). A
  target from the ADR that the measurement misses is reported to the integrator, not absorbed into the budget.
* Stress (`common/stress.rs`): `derived_churn_10k/sustained` in `scenarios()` (not in `BYTES_EXACT`) and the
  layer-A `stress/derived_churn_10k/ops_x1000` in `workloads()`; `common/host.rs`: `ApplyingHost` mirrors
  several `(handle, signal_id)` lists (today it watches one `ListMirror`). Invariants as ADR-039 section 11; the
  `Fault::SkipPatches` mode applied to the view's mirror must make the view invariant fail (add it to the
  harness's fault self-test). `[stress."derived_churn_10k/sustained"]` with `min_per_sec`, `p99_ns`, `p999_ns`,
  `bytes_per_op` (ceiling) per the file's rules, plus `UNDRA_STRESS_SECONDS=10` best-of-three numbers in
  `bench/results/`.
* `RESULTS.md`: a "Derived lists" section with the ADR's baseline tables (core and platforms) and the new rows.

## 9. Docs

* `docs/SPEC.md`: 2.2 (one sentence on `SignalDef`: `computed` with a `key` is a derived keyed list, read-only,
  patched; no new field), 3.8 (the third way: a derived list's ops come from its index; raw writes, overflow and
  large parameter changes are full values), 4.3 (`DerivedList<T>` fields; key required), 5.5 (derived slots in
  the commit; empty drains add no entry), 5.9 (derived lists are left out of snapshots like computeds), 10 (a
  sentence: a derived list is generated as a read-only keyed list; no new shape), 16.1 (the API of section 2.3,
  `attach_derived`, the derived-slot paragraph after "Keyed lists"), 16.3 (what the macro emits).
* `site/docs/concepts.html`: a "Derived lists" section after "Keyed lists" (what, the reference semantics, the
  cost table at 10,000 rows from the ADR, the rule of thumb: write the source with the recorded operations;
  closures are pure; parameters), the Computeds section's `visible` example rewritten, the TOC entry.
* `crates/undra-signals/README.md` and the crate docs (`lib.rs`): a derived lists paragraph.
* `docs/HIGH_FREQUENCY.md`: one paragraph: a view over a churning list costs the change.

## 10. Commands

```bash
source scripts/env.sh
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace && cargo test -p undra-signals --release
UNDRA_DERIVED_CASES=100000 cargo test -p undra-signals --test derived            # once, debug; record the time
cargo build -p undra-signals --target wasm32-unknown-unknown
UPDATE_GOLDEN=1 cargo test -p undra-bindgen --test golden && cargo test -p undra-bindgen
undra bindgen -C examples/playground --docs && undra bindgen -C examples/playground --docs --check
cargo test -p undra-bench --test budgets --release && UNDRA_STRESS_SECONDS=10 cargo test -p undra-bench --test stress --release
bash contract-tests/run-all.sh                                                      # 19 x 3
(cd runtimes/ts/@undra/runtime && npm test); runtimes/kotlin/undra-runtime/scripts/test-local.sh; (cd runtimes/swift/UndraRuntime && swift test)   # unchanged, must stay green
```

## 11. Definition of done

* Every test of section 7 exists and passes; the 100,000-case model run passed once (count and time in the
  decision record); every existing suite passes unchanged (the platform runtimes are not modified).
* The schema hash golden is untouched; no existing bindgen golden changes except `stores` (and `full` if it
  embeds `stores`); the playground's generated diff is the `Todos` apply and `fill`; `--check` passes.
* Contract grid 19 x 3 green.
* The nine budget rows, two ratios and the stress scenario are in `budgets.toml`, measured by the file's
  rules, and every ADR-039 target is met on the reference host or reported as a finding.
* Docs of section 9 landed; every `pub` item documented; clippy, fmt, wasm32 build clean; no `unsafe`, no new
  dependency, no `HashMap` whose iteration order can reach an output (R12).
* `.10x/decisions/sde/derived-keyed-lists.md` records what was built, the measured numbers and any deviation.

## 12. Commits (small, in order)

1. `feat(signals): order-statistic index for derived lists` (index + its tests)
2. `feat(signals): source taps on Signal<Vec<T>>` (taps, recording, invalidation + tests)
3. `feat(signals): DerivedList pipelines: filter, map, sort_by_key, parameters, count` (node, builder, Dep, purity checks + model test)
4. `feat(signals): derived slots in StoreCell` (slot, commit/observe/abandon/snapshot + delivery tests)
5. `feat(macros): DerivedList store fields` (+ expansion and diagnostics tests)
6. `test(meta,bindgen): pin derived signals' schema; stores golden with a derived list`
7. `feat(playground): Todos on recorded operations and derived lists` (+ regenerated bindings)
8. `test(contract): S19 derived keyed list on Swift, Kotlin and TypeScript`
9. `bench: derived list rows, ratios and the derived_churn_10k stress scenario`
10. `docs: derived keyed lists in SPEC, concepts and the signals README`

## 13. What the reviewer attacks

* **Equivalence.** Write an independent model (not the implementer's) and fuzz: ties with source moves, a
  sort key and a filter changing in one `update_at`, inserts and removes at both ends, `clear` then inserts,
  a parameter change in the same transaction as source ops (the walk must run with the new values after the ops
  ran with the old), two views and a `count()` on one source, a raw write in the middle of a transaction of
  recorded ops. The host mirror must equal `view(source)` after every change-set.
* **The bounds**: 4,095/4,096/4,097 pending ops, `OUT_LIMIT`, 256/257 in a walk; an unobserved view after
  100,000 source ops (memory flat, one rebuild on read).
* **Delivery**: abandon (panicking sink, panicking computed in the same store), observe rollback, re-observe
  after a bad patch, a commit that claims the view while another thread writes the source (no stranded op: the
  ADR-027 argument must hold for taps), restore then observe.
* **Re-entrancy and purity**: every path in ADR-039 section 6 panics instead of hanging, in both profiles where
  the ADR says so; a closure that captures the source and reads it in release builds is documented, not hung.
* **Cost**: the scaling ratio (an O(n) hiding in a drain, e.g. a materialisation on the commit path or a
  `Vec::insert` on an index array); allocations per op; the per-op cost with 10 views on one source; the
  unsorted view's memory per row (24 bytes target).
* **Schema and codegen**: every existing schema hashes as before (run the hash golden and the playground's
  hash); the generated declaration of a derived list equals a computed list's; out-of-bounds patch on each
  platform takes the resync path for a computed signal too.
* **wasm32**: the signals crate builds; the playground's web core runs S19.
