# ADR-039: derived keyed lists: filtered, sorted and mapped views kept up to date from the source's recorded operations

Status: **Accepted** (2026-10-01, implemented on `wt/derived-lists`; proposed the same day as v1.2 bet E2,
approved by the founder in Amendment A of `.10x/specs/2026-10-01-v1x-default-choice-design.md`; open decisions
1-5 settled at their recommended values by the integrator). See "Implementation notes" at the end for what was
measured and where the implementation departs from the text. Implementation is scheduled after Track A
(ADR-034/035 and the A3 amendment to ADR-019), which touches the same files of `undra-signals`. Touches
SPEC 2.2 (a sentence: what `computed: true` with a `key` means), 3.8 (a third way the core finds a patch),
4.3 (`DerivedList<T>` store fields), 5.5 and 5.9 (derived slots in commits and snapshots), 10 (a sentence:
no new shape), 16.1 and 16.3 (the API and what the macro emits). **No wire change** (SPEC 3.5 and 3.8 bytes
are unchanged), **no C ABI or wasm ABI change, no schema change** (no new field: the schema hash of every
existing core is unchanged, pinned by a test), **no new generated surface** (a derived list is generated
exactly as a read-only keyed list already is). Constitution R11: the runtime model of a store slot (a
computed that ships keyed patches, maintained incrementally) and a new public API of `undra-signals`, so it
is decided here before code. Brief: `.10x/specs/2026-10-01-derived-keyed-lists-impl.md`.

## Context

### What a filtered list costs today

The playground's `Todos` store (`examples/playground/core/src/todos.rs:57-65`) is the shape every app has:
a keyed `todos: Signal<Vec<Todo>>` and a `visible: Computed<Vec<Todo>>` that filters it
(`todos.rs:76-82`). Since ADR-027 a one-row change of `todos` crosses as a one-op keyed patch. `visible`
does not:

* every write to `todos` invalidates `visible`; an observed computed is recomputed at commit by running
  its closure over the whole list and allocating a new `Vec` (`crates/undra-signals/src/computed.rs:198-222`,
  `:209`), which clones every row that passes the filter;
* a computed slot is always sent as a full value (`crates/undra-signals/src/store.rs:236-249`, the commit
  at `store.rs:735-737`); only a `Signal<Vec<T>>` attached with `attach_keyed` has a baseline and an op log
  (`store.rs:202-228`), and the macro refuses `#[undra(key)]` on a `Computed` (E0008,
  `crates/undra-macros/src/impl_/store.rs:214-230`, test `:916-925`);
* each platform decodes the whole list and replaces its mirror: Swift `self.visible = value`
  (`examples/playground/generated/swift/Sources/PlaygroundCore/Generated/Stores.swift:2261`), Kotlin
  `_visible.value = codecVecTodo.decode(reader)` (`.../generated/kotlin/.../Stores.kt:1632`), TypeScript
  `this.visible._set(decodeValue(vecTodo, value))` (`examples/playground/generated/ts/src/stores.ts:1507`).

So the cost of `visible` is O(list) in the core, on the boundary and on every platform, for every change of
any row, and ADR-027's O(change) promise stops at the first computed.

### Measured baseline (throwaway probes, not committed)

Apple M5 Pro, shared machine (load average 5 to 8), best and median of three runs; rows are `Todo { id:
Uuid, title: String (34 bytes), done: bool }`, every fourth one done, so `visible` holds 75% of them. The
change is one `update_at` of a visible row's title through the recorded API.

**Core** (`undra-signals` directly: a `StoreCell` with `todos` keyed and `visible` computed, both observed; a
sink that copies each change-set as the host does):

| Rows | Per change: commit + recompute + encode + copy | Change-set | Recompute alone | Encode alone | Copy | Keyed `todos` alone (the O(change) reference) |
|---|---|---|---|---|---|---|
| 1,000 | 17.8 us | 41.4 KB | 14.6-16.4 us | 2.5 us | 0.33 us | 0.20-0.22 us, 93 bytes |
| 10,000 | 179-182 us | 412.6 KB | 139-144 us | 25-26 us | 4.6 us | 0.21-0.25 us, 93 bytes |
| 100,000 | 2.68-3.68 ms | 4.13 MB | 1.64-1.70 ms | 0.26-0.28 ms | 51-57 us | 0.24-0.28 us, 93 bytes |

Eighty percent of the core cost is cloning the rows that pass (a `String` per `Todo`); the rest is encoding
them.

**Platforms** (each runtime's own reader, codecs and `applyPatch`, a `Todo` codec identical to the generated
one; `visible` = 750 / 7,500 / 75,000 rows): the full value today against a one-op keyed patch.

| Rows (visible) | TypeScript, Node 24 V8: full / 1-op | Kotlin, JVM 17 HotSpot: full / 1-op | Swift 6, `-O`: full / 1-op in place / 1-op while a view holds the array |
|---|---|---|---|
| 1,000 (750) | 115-126 us / 0.40 us | 12.1 us / 0.08 us | 69-71 us / 0.22 us / 4.6 us |
| 10,000 (7,500) | 1.13-1.19 ms / 1.6 us | 121-123 us / 0.85 us | 0.72-0.80 ms / 0.22 us / 43-49 us |
| 100,000 (75,000) | 12.7-13.4 ms / 74 us | 1.19-1.21 ms / 6.8 us | 7.1-7.5 ms / 0.21 us / 418-431 us |

What remains of a one-op patch at 100,000 rows is the list copy the TypeScript and Kotlin appliers make
(`runtimes/ts/@undra/runtime/src/wire/payloads.ts:880-881`, `runtimes/kotlin/.../wire/KeyedPatch.kt:131-132`)
and Swift's copy-on-write when another reference holds the array (`applyPatch(_:to:)` is `inout`,
`runtimes/swift/.../Wire/KeyedPatch.swift:133`); ADR-031's per-drain merge pays it once per frame.

**Where a patch stops paying** (insert/remove ops in the middle of the 7,500-row list against its full
value): TypeScript about 2,500-3,000 ops (a 2,048-op patch costs 0.78-0.85 ms against 1.13 ms), Swift about
300 (2.5 us per op against 0.72-0.80 ms), Kotlin about 150 (0.75 us per op once the applier has copied the
list, against 121 us). Each positional op moves the tail of the array; a full value is one decode.

The device numbers are slower than these host numbers (an A15 core, ART); the ratios are what this ADR
relies on.

## Decision

### 1. The primitive: `DerivedList<T>`, built from a list signal by a pipeline

```rust
// undra-signals (re-exported by undra::prelude)
impl<T: SignalValue> Signal<Vec<T>> {
    pub fn derive(&self) -> Derive<T>;                       // starts a pipeline over this list
}
impl<T, U, O> Derive<T, U, O> {                              // O: Unsorted | Sorted<K> (typestate)
    pub fn filter(self, f: impl Fn(&U) -> bool + Send + Sync + 'static) -> Derive<T, U, O>;
    pub fn filter_with<D: Dep>(self, param: D, f: impl Fn(&D::Value, &U) -> bool + Send + Sync + 'static) -> Derive<T, U, O>;
    pub fn map<V: SignalValue>(self, f: impl Fn(&U) -> V + Send + Sync + 'static) -> Derive<T, V, O>;
    pub fn build(self) -> DerivedList<U>;                    // the view, attachable to a store
    pub fn count(self) -> Computed<u32>;                     // only its length: O(log n) per row change, no scan
}
impl<T, U> Derive<T, U, Unsorted> {                          // at most one sort, enforced by the type
    pub fn sort_by_key<K: Ord + Clone + Send + Sync + 'static>(self, f: impl Fn(&U) -> K + Send + Sync + 'static) -> Derive<T, U, Sorted<K>>;
    pub fn sort_by_key_with<D: Dep, K: ..>(self, param: D, f: impl Fn(&D::Value, &U) -> K + ..) -> Derive<T, U, Sorted<K>>;
}
pub struct DerivedList<T>;   // Clone = the same list; Send + Sync
impl<T: SignalValue> DerivedList<T> {
    pub fn len(&self) -> usize;  pub fn is_empty(&self) -> bool;          // O(1) once caught up
    pub fn get(&self) -> Vec<T>;  pub fn with<R>(&self, f: impl FnOnce(&Vec<T>) -> R) -> R;   // materialise: O(n), cached until the next change
    pub fn ptr_eq(&self, other: &Self) -> bool;  pub fn is_attached(&self) -> bool;  pub fn stats(&self) -> DerivedStats;
}
impl<T: SignalValue> Dep for &DerivedList<T> { type Value = Vec<T>; }  // usable in Computed::new / Effect::new (materialises)
impl StoreCell { pub fn attach_derived<T: SignalValue>(self: &Arc<Self>, list: &DerivedList<T>, signal_id: u32, key: fn(&T) -> u64) -> Result<(), SignalsError>; }
```

**Semantics, stated as the reference the property tests compare with.** For a source list `s` and the
current parameter values, the view is

```text
view(s) = stable_sort_by_key( [ out(t) for t in s if passes(t) ] )      // no sort stage: no sort
```

where `passes` and `out` are the pipeline's filters and maps applied in the order they were written, and the
sort is stable: rows with equal sort keys keep the order they have in the source (what `Vec::sort_by_key`
does). A filter after a sort is the same as a filter before it, and a map after a sort does not move a row, so
every chain normalises to "one composed per-row function `&T -> Option<(K, U)>`, then at most one stable
sort by `K`". Pipelines are fused into one node; a `DerivedList` is not itself a source of another one in v1
(section 10).

The incremental machinery is invisible except in cost: after every change-set, the host's list equals
`view(source)` exactly, and so does `DerivedList::get()`.

### 2. Maintenance: one source op in, at most two derived ops out

A derived list does not read the source list to stay current. It replays the source's **recorded
operations** (ADR-027: `push`, `insert`, `remove`, `update_at`, `move_item`, `clear`) on an index of its own
and emits, for each, the SPEC 3.8 ops that move the host's view along. Positions are what the source ops
carry, so the index is positional and never hashes a key:

| Source op | Row before → after | Derived ops emitted (indices in the view) |
|---|---|---|
| `Insert{i, t}` | none → passes | `Insert{rank, out(t)}` |
| | none → filtered out | none |
| `Remove{i}` | passed | `Remove{rank}` |
| | filtered out | none |
| `Update{i, t}` | passed → passes, same sort key | `Update{rank, out(t)}` |
| | passed → passes, new sort key | `Move{old rank, new rank}` (when they differ) then `Update{new rank, out(t)}` |
| | out → passes | `Insert{rank, out(t)}` (a filter transition is an insert) |
| | passed → out | `Remove{rank}` |
| | out → out | none |
| `Move{from, to}` | passed | unsorted: `Move{old rank, new rank}` when they differ; sorted: the row keeps its place unless it has equal-key neighbours whose source order it crossed, then `Move` |
| | filtered out | none |
| `Clear` | | `Clear` if the view was not empty |

*Rank* is the row's index in the view: the number of passing rows before it in source order (unsorted), or
its rank by `(sort key, source position)` among passing rows (sorted). Each emitted op is computed right
after the index applied the source op, so the ops are sequential exactly as SPEC 3.8 applies them.

A sort key that changes is a `Move` plus an `Update`, not a `Remove` plus an `Insert`: the same two ops, but
the platforms keep the row's identity (SwiftUI and Compose animate a move; React keeps the component). A map
whose output did not change is still an `Update` (no minimality, as in ADR-027). Keys play no part in
maintenance: two rows with the same key are two rows, and a row whose key field changes is an `Update`.

**Many source ops in a transaction become one patch.** The ops of a transaction are replayed in order when
the derived slot is committed and their derived ops concatenated into one keyed-patch entry. A source op
that changes nothing in the view emits nothing, and a derived slot whose drain emitted no op contributes no
entry to the change-set (a change-set left with no entry is not delivered). Toggling a row the filter hides
costs the view nothing on the wire.

### 3. The index: two arena AVL order-statistic trees with parent pointers

* **Positional tree**: one node per source row, in source order, augmented with the subtree's row count
  and passing count. `select(i)` finds the row at source index `i`; `insert_at`, `remove`, `move` follow
  the source op; a node's passing prefix (`rank` for an unsorted view) is a walk up the parent pointers.
  O(log n) each.
* **Sorted tree** (only with a sort stage): one node per passing row, ordered by `(K, source position)`,
  augmented with the subtree size; the row's sort key lives in its node. Insertion descends by comparison;
  equal keys are ordered by their current source position, read from the positional tree (O(log n)), so a
  comparison is O(1) except between equal keys. Removal and rank start from the node (parent pointers), so
  **removal never searches**: a sort key whose `Ord` is inconsistent can misplace a row but cannot lose one.
* Arenas are `Vec`s of nodes with `u32` links, reused through free lists; a row keeps its node ids for life
  (a `move` detaches and reattaches the same node). Safe Rust, no dependency, deterministic (no hashing, no
  randomised balancing: a treap or skip list would need randomness, R12), and worst-case O(log n) per
  operation.

Cost per source op: **O(log n)**; O(log² n) in the worst case of a sorted view whose rows mostly share one
key (each comparison with an equal key reads a position). Measured on a throwaway prototype of exactly this
structure (per source op, including the closure-equivalent work of cloning the item and the key, a fixed
cycle of toggles, key changes, inserts, removes and moves; 200,000 ops after 20,000 warm-up):

| Rows | Filter only | Filter + `sort_by_key(title)` | Filter + `sort_by_key(priority)`, 4 distinct keys (heavy ties) |
|---|---|---|---|
| 10,000 | 256 ns | 449 ns | 588 ns |
| 100,000 | 515 ns | 1,036 ns | 1,240 ns |

The prototype also replayed 20,000 random ops on three views of a 300-row list and applied every emitted
op to a host-side list: it equalled `filter + stable sort` of the source after every op.

**Memory per derived list**: 24 bytes per source row for the positional tree (unsorted view: 240 KB at
10,000 rows, 2.4 MB at 100,000); a sorted view adds about 24 bytes plus `size_of::<K>()` (and the key's heap,
for a `String` key) per passing row. Plus, transiently, at most 4,096 pending source ops and 4,096 pending
derived ops (section 4), and the materialised `Vec<T>` only after a Rust read (dropped at the next change).
Today's computed caches the whole filtered `Vec` (about 0.6 MB at 10,000 rows of `Todo`, its titles
included), so an unsorted view uses less memory than the computed it replaces.

### 4. How the ops reach the index: source taps, bounded

* A `Signal<Vec<T>>` gains a list of **taps**, one per derived list built from it, next to the slot's op log
  (`crates/undra-signals/src/signal.rs:98-100` has room for one consumer today). A recorded operation
  builds its op once for each consumer that is recording and appends it **inside the value's write lock**,
  the same critical section as ADR-027's log (`crates/undra-signals/src/signal/list.rs:13-57`); a raw write
  (`set`, `update`, `replace`) marks every tap stale before it touches the list (`signal.rs:152-185,
  223-227`). The source keeps taps by `Weak`, so dropping a derived list removes its tap. A source with no
  derived list pays one empty check per recorded op.
* A tap records independently of the source slot: the source need not be attached, observed or keyed.
* **Draining** takes the tap's ops and replays them on the index. It happens when a commit claims the
  derived slot, when Rust code reads the list (`len`, `get`, `with`, `count()`, a `Computed` over it), and
  on `observe(on)`. A drain that also needs the source's items (materialising, rebuilding, a parameter walk)
  takes the ops and an `Arc` snapshot of the list together under the value's read lock, exactly as
  `KeyedLog::take` does (`crates/undra-signals/src/oplog.rs:139-169`), so the ops it replays are exactly the
  ones the snapshot includes.
* **Bounded at 4,096 ops**, with ADR-027's numbers but not its scaling: a tap that holds 4,096 ops without
  being drained goes stale and stops recording, and the next drain **rebuilds** the index from the source
  (O(n log n): one pass, a stable sort of the passing rows, two balanced builds). Unlike ADR-027's slot log
  (`max(4096, len)`, `oplog.rs:117-123`) the bound does not grow with the list: a derived list nobody
  observes or reads would otherwise hold up to a full copy of the list in pending items. The emitted derived
  ops waiting for the next commit are bounded the same way (4,096, which is also where ADR-031's mirror drops
  a merged patch, SPEC 11.1); past it the next commit sends the full value and the index stays valid.
* **A raw write to the source rebuilds the view and sends its full value.** The source slot diffs a raw
  write (ADR-027); a derived list has no baseline of its own items to diff against and does not keep one (it
  would double its memory). Store code that wants O(change) views writes its source with the recorded
  operations, which it needs for the source itself anyway.
* **Parameters.** `filter_with(&p, ..)` and `sort_by_key_with(&p, ..)` take one `&Signal<P>` or
  `&Computed<P>` (several: put them in a `Computed` of a tuple). The derived list subscribes to it like a
  computed does, keeps the parameter's last snapshot and compares the new one by its encoded bytes at the
  next drain. When it changed, the drain first replays the source ops with the old values, then walks every
  row, re-evaluates it with the new values and applies the same per-row transition as an `Update` for each
  row whose membership or sort key changed: O(n) evaluation plus O(k log n) for k changed rows. **The walk
  emits a patch of at most 256 ops; beyond that the view is sent as a full value.** 256 sits between the
  measured break-even points of the three platforms (Kotlin about 150 ops, Swift about 300, TypeScript about
  2,500); a filter switch on a large list is a full value, a narrowing filter on a short one is a patch. A
  `filter_with` closure must read its parameter through its first argument: a filter or key that reads other
  signals inside the closure is **not supported in v1** (section 6).

### 5. Delivery: a derived slot in the store

`StoreCell::attach_derived` installs a slot of a new kind next to `Plain`, `Keyed` and `Computed`
(`crates/undra-signals/src/store.rs:54-58`). Every rule of the existing slots applies, with the derived list
standing in for the keyed baseline:

* **Commit** (`store.rs:723-738`): claim, drain, then take the pending derived ops: a patch (`op = 1`), the
  full value (`op = 0`) when the pending ops overflowed or the view was rebuilt, or **nothing** (no entry).
  An observed derived list keeps its pending derived ops (the host's state is "everything sent"); an
  unobserved one discards them as it drains.
* **`observe(on)`** (`encode_settled`, `store.rs:530-564`): drain, materialise, encode the full value, start
  keeping pending ops from that instant. **`observe(off)`** (`store.rs:498-505`): stop keeping them; the
  index stays (Rust reads and `count()` still need it).
* **Abandoned delivery** (ADR-019, `Abandon` at `store.rs:795-813`): pending ops are dropped and the slot is
  unsent; the next commit sends the full value. A closure that panics in the middle of a drain marks the
  index for rebuild (a guard, like `StaleOnUnwind` in `signal/list.rs:52-76`), so a half-applied op cannot
  survive. When Track A's per-slot isolation (A3) lands, a derived slot is isolated exactly as a computed
  slot is.
* **`no_coalesce`** while unobserved: the full value each time, as for a keyed list.
* **Snapshots** leave derived slots out, as they leave computeds out (`encode_snapshot`, `store.rs:586-604`;
  SPEC 5.9); restore rebuilds them through the store's `restore` hook, which already has to build the
  computeds.
* **Lock order**: the store's delivery lock, then the derived list's drain lock (held while its closures run,
  as a computed's closure runs under the delivery lock, ADR-020), then the source's value lock (read, only
  to take ops with a snapshot), then the tap (a leaf). Writers take the source's value lock, then the slot
  log or a tap, one leaf at a time. No writer ever takes a drain lock.
* **Ordering and merging** (ADR-019, ADR-020, ADR-031): unchanged. A derived entry is an ordinary keyed patch
  or full value relative to what the host had after the previous change-set of the store, so the platform
  concatenates consecutive patches of one key per drain and lets a full value supersede them, as it does for
  every keyed list.

### 6. Determinism, purity and re-entrancy

* **Purity by API shape (R12).** Every pipeline closure is `Fn(&U) -> X + Send + Sync + 'static` (or
  `Fn(&P, &U) -> X` with a declared parameter): no `&mut` to the row, no `Ctx`, no return channel other than
  its value. The tie order is the source order, never a hash, so a view is a pure function of the source and
  the parameter values. What the type system cannot stop is a closure that reaches state through something it
  captured (a `Signal`, a `Mutex`, a clock through `Ctx::current()`); such a closure makes the maintained
  view diverge from `view(source)`, because a row is re-evaluated only when it changes. Documented on every
  method, and enforced where it is cheap: **debug builds panic** when a pipeline closure reads or writes a
  signal, a computed or a derived list (a thread-local "deriving" depth checked where reads and writes
  already pass: `Signal::snapshot`, `ComputedInner::current`, `write_with`). Release builds do not check.
* **Sort keys must be a total order.** `Ord` is trusted; an inconsistent one cannot cause unsafety or lose a
  row (removal never searches), and std's sort may panic on it during a rebuild, which is contained like a
  panicking computed.
* **Re-entrancy (ADR-021).** A closure that reads its own derived list (through a captured handle) would wait
  for the drain lock its thread holds: detected through a thread-local stack of draining lists and reported
  as a panic ("derived list cycle"), the computed-cycle rule of ADR-021 L1 (`computed.rs:72-110`). Reading a
  derived list inside the `update` or `update_at` closure of its source would wait for the source's value
  lock: the drain checks `assert_not_updating` (`signal.rs:236-248`) on the source first and panics with the
  ADR-021 L3 message, whether or not that drain needed the lock. Two threads whose impure closures read each
  other's derived lists could deadlock, as two computed closures under two delivery locks can (ADR-020); debug
  builds already panic at the read.

### 7. The schema and the generated code do not change

A derived list is described as `SignalDef { ty: Vec<T>, computed: true, key: Some("field"), .. }`
(`crates/undra-meta/src/def.rs:184-203`). That combination was impossible until now (E0008 refuses a key on a
`Computed`), so **no existing schema changes and no hash moves**; no new field is needed, because the
platforms only need to know that the signal is read-only (`computed`) and receives keyed patches (`key`).
Every generator already emits the patch branch for any signal with a key and a `Vec` type, computed or not
(`crates/undra-bindgen/src/swift.rs:1168-1176`, `ts.rs:1831-1846`, `kotlin.rs:1553-1570`), validation only
asks for the `Vec` (`validate.rs:831-837`), and a computed signal is documented "Computed by the core;
read-only." (`model.rs:448-450`). So a derived list's generated property is byte-for-byte the property a
`Computed<Vec<T>>` had (Swift `public private(set) var visible: [Todo]`, Kotlin `val visible:
StateFlow<List<Todo>>`, TypeScript `readonly visible: Signal<Todo[]>`), and its `apply` gains the keyed-patch
case every keyed list has: R3 holds with no new generated surface. Pinned by tests: the representative
schema's hash golden (`crates/undra-meta/src/canonical.rs:385-392`, `0xd5b8_c3a3_afbd_bc33`) does not move; a
derived signal's canonical JSON is exactly `..."computed":true,"key":"id"}` with no other field; the bindgen
`stores` golden gains a derived field whose Swift, Kotlin and TypeScript differ from a computed's only by the
patch case.

Rejected: a `SignalDef.derived` flag written only when set (the ADR-031 `no_coalesce` precedent) and a
`TypeRef::Keyed`. No consumer needs them (R1 describes what crosses, and what crosses is a read-only keyed
list); either would move the hash of every schema that adopts a derived list for nothing. Devtools (B4) can
add the flag, with the skip-when-false rule, if it ever needs to tell the two apart.

### 8. Lazily paged lists stay separate

`Lazy<T>` (`crates/undra-runtime/src/lazy.rs`) is a different contract: a runtime-owned object of
pre-encoded items (`lazy.rs:29-45`) that the host pages with `(offset, limit)` (`lazy.rs:109-127`, SPEC 3.3
target 3) and that tells the host only "invalidated" (op 2); no store can declare one in v1 (E0001,
`crates/undra-macros/src/impl_/store.rs:188-203`) and no platform has a paging API (Track E3). A derived list
is a mirrored signal: the host holds the whole view and receives patches. **Decision: they do not compose in
v1.2; a derived list cannot be paged.** Making one pageable is a new generated shape (R3, R11) and belongs to
E3's ADR. The index is built so that it can: `select(k)` on the sorted (or positional) tree is a page's
starting row in O(log n), so a future `Lazy<T>` over a `DerivedList<T>` costs O(log n + limit) per page and
can invalidate only when a changed rank falls inside a window the host holds. Until then the guidance (docs)
is: mirror views up to tens of thousands of rows; the host pays memory for the whole view.

### 9. API in a store, and the playground rewritten

`#[undra::store]` recognises a `DerivedList<T>` field (by its last path segment, as it does `Signal` and
`Computed`), **requires** `#[undra(key = "field")]` on it (E0008 otherwise: the platforms apply its changes as
keyed patches, and the key names the row identity a UI diffs by), records it as above and attaches it with
`attach_derived(&self.visible, id, __undra_key_visible)?`. The key function is used only by debug builds, to
check at each full value that the view's keys are unique (a duplicate breaks `ForEach`/`LazyColumn` identity
on the platform; maintenance does not care). A derived field is computed for `E0013` (a store that has one
needs a `restore` hook) and is left out of the hook's parameters. `#[undra(key)]` on a `Computed<Vec<T>>` stays
E0008, whose help now says to build it as a `DerivedList<T>`. Builders run in the constructor; there is no
attribute form (a pipeline is code, and the macro cannot type-check closures it would have to parse out of an
attribute).

```rust
#[undra::store(restore = "Self::assemble")]
pub struct Todos {
    next: AtomicU64,
    #[undra(key = "id")]
    todos: Signal<Vec<Todo>>,
    filter: Signal<Filter>,
    #[undra(key = "id")]
    visible: DerivedList<Todo>,
    remaining: Computed<u32>,
}

#[undra::api(store)]
impl Todos {
    fn assemble(_ctx: Ctx, todos: Signal<Vec<Todo>>, filter: Signal<Filter>) -> Self {
        // Kept current from the list's recorded operations: one toggle is one op at 10 or 100,000 rows.
        // A filter change walks the list once and sends what entered and left (or the whole view).
        let visible = todos.derive().filter_with(&filter, |filter, todo| filter.matches(todo)).build();
        // Only the length of a view is needed here: kept as rows come and go, no scan per change.
        let remaining = todos.derive().filter(|todo| !todo.done).count();
        let next = todos.with(|list| list.iter().map(|t| counter_of(t.id)).max().unwrap_or(0));
        Self { next: AtomicU64::new(next + 1), todos, filter, visible, remaining }
    }

    pub async fn add(&self, title: String) -> Result<Todo, TodoError> {
        // .. validation and id as today ..
        self.todos.push(todo.clone());                          // was `update(|l| l.push(..))`: now one Insert
        Ok(todo)
    }

    pub fn toggle(&self, id: Uuid) {
        if let Some(at) = self.position(id) {
            self.todos.update_at(at, |todo| todo.done = !todo.done);   // todos: Update; visible: Update, Insert or Remove
        }
    }

    pub fn remove(&self, id: Uuid) {
        if let Some(at) = self.position(id) { self.todos.remove(at); }
    }

    pub fn clear_done(&self) {
        let done: Vec<usize> = self.todos.with(|l| (0..l.len()).rev().filter(|&i| l[i].done).collect());
        txn(|| {
            for at in done {
                self.todos.remove(at);                          // highest index first; one change-set
            }
        });
    }

    fn position(&self, id: Uuid) -> Option<usize> {
        self.todos.with(|list| list.iter().position(|todo| todo.id == id))
    }
}
```

The generated Swift, Kotlin and TypeScript of `Todos` keep every public declaration; `visible` changes from
"decoded whole on every change" to "patched". What stays O(n) in this store is app code: finding a row by id
(`position`, 2.6-3.2 us at 10,000 rows, 22-26 us at 100,000, measured) and `clear_done`'s scan; neither
allocates or crosses the boundary. (Today's `remaining` scan is 5.3-6.6 us at 10,000 rows; `count()` replaces
it with an O(log n) update per row change and an O(1) read.)

### 10. Not in v1.2

A derived list as the source of another (pipelines are fused instead; chaining nodes needs ordered drains of
an upstream's emitted ops and is a follow-up if fused pipelines prove insufficient); `map_with` (a parameter
that changes every row's output is a full value: a `Computed` over the list says that more honestly);
aggregates other than `count()` (a `sum`/`group_by` is a follow-up on the same index); diffing a raw write
(a per-row fingerprint of the mapped value would allow a keyed diff on rebuild; deferred until a measured
need); paging (section 8); a position index by key on the source (`position_of(key)`, the same tree with a
key map; it would make the playground's `position` O(log n)).

### 11. Budgets (R9)

New `bench/budgets.toml` rows, set by the file's rule (5x the measured p50, floor 250 ns, best of three on the
reference host) once implemented. The targets below are what the design commits to on the reference host;
missing one is a finding, not a budget to widen.

| Row (`[bench."..."]`) | What one iteration is | Target (host) |
|---|---|---|
| `signals/derived_10k/update_visible` | `update_at` of a row an unsorted view shows; source and view observed; one change-set with two one-op entries | <= 1 us |
| `signals/derived_10k/toggle_membership` | `update_at` that moves a row in or out of the filter: view `Insert` / `Remove` | <= 1.5 us |
| `signals/derived_10k/sort_key_change` | filter + `sort_by_key(title)`, `update_at` that changes the title: `Move` + `Update` | <= 2 us |
| `signals/derived_100k/update_visible` | as above at 100,000 rows | <= 2 us |
| `signals/derived_100k/sort_key_change` | as above at 100,000 rows | <= 4 us |
| `signals/derived_10k/insert_sorted` | `insert` in the middle of the source; view sorted: one `Insert` | <= 1.5x `signals/keyed_10k/insert` |
| `signals/derived_10k/param_flip` | `filter_with` parameter flipped (2,500 rows change): O(n) walk, full value | <= 300 us (guards the O(n) path at today's cost) |
| `signals/derived_10k/rebuild_after_replace` | `replace` on the source: rebuild, full value | <= 1 ms (guard) |
| `signals/derived_10k/count_toggle` | `count()` over a filter attached as an observed `Computed<u32>`, source observed; `update_at` toggling membership | <= 1 us |

Ratio gates (speed cancels): `[ratio."derived_sort_scaling"]` = `derived_100k/sort_key_change` /
`derived_10k/sort_key_change`, max 4 (O(log n) and cache effects measured 2.0-2.6x on the prototype; an O(n)
step would be about 10x); `[ratio."derived_vs_keyed_update"]` = `derived_10k/update_visible` /
`keyed_10k/update`, max 6 (the view costs a small multiple of the keyed source it follows). Final `max`
values follow the file's 1.15x-of-largest-healthy-sample rule.

Stress (layer B, `bench/common/stress.rs`): **`derived_churn_10k/sustained`**, the `Churn` store
(`bench/common/fixtures.rs:340-437`) with an observed `open: DerivedList<Item>` = `rows.derive().filter(|r|
!r.done).sort_by_key(|r| r.title.clone())`; the existing fixed cycle (every `Update` toggles `done`, so it is a
membership change) drives source and view; the host mirrors both lists. Invariants: the host's view equals
`filter + stable sort` of the core's rows field for field at the end; every source op produced at most two
derived ops and every view change-set entry was a patch after the first full value (no rebuild:
`DerivedList::stats`); no patch failed; one change-set per op; the fault-injection mode that skips patches
breaks the view invariant. Gates: `min_per_sec` (target: at least half of `keyed_churn_10k/sustained`'s
measured 169,000/s), `p99_ns`, `p999_ns`, and `bytes_per_op` as a ceiling (the view's bytes depend on which
ops fall inside it, so this scenario is not in `BYTES_EXACT`). Layer A row `stress/derived_churn_10k/ops_x1000`.

## Alternatives considered

* **Recompute and diff on the platform** (keep `Computed<Vec<T>>`, let SwiftUI/Compose/React diff by id).
  They already do; it does not touch the cost, which is the core's clone and encode, the 412 KB on the
  boundary and the platform's decode (1.1 ms on TypeScript at 10,000 rows). Rejected.
* **Diff in the core** (`#[undra(key)]` on a `Computed<Vec<T>>`: keep a baseline of its last value and send
  `KeyedPatch::diff`). O(change) on the boundary and the platforms, but O(list) twice in the core: the
  recompute (140 us at 10,000 rows) plus the diff (`wire/keyed_patch_10k/diff`, 435 us,
  `bench/budgets.toml:185-187`), plus a baseline copy of the view. It is the right tool for a computed list
  that is not a pipeline (a join, a grouping); kept as a possible follow-up for those, not as E2's answer.
* **Materialised views through the query layer or the `Db` port (G3)**: views as cached queries over a
  local table. The query cache holds encoded bytes per key and refetches, which is not incremental; SQL views
  live behind an async foreign port (SQLite on the platform side), so every change would be a port round
  trip, outside the core's determinism, and would tie E2 to G3. Reactive SQL over persisted data is a
  different problem worth its own ADR.
* **A general incremental-computation engine** (differential dataflow, salsa-style memoisation). Far more
  than three operators need, with dependencies that do not build for wasm32 and mobile under our rules.
* **Other index structures.** `BTreeMap<(K, position), _>`: no rank, so a view index costs O(n). Treap or
  skip list: need randomness (R12), or hashed priorities with adversarial worst cases. Scapegoat tree: an
  occasional O(n) rebuild, a latency spike at 100,000 rows. Order-maintenance labels for the tie order:
  O(1) tie comparisons but a relabelling algorithm to get right; worth it only if heavy ties miss the budget
  (the prototype says they do not). A counted B+tree: better constants, roughly three times the code;
  revisit if a target is missed.
* **Remove + Insert for a sort-key change.** Same op count; the platforms lose the row's identity. Rejected
  for Move + Update.
* **Ties broken by key.** O(1) comparisons, but rows with equal sort keys would appear in hash order (a
  "sort by done" list scrambled within each group). Rejected for the source order, which is what
  `Vec::sort_by_key` gives.
* **Tap bound `max(4096, len)` as ADR-027.** A view nobody reads would hold up to a copy of the list in
  pending items; a fixed 4,096 bounds that, at the price of an O(n log n) rebuild after a long unobserved
  stretch (which an observe pays O(n) for anyway).
* **Derived lists computed on the platform** (the TypeScript mirror filters `todos` itself). Domain logic in
  three languages, which is what Undra exists to prevent.

## Consequences

* At 10,000 rows a toggle of a visible row goes from about 180 us and 412 KB in the core and 0.12-1.2 ms on
  the platforms to about 1 us and one 81-byte entry in the core and 0.2-1.6 us on the platforms (host
  numbers; 43-49 us on Swift when a view holds the array and the patch copies it); at 100,000 rows from
  2.7 ms, 4.1 MB and 1.2-13 ms to about 2 us, 81 bytes and 0.2-75 us (Swift's copy-on-write 0.42 ms). What the
  platforms still pay at 100,000 rows is their list copy, once per drain (ADR-031), not the core.
* New public API in `undra-signals` (`Derive`, `DerivedList`, `DerivedStats`, `Signal<Vec<T>>::derive`,
  `StoreCell::attach_derived`, `Dep for &DerivedList<T>`), `DerivedList` in `undra::prelude`, one new field
  form in `#[undra::store]` and E0008 texts. No existing signature changes; `SignalInner` gains a tap list;
  `SlotKind` gains `Derived`; a commit may skip a slot that produced nothing.
* O(change) holds only for sources written with the recorded operations; a raw write costs a rebuild and a
  full value of every view over that source. The docs say so where they say it for keyed lists, and
  `DerivedList::stats()` (rebuilds, full values sent) lets a test or devtools catch it.
* Cost when unused: one empty check per recorded op, one pointer per `Signal`.
* Contract scenario **S19 derived keyed list** on all three platforms (the grid grows to 19 x 3): the
  rewritten `Todos`, observed through the raw mirror; each `add` is a one-op `Insert` on `visible`; a toggle
  is a one-op `Update`, or a one-op `Insert`/`Remove` under `Active`; `set_filter` sends the membership patch;
  a 10,000-row `fill` then a toggle sends a `visible` entry under 100 bytes; the generated `visible` equals the
  model after every step. S11 keeps passing unchanged (it reads values).
* Tests (in the brief): the index against a `Vec` model; the pipeline against `view(source)` for random op
  sequences, transactions, raw writes, parameter changes, observe/unobserve, abandoned commits and concurrent
  writers, with a host mirror applying every change-set (1,500 cases by default, 100,000 once in a debug
  build); delivery-integrity, re-entrancy and macro diagnostics; the schema pins.
* Track A dependency: A2 (ADR-035) changes `write_with` and `StoreCell` (owner); A3 isolates a panicking
  computed slot. The implementation rebases on both and gives derived slots A3's isolation.

## Open decisions for the integrator

1. **Parameters in v1** (`filter_with`, `sort_by_key_with`): recommended yes; without them the playground's
   `visible`, the founder's example, cannot be expressed. A closure that reads other signals stays out.
2. **Names**: `DerivedList<T>`, `derive()`, `Derive`; the alternative `Derived<Vec<T>>` reads like
   `Computed<Vec<T>>` but admits non-list `T`s it cannot support.
3. **Caps**: 4,096 pending ops (both directions) and 256 for a parameter walk; both named constants.
4. **`count()`** in v1 (recommended: it is what `remaining` and every badge count needs, and it is small).
5. **Tie order and sort-key changes**: source order, `Move` + `Update` (recommended).

## Implementation notes (2026-10-01)

Built as decided; the record is `.10x/decisions/sde/derived-lists.md`. Every section 11 target is met on the
reference host (`bench/RESULTS.md`, finding 5): one change of a 10,000-row filtered view costs 333 ns and 158
bytes in the core (176.5 µs and 353 KB as a computed), 392 ns through the runtime; the sorted view scales
1.04-1.12x from 10,000 to 100,000 rows; a parameter flip of 2,500 rows is 218 µs; the sustained derived churn
runs at 122,000 operations a second beside 169,000 for the list alone. Departures from the text:

* Section 5, "A closure that panics in the middle of a drain marks the index for rebuild": done, and with the
  ADR-019 amendment landed first a derived slot is **isolated** like a computed (held back, listed in
  `failed_signals`, sent whole when it next evaluates) rather than abandoning the change-set.
* Section 3 and 4: beyond the decision, ranks are computed only when an op is kept for the host (an unobserved
  list and a `count()` never compute one), a parameter walk skips rows whose membership and key did not
  change, and a full value is encoded from the source rows without materialising the view. The first
  measurement without these missed the `param_flip` target (1.43 ms).
* Section 11: `[ratio."derived_vs_keyed_update"]` max is 2.5, not 1.15x of a host sample (no runner samples
  yet); `[ratio."derived_sort_scaling"]` keeps 4 (cache-bound, the fan-out precedent). The rebuild row's fixture
  swaps in a prebuilt list so it measures the rebuild.
* Section 7: no bindgen golden changed. The `stores` golden already describes a computed keyed list; the
  pinning test compares it with a computed list it adds to its own copy of the schema.
* Consequences, S19: the Kotlin and Swift S11 raw sub-steps now apply `visible`'s patch (they decoded a full
  value); S19 adds a step 9 that replays 60,000 recorded operations over three views through each runtime's
  decoder and applier.

