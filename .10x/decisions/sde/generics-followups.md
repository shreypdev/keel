# generics-followups: the open lows of the generics-fn-obj review (L6, L7, L8)

Piece `generics-followups`, worktree `wt/generics-followups`, base `ae362ac`. Author: Claude Opus. The review is
`.10x/reviews/2026-10-02-generics-review.md` (its "Follow-ups" section); L2, L4 and L5 are documented limits, not defects,
and stay open as such.

## What changed

* **L6** (`crates/undra-macros/src/impl_/generic.rs`, `rule_key`): the module-level E0070 constant of a generic object's or
  store's instantiation names its type arguments with a lossless encoding (ASCII letters and digits as written, every other
  character `_` plus a code, arguments joined by `_A`), so two different instantiations in one module never define one
  constant (a false "declared twice"). A plain argument (`Todo`) reads as before: no trybuild golden moved. Test:
  `the_rule_of_e0070_names_each_instantiation_unambiguously` (four of its five pairs collided before). SPEC 4.3 has the rule.
* **L7** (`examples/playground/core/src/selection.rs`): `Selection::assemble` raises the process-wide draft counter above the
  drafts the restored selection holds, so `draft` never reissues an identity an older run handed out and `toggle` never
  unticks the old draft instead of ticking the new one. Test through the dispatcher only (TestRuntime, ids):
  `a_draft_after_a_restore_is_above_every_drafted_row_the_selection_holds`. The schema did not move.
* **L8** (`crates/undra-cli/tests/symbols.rs`): the Android half of `shipped_artefacts_do_not_grow_and_no_symbols_writes_none`
  compares the shipped library with the `--no-symbols` one section by section (`not_the_plain_library`): same sections and
  kinds in order, non-code sections the same size exactly, `.shstrtab` not larger, code plus unwind tables within 1/1000;
  the unstripped twin must fail it. Established first: on `1e8f33c` (main before ADR-058) the old test passes, with the
  with-symbols x86_64 library 16 bytes smaller; on this branch it is 16 bytes larger. The byte delta is `.text` only.

## Verification (local, macOS, Rust 1.99.0)

| Check | Result |
|---|---|
| `cargo test -p undra-macros` (unit, behaviour, trybuild) | pass; the L6 test fails on the old key |
| `cargo test -p playground-core` | 112 + 4 pass; the L7 test fails before the fix |
| `cargo test -p undra-cli --test symbols shipped_artefacts_do_not_grow_and_no_symbols_writes_none` (emulator booted, NDK r27) | pass on this branch (x86_64 +16 B of `.text`); the old test, on `1e8f33c`, passes |
| `undra bindgen --check --docs` (playground, two-cores a and b) | up to date |
| fmt, clippy `-D warnings` on the touched crates | clean |
