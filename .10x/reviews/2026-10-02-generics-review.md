# `generics-fn-obj` (ADR-058: generic functions monomorphised from a declared list; generic objects and stores as templates instantiated through an alias) - adversarial review

**Date:** 2026-10-02 · **Reviewer:** Claude Fable 5.1 (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:** `wt/generics-fn-obj` at
`4d18393` (23 commits on `a309e9f`) · **Read:** `CLAUDE.md` (R1, R3, R4, R7, R8, R9, R12), ADR-058 with its implementation note and
deviations, `.10x/decisions/architect/generics-fn-obj.md`, `.10x/decisions/sde/generics-fn-obj.md`, SPEC 1.1, 2.2, 4.1, 4.3, 10.3d, 12, and
the diff: `undra-meta` (`GenericOf`, the canonical form, the E0072 validation), `undra-macros` (`generic_fn.rs`, `generic_object.rs`,
`generic.rs`'s `__instantiate!` and re-homing, `object.rs`'s template/instance modes, `check.rs`'s listed and application assertions), the
three generators and the `generic_functions` / `generic_objects` goldens and fixtures, `selection.rs` and the three apps, S34,
`site/docs/generics.html`, the types page, the post and roadmap wording · **Fixes:** six `fix(generics): review fixes` commits, then this
record.

## Verdict

**Sound with fixes; merge once CI is green on the pushed head.** The design holds where it matters: a schema without generics is byte-identical
in canonical form and hash (the pin compares against `0xd5b8_c3a3_afbd_bc33`, the golden `main` already had, and no existing golden file
moved), the label is in the canonical form only when set, the order of a list never moves the hash, every instantiation is an ordinary
definition with an ordinary id, and nothing on the wire, the ABI or a platform runtime changed. The macros do not swallow a user's own type
error: a deliberate mismatch inside a generic function, a generic object's method and a generic store's method each comes out as `rustc`'s
plain `E0308` at the user's line, once. The TypeScript overload sets are closed (the implementation signature is not callable, a literal with
the other type's rows does not compile; now a test). No **High** finding. Two **Medium**, fixed: a TypeScript name collision the validation
let through (the generated package would not compile), and a duplicate instantiation reported with a fix a real core cannot follow. Four
**Low**, fixed (two wrong fix texts, E0074's and a foreign object alias's E0070, and two undocumented behaviours in the guide). The rest are open items, none blocking.

## Findings

| # | Sev | Where | Finding | Status |
|---|---|---|---|---|
| M1 | Medium | `crates/undra-bindgen/src/validate.rs` (`check_object`, `check_top_level_values`) | TypeScript keeps one **private** function or method per instantiation, named like its id constant (`pinnedTodo`, `newestTodo`), in the same class or module as the store's signals and the other families' exported overload sets. Validation compared those names only with other id constants, so a store signal `pinned_todo` beside `pinned<Todo>`, or a family `newest_todo<Note>` beside `newest<Todo>`, passed and the generated TypeScript declared one name twice (the existing test even asserted the second pair was fine). | **Fixed** (`5127fbb`): `private_names_of_instantiations` reports E0051 against signals and other families' native names; two tests, the old assertion corrected. |
| M2 | Medium | `crates/undra-meta/src/validate.rs` (`disagreement`) | Two modules of one core that each list `Todo` for a `newest` make two `newest<Todo>`. `undra-meta` reported E0072 "the instantiations of `newest` disagree … are instantiated with the same type" with the fix **"regenerate the schema from the core (`undra build`); a schema is not edited by hand"**, which is what produced it; and meta errors short-circuit the bindgen's. | **Fixed** (`5127fbb`): one instantiation twice is left to the bindgen's name check, which reports it as it reports two plain functions with one name (E0051, "`newest<Todo>`, `newest<Todo>` all become `newestTodo`"). A label that does not match its name is still E0072. Test in `tests/validate.rs` fails before. |
| L1 | Low | `crates/undra-macros/src/impl_/generic_object.rs:146` (`check_header`) | E0074's fix took its parameter count from the block, not the type: `impl<A> Pair<A, Todo>` was told `write impl<A> Pair<A>` and `pub type TodoPair = Pair<Todo>;`, which do not compile for a two-parameter `Pair`. | **Fixed** (`7bf98b7`): the count comes from the self type (`impl<A, B> Pair<A, B>`, `Pair<Todo, Tag>`); unit test over five headers; trybuild golden and `errors.html` regenerated. |
| L2 | Low | `site/docs/generics.html` ("Where to put what"), SPEC 4.3 | The guide says an alias away from its template needs the template's **types** in scope. For a **store** that is not enough: the instantiation reads the store's private fields and calls its private `restore` hook, so an alias in another module fails with `rustc`'s "field `rows` of struct `Selection` is private" / "associated function `assemble` is private" (probed). The ADR's prototype claimed a store alias in another module works; the tests only ever put a plain object's alias elsewhere. | **Fixed (docs)** (`835bb1b`): the guide and SPEC say so. Open: a dispatcher generated at the template (ADR-058's own lift for 3.5) would remove the rule. |
| L3 | Low | `site/docs/generics.html` | An argument that does not carry its type picks no overload: Swift `newest(rows: [])` and `nil` are "ambiguous use of", Kotlin `newest(emptyList())` is "cannot infer type" (measured, Swift 6.3, kotlinc 2.0.21 and 2.4.20). The ADR mentions it; the guide did not. | **Fixed (docs)** (`835bb1b`), with the annotations that resolve it (`[Todo]()`, `emptyList<Todo>()`, a declared result type in Swift); TypeScript's literal needs none. |
| L9 | Low | `crates/undra-macros/src/impl_/generic.rs` (`instantiate_object_in`) | A generic **object's** alias in another crate (probed with a two-crate scratch workspace: one clean E0070) was told "or declare a concrete `#[undra::api]` struct or enum in this crate", the data types' alternative. | **Fixed** (`daa882b`): "or write the object out in this crate, with an `#[undra::api]` impl block of its own"; the unit test asserts it. |
| L4 | Low | `crates/undra-bindgen/src/ts.rs` (family emitter) | Tree-shaking: the exported overload set switches on the literal, so a bundle that calls `newest("Todo", ..)` carries every instantiation's function and codec. Inherent to one exported name per family. | **Documented** (`835bb1b`); open if a web app reports it. |
| L5 | Low | ADR-058 deviation 10, `.10x/decisions/sde/generics-fn-obj.md` | The size note said `main` 116,550 and +665 B. Built here with Rust 1.99.0 and wasm-opt 133, the merge base `a309e9f` is **116,023** and the branch **117,215**: **+1,192 B** gzipped, above the Risks' "well under 1 KB", inside the 120,000 gate (2,785 B left). The JavaScript up front is 22,100 on both (gate 22,100). | **Corrected** in ADR-058 (`835bb1b`). Open: the serde `Serialize` of two new structs is most of it, a hand-written writer would cut it if the web gate gets tight. |
| L6 | Low | `crates/undra-macros/src/impl_/generic.rs` (`instantiate_object_in`, the E0070 rule constant) | The module-level constant that makes a second alias the first error is named from the type arguments with every non-alphanumeric character dropped and the arguments joined by `_`: `Cache<A_B, C>` and `Cache<A, B_C>`, or `Cache<Vec<Todo>>` and `Cache<VecTodo>`, in one module would be told "declared twice". Needs type names nobody writes. | Open. |
| L7 | Low | `examples/playground/core/src/selection.rs:34` (`DRAFTS`) | `draft`'s serial is a process-global `AtomicU64` from 0, so after a restore a "New draft" can get the serial of a drafted row the restored selection still holds, and `toggle` (identity by serial) unticks it instead of adding one. Deterministic (R12 holds), playground only. | Open (nit). |
| L8 | Low | `crates/undra-cli/tests/symbols.rs:1090` | Locally (an emulator booted) the Android half of `shipped_artefacts_do_not_grow_and_no_symbols_writes_none` fails: the with-symbols `libplayground_core.so` is 16 B longer (`.text`), with no margin where the wasm half has 0.25%. CI has no device and skips it. Same as the implementer saw. | Open: give the Android comparison the wasm half's margin, in the symbols piece's owner's hands. |

## The attack, surface by surface

**1. R1/R7: the schema and the hash.** *Pin:* `a_generic_label_is_written_only_when_set` asserts the representative schema hashes to
`0xd5b8_c3a3_afbd_bc33`, the constant `main`'s `canonical.rs` already pins in three tests (not a value it computes); no golden file of an
existing bindgen case changed, and their `Ids` files carry the hash. *Order:* the canonical form sorts functions and an object's methods by
**name** (`crate::sort::by_name`), so the key is `newest<Note>` / `newest<Todo>`: independent of declaration order and of any other type's
name; the proptest shuffles six instantiations and adds/removes one. *Same schema name from two modules:* listing `a::Todo` and `b::Todo` is
E0072 "listed twice" at the macro (the schema name is the last segment); two functions each listing `Todo` was M2. *Id space:* a function name
with `<` cannot be written (Rust identifiers, no rename option in `attrs.rs`), so `fnv1a32("fn.newest<Todo>")` lives beside the existing ids
without a rule of its own; an fnv collision is the runtime's `DispatchTable::collect` collision report at init and the bindgen's `dup_ids`
(E0050), as for every id. *Fingerprints:* `TodoSelection` and `NoteSelection` differ (`selection.rs` test, S34 step 4 restores both).
Result: clean but M2.

**2. Macro soundness and hygiene.** Fuzzed in a scratch crate against the branch's `undra` and in the existing trybuild/unit suites: empty
list, duplicates, `Vec<Todo>`/`u32`/`String`/`Arc<Todo>` (E0072 each), an object in the list (E0072 by const assertion), a plain struct
(E0001/E0061), two parameters, lifetimes (E0003), const generics (E0002), a `where` clause (emitted as written; an unmet bound is `rustc`'s
E0277 **at the list entry**), `impl Trait` arguments (E0004), `async` + `ctx`, streams, a method with its own list in a generic object
(E0074, with and without a list), another crate (E0070: unit test, and a two-crate probe that gets exactly one error; L9), the alias away from its template (types: the documented `rustc` message;
store fields and hook: L2), a store with a `Computed` and a `restore` hook, keyed lists keyed by a field of `T`, `Lazy<T>` / `DerivedList<T>`
stores (`tests/generic_objects.rs`). Every refusal has the four parts and a link; the catalogue audit and `errors.html` list E0072 and E0074.
**Span re-homing:** a deliberate `let x: u32 = "…"` in a generic function, `let wrong: String = 5u32` in a generic object's method and
`let oops: bool = row.n()` in a generic store's method each give exactly one `E0308` at the user's line, no macro-internal text. A
misspelled type in a template **signature** is reported at the signature once for the template and once per alias ("in this attribute macro
expansion"): `rustc`'s repetition, pointing at the user's line. The trybuild goldens were re-recorded on 1.99.0 (the branch's six older ones
already were). Result: clean but L1, L2.

**3. R3: the generated code.** Swift: overloads on the parameter's type, `_ type: Todo.Type` only when no parameter fixes `T`; `[]` and
`nil` are ambiguous (L3). Kotlin: every overload has `@JvmName(<id name>)`, which the id check keeps unique among functions and mutations
(top-level functions are in `ObjectsKt`, mutations in `QueriesKt`); `KClass` tokens compile with `-Werror` and run under kotlinc 2.4.20 and
2.0.21 (`typecheck_kotlin`, both). Java sees `newestTodo(List, UndraCore)`. TypeScript: overload signatures plus one implementation that is
not callable; a new tsc test (`a53a521`) makes `newest("Todo", notes)`, `draft("Draft")`, a union literal, a wrong return type, the private
`library.pinnedTodo()` and the unexported `newestTodo` errors; the default branch rejects with `UndraCallError.Refused` (a promise),
throws it (a stream) or reports it (a command). Tree-shaking: L4. The run fixtures (`run_ts` 14, the Kotlin and Swift execution fixtures)
pass. Result: M1, L3, L4.

**4. Objects and stores through aliases.** One `ObjectDef` per alias, its own type id, byte for byte a hand-written store (the
`generic_objects` golden locks "no generator change"); ADR-040 as parameter and return (`Arc<TodoCache>`, `&TodoCache`, S34 step 6: a
`RecentTodos` is the alias's class, a second call another wrapper, closing one leaves the other); restore through the instantiation's own
restorer (S34 step 4, handles kept, type ids differ); two aliases independent (S34 step 3: a tick is one keyed `Insert` on one store and
nothing on the other); devtools render `newest<Todo>` and `Counter.pinned<Todo>` as text (unit test), and a store under its alias (schema
driven); recordings carry ids as numbers and were re-blessed for the new playground schema. Result: clean.

**5. Sizes.** `scripts/wasm-size.sh` on the merge base and on the branch, same machine, Rust 1.99.0, wasm-opt 133: hello wasm **116,023 →
117,215** gzipped (+1,192; gate 120,000); up-front JavaScript **22,100 → 22,100** (gate 22,100; `bindings_gzipped` 1,471 both). Result: L5.

**6. Machine-speed independence.** No new test asserts an absolute time or wraps a compiler run in a deadline (searched the S34 runners,
the macro tests, the bindgen fixtures, the smoke steps). The bench row `dispatch/call_sync/generic_fn` has an absolute budget (250 ns,
scaled by `UNDRA_BENCH_SCALE` like every row) and is gated by the ratio `generic_fn_vs_function` (1.2), which held here at load average
above 100. Result: clean.

## Verification (local, macOS, Rust 1.99.0, at the review's last fix)

| Check | Result |
|---|---|
| `cargo fmt --check`; clippy `-D warnings` (workspace, all targets; `undra-ffi` on wasm32); `cargo doc --no-deps` with `-D warnings` | clean |
| `cargo test --workspace` (`UNDRA_REQUIRE_TOOLCHAINS=1`) | 3,614 passed, 1 failed (L8, local only), 23 ignored |
| wasm ABI harness | 36 pass |
| TypeScript runtime `npm test`, `tsc` | 1,856 (three 5 s timeouts at load average 121 pass alone and on the merge base: the runtime is unchanged); tsc clean |
| Swift runtime; Swift over the C ABI | 870, 0 failures; 6 |
| Kotlin runtime under kotlinc 2.4.20 and 2.0.21 | 881 cases (2 skipped) + 32 testkit, 0 failed, each |
| Contract grid (`run-all.sh`, Kotlin on 2.0.21 as CI) | TS 34/34, Swift 32/32, Kotlin 32/32: **98 of 98** cells pass, S34 on all three |
| React Native: `npm test`, typecheck, `test:contract`, `cpp/test/run.sh` | 110; clean; 25 pass + 2 skipped (S34 passes); 30 checks |
| interop (`crates/undra-transport/interop/run.sh ts`, `kotlin`) | OK, OK |
| `undra bindgen --check --docs` (playground, two-cores a and b, cookbook, Fieldbook); `--check` ios15-sample; `schema_retention`, `schema_docs` | up to date; pass |
| Budgets (`undra-bench --test budgets --release`), alloc gates (`sync_alloc`, `commit_alloc`, `derived_alloc`, `lazy_alloc`) | pass |
| Both size gates | pass (above) |
| Playground web (`npm test`, `npm run build`), Android `:app:assembleDebug` | 135, built; `BUILD SUCCESSFUL` |
| Site `build-all` (no diff after the last commit), `check-links --words` | clean; landing 347 words |
| Devtools `build.sh --check` | differs from a rebuild **on the merge base too** on this Mac (toolchain): CI decides |

## Landing

`main` merged twice after the matrix: `fc326d6` (the CI piece: Rust 1.99.0 pins, the trybuild goldens it regenerated, Node 24, the `wt/**`
trigger) as `f344ac5`, then `1e8f33c` (diagram-rn, site and state only) as `2c6fd7e`; each time one conflict, the line of each piece in
`.10x/decisions/sde/_index.md` (both kept), every trybuild golden auto-merged and `site/scripts/build-all.mjs` left nothing to regenerate.
`scripts/ci-local.sh` on `f344ac5`: the jobs that do not depend on this machine's provisioning pass (Rust, TypeScript, wasm, Swift and C ABI,
React Native, playground web, budgets on a trial run, size, two cores on iOS, JVM and Node, Site); the Kotlin steps need
`UNDRA_SQLITE_JDBC` at the runner's path, so the Kotlin runtime (both compilers) and the Kotlin contract column were run directly above. The
pushed head's CI, Bench, Two cores and Site runs are in the integrator's hand-back.

## Open items

L2's lift (dispatch generated at the template), L4, L5's writer, L6, L7, L8. None blocks the merge.

## Follow-ups (2026-10-02, `wt/generics-followups`)

The open lows that were defects are closed; L2, L4 and L5 stay as documented limits (`.10x/status.md`).
Record: `.10x/decisions/sde/generics-followups.md`.

| # | Closed by | How |
|---|---|---|
| L6 | `1da4919` | The E0070 rule constant spells its type arguments without loss: letters and digits as written, every other character `_` and a code (`_` is `__`, `<` is `_L`, `:` is `_C`, ..., anything else `_U<hex>_`), the arguments joined by `_A`. `Cache<A_B, C>` / `Cache<A, B_C>`, `Vec<Todo>` / `VecTodo`, `crate::model::Todo` / `cratemodelTodo` and `A_B` / `A, B` were one constant each before (the test also keeps `A, BC` / `AB, C` apart); the new unit test fails on the old key and passes now. `Todo` still reads as `Todo`, so no trybuild golden moved. SPEC 4.3 says so. |
| L7 | `2691d73` | `Selection::assemble` raises the draft counter above every draft the restored selection holds (`fetch_max`), as `Todos::assemble` continues its identities above the snapshot's. Test: a selection holding a draft a million serials ahead (as an earlier run of the core would leave it), snapshot, restore, `draft<Todo>` through the dispatcher: the new draft is above it (it was not before). No schema change: the playground's and two-cores' bindings are up to date. |
| L8 | `e853565` | Not a regression of ADR-058: on `1e8f33c` (main before it) the same test passes, and the same build is 16 bytes **smaller** with symbols on x86_64 (56 on the host dylib); on this branch it was 0 on arm64 and +16 on x86_64. `llvm-readelf` shows why: both libraries have the same 26 sections of the same sizes but `.text` (±16 bytes: LLVM lays code out slightly differently with `debug = "line-tables-only"`), `.relro_padding` (no file bytes) and `.shstrtab` (rewritten by `llvm-strip`, 5 bytes smaller). The test now claims what must hold: the same sections in the same order (no `.debug_*`, `.symtab`, `.strtab`), every non-code section the same size, the code and unwind tables within a thousandth (two orders above that noise, two below a change in what is compiled); and the unstripped twin must fail the comparison. |
| CI | `1a4d996` | Found on this branch's first CI run (7f561c0): the Kotlin runtime's `ChangeSetTests` "iteration allocates nothing per entry" measured 2,120 extra bytes against a fixed 2,048 (code untouched since the rename). The bound is now per entry, less than one byte per extra entry: one allocation per entry is at least 16 bytes, and the mutation (a reader per entry in `forEachEntry`) fails it with 639,680 bytes. Swift and TypeScript count allocation events exactly and have no such bound. |
