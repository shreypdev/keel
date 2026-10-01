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

A `Result<T, E>` becomes a thrown `E` in every language (Swift `throws`, Kotlin an exception, TypeScript a rejection). Swift (ADR-032) adds a rule the other two get from their exception model: a generated call fails with exactly one of three things, its own `E`, `CancellationError` (the calling task was cancelled) or `UndraCallError` (a core panic, a cancellation by the core, a refused call, an unreachable core, a reply that does not decode). Calls use plain `throws`; the generated code is one `do`/`catch` per call that hands the error to `UndraCallError.mapped(_:domain:)`, so the mapping lives once, in the runtime.

A synchronous Swift method that returns nothing and has no error type is a **command** (`todos.toggle(id:)`). It cannot throw, because it is called from `Button` actions and binding setters, so it logs a failure, passes it to `LoadOptions.onError` and returns. Nothing generated traps.

Typed throws remain on **port requirements** only (`Generator::swift_typed_throws`): the host implements them, and `throws(HttpError)` tells the implementer exactly which errors the core can understand. With the option off they emit plain `throws`.

### The standard library

Every core links `undra-ports`, so its schema contains the ten standard ports (`Clock`, `Rng`, `Log`, `Http`, `Kv`, `SecureStore`, `Fs`, `Timer`, `Connectivity`, `Lifecycle`) and the eight types they exchange (`HttpMethod`, `Header`, `HttpRequest`, `HttpResponse`, `HttpError`, `FsError`, `NetKind`, `AppState`), and the schema hash covers them. The platform runtimes already implement the ports and ship the types, so the generators **leave them out** of an app's bindings and let references resolve to the runtime's own (ADR-024, SPEC 10.5):

| | The standard ports | A reference to a standard type |
|---|---|---|
| TypeScript | not generated | `import { type HttpRequest, HttpRequestCodec } from "@undra/runtime"` |
| Kotlin | not generated | `import dev.undra.runtime.adapters.HttpRequest` (its companion is the codec) |
| Swift | not generated | `AppState` is `UndraRuntime.UndraAppState`; the runtime keeps the other seven internal, so the ones something refers to are declared in the app's module |

Only an **exact** match is left out: same name, same id and same shape (`undra_bindgen::stdlib` holds the table, ids pinned as hex and checked against `undra-ports`). Ids are derived from names, so an app type that only shares a name, `struct HttpRequest { url: String }` for example, has another shape and is generated as the app's own; one with a standard name and another id (a hand-written schema) is E0052. A standard type that refers to another (`HttpRequest` to `Header`) is left out only while that one is. `validate` accepts the standard entries, and an app may have a record named like a standard *port* (`Timer`, `Log`): the ports are not declared, so they claim no names.

`Generator::emit_standard_library` declares everything as ordinary items; `undra-ports` uses it to prove its own schema generates in all three languages.

## The runtime API the output depends on

SPEC section 17 lists the base classes; these are the names generated code actually calls, including the few the specification does not spell out (marked *addition*). The tests compile and run the output against the real wire layers plus hand-written stand-ins of exactly this surface (`tests/fixtures/`).

**Wire layer (exists today).** TypeScript `codecs`, `Codec<T>`, `UndraWriter`, `UndraReader`, `encodeValue`, `decodeValue`, `decodePatch`, `applyPatch`, `PatchError`, `WireError`, `ChangeOp`, `CallTarget`, `ReplyStatus`, `ALL_SIGNALS`; Kotlin `Codecs`, `UndraCodec`, `UndraWriter`, `UndraReader`, `KeyedPatch`, `WireException`, `Payloads.{CallTarget,ChangeOp,ReplyStatus}`, `Handle`, `Timestamp`, `decodeAll`, `encodeToByteArray`; Swift `UndraCodec`, `UndraWriter`, `UndraReader`, `UndraBytes`, `UndraHandle`, `WireError`, `PatchOp`, `decodePatch`, `applyPatch`, `PatchError`, `CallTarget`, `ChangeOp`, `ReplyStatus`, `Observe.allSignals`.

**Standard types (SPEC 8).** TypeScript `HttpMethod`, `Header`, `HttpRequest`, `HttpResponse`, `NetKind`, `AppState`, `HttpError`, `FsError` and their `<Name>Codec` (`adapters/types.ts`, `adapters/codecs.ts`, exported from `@undra/runtime`); Kotlin the same eight in `dev.undra.runtime.adapters`, each with a companion `UndraCodec`; Swift `UndraAppState`. Generated code decodes a standard error at the call site (`decodeValue(HttpErrorCodec, ..)`, `HttpError.decodeAll(..)`) because these types have no `fromReply`.

**Base classes (SPEC 17).** `UndraCore` (`callSync`, `call`, `stream`, `construct`, `observe`, `mirror`, `registerPort`, `shared`), `UndraObject`, `UndraStore` (Swift `apply`, Kotlin `apply` and `signal(initial)`, TypeScript `_apply` and `_signals`), `Signal<T>`, `UndraError`, `UndraReplyError` / `UndraReplyException`, `UndraPort`, `UndraRecord`, `UndraEnum`, `UndraException`, `PortImpl`.

**Additions.**

* Call targets carry the object: TypeScript passes `{ target: CallTarget.ObjectMethod, handle }` (or `{ target: CallTarget.FreeFunction }`) because the wire enum has no room for a handle; Swift and Kotlin pass the existing `CallTarget` values and repeat the method id as the separate argument SPEC 17 lists.
* `UndraCore.event(port, method, payload)`: host-to-core events of event ports (`undra_event`).
* Typed port failures: a generated port adapter throws `UndraPortError(body:)` / `UndraPortException(body)` / `UndraPortError(body)` carrying the encoded `E`; the runtime replies with port status 1.
* `PortImpl`: TypeScript `{ sync, methods }` as in SPEC 17.1, Kotlin `PortImpl(sync, methods: Map<UInt, suspend (ByteArray) -> ByteArray>)`, Swift `PortImpl.sync([UInt32: ([UInt8]) throws -> [UInt8]])` and `PortImpl.async([UInt32: ([UInt8]) async throws -> [UInt8]])`.
* `UndraCore.observe` returns `Promise<void>` in TypeScript, resolving once the initial change-set is applied; in process, Swift and Kotlin apply it inline before `observe` returns (SPEC 5.5), so a store never exposes its placeholder values.
* `new Signal<T>(initial)`, `new UndraError(kind, message, options?)`, and `UndraStore` subclasses call `super(core, handle)`; the base class registers with the mirror and unregisters on `close()`.
* `Sendable` on every generated port protocol (Swift 6 strict concurrency).

## Query handles

Every `#[undra::query]` becomes a `<Name>QueryHandle` store with the five signals of SPEC section 9 (`data`, `status`, `error`, `fetching`, `updatedAt`; `error` is `String?` when the query has no error type), a `QueryStatus` unit enum, and two methods that `undra-query` must dispatch on handle objects:

| Method | Id |
|---|---|
| `refetch()` | `fnv1a32("query.refetch")` = `0x21d1b9e2` (`undra_bindgen::QUERY_REFETCH_ID`) |
| `invalidate()` | `fnv1a32("query.invalidate")` = `0x44cec2fa` (`undra_bindgen::QUERY_INVALIDATE_ID`) |

Constructing a handle is a constructor call whose `type_id` and `method_id` are both the query id. A `#[undra::mutation]` becomes an async function that calls the mutation id as a free function.

## Limitations

Rejected with a diagnostic instead of generating wrong code: `Lazy<T>` signals (no lazy-list runtime API exists yet), object handles used as values, `Option<Option<T>>` (not representable in Kotlin and TypeScript), a `Result` whose error type is not an error enum, `()` outside return position. Swift records and enums derive `Codable` only when every field is `Codable`, so a record holding a `Duration` (Codable only from the Swift 6.0 standard library) or a data enum does not. `Bytes` nested inside `Vec` or `Map` decode one byte at a time in Swift; direct and optional `Bytes` use the bulk path. `#[undra(js_number)]` does not exist in the schema yet, so `Generator::ts_js_number` switches `i64` and `u64` to `number` for the whole package.

## Tests

`cargo test -p undra-bindgen` runs the golden comparison of eight schemas (`tests/golden/<case>/schema.json` and the expected trees), the diagnostics tests, and, when the toolchains exist, type-checks and executes the TypeScript (`tsc` and `node`) and compiles and executes the Kotlin (`scripts/kotlinc.sh`) against the real runtimes' wire layers. Regenerate the goldens with `UPDATE_GOLDEN=1 cargo test -p undra-bindgen --test golden`. The messages that schema validation raises (E0001, E0005, E0006, E0010, E0011, E0031, E0050 to E0052: what, why, fix and the docs link of the code) are locked in `tests/golden/diagnostics/<code>.txt` by `tests/diagnostics.rs` (`UPDATE_GOLDEN=1 cargo test -p undra-bindgen --test diagnostics`); the error-codes page of the site shows them. Swift cannot be built where this crate is developed; it is verified on a developer Mac and in CI (see CLAUDE.md), and here only by desk-check and a bracket-balance test.
