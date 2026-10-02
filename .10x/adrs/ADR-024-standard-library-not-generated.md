# ADR-024: the standard library is in every schema and in no app's generated bindings

Status: accepted (2026-09-30). Touches SPEC 8 and 10 (new 10.5) and 12 (E0052); `keel-bindgen`
(`stdlib`, `Model`, the three generators, `validate`). No wire, ABI or runtime-model change.

## Context

Every app core links `keel-ports` (the `keel` facade re-exports it, and `Ctx::http()` and friends need
it), so `collect_schema` returns the ten standard ports and the eight types they exchange
(`HttpMethod`, `Header`, `HttpRequest`, `HttpResponse`, `HttpError`, `FsError`, `NetKind`,
`AppState`) in every app's schema. That is correct: the schema is the only truth (R1) and its hash
covers the standard surface, so a core built against a different `keel-ports` is refused at load (R7).

The three platform runtimes already carry exactly this surface by hand, because the default adapters
must not depend on generated code: TypeScript `adapters/types.ts` and `adapters/codecs.ts`, Kotlin
`StandardRecords.kt` and `StandardPorts.kt`, Swift `Core/StandardPorts.swift`. Generating it again
into every app has three costs:

* two `FsError` types (and two of every other) in one app: a Kotlin file that imports the runtime's
  silently shadows the package's own, and a TypeScript file has to alias one of the two;
* the generated twin is a second implementation of a wire layout that the runtime already locks byte
  for byte, so the two can drift;
* output an engineer who has never seen Rust would not write (R3): the template core's bindings were
  2,836 lines, 1,894 of them the standard library.

## Decision

1. **The schema keeps the standard library. Generation filters it.** `keel_bindgen::stdlib` is the
   table: the ten port ids and eight type ids pinned as hex (each recomputed with FNV-1a by a test and
   cross-checked against the registrations of `keel-ports`), the method ids, and the shape of every
   item. `validate` still accepts the standard entries; only the generators leave them out.
2. **Only exact matches are skipped.** Type ids are derived from names (SPEC 1.1), so the id alone
   cannot tell a user's own `HttpRequest` from the standard one. An item is standard when name, id and
   shape agree (documentation is ignored; ports also compare kind, method ids and signatures), and a
   standard type counts only while every standard type it refers to does. A type that merely shares a
   name stays the app's own and keeps being generated. An item with a standard name and another id (a
   hand-written or foreign schema; the macros cannot produce one) is E0052, telling the author to
   rename it.
3. **References point at the runtime's own types.** A user type, method or port that mentions a
   standard type refers to it instead of declaring it:

   | Type | TypeScript | Kotlin | Swift |
   |---|---|---|---|
   | `HttpMethod`, `Header`, `HttpRequest`, `HttpResponse`, `HttpError`, `FsError`, `NetKind` | `@keel/runtime` (type and `<Name>Codec`) | `dev.keel.runtime.adapters.<Name>` | not public (`PortHttpRequest`, ...): declared in the app's bindings, but only when something refers to it, and what it refers to |
   | `AppState` | `@keel/runtime` | `dev.keel.runtime.adapters.AppState` | `KeelRuntime.KeelAppState`, so structs that hold it do not derive `Codable` |

   The Swift runtime keeps seven of the eight internal on purpose (its README: a public twin would make
   the names ambiguous against generated modules); generating those seven when referenced is the
   fallback, and it cannot clash because the runtime's are invisible to the app.
4. **No helper of a generated class is called on a runtime type.** The generated typed errors have a
   `fromReply`; the runtime's `HttpError` and `FsError` do not. At a call site whose `Result` error is a
   standard error the generators decode with the runtime's codec (`error instanceof KeelReplyError && ...
   ? decodeValue(HttpErrorCodec, error.body)`, `if (e.status == ReplyStatus.ERROR) HttpError.decodeAll(e.body)`).
   Placeholder values of store signals use positional arguments for runtime errors, whose payload
   names are the runtime's.
5. **The standard ports never appear and do not claim names.** An app may have a record called `Timer`
   or `Log`, which the schema-level name check rejected while the standard ports were generated.
6. `Generator::emit_standard_library` (default off) declares everything as ordinary items. `keel-ports`
   uses it to keep proving that its own schema generates in all three languages.

## Alternatives rejected

* **The facade drops `keel-ports`.** Any Rust-side use of a standard port links its registrations
  (`#[keel::port]` registers at link time), so the schema would still contain them, and `Ctx::http()`
  would become opt-in for no gain.
* **Bindgen always generates them** (the status quo). The duplicates above.
* **Skip by name or by id.** The ids are name-derived, so a user's differently shaped `HttpRequest`
  would silently be replaced by the runtime's and mis-decode on the wire.
* **Give the runtime types a `fromReply`, or make the Swift ones public and unprefixed.** Both change a
  runtime another team owns. The second would let Swift filter all eight; it is the natural follow-up
  (open question below) and needs no bindgen redesign: `runtime_spelling` would return the public name.

## Consequences

* Generated TypeScript and Kotlin now depend on the shape the runtimes give these eight types
  (field names, `<Name>Codec`, companion codecs). The `stdlib` golden case pins the output; the
  TypeScript case is type-checked and executed against the real runtime declarations and code, the
  Kotlin case is compiled with `-Werror` and run against the real runtime.
* Changing the standard surface already needs an ADR (R7, R11, the `keel-ports` golden). It now also
  means updating `keel_bindgen::stdlib`; `tests/stdlib.rs` fails if the table and `keel-ports` disagree.
* The template core's bindings shrink from 2,836 lines (909 Swift, 853 Kotlin, 1,074 TypeScript) to 942.
* The `full` golden case used a port that is exactly the standard `Clock`; it is renamed `WallClock` so
  the case keeps testing a sync port of the app's own.

## Open question

Should the Swift runtime export the standard types publicly (dropping the `Port` prefix and the
`KeelAppState` name)? Then Swift filters the same eight types as the others and generated Swift stops
declaring any of them. The runtime's README chose internal types so that a generated module declaring
its own `HttpRequest` would not be ambiguous with a public twin. Once generators no longer declare the
standard types, the only `HttpRequest` a module can declare is the app's own, differently shaped one,
and in Swift a declaration in the app's module wins over an imported name, so the ambiguity the README
worries about does not arise. It is the runtime team's call; `stdlib::runtime_spelling` is the single
place in bindgen that changes.

## Amendment (2026-10-01): the Swift runtime exports the standard types

Status: accepted. Answers the open question above. Touches SPEC 10.5, `undra_bindgen::stdlib`
and `Model`, and `runtimes/swift/UndraRuntime` (`Core/StandardRecords.swift`). No wire, ABI or
runtime-model change: the layouts and ids are the ones `StandardPortTests` already asserted.

The body above is unchanged and describes what was decided on 2026-09-30; where it says the Swift
runtime keeps seven of the eight types internal and that bindgen declares the ones something refers
to, this section supersedes it.

### What moved

The seven internal, prefixed types became public API of `UndraRuntime`, with the names Kotlin and
TypeScript already use:

| Before (internal) | Now (public) |
|---|---|
| `PortHttpMethod` | `HttpMethod` |
| `PortHeader` | `Header` |
| `PortHttpRequest` | `HttpRequest` |
| `PortHttpResponse` | `HttpResponse` |
| `PortHttpError` | `HttpError` |
| `PortFsError` | `FsError` |
| `PortNetKind` | `NetKind` |
| `UndraAppState` (public since v1) | `UndraAppState` (unchanged) |

They live in `Core/StandardRecords.swift` with the hand-written codecs they always had (the
`Wire/Foundation+Undra.swift` bridge holds their `LocalizedError` conformances, so the core stays
Foundation-free) and have the shape generated code for `undra-ports` had: `Sendable`, `Hashable` and
`Codable` structs and unit enums (`UndraRecord` / `UndraEnum`, `CaseIterable`), the two errors as
`UndraError`s (so `UndraCallError.mapped(_:domain:)` takes them) with the messages of the Rust
`#[error]` attributes, public initializers (`HttpRequest` defaults headers, body and timeout to
nothing, as the Kotlin data class does), and doc comments from the Rust docs.

### Decisions

1. **The Rust names, unprefixed.** The open question's argument held: once generators declare no
   standard type, the only `HttpRequest` a generated module can declare is the app's own,
   differently shaped one, and a declaration in a module wins over an imported name inside that
   module. The one place a public twin is ambiguous is code that imports both modules and writes
   the bare name; the compiler says so and the fix is to qualify (`PlaygroundCore.HttpRequest`).
   That is a rare, loud and local cost, against a permanent one: a second spelling of every
   standard type for every Swift engineer, and generated code that differs from the other two
   languages.
2. **`AppState` stays `UndraAppState`.** It has been public under that name since v1, and an app's
   own `AppState` model is the commonest type name in Swift: the ambiguity above would not be rare
   for this one. `stdlib::runtime_spelling` is the single place that says so.
3. **`NetKind.disconnected` is Rust's `NetKind::None`.** A case called `none` is ambiguous with
   `Optional.none` wherever the value is optional (`NetKind?`). Generated code never names that
   variant (placeholders use a type's first variant), so the spelling is the runtime's to choose;
   it keeps the name the internal type had.
4. **Bindgen declares nothing from the standard library, in any language.** `runtime_spelling` is
   total over the table (it took a name and returned an `Option` only because Swift said `None`);
   the "declare it when something refers to it" machinery in `Model::new` (`Pending`, the
   reachability walk and its helpers, 130 lines net) is deleted. A record that holds a standard type derives
   `Codable` again, because the runtime's records are `Codable`: before, the special case for
   runtime types made it `false`, which the fallback-declared copies had hidden for seven of the
   eight (the golden's `Connectivity`, with an `UndraAppState` field, gains `Codable`).

### Consequences

* Generated Swift for an app that mentions standard types shrinks: the `stdlib` golden by 275 lines,
  the playground's `Errors.swift` by the 63 lines of its `HttpError`.
* An app that answers `Http`, `Fs` or `Connectivity` itself no longer writes the wire types by hand:
  the playground's `HttpWire.swift` (an 87-line copy of `HttpRequest`, `HttpResponse` and `Header`,
  there only because they were internal) is gone, and `PlaygroundNetwork` decodes
  `HttpRequest.undraDecoded(from:)` and answers `HttpResponse(...).undraEncoded()`.
* The public API surface of the Swift runtime grows by seven types and their conformances, which
  are now compatibility commitments (the wire layouts already were).
* `tests/typecheck_swift.rs` compiles the generated Swift of every golden case (the `stdlib` case
  refers to all eight types and declares none) against the real runtime, and
  `PublicStandardTypesTests` is a plain `import`, not `@testable`, so either stops compiling if one
  of the types or what generated code needs of it stops being public.

## Amendment (2026-10-01, ADR-049): the standard surface gains `StorageError`

ADR-049 decision 1 changes the standard surface this ADR lists, once (the "one standard-surface
revision" of Amendment D of the v1.x plan): `Kv` and `SecureStore` methods return
`Result<_, StorageError>`, the new `#[undra::error] enum StorageError { Unavailable(String), Full,
Locked, Corrupt(String), Io(String) }` is the ninth standard type (type id `0x3d40_b010`), and
`FsError` gains `Full = 3` and `Unavailable(String) = 4`. `undra-bindgen`'s `stdlib` table, its
golden and the three runtimes' standard types follow; every core's schema hash moves (the standard
surface alone is now `0xbbf6_f70d_0c56_7f47`). What this ADR decides is unchanged: the standard
types are in every schema and in no app's generated bindings.

## Amendment (2026-10-01, ADR-046): the standard surface gains `Diagnostics`, three records and one function

ADR-046 changes the standard surface once more (the standard surface alone now hashes to
`0x543d_0961_0867_e387`): the sync port `Diagnostics` (`panicked(report: PanicReport)`, port id
`0xab68cd7c`), the records `PanicFrame`, `PanicReport` and `BackgroundReport` (type ids `0x19a497d1`,
`0xd08d5436`, `0x5dbea5f3`: the table is now eleven ports and twelve types) and **one standard
function**, `run_background(deadline_ms: u64) -> BackgroundReport` (function id `0x0e5b14ff`).
What this ADR decides holds, with two additions:

1. **A standard function is a standard item.** `undra_bindgen::stdlib` has a `FUNCTIONS` table next to
   `TYPES` and `PORTS` (name, id, declaration), `covered` reports the functions a schema declares
   exactly as the table does, `Model::new` leaves them out of every language's output, and an item
   named like one with another id is E0052. The platform runtimes call it themselves
   (`runInBackground`).
2. **The three report types are spelled `Undra...` in every language** (`UndraPanicReport`,
   `UndraPanicFrame`, `UndraBackgroundReport`), the name TypeScript has used for the report since
   ADR-049, rather than the bare names the other standard types have: they are values the runtime
   hands the app (`onPanic`, `runInBackground`), never names an app's own records or generated code
   mention, and the prefix keeps them clear of an app's own `PanicReport`
   (`stdlib::runtime_spelling` is still the one place that says so).

## Note (2026-10-01): the standard types at an iOS 15 floor (ADR-045)

No change to the decision or to any shape. The Swift runtime's floor drops to iOS 15 / macOS 12 (ADR-045), and the
standard surface needs nothing newer: none of the eight standard types or ten ports carries a `Duration` (the
request timeout is `u32` milliseconds), so `Core/StandardRecords.swift` and `Core/StandardPorts.swift` compile at the
floor unchanged, and the `stdlib` golden case is built for the iOS 15.0 and 16.0 simulators with the rest
(`typecheck_swift`). What the floor does change is the one generated spelling that this ADR left to the schema:
a wire `Duration` field of an app's own type is `Swift.Duration` from a floor of iOS 16 and the runtime's
`UndraDuration` below it; neither is a standard type, so the filtering rules above are untouched.
