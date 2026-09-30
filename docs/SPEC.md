# Keel v1 — Implementation Specification

This is the binding technical specification for Keel v1. Every crate, runtime and generated file conforms to it. When code and this document disagree, the code is wrong until an ADR changes the document.

The product design lives in the blueprint (`docs/BLUEPRINT.md`); this file is the engineering contract that lets independent workers build the pieces in parallel and have them fit.

---

## 0. Scope of v1

In scope: everything under the pixels.

* Rust core crates: `keel-meta`, `keel-wire`, `keel-macros`, `keel-signals`, `keel-runtime`, `keel-ports`, `keel-query`, `keel-ffi`, `keel-transport`, `keel-bindgen`, `keel-cli`, `keel` (facade).
* Platform runtimes: Swift (`runtimes/swift/KeelRuntime`), Kotlin (`runtimes/kotlin/keel-runtime`), TypeScript (`runtimes/ts/@keel/runtime`).
* Generated bindings for records, enums, errors, objects, stores, ports, sync/async methods, streams, signals.
* Reactive state: signals, computed, transactions, change-sets, keyed list patches, observation.
* Data layer: query cache, mutations with optimistic patches and rollback, invalidation, retry, persistence, offline queue.
* Ports: Http, Kv, SecureStore, Fs, Clock, Rng, Log, Timer, Connectivity, Lifecycle. Default adapters on each platform and Rust fakes.
* Dev loop: `keel dev` remote core over WebSocket; devtools protocol messages (inspector UI is a stretch goal).
* Playground app on all three platforms, benchmarks, contract tests.

Out of scope for v1: sync engine, hosted services, desktop targets beyond macOS-via-Swift, shared UI of any kind, Rust-owned SQLite (Kv is a foreign port in v1, see ADR-014).

Toolchain baseline: Rust 1.85+ (edition 2024), Swift 6.0 / iOS 17+, Kotlin 2.0 / Android API 26+ (NDK r27, 16 KB pages), TypeScript 5.5 / ES2022, Node 20+.

---

## 1. Core concepts and identifiers

| Concept | Definition |
|---|---|
| **Record** | A `struct` with `#[keel::api]`. Crosses by value. |
| **Enum** | An `enum` with `#[keel::api]`. Unit variants or data variants. Crosses by value. |
| **Error** | An `enum` with `#[keel::error]`. Like an enum, plus `std::error::Error` + `Display`. |
| **Object** | A type whose `impl` block has `#[keel::api]`. Crosses by handle. Methods are sync or async. |
| **Store** | An object whose struct has `#[keel::store]`. Has signal fields the platforms mirror. |
| **Port** | A trait with `#[keel::port]`. Implemented by the platform (foreign) or by a Rust fake. |
| **Query / Mutation** | An `async fn` with `#[keel::query]` / `#[keel::mutation]`. Managed by `keel-query`. |
| **Function** | A free `fn` with `#[keel::api]`. Crosses like a method with no receiver. |

### 1.1 Stable identifiers

All identifiers are computed at compile time by the macros and embedded in the schema, so every platform agrees without a registry lookup.

* `fnv1a32(s)` / `fnv1a64(s)`: FNV-1a over the UTF-8 bytes of `s`, offset basis `0x811c9dc5` / `0xcbf29ce484222325`, prime `0x01000193` / `0x100000001b3`.
* **type_id** (`u32`) = `fnv1a32("<TypeName>")` where `TypeName` is the Rust identifier, no module path. Type names must be unique within a core crate; the macro cannot check this, `keel-bindgen` does and fails on collision.
* **method_id** (`u32`) = `fnv1a32("<TypeName>.<method_name>")`; for free functions `fnv1a32("fn.<name>")`.
* **port_id** (`u32`) = `fnv1a32("port.<TraitName>")`; port method ids = `fnv1a32("<TraitName>.<method>")`.
* **signal_id** (`u32`) = zero-based index of the signal field in declaration order within the store struct (non-signal fields are skipped). `u32::MAX` means "all signals of the store".
* **query_id** (`u32`) = `fnv1a32("query.<fn_name>")`; mutation ids `fnv1a32("mutation.<fn_name>")`.
* **schema_hash** (`u64`) = `fnv1a64(canonical_schema_json)`, see §2.3.

### 1.2 Handles

A handle is a `u64`: low 32 bits = slot index, high 32 bits = generation (starts at 1, never `0`, never `u32::MAX` after the counter is spent). `0` is the null handle. Handles are issued by the runtime's object table (§5.4) and are only meaningful inside the runtime instance that issued them. Generations come from one monotonically increasing counter, so a `(slot, generation)` pair is never issued twice in a process (ADR-022).

### 1.3 Call ids

`call_id: u32` is chosen by the **foreign side** (monotonically increasing per runtime instance, wrapping allowed, `0` reserved). `port_call_id: u32` is chosen by the **core**. Both are unique among in-flight calls in their direction.

---

## 2. Schema (`keel-meta`)

`keel-meta` has zero dependencies except `serde` + `serde_json` (feature `serde`, on by default). It defines the data model below, JSON (de)serialization, the canonical form, the hash, and the FNV helpers. `keel-macros` emits it, `keel-bindgen` consumes it, `keel-ffi` exports it.

### 2.1 Type references

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "of", rename_all = "snake_case")]
pub enum TypeRef {
    Bool, I8, I16, I32, I64, U8, U16, U32, U64, F32, F64,
    String, Bytes, Unit, Duration, Timestamp, Uuid,
    Option(Box<TypeRef>),
    Vec(Box<TypeRef>),
    Map(Box<TypeRef>, Box<TypeRef>),
    Lazy(Box<TypeRef>),                    // lazy list handle, item type
    Named(String),                          // record, enum, error, object (by TypeName)
    Result(Box<TypeRef>, Box<TypeRef>),     // only as a return type
    Stream(Box<TypeRef>),                   // only as a return type
}
```

Serialized JSON examples: `{"kind":"string"}`, `{"kind":"option","of":{"kind":"named","of":"Todo"}}`, `{"kind":"map","of":[{"kind":"string"},{"kind":"i32"}]}`.

Rules enforced by the macro (error codes in §12): map keys must be `String`, integers, `Bool` or `Uuid`; `Result` and `Stream` only in return position; `Option<Option<T>>` rejected (Kotlin cannot express nested optionality: `Some(None)` and `None` would both arrive as `null`; E0063); `Lazy<T>` only as a store signal type. The error side of a `Result` is a `Named` `#[keel::error]` enum. A `Named` reference is checked against the type it resolves to at compile time (E0060, E0061, §12), so an alias or a type that shadows a built-in name cannot make the schema disagree with the wire.

### 2.2 Definitions

```rust
pub struct Schema {
    pub keel_version: String,          // "1.0.0"
    pub crate_name: String,            // Cargo package name of the core
    pub records: Vec<RecordDef>,
    pub enums: Vec<EnumDef>,           // includes errors (is_error = true)
    pub objects: Vec<ObjectDef>,       // includes stores (store = Some(..))
    pub functions: Vec<FunctionDef>,
    pub ports: Vec<PortDef>,
    pub queries: Vec<QueryDef>,
}

pub struct RecordDef { pub name: String, pub type_id: u32, pub fields: Vec<FieldDef>, pub docs: String }
pub struct FieldDef  { pub name: String, pub ty: TypeRef, pub default: bool /* #[keel(default)] */, pub docs: String }

pub struct EnumDef { pub name: String, pub type_id: u32, pub is_error: bool, pub variants: Vec<VariantDef>, pub docs: String }
pub struct VariantDef { pub name: String, pub index: u16, pub fields: Vec<FieldDef> /* empty = unit; named or tuple */, pub tuple: bool, pub message: Option<String> /* #[error("...")] */, pub docs: String }

pub struct ObjectDef {
    pub name: String, pub type_id: u32,
    pub constructors: Vec<MethodDef>,  // fns returning Self / Result<Self, E>
    pub methods: Vec<MethodDef>,
    pub store: Option<StoreDef>,
    pub docs: String,
}
pub struct MethodDef {
    pub name: String, pub method_id: u32,
    pub params: Vec<ParamDef>,         // excludes self and Ctx
    pub returns: TypeRef,              // Unit | T | Result<T,E> | Stream<T> | Result<Stream<T>,E>
    pub is_async: bool,
    pub takes_ctx: bool,               // first param is `ctx: Ctx` or `&Ctx` (constructors and free fns only)
    pub docs: String,
}
pub struct ParamDef { pub name: String, pub ty: TypeRef }

pub struct StoreDef { pub signals: Vec<SignalDef> }
pub struct SignalDef { pub name: String, pub signal_id: u32, pub ty: TypeRef, pub computed: bool, pub key: Option<String> /* #[keel(key = "id")] */ }

pub struct FunctionDef { pub name: String, pub method_id: u32, pub params: Vec<ParamDef>, pub returns: TypeRef, pub is_async: bool, pub takes_ctx: bool, pub docs: String }

pub struct PortDef { pub name: String, pub port_id: u32, pub kind: PortKind /* Sync | Async | Event */, pub methods: Vec<MethodDef>, pub docs: String }

pub struct QueryDef { pub name: String, pub query_id: u32, pub kind: QueryKind /* Query | Mutation */, pub key: String, pub params: Vec<ParamDef>, pub returns: TypeRef, pub stale_ms: Option<u64>, pub persist: bool, pub idempotent: bool }
```

### 2.3 Canonical JSON and the hash

Canonical form: `serde_json` with all `Vec`s sorted by `name` (variants keep declaration `index` and are sorted by index), map keys in struct-field order as declared above, no whitespace, `docs` fields **excluded**. `schema_hash = fnv1a64(canonical_bytes)`. `keel-meta` exposes `Schema::canonical_json()` and `Schema::hash()`. Two cores with the same public surface produce the same hash regardless of doc comments or source order of *unordered* things. Ordered (part of the wire layout, kept in declaration order): record fields, variant fields, params, signals (signal_id), variants (index). Unordered (sorted by name in canonical form): the six top-level lists, object methods and constructors, port methods. `crate_name` and `keel_version` are labels and are **excluded** from the canonical form (they do not change the wire).

### 2.4 Registration (`inventory`)

Each macro emits `inventory::submit! { keel_meta::Registration::Record(&RECORD_DEF) }` etc., where the def is a `static` built from `const` data (`&'static str`, `&'static [..]`). `keel_meta::Registration` is:

```rust
pub enum Registration { Record(&'static RecordMeta), Enum(&'static EnumMeta), Object(&'static ObjectMeta), Function(&'static FunctionMeta), Port(&'static PortMeta), Query(&'static QueryMeta) }
inventory::collect!(Registration);
pub fn collect_schema(crate_name: &str) -> Schema  // builds the owned Schema from all registrations
```

`*Meta` are `'static`, const-constructible mirrors of the `*Def` types (using `&'static [T]` instead of `Vec`) so they can live in statics. `keel-meta` provides `impl From<&RecordMeta> for RecordDef` etc.

On `wasm32-unknown-unknown` the TS runtime calls the exported `_initialize` (or `__wasm_call_ctors`) once after instantiation so `inventory` registrations run (see §7).

Dispatch registration uses the same mechanism: `Registration::Object` carries a `dispatch: fn(&Runtime, DispatchCall) -> DispatchResult` pointer and `Registration::Function` likewise (§5.6).

---

## 3. Wire format (`keel-wire`)

Little-endian throughout. No alignment, no padding. All lengths are `u32`. Encoders write into `Writer` (a `Vec<u8>` wrapper); decoders read from `Reader<'a>` (a `&'a [u8]` + cursor). Decoding never panics on malformed input; it returns `WireError`.

### 3.1 Value encoding

| Type | Encoding |
|---|---|
| `bool` | `u8` 0/1; decoder rejects other values |
| `i8..i64`, `u8..u64` | fixed width, two's complement, LE |
| `f32`, `f64` | IEEE 754 LE bits |
| `Unit` | nothing. `Unit` is legal only as a return type or a variant with no fields; `Vec<Unit>`, `Option<Unit>` and `Map<_, Unit>` are rejected (E0001) because zero-width items defeat length validation. |
| `String` | `u32` byte length + UTF-8 bytes (decoder validates UTF-8) |
| `Bytes` | `u32` length + raw bytes |
| `Option<T>` | `u8` tag 0 = None, 1 = Some + `T` |
| `Vec<T>` | `u32` count + items |
| `Map<K,V>` | `u32` count + (`K`,`V`) pairs; encoder sorts by encoded key bytes for determinism |
| `Duration` | `i64` nanoseconds |
| `Timestamp` | `i64` milliseconds since Unix epoch |
| `Uuid` | 16 raw bytes, big-endian as per RFC 4122 |
| Record | fields in declaration order |
| Enum / Error | `u16` variant index + variant fields in order |
| `Result<T,E>` | `u8` 0 = Ok + `T`, 1 = Err + `E` |
| Handle (object) | `u64` |
| `Lazy<T>` | `u64` handle of a lazy-list object |
| `Signal<T>` | never encoded as a value; appears only in change-sets |

Records with `#[keel(default)]` fields: the wire layout still contains the field; `default` affects only construction ergonomics in generated code and schema evolution rules (a field added with `default` bumps the schema hash like any other change; the *runtime* still refuses mismatched hashes. Evolution across hash mismatch is a v2 feature).

### 3.2 Envelope (transports only)

Used on WebSocket and Worker transports. In-process calls pass `kind` implicitly through the function they call and carry only the payload.

```
magic      4 bytes  "KEEL"
version    u16      1
schema     u64      schema_hash of the core that produced/expects this message
kind       u8       see table
seq        u32      per-direction monotonically increasing, for ordering and debugging
len        u32      payload length
payload    len bytes
```
Header size is 23 bytes.

| kind | name | direction | payload |
|---|---|---|---|
| 1 | Call | host→core | §3.3 |
| 2 | Reply | core→host | §3.4 |
| 3 | ChangeSet | core→host | §3.5 |
| 4 | PortCall | core→host | §3.6 |
| 5 | PortReply | host→core | §3.6 |
| 6 | Cancel | host→core | `call_id u32` |
| 7 | StreamCredit | host→core | `call_id u32, credit u32` |
| 8 | StreamItem | core→host | §3.7 |
| 9 | Observe | host→core | `handle u64, signal_id u32, on u8` |
| 10 | Release | host→core | `handle u64` |
| 11 | Event | host→core | `port_id u32, method_id u32, payload` |
| 12 | Hello | both | `keel_version String, schema_hash u64, platform String, mode String` |
| 13 | Log | core→host | `level u8, target String, message String` |
| 14 | TimerFired | host→core | `timer_id u32` |
| 15 | Snapshot | core→host | §5.9 |
| 16 | Restore | host→core | §5.9 |

### 3.3 Call payload

```
target     u8    0 = free function, 1 = object method, 2 = constructor, 3 = lazy-list page
handle     u64   0 for targets 0 and 2
method_id  u32   (type_id for target 2 identifies the object type; method_id selects the constructor)
call_id    u32
args       encoded params in declaration order
```
For target 2 the layout is `target u8, type_id u32, method_id u32, call_id u32, args` (no handle field). For target 0 the handle field is present and written as 0; decoders ignore its value.
For target 3: `target u8, handle u64, offset u32, limit u32, call_id u32`.

### 3.4 Reply payload

```
call_id  u32
status   u8   0 = ok, 1 = error (typed), 2 = panic, 3 = cancelled, 4 = stream_opened, 5 = bad_request
body     status 0: the return value (Unit = empty; for `Result<T,E>` the `T`)
         status 1: the `E` value (for `Result<T,E>`); for a non-Result method this status cannot occur
         status 2: String message + String backtrace
         status 3: empty
         status 4: empty; items follow as StreamItem
         status 5: String reason (unknown method, decode failure, schema mismatch)
```

### 3.5 ChangeSet payload

```
txn_id     u64
count      u32
entries    count × { handle u64, signal_id u32, op u8, len u32, value bytes }
```
`op`: 0 = full value (`value` is the signal's `T` encoded), 1 = keyed patch (§3.8), 2 = lazy list invalidated (value empty; the host re-pages). `len` lets a host skip an entry it cannot decode.

Ordering guarantee: change-sets are delivered in commit order; a change-set is never split. For one store this holds whichever thread commits: the change-sets of a store reach the sink one at a time, in claim order, with strictly increasing `txn_id`s (§16.1, ADR-020).

### 3.6 Port call and reply

PortCall payload: `port_id u32, method_id u32, port_call_id u32, args` (params in order).
PortReply payload: `port_call_id u32, status u8 (0 ok, 1 error, 2 unavailable), body` where the body is the port method's return value (`Result<T,E>` collapses into status 0/1 like §3.4). Status 2 carries an empty body.

### 3.7 StreamItem payload

```
call_id  u32
flag     u8    0 = item, 1 = end, 2 = error
body     flag 0: item T; flag 2: error E (or String if the stream has no error type)
```
Flow control: the core sends at most `credit` items beyond what has been credited; the host grants credit with StreamCredit. Initial credit is 0; generated bindings grant 16 on subscribe and top up when consumption drops below 8. A Cancel with the stream's `call_id` closes it.

### 3.8 Keyed patch

For `Signal<Vec<T>>` with `#[keel(key = "field")]`. Encoded as:
```
count  u32
ops    count × { op u8, ... }
   0 Insert  { index u32, item T }
   1 Remove  { index u32 }
   2 Update  { index u32, item T }
   3 Move    { from u32, to u32 }
   4 Clear   { }
```
Ops are applied sequentially to the host's current list; indices refer to the list state after the previous op. `Move` means remove the item at `from`, then insert it so that it ends at index `to` (both indices valid in the list before the op). A host that hits an out-of-bounds op treats the signal as desynchronised and re-observes it. The core computes patches by key equality and full-item encoded equality; a change that removes more than 50% of items or has no key overlap is sent as `op = 0` (full value) instead.

### 3.9 Rust API

```rust
pub struct Writer { buf: Vec<u8> }              // write_u8 .. write_f64, write_bool, write_str, write_bytes, write_len(u32), into_vec()
pub struct Reader<'a> { buf: &'a [u8], pos: usize } // read_* mirrors; read_str returns &'a str; read_bytes returns &'a [u8]; remaining(); finish() -> Result<(), WireError> (errors if trailing bytes)
pub trait Encode { fn encode(&self, w: &mut Writer); }
pub trait Decode: Sized { fn decode(r: &mut Reader<'_>) -> Result<Self, WireError>; }
pub enum WireError { UnexpectedEof { needed: usize /* bytes the failing read asked for */, at: usize }, InvalidUtf8 { at: usize /* offset of the string body */ }, InvalidTag { tag: u32, at: usize, ty: &'static str }, LengthTooLarge { len: u32, at: usize }, TrailingBytes { count: usize }, BadMagic, UnsupportedVersion(u16), SchemaMismatch { expected: u64, got: u64 }, DuplicateKey { at: usize }, NegativeDuration { at: usize }, NestingTooDeep { at: usize } }
// TS additionally has `unsafe_integer` (a u64/i64 read as `number` outside the safe range); Kotlin additionally has `PatchOutOfBounds`. Decoders reject counts that cannot fit in the remaining bytes (using each item's minimum encoded length; 1 for unknown types).
pub struct Envelope<'a> { pub kind: Kind, pub seq: u32, pub schema: u64, pub payload: &'a [u8] }  // parse(&[u8]) / write(&mut Writer, ..)
```
`Encode`/`Decode` are implemented for all primitives, `String`, `Vec<u8>` (as Bytes via newtype `Bytes(pub Vec<u8>)`), `Option<T>`, `Vec<T>`, `HashMap<K,V>`/`BTreeMap`, `Duration`, `Timestamp`, `Uuid`, `Result<T,E>`, tuples up to 4, and `()`.

`keel-wire` contains no `unsafe`. It has proptest round-trip tests for every type and a byte-fuzz test (random bytes never panic the decoder).

---

## 4. Macros (`keel-macros`)

All attribute macros are re-exported from the `keel` facade as `keel::api`, `keel::error`, `keel::store`, `keel::port`, `keel::query`, `keel::mutation`. They must produce clear diagnostics (§12) and never silently ignore an item.

### 4.1 `#[keel::api]`

* On `struct` (record): generates `impl Encode`, `impl Decode`, `impl KeelRecord` (type_id), and registers `RecordMeta`. Requires all field types to be wire types. Fields may be `pub` or not; all are encoded.
* On `enum`: generates `Encode`/`Decode` (u16 index + fields) and registers `EnumMeta`.
* On `impl Type { .. }` (object): every `pub fn` becomes a method; `pub fn new(..) -> Self`/`Result<Self,E>` and any fn returning `Self` becomes a constructor. Generates the dispatch function (§5.6), `impl KeelObject for Type` (type_id, name), and registers `ObjectMeta`. Receiver must be `&self` (objects are shared: `Arc<Type>`; interior mutability via signals or `Mutex`). `&mut self` is rejected (E0020).
* On free `fn`: generates a dispatch entry and registers `FunctionMeta`.

Method rules: parameters are wire types, or `ctx: &Ctx` / `ctx: Ctx` as the first parameter (constructors and free fns only; methods get `Ctx` from the object via `self.ctx` convention or `Ctx::current()`); return type is `T`, `Result<T,E>`, `impl Stream<Item = T>`, or `Result<impl Stream<Item = T>, E>`; `async fn` marks `is_async`.

### 4.2 `#[keel::error]`

On an enum. Requires `#[error("…")]` per variant (thiserror-style; `{0}`/`{field}` interpolation, `transparent`). Generates `Display`, `std::error::Error`, `From` for `#[from]` fields, plus everything `#[keel::api]` does with `is_error = true`.

### 4.3 `#[keel::store]`

On a struct. Fields of type `Signal<T>` and `Computed<T>` are signals (in declaration order); other fields are private state (`Ctx`, config). `Lazy<T>` is reserved: it is rejected in v1 (E0001, "lazy lists are not available in v1"), as `keel-bindgen` rejects it. Generates `impl StoreObject for Type` (`cell`, `restore`), a `StoreRestorer` registration and the store part of the object meta (the struct must also have a `#[keel::api(store)] impl` block with at least one constructor; §16.3 has the details, including the hidden `CellSlot` field). Attributes: `#[keel(key = "id")]` on `Signal<Vec<T>>` enables keyed patches; `#[keel(no_coalesce)]` forces every commit of this signal to be delivered. `#[keel::store(restore = "Self::assemble")]` names the function that rebuilds the store from its plain signals on restore; it is required when the store has a `Computed` field (E0013).

### 4.4 `#[keel::port]`

On a trait. Attribute args: `sync` (default for methods without `async`; a port is Sync iff all methods are sync), `event` (methods return `()` and are fire-and-forget host→core). Generates: `PortMeta` registration, a proxy type `<Trait>Proxy` that encodes calls and routes them through the runtime's port table (§5.7), `impl <Trait> for <Trait>Proxy`, and `keel::ports::<Trait>` accessor `Ctx::port::<dyn Trait>()`. Async port methods are `async fn` in the trait (Rust 1.75+ AFIT); the proxy implements them.

### 4.5 `#[keel::query]` and `#[keel::mutation]`

On an `async fn(ctx: &Ctx, ..params) -> Result<T, E>`. Args: `key = "literal"` (may include `{param}` placeholders), `stale = "30s"`, `persist`, `retry = 3`, `idempotent`. Generates a `struct <Name>Query` implementing `keel_query::QueryDef`, and registers `QueryMeta`. See §9.

### 4.6 Emitted metadata

Statics are emitted as `static __KEEL_META_<TypeName>: keel_meta::RecordMeta = RecordMeta { name: "Todo", type_id: 0x…, fields: &[ FieldMeta { name: "id", ty: TypeRef::Uuid, default: false } ] };` using `const`-constructible types. `TypeRef` must therefore be const-constructible: `keel-meta` provides a `const`-friendly mirror `TypeRefMeta` (`&'static`-based, e.g. `TypeRefMeta::Option(&TypeRefMeta::String)`), with `From<&TypeRefMeta> for TypeRef`.

---

## 5. Runtime (`keel-runtime`)

The runtime is dependency-light (no tokio). It provides the executor, the core lock, the object table, transactions, change-sets, ports, timers, panic guard, and snapshots. `keel-ffi` (native) and the wasm exports are thin shells over it.

### 5.1 Threading model

* **Core lock.** A `parking_lot::Mutex<Core>` (native) / `RefCell` (wasm). Whoever holds it *is* the core loop. Sync calls from the host run on the caller's thread holding the lock. Async tasks are polled by the core thread holding the lock. This preserves "one mutator" semantics while keeping sync calls at mutex-acquire cost.
* **Core thread** (native): one `std::thread` named `keel-core` that owns the executor loop: wait for work → lock → poll ready tasks (bounded batch, max 64) → unlock → repeat.
* **Blocking pool** (native, and the test runtime): `keel_runtime::spawn_blocking(f)` runs `f` on a pool of `min(4, cores)` threads without the lock and resumes the awaiting task via the executor. `f` must not write signals: debug builds refuse such a write (the runtime installs `keel_signals::set_write_checker`, §16.1). The check is an allowlist (ADR-023): a signal write with consequences is allowed on a thread that holds a runtime's core lock (a dispatched call, a task poll, an event subscriber, `observe`, `restore`), on a `TestRuntime` driver thread and inside `testing::unchecked_writes`, and refused on every other thread (a pool worker, a host or embedder thread, a thread inside no runtime). Release builds do not evaluate it; the store's delivery lock (§16.1) keeps an off-core write from being overtaken.
* **wasm**: single thread; `keel_poll()` export drives the executor; wakers call the `keel_host_schedule()` import (deduplicated per turn).
* **Shutdown.** `keel_shutdown` / `Runtime::shutdown` first answers every call still in flight with status 3 (`cancelled`) and ends every open stream with a `StreamItem` flag 2 (error) whose `String` body starts `"cancelled: "`, each exactly once; then it stops and joins the core, timer and blocking threads, fails pending port calls with `PortError::Cancelled`, clears event subscribers and Rust port bindings (closures that hold a `Ctx` are reference cycles with the runtime), and drops every task and object under the core lock. Afterwards `call` answers status 5, and `spawn`, `sleep`, `port_call` and `event` on a surviving `Ctx` are no-ops that log a warning (never a panic, never queued). It must not be called from the core thread or a host callback (it would wait for itself); debug builds assert this (ADR-023). `Runtime::extension` values are not cleared. A cancelled task's future is always dropped on the core (with the core lock held), so user `Drop` code never runs concurrently with core code.
* **Host callbacks** (reply, change-set, port call) are invoked from whatever thread completed the work, **while the core lock may be held**. The host must not call back into the core synchronously from these callbacks except `keel_buf_free`; it enqueues onto its main thread. Violations are detected in every build and reported as `E_REENTRANT`: the runtime marks the calling thread for the duration of **every** host callback (`reply`, `change_set`, `stream_item`, `port_call`, `timer_set`, `log`, `schedule`) and each core-lock entry point refuses a thread that holds that runtime's core lock or is inside one of its callbacks (ADR-023). The second condition matters: a callback delivered on a thread that does not hold the core lock (an off-core commit hands its change-set to the host while holding the store's delivery lock) that waited for the core would deadlock against a core waiting for that delivery lock. `port_reply`, `timer_fired`, `stream_credit` and `stats_json` never take the core lock and stay allowed from callbacks.

### 5.2 Executor

Own minimal executor: tasks are `Pin<Box<dyn Future<Output = ()> + Send>>` in a slab; a `Waker` pushes the task id onto an MPSC ready queue and nudges the core thread (Condvar) / host scheduler (wasm). Each in-flight call is one task; cancellation drops the task (which drops the future, cancelling awaited port calls: the `PortFuture` `Drop` sends a port cancel notification — v1 marks the port call as abandoned so a late reply is discarded; the host is not told, so the set of abandoned ids is capped at 4096 with FIFO eviction and a WARN, and a reply for a forgotten id is logged as unknown).

### 5.3 Ctx

```rust
pub struct Ctx(Arc<RuntimeInner>);
impl Ctx {
    pub fn port<P: Port + ?Sized>(&self) -> Arc<P>;          // typed proxy or fake
    pub fn http(&self) -> Arc<dyn Http>; pub fn kv(&self) -> …; pub fn clock(&self) -> …; pub fn rng(&self) -> …; pub fn log(&self) -> …; // convenience
    pub fn txn<R>(&self, f: impl FnOnce() -> R) -> R;        // batch writes into one transaction
    pub fn spawn(&self, fut: impl Future<Output = ()> + Send + 'static);
    pub fn spawn_blocking<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> impl Future<Output = T>;
    pub fn sleep(&self, d: Duration) -> impl Future<Output = ()>; // via Timer port
    pub fn query(&self) -> &QueryClient;                      // keel-query
    pub fn mutate<M: MutationDef>(&self, input: M::Input) -> MutationBuilder<M>;
    pub fn events(&self) -> &Events;                          // subscribe to Connectivity/Lifecycle events
    pub fn current() -> Ctx;                                  // thread-local, valid inside any dispatched call
}
```

### 5.4 Object table

`Slab<Slot { generation: u32, entry: Option<Entry { object: Arc<dyn AnyObject>, .. }> }>`. `AnyObject: Any + Send + Sync + KeelObject`. `insert(Arc<T>) -> Handle`, `get::<T>(handle) -> Result<Arc<T>, BadHandle>`, `release(handle)`. **Generations are issued from one process-wide, monotonically increasing `u32` counter** (`fetch_add`; `0` is never issued), not from the slot: every `insert` takes a fresh generation, so a released handle stays stale for good and a restore (§5.9) cannot re-issue one. The counter does not wrap: after `u32::MAX` issues the next `insert` logs FATAL and panics (contained at the boundary, status 2) while everything that exists keeps working, a documented v1 limit (ADR-022). Release decrements; the `Arc` may outlive the handle if a task holds it. Stores additionally register in the `stores` index for change-set routing. Debug builds keep a count of live handles readable via `keel_stats()`.

### 5.5 Transactions and change-sets

* Every signal write outside `ctx.txn` is an implicit single-write transaction.
* A transaction is thread-local depth + a dirty set `(store_handle, signal_id)`. On commit (depth → 0): for each dirty store, recompute observed dirty computeds (dependency order), build the change-set of **observed** signals only (plus `no_coalesce` signals always), assign `txn_id` (monotonic u64), and invoke the sink. Unobserved dirty signals are recorded in the store's `pending_dirty` so that a later `observe` emits the current value.
* Writes are applied directly to the signal cell (no overlay) **but** are wrapped in a panic guard at the dispatch boundary: on panic the runtime marks the store `poisoned`, emits `Log(error)` and the dispatch returns status 2. A poisoned store keeps working (values are still consistent per write); poisoning is informational in v1 (ADR-017 explains why a copy-on-write overlay is deferred). A **computed** that panics makes its store's commit abandon the change-set: the affected slots are resent as full values by the next successful commit (ADR-019), so a computed that panics on its *current* inputs holds back that store's deliveries — loudly, via status 2 and an error log — until a later write lets it recover. Per-signal isolation of a failing computed is a v1.x refinement.
* `keel_observe(handle, signal_id, 1)` immediately emits a change-set with the current value(s) of the newly observed signal(s) (synchronously, before returning, in-process; asynchronously over a transport).

### 5.6 Dispatch

The macro-generated dispatch function has the signature
```rust
fn dispatch(rt: &Runtime, call: DispatchCall<'_>) -> DispatchResult
pub struct DispatchCall<'a> { pub method_id: u32, pub call_id: u32, pub handle: Handle, pub args: &'a [u8] }
pub enum DispatchResult { Sync(Result<Vec<u8>, Vec<u8>>) /* ok bytes / err bytes */, Async(Pin<Box<dyn Future<Output = Result<Vec<u8>, Vec<u8>>> + Send>>), Stream(Pin<Box<dyn Stream<Item = Result<Vec<u8>, Vec<u8>>> + Send>>), Unknown, BadRequest(String) /* status 5 with a reason */ }
```
The runtime looks up the object by handle (`Arc<dyn AnyObject>`), downcasts inside the generated dispatcher, decodes args (status 5 on failure), runs the method, encodes the result. For `Sync` results of a `keel_call_sync`, the bytes are returned directly; for `keel_call` the reply callback is invoked. `Async` results are spawned as a task keyed by `call_id`. Panics are caught by `std::panic::catch_unwind` (`AssertUnwindSafe`) at this boundary on native.

**The synchronous reply slot (ADR-028).** A synchronous method is answered with `rt.sync_ok(&value, Encode::encode)` / `rt.sync_err(&error, Encode::encode)` (both return a `DispatchOutcome`) instead of a built `DispatchResult::Sync`. Under `call_sync` the runtime has armed a per-thread reply slot for that call: the complete `Reply` payload (`call_id u32`, `status u8`, the encoded body, §3.4) is encoded straight into one reusable thread-local buffer and the outcome is a zero-sized marker, so the path from payload decode to reply bytes allocates nothing once the buffer has warmed up (it keeps at most 64 KiB between calls). Everywhere else the slot is not armed (`keel_call`, a dispatch layer that did not opt in, a dispatcher invoked directly, a call made from inside the closure that is reading another reply, a call into a second runtime from a method of the first, a thread that is being torn down) and `sync_ok` / `sync_err` build `DispatchResult::Sync(Ok/Err(Vec))` exactly as before. The two paths put the same bytes on the wire; `testing::call_sync_reference` runs a call through the allocating path so tests can compare them. `Runtime::call_sync_with(payload, |reply| ..)` lends the buffer to a closure (after the core lock is released) and is the allocation-free entry; `Runtime::call_sync` copies it into the `Vec` it returns, the single allocation of a `keel_call_sync`, which is the `KeelBuf` the caller frees (§6). Hand-written dispatchers and `DispatchLayer`s may keep returning `DispatchResult` and need not change.

### 5.7 Ports

The runtime holds a `PortTable`: `port_id → PortBinding { Foreign { cb, user_data } | Rust(Arc<dyn Any>) }`. Proxies generated by `#[keel::port]` encode args and call `runtime.port_call(port_id, method_id, args) -> PortFuture` (async) or `runtime.port_call_sync(..) -> Result<Vec<u8>, PortError>` (sync). Foreign bindings: the runtime invokes the registered callback (§6.3); sync ports must return synchronously (status 0) or the call fails with `PortError::Unavailable`. Rust bindings (fakes, and the built-in native `Timer`) are called directly. Events: `runtime.deliver_event(port_id, method_id, payload)` fans out to `Events` subscribers on the core loop.

An unavailable port is a normal outcome, not a bug: a port nobody registered behaves as `Unavailable` (§6.3), a call can be cancelled, and a host can reply with bytes that do not decode. The proxy of a method returning `Result<T, E>` therefore never panics on those outcomes: `PortError::Failed(bytes)` decodes into `E`, and every other `PortError` becomes `E::from(port_error)`, so the error type implements `From<PortError>` (E0033 at compile time when it does not; `HttpError` and `FsError` do, mapping `Unavailable` to `Network(..)` / `Io(..)`). A method without an error channel has no typed way to say "unavailable"; its proxy panics with a message naming the port and method and how to bind one (E0062), which the runtime contains at the dispatch boundary and which traps the core on wasm (`panic=abort`, §7). Register the optional ports a web build does not implement, or give their methods a `Result` (ADR-025).

Standard ports and their methods are defined in `keel-ports` (§8).

### 5.8 Timers

`Timer` is a port with one method `set(timer_id: u32, delay_ms: u64)` (event-ish: host→core `TimerFired(timer_id)` when due). Native default binding: a Rust timer thread inside `keel-runtime` (`BinaryHeap` + `Condvar`), used unless the host registers a foreign Timer. wasm: the TS runtime registers itself (`setTimeout`). Tests: `FakeClock` implements both `Clock` and `Timer` and fires timers when advanced.

### 5.9 Snapshot and restore

`Snapshot` payload (all little-endian): `count u32, generation_floor u32, stores × { handle u64, type_id u32, signal_count u32, signals × { signal_id u32, len u32, value bytes } }` (computed signals excluded; restored by recomputation). `generation_floor` is the highest handle generation the core had issued when the snapshot was taken (`0` if none). Hosts treat a snapshot as opaque bytes and hand it back unchanged.

`keel_restore(bytes)` rebuilds each store via its generated `restore(ctx, values)` and re-issues the same handles (the table is rebuilt from the snapshot, so handles held by the host remain valid). It raises the generation counter to `max(current, generation_floor, every generation in the snapshot)` and never lowers it, so a handle issued before the snapshot, or between the snapshot and the restore, can never be issued again to another object, in this process or a fresh one (ADR-022). A snapshot whose floor, or any store handle's generation, is `u32::MAX` is refused (status 5 / `BAD_SNAPSHOT`): it would leave nothing to issue. A snapshot in the layout without the floor fails to decode. Objects that are not stores are not snapshotted; their handles become invalid after restore (status 5 `stale_handle`). **In-flight calls and streams whose receiver the restore replaced or invalidated are cancelled** (ADR-023): every call made on a store is (each store is rebuilt, so a call still running on the old object would finish on a detached store and report success for a write the restored store never saw). A plain call is answered with status 3 (`cancelled`), exactly once; a stream ends with a `StreamItem` flag 2 (error) whose body is a `String` (`"cancelled: ..."`), the same shape as a stream panic, because the host did not ask for the end; the tasks are dropped. Calls with no receiver (free functions, constructors) carry on. Restore emits change-sets for all observed signals: one per re-observed store, in handle order, each built and delivered under that store's delivery lock through the path `keel_observe` uses (§16.1 `observe_and_deliver`), inside one transaction (ADR-023).

### 5.10 Devtools protocol (transport only, optional)

When a transport is attached with `mode = "dev"`, the core additionally emits `Log` messages for every transaction commit (`txn_id`, dirty count, bytes), every port call (timing), and every panic. The inspector is a consumer of the same envelope stream; no separate protocol.

---

## 6. Native C ABI (`keel-ffi`)

Exported with `#[unsafe(no_mangle)] pub extern "C"`, C-compatible types only. All `*const u8, u32` pairs are borrowed for the duration of the call unless stated (a null `ptr` is an empty payload). The functions may be called from any thread, concurrently, subject to the host contract below. `keel-ffi` is the only crate besides the JNI shim allowed to contain `unsafe`, and every block has a `// SAFETY:` comment.

```c
typedef struct { uint8_t *ptr; uint32_t len; uint32_t cap; } KeelBuf;        // owned by the core; free with keel_buf_free
typedef void (*keel_reply_cb)(void *user, uint32_t call_id, const uint8_t *ptr, uint32_t len);
typedef void (*keel_changeset_cb)(void *user, const uint8_t *ptr, uint32_t len);
typedef uint8_t (*keel_port_cb)(void *user, uint32_t port_id, uint32_t method_id, uint32_t port_call_id, const uint8_t *ptr, uint32_t len, KeelBuf *out_reply); // returns 0 = replied synchronously into out_reply, 1 = will reply async, 2 = unavailable
typedef void (*keel_stream_cb)(void *user, uint32_t call_id, const uint8_t *ptr, uint32_t len);  // StreamItem payload

uint32_t keel_abi_version(void);                       // 1
uint64_t keel_schema_hash(void);
KeelBuf  keel_schema_json(void);                       // owned copy
uint32_t keel_init(const uint8_t *cfg, uint32_t len, keel_reply_cb reply, keel_changeset_cb changes, keel_stream_cb stream, void *user); // idempotent per process; cfg = encoded RuntimeConfig record; returns 0 ok
void     keel_shutdown(void);                          // answers every in-flight call (status 3) and ends every open stream (§5.1 Shutdown) before stopping the threads; then drops the port registrations, waiting for port callbacks still running (host contract 5)
uint32_t keel_call(const uint8_t *ptr, uint32_t len);  // Call payload (§3.3); returns 0 accepted, 5 bad request. Reply via reply_cb. Works for sync and async methods.
KeelBuf  keel_call_sync(const uint8_t *ptr, uint32_t len); // Reply payload (§3.4) returned directly; only for sync methods (async → status 5)
void     keel_cancel(uint32_t call_id);
void     keel_stream_credit(uint32_t call_id, uint32_t credit);
void     keel_observe(uint64_t handle, uint32_t signal_id, uint8_t on);
void     keel_release(uint64_t handle);
void     keel_port_register(uint32_t port_id, keel_port_cb cb, void *user); // cb NULL removes; removing or replacing waits for the old registration's running callbacks (host contract 1, 5)
void     keel_port_reply(const uint8_t *ptr, uint32_t len);   // PortReply payload; allowed from a callback; port_call_id 0 is ignored (host contract 6)
void     keel_event(uint32_t port_id, uint32_t method_id, const uint8_t *ptr, uint32_t len);
void     keel_timer_fired(uint32_t timer_id);
KeelBuf  keel_snapshot(void);
uint32_t keel_restore(const uint8_t *ptr, uint32_t len);
KeelBuf  keel_stats_json(void);                        // live handles, tasks, txn count, crossings
void     keel_buf_free(KeelBuf buf);
```

`RuntimeConfig` record: `{ platform: String, mode: String /* "inproc" | "dev" */, core_threads: u8, blocking_threads: u8, log_level: u8 }`.

`keel_snapshot` with no running runtime (before `keel_init`, after `keel_shutdown`) returns an empty snapshot (`count 0`) whose `generation_floor` is the process-wide generation counter, which survives shutdown (§5.9, ADR-022).

Return codes (as implemented): `keel_init` returns 0 ok, or a nonzero `init_code` (bad argument, undecodable config, already initialized with a *different* embedder — a repeat init with the same callbacks and `user` is a no-op returning 0). `keel_restore` returns 0 ok or a nonzero `restore_code`; a failed restore leaves the core unchanged. The native ABI has no `keel_poll`, so `core_threads == 0` is treated as 1. There is no native log callback: core log records reach the host through its registered `Log` port (the JNI `Callbacks` interface likewise has none).

**Host contract** (the same text is the header comment of `keel.h`; a host that breaks a rule has undefined behaviour). The *callbacks* are `reply_cb`, `changeset_cb`, `stream_cb` (given to `keel_init`) and every `port_cb` (given to `keel_port_register`).

1. **Lifetime.** The three `keel_init` callbacks and its `user` stay valid until `keel_shutdown` *returns*; none is called afterwards. A `port_cb` and its `user` stay valid until `keel_port_register(id, NULL, ..)` or a replacing `keel_port_register(id, ..)` has returned for that id, or `keel_shutdown` has returned. When one of those calls returns, no invocation of the old registration is running, none will start and its `user` is never read again: the host may free `user` right then (ADR-026; the core waits, see 5).
2. **Threads.** A callback runs on the thread that produced the event (the `keel-core` thread, a blocking-pool thread, or a host thread inside a `keel_*` call such as `keel_call`), possibly with the core lock or a store's delivery lock held (§5.1). Callbacks run **concurrently** (four simultaneous `port_cb` invocations are measured): they must be thread-safe, short, and must not assume the main thread.
3. **No unwinding.** A callback must not throw, `longjmp` or otherwise unwind through the core.
4. **Re-entrancy.** A callback must not call back into the core except the entries that never take the core lock: `keel_buf_free`, `keel_port_reply`, `keel_stream_credit`, `keel_timer_fired`, `keel_stats_json`, and the read-only `keel_abi_version`, `keel_schema_hash`, `keel_schema_json` (§5.1). `keel_call`, `keel_call_sync`, `keel_cancel`, `keel_observe`, `keel_release`, `keel_event` and `keel_restore` take the core lock and are refused with `E_REENTRANT` (status 5, restore code 6, or logged and ignored), never deadlocked; `keel_init`, `keel_shutdown`, `keel_port_register` and `keel_snapshot` must not be called from a callback at all.
5. **Blocking.** Removing or replacing a port registration (`keel_port_register`) and `keel_shutdown` wait for the port callbacks of the registrations they remove that are running on other threads. They must not be called from inside a callback (debug builds assert; release builds skip the wait for the calling thread's own callbacks), not while holding a lock a `port_cb` needs, and a `port_cb` that never returns keeps them from returning. `keel_shutdown` may be called from any thread but a callback, also concurrently with other entries (they complete or fail softly); it removes the port registrations inside the same critical section that serialises `keel_init`, so an init on another thread waits for it and a registration made after that init returns is never lost to it.
6. **Log.** The core's log records reach the host as `Log.log` calls with `port_call_id 0` (fire and forget). Nothing waits for the answer (0, 1 or 2 are all accepted) and a `keel_port_reply` carrying id 0 is ignored silently: acting on it would log "no port call 0 is pending", which is one more Log call. Real port calls are numbered from 1.

Register ports before the core needs them. `InitHook`s (query hydration reads the `Kv` port) run on the core as soon as the runtime has been created, which is before `keel_init` has returned to the host. Ports the host supplies *with* the init call (the JNI `Callbacks` object, the wasm `port_call` import) are in place by then; a port registered with `keel_port_register` **after** `keel_init` returns is racy against the hooks: a hook's first call to it can reach the host before the registration and is answered `Unavailable` (§6.3), and the hook has silently lost its start-up work unless it retries (`keel-query` hydration retries for about five seconds). A host that needs a port at start-up therefore supplies it with `keel_init`, or registers it before any hook could run it, and must not assume a late registration is seen by start-up work.

### 6.1 JNI shim (feature `jni`)

`keel-ffi` with feature `jni` exports `Java_dev_keel_runtime_KeelNative_<name>` natives registered via `JNI_OnLoad` → `RegisterNatives` (no per-call lookup). Java signatures (class `dev.keel.runtime.KeelNative`):

```java
static native int    abiVersion();
static native long   schemaHash();
static native byte[] schemaJson();
static native int    init(byte[] cfg, KeelNative.Callbacks cb);   // cb.onReply(int callId, ByteBuffer reply), cb.onChangeSet(ByteBuffer), cb.onStream(int callId, ByteBuffer), int cb.onPortCall(int portId, int methodId, int portCallId, ByteBuffer args) returns 0/1/2, byte[] cb.portSyncReply() (read after a 0 return)
static native int    call(byte[] payload);
static native byte[] callSync(byte[] payload);
static native void   cancel(int callId);
static native void   streamCredit(int callId, int credit);
static native void   observe(long handle, int signalId, boolean on);
static native void   release(long handle);
static native void   portReply(byte[] payload);
static native void   event(int portId, int methodId, byte[] payload);
static native void   timerFired(int timerId);
static native byte[] snapshot();
static native int    restore(byte[] snapshot);
static native String statsJson();
```
`ByteBuffer`s passed to callbacks are **direct** buffers over core memory valid only during the callback; the Kotlin runtime decodes immediately. `byte[]` arguments are copied once via `GetByteArrayRegion`. The JNI callbacks follow the host contract of §6. In particular a synchronous port is a two-call protocol with hidden per-thread state: the shim calls `portSyncReply()` on the same thread, right after `onPortCall` returned 0, and callbacks run concurrently, so an implementation must carry the reply in thread-local state (the shipped `InprocTransport` does, in a `ThreadLocal`), never in a shared field.

### 6.2 Swift

Swift calls the C ABI through a module map (`KeelFFI` C module inside the XCFramework). `KeelRuntime` wraps it; generated code never touches C.

### 6.3 Port callback contract

For **sync** ports the host must fill `out_reply` with a PortReply payload and return 0 before returning. For **async** ports the host returns 1 and later calls `keel_port_reply`. Returning 2 fails the call with `PortError::Unavailable`. A port that is not registered behaves as 2.

`out_reply` memory rule (the one buffer a host allocates): on returning 0 the host stores a block from the C allocator (`malloc`) with `len` set; ownership passes to the core, which copies the bytes and **always** releases the block with `free`, never `keel_buf_free` and never as a Rust allocation. `cap` is reserved: the host sets it to 0 and the core ignores it (a host-written `cap` used to select a Rust deallocator, which a `malloc`ed block with `cap = len` turned into allocator-mismatch undefined behaviour, review M1). On wasm (§7) a sync port must call `keel_port_reply` *before* returning 0 from the `port_call` import; returning 0 without having replied fails that call instead of leaving it pending.

---

## 7. wasm ABI (`keel-ffi`, target `wasm32-unknown-unknown`)

No wasm-bindgen. Exports and imports use only `i32`/`i64`/`f64`. Memory is the module's exported `memory`. `_initialize` is exported when present (reactor); the host calls it once after instantiation.

Exports:
```
keel_alloc(len: i32) -> i32 ptr           keel_free(ptr: i32, len: i32)
keel_abi_version() -> i32                  keel_schema_hash() -> i64
keel_schema_json() -> i32 (ptr to KeelBuf struct { ptr i32, len i32, cap i32 })
keel_init(cfg_ptr, cfg_len) -> i32
keel_call(ptr, len) -> i32                 keel_call_sync(ptr, len) -> i32 (KeelBuf*)
keel_cancel(call_id)                       keel_stream_credit(call_id, credit)
keel_observe(handle_lo: i32, handle_hi: i32, signal_id, on)   // u64 split to avoid BigInt requirement
keel_release(handle_lo, handle_hi)
keel_port_reply(ptr, len)                  keel_event(port_id, method_id, ptr, len)
keel_timer_fired(timer_id)                 keel_poll()                                // drive the executor
keel_snapshot() -> i32 (KeelBuf*)          keel_restore(ptr, len) -> i32
keel_buf_free(buf_ptr)                     keel_stats_json() -> i32 (KeelBuf*)
```
Imports (module `"keel"`):
```
reply(call_id, ptr, len)        changeset(ptr, len)        stream(call_id, ptr, len)
port_call(port_id, method_id, port_call_id, ptr, len) -> i32 (0 sync: host wrote reply via keel_port_reply *before returning*; 1 async; 2 unavailable)
schedule()                      // host must call keel_poll() on the next microtask
timer_set(timer_id, delay_ms_lo, delay_ms_hi)
log(level, ptr, len)            // payload at ptr/len: target String, message String
now_ms() -> f64                 // Date.now()
random(ptr, len)                // crypto.getRandomValues into memory
```
The Clock/Rng/Log ports have built-in wasm bindings over these imports so a web app needs no adapter code for them. All ports remain overridable.

`keel_alloc(len)` never returns 0: it traps (after a level-5 `log` record) when memory is exhausted and when `len` is a size no allocation can have (`>= 0x7fff_fff9` on wasm32); a host that does not check the result would otherwise write at linear address 0, the bottom of the shadow stack (the TypeScript runtime also refuses a 0). `port_call` returning 0 means the host called `keel_port_reply` for **that** `port_call_id` before returning; a reply for some other pending call does not count, and the call then fails instead of staying pending.

Build: `--release`, `-C panic=abort`, `-C opt-level=z` or `s` (measured), `-C lto=fat`, `-Z`-free. `wasm-opt -Oz` when available. Panics call the `log` import with level 5 (fatal) before trapping so the host can restart from snapshot.

---

## 8. Standard ports (`keel-ports`)

```rust
#[keel::port(sync)]  pub trait Clock { fn now_ms(&self) -> i64; fn monotonic_ns(&self) -> u64; }
#[keel::port(sync)]  pub trait Rng   { fn fill(&self, len: u32) -> Bytes; }
#[keel::port(sync)]  pub trait Log   { fn log(&self, level: u8, target: String, message: String); }
#[keel::port]        pub trait Http  { async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>; }
#[keel::port]        pub trait Kv    { async fn get(&self, key: String) -> Option<Bytes>; async fn set(&self, key: String, value: Bytes); async fn delete(&self, key: String); async fn list(&self, prefix: String) -> Vec<String>; }
#[keel::port]        pub trait SecureStore { /* same as Kv */ }
#[keel::port]        pub trait Fs    { async fn read(&self, path: String) -> Result<Bytes, FsError>; async fn write(&self, path: String, data: Bytes) -> Result<(), FsError>; async fn delete(&self, path: String) -> Result<(), FsError>; async fn list(&self, dir: String) -> Result<Vec<String>, FsError>; }
#[keel::port]        pub trait Timer { fn set(&self, timer_id: u32, delay_ms: u64); }          // sync fire-and-forget; completion via TimerFired
#[keel::port(event)] pub trait Connectivity { fn changed(&self, online: bool, kind: NetKind); }
#[keel::port(event)] pub trait Lifecycle    { fn changed(&self, state: AppState); }            // Active | Background | Inactive
```
Records: `HttpRequest { method: HttpMethod, url: String, headers: Vec<Header>, body: Option<Bytes>, timeout_ms: Option<u32> }`, `HttpResponse { status: u16, headers: Vec<Header>, body: Bytes }`, `HttpError { Network(String), Timeout, Cancelled, InvalidUrl(String) }`, `Header { name: String, value: String }`, `FsError { NotFound, Denied, Io(String) }`, `NetKind { Wifi, Cellular, Wired, Unknown, None }`, `AppState { Active, Inactive, Background }`.

The standard surface (these ten ports and these types, plus `HttpMethod { Get, Post, Put, Delete, Patch, Head, Options }`) ships in each platform runtime (`@keel/runtime`, `dev.keel.runtime.adapters`, `KeelRuntime`), not in generated code: every core links `keel-ports`, so its schema contains all of it and the schema hash covers it, but `keel-bindgen` leaves it out of an app's bindings and the generated code refers to the runtime's own types (section 10.5, ADR-024).

`HttpError` and `FsError` implement `From<PortError>` (§5.7): an unavailable port is `Network("the Http port has no adapter registered")` / `Io("the Fs port has no adapter registered")`, a cancelled call is `Cancelled` / `Io(..)`, a reply that does not decode is `Network("malformed port reply: ..")` / `Io(..)`; the wire layouts are unchanged.
Fakes (all in `keel-ports::fakes`, `Send + Sync`): `FakeHttp` (script responses by matcher; records calls), `MemKv`, `MemSecureStore`, `MemFs`, `FakeClock` (settable `now`, `advance(d)` fires due timers; implements `Clock` + `Timer`), `SeededRng` (xorshift64\*), `CaptureLog`, `ScriptedConnectivity`, `ScriptedLifecycle`. `TestRuntime::new()` installs all fakes and runs the executor on the test thread (`run_until(fut)` / `run_pending()`).

---

## 9. Query (`keel-query`)

* `QueryClient` lives in the runtime. Cache: `HashMap<QueryKey, Entry>`; `QueryKey = (query_id, encoded_params_bytes)`; `Entry { data: Option<Vec<u8>>, error: Option<Vec<u8>>, status: Status /* Idle | Fetching | Success | Error */, updated_at_ms, stale_ms, observers: u32, inflight: Option<TaskId>, persist: bool, gc_at: Option<i64> }`.
* A query is observed through a **QueryHandle object** (a store generated by the macro): signals `data: Option<T>` (0), `status: QueryStatus` (1), `error: Option<E>` (2), `fetching: bool` (3), `updated_at: Option<Timestamp>` (4). Constructing the handle registers an observer and triggers a fetch if stale or missing; releasing it decrements; when observers hit 0 the in-flight fetch is cancelled and `gc_at = now + gc_ms` (default 5 min).
* Refetch triggers: observer added while stale; `Lifecycle::Active` (all observed stale); `Connectivity online` (all observed); `interval_ms` if set; `invalidate(prefix)`.
* Retry: `retry` attempts (default 3) with backoff `min(1000 × 2^n, 30000)` ms ± 20% jitter (via `SeededRng`/`Rng`), via `Timer`.
* Dedup: one in-flight fetch per key; concurrent observers share it.
* Mutations: `ctx.mutate(M(input)).optimistic(|cache| ..).invalidates([..]).await`. Optimistic closure runs inside a transaction with `CacheView` giving typed access to entries (`cache.get::<TodosQuery>(params) -> Option<T>`, `set`, `update`). On error, the pre-mutation entries are restored in one transaction. On success, `invalidates` keys are marked stale and refetched if observed.
* Offline queue: mutations marked `idempotent` that fail with `HttpError::Network` while `Connectivity` is offline are appended to the persisted queue (`Kv` key `keel.query.queue`) with `mutation_id, params bytes, idempotency_key (Uuid)`, and replayed FIFO on `online`. Non-idempotent mutations fail immediately when offline.
* Persistence: entries with `persist` are written to `Kv` under `keel.query.cache.<query_id>.<fnv1a64(params)>` as `{ schema_hash u64, updated_at i64, data bytes }` after each successful fetch (debounced 250 ms), and hydrated at `QueryClient::hydrate()` (called by `keel_init`); mismatched `schema_hash` entries are dropped.

---

## 10. Generated code shapes (`keel-bindgen`)

`keel-bindgen` takes a `Schema` and emits three trees. Each generator is a Rust module with `write_*` functions and a golden-file test suite (`crates/keel-bindgen/tests/golden/<case>/{schema.json, swift/, kotlin/, ts/}`). Emitted code depends only on the matching runtime package. Naming: Rust `snake_case` → Swift/Kotlin/TS `camelCase` for methods and fields, `PascalCase` for types; enum variants → Swift `lowerCamel` cases, Kotlin `UPPER_SNAKE` for unit enums and `PascalCase` classes for data enums, TS string-literal `"camelCase"` / `kind: "camelCase"`.

### 10.1 Swift

```swift
// record
public struct Todo: KeelRecord, Sendable, Hashable, Codable {
    public var id: UUID; public var title: String; public var done: Bool
    public init(id: UUID, title: String, done: Bool)
    public static func keelDecode(_ r: inout KeelReader) throws -> Todo
    public func keelEncode(_ w: inout KeelWriter)
}
// unit enum
public enum Filter: UInt16, KeelEnum, CaseIterable, Sendable, Codable { case all = 0, active = 1, done = 2 }
// data enum
public enum Shape: KeelEnum, Sendable, Hashable { case circle(radius: Double); case rect(w: Double, h: Double) }
// error
public enum TodoError: KeelError, Error, Sendable, Hashable { case emptyTitle; case http(HttpError) }   // description from #[error]
// object
public final class Calculator: KeelObject, @unchecked Sendable {
    public init(ctx: KeelCore = .shared) throws           // constructor `new`
    public func add(a: Int32, b: Int32) -> Int32           // sync
    public func fetch(url: String) async throws(HttpError) -> String   // async Result
    public func ticks() -> AsyncThrowingStream<UInt32, Error>          // stream
}
// store
@MainActor @Observable public final class Todos: KeelStore {
    public private(set) var todos: [Todo]; public private(set) var filter: Filter; public private(set) var visible: [Todo]
    public init(ctx: KeelCore = .shared) throws
    public func setFilter(_ f: Filter)
    public func add(title: String) async throws(TodoError) -> Todo
}
// port
public protocol Http: KeelPort { func request(_ req: HttpRequest) async throws(HttpError) -> HttpResponse }
```
Sync methods in `inproc` mode call `keel_call_sync`. Store initial values are decoded from the change-set emitted by `keel_observe` during `init`. Typed throws require Swift 6; the generator also has a `--swift-typed-throws=false` flag that emits plain `throws`.

### 10.2 Kotlin

```kotlin
data class Todo(val id: UUID, val title: String, val done: Boolean) : KeelRecord { companion object : KeelCodec<Todo> }
enum class Filter(val index: UShort) : KeelEnum { ALL(0u), ACTIVE(1u), DONE(2u) }
sealed interface Shape : KeelEnum { data class Circle(val radius: Double) : Shape; data class Rect(val w: Double, val h: Double) : Shape }
sealed class TodoError : KeelException() { data object EmptyTitle : TodoError(); data class Http(val cause: HttpError) : TodoError() }
class Calculator(ctx: KeelCore = KeelCore.shared) : KeelObject(ctx) {
    fun add(a: Int, b: Int): Int
    suspend fun fetch(url: String): String            // throws HttpError
    fun ticks(): Flow<UInt>
}
class Todos(ctx: KeelCore = KeelCore.shared) : KeelStore(ctx) {
    val todos: StateFlow<List<Todo>>; val filter: StateFlow<Filter>; val visible: StateFlow<List<Todo>>
    fun setFilter(f: Filter)
    suspend fun add(title: String): Todo                // throws TodoError
}
interface Http : KeelPort { suspend fun request(req: HttpRequest): HttpResponse }   // throws HttpError
```
Compose consumers use `collectAsState()` on the `StateFlow`s (no extra module). `KeelStore` and `KeelObject` implement `AutoCloseable`; a `Cleaner` releases leaked handles.

### 10.3 TypeScript

```ts
export interface Todo { id: string; title: string; done: boolean }
export type Filter = "all" | "active" | "done";
export type Shape = { kind: "circle"; radius: number } | { kind: "rect"; w: number; h: number };
export class TodoError extends KeelError { readonly kind: "emptyTitle" | "http"; readonly cause?: HttpError }   // subclasses TodoError.EmptyTitle, TodoError.Http for instanceof
export class Calculator extends KeelObject {
  static create(core?: KeelCore): Promise<Calculator>;
  add(a: number, b: number): Promise<number>;
  fetch(url: string): Promise<string>;                    // rejects with HttpError
  ticks(): AsyncIterable<number>;
}
export class Todos extends KeelStore {
  static create(core?: KeelCore): Promise<Todos>;
  readonly todos: Signal<Todo[]>; readonly filter: Signal<Filter>; readonly visible: Signal<Todo[]>;
  setFilter(f: Filter): Promise<void>;
  add(title: string): Promise<Todo>;
}
export interface Http extends KeelPort { request(req: HttpRequest): Promise<HttpResponse> }
```
All methods return `Promise` (uniform across main-thread, worker and remote modes). `Signal<T>` has `get()`, `subscribe(fn)`, `peek()`; `@keel/runtime/react` exports `useKeel(Class)` and `useSignal(signal)`; `vue`, `svelte`, `solid` adapters are thin files. `i64`/`u64` → `bigint`; `#[keel(js_number)]` → `number`.

### 10.4 Codecs

Each runtime ships `KeelWriter`/`KeelReader` mirroring §3.9 and the generated code implements per-type encode/decode. Generated codecs must be allocation-conscious: decode records into constructors directly, decode `Vec` with a preallocated capacity, and never go through JSON.

### 10.5 The standard library

The ten standard ports of section 8 and the eight types they exchange (`HttpMethod`, `Header`, `HttpRequest`, `HttpResponse`, `HttpError`, `FsError`, `NetKind`, `AppState`) are in every core's schema but not in an app's bindings: the runtimes already implement the ports and ship the types, and a second `FsError` in the app's namespace would fail native review. The schema keeps them (R1; the schema hash covers them); the generators filter them at generation time (ADR-024).

* **What is standard.** An item is left out only when it is exactly the standard one: same name, same id and same shape (fields, variants and indices, method ids and signatures; documentation is ignored). Ids are derived from names (section 1.1), so a schema item that only shares a name with a standard one has the standard id but another shape and stays the app's own type; one with the name and another id is E0052 (section 12). A standard type is left out only while every standard type it refers to is (a schema with its own `Header` does not get the runtime's `HttpRequest`). The table is `keel_bindgen::stdlib`, with the ids pinned as hex and cross-checked against the registrations of `keel-ports`.
* **What references become.** A port or type of the app that mentions a standard type refers to the runtime's own: TypeScript imports the type and its `<Name>Codec` from `@keel/runtime`; Kotlin imports `dev.keel.runtime.adapters.<Name>` (spelled in full where a variant of the enclosing sealed type shadows the name); Swift's runtime keeps seven of the eight internal (`PortHttpRequest`, ...), so the generator declares each one that something refers to (and what that one refers to) and spells `AppState` as the runtime's public `KeelAppState`. A typed failure whose error is `HttpError` or `FsError` is decoded by the runtime's codec at the call site instead of the `fromReply` helper of a generated error.
* **Ports.** The standard ports are never generated. They do not claim names in the generated namespace either, so an app may have a record called `Timer` or `Log`.
* **Escape hatch.** `Generator::emit_standard_library` declares everything as ordinary items; `keel-ports` uses it to prove its own schema generates.

---

## 11. Platform runtimes

Shared responsibilities (each runtime): load/attach the core; own `call_id` allocation; map replies to continuations/promises; hold the **mirror** (store handle → signal id → decoded value) and apply change-sets on the main thread with per-frame coalescing; implement `Observe`/`Release`; provide default adapters; expose a `Transport` abstraction with `inproc` and `remote` (WebSocket) implementations (TS adds `worker`); implement the wire codecs; enforce the schema-hash check at attach with a clear error (`KeelSchemaMismatch { expected, got }`).

Main-thread delivery: Swift `MainActor`; Kotlin `Dispatchers.Main.immediate` (falls back to a single-thread executor on JVM without Android); TS `queueMicrotask` batching into one `flush()` per macrotask.

Handle lifetime: explicit `close()`/`[Symbol.dispose]`; finalizers (`deinit`, `Cleaner`, `FinalizationRegistry`) as backstop; `KeelCore.stats()` exposes live handle counts.

Default adapters:
| Port | Swift | Kotlin (Android) | Kotlin (JVM) | TS (browser) | TS (node) |
|---|---|---|---|---|---|
| Http | URLSession | OkHttp (optional dep) or HttpURLConnection | HttpURLConnection | fetch | fetch |
| Kv / SecureStore | files in Application Support / Keychain | SharedPreferences-backed files / EncryptedFile (Keystore) | files | IndexedDB / IndexedDB + WebCrypto | files |
| Fs | FileManager | Context.filesDir | java.io | OPFS | fs |
| Clock, Rng, Log | Foundation / SecRandom / os_log | System / SecureRandom / Log | same | built-in (§7) | built-in |
| Timer | DispatchQueue | Handler / ScheduledExecutor | ScheduledExecutor | setTimeout | setTimeout |
| Connectivity / Lifecycle | NWPathMonitor / scenePhase | ConnectivityManager / ProcessLifecycleOwner | stubs | navigator.onLine / visibilitychange | stubs |

---

## 12. Diagnostics

Macro errors use stable codes and a fixed shape: `error[keel::E00NN]: <what>` + a note `<why>` + `help: <fix>` + `docs: https://keel.dev/errors/E00NN`. Initial catalogue:

| Code | Trigger |
|---|---|
| E0001 | unsupported type in a public position (lists the type and the allowed set); includes `Lazy<T>` (lazy lists are not available in v1), `Handle` (no schema type; objects cross through constructors), a `Result` whose error type is not a `#[keel::error]` enum, and a type used as a value that cannot cross (an `Encode`/`Decode` bound that is not met) |
| E0002 | generic parameter on a `#[keel::api]` item |
| E0003 | lifetime in a public signature |
| E0004 | trait object / `dyn` / `Box<dyn Fn>` |
| E0005 | `Result` or `Stream` outside return position |
| E0006 | map key type not allowed |
| E0007 | unsupported item shape (a tuple or unit struct, a record without fields, an empty enum, an impl item that is neither a method nor a constructor, a receiver that is not `&self`, a store that is not a struct with named fields, the reserved field name `__keel_cell`, `#[keel::port]` on an inherent impl, `#[keel::api]` on a method, `#[keel::query]` or `#[keel::mutation]` inside an `impl` block) |
| E0008 | unknown or misplaced `#[keel(..)]` attribute or macro argument (an unknown key, a value of the wrong kind or a value on a flag, `key` on a signal that is not a `Signal<Vec<T>>`, `#[cfg]` on a public item, an invalid `crate = ".."` path); a `cfg_attr` whose attributes are all documentation or lint levels is accepted |
| E0010 | `#[keel::error]` variant without `#[error(..)]`, a message with an implicit `{}` (write `{0}` or `{name}`), `#[error]`, `#[from]` or `#[source]` on a plain `#[keel::api]` enum |
| E0011 | `#[keel::store]` and its `#[keel::api(store)]` impl block disagree, including a store with no impl block (macros); a store without a constructor (meta, bindgen) |
| E0012 | trait object in a record field (the blueprint example) |
| E0013 | store cannot be restored automatically: it has a `Computed` field, or a field that is neither a signal, a `Ctx` nor `Default`, and no `#[keel::store(restore = "..")]` hook |
| E0020 | `&mut self` receiver |
| E0021 | `self` by value |
| E0022 | non-`Send` future in an async method |
| E0030 | port method with a non-wire parameter |
| E0031 | event port method with a return type |
| E0032 | invalid port trait shape (an `async` method on a `sync` port, a parameter that is not a plain name, an associated type or const, no `&self` receiver) |
| E0033 | the error type of a `Result` port method has no `From<PortError>` (an unavailable port cannot be reported) |
| E0040 | query without `key` / mutation with `stale` / a query argument of the wrong kind |
| E0041 | query or mutation function with an invalid signature (not `async`, no `ctx: &Ctx` first parameter, not returning `Result<T, E>`, a stream result, `self`) |
| E0042 | query whose success value is `()` or an `Option` (a query caches a value; use a mutation for effects) |
| E0050 | duplicate type name (bindgen) |
| E0051 | a name that collides after case conversion (`a_b` and `aB`) or is not an identifier in a target language (bindgen) |
| E0052 | a record, enum, error, object or port named like a standard library item (section 8) but with another id: the runtimes implement the standard items under those names and ids (bindgen) |
| E0060 | a spelling that looks like a built-in Keel type (`Bytes`, `Uuid`, `String`, `Vec`, ..) is a different type; reported at the field or parameter by the same-type assertion the macros emit |
| E0061 | the schema records a name that the type written there does not have: an alias (`type Todo = Item`), a renamed import (`use m::Item as Todo`) or a type that is not declared with `#[keel::api]`; reported by a const assertion comparing `KEEL_TYPE_ID` |
| E0062 | a port call had no adapter (runtime message of a method without an error channel; a typed error for one with a `Result`) |
| E0063 | `Option<Option<T>>` in a public position |
| E0064 | an object (`#[keel::api] impl`) used as a field, parameter or return value: objects cross by handle |

---

## 13. Crate and package layout

```
Cargo.toml (workspace, resolver 3)
crates/keel-meta        no unsafe; deps: serde, serde_json, inventory
crates/keel-wire        no unsafe; deps: none (proptest dev-dep)
crates/keel-macros      proc-macro; deps: syn 2 (full), quote, proc-macro2
crates/keel-signals     no unsafe; deps: keel-wire, parking_lot
crates/keel-runtime     no unsafe; deps: keel-meta, keel-wire, keel-signals, parking_lot, slab, inventory, futures-core, pin-project-lite
crates/keel-ports       no unsafe; deps: keel-runtime, keel-macros (uses its own macros)
crates/keel-query       no unsafe; deps: keel-runtime, keel-ports
crates/keel-ffi         unsafe allowed; deps: keel-runtime; features: jni (jni crate), wasm
crates/keel-transport   no unsafe; deps: keel-runtime, tungstenite (feature server); WebSocket server + framing
crates/keel-bindgen     no unsafe; deps: keel-meta, serde_json, heck
crates/keel-cli         deps: keel-bindgen, clap, notify, libloading (loads the host cdylib to extract the schema)
crates/keel             facade: re-exports prelude, macros, runtime, ports, query; `dev::serve()`
runtimes/swift/KeelRuntime          Package.swift, Sources/KeelRuntime, Sources/KeelFFI (module map), Tests
runtimes/kotlin/keel-runtime        settings.gradle.kts; modules: runtime (JVM+Android), android-adapters
runtimes/ts/@keel/runtime           package.json (ESM, exports: ., ./react, ./vue, ./svelte, ./solid, ./worker, ./node), src/, test/
examples/playground/core            the Rust core used by every playground app and by the contract tests
examples/playground/{ios,android,web}
contract-tests/                     schema fixture + per-language runners + the shared scenario list
bench/                              criterion (Rust), node bench, JVM bench, iOS bench target notes
```

Schema extraction: `keel-cli` builds the core for the host as a cdylib, `dlopen`s it, calls `keel_schema_json`, and runs bindgen. Fallback: `keel bindgen --schema schema.json`.

---

## 14. Quality gates

* `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`, `cargo doc --no-deps` warning-free.
* `#![forbid(unsafe_code)]` in every crate except `keel-ffi`.
* proptest round-trips for every wire type; byte-fuzz on `Reader`, `Envelope::parse`, change-set and patch decoders.
* Golden tests for bindgen (three languages).
* Contract scenarios (`contract-tests/scenarios.md`) executed by each runtime's test suite against the playground core: primitives round-trip, records/enums/errors, sync call, async call, error propagation, cancellation, stream with backpressure, store observe → initial change-set, transaction → single change-set, keyed patch, computed, query fetch/stale/refetch, optimistic mutation rollback, offline queue replay, snapshot/restore, schema mismatch rejection, panic containment.
* Benchmarks (criterion): wire encode/decode per type, dispatch overhead, change-set build for 100 signals, keyed patch on 10k items. Cross-boundary benchmarks per runtime with the numbers written to `bench/RESULTS.md`.
* Every `pub` item documented. Every crate has a README with a 30-line example.

---

## 15. Conventions

* Edition 2024. `#[unsafe(no_mangle)]` spelling. MSRV 1.85.
* Errors: `thiserror`-style enums; no `anyhow` in library crates.
* No `println!`; use the `Log` port through `keel_runtime::log!`.
* Determinism: no `std::time::SystemTime::now()`, `Instant::now()`, `rand`, or threads spawned outside `keel-runtime`; Clock/Rng/Timer ports only (Constitution R12). The one exception is the native default `Timer`/`Clock` binding inside `keel-runtime`, gated behind `cfg(not(target_family = "wasm"))`.
* Commit messages: `type(scope): summary` — `feat`, `fix`, `test`, `docs`, `bench`, `state`, `chore`.

---

## 16. Internal Rust contracts (between keel-signals, keel-runtime, keel-macros)

These are the exact names the macros emit and the runtime consumes, as merged. They are exercised end to end by `crates/keel/tests/e2e_todo.rs` and, per crate, by the macros' behaviour tests (`crates/keel-macros/tests`), which compile and run generated code against the real runtime and signals. Change them only together.

### 16.1 keel-signals

```rust
// ---- the per-store table -------------------------------------------------------------------
pub struct StoreCell { .. }                      // one per store instance; Send + Sync
impl StoreCell {
    pub fn new(type_id: u32) -> Arc<StoreCell>;
    // Binding. Generated code calls exactly one of the next three per signal field, in
    // declaration order (ids 0, 1, 2, ..), and every one can fail (see SignalsError).
    pub fn attach<T: SignalValue>(self: &Arc<Self>, signal: &Signal<T>, signal_id: u32) -> Result<(), SignalsError>;
    pub fn attach_keyed<T: SignalValue + ListLike>(self: &Arc<Self>, signal: &Signal<T>, signal_id: u32, key: KeyFn<T>) -> Result<(), SignalsError>;   // #[keel(key = "..")]
    pub fn attach_computed<T: SignalValue>(self: &Arc<Self>, computed: &Computed<T>, signal_id: u32) -> Result<(), SignalsError>;
    pub fn set_no_coalesce(&self, signal_id: u32) -> Result<(), SignalsError>;   // #[keel(no_coalesce)]; called after that signal's attach
    pub fn set_handle(&self, handle: u64);       // the runtime, when the store enters the object table (and restore)
    pub fn handle(&self) -> u64;                 // 0 until published
    pub fn type_id(&self) -> u32;
    pub fn signal_count(&self) -> u32;           // attached signals, computeds included
    pub fn is_observed(&self, signal_id: u32) -> bool;
    pub fn observe(&self, signal_id: u32 /* or ALL_SIGNALS */, on: bool, out: &mut keel_wire::Writer) -> u32;
        // on: appends one ChangeSet ENTRY per targeted signal (`handle u64, signal_id u32, op u8 = Full, len u32, value`)
        // with its current value, also for already observed ones (re-observing resynchronises a host), and returns the count;
        // the runtime wraps the entries into a payload (`txn_id u64, count u32, entries`). off: stops delivery, returns 0.
        // on runs inside a transaction (ADR-019): a computed's closure that writes signals while it is evaluated does not commit ahead of the entries; the entries are re-encoded (at most 8 passes) until no target was written meanwhile, so they carry the post-write values and the writes' own commit finds those targets clean. Writes to slots that were not targeted commit normally afterwards.
    pub fn observe_and_deliver(&self, signal_ids: &[u32] /* ids, or ALL_SIGNALS */, deliver: impl FnOnce(&[u8])) -> u32;
        // observe(on) plus delivery, for callers that must put the values in front of the host themselves (the runtime's `observe` and `restore`, ADR-023): ids are deduplicated and the entries ordered by signal_id; ONE complete change-set (`txn_id u64, count u32, entries`) is built and handed to `deliver` UNDER the store's delivery lock, with the `txn_id` allocated under it and recorded in the store's last txn (so a commit holding an older shared id replaces it). A commit of the same store on another thread therefore delivers completely before it (older values) or waits and delivers after it, and the host's last word is the newest value. Returns the entry count; 0 (for unknown ids) means `deliver` was not called. Same settle loop and transaction as `observe(on)` (leftover writes commit after the lock is released). A panic while encoding or in `deliver` rolls the observe back. `deliver` follows the ChangeSink contract (must not wait for another thread writing this store) and must not call the cell's `observe` family or commit this store from the same thread.
    pub fn encode_signal(&self, signal_id: u32, out: &mut keel_wire::Writer) -> bool;   // full value, no header; false if unknown
    pub fn encode_snapshot(&self, out: &mut keel_wire::Writer);   // one store record, §5.9: handle u64, type_id u32, signal_count u32, signals × { signal_id u32, len u32, value }; computeds left out
}
pub const ALL_SIGNALS: u32 = u32::MAX;

pub struct CellSlot { .. }                       // Default + Debug + Send + Sync, deliberately not Clone
impl CellSlot {                                  // the hidden field `#[keel::store]` adds: empty until first use, then one cell for the store's life
    pub const fn new() -> CellSlot;
    pub fn get(&self) -> Option<&Arc<StoreCell>>;
    pub fn get_or_init(&self, init: impl FnOnce() -> Arc<StoreCell>) -> &Arc<StoreCell>;
    pub fn get_or_try_init<E>(&self, init: impl FnOnce() -> Result<Arc<StoreCell>, E>) -> Result<&Arc<StoreCell>, E>;   // racing threads agree on one cell; on Err the slot stays empty
}

#[non_exhaustive] pub enum SignalsError {        // Display + std::error::Error; typed values, never panics
    AlreadyAttached,                             // the signal belongs to a store (this one or another) for its whole life
    OutOfOrder { expected: u32, got: u32 },      // signals attach in declaration order
    UnknownSignal { signal_id: u32 },            // set_no_coalesce on an id that was not attached
}

pub type KeyFn<T> = fn(&<T as ListLike>::Item) -> u64;   // keyed lists: the generated fn hashes the encoded key field with fnv1a64
pub trait ListLike { type Item: SignalValue; fn items(&self) -> &[Self::Item]; }   // implemented for Vec<I>
pub trait SignalValue: keel_wire::Encode + Clone + Send + Sync + 'static {}
impl<T: keel_wire::Encode + Clone + Send + Sync + 'static> SignalValue for T {}

// ---- reactive primitives -------------------------------------------------------------------
pub struct Signal<T>;      // Clone = the same signal
impl<T: SignalValue> Signal<T> {
    pub fn new(value: T) -> Signal<T>;
    pub fn get(&self) -> T;                       // clone
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R;
    pub fn set(&self, value: T);                  // implicit transaction if none is open
    pub fn update(&self, f: impl FnOnce(&mut T));  // f runs with the value write-locked: it must not read or write this signal, nor read a Computed derived from it (transitive); a violation panics instead of deadlocking (ADR-021)
    pub fn ptr_eq(&self, other: &Signal<T>) -> bool;
    pub fn is_attached(&self) -> bool;
}
pub struct Computed<T>;
impl<T: SignalValue> Computed<T> {
    pub fn new<D: Deps>(deps: D, f: impl for<'a> Fn(D::Values<'a>) -> T + Send + Sync + 'static) -> Computed<T>;
        // Deps: `&Signal<A>` or `&Computed<A>`, or a tuple of up to 6 of them; the closure receives REFERENCES
        // (one reference, or a tuple of references): `Computed::new((&todos, &filter), |(todos, filter)| ..)`.
    pub fn get(&self) -> T;                       // recomputes lazily when dirty; a cycle through a handle the closure captured (not a declared dependency) panics with "computed cycle detected" instead of overflowing the stack (ADR-021)
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R;
    pub fn ptr_eq(&self, other: &Computed<T>) -> bool;
    pub fn is_attached(&self) -> bool;
}
pub struct Effect;         // Effect::new(deps, f) runs f after each commit that dirtied a dep; dropped or `cancel(self)` = cancelled
pub fn txn<R>(f: impl FnOnce() -> R) -> R;        // batch; nested calls join the outer transaction; exception safe
pub trait ChangeSink: Send + Sync { fn deliver(&self, change_set: &[u8]); fn round_cap_hit(&self, rounds: usize) {} }   // round_cap_hit: default no-op; the runtime's sink logs it at error level (ADR-020)
pub fn set_sink(sink: Arc<dyn ChangeSink>);      // installed by the runtime; a global, one per process
pub fn set_write_checker(f: fn() -> bool);       // installed by the runtime; a global. Debug builds assert f() before every write to a signal that is attached or has dependents ("may this thread mutate?"); release builds never call it. The runtime's checker is an allowlist (ADR-023, superseding the pool denylist of ADR-020): core-lock holders, TestRuntime driver threads and testing::unchecked_writes
pub fn clear_write_checker();
pub fn clear_sink();  pub fn with_sink<R>(sink: Arc<dyn ChangeSink>, f: impl FnOnce() -> R) -> R;   // the latter is thread-scoped, for tests
pub fn next_txn_id() -> u64;
pub mod testing { pub struct CaptureSink; }      // records change-sets: take(), take_decoded()
```

Commit algorithm: on outermost `txn` exit (or after a bare `set`), for each dirty `StoreCell` with a handle: recompute observed dirty computeds in dependency order; encode entries, ordered by `signal_id`, for signals that are observed or `no_coalesce` (keyed lists as a patch when one is possible and worthwhile, else the full value); one `ChangeSet` payload per store per transaction (never split; stores committed by one transaction share a `txn_id`); deliver via the sink; run effects; clear dirty bits. Signals that are dirty but unobserved stay marked so `observe(on)` sends fresh values. Writes before `attach`/`set_handle` are plain writes with no delivery. The claim is transactional (ADR-019): if building or delivering a store's change-set is abandoned (a computed, an encoder or the sink panicked), its slots are remembered as unsent and the keyed baselines among them are dropped, and the next commit that touches the store sends every one of them again as a full value; `observe(on)` of a slot clears its unsent mark. A failed `observe(on)` appends no entries and leaves no target newly observed. An unknown `signal_id` in `observe` is ignored (returns 0) in every build. The only lock held while user code runs is the store's **delivery lock** (ADR-020): a commit takes it when it claims the store's dirty slots and releases it when the sink has returned (`observe_and_deliver` holds it from before the entries are encoded until `deliver` has returned), so the change-sets of one store reach the sink one at a time, in claim order, with strictly increasing `txn_id` (a transaction id shared by several stores is replaced for a store that has already seen a newer one). Sinks and computed closures therefore run under it: a sink must not wait for another thread that writes the same store (writes made on the sink's own thread are queued and are fine), and the runtime's sink only hands the payload to the host. No other lock is held while user code (sink, effect, computed closure, encoder) runs, and effects run after the delivery lock is released. Writes from two threads do not form one transaction: a write to a slot that another thread has dirty in an open transaction ships with that transaction. Signal writes belong on the core; debug builds enforce it through `set_write_checker`. A commit is bounded: after 1000 rounds (effects, computeds or sinks that keep writing signals that trigger themselves) it stops running effects, delivers the changes that are already dirty one last time, drops the still-queued effects from the queue (a later change queues them again), releases whatever that delivery queued (those slots are remembered as unsent, as for an abandoned change-set), and reports `ChangeSink::round_cap_hit(1000)`. Nothing stays claimed by the capped thread, so other threads' writes to those slots and effects work normally.

Keyed lists (SPEC 3.8): the cell keeps the list as the host last saw it (one clone per *observed* keyed signal; an unobserved `no_coalesce` keyed list keeps none and is delivered as a full value each time) and sends a patch of `Insert`/`Remove`/`Update`/`Move` ops. It sends the full value instead when the lists share no key (including empty to non-empty and back), a key occurs twice, or more than half of the old items were removed. Items with equal keys are compared by their encoded bytes.

Attach failures leave the failed signal unattached, but signals attached before it stay bound to the discarded cell, so a store whose attach failed is unusable and must not be published. Generated code therefore builds the cell with `?` and the constructor's dispatch arm answers `DispatchResult::BadRequest` with the `SignalsError` text (§16.3).

### 16.2 keel-runtime

```rust
pub struct Runtime;                                 // Arc<Runtime>; Runtime::init registers the process global, Runtime::new does not (any number can coexist)
pub struct RuntimeConfig { platform: String, mode: String /* "inproc" | "dev" */, core_threads: u8, blocking_threads: u8, log_level: u8 }  // hand-written Encode/Decode
pub enum InitError { AlreadyInitialized, InvalidMode(String), Spawn(String) }
pub trait Host: Send + Sync + 'static {            // implemented by keel-ffi, the wasm shell, the transport server and testing::RecordingHost
    fn reply(&self, call_id: u32, payload: &[u8]);
    fn change_set(&self, payload: &[u8]);
    fn stream_item(&self, call_id: u32, payload: &[u8]);
    fn port_call(&self, port_id: u32, method_id: u32, port_call_id: u32, args: &[u8]) -> PortCallOutcome;
    fn log(&self, level: u8, target: &str, message: &str);
    fn schedule(&self) {}                           // wasm and manual runtimes: ask the host to call poll() soon
    fn timer_set(&self, timer_id: u32, delay_ms: u64) -> bool { false } // true if the host owns timers (wasm)
}
pub enum PortCallOutcome { Sync(Vec<u8> /* a complete PortReply payload */), Async, Unavailable }
impl Runtime {
    pub fn init(config: RuntimeConfig, host: Arc<dyn Host>) -> Result<Arc<Runtime>, InitError>;   // executor, object/port/dispatch tables, change sink, init hooks, `keel-core` thread (unless core_threads == 0)
    pub fn new(config: RuntimeConfig, host: Arc<dyn Host>) -> Result<Arc<Runtime>, InitError>;    // same, not global
    pub fn global() -> Option<Arc<Runtime>>;
    pub fn shutdown(&self);                          // §5.1 Shutdown: answers in-flight calls (status 3) and ends open streams, joins threads, clears subscribers and port bindings, breaks the runtime <-> Ctx reference cycles; not from the core or a host callback (debug assertion)
    pub fn call(&self, payload: &[u8]) -> u32;       // §3.3; 0 accepted / 5 bad request; replies through Host::reply
    pub fn call_sync(&self, payload: &[u8]) -> Vec<u8>;   // §3.4 reply payload; one allocation (the Vec), §5.6
    pub fn call_sync_with<R>(&self, payload: &[u8], read: impl FnOnce(&[u8]) -> R) -> R;   // the same reply lent to `read` (valid only inside it, core lock released): no heap allocation once the thread's reply buffer is warm, §5.6
    pub fn cancel(&self, call_id: u32);              // status 3 exactly once for a plain call
    pub fn stream_credit(&self, call_id: u32, credit: u32);
    pub fn observe(&self, handle: u64, signal_id: u32, on: bool);   // the initial change-set is delivered synchronously through Host::change_set
    pub fn release(&self, handle: u64);
    pub fn port_reply(&self, payload: &[u8]);
    pub fn event(&self, port_id: u32, method_id: u32, payload: &[u8]);
    pub fn timer_fired(&self, timer_id: u32);
    pub fn poll(&self);                              // drive the executor (wasm and manual runtimes); run_pending() runs until idle
    pub fn snapshot(&self) -> Vec<u8>;
    pub fn restore(&self, payload: &[u8]) -> Result<(), RestoreError>;   // all or nothing; cancels in-flight calls on replaced receivers (§5.9); resumes the generation counter above the snapshot's floor (ADR-022)
    pub fn stats_json(&self) -> String;
    pub fn schema(&self) -> &keel_meta::Schema; pub fn schema_hash(&self) -> u64;
    pub fn ctx(&self) -> Ctx;
    // What generated code calls:
    pub fn object<T: Send + Sync + 'static>(&self, handle: u64) -> Result<Arc<T>, object_table::BadHandle>;   // Display says null / unknown / stale / wrong type
    pub fn sync_ok<T>(&self, value: &T, encode: fn(&T, &mut Writer)) -> DispatchOutcome;   // a sync method's success (status 0): written into the armed reply slot under call_sync, else DispatchResult::Sync(Ok(..)); `encode` is `Encode::encode` (ADR-028)
    pub fn sync_err<E>(&self, error: &E, encode: fn(&E, &mut Writer)) -> DispatchOutcome;  // its typed error (status 1), likewise
    pub fn insert_object<T: KeelObject>(&self, object: Arc<T>) -> Handle;   // stores (a StoreRestorer is registered for the type) get their cell's handle set
    pub fn insert_store<T: StoreObject>(&self, object: Arc<T>) -> Handle;
    pub fn bind_port<P: ?Sized + 'static>(&self, port_id: u32, imp: Arc<dyn Any + Send + Sync>);   // imp is an Arc<Arc<P>> behind Any
    pub fn bind_dyn_port<P: ?Sized + Send + Sync + 'static>(&self, port_id: u32, imp: Arc<P>);      // does the wrapping
    pub fn bind_foreign_port(&self, port_id: u32);  pub fn unbind_port(&self, port_id: u32) -> bool;
    pub fn rust_port<P: ?Sized + Send + Sync + 'static>(&self, port_id: u32) -> Option<Arc<P>>;
    pub fn extension<T: Default + Send + Sync + 'static>(&self) -> &T;      // per-runtime state of layered crates (keel-query)
}
#[derive(Clone)] pub struct Ctx(..);   // §5.3; an Arc<Runtime>. `Ctx::current()` / `try_current()` read a thread-local set by dispatch, by the executor while polling and by `Ctx::enter()`
impl Ctx {
    pub fn txn<R>(&self, f: impl FnOnce() -> R) -> R;                       // one transaction, delivered through this runtime
    pub fn spawn(&self, fut: impl Future<Output = ()> + Send + 'static) -> TaskId;   pub fn cancel_task(&self, id: TaskId);   // the cancelled future is dropped on the core (core lock held, or queued for the core's next turn); after shutdown spawn/sleep/port_call/event are logged no-ops
    pub fn spawn_blocking<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> BlockingTask<T>;
    pub fn sleep(&self, d: Duration) -> Sleep;                              // through the host's timer or the internal one
    pub fn events(&self) -> &Events;                                        // subscribe(port_id, method_id, Box<dyn Fn(&[u8]) + Send + Sync>) -> Subscription
    pub fn port_call(&self, port_id: u32, method_id: u32, args: Vec<u8>) -> PortFuture;
    pub fn port_call_sync(&self, port_id: u32, method_id: u32, args: &[u8]) -> Result<Vec<u8>, PortError>;
    pub fn rust_port<P: ?Sized + Send + Sync + 'static>(&self, port_id: u32) -> Option<Arc<P>>;   pub fn bind_port / bind_dyn_port;
    pub fn enter(&self) -> CtxScope;  pub fn runtime(&self) -> &Runtime;
}
// The typed accessors of §5.3 are not methods: `#[keel::port]` generates a free function `<trait_snake>(ctx: &Ctx) -> Arc<dyn Trait>` (the Rust binding, else a proxy to the
// platform); `ctx.query()` / `ctx.mutate()` come from keel-query as extension traits over `Ctx`.

pub enum DispatchResult {
    Sync(Result<Vec<u8>, Vec<u8>>),                 // Ok bytes: status 0; Err bytes: status 1 (typed error). Generated dispatchers do not build this for a sync method any more: they call `Runtime::sync_ok` / `sync_err`, which return this when no reply slot is armed and otherwise write the reply into the slot (§5.6, ADR-028). Hand-written dispatchers and layers may still return it.
    Async(Pin<Box<dyn Future<Output = Result<Vec<u8>, Vec<u8>>> + Send>>),
    Stream(Pin<Box<dyn futures_core::Stream<Item = Result<Vec<u8>, Vec<u8>>> + Send>>),
    Unknown,                                        // status 5: the dispatcher does not implement this method id
    BadRequest(String),                             // status 5 with the reason: arguments that do not decode, stale or wrongly typed receiver, a store whose signals cannot attach
}
// Generated dispatchers are `keel_meta::DispatchFn = fn(&dyn Any, DispatchCall<'_>) -> DispatchOutcome`: they downcast `&dyn Any` to `&Runtime` and return `DispatchOutcome::new(DispatchResult::..)` (Async, Stream, BadRequest, Unknown) or, for a sync method's answer, `rt.sync_ok(..)` / `rt.sync_err(..)`. The outcome of those two is either a `DispatchResult::Sync` or a zero-sized runtime-private marker meaning "the reply is in this thread's slot"; `DispatchOutcome` stays an opaque `Box<dyn Any + Send>`, so `keel-meta` is unchanged and a zero-sized value boxes without allocating.
pub trait KeelObject: Send + Sync + 'static { const TYPE_ID: u32; const NAME: &'static str; }
pub trait StoreObject: KeelObject {
    fn cell(&self) -> &Arc<keel_signals::StoreCell>;
    fn restore(ctx: Ctx, r: &mut keel_wire::Reader<'_>) -> Result<Self, keel_wire::WireError> where Self: Sized;   // r is positioned at the store BODY (`signal_count u32, signals × { signal_id u32, len u32, value }`)
}
pub struct StoreRestorer {                           // submitted with `inventory::submit!` by #[keel::store], one per store type
    pub type_id: u32,
    pub restore: fn(Ctx, u64 /* the re-issued handle */, &mut Reader<'_>) -> Result<Arc<dyn Any + Send + Sync>, WireError>,   // builds the store and tells its cell the handle
    pub cell: fn(&(dyn Any + Send + Sync)) -> Option<&Arc<StoreCell>>,   // how the runtime recognises a store inside `dyn Any`
}
pub trait Port: Send + Sync + 'static { const PORT_ID: u32; const NAME: &'static str; const KIND: keel_meta::PortKind; }   // implemented for `dyn Trait`
pub struct PortDispatcher {                          // submitted by #[keel::port], one per port trait: how a Rust binding answers an encoded call
    pub port_id: u32,
    pub dispatch: fn(imp: &(dyn Any + Send + Sync), method_id: u32, args: &[u8]) -> PortDispatch,   // imp is the Arc<Arc<dyn Trait>> given to bind_port
}
pub enum PortDispatch { Sync(Vec<u8>), Async(Pin<Box<dyn Future<Output = Vec<u8>> + Send>>) }   // bytes: `status u8` (0 ok, 1 typed error, 2 unavailable) then the body
pub struct PortFuture;    // Future<Output = Result<Vec<u8>, PortError>>; dropping it abandons the call
pub fn port_call_sync(rt: &Runtime, port_id: u32, method_id: u32, args: &[u8]) -> Result<Vec<u8>, PortError>;
pub enum PortError { Unavailable, Cancelled, Decode(keel_wire::WireError), Failed(Vec<u8> /* encoded E */) }   // Clone + Debug + Display + Error
pub enum RestoreError { Decode(WireError), UnknownStoreType { type_id: u32 }, Store { type_id: u32, source: WireError }, Panicked { type_id: u32, message: String }, BadHandle { handle: u64 } /* null, duplicate, generation 0 or u32::MAX, index too far */, GenerationFloor { floor: u32 } /* floor == u32::MAX */, ShutDown, Reentrant }
pub struct InitHook { pub name: &'static str, pub run: fn(&Ctx) }   // submitted by layered crates, run for every new runtime
pub mod object_table;     // ObjectTable, BadHandle, BadHandleReason: generation-tagged slots; Handle issue/lookup/release
pub mod log;              // level constants TRACE..FATAL and `log::log(level, target, msg)`
pub mod executor;         // spawn, spawn_blocking, sleep, cancel, yield_now, Notify, TaskId
pub mod testing;          // TestRuntime: a real Runtime with no core or timer thread, a manual clock, the real blocking pool (run_pending/run_until/advance wait for its closures), a private generation counter and a RecordingHost; unchecked_writes(f) lifts the debug write check on one thread, drive_from_this_thread() marks a harness thread a driver; call_sync_reference(rt, payload) is call_sync through the allocating path (§5.6);
                          // call / call_sync / run_pending / run_until(fut) / advance(Duration) / take_replies; host(): take_decoded_change_sets, take_stream_items,
                          // take_port_calls, take_timeline, take_logs, script_port*(port_id, method_id, ..); helpers call_payload, decode_reply, port_reply, port_reply_ok, sync_ok
```
`keel-runtime` re-exports `keel_signals`, `keel_wire`, `keel_meta` (and `keel_meta::inventory`) and `futures_core::Stream`. The `keel` facade re-exports `keel_runtime as runtime`, `keel_signals as signals`, `keel_wire as wire`, `keel_meta as meta`, the six macros at the crate root and in `prelude`, `pub mod query` (`QueryDef`, `MutationDef`, `BoxFuture`, `CacheValue`, the traits keel-query implements the client against), and `prelude::*` = `{Signal, Computed, Effect, Ctx, Bytes, Uuid, Timestamp, Duration, txn, Handle}` plus the macros. (`keel_ports as ports` and `keel_query as query` join the facade when those crates land; until then `keel::query` is the trait module.)

Ids are `fnv1a` hashes computed in `keel_meta::ids`: `type_id(name)`, `method_id(type, method)` (constructors are methods called `new`, or whatever the fn is named), `function_id(name)`, `port_id(trait)`, `port_method_id(trait, method)`, `query_id`, `mutation_id`, `fnv1a64(bytes)` (keyed-list keys).

### 16.3 What the macros emit (paths)

Generated code uses absolute paths through the facade: `::keel::wire::{Encode, Decode, Writer, Reader, WireError}`, `::keel::meta::{inventory, Registration, RecordMeta, ObjectMeta, StoreMeta, SignalMeta, .., DispatchCall, DispatchOutcome, ids}`, `::keel::runtime::{Runtime, Ctx, DispatchResult, KeelObject, StoreObject, StoreRestorer, Port, PortDispatcher, PortDispatch, PortError, Stream, Subscription}`, `::keel::signals::{Signal, StoreCell, CellSlot, SignalsError}`, `::keel::query::{QueryDef, MutationDef}`. A `#[keel(crate = "path")]` attribute (or `crate = "path"` in the macro arguments) overrides the root (for keel-ports and tests inside the workspace, which use `::keel_runtime` directly).

Every type position the schema records is also checked against the type it resolves to, at compile time, in the user's crate: built-in mappings by a same-type assertion (E0060), `Named` mappings by comparing the inherent `KEEL_TYPE_ID` of the type with `type_id` of the recorded name (E0061), with `KEEL_IS_ERROR` required on the error side of a `Result` and `__KEEL_IS_OBJECT` refusing an object used as a value (E0064). `impl StoreObject` is written by the `#[keel::api(store)]` impl block next to `impl KeelObject`, forwarding to hidden members of the struct.

`#[keel::store]` and its impl block:

* The struct must be marked on its impl block: `#[keel::api(store)] impl Todos { .. }`. The marker is what wires the constructors to the signals; a struct without `#[keel::store]`, or an impl block without the marker, is E0011.
* The macro appends a hidden field `pub __keel_cell: ::keel::signals::CellSlot`. Struct literals of the type inside its `#[keel::api(store)]` impl block get `__keel_cell: Default::default()` added; struct literals anywhere else must spell it out. The field name is reserved (E0007).
* `#[keel::store(restore = "Self::assemble")]` names the function `restore` rebuilds the store with: `fn(ctx: Ctx, <one Signal<T> per non-computed signal, in declaration order>) -> Self`, the same code the constructor uses. Without it the store is rebuilt by a struct literal, which works only when every non-signal field is a `Ctx` (cloned from the argument) or `Default`; a store with a `Computed` field needs the hook (E0013). Snapshots hold the plain signals only.
* Field kinds: `Signal<T>` and `Computed<T>` are signals, numbered in declaration order; every other field is private state. `Lazy<T>` is rejected in v1 (E0001, "lazy lists are not available in v1"), like `keel-bindgen` rejects it.
* The signal table is built by a hidden method that calls, per field and in order, `attach` (plain), `attach_keyed` with a generated `fn(&Item) -> u64` (`#[keel(key = "id")]`: `fnv1a64` over the encoded key field, encoded through a per-thread scratch buffer) or `attach_computed`, each followed by `set_no_coalesce(id)` for `#[keel(no_coalesce)]`, all with `?`. `StoreObject::cell()` creates the cell once through `CellSlot::get_or_init`; a store whose attach failed gets an empty cell there rather than a panic.
* A constructor's dispatch arm first calls `__keel_attach_all()` (`CellSlot::get_or_try_init`); on `Err(SignalsError)` nothing is inserted and the arm answers `DispatchResult::BadRequest("store `Todos` could not attach its signals: <error>")`. Otherwise it inserts the object (`Runtime::insert_object`, which gives the cell its handle) and replies with the handle (`u64`). `restore` fails with `WireError::InvalidTag` if the rebuilt store cannot attach.
* One `StoreRestorer` per store type is submitted through `inventory`.

Dispatch arms answer `BadRequest` with a reason for arguments that do not decode (naming the argument and `Type.method`) and for a receiver handle that does not resolve to that type (the `BadHandle` text); `Unknown` is only for a method id the dispatcher does not implement.

A synchronous arm ends in `__rt.sync_ok(&value, ::keel::wire::Encode::encode)` (a `Result<T, E>` method: `Ok(v)` goes to `sync_ok`, `Err(e)` to `sync_err`, a constructor answers its handle with `sync_ok`), which is a `DispatchOutcome` already; an `async` or stream arm wraps its `DispatchResult::{Async, Stream}` in the dispatcher's `__keel_out(..)`. The encoder is passed as the path `Encode::encode`, not as an `Encode` bound on `sync_ok`, so a type that cannot cross the boundary is still reported by the one E0001 diagnostic of §12 and not by a second "required by a bound in `Runtime::sync_ok`" note (ADR-028).

---

## 17. Platform runtime base API (what generated code depends on)

Generated code calls only these names. Runtimes implement them; bindgen golden files pin the usage.

### 17.1 TypeScript (`@keel/runtime`)

```ts
export class KeelCore {
  static load(opts: LoadOptions): Promise<KeelCore>;          // { mode: 'wasm-main' | 'wasm-worker' | 'remote', wasm?: URL | BufferSource, url?: string /* ws:// for remote */, adapters?: Partial<Adapters>, expectedSchemaHash: bigint }
  static get shared(): KeelCore;                               // set by the first load; throws if none
  callSync(target: CallTarget, methodId: number, args: Uint8Array): Uint8Array;        // only mode 'wasm-main'; others throw KeelModeError
  call(target: CallTarget, methodId: number, args: Uint8Array, signal?: AbortSignal): Promise<Uint8Array>;   // resolves with reply body (status ok) or rejects with KeelReplyError { status, body }
  stream(target: CallTarget, methodId: number, args: Uint8Array): AsyncIterable<Uint8Array>;   // handles credit
  construct(typeId: number, methodId: number, args: Uint8Array): Promise<bigint>;     // returns handle
  observe(handle: bigint, signalId: number, on: boolean): void;
  release(handle: bigint): void;
  mirror: Mirror;      // mirror.register(handle, applyFn: (signalId, op, value: Uint8Array) => void); mirror.unregister(handle)
  registerPort(portId: number, impl: PortImpl): void;          // PortImpl = { methods: Record<number, (args: Uint8Array) => Uint8Array | Promise<Uint8Array>>, sync: boolean }
  stats(): Promise<KeelStats>;
}
export abstract class KeelObject { protected constructor(core: KeelCore, handle: bigint); readonly core; readonly handle; close(): void; [Symbol.dispose](): void }
export abstract class KeelStore extends KeelObject { protected _signals: Signal<unknown>[]; protected _apply(signalId: number, op: ChangeOp, value: Uint8Array): void /* implemented by generated code */; }
export class Signal<T> { get(): T; peek(): T; subscribe(fn: (v: T) => void): () => void; /* internal */ _set(v: T): void }
export class KeelError extends Error { readonly kind: string }
export class KeelReplyError extends KeelError { status: ReplyStatus; body: Uint8Array }
export interface KeelPort {}
```
Ports: generated port interfaces are plain TS interfaces; `core.registerPort(id, generatedAdapter(impl))` wraps an implementation with codecs (bindgen emits the adapter).

### 17.2 Kotlin (`dev.keel.runtime`)

```kotlin
class KeelCore private constructor(...) {
  companion object { fun load(options: LoadOptions): KeelCore; val shared: KeelCore }   // LoadOptions(mode = Mode.INPROC | Mode.REMOTE, remoteUrl, adapters, expectedSchemaHash: ULong)
  fun callSync(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray            // reply body or throws KeelReplyException
  suspend fun call(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray         // cancellable
  fun stream(target: CallTarget, methodId: UInt, args: ByteArray): Flow<ByteArray>
  fun construct(typeId: UInt, methodId: UInt, args: ByteArray): Long                        // sync in INPROC
  fun observe(handle: Long, signalId: UInt, on: Boolean); fun release(handle: Long)
  val mirror: Mirror                                                                        // register(handle) { signalId, op, reader -> }
  fun registerPort(portId: UInt, impl: PortImpl)
  fun stats(): KeelStats
}
abstract class KeelObject(val core: KeelCore, val handle: Long) : AutoCloseable
abstract class KeelStore(core: KeelCore, handle: Long) : KeelObject(core, handle) { protected abstract fun apply(signalId: UInt, op: ChangeOp, reader: KeelReader); protected fun <T> signal(initial: T): MutableStateFlow<T> }
open class KeelException(message: String) : RuntimeException(message)
class KeelReplyException(val status: ReplyStatus, val body: ByteArray) : KeelException(..)
interface KeelPort
```
Main-thread delivery through `KeelDispatchers.main` (Android: `Dispatchers.Main.immediate`; JVM: a single-thread executor).

### 17.3 Swift (`KeelRuntime`)

```swift
public final class KeelCore: @unchecked Sendable {
  public static func load(_ options: LoadOptions) throws -> KeelCore     // .inproc(adapters:) | .remote(url:adapters:), expectedSchemaHash
  public static var shared: KeelCore { get }
  public func callSync(_ target: CallTarget, method: UInt32, args: [UInt8]) throws -> [UInt8]
  public func call(_ target: CallTarget, method: UInt32, args: [UInt8]) async throws -> [UInt8]   // cancellation-aware
  public func stream(_ target: CallTarget, method: UInt32, args: [UInt8]) -> AsyncThrowingStream<[UInt8], Error>
  public func construct(type: UInt32, method: UInt32, args: [UInt8]) throws -> KeelHandle
  public func observe(_ handle: KeelHandle, signal: UInt32, on: Bool); public func release(_ handle: KeelHandle)
  public let mirror: Mirror        // register(handle) { @MainActor (signalId, op, reader) in … }
  public func registerPort(_ id: UInt32, _ impl: PortImpl)
  public func stats() -> KeelStats
}
open class KeelObject: @unchecked Sendable { public init(core: KeelCore, handle: KeelHandle); public func close() }
@MainActor open class KeelStore: KeelObject { open func apply(signal: UInt32, op: ChangeOp, reader: inout KeelReader) }   // generated subclass is @Observable
public struct KeelReplyError: Error { public let status: ReplyStatus; public let body: [UInt8] }
public protocol KeelRecord: KeelCodec, Sendable, Hashable {}; public protocol KeelEnum: KeelCodec, Sendable, Hashable {}; public protocol KeelError: KeelCodec, Error, Sendable, Hashable {}; public protocol KeelPort {}
```
Swift payload types live under `enum Wire { … }` (`Wire.Log`, `Wire.Event`, …) to avoid clashing with generated port protocols.
