# SDE: D1, macro diagnostic polish and the E-code audit (branch `wt/diagnostics`)

Track D1 of the v1.x design (`.10x/specs/2026-10-01-v1x-default-choice-design.md`). Constitution
R8: an engineer who makes a mistake in a `#[undra::..]` item is told what to change, every time.
Closes the diagnostic items the macros review left open (`.10x/reviews/2026-09-30-keel-macros-review.md`,
"Integrator resolution": d05 query-in-impl, the d03 split-impl follow-ons, NF1, NF2) and audits every
code of SPEC section 12. No ADR: no change to the wire, the runtime or threading model, the schema
or a generated Swift/Kotlin/TypeScript shape (the schema hash is untouched, the bindgen goldens
did not move). What the generated *Rust* gained (hidden record members, a guard `macro_rules!`,
private `const _` blocks) is listed below and in SPEC 16.3.

## The audit

`crates/undra-macros/tests/catalogue.rs` keeps the audit from rotting: SPEC 12 and the code table
of `diag.rs` list the same codes; every constant is in SPEC and every code a macro emits has a
constant; every code is emitted by code that is not a test; every code has a golden with its real
message; every message in a golden has what, note, help and the link of its code; every docs link
in the source is the link of a code; the command-line codes are SPEC's. `site/scripts/build-errors.mjs`
reads the same tables and goldens and fails on a code with no real message.

| Code | Raised by | Emitting site | Golden | What the audit found and changed |
|---|---|---|---|---|
| E0001 | macros, schema, `Encode`/`Decode` bounds | `types.rs`, `object.rs` (Ctx), `store.rs` (Lazy), `check.rs`, `undra-wire` `codec.rs`, `undra-meta`/`bindgen` `validate.rs` | 9 ui + `E0001.txt` | schema-level message had no why/fix; array, tuple and catch-all helps name the replacement; `&str` in a failed record no longer adds rustc's E0106 ("introduce a lifetime"), which contradicted the advice |
| E0002 | macros | `common.rs`, `object.rs` | `e0002_generics` | none needed |
| E0003 | macros | `common.rs`, `object.rs`, `types.rs` | `e0003_lifetimes`, `m1_store_bad_field_type` | none needed |
| E0004 | macros | `types.rs` | `e0004_trait_objects` | none needed |
| E0005 | macros, schema | `types.rs`, `validate.rs` | ui + `E0005.txt` | schema message gains why and fix |
| E0006 | macros, schema | `types.rs`, `validate.rs` | ui + `E0006.txt` | schema message gains why and fix |
| E0007 | macros | `mod.rs`, `object.rs`, `port.rs`, `query.rs`, `record.rs`, `store.rs` | 9 ui | wrong-item names what the item is and what the attribute goes on; `Self` in a function signature; an Undra attribute on a port method; the query-in-impl guard (below) |
| E0008 | macros | `attrs.rs`, `mod.rs`, `paths.rs`, `port.rs`, `store.rs` | 5 ui | unknown option or argument suggests the nearest name or lists the options; a misplaced option says where it belongs; NF1: a key naming no field |
| E0010 | macros, schema | `error.rs`, `record.rs`, `validate.rs` | ui + `E0010.txt` | schema message gains why and fix |
| E0011 | macros, schema | `object.rs`, `store.rs`, `validate.rs` | 3 ui + `d1_two_store_blocks` + `E0011.txt` | "no constructor" says a type takes one block and where the methods go; the probe messages are built by `Diag` (they were string literals with a hard-coded link) |
| E0012 | macros | `types.rs` | `e0012_trait_object_field` | none needed |
| E0013 | macros | `store.rs` | `e0013_store_restore`, `m5_store_state_without_default` | the error is on the first computed field, not on the struct name |
| E0020, E0021 | macros | `object.rs`, `port.rs` | `e0020_mut_self`, `e0021_self_by_value` | none needed |
| E0022 | rustc, named by a macro | `common.rs` (`send_assertion`) | `e0022_future_not_send` | **was a code with no emitting site**: a constant now exists, the assertion rustc quotes in its last note is named after it, and the call is spanned at the method; the page shows the compiler's message |
| E0030 to E0032 | macros | `port.rs` | one ui test each | none needed |
| E0033 | macros (a bound) | `port.rs` | `h2_port_error_needs_from` | none needed |
| E0040 to E0042 | macros | `query.rs` | `e0040_`, `e0041_`, `m3_query_cannot_cache_unit_or_option` | none needed |
| E0050 | schema | `validate.rs` | `E0050.txt` (5 cases) | **no golden, no why/fix**: message rewritten; a reserved name is a variant of its own; ids say "hash to the same id" |
| E0051 | schema | `validate.rs` | `E0051.txt` (3 cases) | **no golden, no why/fix**: "not an identifier" is a variant of its own; collision messages explain the conversion |
| E0052 | schema | `validate.rs` | `E0052.txt` | **no golden, no why/fix**: message rewritten |
| E0060 | macros (check) | `check.rs` | `h1_builtin_shadowed` | none needed |
| E0061 | macros (check) | `check.rs` | `h1_aliased_named_type`, `d1_not_an_undra_type` | NF2: a type with no Undra declaration is "not a type declared with `#[undra::api]`" (it said "alias"); the alias message names both fixes |
| E0062 | runtime (port proxy) | `port.rs` | `E0062.txt` | **runtime message was `undra: ..`**: now the four-line shape with the code and link; typed `HttpError::Network` / `FsError::Io` carry the code and link too |
| E0063, E0064 | macros | `types.rs`, `check.rs` | `m3_nested_option`, `m5_method_returns_an_object` | none needed |
| C0001 to C0014 | `undra` | `undra-cli` | 11 goldens | **linked to `errors.html#C00NN`, which did not exist on the page**: SPEC 12 has a table for them, the page has a section, `config.rs` gives a real example for a wrong-typed value (`name = "text"`, it printed `name = ...`) |

Totals: 30 E codes, every one with an emitting site that is not a test and a golden (E0050 to E0052 and
E0062 are not macro codes: their goldens are `crates/*/tests/golden/diagnostics/<code>.txt`), and 14
C codes (11 with a golden; C0006, C0012 and C0013 need a built core, another platform or a failing dev
server and are listed in `catalogue.rs`). Before: 10 codes showed "no compile-fail example" on the page
(E0011, E0022, E0033, E0050 to E0052, E0060 to E0062, E0064: four had no golden at all, six had one
in a shape the page's parser did not read), E0022 had no constant, the schema-level messages of nine
codes had no why or fix, the runtime message of E0062 was not in the shape and the CLI's 14 codes
linked to anchors that did not exist. After: the page shows 44 codes and 134 real messages (135 after the review added a case).

E0050 to E0052 cannot be a compile error (a macro sees one item), so their tests are two kinds: message
goldens for hand-built schemas (`undra-bindgen/tests/diagnostics.rs`) and the path a user takes, a core
written with the macros whose registrations add up to an invalid schema (`crates/undra/tests/schema_diagnostics.rs`:
two `Todo` records in two modules, `due_date` and `dueDate`, a record named `Signal`). E0052 needs a
hand-written schema (ids are derived from names, so the macros never produce another id).

## The polish items (before and after)

**Query in an `impl` block that is not `#[undra::api]` (d05).** A macro cannot see the block it is
in. In an `#[undra::api] impl` it was already E0007 (`m5_query_inside_an_impl`). In a plain `impl`
the items a query adds are module-level, and rustc complained about each one: ten errors, none
naming the query ("struct is not supported in `trait`s or `impl`s", "associated `static` items are
not allowed", ..). Now the struct, constants, statics and registrations are declared and invoked
through a `macro_rules!` named after the rule, so there are three: "macro definition is not supported
in `trait`s or `impl`s" (rustc's help: move it out to a nearby module scope), "implementation is not
supported in `trait`s or `impl`s" and "cannot find macro
`_undra_error_E0007_a_query_is_a_free_function_move_it_out_of_the_impl_block`". The `QueryDef` impl
and the checks stay outside the guard: they hold the user's own types, and an ordinary error on one
(a parameter that cannot cross) must not claim to come from the guard (the first attempt wrapped
everything and leaked that note into every E0001 of a query or function; it was backed out for
`#[undra::api] fn`, which keeps its old cascade). A signature that names `Self` is certain to be in
an impl block: branded E0007 for a query, a mutation and `#[undra::api] fn`
("`#[undra::query]` on `admins`, which is inside an `impl` block: its signature uses `Self`"). An
Undra attribute on a port trait method is E0007 too. Tests: `d1_query_in_a_plain_impl`,
`d1_api_function_in_a_plain_impl`, `d1_port_method_with_an_undra_attribute`.

**Split impl (d03).** Two `#[undra::api] impl Calc` blocks gave seven errors (E0428 x4, E0119 x2,
E0592). The block's private items (dispatcher, registration, store probe) now live in a `const _`
block; three errors remain, the ones Rust forces: the sentinel constant, whose redefinition reads
"the name `_undra_error_E0007_Calc_has_two_undra_api_impl_blocks_merge_them_into_one` is defined
multiple times", `UndraObject` implemented twice, and `__UNDRA_IS_OBJECT` defined twice
(`m5_two_api_impl_blocks`). A second `#[undra::api(store)]` block has no constructor, which read "store
`Counter` has no constructor / add `new` to the impl block" and now says a type takes one block, that a
constructor in another is not seen, and to move the methods there (`d1_two_store_blocks`). Not done:
a store literal `Self { .. }` in a *plain* `impl` of the store still needs `__undra_cell` spelled out
(rustc's E0063); SPEC 16.3 documents it.

**NF1, a key that names no field.** rustc's E0609 suggested `key = id`, unquoted, itself an error.
The macro cannot see the item type, so the lookup moved into the user's crate: every `#[undra::api]`
record has `__UNDRA_FIELDS` and `__undra_encode_field::<I>`, `undra_meta::keys::index_of` finds the
index in a constant, and a key that names no field panics there with
"`#[undra(key = "idd")]` on `typo` names no field of `Row` / `Row` has the fields `id`, `title`",
reported on the string, with no rustc error after it (`d1_key_names_no_field`, `l5_key_names_a_missing_field`).
Cost: two hidden members per record. The key function calls the field's encoder by constant index, the
same bytes as before (the keyed-list e2e, ports and playground tests pass; `Box<Row>` items and
non-record items are handled).

**NF2, a type that is not an Undra type.** A plain struct in a field gave E0001 (Encode), E0001
(Decode) and an E0061 that said the type "is not that type" (it is exactly the type that is written).
Now E0061 says "`Plain` is not a type declared with `#[undra::api]`", with the alias case
(`type Id = u64`) named: it is also the only early catch for an alias of a type that has `Encode`
(`d1_not_an_undra_type`). Two E0001 remain, one per codec trait, and one E0061: three errors that
agree on the fix; rustc will not merge them. An object used as a value is still E0001 plus E0064.

## Other changes

* **Wording.** Unknown options and macro arguments suggest the nearest name (`did you mean
  `default`?`); a misplaced option says where it belongs; the attribute on the wrong item says what
  the item is and what to put it on; tuples and arrays name the replacement (`struct Pair { first:
  u32, second: String }`, `Vec<T>` / `Bytes`).
* **E0062 at run time** has the shape of every diagnostic (`Diag::runtime_template`); `HttpError` and
  `FsError` for an unavailable port carry `E0062` and the link. SPEC 8 and 12, the ports README and
  the site's ports page say so. No platform test depended on the old text.
* **Schema validation** messages (`undra-meta`, `undra-bindgen`) use `undra_meta::diag::message`: what, note,
  help, docs. `BindgenError::Unsupported` gained a `why`; `ReservedName` and `NotAnIdentifier` are new
  variants (E0050, E0051).
* **Site.** `errors.html` is generated from SPEC 12's tables, the diag.rs table, `Code`'s docs, the ui
  goldens and `crates/*/tests/golden/diagnostics/*.txt`; its parser reads all three message shapes
  (compile_error, `on_unimplemented`, const panic); the compiler's own message is shown for E0022.

## What is still not ideal (for the integrator)

* The query-in-plain-impl and split-impl cases are the compiler's messages with the rule in a name;
  a macro cannot see its parent, and nothing stable lets it ask.
* `#[undra::api] fn` without `Self` in a plain impl keeps rustc's seven-error cascade (a guard
  leaked into unrelated errors, see above).
* `e0011_store_marker_without_store` keeps an E0560 ("no field `__undra_cell`") before the branded
  E0011: the macro patches struct literals, and cannot know the struct is not a store.
* E0022 cannot carry the code: rustc picks the leaf (`Send`) message for auto traits, which
  `#[diagnostic::on_unimplemented]` cannot override (tried).
* The CLI's C0006, C0012 and C0013 have no golden; `catalogue.rs` lists why.
* `undra-wire`'s `Encode`/`Decode` messages still say "return a record" for a parameter; harmless.

## Verification

`cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace
--no-fail-fast` are clean: 122 suites, 2280 tests passed, 0 failed, 10 ignored (the ignored ones were
before). `cargo test -p undra-bench --test budgets --release` passes (the bench fixtures use keyed stores,
the one hot path whose generated code changed; the field encoder is a constant `match`, inlined).
`undra-meta` still checks for `wasm32-unknown-unknown`, `aarch64-apple-ios` and `aarch64-linux-android`. New
tests: seven ui tests (`d1_*`, 42 to 49), nine in `catalogue.rs`, nine in `src/tests/diagnostics.rs`
(27 to 36 plus unit tests in `attrs.rs`, `check.rs`, `store.rs`, `query.rs`, `port.rs`), two in
`undra-bindgen/tests/diagnostics.rs`, three in `crates/undra/tests/schema_diagnostics.rs`, eleven in
`undra-cli/tests/diagnostics.rs`, the shape assertions in `undra-meta`, `undra-ports` and the `keys` module.
Site: `node site/scripts/build-errors.mjs && node site/scripts/build-all.mjs && node site/scripts/check-links.mjs`
(44 codes, 134 real messages, 20 pages OK).

## Commands

`TRYBUILD=overwrite cargo test -p undra-macros --test compile_fail`, `UPDATE_SNAPSHOTS=1 cargo test -p
undra-macros --lib`, `UPDATE_GOLDEN=1 cargo test -p undra-bindgen --test diagnostics`, `UPDATE_GOLDEN=1 cargo
test -p undra-ports --test ports_runtime`, `UPDATE_GOLDEN=1 cargo test -p undra-cli --test
diagnostics`, then `node site/scripts/build-errors.mjs && node site/scripts/build-all.mjs && node
site/scripts/check-links.mjs`.

## After review

The adversarial review (`.10x/reviews/2026-10-01-diagnostics-review.md`) changed three things described
above: a record now carries `__UNDRA_FIELDS` only (the per-record `__undra_encode_field::<I>` cost about 16 %
of `cargo check` on a records-only crate; the key function reads the field by name through a reference typed by
the constant check, so a key naming no field is still the one E0008); the E0011 store probe is the length of an
array in a signature, so `e0011_store_marker_without_store` reports E0011 before the E0560; and the catalogue
and the page accept a compiler-only golden for E0022 alone. SPEC 16.3 describes the result.
