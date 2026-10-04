# SDE - full-JSON `undra_schema_json` and public Swift standard types (wt/schema-json, 2026-10-01)

Track C1 and C2 of the v1.x design (`.10x/specs/2026-10-01-v1x-default-choice-design.md`): the two
queue items "`undra_schema_json` full-JSON variant so dlopen bindgen keeps docs" and "Swift runtime
`Port*` types public (drops the bindgen fallback, ADR-024)". One worktree, no push, no merge.

## C1. `undra_schema_json` carries the docs

### What it does now

* `undra-meta`: `Schema::to_json()` (compact, labels and docs, declaration order: the document
  `to_json_pretty` indents) and `Schema::without_docs()`. `to_json` serializes a borrowed twin of
  `Schema` (`Document`) that shares its slice serialization with the canonical form's `Canonical`,
  so a core links one copy of the serialization code, not two; a test pins the twin to the derived
  `Serialize`, so a field added to `Schema` and not to the twin fails.
* `undra-ffi`: `api::schema_json()` returns `Schema::to_json()`. The native, wasm and JNI exports
  all go through it. Nothing else of the ABI moved (version 1, the same symbols and signatures).
* `undra-cli`: `undra bindgen` always builds the host library, `dlopen`s it and reads the whole
  schema. `schema.rs` relabels it (`crate_name` is the core's package; the library only knows
  itself as `undra-core`). Without `--docs` the docs are dropped (`without_docs()`), so the
  default output is byte for byte what it was; with `--docs` they are kept. `--docs` no longer
  builds and runs the dev runner (one build less, and the doc flag cannot disagree with the
  library). The runner keeps `--print-schema`: it is the other route to the same JSON, kept for
  debugging and as the thing the equivalence test compares against. `--schema FILE` is unchanged
  (whatever the file carries is used).

### Docs in the binary: unconditional, measured

The question was whether docs live in the binary unconditionally or behind a cargo feature
(`undra/schema-docs`, off for release device builds, with the CLI falling back to the runner).
Docs **already are** in every binary: the macros emit `docs: "..."` as `&'static str` fields of the
`*Meta` statics behind every `inventory` registration, whether or not anything exports them. So
exporting them costs no data; the only new cost is the code that serializes the `Schema` document.

Sizes, release builds through `undra build --release` of `examples/playground` and of a fresh
`undra init hello` project (the "template" rows: the to-do core, which is what `undra build`'s
"hello world" budget lines are printed against); exact bytes, gzip is `gzip -9`:

| Artefact | Before C1 | After C1 | Delta | Before C1 with every doc stripped (macro experiment, not kept; delta against "Before") |
|---|---|---|---|---|
| Playground host `libundra_core.dylib` | 1,482,720 | 1,482,720 | 0 | 1,465,840 (-16,880) |
| Playground wasm (wasm-opt) | 567,728 (gz 218,526) | 568,589 (gz 218,794) | +861 (+268 gz, +0.12%) | 555,540 (gz 212,806; -12,188 / -5,720 gz) |
| Playground Android arm64-v8a `.so` | 1,569,752 | 1,572,176 | +2,424 (+0.15%) | 1,552,920 (-16,832) |
| Playground Android x86_64 `.so` | 1,695,088 | 1,697,352 | +2,264 | 1,678,256 (-16,832) |
| Template ("hello") host dylib | 1,051,472 | 1,051,488 | +16 | 1,051,312 (-160) |
| Template wasm (wasm-opt) | 351,149 (gz 135,156) | 351,989 (gz 135,475) | +840 (+319 gz, +0.24%) | 346,538 (gz 132,937; -4,611 / -2,219 gz) |
| Template Android arm64-v8a `.so` | 1,102,896 | 1,105,240 | +2,344 (+0.21%) | 1,096,384 (-6,512) |
| Template Android x86_64 `.so` | 1,193,512 | 1,195,728 | +2,216 | 1,186,984 (-6,528) |

(The last column came from temporarily making the macros' `docs()` return an empty string, which
also empties the docs of the standard ports, on the tree before C1; it was reverted and is not in
any commit.) Two things follow:

1. The marginal cost of C1 is 0.1 to 0.25% of each artefact, from the serialization code. A first
   version that serialized `Schema` through its derived `Serialize` cost +1.9 KB raw on the template
   wasm; the borrowed twin halved it.
2. The most a stripping feature could ever save is the docs themselves: 2.2 KB gzipped on the
   template wasm (1.6%), 5.7 KB on the playground's (2.6%), 6.5 to 17 KB raw on Android. That is
   real, but it is a different optimisation from C1 and not worth its price now: it needs a macro
   change (the `cfg` of a feature would be evaluated in the user's crate, not in `undra`, so the
   macros would have to route every `docs: ".."` through a macro owned by `undra-meta`), cargo
   features are additive (a "docs off" feature cannot win once any crate in the graph turns it
   on, so it would have to be a negative `strip-docs` feature the shim sets), and it would
   create two flavours of every library whose schema JSON differs, which needs the CLI to detect
   the flavour and fall back to the runner and say so. Against that, one always-complete export
   keeps R1 (the schema is the only truth: the library, the runner and a schema file describe the
   same thing) and has no fallback path to test.

Decision: unconditional. If device size pressure ever justifies the 2 to 6 KB, the mechanism is
the negative feature above, with a loader that says when a library carries no docs so `--docs` can
point at a build that has them.

The wasm budget in `undra build`'s output (120 KB gzipped, hello world) is over at the baseline
(template: 135,156 gz before C1), before this change; not touched here.

### The hash does not move

The hash is `fnv1a64(canonical_json())`; the canonical form drops docs and labels (SPEC 2.3), so
exporting them cannot change it. Pinned three ways:

* `crates/undra-cli/tests/schema_docs.rs` (`#[ignore]`, CI runs it with `--ignored` next to
  `schema_retention`, on Linux and macOS): builds the playground both ways and asserts the schema
  hash `0xabdf844b53e0bc10` from the library (`undra bindgen` report), from the dev runner (the
  `undra dev` banner) and from the `--schema` route.
* `crates/undra-ffi/tests/abi.rs`: `Schema::from_json(export).hash() == undra_schema_hash()`, and
  `without_docs()` of the export has the same hash.
* `crates/undra-ffi/tests/wasm/raw.test.mjs`: the export's canonical form recomputed in JavaScript
  (drop `docs` and labels, sort the unordered lists) hashes to `undra_schema_hash()`; this is the
  recipe a non-Rust host uses (SPEC 2.3 says so).

### The one observable change, and why there is no new ADR

`fnv1a64(bytes of undra_schema_json) == undra_schema_hash()` no longer holds: it held when the export
was the canonical form. Two tests asserted it (`abi.rs` and `raw.test.mjs`, both updated); the CLI
loader parses the JSON and hashes the parsed schema, so it is unchanged in effect. No platform
runtime reads the export for anything but names. R11 asks for an ADR before a boundary change; I
judged this one additive (no symbol, signature, version, wire or threading change, two more keys
and the docs in a document consumers read by key, and the v1.x design lists C1 without an ADR
while reserving 034 to 037 for track A). Integrator: if you read R11 more strictly, this is the
text to promote.

Review (2026-10-01): promoted. The meaning of a C ABI export changed and two tests had pinned the old
one, so R11 wants a record: `.10x/adrs/ADR-050-schema-json-is-the-whole-schema.md` (Accepted). The
review also made `--docs` on a core built before this change a `C0006` error instead of silently
docless bindings (`.10x/reviews/2026-10-01-schema-json-review.md`).

### Tests (all new unless noted)

* `undra-meta`: `to_json` is the whole document, equals the derived `Serialize`, hashes like the
  canonical form; `without_docs` removes every doc and keeps the hash.
* `undra-ffi` ABI: the export carries the docs; it reads back to the schema the dev runner prints
  (`collect_schema(..).to_json_pretty()`); the JSON/hash relation above (changed).
* wasm raw test: the JavaScript canonical form (changed).
* `undra-cli` unit tests: a library schema keeps its docs and takes the caller's label; a library
  that predates the docs still loads.
* `schema_retention` (CI): the loaded playground core's schema has docs.
* `schema_docs` (CI): `undra bindgen --docs` (library) and `--schema` of the runner's
  `--print-schema` generate the same files byte for byte; both equal the committed
  `examples/playground/generated`; without `--docs` the same schema generates the same files
  minus the documentation at the same hash.

Verified by hand as well, before the test existed: `undra bindgen --docs` output from the library
and from the runner's JSON are `diff -r` identical; the default (no `--docs`) output equals what the
old canonical route produced.

### Docs touched

SPEC 2.3 (the exchange form and the canonical recipe), 6 (the C block), 13 (schema extraction);
`undra.h`, `UndraNative.kt`, the ffi README and the three exports' doc comments; the CLI's
`bindgen --help` and the crate docs; the site's CLI page (it said `--docs` runs the core once more)
and the generated `llms-full.txt`, regenerated with `site/scripts/build-all.mjs`.

## C2. The Swift standard types are public

### What became public (`runtimes/swift/UndraRuntime/Sources/UndraRuntime/Core/StandardRecords.swift`)

`HttpMethod` (was `PortHttpMethod`), `Header` (`PortHeader`), `HttpRequest` (`PortHttpRequest`),
`HttpResponse` (`PortHttpResponse`), `HttpError` (`PortHttpError`), `FsError` (`PortFsError`),
`NetKind` (`PortNetKind`), and `UndraAppState` (already public, moved into the file). They are
`Sendable`, `Hashable` and `Codable` (the structs and unit enums), `UndraRecord` / `UndraEnum` /
`UndraError` (so `UndraCallError.mapped(_:domain:)` takes the errors), `CaseIterable` (the unit
enums), with public initializers (`HttpRequest` defaults headers, body and timeout, like Kotlin's
data class), `HttpMethod.name`, the Rust doc comments, and the Rust `#[error]` messages as
`description` / `errorDescription` (`LocalizedError` is in `Wire/Foundation+Undra.swift`, so the core
files stay Foundation-free). The wire layouts and ids are the ones `StandardPortTests` asserted
before, renamed.

Choices (recorded in the ADR-024 amendment too): the Rust names, unprefixed, as Kotlin and
TypeScript; `AppState` stays `UndraAppState` (public since v1 and the commonest app type name);
`NetKind.None` is `.disconnected` (no `Optional.none` ambiguity; generated code never names that
variant). The one cost, a module that imports both an app module declaring its own `HttpRequest` and
the runtime, must qualify, is rare, loud and local.

### The fallback is gone

`runtime_spelling` is total over the stdlib table (no `Option`); `Model::new` declares nothing from
the standard library (`Pending`, the reachability walk and helpers: 130 lines net removed);
`Types::codable` no longer special-cases runtime types, so a record that holds a standard type
derives `Codable` again (the `stdlib` golden's `Connectivity`, which holds a `UndraAppState`,
gains it). `emit_standard_library` (the `undra-ports` self-proof) is unchanged.

### Goldens and generated output

* `UPDATE_GOLDEN=1 cargo test -p undra-bindgen --test golden`: only `stdlib/swift` moved
  (`Types.swift` -159 lines, `Errors.swift` -117, `Connectivity` gains `Codable`); the other eight
  cases are untouched.
* `UPDATE_GOLDEN=1 cargo test -p undra-cli --test bindgen_schema`: the CLI golden (`stores`) did not
  change (it refers to no standard type).
* `undra bindgen -C examples/playground --docs`: one file changed, `Errors.swift` loses the 63 lines
  of its `HttpError` declaration; the other 26 files are unchanged.

### What else changed because of it

* `contract-tests/swift` (`FakeServer.swift`, `Support.swift`) and the runtime's own tests and
  adapters use the new names.
* The playground iOS app's `HttpWire.swift` (87 lines: hand-written copies of `HttpRequest`,
  `HttpResponse` and `Header`, there only because they were internal) is deleted;
  `ConnectivityWire.swift` keeps the two ids and the `Connectivity.changed` payload (now written
  with the public `NetKind`), and `PlaygroundNetwork` decodes `HttpRequest.undraDecoded(from:)` and
  answers `HttpResponse(...).undraEncoded()`. R10: the playground uses only the public API.
* New `crates/undra-bindgen/tests/typecheck_swift.rs`: generates every golden case and compiles the
  nine Swift modules together against the real runtime in Swift 6 mode (macOS only; skips
  elsewhere; `UNDRA_REQUIRE_TOOLCHAINS=1` makes a missing `swift` on macOS a failure; CI's macOS job
  runs it, a step added to `ci.yml`). I found no existing test that compiled the Swift goldens (the
  playground's own bindings are compiled by the Swift contract runner and the app); all nine
  compile (one existing warning, a needlessly escaped `default` in the `errors` golden, is not an
  error).
* `PublicStandardTypesTests` (8 tests, a plain `import UndraRuntime`) proves the types are public
  and `Sendable`, builds and reads each from outside the module, checks the messages, the
  `UndraError` use as a call's domain, `Codable`, and that a module's own `Header` shadows the
  runtime's.
* ADR-024 got a dated "Amendment" section at the end (the body is immutable); SPEC 10.5, the Swift
  runtime README, the bindgen README and `docs/ONBOARDING.md` (Swift test count) follow.

## Verification

See the report that came with the branch for the final counts; the commands are the ones in
`docs/ONBOARDING.md`: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace`, `cargo test -p undra-cli --test schema_retention --test schema_docs --
--ignored`, `swift test` (433), `bash crates/undra-ffi/tests/swift/run.sh`,
`bash crates/undra-ffi/tests/wasm/run.sh`, `bash contract-tests/run-all.sh` (18 scenarios on three
platforms, 54/54), and the iOS playground built with `xcodebuild` against the regenerated bindings.

## Not done / for the integrator

* The `--docs` flag stays opt-in. Making docs the default is a generated-shape change (every
  generated tree gains comments) and needs its own decision; the library has had them all along.
* The site's CLI page was edited because it described the old mechanism; `build-all.mjs` was run so
  the generated site files are in step. Nothing else under `site/` was touched.
* No ADR for C1 (above); superseded at review by ADR-050. ADR numbers 034 to 037 are left to track A.
* `.10x/status.md` and `.10x/handoff.md` are the integrator's: the two v1.x queue items ("full-JSON
  `undra_schema_json`" and "Swift runtime `Port*` types public") are done on this branch.
