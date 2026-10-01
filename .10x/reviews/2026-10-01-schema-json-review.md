# Full-schema `undra_schema_json` and public Swift standard types (C1, C2) — adversarial review

**Date:** 2026-10-01 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/schema-json` at `dcc74c0` (11 commits on `549b1f2`) · **Read:** `CLAUDE.md` (R1, R2, R3, R7, R11), the SDE record
`.10x/decisions/sde/schema-json.md`, ADR-024 and its amendment, ADR-025, ADR-026, `docs/SPEC.md` 2.3, 6, 10.5 and 13,
the whole diff (`undra-meta` canonical/exchange forms, the three `undra-ffi` exports and their tests, the CLI loader,
`bindgen` command and `schema_docs`, `undra-bindgen` model/stdlib/swift and the goldens, `typecheck_swift.rs`,
`StandardRecords.swift`, `Foundation+Undra.swift`, `PublicStandardTypesTests`, the playground iOS network files, CI) ·
**Fixes:** `861e8b1`.

## Verdict

Both halves do what they say. C1: every path that computes the schema hash goes through the canonical form, so a
doc-only change moves the exported JSON and not the hash (probe below: library, CLI loader, dev runner and the runner's
JSON all report `0x4937957423dcf62b` before and after a reworded `///`, while the export grows from 14,993 to 15,043
bytes), the playground stays at `0xabdf844b53e0bc10` on every route including a JavaScript recomputation of its
export, and the size claims reproduce (+882 raw / +261 gzip on the playground wasm against the record's +861 / +268).
C2: the eight standard types are public with the conformances generated code needs, and both guards really guard: making
a type, an initializer or a member internal stops `PublicStandardTypesTests` compiling, and dropping one `Codable` stops
`typecheck_swift` compiling the `stdlib` golden. Two Medium findings, both fixed: the change to what a C ABI export
returns had no ADR (now ADR-050), and `undra bindgen --docs` against a core built before the change silently wrote
docless bindings (now a teaching `C0006`). Nothing High or Medium is open. One pre-existing finding goes to the
integrator: the hello-world web core is 135.2 KB gzipped against a 120 KB budget, it was already 126.4 KB on the day
v1 merged the CLI, and the README, site and blog still claim 85 KB (cause analysis below).

## Findings

| # | Sev | Where (at `dcc74c0`) | Finding | Status |
|---|---|---|---|---|
| M1 | Medium | `crates/undra-ffi/src/api.rs:89-95`, `tests/abi.rs:597-610`, `tests/wasm/raw.test.mjs:110-112`; SDE record "why there is no new ADR" | R11: what a C ABI export returns changed (bytes, size, and the `fnv1a64(export) == undra_schema_hash` relation two tests pinned) with no ADR. Nobody relied on the relation (attack 1), so the change is right, but it is a boundary decision and needs its record. ADR-025 is about type identity and port outcomes, not this export, so an amendment there would hide it. | **Fixed**: `.10x/adrs/ADR-050-schema-json-is-the-whole-schema.md` (Accepted; compatibility both ways, alternatives); SPEC 2.3 and 13 cite it; SDE record annotated. |
| M2 | Medium | `crates/undra-cli/src/schema.rs:116-133`, `src/commands/bindgen.rs:149-158` | New CLI, core built against an `undra-ffi` from before C1 (canonical export), `undra bindgen --docs`: the loader accepted the docless canonical form and `--docs` wrote bindings **byte-identical to the default, docless output, exit 0**. Reproduced: the `dcc74c0` CLI on the playground at `549b1f2`. Before C1 this case worked (the runner printed the docs), so it is a regression for version skew, and silent. | **Fixed**: the loader tells the forms apart (`Schema::to_json` always writes `undra_version`, the canonical form never does); `load_from_library(.., docs)` keeps or drops docs and, for `--docs` on a canonical export, returns `C0006` ("exports its schema without doc comments, so `--docs` has nothing to write" / why: older `undra-ffi`, ADR-050 / fix: `cargo update -p undra` or drop `--docs`). Without `--docs` an old core loads as before. Unit test `docs_are_kept_only_when_asked_for_and_refused_when_the_library_has_none`; the repro now prints the error. |
| L1 | Low | `crates/undra-cli/src/schema.rs:107` | The export was decoded with `String::from_utf8_lossy`. With the canonical export almost any damaged byte also failed the parse or the hash check; docs are outside the hash, so a damaged byte in a doc would pass and land in generated code as U+FFFD. | **Fixed**: strict `str::from_utf8`, copied before `undra_buf_free` (ownership unchanged, ADR-026), `C0006` with what/why/fix. |
| L2 | Low | `crates/undra-cli/src/templates.rs:29` | `CORE_SCHEMA` was documented as "the canonical schema of the template core, as `undra_schema_json` reports it"; the export is no longer canonical. | **Fixed** (doc). |
| L3 | Low | `runtimes/swift/.../PublicStandardTypesTests.swift:121-129` | The comment says the test shows a *module's* `Header` shadowing the runtime's; it declares a function-local struct. Module-level shadowing is Swift's ordinary rule and nothing compiles a second module with a clashing name. | **Fixed** (the comment says what the test shows). |
| L4 | Low | ADR-024 amendment, SDE record | `HttpWire.swift` was 87 lines, not 90 / 89. | **Fixed**. |
| I1 | Info | `crates/undra-ffi/tests/wasm/raw.test.mjs:82-101`, SPEC 2.3 | The JavaScript canonical recipe SPEC points non-Rust hosts at goes through `JSON.parse`: a `stale_ms` above 2^53 would lose precision, and names outside the BMP would sort by UTF-16 rather than by Rust's byte order. Neither occurs (milliseconds; Rust identifiers). The recipe reproduces the playground's hash from its export (`0xabdf844b53e0bc10`, checked with node), a richer schema than the ffi fixture the test uses. | Not changed. |
| I2 | Info | ADR-050 "Compatibility" | Old CLI, new core: the old loader keeps the docs it now finds, so its bindings gain doc comments without `--docs` and `bindgen --check` calls them stale (reproduced with the `549b1f2` CLI on the `dcc74c0` playground). Covered by the documented "matching versions" rule; recorded in the ADR. | Recorded. |
| I3 | Info | `src/commands/bindgen.rs:38` (unchanged) | `--schema FILE` keeps whatever docs the file has whether or not `--docs` is given (pre-existing; `schema_docs` relies on it). The library route now drops them without `--docs`, so the two routes differ on that flag. | Left for a later piece. |
| I4 | Info | `examples/playground/ios/PlaygroundApp/Network/PlaygroundNetwork.swift` | The 404 and 400 replies lost the `Content-Type: application/json` header `WireHttpResponse` defaulted to; their bodies are empty and the core does not read it. | Not changed. |
| X1 | High, pre-existing (integrator) | `README.md:50`, `site/docs/getting-started.html:127`, `site/llms-full.txt`, blog posts; `undra build`'s budget line | The hello-world web core is 135,236 bytes gzipped at `549b1f2` (135,490 with C1) against the 120 KB budget, and was already 126,394 when the CLI merged into `main` on 2026-09-30. The public "85 KB" figure is the CLI *branch* before that merge (84,657 reproduced at `379ac9b`) and was never re-measured. The budget is printed, not gated (R9). | **Not fixed here** (cause analysis below; for the integrator). |

## Attacks

**1. R11 / is `fnv1a64(export) == hash` a contract anyone relied on?** No. Grepped the three runtimes, the CLI, SPEC,
the docs and every test. Swift, Kotlin and TypeScript load cores by `undra_schema_hash` only; none reads the JSON.
The CLI loader (`load_from_library`) parses the JSON and hashes the parsed schema, so it never hashed bytes. TS contract S16
reads names and ids by key; the JNI E2E test and the C harness check presence only. SPEC defined the hash over the
canonical form and the export as "an owned copy"; it never stated the equality. Only `abi.rs` and `raw.test.mjs` pinned
it. Verdict: not relied on, but a changed export is a boundary change, so ADR-050 (M1).

**2. Hash stability.** Every path is `Schema::hash()` = `fnv1a64(canonical_json())`: the running core
(`undra-runtime/src/runtime.rs:437`), the export before `undra_init` (`undra-ffi/src/api.rs:81`), the dev runner's banner
(`templates/runner/main.rs:147`), the CLI loader (`schema.rs:123`) and bindgen's `schemaHash`. There is no macro-time
hash: registrations are static data, hashed when collected. `0xabdf844b53e0bc10` is pinned in
`crates/undra-cli/tests/schema_docs.rs:23` (library via the `bindgen` report, the `undra dev` banner, the `--schema`
route) and in the committed bindings (`Ids.swift:6`, `Ids.kt:10`, `ids.ts:8`), which `bindgen --docs --check` compares and
S16 checks against the running core; `abi.rs` and `raw.test.mjs` use the ffi fixture core and assert consistency rather
than a constant. Probe: `undra init probe` against this tree, `undra build --platform host`, read the library with
ctypes, reword one `///`, rebuild: library hash, `bindgen --docs` report, `bindgen` report, `undra dev` banner and
`bindgen --schema <runner --print-schema>` are `0x4937957423dcf62b` in both rounds; the export is 14,993 then 15,043
bytes; `--docs` bindings change, default bindings are identical. Verdict: holds.

**3. Loader (R2).** The `unsafe` is unchanged in shape: four `get`s, three calls, one `from_raw_parts`, one free, each
with its `SAFETY` comment. The buffer is copied and then freed exactly once with `undra_buf_free` (ADR-026); `len` is
`u32`, widened; a null pointer gives empty text and a parse error. Lossy UTF-8 (L1) and the old-core case (M2) were the
two problems; both fixed. Verdict: sound after fixes.

**4. Size claims.** Reproduced with fresh target directories (`gzip -9`, `undra build --platform web --release`,
wasm-opt 133): playground 570,117 / 218,778 gz at `549b1f2` and 570,999 / 219,039 at `dcc74c0`, +882 / +261 (record:
+861 / +268); template 352,891 / 135,236 and 353,746 / 135,490, +855 / +254 (record: +840 / +319). Absolute values sit
1.7 to 2.4 KB above the record's because my scratch path is longer (panic locations carry absolute paths). By crate
(raw code with names kept), C1 is `serde_json` +6.0 KB and `undra-meta` -5.0 KB: the borrowed twin does share the
serializer. A measurement trap worth knowing: reusing one `CARGO_TARGET_DIR` for the playground and then the template
of the same tree produced a 367,202-byte template (+14 KB, wrong); a fresh target agrees with the record. Verdict:
honest.

**5. Swift public API (R3).** Read as a Swift engineer: Rust names unprefixed as in Kotlin and TypeScript, memberwise
initializers with the defaults an engineer would write, `Sendable` / `Hashable` / `Codable` where a generated type has
them, errors as `UndraError` with the `#[error]` text as `description` and `LocalizedError` in the one Foundation file,
`NetKind.disconnected` instead of an `Optional`-shadowing `.none` (generated code never names that case: placeholders
take the first variant), `UndraAppState` kept. Nothing is public that need not be (the codec functions are protocol
witnesses). Tried it: `HttpRequest` internal, then `HttpMethod.name` and `HttpResponse.init` internal: each stops
`PublicStandardTypesTests` compiling (the other test files' `@testable` imports do not leak); `NetKind` without
`Codable` stops `typecheck_swift` (`Connectivity` in the `stdlib` golden). Verdict: passes.

**6. Bindgen.** `typecheck_swift` passes (nine modules in Swift 6 mode against the real runtime, 7 s warm). The
`Codable`-again case is covered three ways: `stdlib.rs` asserts `Endpoint` (request, response, methods, headers) derives
it, the golden locks `Connectivity: ..., Codable`, and `typecheck_swift` compiles both. Kotlin and TypeScript are
untouched: the source diff is `model.rs` (the removed branch ran only when `runtime_spelling` returned `None`, which only
Swift did), `stdlib.rs` and `swift.rs`, and only `golden/stdlib/swift` moved. Verdict: passes.

**7. CI.** Linux `rust` job: the existing ADR-029 step now runs `--test schema_retention --test schema_docs -- --ignored`
(named for both). macOS job: a named `typecheck_swift` step with `UNDRA_REQUIRE_TOOLCHAINS=1` right after `swift test`,
and the same ignored pair. Cost, not in the SDE record: `typecheck_swift` always builds the runtime and nine modules from
a fresh scratch package, 7 s here on 18 cores, so roughly half a minute to a minute on a `macos-15` runner;
`schema_docs` adds the playground's dev-runner build on a cold shared target, 14 s here, about a minute on CI, paid in
both jobs. Locally `typecheck_swift` runs inside every `cargo test --workspace` on macOS, like `typecheck_kotlin`
(`UNDRA_SKIP_SWIFT=1` skips it). Verdict: wired correctly.

## X1 for the integrator: why the hello-world wasm is 135 KB

`undra init hello --platforms web`, `undra build --platform web --release`, each commit in its own tree and target
directory, `gzip -9` of `build/web/*_core.wasm`:

| Point | Commit | Bytes | gzip | Step |
|---|---|---|---|---|
| CLI branch tip (source of the "85 KB") | `379ac9b` | 231,449 | 84,657 | |
| CLI merged into `main` | `5a595ce` | 332,633 | 126,394 | **+41,737** |
| standard ports in every template schema | `92c8569` | 333,229 | 126,611 | +217 |
| runtime review fixes (ADR-022, ADR-023) | `4820127` | 344,217 | 131,412 | **+4,801** |
| macros review fixes (ADR-025) | `8368182` | 343,258 | 131,025 | -387 |
| ffi fixes (ADR-026), bench, playground | `bc72e1c` | 345,350 | 131,761 | +736 |
| query rollback | `68fcb61` | 345,044 | 131,586 | -175 |
| keyed-list ops (ADR-027), platform polish | `03e02b7` | 349,096 | 133,272 | +1,686 |
| CLI polish | `a2f02df` | 349,279 | 133,375 | +103 |
| allocation-free sync dispatch (ADR-028) | `34207ae` | 353,165 | 135,434 | +2,059 |
| ADR-029, rename (ADR-030) | `31a1f37` | 353,440 | 135,592 | +158 |
| Swift error channel (ADR-032), magic (ADR-033) | `52188db` | 353,221 | 135,448 | -144 |
| frame-coalesced delivery (ADR-031) | `b7ec71e` | 353,005 | 135,205 | -243 |
| v1.x base | `549b1f2` | 352,891 | 135,236 | +31 |
| this branch (C1) | `dcc74c0` | 353,746 | 135,490 | +254 |

* The budget was broken the day v1 merged the CLI. The "85 KB" was measured on the CLI branch, which predated three
  things `main` already had: `keel-query` (ADR-018 dispatch layer, Kv-hydrated cache, offline queue), the facade's
  re-export of `keel-ports` (all ten standard ports' proxies and registrations in every core) and the signals review
  fixes. Raw code by crate across that merge: query +38.4 KB, ports +21.5 KB, data segments +16.0 KB, `core` +15.8 KB
  (formatting and generic glue), runtime +11.6 KB, `hashbrown` +5.3 KB, wire +4.0 KB, signals +3.8 KB.
* Since then +8.8 KB gzipped, mostly ADR-022/023 (+4.8: runtime +5.3 KB raw code, init +2.6, `hashbrown` +2.0, query
  +1.8), ADR-028 (+2.1) and ADR-027 (+1.7). ADR-031 and ADR-032 are not the cause (-0.4 together); C1 is +0.25; the doc
  strings were always there (2.2 KB gzipped if all were stripped, SDE record).
* The largest single item in the template today is `undra-meta`: 81 KB of the module's 408 KB (code and data, before
  wasm-opt), for collecting the registrations and serializing the schema through serde for the hash and the export. Those serve the load-time hash
  check and `undra bindgen`; a build-time hash constant and a host-only export are the obvious lever, worth more than any
  doc stripping. That is a design change (ADR), not a fix.
* For the integrator: correct the public figure (README, getting-started, `llms-full.txt`, the blog posts, the claims
  ledger), and either make the web budget a gated test (R9) at an honest number or open the size piece.

## Suites after the fixes (`861e8b1`)

`cargo fmt --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test --workspace`
2,249 passed, 0 failed, 11 ignored (2,248 before; one new unit test); `cargo test -p undra-cli --test schema_retention
--test schema_docs -- --ignored` 2/2; `swift test` 433/433; `crates/undra-ffi/tests/swift/run.sh` ok (1/1),
`wasm/run.sh` 29/29, `c/run.sh` ok (smoke, lifetime); `contract-tests/run-all.sh` 54/54 (S01-S18 on TS, Kotlin and
Swift); the iOS playground app builds for the simulator with `xcodebuild` against the public runtime types.
