# undra-bindgen

Turns an [`undra_meta::Schema`](../undra-meta) into idiomatic Swift, Kotlin and TypeScript sources (docs/SPEC.md section 10). It is a library; `undra-cli` calls it after extracting the schema from the core.

Every language's output is derived from the schema alone (constitution rule R1), uses only the base API of the matching runtime package (SPEC section 17), and is pinned by golden files (R3).

```rust
use undra_bindgen::Generator;
use undra_meta::{FieldDef, RecordDef, Schema, TypeRef, ids};

let mut schema = Schema::new("demo-core");
schema.records.push(RecordDef {
    name: "Todo".into(),
    type_id: ids::type_id("Todo"),
    fields: vec![
        FieldDef { name: "id".into(), ty: TypeRef::Uuid, default: false, docs: String::new() },
        FieldDef { name: "title".into(), ty: TypeRef::String, default: false, docs: String::new() },
    ],
    transparent: false,
    docs: "A todo item.".into(),
});

// Names default from the crate name; override the public fields to taste.
let mut generator = Generator::for_crate(&schema.crate_name);
generator.ts_scope = "acme".into();

// `validate` runs `Schema::validate` plus the generator's own checks (E0050,
// E0051, E0001, ...); every generator method calls it first.
undra_bindgen::validate(&schema).unwrap();

let swift = generator.swift(&schema).unwrap();
assert_eq!(swift[0].path, "Sources/DemoCore/Generated/Types.swift");
assert!(swift[0].contents.contains("public struct Todo: UndraRecord"));

let kotlin = generator.kotlin(&schema).unwrap();
assert_eq!(kotlin[0].path, "src/main/kotlin/dev/undra/generated/demo_core/Types.kt");

let ts = generator.typescript(&schema).unwrap();
assert!(ts.iter().any(|f| f.path == "src/types.ts"));
assert!(ts.iter().any(|f| f.path == "package.json" && f.contents.contains("\"@acme/demo-core\"")));
```

## Output

| | Files |
|---|---|
| Swift | `Sources/<Module>/Generated/{Types,Errors,Objects,Stores,Ports,Queries,Ids}.swift` |
| Kotlin | `src/main/kotlin/<package path>/{Types,Errors,Objects,Stores,Ports,Queries,Ids}.kt` |
| TypeScript | `src/{types,errors,objects,stores,ports,queries,ids,index}.ts`, `package.json` (name `@<scope>/<crate>`, peer dependency `@undra/runtime`), `tsconfig.json` |

All files are always written, empty ones with only their header, so the file list depends on the language and the configuration, never on the schema. Items are sorted by name, so the output does not depend on the order in which `inventory` yielded the registrations.

Free functions live in `Objects`, mutations and query handles in `Queries`, event port emitters in `Ports`, and `UndraIds` (type, method, port and query ids for debugging, plus the schema hash to pass to `UndraCore.load`) in `Ids`.

### Naming

Type names are kept exactly as declared. Fields, parameters, methods and signals are `camelCase`; enum variants are `lowerCamel` cases in Swift, `UPPER_SNAKE` entries of unit enums and `PascalCase` classes of data enums and errors in Kotlin, and `"camelCase"` literals or `kind` values in TypeScript. Reserved words are escaped the way each language spells it: backticks in Swift and Kotlin, a trailing underscore in TypeScript binding positions (`default_`; property and method names may be reserved words in TypeScript and are left alone).

A constructor called `new` becomes `init` (Swift), `create` plus a convenience constructor (Kotlin) and `static create` (TypeScript); any other constructor is a static factory. The runtime's `UndraCore` is the **last** parameter of constructors and free functions (`ctx` in Swift and Kotlin, `core` in TypeScript), defaulting to the shared core. In Swift a method with a single record or enum parameter takes it unlabeled (`setFilter(_ f: Filter)`); every other parameter is labeled.

### Failures

A `Result<T, E>` becomes a thrown `E` in every language (Swift `throws`, Kotlin an exception, TypeScript a rejection). ADR-032 (and its amendment A for Kotlin and TypeScript) gives all three the same rule: a generated call fails with exactly one of three things, its own `E`, the caller's cancellation (`CancellationError`, `CancellationException`, the `AbortSignal`'s reason) or `UndraCallError` (a core panic, a cancellation by the core, a refused call, an unreachable core, a reply that does not decode). The generated code is one `do`/`catch` (`try`/`catch`) per call that hands the error to `UndraCallError.mapped(...)`, so the mapping lives once, in each runtime; a synchronous method with no result and no error type is a **command** that reports through `core.report` and returns instead of failing (`docs/ERRORS.md`).

A synchronous Swift method that returns nothing and has no error type is a **command** (`todos.toggle(id:)`). It cannot throw, because it is called from `Button` actions and binding setters, so it logs a failure, passes it to `LoadOptions.onError` and returns. Nothing generated traps.

Typed throws remain on **port requirements** only (`Generator::swift_typed_throws`): the host implements them, and `throws(HttpError)` tells the implementer exactly which errors the core can understand. With the option off they emit plain `throws`.

### The standard library

Every core links `undra-ports`, so its schema contains the ten standard ports (`Clock`, `Rng`, `Log`, `Http`, `Kv`, `SecureStore`, `Fs`, `Timer`, `Connectivity`, `Lifecycle`) and the eight types they exchange (`HttpMethod`, `Header`, `HttpRequest`, `HttpResponse`, `HttpError`, `FsError`, `NetKind`, `AppState`), and the schema hash covers them. The platform runtimes already implement the ports and ship the types, so the generators **leave them out** of an app's bindings and let references resolve to the runtime's own (ADR-024, SPEC 10.5):

| | The standard ports | A reference to a standard type |
|---|---|---|
| TypeScript | not generated | `import { type HttpRequest, HttpRequestCodec } from "@undra/runtime"` |
| Kotlin | not generated | `import dev.undra.runtime.adapters.HttpRequest` (its companion is the codec) |
| Swift | not generated | `import UndraRuntime` (`HttpRequest`, `HttpError`, ...; public, with `Codable`, `UndraRecord` and `UndraError` conformances); `AppState` is `UndraAppState` |

Only an **exact** match is left out: same name, same id and same shape (`undra_bindgen::stdlib` holds the table, ids pinned as hex and checked against `undra-ports`). Ids are derived from names, so an app type that only shares a name, `struct HttpRequest { url: String }` for example, has another shape and is generated as the app's own; one with a standard name and another id (a hand-written schema) is E0052. A standard type that refers to another (`HttpRequest` to `Header`) is left out only while that one is. `validate` accepts the standard entries, and an app may have a record named like a standard *port* (`Timer`, `Log`): the ports are not declared, so they claim no names.

`Generator::emit_standard_library` declares everything as ordinary items; `undra-ports` uses it to prove its own schema generates in all three languages.

## The runtime API the output depends on

SPEC section 17 lists the base classes; these are the names generated code actually calls, including the few the specification does not spell out (marked *addition*). The tests compile and run the output against the real wire layers plus hand-written stand-ins of exactly this surface (`tests/fixtures/`).

**Wire layer (exists today).** TypeScript `codecs`, `Codec<T>`, `UndraWriter`, `UndraReader`, `encodeValue`, `decodeValue`, `decodePatch`, `applyPatch`, `PatchError`, `WireError`, `ChangeOp`, `CallTarget`, `ReplyStatus`, `ALL_SIGNALS`; Kotlin `Codecs`, `UndraCodec`, `UndraWriter`, `UndraReader`, `KeyedPatch`, `WireException`, `Payloads.{CallTarget,ChangeOp,ReplyStatus}`, `Handle`, `Timestamp`, `decodeAll`, `encodeToByteArray`; Swift `UndraCodec`, `UndraWriter`, `UndraReader`, `UndraBytes`, `UndraHandle`, `WireError`, `PatchOp`, `decodePatch`, `applyPatch`, `PatchError`, `CallTarget`, `ChangeOp`, `ReplyStatus`, `Observe.allSignals`.

**Standard types (SPEC 8).** TypeScript `HttpMethod`, `Header`, `HttpRequest`, `HttpResponse`, `NetKind`, `AppState`, `HttpError`, `FsError` and their `<Name>Codec` (`adapters/types.ts`, `adapters/codecs.ts`, exported from `@undra/runtime`); Kotlin the same eight in `dev.undra.runtime.adapters`, each with a companion `UndraCodec`; Swift the same eight as public types of `UndraRuntime` (`HttpMethod`, `Header`, `HttpRequest`, `HttpResponse`, `HttpError`, `FsError`, `NetKind` and `UndraAppState`, which keeps the `Undra` prefix). Generated code maps a standard error like any other: `UndraCallError.mapped(error, HttpErrorCodec)` (TypeScript), `UndraCallError.mapped(e, HttpError)` (Kotlin, through the companion codec), `UndraCallError.mapped(_:domain: HttpError.self)` (Swift).

**Base classes (SPEC 17).** `UndraCore` (`callSync`, `call`, `stream`, `construct`, `observe`, `mirror`, `registerPort`, `shared`), `UndraObject`, `UndraStore` (Swift `apply`, Kotlin `apply`, `signal(initial)` and `observeAll()`, TypeScript `_apply`, `_signals` and `_observeAll()`), `Signal<T>`, `UndraError`, `UndraCallError`, `UndraCore.report`, `UndraReplyError` / `UndraReplyException`, `UndraPort`, `UndraRecord`, `UndraEnum`, `UndraException`, `PortImpl`.

**Additions.**

* Call targets carry the object: TypeScript passes `{ target: CallTarget.ObjectMethod, handle }` (or `{ target: CallTarget.FreeFunction }`) because the wire enum has no room for a handle; Swift and Kotlin pass the existing `CallTarget` values and repeat the method id as the separate argument SPEC 17 lists.
* `UndraCore.event(port, method, payload)`: host-to-core events of event ports (`undra_event`).
* Typed port failures: a generated port adapter throws `UndraPortError(body:)` / `UndraPortException(body)` / `UndraPortError(body)` carrying the encoded `E`; the runtime replies with port status 1.
* `PortImpl`: TypeScript `{ sync, methods }` as in SPEC 17.1, Kotlin `PortImpl(sync, methods: Map<UInt, suspend (ByteArray) -> ByteArray>)`, Swift `PortImpl.sync([UInt32: ([UInt8]) throws -> [UInt8]])` and `PortImpl.async([UInt32: ([UInt8]) async throws -> [UInt8]])`.
* `UndraCore.observe` returns `Promise<void>` in TypeScript, resolving once the initial change-set is applied; in process, Swift and Kotlin apply it inline before `observe` returns (SPEC 5.5), so a store never exposes its placeholder values.
* `new Signal<T>(initial)`, `new UndraError(kind, message, options?)`, and `UndraStore` subclasses call `super(core, handle)`; the base class registers with the mirror and unregisters on `close()`.
* `Sendable` on every generated port protocol (Swift 6 strict concurrency).

## Newtypes, generic instantiations and `Decimal` (ADR-042)

A schema record flagged `transparent` with the one field `value` is a **newtype** (`struct UserId(Uuid)`): it crosses as its inner value, byte for byte, and each language wraps it in its own idiom.

| | `struct UserId(Uuid)` | `struct Meters(f64)` |
|---|---|---|
| Swift | `public struct UserId: RawRepresentable, UndraRecord, Sendable, Hashable, Codable` with `rawValue`, `init(rawValue:)`, `init(_:)` and a single-value `Codable` | the same plus `Comparable` |
| Kotlin | `@JvmInline value class UserId(val value: UUID) : UndraRecord`, its companion the codec | the same plus `Comparable<Meters>` |
| TypeScript | `export type UserId = string & { readonly __brand: "UserId" }`, a function `UserId(value)` that brands without a check, and `UserIdCodec` | the same over `number` |

`Comparable` is generated where the inner type has an order that means something: integers, floats, `String`, `Timestamp`, `Duration`, `Decimal` and a newtype of one of those; not `Uuid` (Foundation's `UUID` is `Comparable` only from iOS 17), `bool`, bytes, records, enums or collections. There is no `ExpressibleBy*Literal` conformance: it would let any literal become a `UserId`. The inner type may be a scalar, a record, an enum, a `Vec<T>`, a map, an `Option<T>`, `Decimal` or another newtype; a newtype is a valid map key when the type it wraps is (`Schema::is_valid_map_key`), and a keyed list may be keyed on a newtype field (the patches are positional, so the generated code needs nothing).

Where a language cannot express the wrapper as ADR-042 draws it, it does what the language can: a Kotlin newtype of bytes is an ordinary `class` with `equals` and `hashCode` over the content (`equals` and `hashCode` are reserved for value classes, and a `ByteArray` compares by identity); a TypeScript newtype of a newtype is branded on what the inner one wraps (a brand on a brand is two `__brand` literals in one intersection, which TypeScript reduces to `never`), its constructor taking the inner newtype; and a TypeScript newtype of an option brands the value inside (`(string & Brand) | null`). An `Option<N>` where `N` wraps an option is a nested option (E0001), because Kotlin and TypeScript cannot tell `Some(None)` from `None`.

**Generic instantiations** need nothing in bindgen: `Page<Todo>` and `Loadable<User>` reach the schema as the plain records and enums `TodoPage` and `LoadableUser`, which generate like any others (the `generics` golden case shows them).

**`Decimal`** is `Foundation.Decimal` in Swift, `java.math.BigDecimal` in Kotlin (`equals` is scale-sensitive, as Kotlin engineers expect) and the `Decimal` class of `@undra/runtime` in TypeScript. It is never a map key; a newtype of one is `Comparable` (`BigDecimal.compareTo` ignores the scale).

## Query handles

Every `#[undra::query]` becomes a `<Name>QueryHandle` store with the five signals of SPEC section 9 (`data`, `status`, `error`, `fetching`, `updatedAt`; `error` is `String?` when the query has no error type), a `QueryStatus` unit enum, and the methods that `undra-query` must dispatch on handle objects:

| Method | Id |
|---|---|
| `refetch()` | `fnv1a32("query.refetch")` = `0x21d1b9e2` (`undra_bindgen::QUERY_REFETCH_ID`) |
| `invalidate()` | `fnv1a32("query.invalidate")` = `0x44cec2fa` (`undra_bindgen::QUERY_INVALIDATE_ID`) |
| `setPollInterval(interval)` | `fnv1a32("QueryHandle.set_poll_interval")` = `0xe327e53b` (`undra_meta::ids::SET_POLL_INTERVAL_METHOD_ID`); the argument is the wire `Option<Duration>`, and none clears this observer's override (ADR-043) |

`setPollInterval` is `setPollInterval(_ interval: Duration?)` in Swift (`UndraDuration?` below iOS 16), `setPollInterval(interval: Duration?)` (`kotlin.time.Duration`) in Kotlin and `setPollInterval(ms: Duration | null)` in TypeScript, where a `Duration` is a number of milliseconds.

An `infinite` query (ADR-043) is a **paged** list. Its `data` is the list of every row loaded, **keyed by the item key** (empty before the first page, never `nil`; the next page arrives as a keyed patch that appends), and its handle has two more signals and a command:

| | |
|---|---|
| signals 5 and 6 | `hasNextPage`, `fetchingNextPage` (`bool`) |
| `fetchNextPage()` | `fnv1a32("QueryHandle.fetch_next_page")` = `undra_meta::ids::FETCH_NEXT_PAGE_METHOD_ID`, no arguments |
| Swift | `loadMore(ifNeededFor: item, threshold: 5)` fetches the next page when `item` is among the last `threshold` rows, a next page exists and none is loading; the row type is `Identifiable` when its key field is called `id` (otherwise a view passes `id: \.<key>`) |
| Kotlin | the handle implements `dev.undra.runtime.InfiniteQuery` (`hasNextPage`, `fetchingNextPage` and `fetchNextPage()` are `override`s), what the Compose helper `LoadMoreWhenNearEnd` takes |
| TypeScript | `hasNextPage: Signal<boolean>`, `fetchingNextPage: Signal<boolean>`, `fetchNextPage(): Promise<void>`, what `useLoadMore` takes |

An infinite query's schema `returns` is `Vec<T>` (`E0073` otherwise); when it also carries an error type (`Result<Vec<T>, E>`) the handle's `error` signal is that `E`, as for any query.

Constructing a handle is a constructor call whose `type_id` and `method_id` are both the query id. A `#[undra::mutation]` becomes an async function that calls the mutation id as a free function.

## Lazy lists (ADR-043)

A `Lazy<T>` store signal is not a value the store keeps but a list the platform **pages through**: a runtime class, made with the store and handed the signal's change-set entries, never generated code.

| | Property | Made with |
|---|---|---|
| Swift | `public let books: UndraLazyList<Book>` (`UndraLazyListObject<Book>` in the `ObservableObject` mode) | `UndraLazyList(core: core)` before `super.init` |
| Kotlin | `val books: UndraLazyList<Book>` | `UndraLazyList(core, Book)`; the store's `close()` closes its lists |
| TypeScript | `readonly books: LazyList<Book>` | `new LazyList(this.core, BookCodec)` |

`apply` hands the entry to the list: op `Full` to `applyFull(reader)`, op `LazyInvalidated` to `applyInvalidated(reader)` (each consumes the reader and checks it is complete); a keyed patch is not something a lazy list takes and is ignored. A lazy list is not a signal: it has no placeholder value, and TypeScript leaves it out of `_signals`. It may be keyed (`#[undra(key = "id")]`) or derived (`computed`).

## Limitations

Rejected with a diagnostic instead of generating wrong code: object handles used as values, `Option<Option<T>>` and an option of a newtype that wraps an option (not representable in Kotlin and TypeScript), a `Result` whose error type is not an error enum, `()` outside return position. Swift records and enums derive `Codable` only when every field is `Codable`, so a record holding a `Duration` (Codable only from the Swift 6.0 standard library) or a data enum does not. `Bytes` nested inside `Vec` or `Map` decode one byte at a time in Swift; direct and optional `Bytes` use the bulk path. `#[undra(js_number)]` does not exist in the schema yet, so `Generator::ts_js_number` switches `i64` and `u64` to `number` for the whole package.

## Tests

`cargo test -p undra-bindgen` runs the golden comparison of every case (`tests/golden/<case>/schema.json` and the expected trees: `newtypes`, `generics`, `decimal`, `polling`, `infinite` and `lazy` are the cases of ADR-042 and ADR-043), the diagnostics tests, and, when the toolchains exist, type-checks and executes the TypeScript (`tsc` and `node`) and compiles and executes the Kotlin (`scripts/kotlinc.sh`) against the real runtimes' wire layers. Regenerate the goldens with `UPDATE_GOLDEN=1 cargo test -p undra-bindgen --test golden`. The messages that schema validation raises (E0001, E0005, E0006, E0007, E0010, E0011, E0031, E0050 to E0052 and E0073: what, why, fix and the docs link of the code) are locked in `tests/golden/diagnostics/<code>.txt` by `tests/diagnostics.rs` (`UPDATE_GOLDEN=1 cargo test -p undra-bindgen --test diagnostics`); the error-codes page of the site shows them. Swift cannot be built where this crate is developed; it is verified on a developer Mac and in CI (see CLAUDE.md), and here only by desk-check and a bracket-balance test.
