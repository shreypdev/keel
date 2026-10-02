# generics-fn-obj: ADR-058 (generic functions, methods, objects and stores cross as declared instantiations)

Piece `generics-fn-obj` (the three pieces of the brief, A, B and C, on one branch), worktree `wt/generics-fn-obj`, base `4fdd149`.
Author: Claude Sonnet 5.5 (the brief asked for Fable 5.1; the harness ran this session as Sonnet 5.5, and the commit trailers say so).
The decision is ADR-058 (flipped to Accepted with the dated implementation note and the deviations); this record is what was built,
how it was verified and what the review should look at.

## What landed, in commit order

**A. `generics-meta-macros`** (`undra-meta`, `undra-macros`, `undra`)

* `undra-meta`: `GenericOf { of, args: [GenericArg { param, ty, inferred }] }` on `FunctionDef` and `MethodDef` (written only when
  set), the `'static` mirrors, the canonical form (so every schema without generics keeps its hash: pinned by tests), `without_docs`,
  the validation (E0072, `BadGeneric`: a label that does not describe its definition, instantiations of one family that disagree in
  shape, a parameter or type that does not exist), proptests (order-independent hash; a third instantiation changes it).
* `undra-macros`, functions and methods: `generic(T = [..])` on `#[undra::api]` fns and as a method helper attribute; the ordinary
  function/method expansion once per listed type, with the type substituted as a `None`-delimited group so `Scope::Instance` checks
  exactly it; the template pass over the generic signature (`Scope::Template`, errors once, at the function); instance diagnostics
  say "in `newest<Todo>`" (a thread-local guard in `diag.rs`).
* Objects and stores: templates (`#[undra::api(generic)]`, `#[undra::api(store, generic)]` over `#[undra::store(generic)]`) with
  positional placeholders, the hidden `macro_rules! __undra_template_<Name>` re-exported under the type's name, `undra::__instantiate!`
  (re-homes spans for objects and stores only, so ADR-042's record goldens did not move), the store's two halves meeting through a
  local macro and `undra::__compose_store!`, E0070 as a module-level constant named by template and arguments plus the inherent
  constant, `__UNDRA_IS_GENERIC_STORE`, stub templates on failure, E0074, `SignalValue` in the prelude.
* Section 4: `UNDRA_TYPE_NAME` / `__UNDRA_OBJECT_NAME` on every instantiation, `KType::NamedOf` / `ObjectOf`, a generic application of
  a type parameter (`Page<T>`, `Arc<Selection<T>>`) named through its alias, the fallback assertion (E0002 / E0064).
* Tests: `tests/generic_functions.rs`, `tests/generic_objects.rs` (run against the real runtime), expansion snapshots, one trybuild
  case per row of ADR-058 section 7 (goldens re-recorded on 1.99.0, where six older ones differ only by tuple-impl order), unit tests,
  the catalogue audit. SPEC 1.1, 2.2, 2.3, 4.1, 4.3, 12, 16.3; `site/docs/errors.html` regenerated.

**B. `generics-bindgen`** (`undra-bindgen`)

* `model.rs`: families (definitions sharing `generic.of`, among a schema's functions and among one object's methods), the native
  name and the id name of a definition, whether a family takes a type token. `validate.rs`: identifier and case-collision checks on
  both names (E0051 names the schema names that meet), E0072 passes through. `swift.rs`, `kotlin.rs`, `ts.rs`: overloads; the id files
  use the id names.
* Goldens `generic_functions` (a free family with `T` in the parameters, one with `T` in the return only, an `async` one with an error
  type, a stream, a command, a family of methods on an object and on a store, a parameter called `type`) and `generic_objects`
  (what two aliases of an object and of a store produce: no generator change). Diagnostics goldens E0051 (+2 cases) and E0072.
  Type-checked (`swiftc`, `kotlinc -Werror` under 2.4.20 and 2.0.21, `tsc`) and run (`run_ts`, a Kotlin and a Swift execution fixture).
* SPEC 10.3d, E0072 among the C0007 codes.

**C. `generics-playground`**

* `examples/playground/core/src/selection.rs`: `Row`, `newest` and `draft` over `Todo` and `Note`, `Selection<T>` with `TodoSelection`
  and `NoteSelection`, `Recent<T>` with `RecentTodos`, `recent_todos`; bindings regenerated (and `examples/two-cores/{a,b}`, which share
  the playground core; the testkit recordings re-blessed for the new schema hash).
* The three apps: a selection strip on the to-do and the note screens (a select control per row, "n selected", "Select all", "Clear",
  "Remove selected" on the to-dos, "New draft" = `draft`, "Latest" = `newest`). Web (`SelectionBar.tsx`, smoke steps for both screens),
  iOS (`SelectionStrip.swift`), Android (`SelectionStrip.kt`).
* Contract scenario S34 (six steps) on Swift, Kotlin and TypeScript, and in React Native's column (its include list); `check.sh`,
  `run-all.sh`, the runners' comments; S16 (TypeScript) learned the id names of instantiations.
* Bench: `dispatch/call_sync/generic_fn` (`add_one_for<Record5>`, the body of `add_one`) and the ratio gate `generic_fn_vs_function`.
* Docs: `site/docs/generics.html` (new, in the Core group), the "Generics" section and the limits table of `types.html`,
  `from-kmp.html` and `from-uniffi.html`, `site/data/roadmap.json`, README, the default-choice post (`index.html`, `claims.md` rows
  M03-N3, O34, T22, MI04, section 11), `site/data/tests.json` (`cellsPassing` 98), the devtools unit test that `newest<Todo>` and
  `Counter.pinned<Todo>` render as text.

## What the goldens look like to a native engineer (R3)

* Swift: `newest(rows: [Todo]) throws -> Todo?` and the same for `Note`: plain overloads on the parameter's type, one call site
  `try newest(rows: todos)`; `draft(_ type: Todo.Type, title: String)` where the type is in the return only, `try draft(Todo.self, ...)`;
  methods overloaded inside the class (`pinned(_ type: Todo.Type)`). No generics, no marker types.
* Kotlin: `newest(rows: List<Todo>): Todo?` with `@JvmName("newestTodo")` (so the JVM signatures differ), `draft(type: KClass<Todo>, ...)`;
  the ids are `NEWEST_TODO`. `-Werror` clean under both compilers.
* TypeScript: one exported overload set per family: `newest(type: "Todo", rows: Todo[], core?): Promise<Todo | null>` per type, one
  implementation switching on the literal (a refusal for anything else), private per-instantiation functions; methods the same.
  The leading literal is always there, so every family is called the same way.
* Stores: `TodoSelection` is generated exactly as a hand-written store (`@Observable` class, `StateFlow`s, `Signal`s): no generator
  change, which `generic_objects` locks.

## Verification (once, at the end; commands in the brief)

TBD

## Deviations

ADR-058, "Implementation and deviations" (11 items). Not in the ADR: the Swift typecheck target of the golden is named
`GoldenGenericFunctions` (the golden package's name, not the case's); the first red clippy of the branch was a `map_err(|e| e)` in a
test of `generic_fn.rs`, fixed.

## What the adversarial review should look at

1. The hygiene of `__instantiate!` (spans re-homed for objects and stores only) and the two-macro hand-over of a generic store, on the
   MSRV (not built locally) and on the pinned CI toolchain.
2. The scope rule (names resolve where the alias stands) against the guide's "declare the alias next to the template".
3. `Selection<T>`'s `serial()` identity in `selection.rs` (a to-do's counter is the first eight bytes of its `Uuid`), and the draft
   counter (`DRAFT_BASE + n`, a core-global atomic: relative assertions only).
4. The ratio gate's bound (1.2) against the ADR's 1.1.
