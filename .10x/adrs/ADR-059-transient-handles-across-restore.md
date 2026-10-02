# ADR-059: transient handles across a restore: the snapshot keeps what a query handle is made of, a restore re-issues the handle, and the object is built again when the host first uses it

Status: **Accepted** (2026-10-02; implemented in `wt/reload-handles`: see "Implementation note" at the end for what differs from
the text above it, which is left as the architect wrote it). Touches `undra-wire`
(one reserved field id and a reader helper; **no layout change**), `undra-runtime` (the object table, `snapshot`,
`restore`, `observe`, dispatch, `RestoreReport`), `undra-query` (the record and what builds a handle again),
`undra-transport` (the devtools hub's store list), `undra-cli` (the dev runner's counts and one terminal phrase),
`undra-bindgen` and the TypeScript runtime (the host-side replay of ADR-049 decision 3.4.5 is **removed**), the
contract scenarios (a new S35; S15 and S22 reworded), SPEC 1.1, 5.4, 5.9, 5.10, 9, 10.3, 16.2 and 17.1, and the docs,
the roadmap and the post that list this as open. It does **not** touch the envelope (3.2), the layout of any payload,
the C ABI, the wasm ABI, the `undra_restore` codes, the schema or its hash, the Swift and Kotlin runtimes, or generated
Swift and Kotlin. It supersedes ADR-049 decision 3.4.5 (the TypeScript replay), the "query handles" row of ADR-053's
"What is carried" table and its decision 2 as far as query handles go, and the sentence "query handles go stale" of
ADR-054 decision 4. Constitution R11: decided here, before the code; R7: section 4.

## Context

A query is observed through a **query handle** (SPEC 9): an object the host constructs (type id and constructor id
are the query id, the arguments are the query's parameters), with five signals (seven for an infinite query) and the
methods `refetch`, `invalidate`, `set_poll_interval` and `fetch_next_page`. In the core it is an object-table entry
whose object is a view of the query cache: an observer registered on the entry keyed by `(query_id, encoded
parameters)`, the observer's own polling interval, and a store cell with the signals
(`crates/undra-query/src/handle.rs`, `dispatch.rs:73-98`). The data is the cache's, not the handle's. The handle
says `transient() = true` (`handle.rs:410-413`), so `Runtime::snapshot` leaves it out and `Runtime::restore`, which
empties the object table and rebuilds the stores (`runtime.rs:3298`), makes its handle stale: status 5.

That is what every restore path does to it today:

| Path | What the app sees today |
|---|---|
| `undra dev` reload (ADR-053) | The Remote tab keeps its last values, `refetch` is refused (`UndraCallError.Refused`), the bar says `Reloaded, state kept (1 object not carried over)`. The app has to run the query again (re-mount the screen). The published post and the roadmap list it as open. |
| Devtools time travel (ADR-054) | The same, on the same runtime, and the client is not told which handles died. |
| Web crash restart (ADR-049) | Works, on the web only, by a second mechanism: generated TypeScript records each query handle's constructor call (`recreate`), and after a restart `recovery.ts` runs it again, gets a **new** handle, moves the mirror registration and rewrites the wrapper's `handle` (`_rebindObject`). The observer's polling interval is not replayed. |
| An app's own `core.restore(snapshot)` | Stale, on every platform. |

What a fix has to respect:

* **A handle is identity** (ADR-022, ADR-040). Generations are never issued twice; a restore re-issues a store under
  its **same** handle; each runtime keeps one wrapper per handle, and its identity map, its mirror registration, its
  observed set, its finalizer record and `requireOwn` are all keyed by the handle value. Swift and Kotlin wrappers
  hold the handle in a `let` / `val` that calls read without a lock.
* **A restore is all or nothing** (ADR-023), with one stated exception (ADR-037 decision 7: a store type the build no
  longer has is left out and reported).
* **Nobody tells the client.** A reload is, to a client, a reconnect followed by the answers to its own `Observe`s
  (ADR-051, ADR-053); a time travel is change-sets and a dev notice (ADR-054). All three runtimes observe **every
  handle they observed** again after a reconnect, without knowing what the handle is (`core.ts` `_reconnected`,
  `ConnectedCore.kt:769`, `UndraCore.swift:1559`), and the web's crash recovery does the same (`recovery.ts`
  `#reattach`).
* **R12.** A restore is a function of the state and the snapshot. A query constructor reads the `Clock` port and
  starts a fetch.
* A `Lazy<T>` page server (ADR-043) is transient too, and already comes back: a restore registers a new server for
  each lazy signal of each restored store, and the store's `LazyValue` (op 0), which the restore's re-observe and a
  client's `Observe` deliver, names it. The three lazy lists take a new handle by dropping their page cache and
  paging again. Nothing new is needed there; this ADR pins it with a test.

## Decision

**One mechanism, in the core. A snapshot keeps, for each query handle, a small record of what the handle is made
of. A restore re-issues the handle's value from the record, without building anything. The object is built again
when the host first uses the handle. A query handle that is alive in the runtime being restored is not touched at
all.** Hosts do nothing: the handle they hold keeps working.

### 1. Where re-creation lives

**1.1 Re-creatable objects (`undra-runtime`).** `UndraObjectDyn` gains

```rust
/// What a snapshot keeps of an object that is not snapshotted as a store but can be built again from what it
/// was made of. `None` (the default) for everything else: its handle is stale after a restore into another runtime.
fn recreation(&self) -> Option<Vec<u8>> { None }
```

and the runtime gains a registry of what builds such objects again, added at run time like an inspector or a stats
section (`Runtime::add_reviver`, one per name):

```rust
pub struct Reviver {
    pub name: &'static str,                 // identity, and what logs call it
    pub object_name: &'static str,          // what a dormant handle reports as its type name ("QueryHandle")
    /// Whether this reviver builds objects of `type_id`, and the fingerprint of what their record depends on.
    pub fingerprint: fn(&Runtime, u32) -> Option<u64>,
    /// Checks a record without building anything and without calling a port (restore: decision 1.3).
    pub check: fn(&Runtime, u32, &[u8]) -> Result<(), String>,
    /// Builds the object; the core lock is held and the runtime is current, as in a dispatched call.
    pub revive: fn(&Runtime, u32, &[u8]) -> Result<Arc<dyn AnyObject>, String>,
}
```

**All of the code below is reachable only through `add_reviver`** (a table of function pointers the first
`add_reviver` installs, the way the first store with a `Lazy` field installs the page dispatcher, ADR-052), so a core
that links no query runtime links none of it. Such a runtime handles a recreation record as a store type it does not
have (ADR-037 decision 7: left out, a WARN, `RestoreReport::dropped`), which is also what a runtime older than this ADR
does with one.

`undra-query` adds its reviver where it adds its stats section (`shared_of`, which the `HYDRATE` init hook reaches
when the runtime starts, before any restore; a core whose only queries are hand-written `QueryDef`s adds it at the
first use of the client). Its record is `format u16 = 1, params bytes, poll_ms Option<u64>`: the canonical encoding
of the query's parameters (the cache key's bytes, which are the constructor's arguments) and the interval this
observer set for itself (`set_poll_interval`), if any. Its fingerprint is `fnv1a64` of the canonical closure of the
query's parameters by name (`Schema::closure_of_params`, ADR-037's function for a mutation's input), `0` for a query
the schema does not describe, computed once per query id and cached. The record holds no time, no data and nothing
of the cache, and never a handle or a callback instance: a query's parameters are values (objects and callbacks are
refused there, ADR-040 decision 9). A query handle still answers `transient() = true`: it is not snapshotted as a
store.

**1.2 The snapshot (`Runtime::snapshot`).** After the stores, one **recreation record** for every object the host
holds a reference to whose `recreation()` is `Some`, and for every dormant handle (1.3): the record shape layout 2
already has for a store,

```
handle u64, type_id u32, signal_count u32 = 1, { signal_id u32 = 0xFFFF_FFFE, len u32, record bytes }
```

with the type listed once in the snapshot's type table as `(type_id, the reviver's fingerprint)`. `0xFFFF_FFFE` is
reserved (`undra_wire::payload::RECREATION_FIELD`; SPEC 1.1: no store has a signal with that id, as none has
`u32::MAX`), and a record is a recreation record when it has exactly that one field
(`StoreSnapshot::recreation() -> Option<&[u8]>`). The description (the stores' closures) does not mention these
types: `Schema::stores_closure` skips ids that are not stores, as it does today. `count` counts both kinds.

**1.3 The restore (`Runtime::restore_with_report`).** Three rules, in the existing phases.

*Phase 1 (build; nothing is touched).* A store record is handled as today. A recreation record is **checked, not
built**: the reviver that claims its type is found, its current fingerprint compared with the snapshot's, and
`check` run under the panic guard. A record that fails any of the three is **refused**: left out, its handle stale,
reported in `RestoreReport::refused: Vec<RefusedHandle { handle, type_id, reason }>` and a WARN
(`restore: Handle(..) (0x..) is not re-issued: <reason>`). It never fails the restore.

*Phase 2 (replace the table).*
(a) **A live re-creatable object stays exactly as it is**: its entry, its references, what was observed, its
observer in the cache, its polling timer, a fetch in flight, the pages of an infinite query. `ObjectTable::clear`
becomes `clear_keeping(keep)`; the entries kept are the ones whose `recreation()` is `Some` (dormant ones
included). One exception keeps all-or-nothing for stores: when a store of the snapshot needs the slot (the same
index under another generation: the store was released and its slot reused since the snapshot was taken), the store
wins and the object there is removed like any other (its handle is stale, and it is listed in
`RestoreReport::displaced`).
(b) The stores are placed, as today.
(c) Each accepted record whose handle is **not live** is placed at its handle as a **dormant** entry: an object
that holds the record and the reviver, reports the record's type id, is not a store, holds one host reference, and
whose `recreation()` is the record (so the next snapshot carries it on). A record whose handle is live is not
needed (the object is there). A record whose slot is taken by another kept handle is skipped (DEBUG): the slot
was reused, so the handle it names was released.
The generation counter is raised over the records' generations exactly as over the stores' (ADR-022; `insert_at`
does it). `RestoreReport::reissued` counts the handles of re-creatable objects that are valid afterwards (kept and
dormant).
(d) `cancel_calls_replaced_by_restore` sees a kept object as the same object before and after (its address is in
`before`), so nothing running on it is cancelled.

*Phase 3* is unchanged: a kept object's mirror is already current, and a dormant handle has nothing to say yet.

**1.4 First use builds.** Where a handle is resolved to its object for use, under the core lock: `Runtime::observe(
handle, _, true)`, and `Runtime::object` / `Runtime::param`, which every dispatcher (generated, or a layer's, as
`undra-query`'s `handle_method`) calls for its receiver and its object parameters. Routing needs no change: a dormant
handle reports the record's type id, so the call reaches the dispatcher it would reach. `object` and `param` look for a
dormant entry **only when the typed lookup misses** (the dormant object is not a `T`), so a call on a live object pays
nothing. If the entry is dormant, `revive` runs under the panic guard and its result replaces the dormant object **in
the same entry** (`ObjectTable::replace_object`: the handle, the references and the slot stay; a store's cell is told
its owner and its handle, and its lazy hooks run), then the use proceeds: an `Observe` is answered with the handle's
current values, a `refetch` refetches. If `revive` fails or panics, the entry is released, the handle is stale for
good, an ERROR names the handle, the reviver and the reason, `stats_json` counts it (`revive_failed`), and the use
that triggered it is answered as for any stale handle (status 5 with that reason; an `Observe` logs it).
`observe(.., false)` and `release` do **not** build: releasing a dormant handle forgets the record.

For a query handle `revive` is what its constructor does (`vt.open` with the recorded parameters: an observer on
the cache entry, a fetch if the entry is stale or missing by the rules of SPEC 9, the entry's current view shown),
then the recorded polling interval is set again. `check` decodes the record and the parameters, so a record that
passed the restore cannot fail to open.

**1.5 What the object table reports.** A dormant handle is a live entry: `live_handles` and `host_refs` count it
(the host owns the reference), `type_of` answers its type id and `object_name`. `stats_json` gains
`"dormant_handles"` and `"revive_failed"`.

### 2. What survives, exactly

"Same runtime" is a restore into the runtime that holds the handle (time travel, an app's `restore` on a live
core). "Fresh runtime" is a restore into a runtime that never held it (a dev reload, a web crash restart, a cold
start).

| | Same runtime | Fresh runtime |
|---|---|---|
| The handle value | unchanged | unchanged (re-issued from the record) |
| The host's wrapper, identity map, mirror registration, observed set | untouched (no host code runs) | untouched; the host observes again, as after any reconnect or restart |
| The observation (which signals are observed) | kept in the entry | what the host observes again |
| First change-set for the handle after the restore | **none** (the mirror is current; the restore sends nothing for it) | the answer to the host's `Observe`: the full value of each signal, from whatever the cache holds. A `persist` entry hydrated from `Kv` shows its data at once and **`stale` decides** whether a fetch starts; without a cached entry it is `data` absent (an empty list for an infinite query), `status = fetching`, and the fetch's result follows. This is exactly what constructing the handle answers: one rule. |
| The observer's own polling interval | kept | set again from the record |
| The query's polling (`interval`) | continues, the timer is not touched | starts with the first fetch after the handle is built |
| A fetch in flight | carries on (it belongs to the cache entry, which a restore does not touch; a handle's methods are synchronous, so there is no call of the handle for ADR-023 to cancel) | gone with the old process; the build starts one if the entry needs it |
| An infinite query's pages | all of them stay | the persisted pages (`persist_pages`) at once if the query persists, else the first page is fetched; further pages are loaded on demand (`fetchNextPage`, `loadMore`, `useLoadMore`), not fetched back to the old depth |
| A `Lazy<T>` page server | a new server per restored store, named by the store's op 0 from the restore's re-observe; the host drops its page cache and pages again (as today) | the same, from the host's `Observe` |
| Mutations in flight, the offline queue | unchanged: a mutation is a function call, which a restore does not cancel unless it holds a replaced store (ADR-023); the queue is persisted by its own rules (ADR-037) | unchanged: the queue is in `Kv`; a call in flight is lost with the process (ADR-053) |
| A handle the host released after the snapshot was taken | not live, so the record makes it dormant; nobody uses it, so it is never built: no observer, no fetch, no poll. It costs one inert table entry until the core closes (or the next dev reload, below). | the same; after a dev reload's restore the runner releases every re-issued handle the handed-over session does not hold |
| A handle created after the snapshot was taken | kept (it is not state) | does not exist |

### 3. Every restore path

| Path | Runtime | What the app observes | What is said | What is counted |
|---|---|---|---|---|
| Dev reload, same schema | fresh process; the kept session's handle list is handed to the new core unchanged (ADR-053) | Stores as today. Each query screen shows its handle's values again after the reconnect's `Observe` (loading, then data fetched by the **new** code; at once for a persisted query); `refetch` is accepted; polling runs. No app code. | bar: `Reloaded, state kept`; terminal: `state kept (2 stores, 1 query handle, 1 KiB, ..)` | `(N objects not carried over)` now counts only what is still stale: plain objects, objects a method returned, refused records |
| Dev reload, a migratable schema change (ADR-037) | fresh process, `restore_with_report` migrates the stores | A record whose query still exists with the same parameter types is re-issued; one whose parameters changed, or whose query is gone, is refused. (A client built from the old bindings is refused at its `Hello` regardless, R7.) | as above, `(the schema changed; 1 object not carried over)` | refused records are among the objects not carried over |
| Devtools time travel | same runtime | Stores go to the step's values. Query handles are untouched: no change-set, no refetch, polling continues, also for a handle created after the step. | bar: `time travel: step N` (and the existing count of stores built since) | a displaced handle (1.3a) is added to that count |
| Web crash restart | fresh wasm instance, the kept snapshot | Stores as today; query handles come back **on their own handle** when `#reattach` observes them again. The TypeScript replay is gone. A handle created after the last kept snapshot is stale, as a store created then is lost. | `onCoreRestarted` / `UndraCoreRestarted` as today | `staleObjects` as today (a query handle in the snapshot is not stale) |
| An app's `core.restore(snapshot)` on a live core | same runtime | as time travel | nothing | `RestoreReport` for Rust callers |
| A cold start that restores a saved snapshot | fresh runtime | A handle the app adopts and observes is built then; one it never uses stays dormant and does nothing | nothing | `RestoreReport` |

### 4. The wire and the ABI (R7)

* **No layout change.** A snapshot with recreation records is a layout-2 snapshot: the same words, the same record
  shape, one reserved field id. The envelope, every payload, both ABIs and the schema hash are unchanged, and
  `undra_restore` returns the codes it returned (a refused record is not an error).
* **Old snapshots** (no such record) restore as before.
* **A new snapshot in an old runtime** restores: the old runtime finds no restorer for the record's type and takes
  ADR-037's exception, exactly as for a store type it does not have. The stores are restored, the handle is stale,
  a WARN says so. Prototype P1 ran this against `main` unmodified. That is the rollback case (an app update
  reverted, a backup restored into an older build): it loses query handles, which that build never kept, and
  nothing else.
* The platform `Snapshot` codecs (tests only) decode the record as they decode a store's; a wire vector
  `snapshot_recreation` pins it in all four codecs with no codec change.
* Generated TypeScript changes shape (the `recreate` option and the `args` constructor parameter of query handles
  go): goldens move, and that is this ADR (R3, R11).

### 5. Determinism and containment (R12, R6)

`snapshot` and `restore` read no port and start nothing: `fingerprint` reads the schema, `check` decodes bytes.
The fetch, the `Clock` read for staleness and the `Timer` for polling happen in `revive`, inside the host's own
`Observe` or call: the same entry, the same ports and the same rules as the constructor call the host made the first
time. A restore followed by the same host calls is as reproducible as a fresh start followed by them. `check` and
`revive` run under the panic guard; a panic in `check` refuses the record, one in `revive` makes the handle stale
(1.4). On wasm a panic traps as anywhere else.

**Why the exception to all-or-nothing is right here.** ADR-023's rule exists because a half-restored app has
handles that point at nothing and stores that disagree. A refused record changes no store and leaves one handle
stale, which is today's behaviour for every query handle; failing the whole restore over it would turn "one query
was removed by the rebuild" into "state reset", the trade ADR-053 already refused for plain objects. It is ADR-037
decision 7 applied to the same situation (the build no longer has the thing), reported the same way.

### 6. Scope

* **In (v1.x):** the framework's own handles: query handles and infinite-query handles (every `#[undra::query]`
  and hand-written `QueryDef` whose reviver is registered), on every platform and every restore path; `Lazy<T>` page
  servers (already carried; pinned by tests here).
* **Deferred: user objects.** No `#[undra::api(recreate)]`. The seam is general (`recreation`, `Reviver`), but a
  user object that is re-built from its constructor arguments silently loses whatever its methods changed since,
  re-runs its constructor's side effects, and cannot take objects or callbacks as arguments (their handles and
  instances are the host's). The supported way to carry an object across a restore is to make it a store (a store
  without signals is a store; its `restore` hook rebuilds what it holds). Revisit with a use case.
* **Refused, with the reason:**
  * objects first issued by a **return** (ADR-040): there is no constructor call to record, the object is a
    function of its receiver's state; the host calls the method again (unchanged);
  * a hand-inserted `LazyList` / `insert_lazy_source`: its items are core memory outside a store; put the list in a
    store's `Lazy<T>`;
  * callback instances (ADR-041): the host's, unchanged;
  * calls, streams and tasks in flight: ADR-023 and ADR-053, unchanged;
  * **the query cache itself.** A snapshot does not carry cache data: a query without `persist` is one the
    developer chose not to write down, and a snapshot is something apps write down; it would also make every poll
    a new devtools step. So a non-persisted query shows "loading" once after a dev reload. Keeping the rows on
    screen across a reload ("warm reload") is a separate, dev-only hand-over beside the snapshot, left to its own
    decision (the record, decision 4).

## Alternatives considered

| Alternative | Why not |
|---|---|
| **(a) Host-side replay on every platform** (what the web has: record the constructor call per wrapper, run it again after a restore, move the wrapper to the new handle) | Four implementations (TypeScript, Kotlin, Swift, the React Native layer) and nothing for a raw client. The handle changes under a live wrapper: `let` / `val` handles become locked or atomic state on the call path, and the identity map, mirror registration, observed set, finalizer record and `requireOwn` all have to move with it. It cannot work for time travel at all without a new message telling the client which handles died (a wire change). The existing web replay also loses the observer's polling interval. |
| **(b1) Core-side, built eagerly at restore** | A restore would call ports (`Clock`, a fetch) and start timers: R12, and a fetch for every handle the host has since released (a same-runtime restore cannot tell a released handle from one it must rebuild: generations of a fresh process and of an old snapshot overlap). On a dev reload the fetches would run before any client is attached, with the client's ports missing. Building at first use lets the host's own behaviour decide, exactly. |
| **(b2) Core-side, re-issue at restore, build at first use** | **Chosen.** |
| **(c) Query handles are not handles: the host addresses a query by key and the core resolves it on each call** | Change-sets address `(store handle, signal)`, and the observer needs an identity for its own polling interval and for the observer count: a new addressing mode on the wire and in four mirrors, to solve what re-issuing the handle solves with none. |
| **A "layout 3" snapshot: a section after the store records, behind a layout tag** | The snapshot has no tag; a trailer is refused whole by every runtime before this ADR (`Reader::finish`), so a rollback to an older build would lose **all** restored state whenever a query screen was open at snapshot time (P1 shows both behaviours). It also changes four codecs and their vectors. The in-band record costs one reserved id and degrades to today's behaviour. |
| **Hand the records over beside the snapshot** (a second blob: `Runtime::handles()`, a new ABI entry, another field on the runner's pipe) | An ABI change, and every caller of `snapshot`/`restore` (three runtimes, crash recovery, the hub, apps) would have to carry two things in step. |
| **Drop a live handle on a same-runtime restore and re-issue it from the record** (as stores are rebuilt) | It would unregister and re-register the observer (cancelling a fetch in flight and its poll timer), make a query without a stale window refetch on every time travel, lose pages beyond the first of an infinite query, and drop handles created after the snapshot. A query handle has no state for a restore to put back. |
| **Drop handles created after the step on time travel, as stores are** | A store built since has no values in the snapshot; a query handle needs none. Dropping it would break a screen for nothing. |
| **Carry the cache entry's data in the record** | See scope: `persist = false` data in bytes apps persist, larger snapshots once a second under crash recovery, and a devtools step per poll. |
| **`#[undra::api(recreate)]` for user objects now** | See scope. |
| **Keep the TypeScript replay next to the core mechanism** | Two mechanisms for one job, and the web would be the one platform whose query handles change value. Both orders of landing work (P4), so it can go in the same piece. |

## Consequences

* After an `undra dev` reload, a time travel, a web crash restart and an app's own restore, a query handle the host
  holds keeps working with no app code and no runtime code: the same wrapper, the same handle. The Remote tab
  refetches by itself after a reload, with the code that was just rebuilt.
* The TypeScript runtime loses its replay: `StoreOptions.recreate`, `RecreateCall`, `_rebindObject`,
  `UndraCore._recreatable`, `CrashRecovery.track` and the re-creation loop of `#reattach`. `UndraObject.handle`
  never changes again. Measured on the prototype: the hello-world runtime loaded up front goes from 22,100 to
  22,067 bytes gzipped (33 bytes; it has no queries), an app that imports `crashRecovery()` loses 1,979 bytes
  (about 530 gzipped), and each generated query handle class loses its seven-line `recreate` block and its `args`
  parameter.
* A snapshot grows by about 40 bytes plus the encoded parameters per live query handle. It names the parameters
  the app queried with (as it names store values); it holds no query data.
* A handle nobody uses after a restore costs a table entry and nothing else, and stays until the core closes
  (`undra dev` releases the ones no session holds at each reload). That is better than today's equivalent for
  stores (a store the host released after the snapshot is resurrected whole), and it is counted
  (`dormant_handles`).
* New public Rust surface: `UndraObjectDyn::recreation`, `Reviver`, `Runtime::add_reviver`,
  `RestoreReport::{reissued, refused, displaced}`, `RefusedHandle`, `undra_wire::payload::RECREATION_FIELD`,
  `StoreSnapshot::recreation`. `stats_json` gains two numbers.
* `live_handles` after a restore into a fresh runtime includes dormant handles. S15 step 8 ("no extra handles")
  holds: nothing is created that the snapshot did not name.
* ADR-053's dev notice changes meaning slightly: its count no longer includes query handles.
* The first change-set after a reload for a non-persisted query is "loading" (scope). Stated in `DEV_LOOP.md`.

## Risks

* **The hello-world wasm gate (ADR-052, 120,000 bytes gzipped).** `snapshot`, `restore`, `observe` and `object`
  are in every core. Measured with `scripts/wasm-size.sh` on one machine and toolchain: `main` 116,416; the prototype
  with the recreation code inline 119,910 (+3,494: 90 bytes under the gate); the prototype with that code behind the
  table `add_reviver` installs (decision 1.1) 117,718 (+1,302). The requirement "reachable only through
  `add_reviver`" is therefore part of the decision, not an optimisation; the piece re-measures, keeps new collection
  types off the paths that stay (the existing restore already avoids a second map type for this reason), and
  re-records the size in the same commit.
* **A path that resolves a handle without going through `Runtime::object`, `param` or `observe`.** A dispatcher
  that reads the table directly (`rt.objects().get`) would see a dormant handle as a wrong type. Generated code and
  `undra-query` do not; `ObjectTable::get`'s docs say so, and a test enumerates the runtime's entry points
  (call, call_sync, stream, observe, release, a `LazyPage` call) against a dormant handle.
* **Order between restore and `add_reviver`.** A record restored before its reviver exists is refused. The
  `HYDRATE` init hook runs when the runtime starts, before any restore; a core with only hand-written `QueryDef`s
  adds the reviver at the client's first use. Documented on `QueryRegistration`; tested both ways.
* **Snapshot cost.** `recreation()` and the fingerprint lookup run per handle per snapshot, and crash recovery
  snapshots up to once a second: the fingerprint is cached per query id, and two bench rows gate it.
* **A displaced handle** (1.3a) goes stale on a time travel across a slot reuse. It needs a store released and its
  slot reused by a query handle between the step and now; it is reported.
* **Parallel pieces.** `ts-runtime-16k` edits `core.ts` and `object.ts`; `cold-restore-regression` may edit
  `Runtime::restore`. See Dependencies.

## What the prototype proved

On the local branch `proto/reload-handles` (head `364ccb3`: three `proto:` commits on `main` at `a309e9f`; never
pushed, not for merging):

* **P1, the in-band record against `main` unmodified** (`crates/undra-runtime/tests/proto_inband.rs`): a snapshot
  with a record of an unknown type under the reserved id restores, the store comes back, the record is reported in
  `RestoreReport::dropped` and its handle is stale; the same snapshot with four bytes appended (a trailer) is
  refused whole. This decided "in-band" over "layout 3".
* **P2, the mechanism** (`crates/undra-query/tests/proto_reload.rs`, a platform's raw calls): a fresh runtime
  re-issues the handle, makes no HTTP call until the `Observe`, then answers it, fetches, accepts `refetch` and
  polls at the observer's recorded interval; the same runtime keeps a live handle through three restores
  (two of a snapshot with it, one of a snapshot from before it existed) with no change-set, no fetch and the same
  `live_handles`; a handle released after the snapshot stays dormant through two minutes of fake time, is carried
  into the next snapshot and is forgotten by `release` without ever being built; an unknown query and a record that
  does not decode are refused and counted while the good one is re-issued, and a `refetch` on that one, never
  observed, builds it (the miss path of `Runtime::object`). The rest of `undra-runtime`,
  `undra-query`, `undra-wire` and `undra-transport` passes unchanged except the one test that pins the old
  behaviour (`wire.rs` `a_snapshot_leaves_handles_out_and_restore_still_works`).
* **P3, the Remote tab case end to end** (`crates/undra-cli/tests/dev_reload.rs`,
  `proto_a_query_handle_keeps_working_across_a_rebuild`): the real `undra dev` on a copy of the playground, a raw
  client holding the polling `ticker` query's handle, an edit, the rebuild; the client reconnects with its token and
  observes; the same handle answers, `refetch` is status 0, the new process's ticks arrive (`[1, 2, 3]`), the
  notice is `Reloaded, state kept`. The runner template was **not** changed: the session keeps the handle because
  the record is a record. The nine existing reload tests and the three devtools tests pass.
* **P4, the platforms.** TypeScript (`contract-tests/ts`, the real wasm core): with the runtime unchanged all 33
  scenarios pass against the prototype core (the old replay still works beside it, so the core can land first);
  with the replay removed (102 lines deleted from `object.ts`, `core.ts`, `recovery.ts`) they pass again, S22 with
  the assertion turned around: the query handle after the trap **is** the handle before it. The same-runtime half of
  S35 through the generated bindings (`test/proto-s35.test.ts`): across `core.restore` the same wrappers, nothing
  delivered for the query handles, no request, the feed's 100 rows stay, `refetch` accepted, the ticker keeps
  ticking, the third page loads, a second restore changes nothing. Kotlin (JNI) and Swift (the C ABI table): the 31
  scenarios of each column pass against the prototype core with no runtime change.
* **Size**: the numbers under Risks and Consequences (`scripts/wasm-size.sh`, three builds on one machine).
* It killed nothing outright, but it showed two things the brief below fixes: the dev runner counts records as
  stores (`state kept (2 stores` for one store and one handle), and the devtools hub's `stores_of` would list
  records as stores.

## Implementation brief

In this order; each step leaves the workspace green. Names and shapes are in the Decision; the prototype branch is
a working reference for steps 1 to 3 and 7, not code to merge (its tests are named `proto_*` / `p1_` / `p2_`, its
doc comments say PROTOTYPE, and it skips what is marked "final" here).

1. **`undra-wire`** (`payload/snapshot.rs`, `payload/mod.rs`): `RECREATION_FIELD`, `StoreSnapshot::recreation()`,
   the `Snapshot` docs (a record is a store's or a recreation record; `count` counts both); unit tests; a wire
   vector `snapshot_recreation` in `contract-tests/wire-vectors.json`, read by the four codecs' existing vector
   tests (no codec change; SPEC 3 lists the vector).
2. **`undra-runtime`**:
   * `object.rs`: `recreation()`, `Reviver`, the dormant object (crate-private); `lib.rs` exports.
   * `object_table.rs`: `clear_keeping`, `replace_object`, an iterator of the entries the host holds; unit tests
     (kept entries keep references and observation; a kept slot is not on the free list; `replace_object` moves
     the address index and the store index and runs the lazy hooks).
   * `runtime.rs`: `add_reviver`; `snapshot` (1.2; fingerprints cached per type id like `store_fingerprints`);
     `restore_with_report` (1.3, including the slot rule, `displaced`, a record skipped because its slot was
     reused, and kept objects in `before`); `revive` in `observe`, `object` and `param` (1.4; **final:** not on
     `observe(.., false)`; on the miss path only in `object` / `param`; taking the core lock when the caller does not
     hold it, as `snapshot` does); the table of function pointers `add_reviver` installs (1.1); `stats_json`.
   * `config.rs`: `RestoreReport::{reissued, refused, displaced}`, `RefusedHandle`.
   * Tests (`tests/recreation.rs`, a test reviver over a layer object as `tests/layers.rs` builds one): every row
     of the table in section 2 that does not need a query; `check` panics (refused); `revive` fails and panics
     (stale, ERROR, `revive_failed`, everything else kept); a displaced handle; a forged record (a duplicate
     handle, a generation at the ceiling: the existing `BadHandle` refusals); calls in flight on a kept object are
     not cancelled; a snapshot taken inside a change-set callback of a query handle's commit (no lock is held
     across `recreation()`); a runtime with no reviver (the record is `dropped`, as on an old runtime: P1 without
     the `proto` name); **property test:** for arbitrary interleavings of construct, observe, release, snapshot and
     restore over stores, live handles and dormant handles, `restore(s); restore(s)` leaves the same table,
     observations and `stats_json` as `restore(s)`.
   * Bench (R9): `snapshot/encode_100_handles` and `snapshot/restore_100_handles` with budgets in
     `bench/budgets.toml`; the existing snapshot rows unchanged within tolerance; `scripts/wasm-size.sh` within
     the gate (Risks).
3. **`undra-query`**: the record (`format u16 = 1`) and its reader; `HandleOps::recreation` for both handle kinds;
   `HandleObject::recreation`; the reviver and its registration in `shared_of`; a `check` that decodes the
   parameters (a `QueryVTable` function); the cached parameter fingerprint; `lib.rs` and `handle.rs` docs (they say
   the platform re-creates handles). Tests (`tests/restore.rs`): P2's four cases without the `proto` names; an
   infinite query (same runtime: all pages stay; fresh: persisted pages at once, else the first page, and
   `fetch_next_page` works); a persisted entry shown at once with `stale` deciding the fetch; a parameter type
   changed between two schemas (refused); a hand-written `QueryDef` before and after the client's first use;
   `wire.rs`'s old test replaced.
4. **`undra-transport`**: the devtools hub's `stores_of` leaves recreation records out (they are not stores to
   observe or list, and observing one would build it); the count of stores built since a step ignores them and adds
   `displaced`; `tests/devtools.rs`: a time travel with a live query handle (kept, still delivering, the page's
   store list unchanged). No change to sessions: a kept session's handle list already names the handle.
5. **`undra-cli`**: the runner template counts stores, handles and refused records apart
   (`restore_state`: `report.restored`, `report.reissued`, `report.refused` and `displaced` removed from the
   session's list as `dropped` is; a re-issued handle the handed-over session does not hold, every one of them
   when no session was handed over, is released at once, so a dev session does not carry inert handles from
   reload to reload; `take_snapshot`: stores counted by decoding, not from the leading word, and
   any other reader of that word in the workspace likewise); the
   `restored` line and `reload.rs`'s `Outcome::describe` gain the handle count (`2 stores, 1 query handle`);
   `tests/dev_reload.rs`: P3 without the `proto` name, plus a `Library` (a `Lazy<T>` store) paged before and after
   the reload, plus the schema-change case (a query whose parameter type the edit changes is counted in the
   notice); `tests/dev_devtools.rs`: time travel with the ticker's handle; `tests/runner.rs` units for the line.
6. **`undra-ffi`**: nothing to change; a test that `undra_restore` answers 0 for a snapshot with a refused record.
7. **TypeScript runtime and `undra-bindgen`** (the removal): `object.ts` (`recreate`, `RecreateCall`,
   `_rebindObject`, the `handle` doc), `core.ts` (`_recreatable`), `recovery.ts` (`track`, `#recreatable`, the
   loop in `#reattach`, the doc comments), `index.ts` exports, `test/recovery.test.ts` (its two replay tests,
   which fail on the prototype, become "a query handle in the snapshot is observed again on its handle");
   `crates/undra-bindgen/src/ts.rs` (`object_with`'s `recreatable` path and the `args` constructor parameter), the fixture
   `tests/fixtures/ts-base/index.d.ts`, the goldens under `tests/golden/*/ts/src/queries.ts` (decimal, full,
   polling, queries, stdlib, infinite), and the generated examples (`examples/playground`, `examples/cookbook`,
   `examples/two-cores/{a,b}`; `undra bindgen --check`). Re-record `web/hello-runtime-js` with
   `scripts/wasm-size.sh --record`.
8. **Swift, Kotlin, React Native**: no runtime or generated change (their sources say nothing about query handles
   and restores today: checked). The site's API pages regenerate with step 11.
9. **Contract scenario S35, "query handles across a restore"** (`contract-tests/scenarios.md`, the three runners,
   `check.sh`, `run-all.sh`, the README's counts, and the React Native column's include list in
   `runtimes/rn/@undra/react-native/vitest.contract.config.ts`). Through the generated bindings unless a step says
   raw:
   1. `RemoteTodosQueryHandle.create("s35")` shows the server's one item; `TickerQueryHandle` observed;
      `FeedQueryHandle(evenOnly: false)` with two pages loaded; `Library()` with `books[0]` read; a `Counter` with
      `add(5)`; a `Probe`. (The observer's own polling interval is pinned under the fake clock in step 3's tests,
      not here: real timers would make it slow or flaky.)
   2. `snapshot`; `Counter.add(10)`; the server now answers two items; `restore(snapshot)`.
   3. **Same wrappers, nothing blinked:** the counter shows 5; the remote handle's `data` never became absent and
      the GET count did not move (record the values the mirror applied during the restore); the feed still has
      100 rows; `live_handles` is what it was before the restore.
   4. `refetch()` on the same remote wrapper is accepted: GET count +1, two items.
   5. Polling continues: the ticker advances within 2.5 s with no call from the runner.
   6. `fetchNextPage()` on the same feed wrapper gives 150 rows; `books[0]` is readable (a page call succeeds).
   7. The `Probe` is refused (unchanged: it is not re-creatable).
   8. `restore(snapshot)` again changes nothing more (idempotent: the same checks as step 3).
   9. Closing the wrappers returns `live_handles` to its value before step 1.
   10. **A fresh runtime** (TypeScript: a second core of build B loaded in the process; Swift and Kotlin: the
       second process S14 and S15 already run with build B, the snapshot handed over in their file; raw API with
       known ids, as those steps do). Build B changes the parameter type of one query declared for this step in
       `examples/playground/core/src/updates.rs` and leaves `remote_todos` and `ticker` alone (the playground's
       schema hash and generated bindings move once for the new query). `restore(snapshot)`
       succeeds; `stats().liveHandles` counts the re-issued handles; no GET was made yet; after
       `configure_remote` (core state outside stores, as S22 says) `observe(remote handle)` is answered with
       `status = fetching` and then the data; `call(remote handle, refetch)` is status 0; the ticker's handle
       delivers ticks; the `Library`'s `books` pages through the server its op 0 names; the handle of the changed
       query is refused (status 5) and the Log port received the WARN that names it.
   S15 step 9 stays (a plain object is stale); S15 step 8 is reworded (`live_handles` after the restore is the
   surviving stores plus the query handles that were live). **S22 step 4** is reworded: the query handle after
   the restart is the handle before it.
10. **SPEC**: 1.1 (the reserved id), 5.4 (dormant entries, `recreation`, `transient` now means "not snapshotted
    as a store"), 5.9 (the record, the restore's rules, the dev reload paragraph), 5.10 (time travel and the notice
    count), 9 (a query handle across a restore; the table of section 2 in prose), 10.3 and 17.1 (TypeScript: no
    `recreate`; recovery step 5 goes; `UndraObject.handle`), 16.2 (`Reviver`, `add_reviver`, `RestoreReport`),
    14 (S35).
11. **Docs and the site**: `docs/DEV_LOOP.md` ("What carries over": query handles do, non-persisted queries show
    loading once; the troubleshooting row for `refetch` refused goes; time travel), `docs/ERRORS.md:234`,
    `README.md:233` (the "not done" line goes), `examples/playground/README.md:85`, `site/data/roadmap.json`
    ("Query handles across a dev reload" moves to shipped, with this ADR), the post's cell
    (`site/blog/why-undra-is-the-default-choice/index.html`, the "Not yet: query handles across a reload" of the dev
    loop row and the list item) and its `claims.md` rows M06-N5 and O35, then `node site/scripts/build-all.mjs`
    (`llms*.txt`, the search index, the roadmap page) and `check-links.mjs --words`.
12. **The older ADRs**: a dated note under ADR-049 (decision 3.4.5 superseded), ADR-053 (the table row, decision 2,
    known limit (c)) and ADR-054 (decision 4) pointing here.

Evidence required with the piece (R4, R10): the Rust suites, the contract grid with S35 on three columns and the
React Native model, the device proof ADR-053 asked for repeated on the Remote tab (the playground on the iOS
simulator and the `undra` AVD: open Remote, edit the Rust, the bar says `Reloaded, state kept`, the list reloads by
itself and pull-to-refresh works), and the two size gates.

## Dependencies

Builds on ADR-022 (handles are re-issued by value, generations only rise), ADR-023 and ADR-037 (the restore's
phases, all-or-nothing and its exception, fingerprints of parameter closures), ADR-040 (host references, one
wrapper per handle), ADR-043 (infinite queries, polling per observer, lazy page servers), ADR-051 and ADR-053 (the
kept session and its handle list; the client's re-observe), ADR-054 (time travel on the same runtime). Supersedes
ADR-049 decision 3.4.5.

Order with pieces in flight: steps 1 to 6 are independent of `ts-runtime-16k`; step 7 edits `core.ts` and
`object.ts`, which that piece also edits, so whichever lands second cross-merges (the removal only deletes).
`cold-restore-regression` should land before step 2 or cross-merge it (`Runtime::restore`). Nothing here waits for
an ABI or wire revision.

## Implementation note (2026-10-02, `wt/reload-handles`)

Implemented as decided: the integrator's nine decisions are the Decision section as written (the in-band record, build at
first use, same-runtime restores leave live handles alone, no cache hand-over, user objects deferred, the TypeScript replay
removed, the reviver reachable only through `add_reviver`, the wording, S35 in ten steps). The record is
`.10x/decisions/sde/reload-handles.md`. What differs from the text above, and why:

1. **A failed build answers the plain stale-handle refusal.** Decision 1.4 says the use that triggered a failed build is
   "answered as for any stale handle (status 5 with that reason)". `BadHandle` is `Copy` and its reasons are an enum; a
   variant that carries a string would change a public type, and one without it adds nothing a stale handle does not say.
   The reply is status 5 with the ordinary `stale handle ..` text; the reason (handle, reviver, why) is the ERROR record and
   `stats_json`'s `revive_failed`.
2. **A restore registers a store's page servers after it has placed everything the snapshot names** (`ObjectTable::
   insert_at_deferring_lazy`, `enter_lazy`). A page server takes the lowest free slot; with recreation records placed after
   the stores, the servers of the `Library` store took the slot of the ticker's handle and the record was skipped as
   "slot reused" (found by the dev reload test of the Remote tab). The same hazard existed between two stores and is
   closed too. No wire, ABI or generated change.
3. **`Runtime::dormant_handles` is not public**; the count is `stats_json`'s `dormant_handles` (the ADR lists no such
   method). `RestoreReport::displaced` is `Vec<u64>` (handle values) and `RefusedHandle` is `{ handle: u64, type_id: u32,
   reason: String }`; the refusal wording is `this build has nothing that builds it again`, `the types it was made from
   changed`, or the reviver's own reason; the WARN is `restore: Handle(index=<i>, gen=<g>) (0x..) is not re-issued:
   <reason>`.
4. **The runner protocol gained a field.** `snapshot ok .. <stores> <query handles> <bytes> ..` and `restored <stores>
   <query handles> <lost objects> <bytes> <microseconds>`; a core whose only state is query handles still has something to
   carry (`NothingToKeep` needs neither stores nor handles). The terminal omits the handle part when there is none.
5. **S35 has the `RosterQueryHandle(7)` of the changed query in step 1** (needed by step 10; `roster` is a new query of
   `examples/playground/core/src/updates.rs`, so the playground's schema hash and the bindings of the playground and the two
   two-cores apps moved once, and the testkit fixtures' recorded hash with them), step 3's `live_handles` is the reading
   before the restore **less the `Probe`** (the one object the restore makes stale), and step 10 runs on a **new core** of
   build B in the build-B process of Swift and Kotlin (S14's and S15's build-B steps leave stores in the first). The site's
   `build-trust.mjs` accepts a gap in the scenario numbers (S34 is held by `generics-fn-obj`).
6. **Devtools.** A step is also taken when a query handle appears or goes (the snapshot's bytes differ although no store
   changed); history is only longer. `stores_of` leaves recreation records out as decided; the count of stores built since a
   step adds `displaced`.
7. **The fingerprint of a query the schema does not describe** (a hand-written `QueryDef`) is `0` in both builds, so only a
   record that no longer decodes is refused for it; the reviver of a core with only such queries exists from the client's
   first use, and a snapshot restored before that is the "store type this build does not have" case (`QueryRegistration`'s
   docs, tested both ways).
8. **Sizes** (`scripts/wasm-size.sh`, this machine): the hello-world wasm 117,750 bytes gzipped against a record of 116,690
   (+1,060, under the +1.3 KB the decision allows and the 120,000 gate); the up-front JS 22,100 -> 22,067. The TypeScript
   replay's removal measured on `crashRecovery()` as 82,006 raw / 25,838 gzipped bytes against 83,389 / 26,209 (-1,383 /
   -371; the Consequences' 1,979 / 530 were the unminified source). `snapshot/encode_100_handles` 15.6 us,
   `snapshot/restore_100_handles` 19.6 us (fresh runtime), `snapshot/restore_100_handles_live` 38.3 us (the same runtime).
9. **A wrongly typed use builds.** `Runtime::object::<T>` of a dormant handle builds the object before it can tell the type
   is wrong for `T` (a `LazyPage` call aimed at a query handle, say): the handle is the host's own, the cost is one build,
   and the answer is the ordinary wrong-type refusal.
