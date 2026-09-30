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
