# Undra v1 — Implementation Specification

This is the binding technical specification for Undra v1. Every crate, runtime and generated file conforms to it. When code and this document disagree, the code is wrong until an ADR changes the document.

The product design lives in the blueprint (`docs/BLUEPRINT.md`); this file is the engineering contract that lets independent workers build the pieces in parallel and have them fit.

---

## 0. Scope of v1

In scope: everything under the pixels.

* Rust core crates: `undra-meta`, `undra-wire`, `undra-macros`, `undra-signals`, `undra-runtime`, `undra-ports`, `undra-query`, `undra-ffi`, `undra-transport`, `undra-bindgen`, `undra-cli`, `undra` (facade).
* Platform runtimes: Swift (`runtimes/swift/UndraRuntime`), Kotlin (`runtimes/kotlin/undra-runtime`), TypeScript (`runtimes/ts/@undra/runtime`); React Native (`runtimes/rn/@undra/react-native`, v1.2, ADR-038) is a fourth host of the C ABI under the TypeScript runtime (§11.2).
* Generated bindings for records, enums, errors, objects, stores, ports, sync/async methods, streams, signals.
* Reactive state: signals, computed, transactions, change-sets, keyed list patches, observation.
* Data layer: query cache, mutations with optimistic patches and rollback, invalidation, retry, persistence, offline queue.
* Ports: Http, Kv, SecureStore, Fs, Clock, Rng, Log, Timer, Connectivity, Lifecycle, Diagnostics. Default adapters on each platform and Rust fakes.
* Production operations (ADR-046): a structured panic report for the app's crash reporter (§5.6, `Diagnostics`, §8), symbol files from every release build and `undra symbolicate` (§13), background runs through the standard function `run_background` (§5.11, §8) with the iOS and Android helpers (§17), and a debugger path into Rust per platform.
* Dev loop: `undra dev` remote core over WebSocket; devtools protocol messages (inspector UI is a stretch goal).
* Playground app on all three platforms, benchmarks, contract tests.

Out of scope for v1: sync engine, hosted services, desktop targets beyond macOS-via-Swift, shared UI of any kind, Rust-owned SQLite (Kv is a foreign port in v1, see ADR-014).

Toolchain baseline: Rust 1.85+ (edition 2024), Swift 6.0 / iOS 17+, Kotlin 2.0 / Android API 26+ (NDK r27, 16 KB pages), TypeScript 5.5 / ES2022, Node 20+.

---

## 1. Core concepts and identifiers

| Concept | Definition |
|---|---|
| **Record** | A `struct` with `#[undra::api]`. Crosses by value. |
| **Enum** | An `enum` with `#[undra::api]`. Unit variants or data variants. Crosses by value. |
| **Error** | An `enum` with `#[undra::error]`. Like an enum, plus `std::error::Error` + `Display`. |
| **Object** | A type whose `impl` block has `#[undra::api]`. Crosses by handle. Methods are sync or async. |
| **Store** | An object whose struct has `#[undra::store]`. Has signal fields the platforms mirror. |
| **Port** | A trait with `#[undra::port]`. Implemented by the platform (foreign) or by a Rust fake. |
| **Query / Mutation** | An `async fn` with `#[undra::query]` / `#[undra::mutation]`. Managed by `undra-query`. |
| **Function** | A free `fn` with `#[undra::api]`. Crosses like a method with no receiver. |

### 1.1 Stable identifiers

All identifiers are computed at compile time by the macros and embedded in the schema, so every platform agrees without a registry lookup.

* `fnv1a32(s)` / `fnv1a64(s)`: FNV-1a over the UTF-8 bytes of `s`, offset basis `0x811c9dc5` / `0xcbf29ce484222325`, prime `0x01000193` / `0x100000001b3`.
* **type_id** (`u32`) = `fnv1a32("<TypeName>")` where `TypeName` is the Rust identifier, no module path. Type names must be unique within a core crate; the macro cannot check this, `undra-bindgen` does and fails on collision.
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

## 2. Schema (`undra-meta`)

`undra-meta` has zero dependencies except `serde` + `serde_json` (feature `serde`, on by default). It defines the data model below, JSON (de)serialization, the canonical form, the hash, and the FNV helpers. `undra-macros` emits it, `undra-bindgen` consumes it, `undra-ffi` exports it.

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

Rules enforced by the macro (error codes in §12): map keys must be `String`, integers, `Bool` or `Uuid`; `Result` and `Stream` only in return position; `Option<Option<T>>` rejected (Kotlin cannot express nested optionality: `Some(None)` and `None` would both arrive as `null`; E0063); `Lazy<T>` only as a store signal type. The error side of a `Result` is a `Named` `#[undra::error]` enum. A `Named` reference is checked against the type it resolves to at compile time (E0060, E0061, §12), so an alias or a type that shadows a built-in name cannot make the schema disagree with the wire.

### 2.2 Definitions

```rust
pub struct Schema {
    pub undra_version: String,          // "1.0.0"
    pub crate_name: String,            // Cargo package name of the core
    pub records: Vec<RecordDef>,
    pub enums: Vec<EnumDef>,           // includes errors (is_error = true)
    pub objects: Vec<ObjectDef>,       // includes stores (store = Some(..))
    pub functions: Vec<FunctionDef>,
    pub ports: Vec<PortDef>,
    pub queries: Vec<QueryDef>,
}

pub struct RecordDef { pub name: String, pub type_id: u32, pub fields: Vec<FieldDef>, pub docs: String }
pub struct FieldDef  { pub name: String, pub ty: TypeRef, pub default: bool /* #[undra(default)] */, pub docs: String }

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
pub struct SignalDef { pub name: String, pub signal_id: u32, pub ty: TypeRef, pub computed: bool, pub key: Option<String> /* #[undra(key = "id")] */, pub no_coalesce: bool /* #[undra(no_coalesce)]; serialized only when true (ADR-031) */, pub default: bool /* #[undra(default)] on a Signal<T>; serialized only when true (ADR-037) */ }
// `computed: true` with a `key` (and `ty: Vec<T>`) is a derived keyed list (`DerivedList<T>`, ADR-039): read-only on
// the platforms, delivered as keyed patches (§3.8). No other field describes it, so no existing schema or hash changed.

pub struct FunctionDef { pub name: String, pub method_id: u32, pub params: Vec<ParamDef>, pub returns: TypeRef, pub is_async: bool, pub takes_ctx: bool, pub docs: String }

pub struct PortDef { pub name: String, pub port_id: u32, pub kind: PortKind /* Sync | Async | Event */, pub methods: Vec<MethodDef>, pub docs: String }

pub struct QueryDef { pub name: String, pub query_id: u32, pub kind: QueryKind /* Query | Mutation */, pub key: String, pub params: Vec<ParamDef>, pub returns: TypeRef, pub stale_ms: Option<u64>, pub persist: bool, pub idempotent: bool }
```

### 2.3 Canonical JSON and the hash

Canonical form: `serde_json` with all `Vec`s sorted by `name` (variants keep declaration `index` and are sorted by index), map keys in struct-field order as declared above, no whitespace, `docs` fields **excluded**, `SignalDef.no_coalesce` and `SignalDef.default` written only when `true` (so a schema without such a signal hashes as it did before the field existed, ADR-031, ADR-037). `schema_hash = fnv1a64(canonical_bytes)`. `undra-meta` exposes `Schema::canonical_json()` and `Schema::hash()`. Two cores with the same public surface produce the same hash regardless of doc comments or source order of *unordered* things. Ordered (part of the wire layout, kept in declaration order): record fields, variant fields, params, signals (signal_id), variants (index). Unordered (sorted by name in canonical form): the six top-level lists, object methods and constructors, port methods. `crate_name` and `undra_version` are labels and are **excluded** from the canonical form (they do not change the wire).

Exchange form: `Schema::to_json()` (compact) and `Schema::to_json_pretty()` are the *whole* schema, `undra_version` and `crate_name` labels and `docs` included, every list in declaration order; `Schema::from_json` reads either. This is the document `undra_schema_json` returns (§6; ADR-050), `undra-dev-runner --print-schema` prints and a `schema.json` file holds. The hash does not cover docs or labels, so it is the `fnv1a64` of the **canonical** form, not of the exchange form's bytes: a host that wants to check `undra_schema_hash` against the JSON it was given recomputes the canonical form (`Schema::from_json(..).hash()` in Rust; drop `docs` and the labels and sort the unordered lists elsewhere, as `crates/undra-ffi/tests/wasm/raw.test.mjs` does) and hashes that. `Schema::without_docs()` removes every doc and leaves the hash unchanged. Docs reach a binary through the registrations (§2.4), unconditionally: they are `&'static str` fields of the `*Meta` statics, so exporting them adds no data that the core did not already carry.

### 2.4 Registration (`inventory`)

Each macro emits `inventory::submit! { undra_meta::Registration::Record(&RECORD_DEF) }` etc., where the def is a `static` built from `const` data (`&'static str`, `&'static [..]`). `undra_meta::Registration` is:

```rust
pub enum Registration { Record(&'static RecordMeta), Enum(&'static EnumMeta), Object(&'static ObjectMeta), Function(&'static FunctionMeta), Port(&'static PortMeta), Query(&'static QueryMeta) }
inventory::collect!(Registration);
pub fn collect_schema(crate_name: &str) -> Schema  // builds the owned Schema from all registrations
```

`*Meta` are `'static`, const-constructible mirrors of the `*Def` types (using `&'static [T]` instead of `Vec`) so they can live in statics. `undra-meta` provides `impl From<&RecordMeta> for RecordDef` etc.

On `wasm32-unknown-unknown` the TS runtime calls the exported `_initialize` (or `__wasm_call_ctors`) once after instantiation so `inventory` registrations run (see §7).

Dispatch registration uses the same mechanism: `Registration::Object` carries a `dispatch: fn(&Runtime, DispatchCall) -> DispatchResult` pointer and `Registration::Function` likewise (§5.6).

### 2.5 Type closures and fingerprints (ADR-037)

Persisted data (a snapshot, a cached query result, a queued mutation's input) records the structure it was written with, so a later build can read it by name. `undra-meta` defines a **type closure** (`TypeClosure { root, records, enums }`): a root, which is a type (`Type { ty }`: a query's cached success type, the target of a `ty` hook), named parameters (`Params { params }`: a mutation's input) or a store's non-computed signals (`Signals { signals }`, by name, `signal_id`, type and `default`), plus every record and enum the root reaches, transitively. Its canonical JSON is `serde_json` with no whitespace, struct fields in declaration order, records and enums sorted by name, variants by index, record and variant fields in wire order with `name`, `ty` and `default` (written only when `true`), variants with `name`, `index`, `fields`, `tuple`; docs, error messages and type ids are not part of it. The **fingerprint** is `fnv1a64` of that JSON. It moves exactly when the encoded structure of the item can have changed: a change elsewhere in the schema, a doc comment, an error message, a computed signal or a method does not move it.

`Schema::closure(&TypeRef)`, `closure_of_params(&[ParamDef])`, `store_closure(type_id)`, `store_fingerprint(type_id)`, `query_closure(query_id)` (the `T` of `T` or `Result<T, E>`), `mutation_closure(mutation_id)` and `stores_closure(&[type_id]) -> StoresClosure { stores, records, enums }` (several store types at once: a snapshot's description; `closure_of(type_id)` gives back one store's closure with the same fingerprint the writing build computed). `TypeClosure::narrowed(&TypeRef)` is the closure of one type inside another (what a `from` fingerprint of a `ty` hook is computed over).

---

## 3. Wire format (`undra-wire`)

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

Records with `#[undra(default)]` fields: the wire layout still contains the field; `default` affects construction ergonomics in generated code and the migration of persisted data (a field added with `default` bumps the schema hash like any other change). **Live** peers with different schema hashes still refuse each other at attach (R7, §16, `UndraSchemaMismatch`). **Persisted** state evolves across builds (ADR-037): snapshots (§5.9), cached query results and queued mutations (§9) record the fingerprint of their types (§2.5) and are migrated by name on load (§5.9 lists the structural rules), by an app's `#[undra::migrate]` hook, or refused with a typed outcome; nothing is reinterpreted with the wrong types.

### 3.2 Envelope (transports only)

Used on WebSocket and Worker transports. In-process calls pass `kind` implicitly through the function they call and carry only the payload.

```
magic      4 bytes  55 4E 44 52  (fixed tag, the ASCII of "UNDR"; ADR-033)
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
| 12 | Hello | both | `undra_version String, schema_hash u64, platform String, mode String` |
| 13 | Log | core→host | `level u8, target String, message String` |
| 14 | TimerFired | host→core | `timer_id u32` |
| 15 | Snapshot | core→host | §5.9 |
| 16 | Restore | host→core | §5.9 |

**The WebSocket URL (ADR-051).** The upgrade request of a `remote` connection may carry `?undra_session=<token>` (1 to 64 characters of `A-Z a-z 0-9 . _ -`, the same on every connection of one host runtime) and, on a reconnect by a host that holds constructed objects, `&undra_resume=1`. They are transport material, not envelope material: nothing above changes, and a server or client that ignores them behaves as before. A server that keeps a dropped host's objects (`undra dev`) hands them back to a connection that presents the token and asks to resume; one that does not hold the session answers the `Hello` and closes with WebSocket code **4001** (`session lost`).

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
flag     u8    0 = item, 1 = end, 2 = error (the stream's own E), 3 = failed
body     flag 0: item T
         flag 1: empty
         flag 2: E, the stream's own typed error
         flag 3: status u8, message String, detail String
```
Flag 2 carries **only the stream's own `E`** (ADR-036): only a method whose schema return is `Result<Stream<T>, E>` sends it, for an asynchronous opening that failed or an `Err(e)` item of an `impl Stream<Item = Result<T, E>>`; a host that receives flag 2 for a stream without an error type treats it as malformed. Flag 3 says **the call failed**, in the vocabulary of a failed reply (§3.4): `status` is 2 (panicked: `message` is the panic message, `detail` its backtrace), 3 (cancelled by the core: a restore that replaced the receiver, shutdown, or a runtime dropped by its owner; `message` is the reason, `detail` empty) or 5 (refused: `message` is the reason); any other status is invalid (`InvalidTag`, `StreamFailure.status`), as is a flag above 3. Every platform maps a flag-3 item exactly as it maps a failed reply with that status, to the same `UndraCallError` case: status 2 is `panicked` (message and backtrace), 3 is `cancelledByCore`, **5 is `refused`** (the reason), in Swift, Kotlin and TypeScript alike. The raw `stream` entry points end with the failed reply (Swift and TypeScript `UndraReplyError`, Kotlin `UndraReplyException`, with that status and the §3.4 body: message + detail for 2, empty for 3, message for 5) and the generated `mappedStream` / `mapped(streamFailure:)` turns it into the case; an item or a flag-3 body the runtime cannot read ends the stream as `malformed` (`UndraProtocolError`, `UndraProtocolException`, `UndraTransportError("protocol")`), and a flag-2 item on a stream without an error type is `malformed` too. Nothing is ever read from a message's text. Flags 1, 2 and 3 end the stream and need no credit.

Flow control: the core sends at most `credit` items beyond what has been credited; the host grants credit with StreamCredit. Initial credit is 0; generated bindings grant 16 on subscribe and top up when consumption drops below 8. A Cancel with the stream's `call_id` closes it.

### 3.8 Keyed patch

For `Signal<Vec<T>>` with `#[undra(key = "field")]`. Encoded as:
```
count  u32
ops    count × { op u8, ... }
   0 Insert  { index u32, item T }
   1 Remove  { index u32 }
   2 Update  { index u32, item T }
   3 Move    { from u32, to u32 }
   4 Clear   { }
```
Ops are applied sequentially to the host's current list; indices refer to the list state after the previous op. `Move` means remove the item at `from`, then insert it so that it ends at index `to` (both indices valid in the list before the op). A host that hits an out-of-bounds op treats the signal as desynchronised and re-observes it. The core produces a patch one of two ways (§16.1, ADR-027): it sends the list operations a store recorded (`push`, `insert`, `remove`, `update_at`, `move_item`, `clear`) as they were performed, or, for a list written with `set`/`update`/`replace`, it computes the patch by key equality and full-item encoded equality, and a change of that kind that removes more than 50% of items or has no key overlap is sent as `op = 0` (full value) instead. Either way the host applies the ops sequentially by position and the bytes mean the same. A **derived keyed list** (`DerivedList<T>`, §4.3, §16.1, ADR-039) is a third way: its ops come from its own index, which replays the source list's recorded operations (at most two derived ops per source op, a sort-key change being `Move` then `Update`; an op that does not change the view sends nothing). A raw write of the source, more than 4,096 operations kept for one commit, or a parameter change that moves more than 256 rows send the derived list as `op = 0` instead. Its patches are ordinary patches: the host applies and merges them (§11.1) exactly as any other.

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

`undra-wire` contains no `unsafe`. It has proptest round-trip tests for every type and a byte-fuzz test (random bytes never panic the decoder).

---

## 4. Macros (`undra-macros`)

All attribute macros are re-exported from the `undra` facade as `undra::api`, `undra::error`, `undra::store`, `undra::port`, `undra::query`, `undra::mutation`. They must produce clear diagnostics (§12) and never silently ignore an item.

### 4.1 `#[undra::api]`

* On `struct` (record): generates `impl Encode`, `impl Decode`, `impl UndraRecord` (type_id), and registers `RecordMeta`. Requires all field types to be wire types. Fields may be `pub` or not; all are encoded.
* On `enum`: generates `Encode`/`Decode` (u16 index + fields) and registers `EnumMeta`.
* On `impl Type { .. }` (object): a type takes one such block (a second is E0007, reported by the compiler through a constant named after the rule; a store's block must hold its constructors, E0011); every `pub fn` becomes a method; `pub fn new(..) -> Self`/`Result<Self,E>` and any fn returning `Self` becomes a constructor. Generates the dispatch function (§5.6), `impl UndraObject for Type` (type_id, name), and registers `ObjectMeta`. Receiver must be `&self` (objects are shared: `Arc<Type>`; interior mutability via signals or `Mutex`). `&mut self` is rejected (E0020).
* On free `fn`: generates a dispatch entry and registers `FunctionMeta`.

Method rules: parameters are wire types, or `ctx: &Ctx` / `ctx: Ctx` as the first parameter (constructors and free fns only; methods get `Ctx` from the object via `self.ctx` convention or `Ctx::current()`); return type is `T`, `Result<T,E>`, `impl Stream<Item = T>`, `Result<impl Stream<Item = T>, E>`, `impl Stream<Item = Result<T, E>>` or `Result<impl Stream<Item = Result<T, E>>, E>` (the same `E`; a different one is E0005); `async fn` marks `is_async`. A stream of `Result`s ends with its typed error part-way: an `Err(e)` item is sent as flag 2 and the stream is dropped (§3.7, ADR-036). It is recorded in the schema exactly as `Result<Stream<T>, E>`, so the canonical form, the hash and the generated platform code are those of a stream whose opening can fail.

### 4.2 `#[undra::error]`

On an enum. Requires `#[error("…")]` per variant (thiserror-style; `{0}`/`{field}` interpolation, `transparent`). Generates `Display`, `std::error::Error`, `From` for `#[from]` fields, plus everything `#[undra::api]` does with `is_error = true`.

### 4.3 `#[undra::store]`

On a struct. Fields of type `Signal<T>`, `Computed<T>` and `DerivedList<T>` are signals (in declaration order); other fields are private state (a `WeakCtx`, config; a `Ctx` field works but keeps the runtime alive until shutdown, ADR-034). `Lazy<T>` is reserved: it is rejected in v1 (E0001, "lazy lists are not available in v1"), as `undra-bindgen` rejects it. Generates `impl StoreObject for Type` (`cell`, `restore`), a `StoreRestorer` registration and the store part of the object meta (the struct must also have a `#[undra::api(store)] impl` block with at least one constructor; §16.3 has the details, including the hidden `CellSlot` field). Attributes: `#[undra(key = "id")]` on `Signal<Vec<T>>` enables keyed patches, and is **required** on a `DerivedList<T>` (E0008; a derived list is described as `Vec<T>`, `computed: true` with that key, attached with `attach_derived`, left out of snapshots and of the restore hook's parameters, ADR-039); a key on a `Computed<T>` stays E0008, which points at `DerivedList<T>`; `#[undra(no_coalesce)]` forces every commit of this signal to be delivered, and is recorded in its `SignalDef` so the platform mirrors apply every one of them (§11). `#[undra::store(restore = "Self::assemble")]` names the function that rebuilds the store from its plain signals on restore; it is required when the store has a `Computed` or `DerivedList` field (E0013). `#[undra(default)]` on a `Signal<T>` (ADR-037; recorded as `SignalDef.default`, it requires `T: Default`; on a `Computed<T>` it is E0008) makes a restore of a snapshot that lacks the signal fill it with `T::default()` instead of failing.

### 4.4 `#[undra::port]`

On a trait. Attribute args: `sync` (default for methods without `async`; a port is Sync iff all methods are sync), `event` (methods return `()` and are fire-and-forget host→core). Generates: `PortMeta` registration, a proxy type `<Trait>Proxy` that encodes calls and routes them through the runtime's port table (§5.7), `impl <Trait> for <Trait>Proxy`, and `undra::ports::<Trait>` accessor `Ctx::port::<dyn Trait>()`. Async port methods are `async fn` in the trait (Rust 1.75+ AFIT); the proxy implements them.

### 4.5 `#[undra::query]` and `#[undra::mutation]`

On an `async fn(ctx: &Ctx, ..params) -> Result<T, E>`. Args: `key = "literal"` (may include `{param}` placeholders), `stale = "30s"`, `persist`, `retry = 3`, `idempotent`. Generates a `struct <Name>Query` implementing `undra_query::QueryDef`, and registers `QueryMeta`. See §9.

### 4.5a `#[undra::migrate]` (ADR-037)

On a free `fn`: a migration hook for persisted data an older build wrote, used when structural migration (§5.9) cannot convert a value. `#[undra::migrate(ty = "Todo")] fn f(old: &DynValue) -> Result<Todo, MigrateError>` converts any persisted `Todo` whose structure changed, at any depth of a snapshot signal, a cached query result or a queued mutation's input; `#[undra::migrate(store = "Profile", signal = "age")] fn f(old: Option<&DynValue>) -> Result<f32, MigrateError>` one signal of one store in a snapshot (`None` when the snapshot lacks it); `#[undra::migrate(mutation = "add_todo")] fn f(old: &DynRecord) -> Result<DynRecord, MigrateError>` the queued input of one mutation, by parameter name (the result is encoded against the current parameters with the structural rules). `from = "0x<16 hex digits>"` restricts a hook to old data with that fingerprint (a hook with a matching `from` wins over one without). The function stays as written; the expansion submits a `persist::Migration` through `inventory` with a wrapper that encodes a typed result. Wrong arguments, a wrong shape, and a `ty` hook returning another type than the one it names are E0066 at compile time; a store, signal or mutation the core does not have (the macro cannot see them) is an E0066 ERROR logged when a runtime starts.

### 4.6 Emitted metadata

Statics are emitted as `static __UNDRA_META_<TypeName>: undra_meta::RecordMeta = RecordMeta { name: "Todo", type_id: 0x…, fields: &[ FieldMeta { name: "id", ty: TypeRef::Uuid, default: false } ] };` using `const`-constructible types. `TypeRef` must therefore be const-constructible: `undra-meta` provides a `const`-friendly mirror `TypeRefMeta` (`&'static`-based, e.g. `TypeRefMeta::Option(&TypeRefMeta::String)`), with `From<&TypeRefMeta> for TypeRef`.

---

## 5. Runtime (`undra-runtime`)

The runtime is dependency-light (no tokio). It provides the executor, the core lock, the object table, transactions, change-sets, ports, timers, panic guard, and snapshots. `undra-ffi` (native) and the wasm exports are thin shells over it.

### 5.1 Threading model

* **Core lock.** A `parking_lot::Mutex<Core>` (native) / `RefCell` (wasm). Whoever holds it *is* the core loop. Sync calls from the host run on the caller's thread holding the lock. Async tasks are polled by the core thread holding the lock. This preserves "one mutator" semantics while keeping sync calls at mutex-acquire cost.
* **Core thread** (native): one `std::thread` named `undra-core` that owns the executor loop: wait for work → lock → poll ready tasks (bounded batch, max 64) → unlock → repeat.
* **Blocking pool** (native, and the test runtime): `undra_runtime::spawn_blocking(f)` runs `f` on a pool of `min(4, cores)` threads without the lock and resumes the awaiting task via the executor. `f` must not write signals.
* **The write rule (ADR-035, every build).** A signal write with consequences (the signal is attached to a store, or something depends on it) is allowed only on a thread that holds **the owning runtime's** core lock (a dispatched call, a task poll, an event subscriber, `observe`, `restore`, `Ctx::with_core`), on a `TestRuntime` driver thread and inside `testing::unchecked_writes`; a signal of no published store (owner `0`) accepts any thread that holds *a* core lock. Everything else (a pool worker, a host or embedder thread, a thread inside no runtime, another runtime's core) is refused **before the value changes, in release as in debug**: the write panics with the teaching message E0065 (§12), logged at error level through the owning runtime first and counted in `stats_json`'s `off_core_writes`; inside a dispatched call or a task the panic is contained like any other (status 2). `Signal::try_set` / `try_update` return `WriteError::OffCore { owner }` instead and `Signal::can_write` asks. The runtime records the owner on every store it publishes (`StoreCell::set_owner`) and installs the checker (`undra_signals::set_write_checker`, §16.1). A host thread that must write synchronously uses `Ctx::with_core(|| ..)`, which takes the core lock on the calling thread, makes the runtime current, runs the closure in one transaction and releases (`Err(Reentrant)` from a host callback or the core itself); everything else sends the value to the core (`ctx.spawn`, a call, the result of `spawn_blocking`). Change-sets go to the store's owner (§16.1 `ChangeSink::deliver_from`), never to whichever runtime the committing thread is in or to the global one.
* **wasm**: single thread; `undra_poll()` export drives the executor; wakers call the `undra_host_schedule()` import (deduplicated per turn).
* **Shutdown.** `undra_shutdown` / `Runtime::shutdown` first answers every call still in flight with status 3 (`cancelled`) and ends every open stream with a `StreamItem` flag 3 (failed, status 3 cancelled by the core, message `"the runtime shut down"`; §3.7, ADR-036), each exactly once; then it stops and joins the core, timer and blocking threads, fails pending port calls with `PortError::Cancelled`, clears event subscribers and Rust port bindings (closures that hold a `Ctx` are reference cycles with the runtime), and drops every task and object under the core lock. Afterwards `call` answers status 5, and `spawn`, `sleep`, `port_call` and `event` on a surviving `Ctx` are no-ops that log a warning (never a panic, never queued). It must not be called from the core thread or a host callback (it would wait for itself); debug builds assert this (ADR-023). `Runtime::extension` values are not cleared. A cancelled task's future is always dropped on the core (with the core lock held), so user `Drop` code never runs concurrently with core code. Shutdown first closes the runtime's *lifeline*: `Ctx::closed()`, `WeakCtx::closed()` and a pending `WeakCtx::sleep` complete with `Gone::ShutDown` before anything else happens, and `WeakCtx::upgrade` answers `Err(Gone::ShutDown)` from then on, even while the memory is alive (ADR-034).
* **Owners and `Drop` (ADR-034).** The `Arc<Runtime>` that `Runtime::new` / `init` return is the owner; the global runtime's owner is the global slot, so `undra_shutdown` stays the only way to end it. The runtime's own call and stream tasks, the blocking pool's queued jobs, `undra-query`'s tasks and its query handles hold the runtime **weakly**, and event subscribers receive the `Ctx` as an argument, so a runtime whose app code keeps only `WeakCtx`s across awaits is freed when its owner drops it. With no runtime loaded, `undra_stats_json` (and JNI `statsJson`) reports `runtime_threads`, the threads `undra-runtime` started in the process that still run: `0` once a shutdown has joined them, which is how the contract runners check that closing ended the core's work instead of only detaching the host (S17.7). `Drop` then does what shutdown does, including answering what is in flight (status 3 for calls, the cancelled stream item for streams, each once: the `Host` is still owned by the runtime while it drops), closes the lifeline with `Gone::Dropped`, and joins the core, timer and blocking threads, except the one it runs on (the last strong reference can be released at the end of a poll on the `undra-core` thread, or by a blocking job). A `Ctx` kept in a long-lived place (a store field, a task that loops, a closure in an extension) still pins the runtime until `shutdown`, and so does a call or task whose future holds a `Ctx` across an await that does not complete: an async method's `ctx` parameter, or a port proxy (`ctx.http()`, `ctx.kv()`, which hold one) waiting on a port that never answers, keeps the runtime until the port answers or `shutdown` (the runtime's own call task holds it weakly, the method's future does not); `stats_json`'s `strong_refs` makes that visible.
* **Host callbacks** (reply, change-set, port call) are invoked from whatever thread completed the work, **while the core lock may be held**. The host must not call back into the core synchronously from these callbacks except `undra_buf_free`; it enqueues onto its main thread. Violations are detected in every build and reported as `E_REENTRANT`: the runtime marks the calling thread for the duration of **every** host callback (`reply`, `change_set`, `stream_item`, `port_call`, `timer_set`, `log`, `schedule`) and each core-lock entry point refuses a thread that holds that runtime's core lock or is inside one of its callbacks (ADR-023). The second condition matters: a callback delivered on a thread that does not hold the core lock (an off-core commit hands its change-set to the host while holding the store's delivery lock) that waited for the core would deadlock against a core waiting for that delivery lock. `port_reply`, `timer_fired`, `stream_credit` and `stats_json` never take the core lock and stay allowed from callbacks.

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
    pub fn query(&self) -> &QueryClient;                      // undra-query
    pub fn mutate<M: MutationDef>(&self, input: M::Input) -> MutationBuilder<M>;
    pub fn events(&self) -> &Events;                          // subscribe to Connectivity/Lifecycle events; subscribers receive (&Ctx, payload)
    pub fn current() -> Ctx;                                  // thread-local, valid inside any dispatched call
    pub fn downgrade(&self) -> WeakCtx;                       // ADR-034
    pub fn closed(&self) -> Closed;                           // completes (Gone::ShutDown) when shutdown starts, at once if it has
}
pub struct WeakCtx(..);                                       // Clone + Send + Sync; a Weak<Runtime>: does not keep the runtime alive
impl WeakCtx {
    pub fn upgrade(&self) -> Result<Ctx, Gone>;               // Err(ShutDown) once shutdown began (memory may be alive), Err(Dropped) after the last strong reference went
    pub fn is_alive(&self) -> bool;                           // cheap, for loop conditions
    pub fn closed(&self) -> Closed;                           // completes with the reason (the first one: a runtime shut down and then dropped says ShutDown)
    pub fn sleep(&self, d: Duration) -> WeakSleep;            // Ok(()) after d, Err(Gone) as soon as the runtime goes; holds only the weak reference
}
pub enum Gone { ShutDown, Dropped }                           // Copy + Display + Error
```

**The rule (ADR-034): a `Ctx` lives for a call or a task step; anything that outlives the call keeps a `WeakCtx`.** A `Ctx` is a strong reference; one kept by something the runtime owns (a task that loops, a subscriber, a store field) is a reference cycle that keeps the runtime and its threads alive until `shutdown`. The periodic-task idiom is `while weak.sleep(d).await.is_ok() { let Ok(ctx) = weak.upgrade() else { break }; work(&ctx).await }`, which ends with a typed outcome instead of spinning or pinning. A surviving strong `Ctx` after shutdown keeps the behaviour of §5.1 (logged no-ops; `Ctx::sleep` completes at once). `#[undra::store]` restores a `WeakCtx` field by downgrading the restore context, as it clones a `Ctx` field.

### 5.4 Object table

`Slab<Slot { generation: u32, entry: Option<Entry { object: Arc<dyn AnyObject>, .. }> }>`. `AnyObject: Any + Send + Sync + UndraObject`. `insert(Arc<T>) -> Handle`, `get::<T>(handle) -> Result<Arc<T>, BadHandle>`, `release(handle)`. **Generations are issued from one process-wide, monotonically increasing `u32` counter** (`fetch_add`; `0` is never issued), not from the slot: every `insert` takes a fresh generation, so a released handle stays stale for good and a restore (§5.9) cannot re-issue one. The counter does not wrap: after `u32::MAX` issues the next `insert` logs FATAL and panics (contained at the boundary, status 2) while everything that exists keeps working, a documented v1 limit (ADR-022). Release decrements; the `Arc` may outlive the handle if a task holds it. Stores additionally register in the `stores` index for change-set routing. Debug builds keep a count of live handles readable via `undra_stats()`.

### 5.5 Transactions and change-sets

* Every signal write outside `ctx.txn` is an implicit single-write transaction.
* A transaction is thread-local depth + a dirty set `(store_handle, signal_id)`. On commit (depth → 0): for each dirty store, recompute observed dirty computeds (dependency order), build the change-set of **observed** signals only (plus `no_coalesce` signals always), assign `txn_id` (monotonic u64), and invoke the sink. Unobserved dirty signals are recorded in the store's `pending_dirty` so that a later `observe` emits the current value.
* Writes are applied directly to the signal cell (no overlay) **but** are wrapped in a panic guard at the dispatch boundary: on panic the runtime marks the store `poisoned`, emits `Log(error)` and the dispatch returns status 2. A poisoned store keeps working (values are still consistent per write); poisoning is informational in v1 (ADR-017 explains why a copy-on-write overlay is deferred). A **computed** that panics on its current inputs poisons only itself (ADR-019 amendment): the commit (or observe) evaluates each computed under a panic guard, leaves the failing one out of the change-set and delivers every other signal of the store; the write that triggered it succeeds. The failing signal is *held back*: the host keeps the last value it received, it is not re-evaluated by unrelated commits, and it is evaluated again when its inputs change, its full value delivered when that succeeds. The runtime logs the failure once at error level, marks the store poisoned and reports `poisoned_signals` in `stats_json`; `StoreCell::failed_signals()` is the signal's typed poisoned state. A call that reads such a computed panics as before (status 2). A panicking **encoder** of a plain or keyed slot, or a panicking sink, still abandons the store's change-set and resends its slots in full with the next commit (ADR-019). On wasm (`panic=abort`) any panic traps the core.
* A **derived list** (ADR-039) is committed like a computed that ships keyed patches: the commit drains it (replays the source's recorded operations on its index) and adds its pending derived ops as one keyed patch, its full value when they overflowed or the index was rebuilt, or **no entry** when nothing in the view changed; a change-set left with no entry is not delivered. A derived list whose closures panic is held back exactly as a panicking computed is (listed in `failed_signals`), and sent in full when it next evaluates.
* `undra_observe(handle, signal_id, 1)` immediately emits a change-set with the current value(s) of the newly observed signal(s) (synchronously, before returning, in-process; asynchronously over a transport).

### 5.6 Dispatch

The macro-generated dispatch function has the signature
```rust
fn dispatch(rt: &Runtime, call: DispatchCall<'_>) -> DispatchResult
pub struct DispatchCall<'a> { pub method_id: u32, pub call_id: u32, pub handle: Handle, pub args: &'a [u8] }
pub enum DispatchResult { Sync(Result<Vec<u8>, Vec<u8>>) /* ok bytes / err bytes */, Async(Pin<Box<dyn Future<Output = Result<Vec<u8>, Vec<u8>>> + Send>>), Stream(Pin<Box<dyn Stream<Item = Result<Vec<u8>, Vec<u8>>> + Send>>), Unknown, BadRequest(String) /* status 5 with a reason */ }
```
The runtime looks up the object by handle (`Arc<dyn AnyObject>`), downcasts inside the generated dispatcher, decodes args (status 5 on failure), runs the method, encodes the result. For `Sync` results of an `undra_call_sync`, the bytes are returned directly; for `undra_call` the reply callback is invoked. `Async` results are spawned as a task keyed by `call_id`. Panics are caught by `std::panic::catch_unwind` (`AssertUnwindSafe`) at this boundary on native.

**Panic reports (ADR-046).** Every panic the runtime contains (a call, an async call, a stream, a detached task, a computed, an event subscriber, a store's `snapshot`/`restore`/`observe`, an init hook, a migration hook, a timer's or an executor's waker, a host callback, a background task, a boundary entry of `undra-ffi`) is accounted for once: the FATAL log record `undra::panic` (§7 for its text on wasm), `stats_json`'s `panics`, and one **structured report** handed to the standard `Diagnostics` port (§8), `Diagnostics.panicked(report: PanicReport)`, fire and forget (`port_call_id 0`, like a `Log` record, §6 host contract 6; the answer, if the host gives one, is ignored, and so is an `undra_port_reply` carrying id 0 in every host path). It is delivered to a Rust binding if the port has one (the fake `CaptureDiagnostics`), else to the platform; a platform that registered no adapter loses nothing but the report. A report that panics, or one made while a report is being delivered, is dropped (the log has it). The status 2 reply (`UndraCallError.panicked`, §3.4) is unchanged: the report is for the app's crash reporter, the reply for the caller. The report carries `message`, `location` (`file:line:column`, paths remapped in release builds, §7), `operation` (what was running: `Todos.add`, `add_later`, `task`, `computed Todos.visible`, `observe Todos`, `snapshot Todos`, `init hook <name>`), `thread` (the Rust thread's name), `frames`, and the core's `namespace`, `core_version`, `schema_hash` and `image_id`. `frames` are innermost first, at most 48: in a **release** build only `address`, the offset of the instruction into the image that holds the core (the instruction address less the image's load address, pointing into the call instruction, so a symbolicator takes it as it is, §13), which `undra-ffi` reads with the platform unwinder (`_Unwind_Backtrace`; the only `unsafe` involved, R2) and resolves against the symbol files of `undra build --release`; in a **debug** build each frame also has `symbol`, `file` and `line` from the backtrace text. `image_id` is the Mach-O `LC_UUID` or the ELF GNU build id of that image in lowercase hex (`undra build` links Android libraries with `--build-id`), what Crashlytics, Sentry and Play key their symbol files by. A runtime that has no frame source (the test runtime, the dev runner) reports no addresses and an empty `image_id`. `namespace` and `core_version` are the `CoreIdentity` that `undra_ffi::export_core!(.., version = "<core crate version>")` submits through `inventory`. A computed's panic is caught by `undra-signals`, whose sink (`computed_failed`) the runtime turns into the same report with the hook's recording of the panic (message, location and frames), or the message alone when no guard was active; a panic caught at a boundary entry of `undra-ffi` has the entry's name as its operation and no frames.

**The synchronous reply slot (ADR-028).** A synchronous method is answered with `rt.sync_ok(&value, Encode::encode)` / `rt.sync_err(&error, Encode::encode)` (both return a `DispatchOutcome`) instead of a built `DispatchResult::Sync`. Under `call_sync` the runtime has armed a per-thread reply slot for that call: the complete `Reply` payload (`call_id u32`, `status u8`, the encoded body, §3.4) is encoded straight into one reusable thread-local buffer and the outcome is a zero-sized marker, so the path from payload decode to reply bytes allocates nothing once the buffer has warmed up (it keeps at most 64 KiB between calls). Everywhere else the slot is not armed (`undra_call`, a dispatch layer that did not opt in, a dispatcher invoked directly, a call made from inside the closure that is reading another reply, a call into a second runtime from a method of the first, a thread that is being torn down) and `sync_ok` / `sync_err` build `DispatchResult::Sync(Ok/Err(Vec))` exactly as before. The two paths put the same bytes on the wire; `testing::call_sync_reference` runs a call through the allocating path so tests can compare them. `Runtime::call_sync_with(payload, |reply| ..)` lends the buffer to a closure (after the core lock is released) and is the allocation-free entry; `Runtime::call_sync` copies it into the `Vec` it returns, the single allocation of an `undra_call_sync`, which is the `UndraBuf` the caller frees (§6). Hand-written dispatchers and `DispatchLayer`s may keep returning `DispatchResult` and need not change.

### 5.7 Ports

The runtime holds a `PortTable`: `port_id → PortBinding { Foreign { cb, user_data } | Rust(Arc<dyn Any>) }`. Proxies generated by `#[undra::port]` encode args and call `runtime.port_call(port_id, method_id, args) -> PortFuture` (async) or `runtime.port_call_sync(..) -> Result<Vec<u8>, PortError>` (sync). Foreign bindings: the runtime invokes the registered callback (§6.3); sync ports must return synchronously (status 0) or the call fails with `PortError::Unavailable`. Rust bindings (fakes, and the built-in native `Timer`) are called directly. Events: `runtime.deliver_event(port_id, method_id, payload)` fans out to `Events` subscribers on the core loop.

An unavailable port is a normal outcome, not a bug: a port nobody registered behaves as `Unavailable` (§6.3), a call can be cancelled, and a host can reply with bytes that do not decode. The proxy of a method returning `Result<T, E>` therefore never panics on those outcomes: `PortError::Failed(bytes)` decodes into `E`, and every other `PortError` becomes `E::from(port_error)`, so the error type implements `From<PortError>` (E0033 at compile time when it does not; `HttpError` and `FsError` do, mapping `Unavailable` to `Network(..)` / `Io(..)`). A method without an error channel has no typed way to say "unavailable"; its proxy panics with a message naming the port and method and how to bind one (E0062), which the runtime contains at the dispatch boundary and which traps the core on wasm (`panic=abort`, §7). Register the optional ports a web build does not implement, or give their methods a `Result` (ADR-025).

Standard ports and their methods are defined in `undra-ports` (§8).

### 5.8 Timers

`Timer` is a port with one method `set(timer_id: u32, delay_ms: u64)` (event-ish: host→core `TimerFired(timer_id)` when due). Native default binding: a Rust timer thread inside `undra-runtime` (`BinaryHeap` + `Condvar`), used unless the host registers a foreign Timer. wasm: the TS runtime registers itself (`setTimeout`). Tests: `FakeClock` implements both `Clock` and `Timer` and fires timers when advanced.

### 5.9 Snapshot and restore

`Snapshot` payload, layout 2 (ADR-037; all little-endian):

```
count u32, generation_floor u32,                              // ADR-022's two leading words
schema_hash u64,                                              // of the core that took it
type_count u32, types × { type_id u32, fingerprint u64 },     // each store type once: the fast path
description_len u32, description bytes,                       // UTF-8: StoresClosure's canonical JSON (§2.5)
count × { handle u64, type_id u32, signal_count u32, signals × { signal_id u32, len u32, value bytes } }
```

Computed signals and derived lists are excluded (restored by recomputation), and so are they from the description. `generation_floor` is the highest handle generation the core had issued when the snapshot was taken (`0` if none). The description names every snapshotted store type's plain signals (`signal_id`, name, type, `default`) and the records and enums they reach; it is parsed only when a fingerprint differs. Decoders reject a store whose type is not in `types`, a type listed twice and a description that is not UTF-8; with the 64-bit hash after the floor, that is what makes a snapshot of the layout before ADR-037 fail to decode with a typed error (status 5) instead of decoding as something else. Hosts treat a snapshot as opaque bytes and hand it back unchanged (the platform codecs exist for tests; vector `snapshot_v2`).

**Identity and migration (ADR-037).** For each store record: if the store type's fingerprint equals the current build's, the values decode by `signal_id` as today (one comparison per store type; the description is not read). Otherwise each current plain signal is matched **by name** in the description and converted **structurally**: record fields and enum variants by name (order and indices are irrelevant), a field the old value lacks takes `None` when it is an `Option` or its zero value (`false`, `0`, `""`, empty, the nil UUID, the epoch) when it is `#[undra(default)]`, a field the new type lacks is dropped, integers widened without loss (`i8→i16→i32→i64`, `u8→u16→u32→u64`, `uN→i2N` and wider), `f32→f64`, `T→Option<T>`, `Bytes` and `Vec<u8>` either way, `Vec`/`Option`/`Map` recursively; a record or enum inside that does not convert is offered to the `ty` hook of its current type. A signal that does not convert is offered to the app's hook for that store and signal, then to the `ty` hook of its type (`#[undra::migrate]`, §12 E0066, `undra_runtime::persist`); hooks run under the panic guard. A signal the snapshot lacks takes `T::default()` when the field carries `#[undra(default)]` (the generated restore fills it), else the store-and-signal hook given `None`. A signal the store no longer has is dropped. A signal that converts neither way fails the restore as a whole with `RestoreError::Incompatible { type_id, store, signal, reason }` (restore code 7, `INCOMPATIBLE`; the reason is also logged at ERROR); a narrowing, a type change, a renamed field, variant or signal and a variant the new type does not have are not structural. One exception keeps all-or-nothing meaningful: a store **type** the current build no longer has is left out (its handles answer `stale_handle`) and reported (a WARN, and `Runtime::restore_with_report`'s `RestoreReport { restored, migrated, dropped, schema_changed }`). A migrated restore costs at most 10x the fast path (bench `snapshot/restore_100kb_migrated`).

`undra_restore(bytes)` rebuilds each store via its generated `restore(ctx, values)` and re-issues the same handles (the table is rebuilt from the snapshot, so handles held by the host remain valid). It raises the generation counter to `max(current, generation_floor, every generation in the snapshot)` and never lowers it, so a handle issued before the snapshot, or between the snapshot and the restore, can never be issued again to another object, in this process or a fresh one (ADR-022). A snapshot whose floor, or any store handle's generation, is `u32::MAX` is refused (status 5 / `BAD_SNAPSHOT`): it would leave nothing to issue. A snapshot in the layout without the floor fails to decode. Objects that are not stores are not snapshotted; their handles become invalid after restore (status 5 `stale_handle`). **In-flight calls and streams whose receiver the restore replaced or invalidated are cancelled** (ADR-023): every call made on a store is (each store is rebuilt, so a call still running on the old object would finish on a detached store and report success for a write the restored store never saw). A plain call is answered with status 3 (`cancelled`), exactly once; a stream ends with a `StreamItem` flag 3 (failed, status 3 cancelled by the core; §3.7, ADR-036), because the host did not ask for the end and a clean end would read as success; the tasks are dropped. Calls with no receiver (free functions, constructors) carry on. Restore emits change-sets for all observed signals: one per re-observed store, in handle order, each built and delivered under that store's delivery lock through the path `undra_observe` uses (§16.1 `observe_and_deliver`), inside one transaction (ADR-023). An observation is recorded with the store type it was made on, and a handle is re-observed only when the restored store there has that type (ADR-037 decision 9); otherwise the observation is dropped and logged, so a mirror typed for one store never receives another's entries.

**Dev reload (ADR-053).** `undra dev` uses this section to carry a core's state across a rebuild: when a rebuild has succeeded and the rebuilt core has started (not yet listening), the serving core's server is suspended (no new calls, open ones get a short time to finish, the client is closed, the session its client left is kept un-released), `Runtime::snapshot` is taken, and the rebuilt core restores it with `Runtime::restore` **before it listens**; its server then holds the old session (ADR-051) so the reconnecting client resumes it. The snapshot is held in the memory of the `undra dev` process only, never on disk, and is limited to 16 MiB. Across a changed schema hash the state is offered too (`Runtime::restore_with_report`): the restore matches signals by name and migrates what changed structurally or through the app's `#[undra::migrate]` hooks, and otherwise refuses as a whole and changes nothing (ADR-037), so the new core keeps the state (`Reloaded, state kept (the schema changed)`) or starts fresh and says why (`state reset: the core refused the snapshot: store `Counter` ...`). A client built from the old bindings is still refused at its `Hello` (R7). A store type the rebuild removed is left out, its handles stale like a plain object's. Stores keep their handles; objects that are not stores (and transient query handles) do not, and a call on one is status 5. No client takes part, no envelope kind and no payload changes: to a client a reload is a reconnect followed by the change-sets a restore produces.

### 5.10 Devtools protocol (transport only, optional)

When a transport is attached with `mode = "dev"`, the core additionally emits `Log` messages for every transaction commit (`txn_id`, dirty count, bytes), every port call (timing), and every panic. The inspector is a consumer of the same envelope stream; no separate protocol.

**Devtools (ADR-054).** Beside the envelope stream, a dev server started by `undra dev` serves a page and a second kind of connection for it. The listener answers `GET /devtools[/..]` with the page (a fixed table of assets compiled into the runner `undra dev` generates; nothing is read from a disk) and upgrades `/devtools/ws` to a *devtools connection*, which is **not** an envelope stream: its binary messages are `[tag u8][body]` in the primitives of section 3 (`undra-transport`'s `devtools::proto`, mirrored by the page's `proto.ts` and tested against shared vectors), so no kind, payload, ABI or runtime changes and an app client is not aware of it. The app slot is not used: a page and an app client coexist, up to `max_clients` (4) pages. Every request carries `?token=<per-run secret>` (128 random bits that `undra dev` prints in the page's address and hands the runner in its environment) and a missing or wrong token, an unknown path, a server without devtools, or a method other than `GET` is the same `404`, so the endpoint does not announce itself; the `Origin` policy of the app socket applies to the socket too. `undra dev --devtools auto|on|off` (default `auto`) serves it on a loopback `--addr` only. A production core has no `Server`, so none of this exists there, and `mode = "dev"` of a runtime only adds the log records above.

While at least one page is attached the server's *hub* **observes every store** through `Runtime::observe` and routes what the runtime then emits: the pages get every change-set whole (with the call that caused it, when a synchronous method of the app client did, and a `restore` label for a time travel); the app client gets exactly the entries it observed (a re-encoded change-set with the others cut, the original bytes when nothing is cut), so its mirror sees what it would see without the page; the values the hub's own observation emits go to the pages only, as *initial* state; and the runtime's observed flag is the union of the app's and the hub's, so the server records the app's `observe(off)` for a store the hub holds without passing it on, and when the last page leaves it switches the observation off and observes what the app client asked for again (the client is sent those current values like any `Observe`). Computeds nobody shows are evaluated while a page is open. The hub also records the calls the core makes to **platform-implemented ports** (arguments, reply, status and a latency on the server's monotonic clock; calls made while no app client is attached are counted, not listed), samples the **query cache** through the runtime's inspector seam (section 16.2: `undra-query` registers `queries`; at most four times a second, and nine times as far apart as a sample took when that is longer, the cache's lock held only to copy its rows), and sends `undra_stats_json` with the server's own counters every second. Its worker takes a `Runtime::snapshot` after each burst of commits (coalesced over 10 ms, or over nine times the last snapshot's duration when that is longer, so the snapshots, which hold the core lock, take a tenth of the worker's time at most): a **step**, kept in a ring of at most 200 steps, 32 MiB and 4 MiB a step (a bigger state is listed with its size but cannot be restored; a snapshot equal to the newest is not stored again). **Time travel** is `Runtime::restore` (section 5.9) of a step into the same runtime, through the app client's own session: its mirrors converge on the change-sets the restore emits and its dev bar says `time travel: step N` (a dev notice); the restore is itself a labelled commit and a new step (history is append-only), objects that are not stores and query handles go stale as in ADR-053, and stores built since the step are dropped (the answer to the page and the app's dev notice say how many). The ring records only while a page is attached, is cleared when the last one leaves and does not survive a reload (a rebuilt core has a new `core_epoch` in its welcome and a history of its own).

**Dev notices (ADR-053).** The dev server also says one-line things to the developer, as a `Log` record with the target `undra::dev` whose message is the sentence to show (`Reloaded, state kept`, `Reloaded, state kept (1 object not carried over)`, `Reloaded, state reset: the core refused the snapshot: ..`). Only a server started by `undra dev` after a reload sends one: to every client that attaches within 30 seconds, once per session token (a client that resumed its session is told what became of its state; a new one, why it starts fresh). It is not a devtools record (it reaches clients that did not say `mode = "dev"`), an in-process or production core never produces one, and a runtime dispatches it to its `onDevNotice` option only from its `remote` transport (section 17), in addition to its log sink. The target is reserved for the server: a record a core logs under `undra::dev` reaches the server's log sink (the terminal) but is never forwarded to a client. When the reload cut calls off (cancelled at the end of the settle, or sent after the old core stopped running calls, and so not run), the resumed notice says so: `Reloaded, state kept (2 calls lost in the reload)`.

### 5.11 Background runs (ADR-046)

An OS grants a background window (iOS `BGTaskScheduler`, Android WorkManager, a page about to hide); the core spends it on what is worth finishing while the app is not on screen. **A run is an ordinary call**: the standard async function `run_background(deadline_ms: u64) -> BackgroundReport` (§8; function id `0x0e5b14ff`, `Call` target `Function`, args one `u64`, reply body one `BackgroundReport`), with the ordinary cancellation (status 3) and no ABI or wire change. A **background task** is `Runtime::add_background_task(name, pending, run)` (a second of one `name` is ignored): `pending(&Ctx) -> u32` says cheaply how much work is waiting, `run(&Ctx, Deadline) -> Future<Output = BackgroundOutcome>` (`Done` or `Incomplete`) does it and holds the runtime weakly across awaits (ADR-034); a [`Deadline`] says how long the window has left (`remaining()`, by the runtime's monotonic clock) and takes the task's progress (`note_replayed`, `note_refetched`) as it happens, so a run the deadline cuts short still reports what it did. `undra-query` registers three when its client starts: **replay** (reads an unreadable queue again, ADR-049, then replays the offline queue when online and waits until it is empty, the network is gone or the window ends; offline it ends at once, incomplete, and does not hold the window; it counts the mutations answered meanwhile, including those of a replay the client's own `Connectivity` event had started; it is done when the emptied queue is persisted), **refetch** (starts a fetch of every cache entry that is observed or persisted and stale: past a window the query declared, or invalidated, failed, holding an error or never filled; waits for them; writes what they fetched; counts the fetches that succeeded) and **flush** (writes the cache entries whose 250 ms debounce has not ended and waits for the queue's writer). A run starts every task at once, polls them together under the panic guard (a task that panics is contained, reported like any panic with the operation `task`, and counts as incomplete) and returns when they are all done or at `deadline - 500 ms` (so the host can still tell the OS it finished), whichever is first, dropping what is left; `finished` is true only if every task was done in time and nothing is pending. `replayed` counts queued mutations answered, `refetched` fetches that succeeded, `still_pending` is the sum of the tasks' `pending` after the run. Work already done is kept: the queue persists per item (ADR-037), a replay in flight is the client's and goes on, a fetch that was cancelled is fetched again next time, so a run cut at its deadline or cancelled by the host loses nothing and sends nothing twice (the idempotency key is unchanged). A core with **no background task** (a hello world) links none of this (ADR-052: the runner is installed by `add_background_task`) and the function answers `finished` with nothing done. The deadline is enforced by a sleep on the `Timer` port and measured by the runtime's own monotonic clock, so a fake clock decides it in a test (R12); `Fakes::run_background` drives it.

`Lifecycle.Background` does two things in `undra-query`: debounced cache entries are written at once (the process may be suspended before 250 ms pass), and an unreadable queue is read again. `stats_json` gains `"panic_reports"` (reports delivered to `Diagnostics`) and, once a task is registered, `"background": {"tasks", "pending", "runs", "finished", "replayed", "refetched"}`: a platform reads `pending` after it reported `Background` (and after a mutation was queued offline) to decide whether to ask the OS for a window (§17.2, §17.3).

---

## 6. Native C ABI (`undra-ffi`)

**Version 2: one table per core (ADR-044).** A core is reached through exactly **one exported function**, named after its namespace (`[core] namespace` of `undra.toml`, §13): `const UndraApi *<namespace>_undra_api(void)`, which returns the core's immutable `UndraApi` table. `undra-ffi` exports no symbol of its own; the crate built as the core's library calls `undra_ffi::export_core!(<namespace>, jni_class = "<pkg>/UndraCoreNative")`, which exports that function and, with the `jni` feature, `JNI_OnLoad`/`JNI_OnUnload` (§6.1). A core image is self-contained (a cdylib, or on iOS a prelinked object, §13), so two cores in one process share no symbol, no registry, no runtime and no thread: each table is its own core. The table carries C-compatible types only; the entries are `extern "C"` functions of `undra-ffi`, and every `*const u8, u32` pair is borrowed for the duration of the call unless stated (a null `ptr` is an empty payload). They may be called from any thread, concurrently, subject to the host contract below. `undra-ffi` is the only crate allowed to contain `unsafe`, and every block has a `// SAFETY:` comment.

```c
typedef struct { uint8_t *ptr; uint32_t len; uint32_t cap; } UndraBuf;        // owned by the core; free with buf_free of the same table
typedef void (*undra_reply_cb)(void *user, uint32_t call_id, const uint8_t *ptr, uint32_t len);
typedef void (*undra_changeset_cb)(void *user, const uint8_t *ptr, uint32_t len);
typedef uint8_t (*undra_port_cb)(void *user, uint32_t port_id, uint32_t method_id, uint32_t port_call_id, const uint8_t *ptr, uint32_t len, UndraBuf *out_reply); // returns 0 = replied synchronously into out_reply, 1 = will reply async, 2 = unavailable
typedef void (*undra_stream_cb)(void *user, uint32_t call_id, const uint8_t *ptr, uint32_t len);  // StreamItem payload (§3.7: flag 0 item, 1 end, 2 the stream's E, 3 failed with a reply status)

typedef struct UndraApi {
    uint32_t abi_version;   // 2; a host reads it first and refuses any other value
    uint32_t size;          // sizeof(UndraApi) as the core was built; fields are only ever appended
    uint64_t schema_hash;   // fnv1a64 of the canonical schema (§2.3); a host compares it before init (R7)
    const char *name_space; // the core's namespace, NUL-terminated, static
    UndraBuf (*schema_json)(void);                       // owned copy of the whole schema as JSON, doc comments included (§2.3); schema_hash covers its canonical form, not these bytes
    uint32_t (*init)(const uint8_t *cfg, uint32_t len, undra_reply_cb reply, undra_changeset_cb changes, undra_stream_cb stream, void *user); // idempotent per core; cfg = encoded RuntimeConfig record; returns 0 ok
    void     (*shutdown)(void);                          // answers every in-flight call (status 3) and ends every open stream (§5.1 Shutdown) before stopping the threads; then drops the port registrations, waiting for port callbacks still running (host contract 5)
    uint32_t (*call)(const uint8_t *ptr, uint32_t len);  // Call payload (§3.3); returns 0 accepted, 5 bad request. Reply via reply_cb. Works for sync and async methods.
    UndraBuf (*call_sync)(const uint8_t *ptr, uint32_t len); // Reply payload (§3.4) returned directly; only for sync methods (async → status 5)
    void     (*cancel)(uint32_t call_id);
    void     (*stream_credit)(uint32_t call_id, uint32_t credit);
    void     (*observe)(uint64_t handle, uint32_t signal_id, uint8_t on);
    void     (*release)(uint64_t handle);
    void     (*port_register)(uint32_t port_id, undra_port_cb cb, void *user); // cb NULL removes; removing or replacing waits for the old registration's running callbacks (host contract 1, 5)
    void     (*port_reply)(const uint8_t *ptr, uint32_t len);   // PortReply payload; allowed from a callback; port_call_id 0 is ignored (host contract 6)
    void     (*event)(uint32_t port_id, uint32_t method_id, const uint8_t *ptr, uint32_t len);
    void     (*timer_fired)(uint32_t timer_id);
    UndraBuf (*snapshot)(void);
    uint32_t (*restore)(const uint8_t *ptr, uint32_t len);
    UndraBuf (*stats_json)(void);                        // live handles, tasks, txn count, crossings, strong_refs (ADR-034); with no runtime: {"initialized":false,"live_handles":0,"runtime_threads":N}
    void     (*buf_free)(UndraBuf buf);
} UndraApi;

// Each core: const UndraApi *<namespace>_undra_api(void);   (its own <namespace>_undra.h declares it as `const void *`)
```

`undra.h` (`runtimes/swift/UndraRuntime/Sources/UndraFFI/include/undra.h`, byte-identical in `@undra/react-native`) declares exactly this: the table, the callbacks and the host contract, and no function. The 19 operations of version 1 are the 17 entries plus the two constants `abi_version` and `schema_hash`, which are data so that a host checks both before it calls anything; every entry keeps version 1's signature and contract. Below, `undra_<name>` names the table's `<name>` entry (`undra_init` is `api->init`). The table is built on the first call of the export (after the image's static constructors ran) and never changes; a call through it is one indirect call (ADR-044 budgets `boundary/call_sync/add` at no more than 2 ns over version 1's direct call). The wasm ABI (§7) is unchanged.

`RuntimeConfig` record: `{ platform: String, mode: String /* "inproc" | "dev" */, core_threads: u8, blocking_threads: u8, log_level: u8 }`.

`undra_snapshot` with no running runtime (before `undra_init`, after `undra_shutdown`) returns an empty snapshot (`count 0`) whose `generation_floor` is the process-wide generation counter, which survives shutdown (§5.9, ADR-022).

Return codes (as implemented): `undra_init` returns 0 ok, or a nonzero `init_code` (bad argument, undecodable config, already initialized with a *different* embedder — a repeat init with the same callbacks and `user` is a no-op returning 0). "Once" is per core: two cores in one process are initialised, used and shut down independently. `undra_restore` returns 0 ok or a nonzero `restore_code` (2 a store's restore panicked, 5 a malformed snapshot or a bad handle or floor, 6 unavailable or re-entrant, 7 incompatible: a store's values do not migrate to this build, ADR-037); a failed restore leaves the core unchanged. The native ABI has no `undra_poll`, so `core_threads == 0` is treated as 1. There is no native log callback: core log records reach the host through its registered `Log` port (the JNI `Callbacks` interface likewise has none).

**Host contract** (the same text is the header comment of `undra.h`; a host that breaks a rule has undefined behaviour). The *callbacks* are `reply_cb`, `changeset_cb`, `stream_cb` (given to `undra_init`) and every `port_cb` (given to `undra_port_register`).

1. **Lifetime.** The three `undra_init` callbacks and its `user` stay valid until `undra_shutdown` *returns*; none is called afterwards. A `port_cb` and its `user` stay valid until `undra_port_register(id, NULL, ..)` or a replacing `undra_port_register(id, ..)` has returned for that id, or `undra_shutdown` has returned. When one of those calls returns, no invocation of the old registration is running, none will start and its `user` is never read again: the host may free `user` right then (ADR-026; the core waits, see 5).
2. **Threads.** A callback runs on the thread that produced the event (the `undra-core` thread, a blocking-pool thread, or a host thread inside an `undra_*` call such as `undra_call`), possibly with the core lock or a store's delivery lock held (§5.1). Callbacks run **concurrently** (four simultaneous `port_cb` invocations are measured): they must be thread-safe, short, and must not assume the main thread.
3. **No unwinding.** A callback must not throw, `longjmp` or otherwise unwind through the core.
4. **Re-entrancy.** A callback must not call back into the core except the entries that never take the core lock: `undra_buf_free`, `undra_port_reply`, `undra_stream_credit`, `undra_timer_fired`, `undra_stats_json`, and the read-only `undra_schema_json` (the table's `abi_version` and `schema_hash` are plain data) (§5.1). `undra_call`, `undra_call_sync`, `undra_cancel`, `undra_observe`, `undra_release`, `undra_event` and `undra_restore` take the core lock and are refused with `E_REENTRANT` (status 5, restore code 6, or logged and ignored), never deadlocked; `undra_init`, `undra_shutdown`, `undra_port_register` and `undra_snapshot` must not be called from a callback at all.
5. **Blocking.** Removing or replacing a port registration (`undra_port_register`) and `undra_shutdown` wait for the port callbacks of the registrations they remove that are running on other threads. They must not be called from inside a callback (debug builds assert; release builds skip the wait for the calling thread's own callbacks), not while holding a lock a `port_cb` needs, and a `port_cb` that never returns keeps them from returning. `undra_shutdown` may be called from any thread but a callback, also concurrently with other entries (they complete or fail softly); it removes the port registrations inside the same critical section that serialises `undra_init`, so an init on another thread waits for it and a registration made after that init returns is never lost to it.
6. **Log and Diagnostics.** The core's log records reach the host as `Log.log` calls with `port_call_id 0` (fire and forget), and so do its panic reports (`Diagnostics.panicked`, §5.6, ADR-046). Nothing waits for the answer (0, 1 or 2 are all accepted) and an `undra_port_reply` carrying id 0 is ignored silently: acting on it would log "no port call 0 is pending", which is one more Log call. Real port calls are numbered from 1.

Register ports before the core needs them. `InitHook`s (query hydration reads the `Kv` port) run on the core as soon as the runtime has been created, which is before `undra_init` has returned to the host. Ports the host supplies *with* the init call (the JNI `Callbacks` object, the wasm `port_call` import) are in place by then; a port registered with `undra_port_register` **after** `undra_init` returns is racy against the hooks: a hook's first call to it can reach the host before the registration and is answered `Unavailable` (§6.3), and the hook has silently lost its start-up work unless it retries (`undra-query` hydration retries for about five seconds). A host that needs a port at start-up therefore supplies it with `undra_init`, or registers it before any hook could run it, and must not assume a late registration is seen by start-up work.

### 6.1 JNI shim (feature `jni`)

`export_core!(<namespace>, jni_class = "<pkg>/UndraCoreNative")` with `undra-ffi`'s feature `jni` exports `JNI_OnLoad`, which registers the natives below on **the core's own class** with `RegisterNatives` (no per-call lookup), and `JNI_OnUnload`, which stops the core. Nothing is exported under a `Java_*` name: every core registers its natives on its own class, so two cores in one JVM (each `System.loadLibrary`ed, each running its own `JNI_OnLoad`) never bind one another's. The class is the bindings' `<kotlin package>.UndraCoreNative` (§10.2), an `object` implementing the runtime's public `NativeApi` with `override external` members (the shim ignores the second JNI argument, so a native may be a member or a static). The callbacks interface is `dev.undra.runtime.NativeCallbacks`, shared by every core, with no natives. Java signatures:

```java
int    abiVersion();                                 // 2
long   schemaHash();
byte[] schemaJson();
int    init(byte[] cfg, NativeCallbacks cb);         // cb.onReply(int callId, ByteBuffer reply), cb.onChangeSet(ByteBuffer), cb.onStream(int callId, ByteBuffer), int cb.onPortCall(int portId, int methodId, int portCallId, ByteBuffer args) returns 0/1/2, byte[] cb.portSyncReply() (read after a 0 return)
int    call(byte[] payload);
byte[] callSync(byte[] payload);
void   cancel(int callId);
void   streamCredit(int callId, int credit);
void   observe(long handle, int signalId, boolean on);
void   release(long handle);
void   portReply(byte[] payload);
void   event(int portId, int methodId, byte[] payload);
void   timerFired(int timerId);
byte[] snapshot();
int    restore(byte[] snapshot);
String statsJson();
void   shutdown();                                   // what undra_shutdown runs; releases the NativeCallbacks global reference (ADR-034). Kotlin's UndraCore.close() of an in-process core calls it (never from a callback) and waits for it (the core's threads are joined and running port callbacks have returned: not under a lock a sync port needs), and a later load starts a fresh core
```
The library is `lib<namespace>.so` (`.dylib` on macOS); the generated `UndraCoreNative` loads it with `NativeLibrary.load(<namespace>)` (`System.loadLibrary`, or `System.load` of `-Dundra.native.<namespace>.path`). The runtime and the bindings must share a class loader (they do on Android and on a plain classpath): `JNI_OnLoad` finds the class through the loader of the library. R8 keeps both by name: the bindings ship `META-INF/proguard/undra-<namespace>.pro` (`UndraCoreNative` and its natives), the runtime `META-INF/proguard/undra-runtime.pro` (`NativeCallbacks` and its implementations).
`ByteBuffer`s passed to callbacks are **direct** buffers over core memory valid only during the callback; the Kotlin runtime decodes immediately. `byte[]` arguments are copied once via `GetByteArrayRegion`. The JNI callbacks follow the host contract of §6. In particular a synchronous port is a two-call protocol with hidden per-thread state: the shim calls `portSyncReply()` on the same thread, right after `onPortCall` returned 0, and callbacks run concurrently, so an implementation must carry the reply in thread-local state (the shipped `InprocTransport` does, in a `ThreadLocal`), never in a shared field.

### 6.2 Swift

`UndraRuntime`'s C target `UndraFFI` declares the types of `undra.h` (the table, no functions), so the runtime links no core symbol and needs no stand-in: `UndraCore.load(.inproc(api:))` takes a core's table (`UnsafeRawPointer`), reads `abi_version` and `size` before it binds it to `UnsafePointer<UndraApi>`, copies the 17 entries out once and calls through them. The in-process claim is per namespace (the table's `name_space`): two cores load side by side, one namespace loads once. Each core's entry is declared by the generated package's own C target, `<Namespace>CoreFFI` (`<namespace>_undra.h`, `const void *<namespace>_undra_api(void)`); the function is linked from the core's XCFramework, whose slices carry the same header without a module map (a second definition of the module would fail the build). Generated code calls only `UndraCoreEntry` with the entry (§10.1); app code loads through `Undra<Namespace>.load(...)`.

### 6.3 Port callback contract

For **sync** ports the host must fill `out_reply` with a PortReply payload and return 0 before returning. For **async** ports the host returns 1 and later calls `undra_port_reply`. Returning 2 fails the call with `PortError::Unavailable`. A port that is not registered behaves as 2.

`out_reply` memory rule (the one buffer a host allocates): on returning 0 the host stores a block from the C allocator (`malloc`) with `len` set; ownership passes to the core, which copies the bytes and **always** releases the block with `free`, never `undra_buf_free` and never as a Rust allocation. `cap` is reserved: the host sets it to 0 and the core ignores it (a host-written `cap` used to select a Rust deallocator, which a `malloc`ed block with `cap = len` turned into allocator-mismatch undefined behaviour, review M1). On wasm (§7) a sync port must call `undra_port_reply` *before* returning 0 from the `port_call` import; returning 0 without having replied fails that call instead of leaving it pending.

---

## 7. wasm ABI (`undra-ffi`, target `wasm32-unknown-unknown`)

No wasm-bindgen. Exports and imports use only `i32`/`i64`/`f64`. Memory is the module's exported `memory`. `_initialize` is exported when present (reactor); the host calls it once after instantiation.

A wasm module is its own namespace (ADR-044): a core's module is `<namespace>.wasm` (§13), its exports keep the names below, and each core is its own module instance, so several cores share a page by loading several modules (the TypeScript runtime already holds several, each through its generated entry, §10.3). The wasm ABI did not change with the native table: a wasm core's `undra_abi_version()` is still `1`.

Exports:
```
undra_alloc(len: i32) -> i32 ptr           undra_free(ptr: i32, len: i32)
undra_abi_version() -> i32                  undra_schema_hash() -> i64
undra_schema_json() -> i32 (ptr to UndraBuf struct { ptr i32, len i32, cap i32 })
undra_init(cfg_ptr, cfg_len) -> i32
undra_call(ptr, len) -> i32                 undra_call_sync(ptr, len) -> i32 (UndraBuf*)
undra_cancel(call_id)                       undra_stream_credit(call_id, credit)
undra_observe(handle_lo: i32, handle_hi: i32, signal_id, on)   // u64 split to avoid BigInt requirement
undra_release(handle_lo, handle_hi)
undra_port_reply(ptr, len)                  undra_event(port_id, method_id, ptr, len)
undra_timer_fired(timer_id)                 undra_poll()                                // drive the executor
undra_snapshot() -> i32 (UndraBuf*)          undra_restore(ptr, len) -> i32
undra_buf_free(buf_ptr)                     undra_stats_json() -> i32 (UndraBuf*)
```
Imports (module `"undra"`):
```
reply(call_id, ptr, len)        changeset(ptr, len)        stream(call_id, ptr, len)
port_call(port_id, method_id, port_call_id, ptr, len) -> i32 (0 sync: host wrote reply via undra_port_reply *before returning*; 1 async; 2 unavailable)
schedule()                      // host must call undra_poll() on the next microtask
timer_set(timer_id, delay_ms_lo, delay_ms_hi)
log(level, ptr, len)            // payload at ptr/len: target String, message String
now_ms() -> f64                 // Date.now()
random(ptr, len)                // fills every requested byte from a CSPRNG (crypto.getRandomValues), or writes nothing; never a fallback
```
The Clock/Rng/Log ports have built-in wasm bindings over these imports so a web app needs no adapter code for them. **Randomness never degrades silently** (ADR-049): the built-in `Rng.fill` asks `random` for 16 bytes more than requested, pre-filled with a canary; a canary the host left as it was or zeroed means it has no random source, so the port answers *unavailable* after a level-4 `log` record naming the cause, and `Rng`'s proxy (no error channel) panics with E0062, a loud trap instead of predictable idempotency keys. The TypeScript import throws internally when the source fails and writes nothing (its guard catches the throw, so no exception crosses wasm frames), and `UndraCore.load` refuses a platform without `crypto.getRandomValues` before instantiating (§17.1). No import signature changes. All ports remain overridable, except in the TypeScript `wasm-worker` mode, where the worker answers `port_call` itself (protocol 3, §11.1): a port the app implemented in the worker (`worker.ports`) is answered there, a port the host serves asynchronously crosses to the main thread, and every other port gets 2, so the built-ins serve Clock, Rng and Log.

**Restarting after a trap** (ADR-049). A panic traps the instance (`panic=abort`); nothing of it can be called again. A host that recovers (the TypeScript runtime's `recovery` option, §17.1) instantiates the **same compiled module** again, calls `_initialize` and `undra_init`, and restores the last snapshot it kept with `undra_restore` (codes as in §6, including 7 `INCOMPATIBLE`), raising the snapshot's generation floor to the highest generation it holds (ADR-022). It never calls `undra_timer_fired` or `undra_port_reply` on the new instance for a timer or a port call of the instance that trapped: their ids belong to the old one.

`undra_alloc(len)` never returns 0: it traps (after a level-5 `log` record) when memory is exhausted and when `len` is a size no allocation can have (`>= 0x7fff_fff9` on wasm32); a host that does not check the result would otherwise write at linear address 0, the bottom of the shadow stack (the TypeScript runtime also refuses a 0). `port_call` returning 0 means the host called `undra_port_reply` for **that** `port_call_id` before returning; a reply for some other pending call does not count, and the call then fails instead of staying pending.

Build: `--release`, `-C panic=abort`, `-C opt-level=z` or `s` (measured: `z` is 14 KB gzipped smaller on the hello world), `-C lto=fat`, `-Z`-free. `wasm-opt -Oz --strip-debug --strip-producers` when available (a warning when not; the size gate of §14 requires it). `undra build` remaps the builder's home directory to `~` (and a `CARGO_HOME` outside it to `/cargo`) with `--remap-path-prefix` for every crate of every release build, wasm, iOS and Android alike, so panic locations name no machine (ADR-052; through `build.rustflags`, or appended to `RUSTFLAGS` / `CARGO_ENCODED_RUSTFLAGS` when the user sets one; `--remap-path-scope=object` where rustc has it, so compiler messages keep real paths). Panics call the `log` import with level 5 (fatal) before trapping so the host can restart from snapshot. **The FATAL record** (target `undra::panic`) is the panic message, then on lines of their own `    at <file>:<line>:<column>` and, when a call or task was running, `    in <operation>` (4 leading spaces; the message may itself contain newlines, so a host reads the two trailer lines from the end); a panic before `undra_init` logs the older `<message> at <file>:<line>`. A wasm core cannot call out of a panic (`panic = abort`), so it never calls `Diagnostics`: the TypeScript host builds the same `PanicReport` from this record and the trap's stack (§17.1, ADR-046 decision 4.4) before it restarts anything (ADR-049).

---

## 8. Standard ports (`undra-ports`)

```rust
#[undra::port(sync)]  pub trait Clock { fn now_ms(&self) -> i64; fn monotonic_ns(&self) -> u64; }
#[undra::port(sync)]  pub trait Rng   { fn fill(&self, len: u32) -> Bytes; }
#[undra::port(sync)]  pub trait Log   { fn log(&self, level: u8, target: String, message: String); }
#[undra::port]        pub trait Http  { async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>; }
#[undra::port]        pub trait Kv    { async fn get(&self, key: String) -> Result<Option<Bytes>, StorageError>; async fn set(&self, key: String, value: Bytes) -> Result<(), StorageError>; async fn delete(&self, key: String) -> Result<(), StorageError>; async fn list(&self, prefix: String) -> Result<Vec<String>, StorageError>; }
#[undra::port]        pub trait SecureStore { /* same as Kv (ADR-049) */ }
#[undra::port]        pub trait Fs    { async fn read(&self, path: String) -> Result<Bytes, FsError>; async fn write(&self, path: String, data: Bytes) -> Result<(), FsError>; async fn delete(&self, path: String) -> Result<(), FsError>; async fn list(&self, dir: String) -> Result<Vec<String>, FsError>; }
#[undra::port]        pub trait Timer { fn set(&self, timer_id: u32, delay_ms: u64); }          // sync fire-and-forget; completion via TimerFired
#[undra::port(event)] pub trait Connectivity { fn changed(&self, online: bool, kind: NetKind); }
#[undra::port(event)] pub trait Lifecycle    { fn changed(&self, state: AppState); }            // Active | Background | Inactive
#[undra::port(sync)]  pub trait Diagnostics  { fn panicked(&self, report: PanicReport); }       // ADR-046: fire and forget, the report of a contained panic (§5.6)
#[undra::api] pub async fn run_background(ctx: &Ctx, deadline_ms: u64) -> BackgroundReport;      // ADR-046: the one standard function (§5.11)
```
Records: `HttpRequest { method: HttpMethod, url: String, headers: Vec<Header>, body: Option<Bytes>, timeout_ms: Option<u32> }`, `HttpResponse { status: u16, headers: Vec<Header>, body: Bytes }`, `HttpError { Network(String), Timeout, Cancelled, InvalidUrl(String) }`, `Header { name: String, value: String }`, `FsError { NotFound, Denied, Io(String), Full, Unavailable(String) }`, `StorageError { Unavailable(String), Full, Locked, Corrupt(String), Io(String) }` (ADR-049; wire indices in that order), `NetKind { Wifi, Cellular, Wired, Unknown, None }`, `AppState { Active, Inactive, Background }`, and (ADR-046) `PanicFrame { address: u64, symbol: Option<String>, file: Option<String>, line: Option<u32> }`, `PanicReport { message: String, location: String, operation: String, thread: String, frames: Vec<PanicFrame>, namespace: String, core_version: String, schema_hash: u64, image_id: String }` and `BackgroundReport { finished: bool, replayed: u32, refetched: u32, still_pending: u32 }` (type ids `0x19a497d1`, `0xd08d5436`, `0x5dbea5f3`; `Diagnostics` is port `0xab68cd7c`, `panicked` `0xbd147e2e`; `run_background` is function `0x0e5b14ff`; golden bytes in `crates/undra-ports/tests/encoding.rs`).

The standard surface (these eleven ports, these twelve types, `HttpMethod { Get, Post, Put, Delete, Patch, Head, Options }` among them, and the function `run_background`) ships in each platform runtime (`@undra/runtime`, `dev.undra.runtime.adapters`, `UndraRuntime`), not in generated code: every core links `undra-ports`, so its schema contains all of it and the schema hash covers it, but `undra-bindgen` leaves it out of an app's bindings and the generated code refers to the runtime's own types (section 10.5, ADR-024). The ADR-046 types are the runtimes' `UndraPanicReport`, `UndraPanicFrame` and `UndraBackgroundReport` in all three languages (`UndraPanicReport` has been TypeScript's since ADR-049), and `run_background` is each runtime's `runInBackground` (§17); bindgen filters the function like the types (`StandardFunction`). The standard surface changed once for ADR-046, so every core's schema hash changed once (the surface alone: `0x543d_0961_0867_e387`, it was `0xbbf6_f70d_0c56_7f47`).

`HttpError`, `FsError` and `StorageError` implement `From<PortError>` (§5.7): an unavailable port is `Network("the Http port has no adapter registered (E0062: register one, see <docs link>)")` / `FsError::Unavailable(..)` / `StorageError::Unavailable("the Kv port has no adapter registered (E0062: ..)")`, a cancelled call is `Cancelled` / `Io(..)` / `Io("cancelled")`, a reply that does not decode is `Network("malformed port reply: ..")` / `Io(..)` / `Corrupt(..)`, and a `Failed` reply decodes as the error itself.

**Every standard port method that can fail at run time has an error channel** (ADR-049): `Http`, `Fs`, `Kv` and `SecureStore`. The ones without (`Clock`, `Rng`, `Log`, `Timer` and the event ports) are answered by built-ins or cannot fail, which is ADR-025's "register the optional ports … or give their methods a `Result`" holding for the standard surface by construction. Storage adapters answer typed failures (port status 1 with a `StorageError`), never a raw throw: quota or disk exhausted (`NSFileWriteOutOfSpaceError`, `ENOSPC`, `EDQUOT`, `QuotaExceededError`) is `Full`; protected data unreadable now (`errSecInteractionNotAllowed`, data protection before the first unlock, `UserNotAuthenticatedException`) is `Locked`; stored bytes or ciphertext that cannot be read back (a file that does not decode, a Keychain item of the wrong class, `KeyPermanentlyInvalidatedException`, an AEAD tag failure, `OperationError` on decrypt) is `Corrupt`; no backend in this context (no IndexedDB, no secure context, no Keystore) is `Unavailable`, the web registering the ports anyway so they answer it; anything else is `Io` with the platform's message. Out of space under `Fs` is `FsError::Full`. A bridge that catches a non-typed throw (a bug in an adapter) answers status 2 and logs an ERROR naming the port, the method and the adapter. Each runtime has a failure-injection suite (`StorageFailureTests`, `storage-failures.test.ts`).
`Fs` semantics shared by every adapter: paths are relative to the adapter's root (a leading `/` is ignored) and one that would leave it, through `..` or a symbolic link, is `Denied`; `write` creates missing parent directories and is atomic; `delete` removes a file, or a directory with everything in it (a symbolic link is removed, never followed), and a path that names the root itself is refused (`Denied` on Swift, Android and the JVM, `Io` on the web, which has no handle for its root); `list` returns the names of one directory, sorted.

Fakes (all in `undra-ports::fakes`, `Send + Sync`): `FakeHttp` (script responses by matcher; records calls), `MemKv`, `MemSecureStore` (operations recorded; failures injectable: `fail(FailOn, StorageError)`, `fail_times(.., n)`, `heal()`), `FailingKv` (every operation fails with one `StorageError`), `MemFs`, `FakeClock` (settable `now`, `advance(d)` fires due timers; implements `Clock` + `Timer`), `SeededRng` (xorshift64\*), `CaptureLog`, `CaptureDiagnostics` (keeps every `PanicReport`), `ScriptedConnectivity`, `ScriptedLifecycle`; `Fakes::run_background(&t, deadline)` runs the standard function on a `TestRuntime` and lets the window pass on the fake clock until it reports. `TestRuntime::new()` installs all fakes and runs the executor on the test thread (`run_until(fut)` / `run_pending()`).

---

## 9. Query (`undra-query`)

* `QueryClient` lives in the runtime. Cache: `HashMap<QueryKey, Entry>`; `QueryKey = (query_id, encoded_params_bytes)`; `Entry { data: Option<Vec<u8>>, error: Option<Vec<u8>>, status: Status /* Idle | Fetching | Success | Error */, updated_at_ms, stale_ms, observers: u32, inflight: Option<TaskId>, persist: bool, gc_at: Option<i64> }`.
* A query is observed through a **QueryHandle object** (a store generated by the macro): signals `data: Option<T>` (0), `status: QueryStatus` (1), `error: Option<E>` (2), `fetching: bool` (3), `updated_at: Option<Timestamp>` (4). Constructing the handle registers an observer and triggers a fetch if stale or missing; releasing it decrements; when observers hit 0 the in-flight fetch is cancelled and `gc_at = now + gc_ms` (default 5 min).
* Refetch triggers: observer added while stale; `Lifecycle::Active` (all observed stale); `Connectivity online` (all observed); `interval_ms` if set; `invalidate(prefix)`; a background run (§5.11).
* Retry: `retry` attempts (default 3) with backoff `min(1000 × 2^n, 30000)` ms ± 20% jitter (via `SeededRng`/`Rng`), via `Timer`.
* Dedup: one in-flight fetch per key; concurrent observers share it.
* Mutations: `ctx.mutate(M(input)).optimistic(|cache| ..).invalidates([..]).await`. Optimistic closure runs inside a transaction with `CacheView` giving typed access to entries (`cache.get::<TodosQuery>(params) -> Option<T>`, `set`, `update`). On error, the entries the closure wrote are restored in one transaction, each to what it was before the closure wrote it, **unless something else has written that entry since**: every entry carries a write stamp that changes with each write (an optimistic write, `QueryClient::set`, a fetch result), and only an entry whose stamp is still the one the closure left is restored (compared by stamp, never by content). An entry written since keeps that write: a later optimistic mutation's placeholder, even on the same entry, survives an earlier mutation's rollback, and a fetch result or `set` is newer than the failed closure. A later optimistic mutation that wrote directly on top of the failed one's result inherits its restore point, so if it fails too the entry goes back to before both; if it succeeds, its invalidation refetches the entry. On success, `invalidates` keys are marked stale and refetched if observed.
* Offline queue: mutations marked `idempotent` that fail with `HttpError::Network` while `Connectivity` is offline are appended to the persisted queue and replayed FIFO on `online`. Non-idempotent mutations fail immediately when offline.
* Persisted forms (format 2, ADR-037; every value starts `format u16 = 2, schema_hash u64`): a cache entry under `undra.query.cache2.<query_id>.<fnv1a64(params)>` (fixed-width hex) as `{ format, schema_hash, fingerprint u64, updated_at i64, data bytes }`, written after each successful fetch (debounced 250 ms); the queue under `undra.query.queue2` as `{ format, schema_hash, count u32, items × { mutation_id u32, fingerprint u64, params bytes, idempotency_key Uuid } }`; the dead letters under `undra.query.queue.dead` in the queue's layout plus `reason String` per item. A fingerprint is the query's (the closure of its success type) or the mutation's (the closure of its parameters, by name; §2.5); each closure is stored once under `undra.types.<fingerprint as 16 hex digits>`, written before the first entry or item that needs it.
* Hydration (`QueryClient::hydrate()`, run when the runtime starts, by the init hook that every `#[undra::query]` and `#[undra::mutation]` submits; a core that declares neither does not link the query runtime and reads nothing at start, ADR-052; one whose only queries are hand-written `QueryDef`s hydrates on the client's first use instead. What a version with queries persisted stays in `Kv` unread while the core declares none, and the next version that declares one hydrates it as follows. ADR-037 decision 5, per item): fingerprint equal, used as stored; else the stored closure is read and the item migrated structurally (the rules of §5.9; a mutation's input by parameter name, a parameter the old input lacks is `None` if it is an `Option`), then by the app's hook (a mutation's `#[undra::migrate(mutation = ..)]`, the `ty` hook of a query's success type); a migrated item is rewritten in the current form at once. A cache entry that does not migrate is **deleted** (it can be fetched again) and reported (a WARN naming the query and the reason, `persist.dropped`); so is one of a query this build does not define or no longer persists. A queued mutation that does not migrate (or that this build does not define, or whose closure is missing) is **never deleted**: it moves to the dead letters with the reason (a WARN, `persist.dead_lettered`) and the rest of the queue replays. `QueryClient::dead_letters() -> Vec<DeadLetter { mutation, mutation_id, idempotency_key, reason, params: DynRecord, raw }>`, `retry_dead_letter(key)` (runs the migration again, e.g. after an update that added a hook; on success the item goes to the end of the queue) and `discard_dead_letter(key)`. The keys of format 1 (`undra.query.cache.<..>`, `undra.query.queue`) are read once with their own decoders: written under the current schema hash, their items are current and rewritten in format 2; otherwise their identity is unknown (a cache entry is dropped, a queued mutation dead-lettered) and the old key deleted.
* The persisted cache is bounded: at most `max_persisted_entries` (default 1,000, `QueryClient::set_max_persisted_entries`), least recently updated evicted when a new entry is written and at hydration; `undra.types.*` keys nothing references are deleted at hydration once every entry, the queue and the dead letters were read.
* Storage is best-effort (ADR-049): a failed write keeps the entry in memory, logs one WARN per (operation, reason), counts `persist.write_failed` and is tried again at the next write; `Full` pauses new entries (each later write is one probe of the oldest failed one) until a write succeeds. A failed read of a cache entry starts it empty. A failed read of the queue (`Locked`, `Io`, `Unavailable`: an app launched before the device's first unlock) leaves the queue **unread**: nothing replays and the queue key is never written until a read succeeds; new offline mutations wait in memory; the read is tried again on `Lifecycle` `Active` and `Background`, by a background run (§5.11) and after a backoff (1 s doubling to 30 s). `Background` also writes the entries waiting out their debounce at once (§5.11). Bytes that do not decode as a queue (or a store that says `Corrupt`) become a dead letter, bytes intact where they could be read. `QueryClient::persist_stats()` and the `query` section of `stats_json` (`cached_entries`, `persisted_entries`, `pending_mutations`, `dead_letters`, `queue: pending|unreadable|hydrated`, `persist: { write_failed, read_failed, dropped, migrated, dead_lettered }`) report it.

---

## 10. Generated code shapes (`undra-bindgen`)

`undra-bindgen` takes a `Schema` and emits three trees. Each generator is a Rust module with `write_*` functions and a golden-file test suite (`crates/undra-bindgen/tests/golden/<case>/{schema.json, swift/, kotlin/, ts/}`). Emitted code depends only on the matching runtime package. A derived list (`computed: true` with a `key`, ADR-039) has no shape of its own: it is generated exactly as a computed list is (read-only, documented "Computed by the core; read-only."), and its apply has the keyed-patch case every keyed list has.

**The core's entry (ADR-044).** Every tree declares one entry point for its core, `Undra<Namespace>` (`UndraPlaygroundCore` for the namespace `playground_core`, `Generator::namespace`), with the core's `namespace`, `load(options)` (the core's table or library and the bindings' schema hash filled in; refused while this core is loaded) and `core` (the loaded core, or a closed placeholder whose calls fail as unavailable while none is). **Every generated default core is the package's own entry's** (`ctx: UndraCore = UndraPlaygroundCore.core`, `core: UndraCore = UndraPlaygroundCore.core`), never `UndraCore.shared`, so two packages of two cores in one app each default to their own; `UndraCore.shared` (the first core loaded) remains for app code. `UndraIds` carries the namespace. Swift: `Generated/Core.swift` (`public enum Undra<Namespace>`, over `UndraCoreEntry`) and the C target `Sources/<Namespace>CoreFFI/` (`<namespace>_undra.h`, its module map, one source file) that declares `<namespace>_undra_api`. Kotlin: `Core.kt` (`internal object UndraCoreNative : NativeApi`, the core's JNI natives as `override external fun`s, which its `JNI_OnLoad` registers, §6.1; `object Undra<Namespace>` over `CoreEntry`) and `src/main/resources/META-INF/proguard/undra-<namespace>.pro`. TypeScript: `src/core.ts` (`export const Undra<Namespace> = { namespace, schemaHash, load, attach, core }`; `attach` is for a transport the app provides, React Native's). Naming: Rust `snake_case` → Swift/Kotlin/TS `camelCase` for methods and fields, `PascalCase` for types; enum variants → Swift `lowerCamel` cases, Kotlin `UPPER_SNAKE` for unit enums and `PascalCase` classes for data enums, TS string-literal `"camelCase"` / `kind: "camelCase"`.

### 10.1 Swift

```swift
// record
public struct Todo: UndraRecord, Sendable, Hashable, Codable {
    public var id: UUID; public var title: String; public var done: Bool
    public init(id: UUID, title: String, done: Bool)
    public static func undraDecode(_ r: inout UndraReader) throws -> Todo
    public func undraEncode(_ w: inout UndraWriter)
}
// unit enum
public enum Filter: UInt16, UndraEnum, CaseIterable, Sendable, Codable { case all = 0, active = 1, done = 2 }
// data enum
public enum Shape: UndraEnum, Sendable, Hashable { case circle(radius: Double); case rect(w: Double, h: Double) }
// error
public enum TodoError: UndraError, Error, Sendable, Hashable { case emptyTitle; case http(HttpError) }   // description from #[error]
// object
public final class Calculator: UndraObject, @unchecked Sendable {
    public init(ctx: UndraCore = UndraPlaygroundCore.core) throws   // constructor `new`; the default is the package's own core
    public func add(a: Int32, b: Int32) throws -> Int32    // sync
    public func fetch(url: String) async throws -> String  // async Result; throws HttpError
    public func ticks() -> AsyncThrowingStream<UInt32, Error>          // stream
    public func reset()                                    // `fn reset(&self)`: a command, it reports instead of throwing
}
// store
@MainActor @Observable public final class Todos: UndraStore {
    public private(set) var todos: [Todo]; public private(set) var filter: Filter; public private(set) var visible: [Todo]
    public init(ctx: UndraCore = UndraPlaygroundCore.core) throws
    public func setFilter(_ f: Filter)                       // a command
    public func add(title: String) async throws -> Todo     // throws TodoError
}
// port
public protocol Http: UndraPort { func request(_ req: HttpRequest) async throws(HttpError) -> HttpResponse }
// the core's entry (Core.swift)
public enum UndraPlaygroundCore {
    public static let namespace: String                                        // "playground_core"
    public static func load(_ options: LoadOptions = .inproc()) throws -> UndraCore   // .inproc(api: playground_core_undra_api()), UndraIds.schemaHash
    public static var core: UndraCore { get }
}
```
Sync methods in `inproc` mode call the table's `call_sync`. Store initial values are decoded from the change-set emitted by `undra_observe` during `init`.

**Recursive types.** A Swift value type cannot hold itself inline, so bindgen computes the *inline containment graph* of the schema's records, data enums and errors: `A → B` when a field of `A` (a payload field, for an enum) is a `B` or an optional `B`. `Vec`, `Map` and `Bytes` keep their elements on the heap and add no edge. A field whose edge lies on a cycle (`B` reaches `A` again, `A == B` included, so mutual recursion and record/enum cycles are covered) is stored behind a reference; everything else is generated exactly as before.
* A **record** keeps the public shape `public var next: ListNode?`: a computed property over `private var _next: UndraIndirect<ListNode>?`, whose setter replaces the immutable box, so value semantics, the memberwise `init`, `Hashable`, `Sendable` and the `Codable` JSON shape (a nil child is omitted, a missing key decodes as nil, through a private `CodingKeys` that maps `_next` to `"next"`) are those of a plain optional. `UndraIndirect<Value: Sendable>` is an `internal final class` around a `let`, emitted once into `Types.swift`, and only when some field needs it. (A property wrapper would read better but Swift rejects a public property whose wrapper type is internal, and a public wrapper would put a helper type into every generated module's API.)
* A **data enum or error** with a payload on a cycle is `indirect` (an array or dictionary payload never makes it so).
* A store signal's placeholder (the value before the first change-set) is built from the first variant of an enum that does not need the enum itself, and from `nil`, `[]` or `[:]` for an optional, array or map, so recursion ends at the base case.

**Failures (ADR-032; `docs/ERRORS.md` is the short guide).** Generated Swift never stops the process on the outcome of a call: no reply status, transport failure, cancellation or undecodable byte reaches `fatalError`, `precondition` or `assertionFailure`, in any build configuration.

* A generated **call** fails with exactly one of three things: its own error `E` (reply status 1; a `Result<T, E>` in Rust), thrown as `E` itself so `catch TodoError.emptyTitle` works; `CancellationError`, when the calling task was cancelled (async calls); or `UndraCallError` (§17.3) for every failure of the call itself: a panic in the core (status 2), a cancellation by the core (status 3: a restore replaced the receiver, or the core shut down), a refusal (status 5: a closed or stale handle, `E_REENTRANT`, undecodable arguments), an unreachable core (shut down, not loaded, remote connection lost) and a reply the bindings cannot read. A cancellation by the core is not a `CancellationError`: the caller's task was not cancelled, and a write that never landed must not hide behind the quiet-exit idiom.
* Calls use untyped `throws`; the domain error is named in the `- Throws:` documentation. A synchronous method that returns a value throws (`throws -> Int32`); an `async` method is `async throws`; a constructor is `throws` or `async throws` whether or not it has an `E`; a stream is `AsyncThrowingStream<T, Error>` and ends with the same three outcomes (a stream's consumer being cancelled still ends the iteration quietly).
* A synchronous method that returns `()` and has no error type is a **command** and stays non-throwing, because SwiftUI calls store methods from `Button` actions and `Binding` setters that cannot throw. When a command fails, the generated code calls `UndraCore.report(_:operation:)`, which logs at error level and calls `LoadOptions.onError` with an `UndraUnhandledError`, and returns. A command never writes a store property: the stores change only from the mirror's change-sets, so a refused command leaves the UI showing exactly the core's state.
* Every generated call is one `do`/`catch` that hands the error to `UndraCallError.mapped(_:)` (`mapped(_:domain:)` with an `E`, `mapped(streamFailure:)` for streams); the mapping lives in the runtime, once. A store's `apply` skips a change it cannot decode and reports it the same way (operation `"<Store>.apply(signal: N)"`).
* Typed throws (`throws(E)`) remain on **port requirements** only, where the host is the implementer and `E` tells it exactly which errors the core understands (`Generator::swift_typed_throws`, default on, emits plain `throws` when off). The raw entry points of `UndraCore` (`callSync`, `call`, `stream`, `construct`) keep throwing `UndraReplyError` and the transport errors.

### 10.2 Kotlin

```kotlin
data class Todo(val id: UUID, val title: String, val done: Boolean) : UndraRecord { companion object : UndraCodec<Todo> }
enum class Filter(val index: UShort) : UndraEnum { ALL(0u), ACTIVE(1u), DONE(2u) }
sealed interface Shape : UndraEnum { data class Circle(val radius: Double) : Shape; data class Rect(val w: Double, val h: Double) : Shape }
sealed class TodoError : UndraException() { data object EmptyTitle : TodoError(); data class Http(val cause: HttpError) : TodoError() }
class Calculator(ctx: UndraCore = UndraPlaygroundCore.core) : UndraObject(ctx) {
    fun add(a: Int, b: Int): Int                      // throws UndraCallError
    suspend fun fetch(url: String): String            // throws HttpError, CancellationException or UndraCallError
    fun ticks(): Flow<UInt>                           // ends with UndraCallError; collector cancellation is CancellationException
    fun reset()                                       // `fn reset(&self)`: a command, it reports instead of throwing
}
class Todos(ctx: UndraCore = UndraPlaygroundCore.core) : UndraStore(ctx) {
    val todos: StateFlow<List<Todo>>; val filter: StateFlow<Filter>; val visible: StateFlow<List<Todo>>
    fun setFilter(f: Filter)                            // a command
    suspend fun add(title: String): Todo                // throws TodoError, CancellationException or UndraCallError
}
interface Http : UndraPort { suspend fun request(req: HttpRequest): HttpResponse }   // throws HttpError
internal object UndraCoreNative : NativeApi { /* namespace, NativeLibrary.load(namespace), override external fun abiVersion(): Int ... */ }
object UndraPlaygroundCore { const val NAMESPACE: String; fun load(options: LoadOptions = LoadOptions()): UndraCore; val core: UndraCore }
```
Compose consumers use `collectAsState()` on the `StateFlow`s (no extra module). `UndraStore` and `UndraObject` implement `AutoCloseable`; a `Cleaner` releases leaked handles.

**Failures (ADR-032, amendment A; `docs/ERRORS.md` is the short guide).** Generated Kotlin never throws a raw runtime failure into application code.

* A generated **call** fails with exactly one of three things: its own error `E` (reply status 1; a `Result<T, E>` in Rust), thrown as `E` itself so `catch (e: TodoError)` works; `CancellationException`, when the calling coroutine was cancelled; or `UndraCallError` (§17.2) for every failure of the call itself: `Panicked` (status 2), `CancelledByCore` (status 3: a restore replaced the receiver, or the core shut down), `Refused` (status 5: a closed or stale handle, `E_REENTRANT`, undecodable arguments), `Unavailable` (the core is closed, not loaded, or its connection was lost) and `Malformed` (a reply, result or `E` that does not decode). A cancellation by the core is not a `CancellationException`. `E` and `UndraCallError` are both `UndraException`s, so one `catch (e: UndraException)` handles either. Argument validation is not an outcome of the call: a value the wire cannot represent (`WireException.NegativeDuration`, `DuplicateKey`, `IllegalArgumentException`) is a programming error and propagates unchanged.
* A synchronous method that returns `Unit` and has no error type is a **command** and never throws, because Compose calls store methods from `onClick` handlers: its body is one `try`/`catch (e: Exception)` that calls `UndraCore.report(e, "Todos.setFilter")`, which logs at error level and calls `LoadOptions.onError` with an `UndraUnhandledError`. A command never writes a `StateFlow`: stores change only from the mirror's change-sets.
* Every generated call is one `try`/`catch (e: Exception)` that throws `UndraCallError.mapped(e)` (`mapped(e, TodoError)` with an `E`, given the error's companion codec; `mappedStream` for a stream's `Flow.catch`); the mapping lives in the runtime, once. A secondary constructor delegates through `ctx.constructObject(...)`, and a store's `init` calls `observeAll()`, which closes the store and throws an `UndraCallError` when the core is gone. A store's `apply` decodes, checks the value is complete (`reader.finish()`), assigns, and on any failure reports `"<Store>.apply(signal: N)"` and skips the entry.
* The raw entry points of `UndraCore` (`callSync`, `call`, `stream`, `construct`) keep throwing `UndraReplyException`, `UndraTransportException`, `UndraProtocolException` and `WireException`.

### 10.3 TypeScript

```ts
export interface Todo { id: string; title: string; done: boolean }
export type Filter = "all" | "active" | "done";
export type Shape = { kind: "circle"; radius: number } | { kind: "rect"; w: number; h: number };
export class TodoError extends UndraError { readonly kind: "emptyTitle" | "http"; readonly cause?: HttpError }   // subclasses TodoError.EmptyTitle, TodoError.Http for instanceof
export class Calculator extends UndraObject {
  static create(core?: UndraCore): Promise<Calculator>;
  add(a: number, b: number): Promise<number>;             // rejects with UndraCallError
  fetch(url: string, signal?: AbortSignal): Promise<string>;   // rejects with HttpError, the signal's reason or UndraCallError
  ticks(): AsyncIterable<number>;                         // throws UndraCallError; `break` ends it quietly
  reset(): Promise<void>;                                 // `fn reset(&self)`: a command, it never rejects
}
export class Todos extends UndraStore {
  static create(core?: UndraCore): Promise<Todos>;
  readonly todos: Signal<Todo[]>; readonly filter: Signal<Filter>; readonly visible: Signal<Todo[]>;
  setFilter(f: Filter): Promise<void>;                    // a command
  add(title: string, signal?: AbortSignal): Promise<Todo>;     // rejects with TodoError, the signal's reason or UndraCallError
}
export interface Http extends UndraPort { request(req: HttpRequest): Promise<HttpResponse> }
export const UndraPlaygroundCore: {                       // core.ts; `static create(core = UndraPlaygroundCore.core)` everywhere above
  readonly namespace: string; readonly schemaHash: bigint;
  load(options: Omit<LoadOptions, "expectedSchemaHash">): Promise<UndraCore>;
  attach(transport: Transport, options?: Omit<AttachOptions, "expectedSchemaHash">): Promise<UndraCore>;
  readonly core: UndraCore;                               // the loaded core, or UndraCore.unloaded
};
```
All methods return `Promise` (uniform across main-thread, worker and remote modes).

**Failures (ADR-032, amendment A; `docs/ERRORS.md` is the short guide).** A generated call never rejects with a raw runtime failure.

* A generated **call** rejects with exactly one of three things: its own error `E` (reply status 1; a `Result<T, E>` in Rust), as `E` itself so `error instanceof TodoError` works; the reason of the `AbortSignal` that cancelled it (an `AbortError` by default); or `UndraCallError` (§17.1) for every failure of the call itself, with `kind` `"panicked"` (status 2), `"cancelledByCore"` (status 3), `"refused"` (status 5), `"unavailable"` (the core is closed, trapped, not loaded, or its connection was lost) or `"malformed"` (a reply, result or `E` that does not decode). A cancellation by the core is not an `AbortError`. `E` and `UndraCallError` are both `UndraError`s. A value the wire cannot represent (`RangeError`, `TypeError` from the writer) is a programming error and rejects unchanged.
* A synchronous method that returns nothing and has no error type is a **command**: it stays a `Promise<void>` (every method is) but that promise **never rejects**, so `onClick={() => void todos.toggle(id)}` has no unhandled rejection. Its whole body, argument encoding included, is one `try`/`catch` that calls `core.report(error, "Todos.toggle")`, which logs at error level and calls `onError` with an `UndraUnhandledError`. A caller that awaits a command learns that it was sent and answered, not that it succeeded; the effect is read from the store.
* Every generated call is one `try`/`catch` that throws `UndraCallError.mapped(error)` (`mapped(error, TodoErrorCodec)` with an `E`, `mappedStream` for a stream); the mapping lives in the runtime, once. A store's `create()` ends with `await store._observeAll()`, which closes the store and rejects with an `UndraCallError` when the core is gone. A store's `_apply` reports `"<Store>.apply(signal: N)"` and skips an entry it cannot decode.
* The raw entry points of `UndraCore` (`call`, `callSync`, `stream`, `construct`) keep rejecting with `UndraReplyError`, `UndraTransportError` and `WireError`. `Signal<T>` has `get()`, `subscribe(fn)`, `peek()`; `@undra/runtime/react` exports `useUndra(Class)` (creates a store on mount, closes it on unmount; `undefined` until it exists) and `useSignal(signal)` (`useSyncExternalStore`, with a server snapshot); `vue` (`useSignal` as a `shallowRef`, `useUndra`), `svelte` (`signalStore`, a `Readable`) and `solid` (`useSignal` as an `Accessor`, `useUndra`) adapters are thin files. The frameworks are optional peer dependencies; the core package imports none of them. `i64`/`u64` → `bigint`; `#[undra(js_number)]` → `number`.

### 10.4 Codecs

Each runtime ships `UndraWriter`/`UndraReader` mirroring §3.9 and the generated code implements per-type encode/decode. Generated codecs must be allocation-conscious: decode records into constructors directly, decode `Vec` with a preallocated capacity, and never go through JSON.

### 10.5 The standard library

The ten standard ports of section 8 and the eight types they exchange (`HttpMethod`, `Header`, `HttpRequest`, `HttpResponse`, `HttpError`, `FsError`, `NetKind`, `AppState`) are in every core's schema but not in an app's bindings: the runtimes already implement the ports and ship the types, and a second `FsError` in the app's namespace would fail native review. The schema keeps them (R1; the schema hash covers them); the generators filter them at generation time (ADR-024).

* **What is standard.** An item is left out only when it is exactly the standard one: same name, same id and same shape (fields, variants and indices, method ids and signatures; documentation is ignored). Ids are derived from names (section 1.1), so a schema item that only shares a name with a standard one has the standard id but another shape and stays the app's own type; one with the name and another id is E0052 (section 12). A standard type is left out only while every standard type it refers to is (a schema with its own `Header` does not get the runtime's `HttpRequest`). The table is `undra_bindgen::stdlib`, with the ids pinned as hex and cross-checked against the registrations of `undra-ports`.
* **What references become.** A port or type of the app that mentions a standard type refers to the runtime's own: TypeScript imports the type and its `<Name>Codec` from `@undra/runtime`; Kotlin imports `dev.undra.runtime.adapters.<Name>` (spelled in full where a variant of the enclosing sealed type shadows the name); Swift's runtime (`Core/StandardRecords.swift`) exports all eight as public types under the standard names, so Swift refers to them like the other two languages and declares none; the one spelling that differs is `AppState`, which is the runtime's `UndraAppState` (an app's own `AppState` is the commonest type name in Swift, and the runtime has exported that name since v1; ADR-024, amended). A module that declares a type with a standard name but another shape (the app's own `HttpRequest`) shadows the runtime's inside that module, and code that imports both modules qualifies the one it means. A typed failure whose error is `HttpError` or `FsError` is decoded by the runtime's codec at the call site, through the same `UndraCallError.mapped(e, <Name>)` / `mapped(error, <Name>Codec)` / `mapped(_:domain:)` as a generated error (every error type has a codec).
* **Ports.** The standard ports are never generated. They do not claim names in the generated namespace either, so an app may have a record called `Timer` or `Log`.
* **Escape hatch.** `Generator::emit_standard_library` declares everything as ordinary items; `undra-ports` uses it to prove its own schema generates.

---

## 11. Platform runtimes

Shared responsibilities (each runtime): load/attach the core; own `call_id` allocation; map replies to continuations/promises; hold the **mirror** (store handle → signal id → decoded value) and apply change-sets on the main thread under the delivery rules below; implement `Observe`/`Release`; provide default adapters; expose a `Transport` abstraction with `inproc` and `remote` (WebSocket) implementations (TS adds `worker`); implement the wire codecs; enforce the schema-hash check at attach with a clear error (`UndraSchemaMismatch { expected, got }`).

Main-thread delivery: Swift `MainActor`; Kotlin `UndraDispatchers.main` (`Dispatchers.Main.immediate` on Android, a single-thread executor named `undra-main` on a plain JVM); TS the thread that loaded the core (the page's main thread; the JS thread under React Native, §11.2).

Handle lifetime: explicit `close()`/`[Symbol.dispose]`; finalizers (`deinit`, `Cleaner`, `FinalizationRegistry`) as backstop; `UndraCore.stats()` exposes live handle counts.

Default adapters:
| Port | Swift | Kotlin (Android) | Kotlin (JVM) | TS (browser) | TS (node) | React Native (§11.2) |
|---|---|---|---|---|---|---|
| Http | URLSession | HttpURLConnection | `java.net.http.HttpClient` | fetch | fetch | React Native's fetch |
| Kv / SecureStore | files in Application Support / Keychain | files in `filesDir` / AES-256-GCM under an Android Keystore key (files in `noBackupFilesDir`) | files | IndexedDB / IndexedDB + WebCrypto | files | native: Swift's files and Keychain items on iOS, `android-adapters`' files and sealed values on Android |
| Fs | FileManager | `filesDir` | java.io | OPFS | fs | native, Swift's and `android-adapters`' roots |
| Clock, Rng, Log | Foundation / SecRandom / os_log | System / SecureRandom / `android.util.Log` | same | built-in (§7) | built-in | native in the module |
| Timer | DispatchQueue | ScheduledExecutor | ScheduledExecutor | setTimeout | setTimeout | the core's own |
| Connectivity / Lifecycle | NWPathMonitor / scenePhase | ConnectivityManager / ActivityLifecycleCallbacks | stubs | navigator.onLine / visibilitychange | stubs | NWPathMonitor or ConnectivityManager, native / AppState |

On Android all of it is installed by one call, `AndroidPlatformDefaults.install(core, context)` (module `android-adapters`; `android-adapters/README.md`); the runtime alone installs only Clock, Rng, Log and Timer.

Storage adapters answer failures as typed values (ADR-049, §8): `StorageError` for `Kv` and `SecureStore`, `FsError` for `Fs`, mapped by each built-in adapter from the platform's own errors (quota and `ENOSPC` are `Full`; a Keychain item before the first unlock, or an Android Keystore key that needs the user to authenticate, is `Locked`; a value that fails to decrypt or does not have the stored format is `Corrupt`; a missing backend is `Unavailable`; anything else is `Io`). An adapter that throws something untyped is a bug: the port answers status 2 and the runtime logs it at ERROR naming the port, the method and the adapter. The TypeScript `browserAdapters()` always registers `Kv`, `SecureStore` and `Fs`, answering `Unavailable` with the reason ("needs IndexedDB", "needs a secure context", "needs the origin private file system") where the platform lacks one, so the core sees a typed failure instead of an unbound port.

### 11.0 The remote transport reconnects (ADR-051)

A `remote` transport reconnects by itself unless told not to. Attempt `n` (from 1) waits `min(maxDelay, initialDelay * 2^(n-1))` less a random share of up to `jitter` of that (defaults 250 ms, 5 s, 0.5; each attempt takes at most 5 s). The core exposes the state `connecting`, `connected`, `reconnecting(attempt)` or `closed(reason)` (reason: `requested`, `schemaMismatch`, `sessionLost`, `failed`), per platform as in section 17. When the connection drops, every call, stream and pending `observe` fails at once with the platform's existing unavailable outcome, and so does anything started while it is down; the core is not closed. After the next successful handshake the host sends `Observe` for every signal it had observed and `Release` for every handle released meanwhile, and only then reports `connected`: the core answers each `Observe` with the current values (5.5), so every mirror converges. The app closing the core, a schema-hash mismatch on the server's `Hello` (reported once, as `UndraSchemaMismatch`) and a lost session (close code 4001, section 3.2) are final: no retry. In TypeScript and Kotlin a protocol error is final too: a text message, a malformed envelope, and in Kotlin (whose WebSocket client is its own) a frame that breaks RFC 6455 or a message over 64 MiB, which it answers with close 1002 or 1009; Swift logs and drops a malformed message. The first connection of `load` is not retried. The session token is an identifier, not a credential: the dev server has no authentication (ADR-051, threat model). **A rebuild no longer loses the session (ADR-053):** `undra dev` restores the old core's snapshot into the new one and seeds the new server with the old session, so a client that resumes finds its stores (same handles) and observes again; only when the state could not be carried (a schema change, a snapshot over 16 MiB, a refused restore, `--no-keep-state`) is a resuming client told 4001, as above. A frame the server sends right behind its `Hello` (the dev notice) is part of the connection: a client must process it, not drop it because its connecting thread has not yet published the connection.

### 11.1 Delivery: merged per drain, frame-aligned, bounded (ADR-031)

The core hands the host one change-set per transaction per store, in commit order, on the committing thread (§3.5, §5.5). The mirror decides when and how the main thread applies them; none of this changes the wire, the C ABI or the wasm ABI.

* **Queue.** Each change-set's entry table is parsed once, on the thread that received it; a malformed change-set is dropped whole and reported. The queue holds the parsed entries in arrival order.
* **Drain.** A *drain* applies the queue on the main thread. It folds the entries per key `(handle, signal_id)`, in arrival order, without decoding a value:
  * a full value (`op 0`) or a lazy invalidation (`op 2`) supersedes everything queued earlier for the key;
  * keyed patches (`op 1`) that follow each other for the key are concatenated into one patch: `count` = the sum of their counts, ops = each patch's op bytes in arrival order. §3.8 applies ops sequentially, each index relative to the list the previous op left, so the concatenation is the same change; a single patch is passed as it is. A patch too short to hold its count cannot be merged: the key is dropped and resynchronised as below.

  A derived list's patches (ADR-039) are merged the same way: each is relative to what the host had after the store's previous change-set.

  Each key is applied **at most twice per drain** (its last full value, then its merged patch), keys in the order of their first entry in the drain, through the store's generated apply function, which is unchanged: a merged patch that goes out of bounds takes the resynchronisation path of §3.8. A drain's subscribers hear once (TS notifies at the end of the drain, `batch`; Swift `@Observable` and Kotlin `StateFlow` are read at the next frame). Entries for a handle no store registered (a store closed meanwhile) are dropped and counted. Entries queued while a drain runs (a subscriber that makes a synchronous call) are applied by further rounds of the same drain, at most 1000 rounds; the rest goes to the next drain.
* **`no_coalesce`.** A generated store passes the ids of its `#[undra(no_coalesce)]` signals (`SignalDef.no_coalesce`) to its mirror registration. For those keys a drain applies every entry, in order, at its arrival position, and TS announces each one in a batch of its own. Whether the UI shows every value is up to the platform's reactive primitive (a Kotlin `StateFlow` conflates; SwiftUI renders once per frame).
* **When.** What the core produced on its own (timers, streams, events, port completions, background tasks) is drained **at most once per display frame**: TS on `requestAnimationFrame` while the document is visible (with a 100 ms timer as a backstop for a frame that never comes), in a zero-delay task while it is not, in a microtask where there is no document (Node, workers); Swift from a `CADisplayLink` on iOS, tvOS and visionOS (paused while the queue is empty) and a main-actor hop elsewhere; Kotlin through its `FramePacer` (`Choreographer` on Android from `android-adapters`; otherwise a paced single thread on a 16.67 ms grid that posts to `UndraDispatchers.main`); TS under React Native at the vsync of `@undra/react-native`'s frame source (`CADisplayLink` on iOS, `AChoreographer` on Android) while `AppState` is active, with the same 100 ms backstop, and in a zero-delay timer while it is not (§11.2).
* **Immediately, never delayed.** `observe(on)` drains before it returns in process; over a worker or a socket TS drains as soon as the initial change-set arrives (its `observe` promise waits for it), while Swift and Kotlin, whose `observe` does not wait over a socket, apply it at the next frame. A **reply** that arrives while entries are queued drains them before the caller resumes: TS from a microtask queued before the call's promise settles; Swift and Kotlin with an immediate main-thread drain enqueued before the continuation (or the blocking waiter) is resumed, so a caller on the main thread runs after it (FIFO). A **synchronous call made on the main thread** (`callSync`, and `construct` and `restore` on Swift and Kotlin) drains before it returns (made from inside a drain, by a subscriber or a store's apply, it leaves that to the drain's next round). Read-your-writes therefore holds for UI code after `await store.method()` and after a synchronous method alike.
* **Bounded backlog.** The queue is bounded by `maxPendingEntries` (default 65,536) and `maxPendingBytes` (default 16 MiB; an entry counts 17 bytes plus its value). Past either bound the thread that enqueues folds the queue in place with the rules above, every key included (`no_coalesce` ones too: the bound wins over the opt-out), and the next fold waits until the queue has doubled (O(1) amortised per entry). A key whose merged patch then holds more than 4,096 operations **or** more than 1 MiB of operation bytes is dropped and marked *awaiting a full value*: its keyed patches are discarded until a full value arrives, and the next drain re-observes it once (`observe(handle, signal_id, on)` from the main thread; the core answers with the current value, §5.5). Memory is O(observed keys × (value size + 1 MiB)) however long the main thread is blocked or the app is suspended, and it catches up in one drain.
* **Ordering.** Unchanged on the wire (ADR-019, ADR-020): the state after a drain equals the state after applying every queued change-set in commit order; intermediate states inside one drain are not observable. A transaction that touched several stores arrives as consecutive change-sets and may straddle two drains.
* **Observable.** Every mirror counts `changeSetsReceived`, `entriesReceived`, `entriesApplied` (after merging), `drains`, `compactions` and `resyncs`, plus the pending entries and bytes and the dropped entries; `UndraCore.stats()` reports them, and drain listeners are called on the main thread after each drain with the change-sets and entries it consumed, the entries it applied and its duration (§17).
* **TS worker.** In `wasm-worker` mode the worker posts the envelopes one of its tasks produced as one message (`{ t: "envelopes", data: ArrayBuffer[] }`, every buffer transferred, flushed from a microtask); the main thread reads that and the one-envelope shape. The worker protocol is internal to the package, **version 3** (ADR-049), announced in `init { ..., protocol: 3, asyncPorts: number[], portsModule?: string, recovery?: { snapshotEveryMs, maxSnapshotBytes } }`; a worker answers `ready { hello, features }` with the features it serves (`"snapshot"`, `"ports"`, `"recovery"`), and a host never sends a control message whose feature the worker did not announce. Host to worker, besides the envelopes: `ports { asyncPorts }` (after a `registerPort` once loaded), `snapshot { id }`, `restore { id, data }`, `restart { id, generationFloor }`, `close`. Worker to host: `snapshot { id, data | failure }`, `restored { id, code, failure? }`, `restarted { id, hello, restoredFromAgeMs, storeHandles | failure }`, `closed { failure }` (with the trap's stack for a trap). **Who answers a port** in the worker: a port the app implemented in the `worker.ports` module is answered there (inline when its implementation is synchronous, later when it returns a promise); a port in `asyncPorts` (registered on the host and not synchronous) crosses to the main thread; every other port gets 2 from `port_call`, so the built-ins serve Clock, Rng and Log (§7). A synchronous port registered on the main thread in this mode is refused at `load` (and by a later `registerPort`) with `UndraError("options")` naming the port and `worker.ports`; Clock, Rng and the core's timers are overridden through `worker.ports` (its default table and its `adapters` export), never through the main thread's `adapters`. With `recovery`, the worker takes and keeps the snapshots and serves `restart` itself (§17.1). A host older than protocol 3 keeps protocol 2's behaviour (every port but the three built-ins crosses to the main thread). A worker script older than the host (no `"ports"` feature in its `ready`) loads, its missing features degrading as announced, except that a host given `worker.ports` refuses it at load with `UndraTransportError("unsupported")` ("worker.ports needs this @undra/runtime's worker script"): it would ignore the module and send the app's synchronous ports across, trapping the core at their first call.

### 11.2 React Native (ADR-038)

`@undra/react-native` (`runtimes/rn/@undra/react-native`) is a fourth host of the C ABI of §6, under the TypeScript runtime: a pure C++ TurboModule (`UndraNative`, one method, `install(coreNamespace)`) installs `globalThis.__undraNative[namespace]`, one object of JSI host functions over one core's table per namespace, and `NativeTransport` implements §17.1's `Transport` over it (`mode` `"native"`, `synchronous`, `callSync`). `loadNative(entry, options)` attaches that transport through the generated entry (`UndraPlaygroundCore.attach`, §10.3), so the entry's `core` is the React Native core; `UndraCore`, the mirror (§11.1), the codecs, the generated TypeScript bindings and the framework adapters are unchanged. The wire and the schema are unchanged.

* **Threads.** A synchronous entry runs the core on the JS thread under the core lock; async work runs on the `undra-core` thread (§5.1), as under Swift and Kotlin.
* **One inbox.** Every callback appends one record (`kind u8, len u32, payload`; kinds are §3.2's: 2 Reply, 3 ChangeSet, 4 PortCall as `port_id u32, method_id u32, port_call_id u32, args`, 8 StreamItem, 13 Log as `level u8, target String, message String`) to one buffer and returns. The buffer is handed to JavaScript as one `ArrayBuffer` it owns, before a host function that entered the core returns (so a call's reply and change-sets, and `observe`'s initial change-set, are in the mirror when it returns) and through `CallInvoker::invokeAsync` when a record came from another thread. One FIFO keeps commit order across threads (§3.5). Only the outermost drain on the JS thread delivers.
* **Bytes.** Payloads go in as `(ArrayBuffer, byteOffset, byteLength)`, borrowed by the core for the call; an `UndraBuf` comes back as an `ArrayBuffer` that frees it (`undra_buf_free`, once) when collected. Handles cross as two `u32` halves.
* **Ports.** Registered before `undra_init` for every non-event port of the schema except `Timer` (the core's own). `Clock`, `Rng` and `Log` are answered natively on any thread (`Log` records are also delivered to JavaScript). Async methods are queued and answered by the registered `PortImpl` with `PortReply` (status 2 when none is registered). A synchronous method implemented in JavaScript is answered only when the core calls it from a host function on the JS thread; from another thread it is unavailable (§6.3), logged once per port.
* **Default ports (ADR-038, amendment B).** Every standard port of §8 has a default. `Kv`, `SecureStore`, `Fs` and the `Connectivity` source are the module's own native code: each of the first three is registered with a native callback (the table's `port_register`, before `init`, like the others) that copies the call onto that port's serial worker thread and returns 1, and the worker does the I/O and answers with `port_reply`, JavaScript never involved; `Connectivity` is reported with `event` from the platform monitor's thread (`nw_path_monitor`, `ConnectivityManager`) once `init` has returned, the current state first and identical consecutive reports once. Their directories, file layouts, Keychain items and Keystore key are the Swift runtime's on iOS and `android-adapters`' on Android (§8's table), so either shell of an app reads what the other wrote; several cores of one app share them. `Kv` and `SecureStore` failures are "unavailable" and logged, `Fs` failures its typed `FsError` (any `..` or symbolic link on a path is `Denied`). `Http` is `reactNativeHttp()` over React Native's `fetch` (no connection is `HttpError::Network`) and `Lifecycle` is `AppState` (a state reported once until it changes). A value in `adapters` or `ports` replaces a default and `null` removes it; a JavaScript `registerPort` after the load does not reach a native default. Shutdown runs the table's `shutdown`, then stops the event source and joins the workers, and only then releases the core's slot, so nothing a stopped core asked for reaches the next one of its namespace.
* **Gate and lifecycle.** The table's `abi_version` (2) and `schema_hash` are checked before `init` (`UndraSchemaMismatchError`). One `UndraCore` per namespace: several cores (several namespaces) share a process, each with its own host object and inbox; a JS reload shuts each core down with the runtime, and the next `install` starts a fresh one.
* **Finding a core.** One shim per platform resolves a namespace to its table, once, and copies its entries: on iOS `cpp/UndraApiLinked.cpp` calls `+api` of the Objective-C class `UndraCoreTable_<namespace>` that the core's pod compiles (the module is built once for every core of the app, so it cannot name a core's symbol); on Android `cpp/UndraApiAndroid.cpp` does `dlopen("lib<namespace>.so")` and `dlsym("<namespace>_undra_api")`. A namespace that is not a C identifier, a missing core, a table of another `abi_version`, too short, of another namespace or with a null entry is refused with a typed error.
* **Artefacts.** `undra build --platform rn` builds the iOS and Android cores (§13) and writes `build/ios/<Namespace>Core.podspec`, a pod vendoring the XCFramework (prelinked, no `-force_load`) and compiling `<Namespace>CoreTable.m`; the app packages `build/android/jniLibs`. `docs/REACT_NATIVE.md` is the guide.

---

## 12. Diagnostics

Every Undra diagnostic has a stable code and a fixed shape: `error[undra::E00NN]: <what>`, a note `<why>`, `help: <fix>` and `docs: https://shreypdev.github.io/undra/docs/errors.html#E00NN`. The shape is the same wherever the diagnostic comes from: a macro (a `compile_error!`, a `#[diagnostic::on_unimplemented]` message, or a `panic!` in a constant that the compiler evaluates), schema validation (`undra build`, `undra bindgen`, a runtime that loads a core) or the runtime (a message a panic carries). Codes are never reused; a code that is retired stays in the table.

The catalogue is audited by `crates/undra-macros/tests/catalogue.rs`: every row below has a constant and a row in the code table of `crates/undra-macros/src/impl_/diag.rs` (the site's short meaning), an emitting site in the code (the "raised by" column names it) and a golden that shows the real message (`crates/undra-macros/tests/ui/*.stderr` for the macros, `crates/*/tests/golden/diagnostics/*.txt` for the rest), and every message in a golden has all four parts and the link of its code. `site/scripts/build-errors.mjs` generates the error-codes page from this table and those goldens.

| Code | Raised by | Trigger |
|---|---|---|
| E0001 | macros, schema validation, the `Encode`/`Decode` bounds | unsupported type in a public position (lists the type and the allowed set); includes `Lazy<T>` (lazy lists are not available in v1), a `DerivedList` field without its row type (ADR-039), `Handle` (no schema type; objects cross through constructors), a `Result` whose error type is not a `#[undra::error]` enum, a `Ctx` parameter that is misplaced (on a method, not first, or `&mut`), and a type used as a value that cannot cross (an `Encode`/`Decode` bound that is not met: a struct without `#[undra::api]`) |
| E0002 | macros | generic parameter on a `#[undra::api]` item |
| E0003 | macros | lifetime in a public signature |
| E0004 | macros | trait object / `dyn` / `Box<dyn Fn>` / closure / function pointer / an `impl Trait` that is not a stream |
| E0005 | macros, schema validation | `Result` or `Stream` outside return position; a `Result<impl Stream<Item = Result<T, E1>>, E2>` whose two error types differ (ADR-036) |
| E0006 | macros, schema validation | map key type not allowed |
| E0007 | macros | unsupported item shape (a tuple or unit struct, a record without fields, an empty enum, an impl item that is neither a method nor a constructor, a receiver that is not `&self`, a store that is not a struct with named fields, the reserved field name `__undra_cell`, `#[undra::port]` on an inherent impl, an Undra attribute on the wrong kind of item, `#[undra::api]` on a method, `#[undra::query]` or `#[undra::mutation]` inside an `impl` block or a port trait, a function whose signature uses `Self`, a second `#[undra::api]` impl block for one type). A second impl block is reported by `rustc` as "the name `_undra_error_E0007_<Type>_has_two_undra_api_impl_blocks_merge_them_into_one` is defined multiple times": merge the blocks. A query or mutation inside an impl block that is not `#[undra::api]` cannot be seen by its macro; `rustc` then reports "macro definition is not supported in `trait`s or `impl`s" and "cannot find macro `_undra_error_E0007_a_query_is_a_free_function_move_it_out_of_the_impl_block`", which is this code: move the function out of the block |
| E0008 | macros | unknown or misplaced `#[undra(..)]` attribute or macro argument (an unknown key, with the nearest name suggested, a value of the wrong kind or a value on a flag, `key` on a signal that is not a `Signal<Vec<T>>` or a `DerivedList<T>` (a keyed `Computed` is pointed at `DerivedList<T>`), a `DerivedList<T>` without `key` (ADR-039), `key` naming no field of the list's items, `#[cfg]` on a public item, an invalid `crate = ".."` path); a `cfg_attr` whose attributes are all documentation or lint levels is accepted |
| E0010 | macros, schema validation | `#[undra::error]` variant without `#[error(..)]`, a message with an implicit `{}` (write `{0}` or `{name}`), `#[error]`, `#[from]` or `#[source]` on a plain `#[undra::api]` enum |
| E0011 | macros, schema validation | `#[undra::store]` and its `#[undra::api(store)]` impl block disagree, including a store with no impl block and a `#[undra::api(store)]` block without a constructor (a type takes one block; a constructor in another block is not seen); a store without a constructor in the schema |
| E0012 | macros | trait object in a record field (the blueprint example) |
| E0013 | macros | store cannot be restored automatically: it has a `Computed` field, or a field that is neither a signal, a `Ctx`, a `WeakCtx` nor `Default`, and no `#[undra::store(restore = "..")]` hook |
| E0020 | macros | `&mut self` receiver |
| E0021 | macros | `self` by value |
| E0022 | rustc, named by a macro | non-`Send` future in an async method or query: `rustc`'s own "future cannot be sent between threads safely", pointed at the method by an assertion the macros emit; the last note names the assertion, `_undra_error_E0022_the_future_of_an_async_method_must_be_Send`. Do not hold a non-`Send` value (an `Rc`, a `RefCell` borrow, a `MutexGuard`) across an `.await` |
| E0030 | macros | port method with a non-wire parameter |
| E0031 | macros, schema validation | event port method with a return type, or that is `async` |
| E0032 | macros | invalid port trait shape (an `async` method on a `sync` port, a parameter that is not a plain name, an associated type or const, no `&self` receiver) |
| E0033 | macros (a bound the compiler checks) | the error type of a `Result` port method has no `From<PortError>` (an unavailable port cannot be reported) |
| E0040 | macros | query without `key` / mutation with `stale` / a query argument of the wrong kind |
| E0041 | macros | query or mutation function with an invalid signature (not `async`, no `ctx: &Ctx` first parameter, not returning `Result<T, E>`, a stream result, `self`) |
| E0042 | macros | query whose success value is `()` or an `Option` (a query caches a value; use a mutation for effects) |
| E0050 | schema validation | duplicate type name, a type named like something the generated code depends on, two items with one id, two variants with one index |
| E0051 | schema validation | a name that collides after case conversion (`a_b` and `aB`), shadows a member of the runtime base classes, or is not an identifier in a target language |
| E0052 | schema validation | a record, enum, error, object or port named like a standard library item (section 8) but with another id: the runtimes implement the standard items under those names and ids |
| E0060 | macros (a check the compiler runs) | a spelling that looks like a built-in Undra type (`Bytes`, `Uuid`, `String`, `Vec`, ..) is a different type; reported at the field or parameter by the same-type assertion the macros emit |
| E0061 | macros (a check the compiler runs) | the schema records a name that the type written there does not have: an alias of an Undra type (`type Todo = Item`), a renamed import (`use m::Item as Todo`), or a name that is not a type declared with `#[undra::api]` at all (a plain struct, `type Id = u64`); reported by a const assertion on `UNDRA_TYPE_ID` |
| E0062 | runtime (the generated port proxy) | a port call that cannot be answered and has no error channel: no adapter registered, a cancelled call, a reply that does not decode. A method without an error channel panics with this message (the runtime contains the panic at the dispatch boundary; on wasm it traps the core); a method returning `Result` reports it as its error type, and `HttpError::Network` / `FsError::Io` carry the code and the link |
| E0063 | macros | `Option<Option<T>>` in a public position |
| E0064 | macros (a check the compiler runs) | an object (`#[undra::api] impl`) used as a field, parameter or return value: objects cross by handle |
| E0065 | runtime (the write check of `undra-signals`) | a signal of a store was written from a thread that does not hold the owning runtime's core lock: a blocking-pool worker, a host or embedder thread, a thread inside no runtime, or another runtime's core. A panic with this message, in every build; `Signal::try_set` / `try_update` return it as `WriteError::OffCore` (ADR-035) |
| E0066 | macros, runtime (start-up check of the registered hooks) | a `#[undra::migrate]` hook with a wrong target or shape (ADR-037): no target or more than one, `signal` without `store`, a `from` that is not a 64-bit hex fingerprint, a function that is not a plain non-generic `fn` taking the target's old value (`&DynValue`, `Option<&DynValue>` or `&DynRecord`) and returning `Result<_, MigrateError>`, a `ty = "X"` hook that does not return the `#[undra::api]` type `X` (a const assertion the compiler runs); a store, signal or mutation the core does not have, or a signal hook returning another type than the signal's, is logged at ERROR with this code when a runtime starts (the macro cannot see those items) |

The command line has its own codes, `C0001` to `C0014`, in the same shape and with the same docs page (`undra-cli`, `Code`); they are listed in the second table. Eleven have a golden (`crates/undra-cli/tests/golden/diagnostics/`, made by running the binary: `crates/undra-cli/tests/diagnostics.rs`); C0006 (the schema of a built core), C0012 (a platform this machine is not) and C0013 (a dev server that fails) need more than the binary and are listed as exceptions in the audit.

| Code | Raised by | Trigger |
|---|---|---|
| C0001 | `undra` | not inside an Undra project: no `undra.toml` from the working directory up to the filesystem root |
| C0002 | `undra` | `undra.toml` or a schema file cannot be read or has a value the CLI cannot use |
| C0003 | `undra` | a tool the command needs is not installed or not on `PATH` |
| C0004 | `undra` | a build tool ran and failed; its own output follows the diagnostic |
| C0005 | `undra` | the core crate is missing, or is not an Undra core |
| C0006 | `undra` | the schema could not be extracted from the built core |
| C0007 | `undra` | the schema cannot be turned into bindings (the E0001, E0005, E0006, E0010, E0011, E0031, E0050, E0051 and E0052 diagnostics of schema validation follow) |
| C0008 | `undra` | `undra init` or `undra adopt` would write into a directory that already has files in it |
| C0009 | `undra` | an argument has a value the command cannot use |
| C0010 | `undra` | a file or directory operation failed |
| C0011 | `undra` | the Rust toolchain lacks a compilation target the build needs |
| C0012 | `undra` | the platform is not available on this machine |
| C0013 | `undra` | the dev server failed to start or crashed |
| C0014 | `undra` | the core and the project disagree about where Undra comes from, or the project is on a newer Undra than this `undra` (`undra upgrade` moves a project forward only) |

---

## 13. Crate and package layout

```
Cargo.toml (workspace, resolver 3)
crates/undra-meta        no unsafe; deps: serde, serde_json, inventory
crates/undra-wire        no unsafe; deps: none (proptest dev-dep)
crates/undra-macros      proc-macro; deps: syn 2 (full), quote, proc-macro2
crates/undra-signals     no unsafe; deps: undra-wire, parking_lot
crates/undra-runtime     no unsafe; deps: undra-meta, undra-wire, undra-signals, parking_lot, slab, inventory, futures-core, pin-project-lite
crates/undra-ports       no unsafe; deps: undra-runtime, undra-macros (uses its own macros)
crates/undra-query       no unsafe; deps: undra-runtime, undra-ports
crates/undra-testkit     no unsafe; deps: undra-runtime, undra-ports, serde_json; the recording format, Recorder, Replayer, Seed, Harness (re-exported as `undra::testing`, §17.5)
crates/undra-ffi         unsafe allowed; deps: undra-runtime; features: jni (jni crate), wasm
crates/undra-transport   no unsafe; deps: undra-runtime, tungstenite (feature server); WebSocket server + framing
crates/undra-bindgen     no unsafe; deps: undra-meta, serde_json, heck
crates/undra-cli         deps: undra-bindgen, clap, notify, libloading (loads the host cdylib to extract the schema)
crates/undra             facade: re-exports prelude, macros, runtime, ports, query; `dev::serve()`
runtimes/swift/UndraRuntime          Package.swift, Sources/UndraRuntime, Sources/UndraFFI (undra.h: the types of the C ABI table, no functions), Tests
runtimes/kotlin/undra-runtime        settings.gradle.kts; modules: runtime (JVM+Android), android-adapters
runtimes/ts/@undra/runtime           package.json (ESM, exports: ., ./react, ./vue, ./svelte, ./solid, ./worker, ./vite, ./node), src/, test/
runtimes/swift/UndraRuntime (product UndraTestKit), runtimes/kotlin/undra-runtime/testkit (dev.undra:testkit), runtimes/ts/@undra/testkit   the testing kits (§17.5)
testkit/                            the fixtures every kit reads (recordings, a seed) and the fakes' conformance file
runtimes/ts/devtools                 the page `undra dev` serves at /devtools (ADR-054): TypeScript, no framework, imports only @undra/runtime/wire; build.sh bundles it with esbuild into crates/undra-cli/assets/devtools/ (committed, so cargo build needs no Node; `build.sh --check` runs in CI and fails when the committed files differ from a rebuild; 150 KB gzipped at most)
runtimes/rn/@undra/react-native      package.json (ESM; peers @undra/runtime, react-native), src/ (NativeTransport, loadNative), cpp/ (the C++ TurboModule over undra.h, ADR-038), ios/, android/CMakeLists.txt, UndraReactNative.podspec, react-native.config.cjs, babel-plugin.cjs, test/
examples/playground/core            the Rust core used by every playground app and by the contract tests
examples/playground/{ios,android,web}
examples/two-cores/{a,b,ios,android,jvm,node}   the playground core under two namespaces, and a test app per platform loading both (ADR-044)
contract-tests/                     schema fixture + per-language runners + the shared scenario list
bench/                              criterion (Rust), node bench, JVM bench, iOS bench target notes
```

Schema extraction: `undra-cli` builds the core for the host as a cdylib, `dlopen`s it, looks up `<namespace>_undra_api`, checks the table's `abi_version`, `size` and `name_space`, calls its `schema_json` (the whole schema, doc comments included, §2.3), checks the JSON's hash against the table's `schema_hash`, and runs bindgen (a core of C ABI version 1 is refused with `C0006`, saying so); the generated code carries the doc comments with `undra bindgen --docs` and none without. A core built before ADR-050 exports the canonical form: it still loads, and `--docs` on it is `C0006` rather than docless bindings. Fallback: `undra bindgen --schema schema.json`.


**Namespaces and artefacts (ADR-044).** `undra.toml` `[core] namespace` names a core: a lowercase C identifier of at most 32 bytes, by default the core's package name in snake case (`playground-core` → `playground_core`); a project next to another (a sibling directory with its own `undra.toml` and another `[project] id`; a second checkout of the same project, such as a `git worktree`, does not count) with the same namespace is refused (`C0002`), as is a namespace whose entry `Undra<Namespace>` is a name the runtimes or the generated bindings declare (`core` → `UndraCore`), and the runtimes refuse to load two cores with one namespace. The generated shim exports the core with `undra_ffi::export_core!(<namespace>, jni_class = "<kotlin package>/UndraCoreNative")`. `<Namespace>` is the namespace in `PascalCase` and `<Namespace>Core` its bundle name (`playground_core` → `PlaygroundCore`, not `PlaygroundCoreCore`):

| Target | Artefact | Notes |
|---|---|---|
| host | `build/host/lib<namespace>.{dylib,so}` (`<namespace>.dll`) | install name `@rpath/lib<namespace>.dylib`; exports `<namespace>_undra_api`, `JNI_OnLoad`, `JNI_OnUnload` and nothing else |
| android | `build/android/jniLibs/<abi>/lib<namespace>.so` | the same three exports; two cores sit side by side in one APK |
| ios | `build/ios/<Namespace>Core.xcframework`, one `lib<namespace>.a` per slice, `Headers/<namespace>_undra.h` (no module map) | each slice **prelinked** (`ld -r -exported_symbol _<namespace>_undra_api`, `-u _<namespace>_undra_api` for a fat-LTO release library, `-all_load` for a debug one) into one object whose only global is the entry: two cores neither collide nor merge, and the app links it like any library, **no `-force_load`** |
| web | `build/web/<namespace>.wasm` | a wasm module is its own namespace; its exports keep their §7 names |
| rn | the ios and android artefacts, `build/ios/<Namespace>Core.podspec`, `build/ios/<Namespace>CoreTable.m` | the pod compiles the class `UndraCoreTable_<namespace>` (`+api`) for `@undra/react-native` |

An app with several cores prefers an **umbrella core** when one team owns them (one core crate depending on the feature crates: one namespace, one runtime; `inventory` merges registrations and bindgen's E0050 catches name clashes). Independent cores (an SDK vendor's next to the app's) share nothing: no handles (two cores issue the same handle numbers; a handle means something only to its own core), no types, no threads; values pass between them through app code. Two generated Swift packages of local cores need two directory names (SwiftPM names a local package after its directory).

Build-system integration (a project made by `undra init`; `undra build` is never a manual step): the Android app's `app/build.gradle.kts` has an `undraBuild` task (`undra build --platform android`, `--release` when a release variant is built; `preBuild` depends on it; inputs `core/src/**`, the Cargo manifests and `Cargo.lock`, output `build/android/jniLibs`); the Xcode project has a Run Script phase **Build the Undra core**, before Compile Sources, running `undra build --platform ios --configuration $CONFIGURATION` with input and output file lists (`ios/Config/undra-core-{inputs,outputs}.xcfilelist`; the core's prelinked `lib<namespace>.a` is linked by its path in `OTHER_LDFLAGS`, not as a framework, because Xcode reads an XCFramework while planning the build); the web app's `vite.config.ts` uses `undra()` from `@undra/runtime/vite`, which runs `undra build --platform web` on start and, under `vite dev`, on every change of the core's `src/**`, manifests or `Cargo.lock` followed by a full reload (one build at a time; nothing under Vitest's mode `test` unless `inTests`). `undra build --configuration <NAME>` builds release for a name that contains `Release` and debug otherwise, and after an iOS build writes `build/ios/.undra-configuration-<NAME>` (removing the other configurations' stamps) so that Xcode re-runs the phase when the configuration changes. Each integration finds `undra` on `PATH` and in the install directories (the Gradle task and the Vite plugin take `UNDRA_BIN` first), and fails with `error[undra::C0003]` when it is not there. The shim's `Cargo.lock` is seeded from the project's again whenever the project's changes, so the platform libraries are built from the versions the project's lock file names.

`undra doctor` reports one finding per prerequisite: a stable id, a state (`ok`, `missing`, `wrong-version`, `not-applicable`), a severity, the observed value, the fix commands and the heading of `docs/ONBOARDING.md` that explains it; `--fix` prints the commands as one block and runs nothing, `--json` prints the report. `undra upgrade` moves every pin of the Undra version (the core's dependency, `undra.toml`, `@undra/runtime` and `@undra/react-native`, `dev.undra:*`, the `undra-swift` package requirement, `UNDRA_VERSION` of the CI workflow) to the CLI's version in the shapes `undra init` writes, all files or none (a version held in a Gradle variable is left and named), regenerates the bindings and prints the migration notes (`crates/undra-cli/src/migrations.rs`) of each release crossed; a `path` dependency is left alone and a project newer than the CLI is `C0014`. `undra init` writes `.github/workflows/undra.yml` (a job for the core and one per app) unless the project uses a checkout of the repository.
---

## 14. Quality gates

* `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`, `cargo doc --no-deps` warning-free.
* `#![forbid(unsafe_code)]` in every crate except `undra-ffi`.
* proptest round-trips for every wire type; byte-fuzz on `Reader`, `Envelope::parse`, change-set and patch decoders.
* Golden tests for bindgen (three languages).
* Contract scenarios (`contract-tests/scenarios.md`) executed by each runtime's test suite against the playground core: primitives round-trip, records/enums/errors, sync call, async call, error propagation, cancellation, stream with backpressure, store observe → initial change-set, transaction → single change-set, keyed patch, computed, query fetch/stale/refetch, optimistic mutation rollback, offline queue replay, snapshot/restore, schema mismatch rejection, panic containment, coalesced burst, derived keyed list.
* Size (ADR-052): the hello-world web core (the `undra init` template, `undra build --platform web`, `wasm-opt -Oz`) gzipped with zlib at level 9 is at most 120,000 bytes and at most 5% over its recorded size (`scripts/wasm-size.sh`, `[size."web/hello-wasm"]` in `bench/budgets.toml`, the `size` job of `bench.yml`); what the same app ships of `@undra/runtime` (Vite production build, worker script excluded) is at most 26,000 bytes and 5% over its record (`[size."web/hello-runtime-js"]`); a run that cannot measure either fails. The record, `bench/results/web-size.jsonl`, is the number the README and the site publish, and the shipped module must not contain the builder's home directory.
* Benchmarks (criterion): wire encode/decode per type, dispatch overhead, change-set build for 100 signals, keyed patch on 10k items. Cross-boundary benchmarks per runtime with the numbers written to `bench/RESULTS.md`.
* Every `pub` item documented. Every crate has a README with a 30-line example.

---

## 15. Conventions

* Edition 2024. `#[unsafe(no_mangle)]` spelling. MSRV 1.85.
* Errors: `thiserror`-style enums; no `anyhow` in library crates.
* No `println!`; use the `Log` port through `undra_runtime::log!`.
* Determinism: no `std::time::SystemTime::now()`, `Instant::now()`, `rand`, or threads spawned outside `undra-runtime`; Clock/Rng/Timer ports only (Constitution R12). The one exception is the native default `Timer`/`Clock` binding inside `undra-runtime`, gated behind `cfg(not(target_family = "wasm"))`.
* Commit messages: `type(scope): summary` — `feat`, `fix`, `test`, `docs`, `bench`, `state`, `chore`.

---

## 16. Internal Rust contracts (between undra-signals, undra-runtime, undra-macros)

These are the exact names the macros emit and the runtime consumes, as merged. They are exercised end to end by `crates/undra/tests/e2e_todo.rs` and, per crate, by the macros' behaviour tests (`crates/undra-macros/tests`), which compile and run generated code against the real runtime and signals. Change them only together.

### 16.1 undra-signals

```rust
// ---- the per-store table -------------------------------------------------------------------
pub struct StoreCell { .. }                      // one per store instance; Send + Sync
impl StoreCell {
    pub fn new(type_id: u32) -> Arc<StoreCell>;
    // Binding. Generated code calls exactly one of the next four per signal field, in
    // declaration order (ids 0, 1, 2, ..), and every one can fail (see SignalsError).
    pub fn attach<T: SignalValue>(self: &Arc<Self>, signal: &Signal<T>, signal_id: u32) -> Result<(), SignalsError>;
    pub fn attach_keyed<T: SignalValue + ListLike>(self: &Arc<Self>, signal: &Signal<T>, signal_id: u32, key: KeyFn<T>) -> Result<(), SignalsError>;   // #[undra(key = "..")]
    pub fn attach_computed<T: SignalValue>(self: &Arc<Self>, computed: &Computed<T>, signal_id: u32) -> Result<(), SignalsError>;
    pub fn attach_derived<T: SignalValue>(self: &Arc<Self>, list: &DerivedList<T>, signal_id: u32, key: fn(&T) -> u64) -> Result<(), SignalsError>;   // ADR-039; `key` only checks, in debug builds, that a full value's keys are unique
    pub fn set_no_coalesce(&self, signal_id: u32) -> Result<(), SignalsError>;   // #[undra(no_coalesce)]; called after that signal's attach
    pub fn set_handle(&self, handle: u64);       // the runtime, when the store enters the object table (and restore)
    pub fn handle(&self) -> u64;                 // 0 until published
    pub fn set_owner(&self, runtime_id: u64);    // the runtime, before set_handle (ADR-035): writes are checked against it, change-sets are delivered to it
    pub fn owner(&self) -> u64;                  // 0 until published
    pub fn type_id(&self) -> u32;
    pub fn signal_count(&self) -> u32;           // attached signals, computeds included
    pub fn is_observed(&self, signal_id: u32) -> bool;
    pub fn observe(&self, signal_id: u32 /* or ALL_SIGNALS */, on: bool, out: &mut undra_wire::Writer) -> u32;
        // on: appends one ChangeSet ENTRY per targeted signal (`handle u64, signal_id u32, op u8 = Full, len u32, value`)
        // with its current value, also for already observed ones (re-observing resynchronises a host), and returns the count;
        // the runtime wraps the entries into a payload (`txn_id u64, count u32, entries`). off: stops delivery, returns 0.
        // on runs inside a transaction (ADR-019): a computed's closure that writes signals while it is evaluated does not commit ahead of the entries; the entries are re-encoded (at most 8 passes) until no target was written meanwhile, so they carry the post-write values and the writes' own commit finds those targets clean. Writes to slots that were not targeted commit normally afterwards.
    pub fn observe_and_deliver(&self, signal_ids: &[u32] /* ids, or ALL_SIGNALS */, deliver: impl FnOnce(&[u8])) -> u32;
        // observe(on) plus delivery, for callers that must put the values in front of the host themselves (the runtime's `observe` and `restore`, ADR-023): ids are deduplicated and the entries ordered by signal_id; ONE complete change-set (`txn_id u64, count u32, entries`) is built and handed to `deliver` UNDER the store's delivery lock, with the `txn_id` allocated under it and recorded in the store's last txn (so a commit holding an older shared id replaces it). A commit of the same store on another thread therefore delivers completely before it (older values) or waits and delivers after it, and the host's last word is the newest value. Returns the entry count; 0 (for unknown ids) means `deliver` was not called. Same settle loop and transaction as `observe(on)` (leftover writes commit after the lock is released). A panic while encoding or in `deliver` rolls the observe back. `deliver` follows the ChangeSink contract (must not wait for another thread writing this store) and must not call the cell's `observe` family or commit this store from the same thread.
    pub fn encode_signal(&self, signal_id: u32, out: &mut undra_wire::Writer) -> bool;   // full value, no header; false if unknown
    pub fn failed_signals(&self) -> Vec<(u32, String)>;   // computeds held back because their evaluation panicked, with the message (ADR-019 amendment)
    pub fn is_failed(&self, signal_id: u32) -> bool;
    pub fn encode_snapshot(&self, out: &mut undra_wire::Writer);   // one store record, §5.9: handle u64, type_id u32, signal_count u32, signals × { signal_id u32, len u32, value }; computeds left out
}
pub const ALL_SIGNALS: u32 = u32::MAX;

pub struct CellSlot { .. }                       // Default + Debug + Send + Sync, deliberately not Clone
impl CellSlot {                                  // the hidden field `#[undra::store]` adds: empty until first use, then one cell for the store's life
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
pub trait SignalValue: undra_wire::Encode + Clone + Send + Sync + 'static {}
impl<T: undra_wire::Encode + Clone + Send + Sync + 'static> SignalValue for T {}

// ---- reactive primitives -------------------------------------------------------------------
pub struct Signal<T>;      // Clone = the same signal
impl<T: SignalValue> Signal<T> {
    pub fn new(value: T) -> Signal<T>;
    pub fn get(&self) -> T;                       // clone
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R;
    pub fn set(&self, value: T);                  // implicit transaction if none is open
    pub fn update(&self, f: impl FnOnce(&mut T));  // f runs with the value write-locked: it must not read or write this signal, nor read a Computed derived from it (transitive); a violation panics instead of deadlocking (ADR-021)
    pub fn try_set(&self, value: T) -> Result<(), WriteError>;              // ADR-035: a refused write is returned (WriteError::OffCore { owner }) instead of the E0065 panic
    pub fn try_update(&self, f: impl FnOnce(&mut T)) -> Result<(), WriteError>;
    pub fn can_write(&self) -> bool;              // whether this thread may write it now
    pub fn ptr_eq(&self, other: &Signal<T>) -> bool;
    pub fn is_attached(&self) -> bool;
}
impl<I: SignalValue> Signal<Vec<I>> {             // recorded list operations (ADR-027); see the keyed-list paragraph below
    pub fn push(&self, item: I);                  // Insert { len, item }
    pub fn insert(&self, index: usize, item: I);  // Insert; panics if index > len, like Vec::insert
    pub fn remove(&self, index: usize) -> I;      // Remove; panics if index >= len
    pub fn update_at(&self, index: usize, f: impl FnOnce(&mut I));   // Update { index, changed item }; f runs write-locked like `update`; panics if index >= len
    pub fn move_item(&self, from: usize, to: usize);   // Move { from, to }; no-op if equal; panics if either >= len
    pub fn clear(&self);                          // Clear (nothing is recorded for an empty list)
    pub fn replace(&self, items: Vec<I>);         // = set: a raw write, the commit diffs
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
// Derived keyed lists (ADR-039): a view of a list signal kept from its recorded operations.
impl<T: SignalValue> Signal<Vec<T>> { pub fn derive(&self) -> Derive<T>; }
pub struct Derive<T, U = T, O: Order = Unsorted>;   // #[must_use]; O: Unsorted | Sorted<K> (at most one sort, by type)
impl<T, U, O> Derive<T, U, O> {
    pub fn filter(self, f: impl Fn(&U) -> bool + Send + Sync + 'static) -> Self;
    pub fn filter_with<D: Dep>(self, param: D, f: impl Fn(&D::Value, &U) -> bool + ..) -> Self;   // a Signal / Computed parameter
    pub fn map<V: SignalValue>(self, f: impl Fn(&U) -> V + ..) -> Derive<T, V, O>;
    pub fn build(self) -> DerivedList<U>;
    pub fn count(self) -> Computed<u32>;             // the view's length, O(log n) per changed row, O(1) per read
}
impl<T, U> Derive<T, U, Unsorted> {
    pub fn sort_by_key<K: Ord + Clone + Send + Sync + 'static>(self, f: impl Fn(&U) -> K + ..) -> Derive<T, U, Sorted<K>>;   // stable: ties in source order
    pub fn sort_by_key_with<D: Dep, K: ..>(self, param: D, f: impl Fn(&D::Value, &U) -> K + ..) -> Derive<T, U, Sorted<K>>;
}
pub struct DerivedList<T>;   // Clone = the same list; Send + Sync; Dep (its value is the materialised Vec<T>)
impl<T: SignalValue> DerivedList<T> {
    pub fn len(&self) -> usize;  pub fn is_empty(&self) -> bool;   // O(1) once caught up
    pub fn get(&self) -> Vec<T>;  pub fn with<R>(&self, f: impl FnOnce(&Vec<T>) -> R) -> R;   // materialise, cached until the view changes
    pub fn ptr_eq(&self, other: &Self) -> bool;  pub fn is_attached(&self) -> bool;  pub fn stats(&self) -> DerivedStats;
}
pub struct DerivedStats { pub rebuilds: u64, pub full_values: u64, pub patches: u64, pub ops_emitted: u64 }   // #[non_exhaustive]
pub struct Effect;         // Effect::new(deps, f) runs f after each commit that dirtied a dep; dropped or `cancel(self)` = cancelled
pub fn txn<R>(f: impl FnOnce() -> R) -> R;        // batch; nested calls join the outer transaction; exception safe
pub trait ChangeSink: Send + Sync {
    fn deliver(&self, change_set: &[u8]);
    fn deliver_from(&self, owner: u64, change_set: &[u8]) { self.deliver(change_set) }   // what every commit calls, with the store's owner (ADR-035); the runtime's sink routes by it through its registry of live runtimes, and a store with owner 0 delivers nothing
    fn off_core_write(&self, owner: u64, message: &str) {}   // a refused write, reported just before the E0065 panic; the runtime's sink logs it at error level through the owning runtime
    fn computed_failed(&self, owner: u64, handle: u64, signal_id: u32, message: &str) {}   // a computed panicked and is held back (ADR-019 amendment); once per transition, outside the delivery lock
    fn computed_recovered(&self, owner: u64, handle: u64, signal_id: u32) {}
    fn round_cap_hit(&self, rounds: usize) {}     // default no-op; the runtime's sink logs it at error level (ADR-020)
}
pub fn set_sink(sink: Arc<dyn ChangeSink>);      // installed by the runtime; a global, one per process
pub fn set_write_checker(f: fn(owner: u64) -> bool);   // installed by the runtime; a global, the first one installed stays. EVERY build asks f(owner) before every write to a signal that is attached (owner = the cell's owner, 0 until published) or has dependents (owner 0), and refuses the write (E0065 panic, or WriteError from try_set/try_update) when it says no (ADR-035). The runtime's checker is an allowlist (ADR-023): a thread holding that runtime's core lock (any core lock for owner 0), TestRuntime driver threads, testing::unchecked_writes
pub fn clear_write_checker();                    // stops consulting it (tests)
pub fn clear_sink();  pub fn with_sink<R>(sink: Arc<dyn ChangeSink>, f: impl FnOnce() -> R) -> R;   // the latter is thread-scoped, for tests
pub fn next_txn_id() -> u64;
pub mod testing { pub struct CaptureSink; }      // records change-sets: take(), take_decoded()
```

Commit algorithm: on outermost `txn` exit (or after a bare `set`), for each dirty `StoreCell` with a handle: recompute observed dirty computeds in dependency order; encode entries, ordered by `signal_id`, for signals that are observed or `no_coalesce` (keyed lists as a patch when one is possible and worthwhile, else the full value); one `ChangeSet` payload per store per transaction (never split; stores committed by one transaction share a `txn_id`); deliver via the sink; run effects; clear dirty bits. Signals that are dirty but unobserved stay marked so `observe(on)` sends fresh values. Writes before `attach`/`set_handle` are plain writes with no delivery. Each computed is evaluated under a panic guard (ADR-019 amendment): one that panics is left out of the change-set and held back (the cell's failed set, reported through `ChangeSink::computed_failed` once), not retried until its inputs change; the other slots are delivered and the writing call does not see the panic. The claim is otherwise transactional (ADR-019): if building or delivering a store's change-set is abandoned (an encoder or the sink panicked), its slots are remembered as unsent and the keyed baselines among them are dropped, and the next commit that touches the store sends every one of them again as a full value; `observe(on)` of a slot clears its unsent mark. A failed `observe(on)` appends no entries and leaves no target newly observed. An unknown `signal_id` in `observe` is ignored (returns 0) in every build. The only lock held while user code runs is the store's **delivery lock** (ADR-020): a commit takes it when it claims the store's dirty slots and releases it when the sink has returned (`observe_and_deliver` holds it from before the entries are encoded until `deliver` has returned), so the change-sets of one store reach the sink one at a time, in claim order, with strictly increasing `txn_id` (a transaction id shared by several stores is replaced for a store that has already seen a newer one). Sinks and computed closures therefore run under it: a sink must not wait for another thread that writes the same store (writes made on the sink's own thread are queued and are fine), and the runtime's sink only hands the payload to the host. No other lock is held while user code (sink, effect, computed closure, encoder) runs, and effects run after the delivery lock is released. Writes from two threads do not form one transaction: a write to a slot that another thread has dirty in an open transaction ships with that transaction. Signal writes belong on the core; every build enforces it through `set_write_checker` (ADR-035). A commit is bounded: after 1000 rounds (effects, computeds or sinks that keep writing signals that trigger themselves) it stops running effects, delivers the changes that are already dirty one last time, drops the still-queued effects from the queue (a later change queues them again), releases whatever that delivery queued (those slots are remembered as unsent, as for an abandoned change-set), and reports `ChangeSink::round_cap_hit(1000)`. Nothing stays claimed by the capped thread, so other threads' writes to those slots and effects work normally.

Keyed lists (SPEC 3.8): the cell keeps the list as the host last saw it (the *baseline*: one clone per *observed* keyed signal; an unobserved `no_coalesce` keyed list keeps none and is delivered as a full value each time) and sends a patch of `Insert`/`Remove`/`Update`/`Move`/`Clear` ops. A commit finds the ops one of two ways, decided by how the list was written (ADR-027):

* **Recorded operations** (`Signal<Vec<I>>::push`, `insert`, `remove`, `update_at`, `move_item`, `clear`) append the op they perform to a per-signal op log, in the same critical section (the value's write lock) that mutates the list. The commit sends the log as the patch and replays it on the baseline: O(ops), independent of the list's length (the `memmove` a `Vec` insertion or removal in the middle costs aside, which the host pays too); no key is hashed and no item is compared. The ops are sent as recorded: no more-than-50%-removed or key-overlap fallback, no key-uniqueness check, and no attempt at minimality (three mutations are three ops). The commit takes the log under the value's read lock, so the ops it takes are exactly the ones the list it sees includes; it checks that the ops' indices fit the baseline's length as they replay and that the result has the list's current length, and diffs instead if not (a safety net, not a code path).
* **Raw writes** (`set`, `update`, `replace`) mark the log *stale* before they touch the list, and the commit **diffs** the list against the baseline: O(list). It sends the full value instead when the lists share no key (including empty to non-empty and back), a key occurs twice, or more than half of the old items were removed. Items with equal keys are compared by their encoded bytes. A transaction that contains a raw write is diffed as a whole, however many recorded operations it also made.

Derived lists (ADR-039): `list.derive()` then `filter` / `filter_with` / `map` / `sort_by_key` / `sort_by_key_with`, then `build()` (a `DerivedList<U>`, attachable with `attach_derived`) or `count()` (a `Computed<u32>`). Its value is always `stable_sort_by_key([out(t) for t in source if passes(t)])`, the stages fused into one per-row function and at most one stable sort (ties keep source order). It is not recomputed:

* **Taps.** A list signal keeps one *tap* per derived list built on it (by `Weak`; dropping the list drops it). Every recorded operation appends its op to each tap that is recording, inside the value's write lock, after the list changed (a `Clone` of the item that panics there leaves the item in the list and makes every tap stale); every raw write marks every tap stale first. A tap records nothing until its list is first read or observed, and holds at most 4,096 ops: past that it goes stale and frees them.
* **Index.** Two arena AVL order-statistic trees with parent pointers: one node per source row in source order, counting rows and passing rows per subtree, and (with a sort stage) one node per passing row ordered by `(key, source position)`. A **drain** replays the tap's ops on it, O(log n) each (O(log² n) when many rows share a key), each emitting at most two ops of the view, by ADR-039 section 2's table: a filter transition is an `Insert` or a `Remove`, a sort-key change a `Move` then an `Update`, an op that does not change the view nothing. A stale or unread tap, a raw write or a drain that unwound **rebuilds** the index from the list (O(n log n)). A parameter change (`filter_with`, `sort_by_key_with`; compared by its encoded bytes) replays the ops with the old value, then walks every row with the new one: at most 256 ops are sent, else the full value. Reads (`len`, `get`, `with`, `count()`, a `Computed` over it), commits and `observe(on)` drain first (a read inside a transaction therefore sees the writes the transaction already made, as a signal read does; the host gets them at the commit); a drain that needs the items takes the ops and an `Arc` of the list together under the value's read lock, as the keyed slot does.
* **Delivery.** The derived slot keeps the ops it emitted while it is observed (at most 4,096; past that the full value). A commit sends them as one keyed patch, the full value when they overflowed, the index was rebuilt or the host has no copy (`no_coalesce` while unobserved: always the full value), or nothing. `observe(on)` sends the full value and keeps ops from that instant; `observe(off)`, an abandoned delivery and an observe rollback drop them. A drain whose closure panics is isolated as a computed is (held back, `failed_signals`), and the next drain rebuilds.
* **Purity and re-entrancy.** Pipeline closures must be pure functions of their arguments (the list re-evaluates a row only when the row changes). Debug builds panic when one reads or writes a signal, a computed or a derived list; release builds do not check. A closure that reads its own list panics with "derived list cycle detected" in every build; reading a list inside its source's `update` / `update_at` closure panics with the ADR-021 L3 message. Lock order: the store's delivery lock, the list's drain lock (held while its closures run), the source's value lock (read), a tap (a leaf); a drain runs inside a transaction, so a write some closure makes commits after the drain lock is released.

The log lives while the baseline does: it starts empty whenever a baseline is taken (`observe(on)`, the full value an abandoned or first commit sends) and is dropped with it (`observe(off)`, an abandoned commit, ADR-019), so nothing is recorded for a slot nobody observes and a delivery that may not have reached the host never leaves ops that would be replayed twice. It is bounded: a log that outgrows `max(4096, list length)` ops goes stale and the commit diffs. Ops recorded while no sink is installed (or before the store has a handle) stay in the log and are sent by the next commit that delivers. A panic inside `update_at`'s closure makes the log stale.

Attach failures leave the failed signal unattached, but signals attached before it stay bound to the discarded cell, so a store whose attach failed is unusable and must not be published. Generated code therefore builds the cell with `?` and the constructor's dispatch arm answers `DispatchResult::BadRequest` with the `SignalsError` text (§16.3).

### 16.2 undra-runtime

```rust
pub struct Runtime;                                 // Arc<Runtime>; Runtime::init registers the process global, Runtime::new does not (any number can coexist)
pub struct RuntimeConfig { platform: String, mode: String /* "inproc" | "dev" */, core_threads: u8, blocking_threads: u8, log_level: u8 }  // hand-written Encode/Decode
pub enum InitError { AlreadyInitialized, InvalidMode(String), Spawn(String) }
pub trait Host: Send + Sync + 'static {            // implemented by undra-ffi, the wasm shell, the transport server and testing::RecordingHost
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
    pub fn init(config: RuntimeConfig, host: Arc<dyn Host>) -> Result<Arc<Runtime>, InitError>;   // executor, object/port/dispatch tables, change sink, init hooks, `undra-core` thread (unless core_threads == 0)
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
    pub fn restore(&self, payload: &[u8]) -> Result<(), RestoreError>;   // all or nothing (one exception: a store type this build no longer has is left out); migrates by name when a store type's fingerprint differs (ADR-037); cancels in-flight calls on replaced receivers (§5.9); resumes the generation counter above the snapshot's floor (ADR-022)
    pub fn restore_with_report(&self, payload: &[u8]) -> Result<RestoreReport, RestoreError>;   // the same, and what it did: RestoreReport { restored, migrated: Vec<String>, dropped: Vec<DroppedStore { type_id, name, handles }>, schema_changed }
    pub fn stats_json(&self) -> String;              // includes `strong_refs`: strong references besides the global slot (ADR-034)
    pub fn schema(&self) -> &undra_meta::Schema; pub fn schema_hash(&self) -> u64;
    pub fn register_inspector(&self, name: &'static str, inspect: InspectFn);   // ADR-054: the one seam dev tooling reads layered state through; InspectFn = Arc<dyn Fn() -> String + Send + Sync> answers one JSON document; `undra-query` registers "queries" (its cache) with the client; a later registration under a name replaces the earlier; the core never reads inspectors; an inspector holds what it describes weakly (ADR-034)
    pub fn inspect(&self, name: &str) -> Option<String>; pub fn inspectors(&self) -> Vec<&'static str>;   // the inspector runs on the caller's thread with no runtime lock taken by this call; a panicking inspector answers None, is logged once (level 5, `undra::panic`, counted in `panics`) and is skipped until a new one is registered under its name
    pub fn ctx(&self) -> Ctx;
    // What generated code calls:
    pub fn object<T: Send + Sync + 'static>(&self, handle: u64) -> Result<Arc<T>, object_table::BadHandle>;   // Display says null / unknown / stale / wrong type
    pub fn sync_ok<T>(&self, value: &T, encode: fn(&T, &mut Writer)) -> DispatchOutcome;   // a sync method's success (status 0): written into the armed reply slot under call_sync, else DispatchResult::Sync(Ok(..)); `encode` is `Encode::encode` (ADR-028)
    pub fn sync_err<E>(&self, error: &E, encode: fn(&E, &mut Writer)) -> DispatchOutcome;  // its typed error (status 1), likewise
    pub fn insert_object<T: UndraObject>(&self, object: Arc<T>) -> Handle;   // stores (a StoreRestorer is registered for the type) get their cell's handle set
    pub fn insert_store<T: StoreObject>(&self, object: Arc<T>) -> Handle;
    pub fn bind_port<P: ?Sized + 'static>(&self, port_id: u32, imp: Arc<dyn Any + Send + Sync>);   // imp is an Arc<Arc<P>> behind Any
    pub fn bind_dyn_port<P: ?Sized + Send + Sync + 'static>(&self, port_id: u32, imp: Arc<P>);      // does the wrapping
    pub fn bind_dyn_port_with<P: ?Sized + Send + Sync + 'static>(&self, port_id: u32, imp: Arc<P>, dispatcher: &'static PortDispatcher);   // and raw port calls on it run through `dispatcher` (a standard port's, ADR-052)
    pub fn bind_foreign_port(&self, port_id: u32);  pub fn unbind_port(&self, port_id: u32) -> bool;
    pub fn rust_port<P: ?Sized + Send + Sync + 'static>(&self, port_id: u32) -> Option<Arc<P>>;
    pub fn extension<T: Default + Send + Sync + 'static>(&self) -> &T;      // per-runtime state of layered crates (undra-query)
    pub fn try_extension<T: Send + Sync + 'static>(&self) -> Option<&T>;    // the same, without creating it
}
#[derive(Clone)] pub struct Ctx(..);   // §5.3; an Arc<Runtime>. `Ctx::current()` / `try_current()` read a thread-local set by dispatch, by the executor while polling and by `Ctx::enter()`. `downgrade() -> WeakCtx`, `closed() -> Closed` (ADR-034)
#[derive(Clone)] pub struct WeakCtx(..);   // §5.3; a Weak<Runtime> plus the runtime's lifeline: upgrade() -> Result<Ctx, Gone>, is_alive(), closed(), sleep(d) -> WeakSleep (Output = Result<(), Gone>), runtime_id()
pub enum Gone { ShutDown, Dropped }
impl Ctx {
    pub fn txn<R>(&self, f: impl FnOnce() -> R) -> R;                       // one transaction, delivered through this runtime
    pub fn spawn(&self, fut: impl Future<Output = ()> + Send + 'static) -> TaskId;   pub fn cancel_task(&self, id: TaskId);   // the cancelled future is dropped on the core (core lock held, or queued for the core's next turn); after shutdown spawn/sleep/port_call/event are logged no-ops
    pub fn spawn_blocking<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> BlockingTask<T>;
    pub fn sleep(&self, d: Duration) -> Sleep;                              // through the host's timer or the internal one
    pub fn events(&self) -> &Events;                                        // subscribe(port_id, method_id, Box<dyn Fn(&Ctx, &[u8]) + Send + Sync>) -> Subscription; the subscriber gets the runtime's Ctx as an argument so it never captures one (ADR-034)
    pub fn port_call(&self, port_id: u32, method_id: u32, args: Vec<u8>) -> PortFuture;
    pub fn port_call_sync(&self, port_id: u32, method_id: u32, args: &[u8]) -> Result<Vec<u8>, PortError>;
    pub fn rust_port<P: ?Sized + Send + Sync + 'static>(&self, port_id: u32) -> Option<Arc<P>>;   pub fn bind_port / bind_dyn_port / bind_dyn_port_with;
    pub fn enter(&self) -> CtxScope;  pub fn runtime(&self) -> &Runtime;
    pub fn with_core<R>(&self, f: impl FnOnce() -> R) -> Result<R, Reentrant>;   // ADR-035: takes the core lock on the calling thread, runs f in one transaction; the sanctioned synchronous write from a host thread; Err(Reentrant) from a host callback or the core
}
// The typed accessors of §5.3 are not methods: `#[undra::port]` generates a free function `<trait_snake>(ctx: &Ctx) -> Arc<dyn Trait>` (the Rust binding, else a proxy to the
// platform); `ctx.query()` / `ctx.mutate()` come from undra-query as extension traits over `Ctx`.

pub enum DispatchResult {
    Sync(Result<Vec<u8>, Vec<u8>>),                 // Ok bytes: status 0; Err bytes: status 1 (typed error). Generated dispatchers do not build this for a sync method any more: they call `Runtime::sync_ok` / `sync_err`, which return this when no reply slot is armed and otherwise write the reply into the slot (§5.6, ADR-028). Hand-written dispatchers and layers may still return it.
    Async(Pin<Box<dyn Future<Output = Result<Vec<u8>, Vec<u8>>> + Send>>),
    Stream(Pin<Box<dyn futures_core::Stream<Item = Result<Vec<u8>, Vec<u8>>> + Send>>),
    Unknown,                                        // status 5: the dispatcher does not implement this method id
    BadRequest(String),                             // status 5 with the reason: arguments that do not decode, stale or wrongly typed receiver, a store whose signals cannot attach
}
// Generated dispatchers are `undra_meta::DispatchFn = fn(&dyn Any, DispatchCall<'_>) -> DispatchOutcome`: they downcast `&dyn Any` to `&Runtime` and return `DispatchOutcome::new(DispatchResult::..)` (Async, Stream, BadRequest, Unknown) or, for a sync method's answer, `rt.sync_ok(..)` / `rt.sync_err(..)`. The outcome of those two is either a `DispatchResult::Sync` or a zero-sized runtime-private marker meaning "the reply is in this thread's slot"; `DispatchOutcome` stays an opaque `Box<dyn Any + Send>`, so `undra-meta` is unchanged and a zero-sized value boxes without allocating.
pub trait UndraObject: Send + Sync + 'static { const TYPE_ID: u32; const NAME: &'static str; }
pub trait StoreObject: UndraObject {
    fn cell(&self) -> &Arc<undra_signals::StoreCell>;
    fn restore(ctx: Ctx, r: &mut undra_wire::Reader<'_>) -> Result<Self, undra_wire::WireError> where Self: Sized;   // r is positioned at the store BODY (`signal_count u32, signals × { signal_id u32, len u32, value }`)
}
pub struct StoreRestorer {                           // submitted with `inventory::submit!` by #[undra::store], one per store type
    pub type_id: u32,
    pub restore: fn(Ctx, u64 /* the re-issued handle */, &mut Reader<'_>) -> Result<Arc<dyn Any + Send + Sync>, WireError>,   // builds the store and tells its cell the handle
    pub cell: fn(&(dyn Any + Send + Sync)) -> Option<&Arc<StoreCell>>,   // how the runtime recognises a store inside `dyn Any`
}
pub trait Port: Send + Sync + 'static { const PORT_ID: u32; const NAME: &'static str; const KIND: undra_meta::PortKind; }   // implemented for `dyn Trait`
pub struct PortDispatcher {                          // submitted by #[undra::port], one per port trait: how a Rust binding answers an encoded call. The standard request/reply ports' are not submitted but are statics (`undra_ports::KV_DISPATCHER`, ...) that a binding passes to `bind_dyn_port_with` (`fakes::install` does), so a core that binds no Rust implementation of one does not link them (ADR-052)
    pub port_id: u32,
    pub dispatch: fn(imp: &(dyn Any + Send + Sync), method_id: u32, args: &[u8]) -> PortDispatch,   // imp is the Arc<Arc<dyn Trait>> given to bind_port
}
pub enum PortDispatch { Sync(Vec<u8>), Async(Pin<Box<dyn Future<Output = Vec<u8>> + Send>>) }   // bytes: `status u8` (0 ok, 1 typed error, 2 unavailable) then the body
pub struct PortFuture;    // Future<Output = Result<Vec<u8>, PortError>>; dropping it abandons the call
pub fn port_call_sync(rt: &Runtime, port_id: u32, method_id: u32, args: &[u8]) -> Result<Vec<u8>, PortError>;
pub enum PortError { Unavailable, Cancelled, Decode(undra_wire::WireError), Failed(Vec<u8> /* encoded E */) }   // Clone + Debug + Display + Error
pub enum RestoreError { Decode(WireError), UnknownStoreType { type_id: u32 } /* no longer produced since ADR-037: such a store is left out and reported */, Store { type_id: u32, source: WireError }, Panicked { type_id: u32, message: String }, BadHandle { handle: u64 } /* null, duplicate, generation 0 or u32::MAX, index too far */, GenerationFloor { floor: u32 } /* floor == u32::MAX */, Incompatible { type_id: u32, store: String, signal: String, reason: String } /* ADR-037: a signal neither converts structurally nor through a hook; restore code 7 */, ShutDown, Reentrant }
pub mod persist;          // ADR-037: DynValue, DynRecord, MigrateError, decode_dyn, encode_dyn, decode_params, encode_params, migrate (streamed), migrate_value, migrate_params; Migration { name, target: MigrationTarget::{Type, Signal { store, signal }, Mutation}, from: Option<u64>, returns, hook: MigrationHook::{Value, Signal, Mutation} } submitted by #[undra::migrate]; migrations(), check_migrations(&Schema) (the start-up E0066 check)
pub struct StatsSection { pub name: &'static str, pub json: fn(&Runtime) -> Option<String> }   // a section of stats_json a layered crate adds at run time, Runtime::add_stats_section (undra-query's "query", added when its client is created), one per name
pub struct InitHook { pub name: &'static str, pub run: fn(&Ctx) }   // submitted through inventory, run for every new runtime, once per name (ADR-052: the code that needs a hook submits it, so undra-query's is submitted by every #[undra::query] / #[undra::mutation]; DispatchLayer likewise, consulted once per name)
pub mod object_table;     // ObjectTable, BadHandle, BadHandleReason: generation-tagged slots; Handle issue/lookup/release
pub mod log;              // level constants TRACE..FATAL and `log::log(level, target, msg)`
pub mod executor;         // spawn, spawn_blocking, sleep, cancel, yield_now, Notify, TaskId
pub mod testing;          // TestRuntime: a real Runtime with no core or timer thread, a manual clock, the real blocking pool (run_pending/run_until/advance wait for its closures), a private generation counter and a RecordingHost; unchecked_writes(f) lifts the write check on one thread, drive_from_this_thread() marks a harness thread a driver; live_threads() counts the runtime threads still running (ADR-034 tests); call_sync_reference(rt, payload) is call_sync through the allocating path (§5.6);
                          // call / call_sync / run_pending / run_until(fut) / advance(Duration) / take_replies; host(): take_decoded_change_sets, take_stream_items,
                          // take_port_calls, take_timeline, take_logs, script_port*(port_id, method_id, ..); helpers call_payload, decode_reply, port_reply, port_reply_ok, sync_ok
```
`undra-runtime` re-exports `undra_signals`, `undra_wire`, `undra_meta` (and `undra_meta::inventory`) and `futures_core::Stream`. The `undra` facade re-exports `undra_runtime as runtime`, `undra_signals as signals`, `undra_wire as wire`, `undra_meta as meta`, `undra_runtime::persist as persist`, the seven macros (`#[undra::migrate]` since ADR-037) at the crate root and in `prelude` (with `DynValue`, `DynRecord`, `MigrateError`), `pub mod query` (`QueryDef`, `MutationDef`, `BoxFuture`, `CacheValue`, the traits undra-query implements the client against), and `prelude::*` = `{Signal, Computed, DerivedList, Effect, Ctx, Bytes, Uuid, Timestamp, Duration, txn, Handle}` plus the macros. (`undra_ports as ports` and `undra_query as query` join the facade when those crates land; until then `undra::query` is the trait module.)

Ids are `fnv1a` hashes computed in `undra_meta::ids`: `type_id(name)`, `method_id(type, method)` (constructors are methods called `new`, or whatever the fn is named), `function_id(name)`, `port_id(trait)`, `port_method_id(trait, method)`, `query_id`, `mutation_id`, `fnv1a64(bytes)` (keyed-list keys).

### 16.3 What the macros emit (paths)

Generated code uses absolute paths through the facade: `::undra::wire::{Encode, Decode, Writer, Reader, WireError}`, `::undra::meta::{inventory, Registration, RecordMeta, ObjectMeta, StoreMeta, SignalMeta, .., DispatchCall, DispatchOutcome, ids}`, `::undra::runtime::{Runtime, Ctx, DispatchResult, UndraObject, StoreObject, StoreRestorer, Port, PortDispatcher, PortDispatch, PortError, Stream, Subscription}`, `::undra::signals::{Signal, StoreCell, CellSlot, SignalsError}` (a store with a `DerivedList` field calls `attach_derived` on its cell), `::undra::query::{QueryDef, MutationDef, QueryRegistration, MutationRegistration, __private::{HYDRATE, LAYER}}`. A `#[undra(crate = "path")]` attribute (or `crate = "path"` in the macro arguments) overrides the root (for undra-ports and tests inside the workspace, which use `::undra_runtime` directly).

Every type position the schema records is also checked against the type it resolves to, at compile time, in the user's crate: built-in mappings by a same-type assertion (E0060), `Named` mappings by comparing the inherent `UNDRA_TYPE_ID` of the type with `type_id` of the recorded name (E0061; a type without the constant is not declared with Undra at all, `0`, and says so), with `UNDRA_IS_ERROR` required on the error side of a `Result` and `__UNDRA_IS_OBJECT` refusing an object used as a value (E0064). `impl StoreObject` is written by the `#[undra::api(store)]` impl block next to `impl UndraObject`, forwarding to hidden members of the struct.

Diagnostics that need the compiler (v1.x, D1):

* A record carries one hidden member for keyed lists: `__UNDRA_FIELDS: &[&str]`, the field names in declaration order (a constant: nothing is generated per field). `#[undra(key = "id")]` looks the name up in a constant with `undra_meta::keys::index_of`, so a key that names no field is E0008 on the string, listing the fields. The key function then reads the field by name through a reference typed `&<__UndraGate<{ found }> as __UndraPass<Item>>::Out`, which is `&Item` when the check passed and has no type when it failed, so no `rustc` error about an unknown field can follow the diagnostic. A type that is not a record gets an empty list from a fallback trait declared in the key function.
* A type takes one `#[undra::api] impl` block (§4.1). A macro cannot see another block, so the expansion defines `_undra_error_E0007_<Type>_has_two_undra_api_impl_blocks_merge_them_into_one`; a second block redefines it and `rustc` reports that name first. The other items a block defines that are not `impl`s (the dispatcher, the registration, the store probe) live in an anonymous `const _` block, so only what Rust itself forbids conflicts (`impl UndraObject`, `__UNDRA_IS_OBJECT`).
* A query or mutation cannot see whether it sits in an `impl` block either. Its struct, inherent constants, statics and registrations are declared through a `macro_rules!` named `_undra_error_E0007_a_query_is_a_free_function_move_it_out_of_the_impl_block` (`mutation` for a mutation), invoked at once; in an impl block `rustc` then reports a misplaced macro definition and that name as an unknown macro, and nothing else. The `QueryDef` impl (it holds the user's parameter types) and the checks (a named constant, valid in an impl block) stay outside, so an error on a type the user wrote does not name the guard. A signature that names `Self` is reported directly (E0007), for a query and for `#[undra::api] fn`.
* The assertion that an impl block and its struct agree on being a store (E0011) is the length of an array in the signature of a private function (`fn __undra_store_probe() -> [(); { assert!(..); 0 }]`): `rustc` evaluates it while it checks signatures, before any function body, so E0011 comes before the errors the bodies have because of it (a struct literal patched with `__undra_cell` on a struct that is not a store).
* The assertion that a future is `Send` is a function named `_undra_error_E0022_the_future_of_an_async_method_must_be_Send`: `rustc` prints the name of the bound it checks, which is how its own error finds its code.
* The message of a port proxy method without an error channel (E0062) is assembled from the same four-line shape as a compile diagnostic (`Diag::runtime_template`).

`#[undra::store]` and its impl block:

* The struct must be marked on its impl block: `#[undra::api(store)] impl Todos { .. }`. The marker is what wires the constructors to the signals; a struct without `#[undra::store]`, or an impl block without the marker, is E0011.
* The macro appends a hidden field `pub __undra_cell: ::undra::signals::CellSlot`. Struct literals of the type inside its `#[undra::api(store)]` impl block get `__undra_cell: Default::default()` added; struct literals anywhere else must spell it out. The field name is reserved (E0007).
* `#[undra::store(restore = "Self::assemble")]` names the function `restore` rebuilds the store with: `fn(ctx: Ctx, <one Signal<T> per non-computed signal, in declaration order>) -> Self`, the same code the constructor uses. Without it the store is rebuilt by a struct literal, which works only when every non-signal field is a `Ctx` (cloned from the argument), a `WeakCtx` (downgraded from it, ADR-034: what a store should keep, since a `Ctx` field is a reference cycle with the runtime until shutdown) or `Default`; a store with a `Computed` or `DerivedList` field needs the hook (E0013). Snapshots hold the plain signals only.
* Field kinds: `Signal<T>`, `Computed<T>` and `DerivedList<T>` are signals, numbered in declaration order; every other field is private state. A `DerivedList<T>` is recorded as `Vec<T>`, `computed: true`, with its `#[undra(key = "..")]` (required, E0008); one without its row type is E0001. `Lazy<T>` is rejected in v1 (E0001, "lazy lists are not available in v1"), like `undra-bindgen` rejects it.
* The signal table is built by a hidden method that calls, per field and in order, `attach` (plain), `attach_keyed` with a generated `fn(&Item) -> u64` (`#[undra(key = "id")]`: `fnv1a64` over the encoded key field, encoded through a per-thread scratch buffer), `attach_computed`, or `attach_derived` with the same kind of key function for a `DerivedList<T>`, each followed by `set_no_coalesce(id)` for `#[undra(no_coalesce)]`, all with `?`. `StoreObject::cell()` creates the cell once through `CellSlot::get_or_init`; a store whose attach failed gets an empty cell there rather than a panic.
* A constructor's dispatch arm first calls `__undra_attach_all()` (`CellSlot::get_or_try_init`); on `Err(SignalsError)` nothing is inserted and the arm answers `DispatchResult::BadRequest("store `Todos` could not attach its signals: <error>")`. Otherwise it inserts the object (`Runtime::insert_object`, which gives the cell its handle) and replies with the handle (`u64`). `restore` fails with `WireError::InvalidTag` if the rebuilt store cannot attach.
* One `StoreRestorer` per store type is submitted through `inventory`.

`#[undra::query]` and `#[undra::mutation]` submit, besides `Registration::Query` and their `QueryRegistration` / `MutationRegistration`, the query runtime's init hook and dispatch layer: `inventory::submit! { ::undra::query::__private::HYDRATE }` and `{ ::undra::query::__private::LAYER }`. `undra-query` submits neither itself, so a core that declares no query and no mutation does not link the query runtime (ADR-052: 34 KB of the 136 KB gzipped hello-world web core); the runtime runs one init hook and consults one dispatch layer per name, however many definitions submitted it.

Dispatch arms answer `BadRequest` with a reason for arguments that do not decode (naming the argument and `Type.method`) and for a receiver handle that does not resolve to that type (the `BadHandle` text); `Unknown` is only for a method id the dispatcher does not implement.

A synchronous arm ends in `__rt.sync_ok(&value, ::undra::wire::Encode::encode)` (a `Result<T, E>` method: `Ok(v)` goes to `sync_ok`, `Err(e)` to `sync_err`, a constructor answers its handle with `sync_ok`), which is a `DispatchOutcome` already; an `async` or stream arm wraps its `DispatchResult::{Async, Stream}` in the dispatcher's `__undra_out(..)`. The encoder is passed as the path `Encode::encode`, not as an `Encode` bound on `sync_ok`, so a type that cannot cross the boundary is still reported by the one E0001 diagnostic of §12 and not by a second "required by a bound in `Runtime::sync_ok`" note (ADR-028).

---

## 17. Platform runtime base API (what generated code depends on)

Generated code defaults to its own core through its generated entry (§10, ADR-044), which each runtime backs with one small type: `UndraCoreEntry` (Swift), `CoreEntry` with `NativeApi` (Kotlin), and `UndraCore.load`/`attach` with `UndraCore.unloaded` (TypeScript). `UndraCore.shared` is for app code.

Generated code calls only these names. Runtimes implement them; bindgen golden files pin the usage.

### 17.1 TypeScript (`@undra/runtime`)

```ts
export class UndraCore {
  static load(opts: LoadOptions): Promise<UndraCore>;          // { mode: 'wasm-main' | 'wasm-worker' | 'remote', wasm?: URL | BufferSource, url?: string /* ws:// for remote */, adapters?: Partial<Adapters>, expectedSchemaHash: bigint, mirror?: { schedule?, maxPendingEntries?, maxPendingBytes? } /* §11.1 */, onDevNotice?: (message: string) => void /* dev only, remote cores served by `undra dev`, §5.10 (ADR-053) */, worker?: WorkerLike | (() => WorkerLike) | WorkerModeOptions /* { create?, ports?: URL | string }, ADR-049 */ }
                                                                // the wasm modes reject UndraTransportError("unsupported", "WebCrypto is required ...") before instantiating when crypto.getRandomValues is missing (§7)
                                                                // AttachOptions (load and attach): recovery?: CrashRecovery /* crashRecovery({ snapshotEveryMs = 1000, maxSnapshotBytes = 4 MiB, maxRestarts = 3, perMs = 60000 }), one per core; a core loaded without one ships none of the recovery code (ADR-052) */, onCoreRestarted?(e: UndraCoreRestarted), onPanic?(r: UndraPanicReport) /* ADR-046: once per contained panic (a native core: the `Diagnostics` port; a wasm core: once per trap, built from the FATAL record and the trap's stack, before any restart), in order, never throwing into the core (a throwing handler goes to onError); with none set a native core's report is logged, one line */, backgroundRun?: boolean /* default true: when the page goes to the background (`visibilitychange` to hidden, `pagehide`, `freeze`) the lifecycle adapter reports Background and, if `stats().background.pending > 0`, `runInBackground(1000)` runs without being awaited; Background Sync in a service worker is a follow-up */
                                                                // LoadOptions: namespace?: string /* `UndraIds.namespace`, which the generated entry passes; a wasm report's `namespace` */, coreVersion?: string /* a wasm report's `coreVersion`: the module carries none, default "" */
  static get shared(): UndraCore;                               // set by the first load; with none (or after it closed) a closed placeholder: its calls reject UndraCallError.Unavailable, access never throws
  static get unloaded(): UndraCore;                             // that closed placeholder; what a generated entry's `core` is while its core is not loaded (ADR-044)
  static get current(): UndraCore | null;                       // the loaded shared core, or null (then `shared` is the placeholder)
  report(error: unknown, operation: string): void;              // a failure no caller can see: logs at error level, calls onError(UndraUnhandledError); never throws (ADR-032, amendment A); a failure that is a remote core's connection being down (Unavailable while `connection` is reconnecting, or closed for a reason other than "requested") is logged at warning level and not delivered (ADR-051); only logs a failure reported while onError runs or of a call onError started
  callSync(target: CallTarget, methodId: number, args: Uint8Array): Uint8Array;        // only mode 'wasm-main'; others throw UndraModeError; drains the mirror before it returns
  call(target: CallTarget, methodId: number, args: Uint8Array, signal?: AbortSignal): Promise<Uint8Array>;   // resolves with reply body (status ok) or rejects with UndraReplyError { status, body }
  stream(target: CallTarget, methodId: number, args: Uint8Array): AsyncIterable<Uint8Array>;   // handles credit; ends with UndraReplyError { status 1, body E } for flag 2, or with the status and §3.4 body of a flag-3 item (ADR-036)
  construct(typeId: number, methodId: number, args: Uint8Array): Promise<bigint>;     // returns handle
  observe(handle: bigint, signalId: number, on: boolean): void;
  release(handle: bigint): void;
  readonly connection: Signal<ConnectionState>;               // ADR-051: { kind: "connecting" } | { kind: "connected" } | { kind: "reconnecting", attempt, error } | { kind: "closed", reason: "requested" | "schemaMismatch" | "sessionLost" | "failed", error? }; LoadOptions.reconnect (false | { initialDelayMs, maxDelayMs, jitter, maxAttempts }), onConnectionChange; UndraSessionLostError
  mirror: Mirror;      // mirror.register(handle, applyFn: (signalId, op, value: Uint8Array) => void, options?: { noCoalesce?: Iterable<number> }); mirror.unregister(handle)
                       // mirror.stats(): MirrorStats; mirror.addDrainListener(fn: (s: DrainStats) => void): () => void  (§11.1)
  registerPort(portId: number, impl: PortImpl): void;          // PortImpl = { methods: Record<number, (args: Uint8Array) => Uint8Array | Promise<Uint8Array>>, sync: boolean, name?: string /* generated adapters set it: messages name the port */ }; a sync port in 'wasm-worker' throws UndraError("options") (register it in worker.ports)
  stats(): Promise<UndraStats>;                                // ..., mirror: MirrorStats
  snapshot(): Promise<Uint8Array>;                             // §5.9: the persisted state of every store, opaque; wasm modes (a socket: UndraModeError)
  restore(bytes: Uint8Array): Promise<void>;                   // §5.9: rebuilds the stores, same handles; resolves after the restored values reached the stores (the mirror is flushed); a refused snapshot rejects UndraRestoreError { code } and the core is unchanged
}
export abstract class UndraObject { protected constructor(core: UndraCore, handle: bigint); readonly core; readonly handle: bigint /* a query handle re-created after a recovery keeps its wrapper: the runtime moves the registration */; close(): void; [Symbol.dispose](): void }
export abstract class UndraStore extends UndraObject { protected constructor(core: UndraCore, handle: bigint, options?: { noCoalesce?: readonly number[]; recreate?: RecreateCall /* { typeId, methodId, args }: generated query handles pass their constructor call (ADR-049) */ }); protected _signals: Signal<unknown>[]; protected _apply(signalId: number, op: ChangeOp, value: Uint8Array): void /* implemented by generated code */; protected _observeAll(): Promise<void> /* generated create() calls it: closes the store and rejects UndraCallError when the core is gone */ }
export interface MirrorStats { changeSetsReceived; entriesReceived; entriesApplied; drains; compactions; resyncs; pendingEntries; pendingBytes; droppedEntries }   // numbers
export interface DrainStats { changeSets: number; entries: number; appliedEntries: number; durationMs: number }
export function scheduleFrame(fn: () => void): void;          // the default schedule (§11.1)
export class Signal<T> { get(): T; peek(): T; subscribe(fn: (v: T) => void): () => void; /* internal */ _set(v: T): void }
export class UndraError extends Error { readonly kind: string }   // the root of everything the runtime throws on purpose; WireError (kind 'wire') is one
export class UndraReplyError extends UndraError { status: ReplyStatus; body: Uint8Array }   // the raw reply failure of call/callSync; generated code maps it
/// What a generated call rejects with when the failure is neither its own `E` nor the caller's abort (ADR-032, amendment A).
export abstract class UndraCallError extends UndraError {            // kind: 'cancelledByCore' | 'panicked' | 'refused' | 'unavailable' | 'malformed'
  static mapped(error: unknown): unknown;                            // generated methods without an `E`: an abort reason or a foreign error stays itself
  static mapped<E>(error: unknown, domain: Codec<E>): unknown;       // with an `E`: status 1 becomes `E`
  static mappedStream(error: unknown, domain?: Codec<unknown>): unknown;   // a stream ends in the vocabulary of a failed reply (ADR-036): flag 3 maps by its status (3 CancelledByCore, 2 Panicked, 5 Refused), flag 2 decodes as `E` (Malformed if it does not, or when there is no `domain`)
}
// namespace UndraCallError: CancelledByCore | Panicked { panicMessage, backtrace } | Refused { reason } | Unavailable { transport: UndraTransportError } | Malformed { detail }; type UndraCallFailure = their union
export class UndraUnhandledError extends UndraError { operation: string; error: UndraCallError }   // kind 'unhandled'; what `onError` receives (`AttachOptions.onError?: (error: UndraUnhandledError) => void`)
export class UndraRestoreError extends UndraError { code: number }   // kind 'restore'; the non-zero undra_restore code: PANICKED = 2 (a store's restore panicked), BAD_SNAPSHOT = 5, UNAVAILABLE = 6 (core shut down), INCOMPATIBLE = 7 (ADR-037), as static constants
export function crashRecovery(options?: RecoveryOptions): CrashRecovery;   // ADR-049: CrashRecovery { options, lastSnapshot, keepSnapshotNow() }, for LoadOptions.recovery
export class UndraCoreRestarted extends UndraUnhandledError { report: UndraPanicReport; restoredFromAgeMs: number | null; rejectedCalls: number; staleObjects: number }   // ADR-049; operation "wasm core", error Panicked
export interface UndraPanicFrame { address: bigint; symbol: string | null; file: string | null; line: number | null }   // ADR-046: the shape of §8's PanicFrame; a wasm frame's address is the module offset of `wasm-function[i]:0x..`, its symbol the name or `wasm-function[i]`, file and line null
export interface UndraPanicReport { message: string; location: string; operation: string; thread: string; frames: readonly UndraPanicFrame[]; namespace: string; coreVersion: string; schemaHash: bigint; imageId: string }   // the same nine fields on every platform (§5.6, §8)
export interface UndraBackgroundReport { finished: boolean; replayed: number; refetched: number; stillPending: number }
// TransportFailure adds "restarted": what a call or stream in flight across a recovery restart fails with (a generated call sees UndraCallError.Unavailable)
export abstract class StorageError extends UndraError { static from(error: unknown): StorageError }   // namespace StorageError: Unavailable { value } | Full | Locked | Corrupt { value } | Io { value } (ADR-049, wire order of §8); StorageErrorCodec; FsError adds Full and Unavailable { value }; fsErrorFrom(e). The Kv, SecureStore and Fs ports type only StorageError / FsError instances; an adapter maps a raw platform error with StorageError.from / fsErrorFrom
export interface WorkerPortsModule { default?: Record<number, PortImpl>; adapters?: { clock?, rng?, timer? } }   // the worker.ports module (§11.1)
export interface SnapshotPayload { generationFloor: number; schemaHash: bigint; types: { typeId: number; fingerprint: bigint }[]; description: string; stores: StoreSnapshot[] }   // wire/payloads.ts, §5.9 layout 2
export interface UndraPort {}
```
Ports: generated port interfaces are plain TS interfaces; `core.registerPort(id, generatedAdapter(impl))` wraps an implementation with codecs (bindgen emits the adapter).

`runInBackground(deadlineMs: number, options?: { signal?: AbortSignal }): Promise<UndraBackgroundReport>` (ADR-046, §5.11) calls the standard function `run_background` (`0x0e5b14ff`, one `u64`); an abort cancels the call and rejects with the signal's reason; failures are `UndraCallError`. `UndraStats` gains `panicReports` and `background: { tasks, pending, runs, finished, replayed, refetched }` (all `0` for a core that reports none). The panic report of a trap is built by a lazily imported module (only an app with `onPanic` or `crashRecovery()` fetches it) and `imageId` is the SHA-256 of the module's bytes (computed in the background when `onPanic` is set, `""` for a precompiled `WebAssembly.Module`). A native core reached through `remote`, the React Native host or a custom transport receives its reports through the `Diagnostics` port adapter (`diagnosticsPort(onPanic)`), registered for every non-wasm mode.

Snapshot and restore: `snapshot()` and `restore()` call `undra_snapshot` / `undra_restore` (§7) in `wasm-main` and, as control messages answered in order behind the messages sent before them, in `wasm-worker`; a closed core rejects `UndraTransportError('closed')`, a transport without them (`remote`) `UndraModeError`. Calls and streams in flight across a restore end as §5.9 says (status 3, stream error `"cancelled: ..."`). A snapshot is opaque bytes: one taken in either wasm mode restores into a core loaded in the other, or into a fresh core of the same module, or (ADR-037) into a newer build of the core, migrating by name.

Recovery (ADR-049, wasm modes only; off by default). With `recovery: crashRecovery(options?)` (an object the app imports, so a core loaded without it ships none of the recovery code, ADR-052; one per core), the runtime keeps a snapshot where the core runs (the main thread in `wasm-main`, the worker in `wasm-worker`): after the core emitted a change-set, at most once per `snapshotEveryMs`, in `requestIdleCallback` where there is one; a snapshot over `maxSnapshotBytes` is not kept (the previous one stays) and is reported once. When the core traps: (1) `onPanic` receives the panic report (it is called for every trap, with or without recovery); (2) every call and stream in flight fails with `UndraTransportError("restarted")` (a generated call: `UndraCallError.Unavailable`), never retried, and calls made until the restart completes fail the same way; (3) the same compiled module is instantiated again and the kept snapshot restored (§7), so stores keep their handles; (4) every observed store is observed again; (5) query handles are re-created from their recorded constructor call (`recreate`) behind the same wrapper objects; (6) `onCoreRestarted` and then `onError` receive one `UndraCoreRestarted`. Lost: store writes after the last snapshot, objects that are not stores (query handles excepted: calls on them are refused), the core's running tasks and timers, and core state outside stores (configuration a call set: re-apply it in `onCoreRestarted`; re-created query handles fetch before it runs). A port reply that settles after the restart, for a call of the instance that trapped, is dropped. One trap more than `maxRestarts` within `perMs` and the core stays down: `onClose` reports the trap as without recovery, so a deterministic panic cannot loop. Bench rows `ts/snapshot_take_100kb` (budget 2 ms) and `ts/recovery_restart_100kb` (budget 50 ms, until `onCoreRestarted`).

Transport (internal to the package, for alternative transports): `portAdded?(portId, impl)` (a port registered after load; throws `UndraError("options")` for a sync port in `wasm-worker`), `restart?(generationFloor): Promise<RestartResult>`; `TransportHandler.ports?()`; `WasmMainTransport.takeSnapshot()` and `twin()` (a new transport over the same compiled module). Recovery is a layer over the core's transport: `crashRecovery().attach(transport, host)` returns the transport the core then uses.

Ports in `wasm-worker` mode (ADR-049, worker protocol 3, §11.1): the core cannot wait for the main thread, so synchronous ports are answered in the worker. `Clock`, `Rng` and `Log` need nothing: the worker's `port_call` import returns 2 for them (§7) and the shell's built-in bindings serve them from the worker's own `Date.now`, `crypto.getRandomValues` and `log` import, whose records the worker relays to the main thread's Log adapter. An app's synchronous port, and an override of Clock, Rng or the core's timers, goes in the module named by `worker: { ports: URL }` (`WorkerPortsModule`: a default export of port implementations by id, and an optional `adapters` export `{ clock?, rng?, timer? }`), which the worker imports before `undra_init`. A synchronous port registered on the main thread is refused at `load` (and by a later `registerPort`) with `UndraError("options")` naming the port; `adapters.clock`, `adapters.rng` and `adapters.timer` do not reach the worker (a warning, once). Ports with asynchronous methods (Http, Kv, SecureStore, Fs, the app's async ports) cross to the main thread and are answered there, as in `wasm-main`; the worker learns which ones at start and after each `registerPort`. The `undra init` web template loads `wasm-main`; `wasm-worker` is opt-in (`mode: 'wasm-worker'`).

Generated stores call `super(core, handle)`, or `super(core, handle, { noCoalesce: [ids] })` when the store has `no_coalesce` signals.

### 17.2 Kotlin (`dev.undra.runtime`)

```kotlin
class UndraCore private constructor(...) {
  companion object { fun load(options: LoadOptions): UndraCore; fun load(options: LoadOptions, native: NativeApi): UndraCore; val shared: UndraCore; val current: UndraCore? }   // LoadOptions(mode = Mode.INPROC | Mode.REMOTE, remoteUrl, adapters, expectedSchemaHash: ULong? = null, mirror = MirrorOptions(...), onError: ((UndraUnhandledError) -> Unit)? = null, onDevNotice: ((String) -> Unit)? = null /* dev only: a REMOTE core served by `undra dev`, delivered on the runtime's delivery thread, §5.10 (ADR-053) */, onPanic: ((UndraPanicReport) -> Unit)? = null /* last; ADR-046: once per contained panic, in order, on the runtime's main dispatcher; a throwing handler is caught, logged and reported to onError (operation `onPanic`); with none set the report is logged, one ERROR line */); an in-process core needs its natives (the generated UndraCoreNative, ADR-044): load(options) with Mode.INPROC is refused (UndraModeException) and says to use Undra<Namespace>.load
  // for generated code (ADR-044): class CoreEntry(namespace, schemaHash, native: () -> NativeApi) { fun load(options = LoadOptions()): UndraCore; val core: UndraCore }; interface NativeApi (namespace, isAvailable, unavailableReason and the natives of §6.1); interface NativeCallbacks; object NativeLibrary { fun load(namespace): Throwable? }
  // the in-process claim is per namespace: two cores of two namespaces load side by side; one namespace loads once
                                                                                            // `shared` with no core loaded (or after it closed) is a closed placeholder: its calls throw UndraTransportException(CLOSED), generated code reports UndraCallError.Unavailable; access never throws; `current` is null then
  fun callSync(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray            // reply body or throws UndraReplyException; on the main thread, drains the mirror first
  suspend fun call(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray         // cancellable
  suspend fun runInBackground(deadlineMs: Long): UndraBackgroundReport                      // ADR-046, §5.11: the standard function `run_background`; coroutine cancellation cancels the call; failures are UndraCallError; `UndraStats.panicReports` and `UndraStats.background` (tasks, pending, runs, finished, replayed, refetched; -1 for a core that reports none)
  fun stream(target: CallTarget, methodId: UInt, args: ByteArray): Flow<ByteArray>          // ends with UndraReplyException(ERROR, E) for flag 2, or with the status and §3.4 body of a flag-3 item (ADR-036)
  fun construct(typeId: UInt, methodId: UInt, args: ByteArray): Long                        // sync in INPROC
  fun constructObject(typeId: UInt, methodId: UInt, args: ByteArray): Long                  // construct, its failures mapped onto UndraCallError (what generated secondary constructors call)
  fun report(error: Throwable, operation: String)                                           // a failure no caller can see: logs at error level, calls onError(UndraUnhandledError); never throws (ADR-032, amendment A); an Unavailable of reason CONNECTION_LOST (the remote connection is down, which connectionState reports) is only logged, at warning level (ADR-051)
  fun observe(handle: Long, signalId: UInt, on: Boolean); fun release(handle: Long)
  val connectionState: StateFlow<ConnectionState>                                           // ADR-051: Connecting | Connected | Reconnecting(attempt, cause) | Closed(reason: REQUESTED | SCHEMA_MISMATCH | SESSION_LOST | FAILED, cause); LoadOptions(reconnect = ReconnectPolicy(), onConnectionChange); UndraSessionLostException
  val mirror: Mirror                                                                        // register(handle) { signalId, op, reader -> }; register(handle, noCoalesce: Set<UInt>) { ... }
                                                                                            // stats(): MirrorStats; addDrainListener { s: DrainStats -> }: AutoCloseable; flush()  (§11.1)
  fun registerPort(portId: UInt, impl: PortImpl)
  fun stats(): UndraStats                                                                   // ..., mirror: MirrorStats
}
class MirrorOptions(framePacer: FramePacer? = null /* the paced default */, maxPendingEntries: Int = 65_536, maxPendingBytes: Long = 16 MiB)
fun interface FramePacer { fun requestFrame(frame: Runnable) }                              // dev.undra.android.ChoreographerFramePacer (module android-adapters) on Android
class MirrorStats(changeSetsReceived: Long, entriesReceived: Long, entriesApplied: Long, drains: Long, compactions: Long, resyncs: Long, pendingEntries: Int, pendingBytes: Long, droppedEntries: Long)
class DrainStats(changeSets: Int, entries: Int, appliedEntries: Int, duration: Duration)
abstract class UndraObject(val core: UndraCore, val handle: Long) : AutoCloseable
abstract class UndraStore(core: UndraCore, handle: Long, noCoalesce: Set<UInt> = emptySet()) : UndraObject(core, handle) { protected abstract fun apply(signalId: UInt, op: ChangeOp, reader: UndraReader); protected fun <T> signal(initial: T): MutableStateFlow<T>; protected fun observeAll() /* closes the store and throws UndraCallError when the core is gone */ }
open class UndraException(message: String, cause: Throwable? = null) : RuntimeException(message, cause)   // the root of everything the runtime throws on purpose; WireException is one
class UndraReplyException(val status: ReplyStatus, val body: ByteArray) : UndraException(..)   // the raw reply failure of callSync/call/stream/construct; generated code maps it
class UndraTransportException(val reason: Reason /* CLOSED, TIMEOUT, CONNECTION_LOST, INTERRUPTED */, message: String, cause: Throwable? = null) : UndraException(..)   // what every transport throws, RemoteTransport included (a connection that is down, failed or reconnecting is CONNECTION_LOST; ADR-051); mapped to UndraCallError.Unavailable
class UndraProtocolException(message: String, cause: Throwable? = null) : UndraException(..)   // a malformed reply, a reply for another call, the null handle
class UndraRestoreException(val code: Int) : UndraException(..)                                  // restore refused; the core is unchanged
/** What a generated call throws when the failure is neither its own `E` nor the caller's cancellation (ADR-032, amendment A). */
sealed class UndraCallError : UndraException {
  class CancelledByCore : UndraCallError()                                       // status 3
  class Panicked(val panicMessage: String, val backtrace: String) : UndraCallError()   // status 2 (a stream panic has an empty backtrace)
  class Refused(val reason: String) : UndraCallError()                           // status 5, and the re-entrancy refusal (reason E_REENTRANT)
  class Unavailable(val transport: UndraTransportException) : UndraCallError()   // closed, not loaded, connection lost, timeout (a remote core that changed schema included)
  class Malformed(val detail: String, cause: Throwable? = null) : UndraCallError()   // a reply, a result or an `E` that does not decode (a bug in Undra after a successful schema check)
  companion object {
    fun mapped(error: Throwable): Throwable                                       // generated methods without an `E`; CancellationException and foreign throwables stay themselves
    fun <E : Throwable> mapped(error: Throwable, domain: UndraCodec<E>): Throwable   // with an `E`: status 1 becomes `E`
    fun mappedStream(error: Throwable): Throwable                                 // a stream without an `E`: flag 3 maps by its status (3 CancelledByCore, 2 Panicked, 5 Refused; ADR-036), a flag-2 item is Malformed
    fun <E : Throwable> mappedStream(error: Throwable, domain: UndraCodec<E>): Throwable   // a stream with an `E`: flag 2 decodes as `E` (Malformed if it does not), flag 3 as above
  }
}
class UndraUnhandledError(val operation: String, val error: UndraCallError) : UndraException(..)   // what `LoadOptions.onError` receives; runs synchronously on the calling thread (the delivery thread for a malformed change-set or a failed port); must not call into Undra
interface UndraPort
```
**Production operations (ADR-046).** `UndraPanicFrame`, `UndraPanicReport` and `UndraBackgroundReport` are data classes in `dev.undra.runtime.adapters` with companion codecs (`address` and `schemaHash` are `Long`, `line` and the counts `Int`); `StandardPorts.Diagnostics` pins the port (`0xab68cd7cu`, `PANICKED` `0xbd147e2eu`, asserted by `crates/undra-ports/tests/ids.rs`) and `StandardFunctions.RUN_BACKGROUND` the function. The core registers its own `Diagnostics` adapter even with `defaultAdapters = false` (it feeds `onPanic`; an adapter of the app's for that port replaces it). `android-adapters` already reports Active, Inactive and Background by default (activity counting with a 700 ms settling, not `ProcessLifecycleOwner`: no `lifecycle-process` dependency); `AndroidLifecycleAdapter.attach(core, onBackgroundWorkPending)` reads `stats().background.pending` right after it reported `Background` and calls the callback on the main thread, and `AndroidPlatformDefaults.install(.., reportLifecycle = true, onBackgroundWorkPending = null)` adds the opt-out and the callback. The optional module **`android-work`** (`dev.undra:android-work`, WorkManager 2.10, kept out of the base runtime): `UndraWork.configure(loader: (Context) -> UndraCore)`, `UndraWork.schedule(context)` (unique work, `NetworkType.CONNECTED`, `ExistingWorkPolicy.KEEP`), `scheduleIfPending(context, core)`, `cancel(context)`; `UndraWorker : CoroutineWorker` loads the core with the app's loader, runs `runInBackground` with 9 minutes less a 15 s margin and less the time already spent (WorkManager stops a worker at 10), returns `success()` when the run finished and `retry()` otherwise or when the loader or the run fails (`failure()` with no loader registered), and cancels the call in the core when it is stopped. A stopped worker loses nothing: the core keeps what it did.

Main-thread delivery through `UndraDispatchers.main` (Android: `Dispatchers.Main.immediate`; JVM: a single-thread executor), at the frames of `MirrorOptions.framePacer` (§11.1). Generated stores extend `UndraStore(core, handle)`, or `UndraStore(core, handle, noCoalesce = setOf(ids))` when the store has `no_coalesce` signals.

### 17.3 Swift (`UndraRuntime`)

```swift
public final class UndraCore: @unchecked Sendable {
  public static func load(_ options: LoadOptions) throws -> UndraCore     // .inproc(api:adapters:) | .remote(url:adapters:), expectedSchemaHash (UInt64?); LoadOptions.maxPendingEntries / maxPendingBytes (§11.1). In process, `api` is the core's table (ADR-044): read and checked (abi_version 2, size, schema_hash) before init; the claim is per namespace
  // for generated code (ADR-044): public final class UndraCoreEntry(namespace:schemaHash:api:) { func load(_:) throws -> UndraCore; var core: UndraCore }
  public static var shared: UndraCore { get }   // the loaded core, or a shut-down placeholder (calls on it fail with `UndraCallError.unavailable(.closed)`); `current` stays nil then
  public func callSync(_ target: CallTarget, method: UInt32, args: [UInt8]) throws -> [UInt8]   // on the main thread, drains the mirror before it returns
  public func call(_ target: CallTarget, method: UInt32, args: [UInt8]) async throws -> [UInt8]   // cancellation-aware
  public func stream(_ target: CallTarget, method: UInt32, args: [UInt8]) -> AsyncThrowingStream<[UInt8], Error>
  public func stream<Item: Sendable>(_ target: CallTarget, method: UInt32, args: [UInt8], decode: @escaping @Sendable ([UInt8]) throws -> Item,
                                     mapError: @escaping @Sendable (Error) -> Error = { $0 }) -> AsyncThrowingStream<Item, Error>   // what generated stream methods return; decodes on demand so credit follows the consumer (§3.7)
  public func construct(type: UInt32, method: UInt32, args: [UInt8]) throws -> UndraHandle
  public func observe(_ handle: UndraHandle, signal: UInt32, on: Bool); public func release(_ handle: UndraHandle)
  public var connectionState: UndraConnectionState { get }   // ADR-051: .connecting | .connected | .reconnecting(attempt:) | .closed(UndraClosedReason: .requested | .schemaMismatch | .sessionLost | .failed); also `connection` (@MainActor @Observable, `.state`) and `connectionStates() -> AsyncStream`; LoadOptions.reconnect (UndraReconnectPolicy), onConnectionChange; UndraSessionLostError
  public let mirror: Mirror        // register(handle, noCoalesce: Set<UInt32> = []) { @MainActor (signalId, op, reader) in … }; stats() -> MirrorStats;
                                   // addDrainListener { @MainActor (DrainStats) in … } -> DrainListenerRegistration (remove()); @MainActor flush()  (§11.1)
  public func registerPort(_ id: UInt32, _ impl: PortImpl)   // a shut-down core (and the `shared` placeholder) ignores it, with a warning
  public func stats() -> UndraStats   // ..., mirror: MirrorStats, panicReports, background (UndraBackgroundStats: tasks, pending, runs, finished, replayed, refetched)
  public func runInBackground(deadline: TimeInterval) async throws -> UndraBackgroundReport   // ADR-046, §5.11: the standard function `run_background` (milliseconds, rounded up); cancelling the Task cancels the call (CancellationError); a closed core throws .unavailable(.closed)
  public func report(_ error: any Error, operation: String)   // a failure no caller can see: logs at error level, then calls LoadOptions.onError (ADR-032); generated commands and store `apply` call it; a failure that is a remote core's connection being down (.unavailable while connectionState is .reconnecting, or .closed for a reason other than .requested) is only logged, at warning level (ADR-051)
}
public struct MirrorStats: Sendable, Equatable { changeSetsReceived, entriesReceived, entriesApplied, drains, compactions, resyncs, pendingEntries, pendingBytes, droppedEntries: Int }
public struct DrainStats: Sendable, Equatable { changeSets: Int; entries: Int; appliedEntries: Int; duration: Duration }
open class UndraObject: @unchecked Sendable { public init(core: UndraCore, handle: UndraHandle); public func close() }
@MainActor open class UndraStore: UndraObject { public init(core: UndraCore, handle: UndraHandle, noCoalesce: Set<UInt32> = []); open func apply(signal: UInt32, op: ChangeOp, reader: inout UndraReader) }   // generated subclass is @Observable
public struct UndraReplyError: Error { public let status: ReplyStatus; public let body: [UInt8] }   // what the raw entry points throw
public struct LoadOptions: Sendable { …; public var onError: (@Sendable (UndraUnhandledError) -> Void)?; public var onPanic: (@Sendable (UndraPanicReport) -> Void)? /* ADR-046: once per contained panic, in order, on the main queue, never called back into the core; with none set the report is logged, one ERROR line; the last parameter of every initialiser */; public var onDevNotice: (@Sendable (String) -> Void)? /* dev only: a remote core served by `undra dev` (also `.remote(…, onDevNotice:)`), runs on a queue of the runtime's, §5.10 (ADR-053) */ }   // runs synchronously on the calling thread (the main actor for a store); must not call into Undra
/// What a generated method throws when the call itself fails: not its own `E`, not `CancellationError` (ADR-032).
public enum UndraCallError: Error, Sendable, Equatable, CustomStringConvertible, LocalizedError {
  case cancelledByCore                                  // status 3
  case panicked(message: String, backtrace: String)     // status 2 (a stream panic: a flag-3 item with status 2 carries the backtrace too)
  case refused(reason: String)                          // status 5 and the `undra_call` rejection
  case unavailable(UndraTransportError)                 // this UndraCore is shut down, not loaded, or its connection closed or timed out (a remote core that changed schema included)
  case malformed(String)                                // a reply, a result or an `E` that does not decode (a bug in Undra after a successful schema check)
  public static func mapped(_ error: any Error) -> any Error                                        // generated methods without an `E`
  public static func mapped<E: UndraError>(_ error: any Error, domain: E.Type) -> any Error         // with an `E`: status 1 becomes `E`
  public static func mapped(streamFailure error: any Error) -> any Error                            // generated stream methods: a flag-3 item maps like a failed reply with its status; a flag-2 item on a stream without `E` is .malformed (ADR-036)
  public static func mapped<E: UndraError>(streamFailure error: any Error, domain: E.Type) -> any Error   // flag 2 decodes as `E` (.malformed if it does not)
}   // `mapped` returns `E`, `CancellationError` or an `UndraCallError`
public struct UndraUnhandledError: Error, Sendable, Equatable, CustomStringConvertible, LocalizedError { public let operation: String; public let error: UndraCallError }   // what `onError` receives
public protocol UndraRecord: UndraCodec, Sendable, Hashable {}; public protocol UndraEnum: UndraCodec, Sendable, Hashable {}; public protocol UndraError: UndraCodec, Error, Sendable, Hashable {}; public protocol UndraPort {}
```
**Production operations (ADR-046).** The report types are public: `UndraPanicFrame`, `UndraPanicReport` and `UndraBackgroundReport` (`Sendable`, `Hashable`, `Codable`, the codecs of §8). The default `Diagnostics` adapter (`DiagnosticsAdapter`, part of `Adapters.platformDefault` like `Log` and `Clock`; a core loaded with `Adapters.none` has none unless the app adds it) decodes the report, answers the port at once and hands it to `LoadOptions.onPanic` on the main queue. `UndraBackground` (iOS, `BackgroundTasks`): `UndraBackground.register(taskIdentifier:options:loader:)` at launch registers a `BGProcessingTask` under `<id>.processing` (network; a window of minutes, 180 s of work by default) and a `BGAppRefreshTask` under `<id>.refresh` (about 30 s, 25 s of work); the handler loads the core with `loader` if the app was launched in the background, calls `runInBackground(deadline:)`, cancels it from the expiration handler and calls `setTaskCompleted(success:)` once, with `finished && stillPending == 0`; a window that did not finish asks for another. When the app enters the background with `stats().background.pending > 0` (read after the core processed `Lifecycle.Background`) both requests are submitted and the transition is wrapped in `UIApplication.beginBackgroundTask` (a 20 s drain); with nothing pending nothing is scheduled; `BGTaskScheduler` errors are logged and swallowed. `undra init` writes `BGTaskSchedulerPermittedIdentifiers` (`$(PRODUCT_BUNDLE_IDENTIFIER).undra.processing` and `.refresh`) and the `processing` and `fetch` background modes to `Info.plist` and registers `UndraBackground` in the bootstrap. `BackgroundTasks` does not run in the iOS simulator (`submit` is unavailable and `_simulateLaunchForTaskWithIdentifier` finds nothing scheduled): the handler is tested with fakes (`BackgroundEngine`'s seams) and on a device. Lifecycle is reported by default from the `UIApplication` (iOS) and `NSApplication` (macOS) notifications (`LifecycleAdapter`, opt out with `.removing(portId:)`).

Swift payload types live under `enum Wire { … }` (`Wire.Log`, `Wire.Event`, …) to avoid clashing with generated port protocols. Generated stores call `super.init(core: core, handle: handle)`, or `super.init(core: core, handle: handle, noCoalesce: [ids])` when the store has `no_coalesce` signals. Drains run from a `CADisplayLink` on iOS, tvOS and visionOS and on the main actor's next turn elsewhere (§11.1).

### 17.4 React Native (`@undra/react-native`, ADR-038)

Generated code does not depend on this package: it is how a React Native app gets an `UndraCore` (§11.2), after which the generated TypeScript bindings and `@undra/runtime/react` are used as on the web.

```ts
export function loadNative(entry: NativeCoreEntry, options?: NativeLoadOptions): Promise<UndraCore>;  // entry = the generated Undra<Namespace> ({ namespace, schemaHash, attach? }); options = AttachOptions without the hash + { devtools?, logLevel?, platform? }; installs the namespace's module, attaches NativeTransport through entry.attach; again while open: the same core
export function installNative(namespace: string): UndraNativeModule;        // UndraNative.install(namespace) once, then globalThis.__undraNative[namespace]; UndraTransportError("unsupported") when the TurboModule is not linked or the core is not in the app
export class NativeTransport implements Transport {                          // mode "native", synchronous, callSync; the gate of §11.2 in start()
  constructor(options: { namespace: string; native?: UndraNativeModule; expectedSchemaHash: bigint; platform?: string; devtools?: boolean; logLevel?: number; onError?: (e: unknown) => void; nativePorts?: readonly number[] /* the standard ports the module answers itself */ });
  snapshot(): Uint8Array;                                                    // undra_snapshot
  counters(): NativeHostCounters;                                            // records, bytes, wakes, dropped, nativePortCalls, jsSyncPortCalls, unavailableSyncPortCalls
}
export function nativeFrameScheduler(native: UndraNativeModule, options: { isActive(): boolean; onError?(e: unknown): void }): (fn: () => void) => void;  // the mirror schedule of §11.1 under React Native
export function reactNativeAdapters(): AdapterOverrides;                     // { http: reactNativeHttp(), lifecycle: AppState }
export function reactNativeHttp(options?: { fetch?: typeof fetch }): HttpAdapter;  // Http over React Native's fetch; any status is a response; HttpError Network / Timeout / Cancelled / InvalidUrl
export function isHttpUrl(url: string): boolean;                             // http(s)://host[:port]...: what reactNativeHttp() accepts
export function nativePlatformDefaults(namespace: string): NativePlatformDefaults;  // { ports, kv?, fs?, secureStore?, error? }: the ports the module answers natively on this device (asked of the module of the core `namespace`; the same for every core)
export function nativeDefaultPorts(offered: { ports: readonly number[] }, options: { adapters?: AdapterOverrides; ports?: Record<number, PortImpl> }): number[];  // offered minus what the options override (a value or null)
export function portPlan(schemaJson: string): { ports: number[]; syncMethods: number[] };
export interface UndraNativeModule { /* the JSI object of one core: namespace, abiVersion, schemaHash, schemaJson, start(config, offset, length, ports, syncMethods, nativePorts?), platformDefaults, shutdown, call, callSync, cancel, streamCredit, observe, release, portReply, event, timerFired, snapshot, restore, statsJson, hostCounters, requestFrame; set by the transport: sink, portSync, frame */ }
```
The C++ host registers the `Diagnostics` port natively (ADR-046): a panic report from any core thread is queued as an ordinary `PortCall` record (call id 0) for the JavaScript thread, which hands it to `onPanic` once, in order (`loadNative` passes `onPanic` through); `runInBackground` and the stats go through `NativeTransport` unchanged. A core call with port call id 0 is never answered. Importing the package installs `TextDecoder` / `TextEncoder` where Hermes lacks them (import it before `@undra/runtime`); `@undra/react-native/babel-plugin` replaces `import.meta` for Hermes.

### 17.5 The testing kit (`docs/TESTING.md`, ADR-055)

Not a dependency of generated code or of an app's release build: it sits beside the runtimes and changes neither the wire, the C ABI, the schema nor a generated shape. Swift `UndraTestKit` (a product of the `UndraRuntime` package), Kotlin `dev.undra:testkit` (package `dev.undra.testkit`), TypeScript `@undra/testkit` (no runtime dependency beyond `@undra/runtime`), Rust `undra::testing` (crate `undra-testkit`). Each has the same parts:

* **The recording** (`undra.recording`, version 1): JSON, `{format, version, schema_hash: "0x..", source, platform?, events: [{t, kind, ...}]}`, `t` whole milliseconds since the session started, payloads as lower-case hex, handles as `"0x.."` strings, ids as numbers. Kinds: `call` (`target` `function` | `method` | `constructor` | `page`), `reply` (`status` `ok` | `error` | `panic` | `cancelled` | `stream_opened` | `bad_request`), `change_set` (entries `{handle, signal, op: full | patch | lazy_invalidated, value}`), `stream_item` (`flag` `item` | `end` | `error` | `failed`), `port_call` (+ informational `name` for the standard ports), `port_reply` (`status` `ok` | `error` | `unavailable`), `event`, `timer_fired`, `observe`, `release`, `cancel`. The writer is canonical (fixed key order, one event per line, nothing host- or clock-dependent) and every language's writer reproduces `testkit/fixtures/*.json` byte for byte; an unknown `format` or `version` is a typed error naming the field.
* **The seed** (`version` 1): `{now_ms, rng_seed, kv, secure_store, fs, http: [rules], connectivity, lifecycle}`, applied to the deterministic fakes of §8; a malformed value is a typed error naming its path.
* **`PreviewCore`**: the app's core loaded through its bindings' own entry (`Undra<Namespace>.load`, ADR-044; the TypeScript kit calls `UndraCore.load`) with the fakes as its adapters (`LoadOptions.adapters`, `defaultAdapters` off) and a manual clock whose `advance` fires due `Timer` reports and then waits for the core to be idle (at most `maxTimers` firings per call, 1,000 for a preview and 100,000 for a bare fake clock; the next one throws `TimerStormError` / `TimerStormException`, and the Rust fakes panic: a timer that re-arms itself without time passing never ends). Native cores run `ctx.sleep` on the runtime's own timer thread (§5.8): the manual clock moves `Clock` and port-armed timers there; on the web it moves sleeps too.
* **`RecordedCore`**: an `UndraCore` over a transport that plays a recording: replies by target and method in order (call ids rewritten), stream items, and change-sets released by a manual playhead. Built on the runtime seams below.
* **`PortRecorder` / `Replayer`** (and Rust's `Recorder` / `Replayer`): record the traffic of adapters, answer from a recording per port in order, report `mismatch`, `exhausted`, `unanswered` (a recorded call with no reply in the file, answered `Unavailable`) and `unconsumed` as typed values.
* **The seams** the kits are built on, additive and not for apps: Swift `UndraTransport`, `UndraInbound`, `PortCallOutcome`, `TransportInfo`, `TransportStartOptions` and `UndraCore.attach(transport:options:)` are `package` (visible to targets of the same package only); Kotlin `Transport`, `TransportEvents`, `PortOutcome` and `UndraCore.attachTransport` are public behind `@UndraEmbeddingApi` (opt-in, error level); TypeScript's `Transport`, `TransportHandler` and `UndraCore.attach` were already exported.
* **Capture**: `undra dev --record FILE` records the whole session from the dev server's envelope stream (`undra_transport::ServerConfig::tap`, a read-only `FrameTap`) plus the dev runner's native `Clock` and `Rng` readings, one file per core (a reload starts `NAME-2.EXT`, ...); `docs/DEV_LOOP.md`. The calls of the `SecureStore` port are recorded with empty arguments and replies unless `--record-secrets` (`Recorder::redact_secrets`); a file that cannot be written stops the recording (one warning) and not the server.
* **`CaptureDiagnostics`** (ADR-046): each kit has the fake of the `Diagnostics` port (`Fakes.diagnostics`: `reports`, `last`, `take()`, `clear()`), and `PreviewCore` records a trap's or a contained panic's report in it, next to the app's own `onPanic`.
* **Fakes conformance**: `testkit/conformance/fakes.json` is generated from `undra::ports::fakes` (`undra_testkit::conformance::generate`) and replayed against each kit's fakes.

