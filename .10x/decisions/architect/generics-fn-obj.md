# Architect: generic functions, methods, objects and stores across the boundary (2026-10-02)

`wt/generics-fn-obj`. Binding text: **ADR-058** (Proposed). Prototype:
`.10x/specs/2026-10-02-generics-fn-obj-prototype.patch` (applies to `a309e9f`; not landed, no code changed on
this branch). Read: CLAUDE.md, SPEC 2, 3, 4.1, 4.3, 5.4, 5.6, 5.9, 10, 11, 12, 16.3, 17; ADR-037, 039 to 044,
054, 055; the types-paging review; `undra-meta`, `undra-macros` (`generic.rs`, `object.rs`, `store.rs`,
`check.rs`, `types.rs`), `undra-bindgen` (the three emitters, `validate.rs`, the goldens); the catalogue.

## Problem

ADR-042 made generic records and enums cross as named instantiations. A function, method, impl block or store
struct with a type parameter is still E0002, and the post lists it as not built. Catalogue row 22: KMP erases
generics across the Objective-C bridge and Swift export, UniFFI documents none of your own.

## The decision, in short

1. **Monomorphise where the generic is declared.** A function or method lists its types,
   `#[undra::api(generic(T = [Todo, Note]))]`; an object or store is a template instantiated by an alias,
   `#[undra::api] pub type TodoSelection = Selection<Todo>;`, exactly as ADR-042's data types are.
2. **The schema stays free of type parameters.** One `FunctionDef`/`MethodDef` per instantiation, named
   `newest<Todo>`, id `fnv1a32("fn.newest<Todo>")`, with an optional label `generic: { of, args: [{ param, ty,
   inferred }] }`; one ordinary `ObjectDef` per alias, with no label.
3. **No wire, ABI, runtime or platform-runtime change.** One method id per instantiation; a generic object's
   instantiation is byte for byte a hand-written object, so the generators do not change for objects and stores.
4. **Hashes**: the label is written only when set (existing hashes hold); adding an instantiation moves the
   hash, reordering does not.
5. **Platforms present a generic function as the closed set it is**: Swift and Kotlin overloads, with a leading
   type token (`Todo.self`, `Todo::class`) when the arguments do not fix `T`; TypeScript overload signatures
   with a leading literal (`"Todo"`), always. A type that is not instantiated is a compile error in all three.
6. **A generic store's two macros meet through a local macro**: the impl block, below its struct in the same
   module, hands its signatures to the struct's macro, and a hidden proc macro exports one template.
7. **`Page<T>` inside a generic signature** is named through its alias by the compiler (an inherent name
   constant on every instantiation), so `fn page_of<T>() -> Page<T>` works when `TodoPage` exists.
8. **Refused in v1.x, each with a teaching error and a stated lift**: open-ended `T`, two type parameters on a
   function, unnamed types in a function's list, a generic method inside a generic object, generic
   constructors, queries, mutations, ports, callback traits and errors, a template instantiated in another
   crate, and native generic classes or fused native generic functions.
9. New codes **E0072** (a function's instantiation list) and **E0074** (the impl block of a generic object);
   E0002, E0070, E0011 and E0008 gain triggers. Scenario **S34**.

## Probes (throwaway; the patch and the session scratch directory)

* **The real macros, patched**: generic functions with a list (sync, async, `ctx`, return-only `T`); a generic
  object and a generic store through aliases, in and outside the template's module; keyed patches keyed by a
  field of `T`; restore through the instantiation's restorer with a `Computed` and a hook; distinct
  fingerprints; the alias as an ADR-040 parameter and return; the alias name read from the type. 8 tests green
  against the real runtime.
* **A scratch crate pair**: the struct-macro / block-macro hand-over, an alias reaching the composed template by
  `use` and by path, and `rustc`'s message when the block is above its struct or in another module.
* **Native compilers**: the Swift, Kotlin and TypeScript shapes compile (Swift 6.3.3, kotlinc 2.4.20 with
  `-Werror`, tsc 5.9.3 strict) and their messages for an un-instantiated type were captured.
* **What the probes changed**: hygiene (tokens re-homed in `__instantiate!`, for objects and stores only: doing
  it for records moves two ADR-042 goldens); the alias as the self type; names resolve where the alias stands;
  `T: SignalValue` for a signal of `T`; Swift 6.3 fails to produce a diagnostic for an array **literal** of an
  un-instantiated type passed to an overload set (a variable gets a clear message).

## Rejected

Inference from call sites; type-erased `Value`; one id plus a type id on the wire; schema type parameters
(`TypeRef::Param`, a `GenericFnDef`) for v1.x; a Swift protocol per generic function; Kotlin `reified`
dispatch; a TypeScript mapped registry; distinct native names; described type sets (the likely next step);
native generic classes; instantiation lists on impl blocks; trait-constant metadata with a generic dispatcher
at the template; identifier-safe schema names; arbitrary wire types in a function's list. Reasons in ADR-058,
"Alternatives considered".

## Decisions for the integrator, each with my recommendation

1. **Swift presentation of a generic function: overloads, or one generic over a generated protocol.**
   Recommend **overloads** (with `_ type: Todo.Type` when `T` is not in the parameters). They need nothing in
   the schema beyond the label, match Foundation's closed sets (`KeyedDecodingContainer.decode(_:forKey:)`),
   and concrete call sites stay source-compatible if a later ADR generates `func newest<T: Dated>`. Cost: no
   generic callers on the platform, and Swift 6.3's bad diagnostic for an array literal. The protocol form has
   the cleaner diagnostics but needs the generic signature in the schema and a protocol name nobody chose.
2. **Kotlin's type token: `KClass` (`draft(Todo::class)`), the companion (`draft(Todo)`), or `reified`
   (`draft<Todo>()`).** Recommend **`KClass`**: it reads as "the type" and mirrors Swift's `Todo.self`; the
   companion form is shorter but leans on the codec companion; `reified` cannot fail at compile time for an
   un-instantiated type.
3. **TypeScript: a leading string literal (`newest("Todo", rows)`), always.** Recommend it. The alternatives
   are the codec as a token (`newest(TodoCodec, rows)`, structural, so not exact) or distinct names.
4. **The schema name of an instantiation: `newest<Todo>`.** Recommend it over an identifier-safe
   `newest_todo`: it cannot collide with a user's function and reads right in devtools, panic reports and bad
   requests. Generators take native names from the label.
5. **The label is part of the canonical form (and so of the hash) when set.** Recommend yes: the generated
   API depends on it, and it is derived from the name the hash already covers.
6. **The scope rule for aliases of generic objects and stores**: the template's signature types must be in
   scope where the alias stands (`rustc` reports and offers the import when they are not). Recommend
   **accepting it for v1.x** with the guide saying "declare the alias next to the template". The fix is a
   dispatcher generated at the template (generic code), which also lifts "no template from another crate";
   that is a second design, not a tweak. If the rule is unacceptable, the fallback is an instantiation list
   on the impl block (no aliases for objects), which I recommend against (ADR-058, Alternatives).
7. **v1.x scope.** Recommend as written: one type parameter per generic function; named value types in its
   list; generic methods of ordinary objects in; generic methods inside generic objects out; generic
   applications named through the alias (ADR-058 section 4) in. Section 4 is the one part that can be cut
   without touching the rest (the price: `fn page_of<T>() -> Page<T>` stays E0002).
8. **Numbers**: ADR-058, E0072 and E0074, S34. Confirm none is claimed by a branch in flight (I found none on
   the local branches; E0072 and E0074 to E0079 were unused on `main`).
9. **The public wording once it ships**: matrix row 22, the post's "not built yet" item, the roadmap's "now"
   entry and the README. Recommend "generic functions, methods, objects and stores cross as instantiations the
   core declares; the platforms call them with concrete types" and keeping, under what the matrix does not
   show, that there are no open-ended generics, no native generic types and no generic queries, ports or
   callbacks. Row 22 becomes "yes, declared instantiations", not a bare "yes".
10. **Three pieces, in order**: `generics-meta-macros`, `generics-bindgen`, `generics-playground` (ADR-058,
    brief). They own `undra-meta`, `undra-macros` and `undra-bindgen` while they run; piece A also adds
    `generic: None` to struct literals in `undra-ports`, `undra-cli` and the runtime's tests. One local
    branch in flight touches `undra-bindgen` (`claude/objective-hodgkin-5c81ca`, placeholders of recursive
    enums, 3 commits): land or park it before piece B. Recommend a
    full adversarial review of piece A (macro hygiene, the hand-over, the hash and validation rules) and a
    focused one of B and C.
11. **Keep the prototype patch in the repository** as a spec artifact until piece A merges, then delete it.
    Recommend yes: it is the fastest way for the implementer to see the mechanism run.
12. **Toolchain**: trybuild goldens must be recorded on the pinned 1.98.1; this machine has 1.99, on which six
    existing goldens already differ. The implementer needs the pinned toolchain or CI to bless them.

## Plan

ADR-058's brief: A (meta, then functions and methods, then objects and stores, then section 4, tests, SPEC),
B (model, validation, three emitters, two golden cases), C (playground `selection.rs` and one use per
platform, S34, one bench row, sizes, the guide), then the integrator flips the post, roadmap and README.
