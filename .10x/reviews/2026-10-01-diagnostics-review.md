# D1, macro diagnostic polish and the E-code audit - adversarial review

**Date:** 2026-10-01 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/diagnostics` at `c25caee` (5 commits), merged with `main` at `08e4e39` (`4f56fec`) · **Read:** `CLAUDE.md`
(R3, R8, R11), the SDE record `.10x/decisions/sde/diagnostics.md`, `docs/SPEC.md` sections 12 and 16.3, the diff
(`crates/undra-macros/src/impl_/*`, `crates/undra-meta/src/{diag,keys}.rs`, the `d1_*` and changed ui goldens,
`tests/catalogue.rs`, `crates/undra-bindgen/tests/diagnostics.rs`, `crates/undra/tests/schema_diagnostics.rs`, the
CLI goldens, `site/scripts/build-errors.mjs`, `site/docs/errors.html`) · **Method:** every new or changed golden read
as a user; mutation runs of the catalogue test and the page generator; before/after builds of the playground core
(host cdylib and wasm, release, the shim profiles of `undra build`) and of a synthetic 400-record crate; three
alternative expansions compiled in isolation.

## Verdict

R8 holds, now on every case the macros can reach: every branded message has a code, what, why, fix and the link of
its code, every one of the 44 codes has an emitter that is not a test and a golden with its real message, and the
audit fails loudly when any of that drifts (tried: an emitter removed, a wrong link in source and in a golden, a
golden that lost its message, a reflowed SPEC table). The first line is the teaching one everywhere except where
rustc speaks first and nothing stable lets a macro speak before it: a query or mutation in a plain `impl` (rustc's
"macro definition is not supported" parse error leads; the rule's name follows in the same build) and E0022
(rustc's own message; the last note names the assertion). I tried the alternative expansions the brief suggests
and neither helps (below); the one case that could be fixed, E0011's struct-literal E0560 leading, is fixed. The
real problem was cost: the per-record `__undra_encode_field::<I>` that NF1 added to every record made `cargo check`
about 16 % slower on a records-only crate to serve a diagnostic only keyed lists need. It is replaced by a design
where a record carries a constant and nothing else (M1). Three Medium findings, all fixed; ten Low, seven fixed;
nothing High. No ADR is needed (below).

## Cost of the hidden members (brief item 1)

Builds with `CARGO_INCREMENTAL=0` on this Mac (Apple Silicon, 18 cores; other workloads ran at the same time, load
average 13 to 34, so wall times carry a few hundred milliseconds of noise). "Before" is `main` (`08e4e39`), "after"
the branch as reviewed (`4f56fec`).

| Measure | Before | After (as reviewed) | After the fix |
|---|---|---|---|
| Playground host cdylib, release (fat LTO, stripped) | 1,482,720 B | 1,482,720 B (`__text` +152 B, `__const` +640 B, `__cstring` +96 B: the longer E0062 texts; page alignment hides them) | not re-measured: the fix removes a generic method that was never instantiated, and the constant it leaves is read only by constants |
| Playground wasm, `release-wasm` (`opt-level = "z"`, before `wasm-opt`) | 658,431 B | 659,118 B (+687 B, +0.10 %, the E0062 texts) | not re-measured (as above) |
| Playground core, `cargo check` after `touch` (3 runs) | 0.40 / 0.41 / 0.42 s | 0.42 / 0.47 / 0.72 s | not re-measured (see the synthetic row) |
| Playground core + shim, release, after `touch` (3 runs) | 10.5 / 13.4 / 14.8 s | 9.9 / 10.2 / 10.8 s | LTO dominates; noise |
| Synthetic crate, 400 records x 10 fields, `cargo check` (6 runs, min / median) | 1.22 / 1.28 s | 1.38 / 1.50 s (+13 % / +17 %) | 1.25 / 1.30 s (`__UNDRA_FIELDS` only, measured as a variant of the branch) |
| Synthetic crate, release rlib (3 runs) | 7.07 / 7.39 s | 7.26 / 7.46 s | not re-measured (nothing generic left to encode in metadata) |

The members are `#[doc(hidden)]` (`record.rs:375-388` at `4f56fec`), generate no registration and so cannot reach
the schema or the bindings: `undra bindgen -C examples/playground --check --docs` reports the committed bindings up
to date (schema hash `0xabdf844b53e0bc10`, unchanged) and the bindgen goldens did not move; rustdoc of the
playground core shows none of them. The keyed key function encoded the same field as before (the `match` on the
constant index folds), and the budgets gate was green for the SDE and is green after the fix. The binary and
runtime cost was zero; the compile-time cost was not: a generic method whose body is one `Encode::encode` call per
field is type-checked for every record, which is the cost of a second `Encode` impl on every record of every user,
paid so that a typo in a `key` reads well.

## Findings

| # | Sev | Where (at `4f56fec`) | Finding | Status |
|---|---|---|---|---|
| M1 | Medium | `crates/undra-macros/src/impl_/record.rs:151, 375-388`; `store.rs:199-272` | Every record carried `__undra_encode_field::<const I: usize>`, an N-arm `match` of `Encode::encode` calls, so the store's key function could encode the key field by index and never name it (NF1). Measured above: +13 to 17 % `cargo check` on records, for a diagnostic only keyed lists use. | **Fixed.** A record carries `__UNDRA_FIELDS` only (a constant; its own cost is in the noise). The key function keeps the constant lookup and the branded panic, and reads the field by name through `let __row: &<__UndraGate<{ __UNDRA_KEY_IS_A_FIELD }> as __UndraPass<Item>>::Out = __item;` (local items): when the check passes the type is `&Item`, when it fails the reference has no type and `rustc` reports nothing about the field, so NF1 is still exactly one error (goldens `d1_key_names_no_field`, `l5_key_names_a_missing_field`). A keyword key reads the raw field (`key = "type"` reads `r#type`); a key that is not an identifier reads a placeholder that is never type-checked. New unit test `a_key_is_read_as_the_field_it_names`, new ui cases (`Tagged` with `r#type`, `key = "not a name"`). SPEC 16.3, the macros README, `undra_meta::keys` updated. |
| M2 | Medium | `crates/undra-macros/tests/catalogue.rs:259-278`; `site/scripts/build-errors.mjs:84` | The audit's central claim, "every code has a golden with its real message", had a hole: any `tests/ui/e00NN_*.stderr` counted as the golden of E00NN by its file name (meant for E0022, whose message is rustc's). Replacing `e0002_generics.stderr` with a bare rustc error left the catalogue green, and the page would have shown E0002 as "the compiler reports this one itself". | **Fixed.** `COMPILER_MESSAGES` lists E0022 alone; its golden must quote `_undra_error_E0022_..`, and SPEC must say rustc raises it. The page generator takes a compiler-only golden only for a code SPEC says rustc raises, and fails if it does not name the assertion. Re-ran the mutation: both now fail ("E0002 has no golden", "no real message .. for E0002"). |
| M3 | Medium | `crates/undra-macros/src/impl_/object.rs:1373-1375`; `tests/ui/e0011_store_marker_without_store.stderr` | R8 first line: `#[undra::api(store)]` on a struct without `#[undra::store]` led with rustc's E0560 "struct `Plain` has no field named `__undra_cell`" (the macro patches struct literals), and E0011 came second, because a `const _` assertion is evaluated after every body is type-checked. Listed by the SDE as not fixable. | **Fixed.** The probe is the length of an array in a private function's signature (`fn __undra_store_probe() -> [(); { assert!(..); 0 }]`); rustc evaluates it while checking signatures, before bodies, so the branded E0011 is first (golden regenerated; E0560 follows). SPEC 16.3 says so. |
| L1 | Low | `site/scripts/build-errors.mjs:31` | The SPEC row regex required exactly one space around each `|`: a reflowed, padded table would drop codes from the page silently, while the catalogue test (which trims) stayed green. | **Fixed**: whitespace-tolerant regex; the page fails if a code of the diag.rs table or of `Code` is not read from SPEC. |
| L2 | Low | `crates/undra-macros/tests/catalogue.rs:71` | `spec_rows` trimmed `|` before whitespace, so an indented table row was skipped. | **Fixed** (`trim()` first). |
| L3 | Low | `crates/undra-macros/src/impl_/check.rs:395` | E0061 for an alias said "`Todo` here is an alias .. of a different Undra type, not the type declared as `Todo`", implying a `Todo` exists (in `h1_aliased_named_type` none does). | **Fixed**: "`Todo` here is an alias or a renamed import of an Undra type that is declared under another name". Golden, snapshots, page regenerated. |
| L4 | Low | `crates/undra-macros/src/impl_/query.rs:356` | A query's checks moved from `const _` to a named constant (`__UNDRA_CHECKS_<Fn>`, valid in an impl block) and no ui test exercised a failing check on a query. | **Fixed**: `d1_not_an_undra_type` has a query with an alias parameter; E0061 fires from `__UNDRA_CHECKS_ById`. |
| L5 | Low | `docs/SPEC.md:736` | The split-impl error a user sees is "the name `_undra_error_E0007_Calc_has_two_undra_api_impl_blocks_merge_them_into_one` is defined multiple times"; the E0007 row (and so the page and its search index) named the query guard but not this one. | **Fixed**: the row quotes it. |
| L6 | Low | `crates/undra-bindgen/src/validate.rs:729-1130` | Fifteen blank lines inside `BindgenError::Unsupported { .. }` literals (left by the `why` insertion); the note of "an object without a constructor" read "an object can only be reached through other calls, which are not supported". | **Fixed** (blank lines removed; "no other call can hand one out, so without a constructor it can never be created"). |
| L7 | Low | `.10x/decisions/sde/diagnostics.md:56, 153` | The record says 133 and 134 real messages. | **Fixed**: 134 at `c25caee`, 135 now; an "After review" section points here. |
| L8 | Low | `crates/undra-bindgen/src/validate.rs:144` | `BindgenError` is a public enum without `#[non_exhaustive]`; two new variants and a new `why` field on `Unsupported` break its Rust API for anyone matching on it. Only `undra-cli` does, inside the workspace. | **Open**: mention in the release notes; mark it `#[non_exhaustive]` at the next breaking release. |
| L9 | Low | `crates/undra-query/src/defs.rs` (`type Error: CacheValue`) | Found while adding L4's test, not in the D1 audit: a query whose `#[undra::error]` enum does not derive `Clone` gets rustc's unbranded "the trait bound `Failure: Clone` is not satisfied .. required for `<ByIdQuery as QueryDef>::Error` to implement `CacheValue`". An R8 gap. | **Open** (follow-up): `#[diagnostic::on_unimplemented]` on `CacheValue` with an E0041 or E0010 message, or have `#[undra::error]` derive `Clone`. |
| L10 | Low | `tests/ui/d1_not_an_undra_type.stderr` (E0001 for `Encode`), `e0022_future_not_send.stderr` | Two secondary spans point at the attribute (`#[k::api]`: "required by a bound introduced by this call"; E0022's last note). The primary spans are the user's tokens. Pre-existing (the `Encode` impl's path tokens carry the call-site span). | **Open**, cosmetic. |
| I1 | Info | `tests/ui/d1_query_in_a_plain_impl.stderr` | Brief item 2: a query in a plain `impl` leads with rustc's "macro definition is not supported in `trait`s or `impl`s". Tried instead: a `#[macro_export]` identity macro named after the rule, invoked by path (no definition in the output). rustc then reports one parse error per item ("struct is not supported..", "implementation is not supported..", "associated `static` items are not allowed") and never prints the rule. Parse errors always precede resolution errors, so the current expansion, whose third error names the rule, is the best stable Rust allows. | Accepted. |
| I2 | Info | `tests/ui/m5_two_api_impl_blocks.stderr` | Split impl: the first error is the sentinel's E0428, which reads as the rule. A per-crate "count" of blocks needs global state in the proc macro (wrong under rust-analyzer's long-lived server and for two `Calc`s in two modules); the two follow-ons (E0119, E0592) are what Rust forbids itself. | Accepted. |
| I3 | Info | `crates/undra-macros/src/impl_/port.rs:667-714`; `crates/undra-ports/src/records.rs:264` | E0062 at run time: the panic message is the four-line shape; the runtime guard keeps it intact (`crates/undra-runtime/src/guard.rs:105-124`); Swift surfaces it as `UndraCallError.panicked(message:)` ("the Undra core panicked: error[undra::E0062]: .."), Kotlin and TypeScript as "the core panicked: ..". `HttpError::Network` / `FsError::Io` carry the code and link in their payload. No platform test, doc or contract scenario quoted the old `undra: ..` text (grep of `runtimes/`, `contract-tests/`, `examples/`, `site/`); `undra-transport/tests/ports.rs` asserts substrings that still hold. | Note. |
| I4 | Info | site | 44 codes, 135 real messages; `check-links` 20 pages OK; `#C0001`..`#C0014` exist and the CLI's goldens link to them. | Note. |

## Message quality (R8), read as a user

* **First line teaches:** `d1_api_function_in_a_plain_impl`, `d1_port_method_with_an_undra_attribute`,
  `d1_two_store_blocks`, `d1_unknown_option_suggestions` (seven messages, each on the offending token: the option,
  the argument, the item's name, the word `store`), `d1_key_names_no_field` and `l5_` (on the string), `d1_not_an_undra_type`
  (E0001 first, with "declare `Plain` with `#[undra::api]`"; E0061 agrees on the fix), `h1_aliased_named_type`,
  `e0013_store_restore` (on the computed field), `m5_two_api_impl_blocks` (the sentinel's name), and now both E0011
  marker cases. The schema and CLI goldens all have what, note, help and the link, checked by the catalogue.
* **rustc first, rule findable:** the query in a plain impl (the page's E0007 row quotes the guard's name, and a
  `Self` in the signature gives the branded E0007 directly) and E0022 (the page shows the compiler's message and the
  assertion name). Both documented in SPEC 12 and 16.3.
* **Wording nits left as they are:** E0010's schema message repeats the enum name ("the variant of the error `Oops`
  .. at enum Oops, variant Bad"); E0005's schema message shows the schema spelling `result<u8,named:Oops>` rather
  than `Result<u8, Oops>` (the macros catch it first with the Rust spelling).

## Generated Rust under native review (R3)

What the expansion now puts in a user's crate besides v1's members: `__UNDRA_FIELDS` on records (one hidden
constant), a `macro_rules!` named after the query rule and invoked at once (unusual, but it is the only stable way to
name the rule, and SPEC 16.3 explains it), the object's private items inside `const _` (more conventional than the
module-level names it replaces), a signature-level store probe, and, in keyed stores only, the gate type. Each is
`#[doc(hidden)]` or private, none appears in rustdoc or in the bindings. A Rust engineer reading `cargo expand` would
want a comment at the gate and the probe; expansions carry none, so the macro source and SPEC 16.3 hold them.

## The ADR question (R11)

No ADR. R11 covers the wire, the runtime and threading model, and generated *public* shapes: what a Swift, Kotlin or
TypeScript engineer programs against, and the Rust API a core author uses. The hidden members are `__`-prefixed,
`#[doc(hidden)]` associated items that one expansion of `undra-macros` emits for another to read (record for
store), which is what SPEC section 16 already governs ("the exact names the macros emit .. Change them only
together"), the same kind of member as `__UNDRA_IS_OBJECT`, `__UNDRA_IS_STORE` and `__undra_cell`, which SPEC
16.3 governs and no ADR in `.10x/adrs/` introduces. They change no registration, so the schema, its hash and the bindings are unchanged
(`undra bindgen --check --docs` on the playground: up to date, `0xabdf844b53e0bc10`; the bindgen goldens did not
move). Version skew is not possible: one `undra-macros` expands both sides in a Cargo graph, and records of an incompatible Undra are not `Encode` for the other anyway. The same holds
for the query guard, the `const _` blocks and the probe. SPEC 16.3 lists all of it, which is the right home.

## Verification (after the fixes)

`cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`: clean. `cargo test --workspace
--no-fail-fast`: 128 suites, 2,281 passed, 0 failed, 10 ignored (the ignored ones predate the piece). `cargo test
-p undra-bench --test budgets --release`: 6 passed, 1 ignored (the baseline printer). `undra bindgen -C
examples/playground --check --docs`: up to date, schema hash `0xabdf844b53e0bc10` (the committed bindings are generated
with `--docs`; without the flag `--check` reports only their doc comments). `cargo check -p playground-core --target
wasm32-unknown-unknown`: clean. `node site/scripts/build-errors.mjs && node site/scripts/build-all.mjs && node
site/scripts/check-links.mjs`: 44 codes, 135 real messages, 20 pages OK. Rustdoc of the playground core (and its
search index): no `__UNDRA_FIELDS`, `__UndraGate`, `__UNDRA_CHECKS_`, `__undra_store_probe` or `_undra_error_`.
