# ADR-050: `undra_schema_json` returns the whole schema, docs included

Status: Accepted (2026-10-01, implemented on `wt/schema-json`, recorded at its review). Touches the
content of one C ABI export (SPEC 2.3, 6 and 13; `undra_schema_json`, its wasm twin and JNI
`UndraNative.schemaJson`) and the CLI's schema loader. No symbol, signature, ABI version, wire,
schema-hash or threading change. Constitution R11: the meaning of a boundary export changed, so it
is decided here; the implementer judged it additive and wrote no ADR, and the review promoted the
reasoning of `.10x/decisions/sde/schema-json.md` ("The one observable change") into this record.

## Context

`undra_schema_json` returned `Schema::canonical_json()`: the form the hash is computed over, with no
`undra_version`/`crate_name` labels and no doc comments. So `fnv1a64(bytes) == undra_schema_hash()`
held, and two tests asserted it (`crates/undra-ffi/tests/abi.rs`, `tests/wasm/raw.test.mjs`). SPEC
never promised that equality; it defined the hash over the canonical form (2.3) and the export as an
"owned copy" (6). The cost of the canonical export was that `undra bindgen --docs`, which reads the
schema from the built library (SPEC 13), had no docs to read and ran the dev runner as a second
build instead.

Who reads the export: `undra-cli` (parses the JSON, rehashes the parsed schema, compares with
`undra_schema_hash`); the TypeScript contract test S16 (names and ids, by key); the JNI smoke test
and the C harness (presence only). No platform runtime reads it; Swift, Kotlin and TypeScript load
cores by `undra_schema_hash` alone.

## Decision

`undra_schema_json` returns `Schema::to_json()`: the exchange form (SPEC 2.3), compact, labels and
doc comments included, every list in declaration order. The docs are already in every binary (they
are `&'static str` fields of the registrations), so the export adds serialization code only (0.1 to
0.25% of each artefact, measured in the SDE record), and stays unconditional: one export, no
"docs stripped" build flavour for the CLI to detect.

The schema hash is unchanged: still `fnv1a64(canonical_json())` on every path (the registrations
collected at load, the cdylib export, the dev runner, the CLI loader, generated bindings). A host
that wants to check `undra_schema_hash` against the export recomputes the canonical form
(`Schema::from_json(..).hash()` in Rust; elsewhere drop `docs` and the labels and sort the
unordered lists, as `raw.test.mjs` does). `fnv1a64(undra_schema_json bytes) == undra_schema_hash`
no longer holds and is not a contract.

The ABI version stays 1: no symbol or signature changed, and every reader reads the document by
key (`Schema::from_json` ignores unknown fields and defaults missing `docs`).

## Compatibility

* New CLI, core built before this change: the loader accepts the canonical form (it adds the
  labels) and generates the same docless bindings as before. `--docs` on such a core is an error
  (`C0006`, "exports its schema without doc comments, so `--docs` has nothing to write", fix:
  update the core's `undra`), never docless bindings written as if the source had no comments.
* Old CLI, core built after this change: the old loader parses the whole form too, and keeps what
  it finds, so its bindings gain doc comments without `--docs` (and `bindgen --check` reports them
  stale). Matching CLI and core versions is already the documented rule ("use the undra-cli that
  matches the `undra` version of the core").
* A host that hashed the export bytes directly would now see a mismatch. None exists in this
  repository; nothing is published.

## Alternatives rejected

* **A second export (`undra_schema_json_full`).** Two symbols for one document, an ABI addition
  every runtime header carries, and the canonical one has no reader that needs its bytes.
* **Docs behind a cargo feature.** Saves 2 to 6 KB gzipped on the web, but features are additive (a
  "docs off" switch has to be a negative feature the shim sets), the macros would have to route
  every doc string through a macro owned by `undra-meta`, and the CLI would have to detect the
  flavour and fall back to the runner. Revisit only under device size pressure.
