# generated-weight (ADR-062, generated code is an artifact; the schema is the API) - adversarial review

**Date:** 2026-10-02 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/generated-weight` at `8996f13` (draft PR #3; contains `main` `fd7abb4`) · **Read:** `CLAUDE.md` (R1, R3, R7, R8, R11),
`docs/AGENT_WORKFLOW.md` 4, ADR-062 and its implementation note, the SDE record `.10x/decisions/sde/generated-weight.md`,
`docs/SPEC.md` 2.2, 2.3, 2.6, 5.7, 8, 8.1, 10 (10.1 to 10.5), 12, 13, 17, and the diff (`undra-cli` `schema_diff.rs`,
`schema_diff_tests.rs`, `commands/schema.rs`, `schema_file.rs`, `bindgen.rs`, `cli.rs`, the CLI tests and fixtures;
`undra-bindgen` `provenance.rs` and the three generators' header calls; the regenerated example trees; the script; the docs).
**Method:** every rule of SPEC 2.6 checked against what the three generators actually emit (the goldens and the generator
source: `default_value` in `swift.rs`/`kotlin.rs`, the TypeScript interfaces, `swift_callbacks.rs`, `model.rs`), a failing test
written for each wrong verdict before the fix, the stale-file case reproduced by hand in a scratch repository, `git check-attr`
on the committed trees, `npm pkg`/`npm pack --dry-run` on a generated `package.json`, every example's `undra bindgen --check`.
**Fixes:** `b77a26d`, `6a64f2b`, `085c215`, `21a2c03`, `a60b9c6`, `f526f8a`, `73e03a1`.

## Verdict

**Merge after fixes; the fixes are on the branch.** The artifact half is right: the header sentence is one function, the
TypeScript JSON files carry it without upsetting `tsc` (CI's 23 typecheck cases), npm (`npm pkg get`, `npm pack` leaves the
`.gitattributes` out through `files`) or anything in the repository; each tree's `.gitattributes` is
`linguist-generated=true` without `-diff`, and `git check-attr` marks every file of the six committed trees while leaving
`.undra-generated` and the bindgen goldens alone. Swift and Kotlin output is byte-identical to `main` (no `.swift`/`.kt`
golden moved), every example's `--check` passes, and the measurement reproduces (7,787 / 7,365 / 7,692).

What was wrong was the heart of the piece: **four verdicts said `additive` where an app breaks**, and **the comparison could
lie about a stale file**. A field added with `#[undra(default)]` was additive although the TypeScript generator writes a record
as an interface whose every member is required (ADR-062 even said so, and called the label "conservative"); Swift and Kotlin
spell no default for a field of a record or enum type; a callback's `background` flips Swift's protocol between
`@MainActor` and nonisolated; an opt-in port needs a web app to register its adapter. And `--against` compares two committed
files without building, so a `schema.json` nobody exported after an API change printed "No changes" and passed
`--exit-code`. Five false breaks (event ports, `ctx`, a signal's `computed`/`key`, a paged query's cursor) and a hash change
with no line were fixed too. Nothing blocking is open.

## Findings

| # | Sev | Where (at `8996f13`) | Finding | Status |
|---|---|---|---|---|
| H1 | High | `schema_diff.rs` `fields` (`None if f.default => additive`); SPEC 2.6 record-field row; ADR-062 2.3 | **A field added with a default is breaking.** TypeScript: `export interface Todo { … counts: Map<number, bigint>; }`, every member required, so every object literal typed `Todo` (a parameter, a preview, a test) stops compiling. Swift (`swift.rs` `default_value`) and Kotlin (`kotlin.rs` `default_value`) default only a primitive, string, bytes, time, UUID, decimal, optional or collection field, never a `Named` one: `priority: Priority` with `#[undra(default)]` is a required initializer argument in all three. A defaulted field inserted before an existing one shifts Kotlin's positional arguments and `componentN` (`Todo(id, "x", true)` silently means `pinned = true` when both are `Boolean`). | **Fixed** (`b77a26d`): always breaking, the text says which case applies; `lost its default` is additive for a `Named` field (nothing spelled it). Tests: `a_field_added_with_a_default_still_breaks_a_typescript_object_literal`, `a_defaulted_field_inserted_before_another_shifts_kotlin_positional_arguments`, `a_defaulted_field_of_a_record_or_enum_type_has_no_generated_default`, and `the_rule_for_a_defaulted_field_is_what_the_generators_emit` (generates Swift, Kotlin and TypeScript for 15 field kinds and checks `has_generated_default` against the initializers, so the rule cannot drift from the generators). |
| H2 | High | `schema_diff.rs` `port` (`background` → additive); SPEC 2.6 port/callback rows | **A callback's `background` changing is breaking.** `swift_callbacks.rs`: without it `@MainActor public protocol X: AnyObject, Sendable`, with it a nonisolated protocol (and `UndraWeakMainCallback` vs `UndraWeakCallback`); a Swift 6 implementation conforms differently, and an implementation that touches the UI is right on one thread only in every language. | **Fixed** (`6a64f2b`), both directions, test in `callbacks_are_added_freely_but_changing_one_breaks_its_implementers`. |
| H3 | High | `schema_diff.rs` `ports` (`is_standard_port_name(name)` → additive) | **An opt-in standard port added (WebSocket, Sse, Db) is breaking for the web**: Swift's `Adapters.platformDefault` and Kotlin's platform defaults register it, a web app must pass `webSocketPort(..)` in `LoadOptions.ports` (SPEC 17.1), or the core's calls fail as unavailable. | **Fixed** (`b77a26d`): breaking with that text; a port of SPEC 8 stays additive. Test `a_standard_port_is_additive_to_add_but_an_opt_in_one_needs_a_web_adapter`, over the stdlib golden schema (pinned to `undra-ports` by bindgen's tests). |
| H4 | High | `commands/schema.rs` `against` | **A stale `--against` lied.** Nothing is built, so a `schema.json` not re-exported after an API change (bindings regenerated, CI's `bindgen --check` green) made the report say "No changes" and `--exit-code` exit 0; a file stale at the ref made the report count changes from before the ref. Reproduced in a scratch repository. No `--check` existed for the file (the ADR left it a follow-up; the recipe was `export && git diff --exit-code`). | **Fixed** (`085c215`): `provenance::schema_hash_in` reads the hash back from a generated file's first line; `--against` compares the file with the hash of the project's bindings in the working tree and at the ref (`git show <ref>:./<generated>/…`, nothing built) and warns at either end with both hashes; a stale working-tree file fails `--exit-code`. `undra schema export --check` is the CI gate of the file (C0007, says whether the API moved or only the text: flags, a hand edit, another `undra`). Tests: `a_schema_file_the_bindings_disagree_with_is_called_stale_at_either_end` (both ends, the gate), the `--check` half of `export_writes_the_schema_of_the_built_core_and_diff_reads_it_back` (current, stale, other flags, missing), `the_hash_is_read_back_from_the_first_line_only`. |
| M1 | Medium | `schema_diff.rs` `ports` | **Standard was decided by name.** A port of the app's own called `Kv` (another shape) is generated and implemented by the app (SPEC 10.5: standard means name, id and shape), yet was "additive (a standard port)". | **Fixed** (`b77a26d`): `stdlib::covered(new).ports`, the generators' own test. In `a_port_the_app_must_implement_is_breaking_to_add`. |
| M2 | Medium | `schema_diff.rs` `ports`, `port` | **Event ports were treated as implemented by the app.** An event port is host-to-core: the bindings give the app `<Port>Events` to call, there is nothing to implement, so adding one (or a method of one) was a false break that fails a gate. | **Fixed** (`b77a26d`): additive; removal and signature changes stay breaking. Test `the_app_sends_an_event_port_so_adding_one_or_a_method_of_one_is_additive`; the CLI fixture gains an event port. |
| M3 | Medium | `schema_diff.rs` `callable_changes` | **`Ctx` was breaking**: it is not a wire parameter and no generator reads `takes_ctx` (`grep` finds it only in `model.rs` builders). | **Fixed** (`b77a26d`): additive, in `every_change_to_a_signature_is_breaking`. |
| M4 | Medium | `.github/workflows/ci.yml` | **`scripts/generated-weight.test.mjs` ran nowhere** (the record said so). | **Fixed** (`21a2c03`): a step beside `bench-device-report.test.mjs` in the Rust job. |
| M5 | Medium | ADR-062 2.2/2.3, the review page, `schema diff --help`, SPEC 13 | **Docs promised more than the code**: the rules above, "the labels are conservative" next to a non-conservative one, the CI recipe without a check of the file. | **Fixed** (`b77a26d`, `a60b9c6`, `f526f8a`, `73e03a1`): SPEC 2.6 and 13, ADR-062 (tables, example, and an "Amendment (adversarial review)"), the review page and CLI reference (`export --check`, the warning), `--help`, ONBOARDING; the site regenerated, `check-links --words` clean. |
| L1 | Low | `schema_diff.rs` `signal` | A signal's `computed` or `key` changing was breaking; every signal is a `private(set)`/`StateFlow`/`readonly Signal` property either way, the key only picks keyed patches. | **Fixed** (`b77a26d`): additive. |
| L2 | Low | `schema_diff.rs` `query` | A paged query's cursor type and an item key other than `id` were breaking; `fetchNextPage()` takes no cursor, only `id` makes Swift's row `Identifiable`. | **Fixed** (`f526f8a`): additive except a key to or from `id`. |
| L3 | Low | `schema_diff.rs` `case` | A case field's `default` changing moved the hash with no line, and the summary blamed "a wire id". | **Fixed** (`73e03a1`): an additive line (no generator spells a case default). |
| L4 | Low | `commands/schema.rs` `against` | With a relative `-C` the report named the default file by its absolute path. | **Fixed** (`085c215`). |
| L5 | Low | `crates/undra-cli/tests/golden/stores/*/.gitattributes` | The CLI's golden copy of a tree is a real `.gitattributes`, so GitHub collapses that golden in pull requests too. The bindgen goldens (the generator's review surface) are not collapsed. | Open, by choice: one click away, and the CLI golden is a copy of a bindgen case. |
| L6 | Low | the hash-less headers | "One unified header" holds for the sources and the two JSON files; the C entry, module map, R8 rules, `Package.swift` (line 2), `build.gradle.kts` and `.gitignore` keep four wordings of "Generated … do not edit" (ADR-062 1.1 says so). | Open; wording only. |
| L7 | Low | `schema_diff.rs` | A core crate renamed is not a line: `crate_name` is a label (SPEC 2.3), but the default Swift module, Kotlin package, npm package, `Undra<Namespace>` entry and default storage directory derive from it unless `undra.toml` overrides them. | Open; follow-up (it needs the project's naming config, which a schema file does not carry). |
| L8 | Low | n/a | An infinite query added makes its row `Identifiable` in Swift; an app's own `extension Row: Identifiable` then gets the cross-module "already stated" warning (not an error). | Open; a warning. |
| L9 | Low | `examples/` | The repository's own examples commit no `schema.json`, so neither `export --check` nor `diff --against` runs on them in CI (R10). | Open; follow-up, listed in ADR-062. |

## The compatibility cases checked

Breaking, as the generated code shows: a field added with or without a default (TypeScript literal; Swift/Kotlin for a
`Named` field or without a default; Kotlin positional when inserted), removed, retyped (a widened integer included), reordered;
a type renamed with the same shape (removed + added); an enum case added (exhaustive `switch`/`when`), removed, re-payloaded or
re-indexed (Swift's raw value); a parameter added/removed/renamed/retyped/reordered; return type, `async`, generic label;
`background`; a sync or async port or callback gaining a method; a non-standard sync or async port added (E0062), an opt-in
standard port added; a store gaining or losing its store; `infinite` added or removed; an item key to or from `id`.

Additive: any item added; a defaulted field's default gained, or lost on a `Named` field; an error message; a case field's
default; `Ctx`; a signal added, reordered, its `no_coalesce`, `default`, `computed` or `key`; an event port or a method of one;
a standard port of SPEC 8; a callback method's `coalesce` (the bridge only); a query's key, stale time, `persist`,
`idempotent`, interval, `poll_in_background`, a paged query's cursor or a non-`id` item key. Ids are not compared and every
other field of the canonical form now has a rule, so "the hashes differ and no rule found anything" means an id moved.

## What was run

`cargo fmt`, `cargo clippy -p undra-cli -p undra-bindgen --all-targets -D warnings`; `cargo test -p undra-meta -p undra-bindgen
-p undra-cli --no-fail-fast`: every target green, `symbols` (release Android and web builds) and `dev_reload` included, and
`typecheck_swift` (4) and `typecheck_kotlin` (1) compiled locally; `typecheck_ts` (23) and `run_ts` (14) with
`UNDRA_REQUIRE_TOOLCHAINS=1` and a temporary link to the primary checkout's `node_modules` (removed). `undra bindgen --check
--docs` on the playground, cookbook, fieldbook, two-cores `a` and `b`, `--check` on ios15-sample: up to date. `node --test
scripts/generated-weight.test.mjs` 5/5; `node scripts/generated-weight.mjs` reproduces the ADR's tables. `node
site/scripts/build-all.mjs` then `check-links.mjs` and `--words`: clean. The fixture's review size re-measured with `undra bindgen
--schema` on both sides and `diff -r` (581 lines; the old fixture still gives the record's 506).

## What I could not verify

* GitHub's rendering of the collapse: documented behaviour of `linguist-generated`; `git check-attr` shows the attribute on
  every file of the trees, and the API exposes no "generated" flag to test from here.
* The device builds of the generated code (iOS simulator, Android emulator) are the PR's CI; Swift and Kotlin output is
  byte-identical to `main`.
* The per-feature figure (130 to 250 lines per platform) beyond the totals, which reproduce.
