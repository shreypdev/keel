# ADR-058: generic functions, methods, objects and stores cross as declared instantiations

Status: **Accepted** (implemented 2026-10-02, `wt/generics-fn-obj`; the deviations are at the end) (proposed the same day; the open half of ADR-042, catalogue row 22, the post's
"not built yet" item "Generic functions and objects"). Touches SPEC 1.1 (the name an instantiation's id is
computed from), 2.2 (`GenericOf` on `FunctionDef` and `MethodDef`), 2.3 (written only when set), 4.1 and 4.3
(three new accepted shapes), 10.3 (a new subsection of generated shapes), 12 (E0002 and E0070 texts, E0072 and
E0074 new) and 16.3; `undra-meta`, `undra-macros`, `undra-bindgen`, the playground, the contract tests and the
docs. **Wire: no change. C ABI and wasm ABI: no change. Runtime and the three platform runtimes: no change.
Schema: one optional label, written only when set, so no existing schema's hash moves.** Constitution R1 (every
instantiation is a described function or object), R3 (the generated shapes), R7 (hashes), R8 (the refusals) and
R11 (generated public shapes change), so it is decided here before code.

## Context

ADR-042 made generic **data** cross: `#[undra::api(generic)]` on a struct or enum is a template that registers
nothing, and `#[undra::api] pub type TodoPage = Page<Todo>;` registers the instantiation as a plain record under
the alias's name. Everything else that has a type parameter is still E0002:

* a function or method (`crates/undra-macros/src/impl_/object.rs:371`, `analyze` calls `check_generics`), an
  impl block (`:1788`, and `self_type_name` at `:1748` for `impl Cache<Todo>`), a store struct
  (`store.rs:406`), with the text "objects, stores, functions, methods, ports, callbacks, queries and mutations
  are dispatched by id: one dispatcher per instantiation, and a schema that can name a type parameter, would be
  needed" (`common.rs:33`);
* the guide says so (`site/docs/types.html`, "Generic objects, stores, functions, methods, ports, callbacks and
  queries stay out"), the README lists it under "What is not done" (`README.md:231`), the roadmap has it in
  "now" (`site/data/roadmap.json:199`) and the post's matrix row 22 is "partial".

The competitive catalogue (`.10x/specs/2026-10-01-competitive-limitations.md`) is why it matters: KMP-1 and
KMP-6 (Swift export "erases generic type parameters to their upper bounds"; Objective-C generics "can only be
defined on classes, not on interfaces … or functions"), UniFFI documents no generic types of your own (row 22,
[U13]), and KMP-B8 lists "generic classes" among what a Kotlin team shares today.

What apps need is narrow. Reading the playground, the cookbook and the catalogue's examples, the generic code a
core wants to export is (1) helper functions over the records it already describes (`newest`, `page_of`,
`draft`), and (2) a store or an object written once and used for several row types (`Selection<T>`,
`Editor<T>`, `Cache<K, V>`). Nobody needs the platform to invent a new instantiation: the callers are on the
platforms, where the core cannot see them, and every type that crosses is described in the schema anyway.

Three facts decide the design:

1. **A macro sees tokens, not types**, and the schema is built from spellings checked afterwards by the compiler
   (`check.rs`, E0060/E0061). ADR-042's answer, "run the ordinary expansion on concrete tokens under a name",
   needs nothing new in the type mapper.
2. **A dispatcher is per concrete Rust type.** The object table downcasts to the Rust type
   (`Runtime::object::<T>`), `UndraObject::TYPE_ID` is a constant of one impl, and a store's restorer is
   registered by type id. `Selection<Todo>` and `Selection<Note>` are two Rust types, so they are two objects.
3. **Every schema consumer reads concrete definitions**: the three generators, the runtime's dispatch tables,
   the devtools page (`runtimes/ts/devtools/src/schema.ts`, methods by id), the testkit's recordings (ids as
   numbers), the persisted-state closures of ADR-037. A schema with type parameters would change all of them.

## Decision

### 1. A generic function is monomorphised where it is declared

1. **Spelling.** The author lists the types a type parameter is instantiated with:

   ```rust
   /// What the helpers below need of a row. Plain Rust: the schema never sees it.
   pub trait Row: SignalValue {
       fn id(&self) -> Uuid;
       fn changed(&self) -> Timestamp;
   }
   impl Row for Todo { .. }
   impl Row for Note { .. }

   /// The row that changed last, if there is one.
   #[undra::api(generic(T = [Todo, Note]))]
   pub fn newest<T: Row>(rows: Vec<T>) -> Option<T> { rows.into_iter().max_by_key(Row::changed) }

   /// An empty row with a fresh id, ready to edit.
   #[undra::api(generic(T = [Todo, Note]))]
   pub fn draft<T: Row + Default>(ctx: &Ctx) -> T { .. }
   ```

   A method of an object or store (in its `#[undra::api] impl` block) takes the same list as a helper
   attribute, because `#[undra::api]` on a method is already E0007:
   `#[undra(generic(T = [Todo, Note]))] pub fn pinned<T: Row>(&self) -> Vec<T>`.
   The bound is the author's: an entry that does not meet it is `rustc`'s "the trait bound `Tag: Row` is not
   satisfied", pointed at the entry in the list (prototype).
2. **Expansion.** The function is emitted as written. For each listed type the macro substitutes the type for
   the parameter in the signature (type positions only, the `Substitute` visitor of `generic.rs`) and runs the
   **ordinary function expansion on the concrete signature**: the dispatcher (which calls `newest::<Todo>(..)`),
   the `FunctionMeta`, the registration and the identity checks. Every rule of a hand-written function applies
   to every instantiation, by construction: `HashMap<T, u32>` with `T = Todo` is the same E0006 a hand-written
   `HashMap<Todo, u32>` gets, and `Arc<T>` the same E0064 as `Arc<Todo>`.
3. **What the schema records.** One `FunctionDef` (or `MethodDef`) per instantiation, with a label:

   ```rust
   pub struct GenericOf { pub of: String, pub args: Vec<GenericArg> }
   pub struct GenericArg { pub param: String, pub ty: TypeRef, pub inferred: bool }
   // FunctionDef.generic, MethodDef.generic: Option<GenericOf>, serialized only when set
   ```

   * `name` is `<of><<args>>`: `newest<Todo>`. It is what devtools, a panic report's `operation`, a bad
     request's reason and a recording's reader show, and it cannot collide with an identifier.
   * `method_id` is the ordinary formula over that name: `fnv1a32("fn.newest<Todo>")`,
     `fnv1a32("Library.pinned<Todo>")` (SPEC 1.1).
   * `of` is the function's own name (`newest`), `param` the type parameter (`T`), `ty` the argument
     (`Named("Todo")`), and `inferred` says whether the parameter stands in the type of at least one
     parameter of the function, so a caller's arguments fix it (`newest`: true; `draft`: false).

   ```json
   {"name":"newest<Todo>","method_id":3111402365,
    "params":[{"name":"rows","ty":{"kind":"vec","of":{"kind":"named","of":"Todo"}}}],
    "returns":{"kind":"option","of":{"kind":"named","of":"Todo"}},"is_async":false,"takes_ctx":false,
    "generic":{"of":"newest","args":[{"param":"T","ty":{"kind":"named","of":"Todo"},"inferred":true}]}}
   ```

   The label is what lets a generator put the instantiations back under one native name. It carries no type
   parameter: the schema still describes concrete types only.
4. **Wire.** Nothing new: an instantiation is called with its own method id (call target 0 or 1, SPEC 3.3) and
   answers like any function. No type id travels, no table is consulted. A method id the core does not have
   is status 5, as today.
5. **Hashes (R7).** The canonical form writes `generic` only when set, so every existing schema hashes as
   before. Functions and methods are sorted by name in the canonical form, so **adding an instantiation moves
   the hash and reordering the list does not**. The ids of the instantiations that were already there do not
   change when one is added or removed.
6. **Limits in v1.x.**
   * **One type parameter per function or method.** Two are E0072 (a list of pairs is the lift).
   * **A listed type is a named value type**: a record, an enum, an error, a newtype or a named instantiation
     (`TodoPage`), written as a path without arguments. A scalar, `String`, a leaf type, `Vec<..>`,
     `Option<..>`, a map or an object is E0072: each instantiation is presented on the platforms under the
     name of its type (decision 2), and those have no one name in three languages.
   * Lifetimes stay E0003, const parameters E0002. A `where` clause and any bound are fine (the function is
     emitted as written and called through `newest::<Todo>`).
   * A type parameter with no list is E0002, whose text now teaches the list. A constructor, a query, a
     mutation, a port, a callback trait and an error enum stay E0002 (section 6).
7. **Schema validation (R1).** `undra-meta` checks a schema it is handed, whoever made it: a `generic` label
   names an identifier and a type parameter, its `name` is `of<args>`, its arguments are named value types,
   and the definitions that share one `of` (the functions of the schema, or the methods of one object) agree
   on the parameter names of the label and their `inferred` flags, on the number and names of their
   parameters, on `is_async` and `takes_ctx`, and on the shape of their types with the names erased. A label
   on a constructor or a port method is refused. Each failure is E0072.

### 2. How each language presents a generic function (R3)

The instantiations are a **closed set of concrete signatures**. Each language shows a closed set the way its
own libraries do, and none adds a hop: the generated code of an instantiation is the generated code of a
hand-written function, called with its own id.

| | when `T` is fixed by the arguments | when it is not (return only) | precedent |
|---|---|---|---|
| Swift | overloads | overloads with a leading `_ type: Todo.Type` | `UserDefaults.set(_:forKey:)`; `KeyedDecodingContainer.decode(_ type: Bool.Type, forKey:)` |
| Kotlin | overloads, each with `@JvmName` (erasure) | overloads with a leading `type: KClass<Todo>` | the standard library's `sumOf` overloads |
| TypeScript | overload signatures with a leading literal `type: "Todo"`, always | the same | `@types/node`'s `server.on(event: "close", ..)` overloads |

```swift
/// The row that changed last, if there is one.
/// - Throws: ``UndraCallError`` if the call fails in the core or cannot reach it.
public func newest(rows: [Todo], ctx: UndraCore = UndraPlaygroundCore.core) throws -> Todo? {
    var w = UndraWriter()
    rows.undraEncode(&w)
    do {
        let body = try ctx.callSync(
            .freeFunction(methodId: UndraIds.Functions.newestTodo),
            method: UndraIds.Functions.newestTodo,
            args: w.finish()
        )
        return try Optional<Todo>.undraDecoded(from: body)
    } catch {
        throw UndraCallError.mapped(error)
    }
}
public func newest(rows: [Note], ctx: UndraCore = UndraPlaygroundCore.core) throws -> Note? { .. }

/// An empty row with a fresh id, ready to edit.
public func draft(_ type: Todo.Type, ctx: UndraCore = UndraPlaygroundCore.core) throws -> Todo { .. }
public func draft(_ type: Note.Type, ctx: UndraCore = UndraPlaygroundCore.core) throws -> Note { .. }

let latest = try newest(rows: todos)          // Todo?
let blank = try draft(Note.self)              // Note
```

```kotlin
/** The row that changed last, if there is one. */
@JvmName("newestTodo")
fun newest(rows: List<Todo>, ctx: UndraCore = UndraPlaygroundCore.core): Todo? { .. }

@JvmName("newestNote")
fun newest(rows: List<Note>, ctx: UndraCore = UndraPlaygroundCore.core): Note? { .. }

/** An empty row with a fresh id, ready to edit. */
@JvmName("draftTodo")
fun draft(type: KClass<Todo>, ctx: UndraCore = UndraPlaygroundCore.core): Todo { .. }

@JvmName("draftNote")
fun draft(type: KClass<Note>, ctx: UndraCore = UndraPlaygroundCore.core): Note { .. }

val latest = newest(todos)                    // Todo?
val blank = draft(Note::class)                // Note
```

```ts
/** The row that changed last, if there is one. */
export function newest(type: "Todo", rows: Todo[], core?: UndraCore): Promise<Todo | null>;
export function newest(type: "Note", rows: Note[], core?: UndraCore): Promise<Note | null>;
export function newest(
  type: "Todo" | "Note",
  rows: Todo[] | Note[],
  core: UndraCore = UndraPlaygroundCore.core,
): Promise<unknown> {
  switch (type) {
    case "Todo":
      return newestTodo(rows as Todo[], core);   // not exported: the ordinary generated function
    case "Note":
      return newestNote(rows as Note[], core);
    default:
      return Promise.reject(new UndraCallError.Refused(`newest is not declared for ${String(type)}`));
  }
}
export function draft(type: "Todo", core?: UndraCore): Promise<Todo>;
export function draft(type: "Note", core?: UndraCore): Promise<Note>;

const latest = await newest("Todo", todos);   // Todo | null
const blank = await draft("Note");            // Note
```

Rules the generators follow:

1. The native name is `of` in the language's case; the id constant is `of` and the argument's name together
   (`UndraIds.Functions.newestTodo`, `NEWEST_TODO`). A family whose name, or one of whose id names, collides
   with another function after case conversion is E0051, as any collision is.
2. **The type token** (Swift `_ type: Todo.Type`, Kotlin `type: KClass<Todo>`) is the first parameter and is
   never encoded. It is generated when `inferred` is false, which is a fact of the generic function and not
   of the set of instantiations, so adding an instantiation never changes an existing signature. A parameter
   of the function that is itself called `type` renames the token with the generators' usual `avoid`.
3. **TypeScript always takes the literal first**: it has no runtime types, so `Todo[]` and `Note[]` cannot be
   told apart (an empty list is both). The literal is the type's name. The exported function is the overload
   signatures and one implementation that switches on the literal; a method of a class is the same with
   `private` per-instantiation methods. The `default` branch is reachable only by a caller that bypassed the
   types (plain JavaScript, a cast): it fails the way a refused call of that kind of function fails (a
   rejected promise with `UndraCallError.Refused`, a synchronous throw of it for a stream, a report through
   `core.report` for a command).
4. **Kotlin's `@JvmName`** is on every instantiation, also where the JVM signatures would differ without it:
   the output does not depend on which types erase alike.
5. Docs, failures (ADR-032), commands, streams, `async`, object and callback parameters: each instantiation is
   generated by the existing `callable` emitter and behaves as a hand-written function does.

**The error for a type that is not instantiated** is the compiler's, in all three languages (measured with
Swift 6.3.3, kotlinc 2.4.20, tsc 5.9.3):

* Swift: `no exact matches in call to global function 'draft'`, with one note per candidate
  (`candidate expects value of type 'Todo.Type' for parameter #1 (got 'Draft.Type')`). One caveat, found by
  the prototype: with an **array literal** argument of the wrong element type (`newest(rows: [Draft(..)])`)
  Swift 6.3 prints "failed to produce diagnostic for expression"; with a variable it prints the message
  above. Recorded in Risks.
* Kotlin: `None of the following candidates is applicable`, listing each overload with
  `Argument type mismatch: actual type is 'KClass<Draft>', but 'KClass<Todo>' was expected`.
* TypeScript: `No overload matches this call`, listing each overload with
  `Argument of type '"Draft"' is not assignable to parameter of type '"Todo"'`.
* A raw-API caller that sends an id the core does not have gets status 5, `UndraCallError.refused`.

An argument that does not carry its type (`nil`, an empty literal) needs an annotation, as with any overload.

### 3. A generic object or store is a template, instantiated by an alias

1. **Spelling.** ADR-042's, extended to impl blocks and store structs:

   ```rust
   /// The rows the user has ticked.
   #[undra::store(generic, restore = "Self::assemble")]
   pub struct Selection<T> {
       /// The ticked rows, in the order they were ticked.
       #[undra(key = "id")]
       rows: Signal<Vec<T>>,
       count: Computed<u32>,
   }

   #[undra::api(store, generic)]
   impl<T: Row> Selection<T> {
       pub fn new(ctx: Ctx) -> Self { Self::assemble(ctx, Signal::new(Vec::new())) }
       fn assemble(_ctx: Ctx, rows: Signal<Vec<T>>) -> Self {
           let count = Computed::new(&rows, |rows: &Vec<T>| rows.len() as u32);
           Self { rows, count }
       }
       /// Ticks a row, or unticks it.
       pub fn toggle(&self, row: T) { .. }
       pub fn contains(&self, id: Uuid) -> bool { .. }
       pub fn clear(&self) { self.rows.clear(); }
   }

   /// The ticked todos.
   #[undra::api] pub type TodoSelection = Selection<Todo>;
   /// The ticked notes.
   #[undra::api] pub type NoteSelection = Selection<Note>;
   ```

   A plain object has no attribute on its struct, so `#[undra::api(generic)] impl<K, V> Cache<K, V> { .. }`
   and `#[undra::api] pub type UserCache = Cache<UserId, User>;` are all it takes.
2. **The template registers nothing.** It has no type id, no dispatcher and no place in the schema. The impl
   block is emitted as written (a store's struct literals patched with the hidden cell, as today) together
   with a hidden `macro_rules!` that carries the block's **public signatures**, bodies emptied, each type
   parameter replaced by a metavariable, and the type itself applied to its own parameters (`Selection<T>`)
   replaced by the alias. A store's template also carries the struct's fields.
3. **The instantiation is the ordinary expansion on concrete tokens, under the alias's name.** The alias calls
   the template; `undra::__instantiate!` runs the existing object expansion (and, for a store, the existing
   store expansion) with the alias as the self type, without emitting the items: `impl UndraObject for
   TodoSelection` (`TYPE_ID = fnv1a32("TodoSelection")`, `NAME = "TodoSelection"`), the dispatcher over
   `rt.object::<TodoSelection>(handle)` with method ids `fnv1a32("TodoSelection.toggle")`, the `ObjectMeta`,
   and for a store the inherent members a store has today (`__UNDRA_STORE_META` with `Vec<Todo>` in it, the
   keyed list's key function over `&Todo`, attach, restore) and its `StoreRestorer`. The schema gets an
   `ObjectDef` named `TodoSelection` that is **byte for byte what a hand-written `TodoSelection` store
   produces**, and no label: nothing consumes one. Its docs are the alias's, else the template's (a store's:
   the alias's or the struct's, then the block's, as a store's are today).
4. **How a store's two halves meet.** A plain object's template is exported by its impl block's macro alone.
   A store's struct has a macro of its own, the two cannot see each other, and one exported template must
   hold both. The struct's macro defines a local `macro_rules!`; the impl block's
   macro, which must stand below the struct in the same module, calls it with its signatures; that macro
   calls the hidden `undra::__compose_store!`, which emits the one template, re-exported under the type's
   name as ADR-042's is (`use model::Selection;` brings the type and its template). The local macro is named
   after the rule, so a block that is not below its struct is reported by `rustc` as "cannot find macro
   `_undra_error_E0011_Selection_is_not_a_generic_store_declared_above_this_impl_block_in_the_same_module`".
5. **Rules.**
   * The block is for the type with its own parameters, each exactly once: `impl<K, V> Cache<K, V>`. Anything
     else (`impl<T> Cache<Vec<T>>`, `impl<T> Cache<T, Todo>`) is E0074. One block per type, as today (E0007).
   * Type parameters only: a lifetime is E0003, a const parameter E0002. Bounds and `where` clauses are the
     author's, on the struct and on the block: both are emitted as written, and an instantiation that does
     not meet them is `rustc`'s "the trait bound `Tag: Row` was not satisfied", at the alias.
   * Type arguments of the alias are **whatever the positions they land in accept**: the instantiation is
     checked exactly as if it had been written by hand. `Cache<String, User>`, `Cache<UserId, Price>`, a
     `Registry<Mailbox>` whose methods return `Option<Arc<T>>`, a `Signal<HashMap<K, V>>` whose `K` must be a
     map key (E0006 at the alias when it is not).
   * A method with a list of its own inside a generic block is E0074 (one level of generics per item).
   * The alias lives in the crate of its template, once per instantiation (E0070, whose text now says "a
     generic type"): the instantiation implements `UndraObject` for the template's type, which only that
     crate may do. A name another type has is E0050.
   * **Names are resolved where the alias stands.** The instantiation's dispatcher names the types of the
     template's signatures (and a store's signal types), and a `macro_rules!` resolves names where it is
     invoked. Next to the template this is invisible. In another module the template's types must be in
     scope; when one is not, `rustc` says "cannot find type `Order` in this scope" at the signature, "in this
     attribute macro expansion" at the alias, and offers the `use` (prototype). The guide says: declare the
     alias next to the template.
   * Elsewhere, signatures spell the alias, as for data: `Arc<TodoSelection>`, `&TodoCache` (the
     instantiation carries `__UNDRA_OBJECT_ID`, so E0061 passes; prototype).
   * A type parameter that stands in a signal needs the bound the signal needs: `undra::signals::SignalValue`
     (`Encode + Clone + Send + Sync + 'static`), which the prelude now exports and the example's `Row` has as
     a supertrait. Without it `rustc` reports the unmet `Encode` bound at the first use of the signal and
     suggests restricting `T` (prototype).
6. **Platforms: one class per alias, nothing generic.** `TodoSelection` is generated exactly as a
   hand-written store is, so no generator, runtime or mirror changes:

   ```swift
   /// The ticked todos.
   @MainActor @Observable public final class TodoSelection: UndraStore, @unchecked Sendable {
       /// The ticked rows, in the order they were ticked.
       public private(set) var rows: [Todo]
       public private(set) var count: UInt32
       public init(ctx: UndraCore = UndraPlaygroundCore.core) throws
       /// Ticks a row, or unticks it.
       public func toggle(row: Todo)                       // a command
       public func contains(id: UUID) throws -> Bool
       public func clear()
   }
   ```

   ```kotlin
   /** The ticked todos. */
   class TodoSelection internal constructor(core: UndraCore, handle: Long) : UndraStore(core, handle) {
       val rows: StateFlow<List<Todo>>
       val count: StateFlow<UInt>
       fun toggle(row: Todo)
       fun contains(id: UUID): Boolean
       fun clear()
       companion object { operator fun invoke(ctx: UndraCore = UndraPlaygroundCore.core): TodoSelection }
   }
   ```

   ```ts
   /** The ticked todos. */
   export class TodoSelection extends UndraStore {
     static create(core?: UndraCore): Promise<TodoSelection>;
     readonly rows: Signal<Todo[]>;
     readonly count: Signal<number>;
     toggle(row: Todo): Promise<void>;
     contains(id: string): Promise<boolean>;
     clear(): Promise<void>;
   }
   ```

7. **What each part of an object does under instantiation.**

   | Part | Behaviour |
   |---|---|
   | constructors | of the alias: `construct(type_id("TodoSelection"), method_id("TodoSelection", "new"), ..)`; `Arc<Self>` singletons intern per instantiation |
   | methods mentioning `T` | the substituted type: `toggle(row: Todo)`; all ordinary rules and checks |
   | signals of `T` | `rows: Vec<Todo>` in the `StoreDef`; change-sets and mirrors as for any store |
   | keyed lists keyed by a field of `T` | `#[undra(key = "id")]`: the key function is generated per instantiation over `&Todo`; a row type without the field is E0008 at the alias ("names no field of `Tag`", prototype) |
   | `DerivedList<T>`, `Lazy<T>`, `Computed` | as in any store; a store with derived or computed fields names its `restore` hook on the template, written once, generic |
   | persistence (ADR-037) | one `StoreRestorer` and one fingerprint per instantiation, computed from its concrete signals (`TodoSelection` and `NoteSelection` differ; prototype); a snapshot lists the alias's type id; `#[undra::migrate(store = "TodoSelection", ..)]` names the alias; renaming an alias is removing a store type and adding one, as renaming any store is |
   | handles and interning (ADR-040) | one Rust type, one alias, one type id, one native class. The object table, `issue`, the ledger and `adopt` are untouched: every generated call site that returns the object names the alias's class statically, so there is no native type parameter to recover from a handle |
   | callbacks (ADR-041) | a method may take a concrete callback (`Arc<dyn TodoListener>`); a generic callback trait is refused (section 6) |

### 4. A generic type applied to a type parameter is named through its alias

`fn page_of<T: Row>(..) -> Page<T>` and a `Selection<T>` method returning `Page<T>` substitute to
`Page<Todo>`, which ADR-042 refuses in a signature because tokens cannot say which alias names it. Inside an
instantiation the compiler can: every instantiation gains an inherent name constant next to its id constant
(`UNDRA_TYPE_NAME: &'static str` beside `UNDRA_TYPE_ID` for data, `__UNDRA_OBJECT_NAME` beside
`__UNDRA_OBJECT_ID` for an object), and the schema type of a generic application that mentions a substituted
argument is `TypeRefMeta::Named(<Page<Todo>>::UNDRA_TYPE_NAME)`
(`TypeRefMeta::Object(<Selection<Todo>>::__UNDRA_OBJECT_NAME)` for `Arc<Selection<T>>`), a constant
expression the compiler evaluates (prototype). When no alias declares the instantiation, the fallback-trait
technique of `check.rs` turns the missing constant into E0002 naming the alias to declare. A hand-written,
non-generic signature still spells the alias (ADR-042 2.3), and a template that writes `Page<Todo>` with no
parameter in it is still E0002 at the template.

### 5. Interaction with what exists

* **`DerivedList<T>`, `Lazy<T>`, `Computed<T>`, keyed lists**: fields of a generic store, described per
  instantiation as today (section 3.7).
* **`Page<T>`-style templates of ADR-042**: section 4. **`undra::query::Page<T, C>` and infinite queries**: a
  query is never generic (section 6), so nothing changes.
* **`Option`/`Vec`/`HashMap` of instantiations**: `Vec<TodoPage>`, `Option<Arc<TodoSelection>>`,
  `HashMap<UserId, TodoPage>` are ordinary uses of named types.
* **Newtypes as `T`**: a named value type, so allowed in a function's list and as any argument of an alias; a
  newtype key makes `HashMap<K, V>` signatures valid (ADR-042's `MapKey`).
* **`Decimal` and the leaf types**: an argument of an alias (`Cache<String, Decimal>`); in a function's list
  they need a newtype (`Price`), by 1.6.
* **Objects as `T`**: an argument of an alias when the template uses the parameter as `Arc<T>` or `&T`; not in
  a function's list in v1.x.
* **Two cores (ADR-044)**: each core's schema holds the instantiations of the crates it links, and each
  generated package declares its own; nothing is shared at run time. A template in a shared crate can only be
  instantiated in that crate (E0070), as for data.
* **React Native**: the TypeScript surface, unchanged.
* **The testing kit**: recordings hold ids as numbers, and an instantiation's ids are ordinary. A recording
  belongs to one schema hash, so adding an instantiation is a new recording, as any schema change is.
* **Devtools**: schema-driven, so an instantiated store appears under its alias and a call under
  `newest<Todo>`; the page builds text nodes, never markup, so the angle brackets are safe.
* **`undra bindgen --check`, `schema.json`, the API reference**: the label is part of the exchange form.

### 6. What v1.x refuses, and what would lift each refusal

| Refused | Code | Why | What lifts it |
|---|---|---|---|
| a generic function with no list (open-ended `T`) | E0002 | the callers are on the platforms; the core must contain a dispatcher for every type it can be called with | nothing: this is the design |
| two type parameters on a function or method | E0072 | each listed type is one function; two parameters need pairs | a row syntax, `generic((A, B) = [(Todo, Tag), ..])`, and names for pairs on the platforms |
| a scalar, `String`, leaf, container or object in a function's list | E0072 | the instantiation is presented under the type's name | naming rules for unnamed types in three languages, and tests |
| a generic method inside a generic object | E0074 | an instantiation is one class; a second list multiplies it | tests only: the expansion composes |
| a generic constructor | E0002 | a constructor returns the object, whose type has no such parameter | a use case |
| generic queries and mutations | E0002 | a query is cached, persisted and invalidated under one name and key | per-instantiation keys and handle names; a new ADR |
| generic ports and callback traits (`Listener<T>`), generic error enums | E0002 | the platform implements a port once, by the trait's name; an error is thrown by name | named instantiations of traits (`type TodoListener = dyn Listener<Todo>`); a new ADR |
| a template instantiated in another crate | E0070 | the instantiation implements `UndraObject` for a type of the template's crate | dispatch generated at the template (generic), so the alias only registers |
| native generic types or one fused native generic function (`func newest<T: Dated>`, `Selection<Todo>` as a Swift generic class) | — | the schema would need type parameters and named type sets | a `templates` section in the schema and described type sets; the overloads of section 2 stay source-compatible with it |

### 7. Diagnostics (R8)

Every text has the four parts of SPEC 12, rendered `error[undra::E0072]: <what>`, `= note: <why>`,
`= help: <fix>` and the docs link of its code. New codes: **E0072** and **E0074** (E0072 and E0074 to E0079
were free; `.10x/specs/2026-10-01-boundary-surface-plan.md` §7). The first column of each row is the snippet
that triggers it.

| Code | Trigger | What | Why | Fix |
|---|---|---|---|---|
| E0002 | `#[undra::api] pub fn newest<T: Row>(rows: Vec<T>) -> Option<T>` | generic parameter `T` on `newest` has no list of types | the schema describes concrete functions and the platforms call them by id: a generic function crosses once for each type it is declared for, and its callers are on the platforms, where the core cannot see them | list the types the platforms may use: `#[undra::api(generic(T = [Todo, Note]))]` (on a method: `#[undra(generic(T = [Todo, Note]))]`); each becomes a function of its own, `newest<Todo>` and `newest<Note>` |
| E0002 | `#[undra::api] impl<T: Row> Selection<T> { .. }` | generic parameter `T` on the impl block of `Selection` | the schema describes concrete objects: a generic object crosses once per instantiation, each under a name of its own, which the platforms generate a class for | mark the block `#[undra::api(generic)]` (`#[undra::api(store, generic)]` for a store) and declare each instantiation: `#[undra::api] pub type TodoSelection = Selection<Todo>;` |
| E0002 | `#[undra::store] pub struct Selection<T> { .. }` | generic parameter `T` on the store `Selection` | the schema describes concrete objects: a generic store crosses once per instantiation, each under a name of its own, which the platforms generate a class for | write `#[undra::store(generic)]`, mark its impl block `#[undra::api(store, generic)]` and declare each instantiation: `#[undra::api] pub type TodoSelection = Selection<Todo>;` |
| E0002 | `#[undra::query(key = "rows")] async fn rows<T>(ctx: &Ctx) -> Result<Vec<T>, E>` | generic parameter `T` on the query `rows` | a query is cached, persisted and invalidated under its name and key, and the schema describes one result type for it; a list of types would make several queries that share one name | write one query per type (`todo_rows`, `note_rows`) and share the body in a generic Rust function they both call |
| E0002 | `#[undra::callback] pub trait Listener<T> { .. }` (and `#[undra::port]`) | generic parameter `T` on the callback `Listener` | the platform implements the trait by its name, once per instance, and nothing names an instantiation it could implement | declare one trait per type (`TodoListener`), or pass a record or an enum that covers the cases |
| E0002 | `pub fn create<T>(seed: T) -> Self` in an `#[undra::api] impl` | generic parameter `T` on the constructor `create` | a constructor returns the object, and the object has no such parameter: the platforms could not say which `create` they mean | take a concrete type or an enum of the cases; make the object generic if the parameter belongs to it |
| E0002 | `generic(T = [Todo])` on `fn first<T>(n: u32) -> Page<T>` with no alias of `Page<Todo>` | `Page<Todo>` has no name: `first<Todo>` returns it | the schema names every type, and an instantiated generic type is named by the alias that declares it; `Page<Todo>` is reached here by putting `Todo` in place of `T`, and no alias declares it | declare `#[undra::api] pub type TodoPage = Page<Todo>;` next to `Page` |
| E0072 | `generic(U = [Todo])` on `fn newest<T>(..)`; `generic(T = [Todo])` on a function without `T` | `generic(U = [..])` on `newest`, which has no type parameter `U` | the list says which types a type parameter of the function is instantiated with; `newest` declares `T` | write `generic(T = [Todo, Note])` |
| E0072 | `generic(T = [])` | the list of `T` on `newest` is empty | a generic function crosses once per listed type; with no type it would not cross at all | list at least one type, `generic(T = [Todo])`, or remove `#[undra::api]` if the function is not for the platforms |
| E0072 | `generic(T = [Todo, Todo])` | `Todo` is listed twice for `T` on `newest` | each listed type becomes one function, `newest<Todo>`, and two functions cannot share that name | remove one of them |
| E0072 | `generic(T = [Vec<Todo>])`, `[u32]`, `[String]`, `[Arc<Mailbox>]`, an object | `Vec<Todo>` cannot be listed for `T` on `newest` | each instantiation is presented under the name of its type (`draft(Todo.self)` in Swift, `draft(Todo::class)` in Kotlin, `draft("Todo")` in TypeScript), so a listed type is a value type declared with `#[undra::api]`: a record, an enum, a newtype or a named instantiation | list the element type and write `Vec<T>` in the signature, or give the type a name: `#[undra::api] pub struct Todos(pub Vec<Todo>);` |
| E0072 | `generic(A = [Todo], B = [Tag])` on `fn link<A, B>(..)` | `link` has two type parameters, `A` and `B` | a generic function crosses with one type parameter: each listed type is one function, and two parameters would need a list of pairs | keep one parameter generic and write the other type out, or declare one function per pair |
| E0072 | schema validation: a label that does not describe its definition | the generic function `newest<Todo>` is not `newest` with `T = Note` (or: the instantiations of `newest` disagree: `newest<Note>` has 2 parameters and `newest<Todo>` has 1 parameter) | the generators put the instantiations of one generic function under one name, so they must be the same function with one type replaced | regenerate the schema from the core (`undra build`); a schema is not edited by hand |
| E0074 | `#[undra::api(generic)] impl<T> Cache<Vec<T>> { .. }` | the impl block of the generic object `Cache` is for `Cache<Vec<T>>` | a generic object is instantiated by naming its type arguments (`pub type TodoCache = Cache<Todo>;`), so its block is for the type with its own parameters, each exactly once | write `impl<T> Cache<T>` and put `Vec<T>` in the methods' types |
| E0074 | `#[undra(generic(U = [Tag]))]` on a method of an `#[undra::api(generic)]` block | `generic(..)` on `convert`, a method of the generic object `Cache` | an instantiation of a generic object is one class on each platform; a list on a method would multiply every instantiation by it | make `U` a type parameter of `Cache` and name it in the alias, or write the method for a concrete type |
| E0070 | a second alias of one instantiation; an alias in another crate | (the existing constant, `_undra_error_E0070_this_instantiation_of_Selection_is_declared_twice_keep_one_alias_per_instantiation`, first in the instantiation's inherent impl; the existing text for another crate) | an instantiation adds impls to the template's type (its constants and, for an object, `UndraObject`), which only the crate that declares the template may do, once | keep one alias per instantiation, next to the template |
| E0011 | `#[undra::api(store, generic)]` whose struct is not a `#[undra::store(generic)]` above it in the module | `rustc`: cannot find macro `_undra_error_E0011_Selection_is_not_a_generic_store_declared_above_this_impl_block_in_the_same_module` | the block hands its signatures to the struct's macro, which is in scope only below the struct | move the block below the struct, in the same module, and write `#[undra::store(generic)]` on the struct |
| E0011 | `#[undra::store(generic)]` whose block is `#[undra::api(generic)]` without `store` | `Selection` is a `#[undra::store(generic)]` but its impl block is not marked as a store (a constant assertion at the alias, through a constant the generic struct defines) | the block of a store must say so, so its constructors attach the store's signals | write `#[undra::api(store, generic)]` on the impl block |
| E0008 | `#[undra::api(generic)] impl Cache { .. }`; `#[undra::store(generic)] struct Todos { .. }` | `generic` on `Cache`, which has no type parameters (the existing text) | `generic` marks the template of named instantiations, and a template needs a type parameter to be instantiated with | remove `generic`, or add the type parameter |
| E0008 | `#[undra::api(generic(T = [Todo]))]` on a struct, an enum or an impl block | `generic(..)` with a list on `Selection` | a list instantiates the type parameter of a function; a type is instantiated under a name, by an alias | write `generic` alone and declare `#[undra::api] pub type TodoSelection = Selection<Todo>;` |

Two cases stay in `rustc`'s words and are documented under their code on the errors page: an alias whose
template's types are not in scope (3.5; `rustc` offers the import), and an alias of a
`#[undra::store(generic)]` struct that has no generic store block at all ("cannot find macro `Selection` in
this scope … `Selection` is in scope, but it is a struct, not a macro", listed under E0011).

## Alternatives considered

| Alternative | Why not |
|---|---|
| **Infer the instantiations from call sites** | Not knowable: the callers are Swift, Kotlin and TypeScript, compiled after the core. |
| **Type-erased generics over a wire `Value`** | The schema could not say what a function takes or returns (R1), the generated signature would be `Any` (R3), and every call would pay a dynamic decode. |
| **One method id plus a type-id argument, with a decode table per instantiation** | A new wire construct for no gain: the core still needs one monomorphised body per type, and every schema consumer (the generators, devtools' decoder, the testkit, the dispatch tables) would have to learn that one definition is several. With one id per instantiation they all work unchanged. |
| **A `GenericFnDef` with a signature over `TypeRef::Param`**, to generate one native generic function | A second description of the same functions that must agree with the first, and a `TypeRef` variant every exhaustive match in four crates must handle, to feed a presentation v1.x does not generate. The label of 1.3 is forward-compatible with adding it. |
| **Swift: one generic function over a generated protocol** (`func newest<T: NewestItem>`) | Reads well and its diagnostics are clean (prototype), but the protocol needs a name the author never chose (one per function), invites a conformance that cannot work (the set is closed), and needs the generic signature in the schema. Overloads are how Foundation presents a closed set, and concrete call sites stay source-compatible if a later ADR fuses them. |
| **Kotlin: `inline fun <reified T> newest(..)` dispatching on `T::class`** | A type that is not instantiated compiles and fails at run time; overloads fail at compile time. A sealed-interface bound narrows it but still admits the interface itself. |
| **TypeScript: a mapped type over a registry** (`newest<K extends keyof NewestTypes>(type: K, rows: NewestTypes[K][])`) | Needs the generic signature (which positions are `T`); overload signatures need only the instantiations and give the same call sites. |
| **Distinct native names** (`newestTodo(rows)`) | Hides that the functions are one function; three more names per type for the reader to learn. |
| **Described type sets** (`#[undra::api(for(Todo, Note))] trait Dated`, functions bounded by it) | The best native names (a Swift protocol `Dated`), and the list is written once. But it is a new schema concept, the function macro must find the set through a second callback macro, and it decides nothing the inline list forbids. The natural next step if generic callers on the platforms are asked for. |
| **Native generic classes over per-instantiation handles** (`Selection<Todo>` in Swift) | The schema needs type parameters; Kotlin erases a class's parameter and cannot reify a constructor; TypeScript has no runtime type; `adopt` would have to recover the parameter from a type id. ADR-042 rejected the same for records. A class per alias is what a hand-written store is. |
| **An instantiation list on the impl block instead of aliases** | No template macro and no hand-over between two macros, but a store's list is written twice (struct and block), an instantiation has no place for its own docs or visibility, and data types would instantiate one way and objects another. |
| **Metadata from trait constants** (`T: Described`) and a generic dispatcher emitted at the template | Resolves names at the template and lifts E0070, but every record would implement a new trait, the dispatcher and the store's restore would be generated twice (generic and concrete), a keyed list's key function cannot read a field of a generic `T`, and the positional rules (E0063, E0006) would be re-implemented after substitution. The lift for 3.5's scope rule and for cross-crate templates, not v1.x. |
| **Identifier-safe schema names** (`newest_todo`) | Collides with a function a user may write, loses the case of the type, and reads worse in devtools and panic reports than `newest<Todo>`. Generators take native names from the label either way. |
| **Any wire type in a function's list** | Works in the macro for free, but `draft([Todo].self)`, `List::class` (erased) and `"Vec<Todo>"` are three different non-names. A newtype gives the type a name. |

## Consequences

* A core writes a helper or a store once and exports it for each type it lists. The post's row 22 and "not
  built yet" item, the roadmap's "now" entry and the README's open item can be restated (the wording is the
  integrator's, decision record item 9).
* No wire, ABI, runtime or platform-runtime change, and no change to any existing generated file: generic
  objects and stores need **no generator change at all**; generic functions add overloads to `objects` /
  `stores` files and ids to the id files.
* Existing schemas hash as before; a schema that uses a generic function gains labelled definitions.
* Each instantiation is compiled code: N dispatchers, N metas, N monomorphised bodies. A core that declares
  none pays for an `Option` per function and method meta (measured in the brief, item 11).
* The macros gain a third hidden proc macro (`__compose_store!`) and the instantiation path gains objects and
  stores; `__instantiate!` re-homes the spans of its input (Risks).
* Generic callers on the platforms (a SwiftUI view generic over the row type) cannot call an overload set
  generically. They write a protocol of their own over the generated overloads or classes, one conformance
  line per type, until a later ADR generates it.

## Risks

* **Hygiene.** The tokens of a template reach `__instantiate!` through one or two `macro_rules!` and carry
  different hygiene marks; the object expansion binds locals (`__r`, `__e`) with one span and uses them with
  another's, which failed in the prototype ("cannot find value `__r` in this scope") until every input token
  was re-homed (`span.resolved_at(Span::call_site())`, keeping its location). The brief makes this the first
  step of an object's or store's instantiation, with a test, and leaves the record path as it is (re-homing
  it changes two of ADR-042's goldens).
* **The two-macro hand-over** depends on textual macro scope (struct above block, same module) and on
  re-exporting a macro-expanded `macro_rules!` with a single-segment `use`, which ADR-042 already relies on.
  Proven on Rust 1.99; the MSRV (1.85) and the pinned CI toolchain must build the same test.
* **Names resolved at the alias** (3.5) is a rule users can trip over. Mitigation: the guide's layout, and
  `rustc` offering the import. Lift: dispatch at the template.
* **Swift's diagnostic for an array literal of an un-instantiated type** is a compiler failure message in
  Swift 6.3 (section 2). It is the failing case only; recorded so nobody reports it as ours.
* **Code size.** Every instantiation is a full dispatcher and, for a store, a full restore. A template with
  many methods instantiated many times grows the core linearly; the guide says so.
* **Template macro names** are exported at the crate root (`__undra_template_<Name>`): two templates with one
  name in one crate collide, as for data templates (ADR-042, Risks).
* **The web size gates** (ADR-052): the label adds a field to `FunctionMeta`/`MethodMeta` and to the schema's
  JSON writer in every core. Expected well under 1 KB gzipped against 3.5 KB of headroom; the brief measures
  it and stops if the gate is at risk.

## Prototype (2026-10-02, not landed)

`.10x/specs/2026-10-02-generics-fn-obj-prototype.patch` applied to `a309e9f` (1,364 lines: the macros and two
test files; deleted in the implementation's last commit, the implementation having superseded it). It is a proof, not the implementation: no diagnostics, no label, no bindgen. What it showed:

* **Proved**: a generic function with a list (sync, `async`, with `ctx`, `T` in the return only) registers one
  function per instantiation under `newest<Todo>` and dispatches to the generic body; a generic object and a
  generic store instantiate through an alias, in the template's module and in another one, with the existing
  object and store expansions run on the alias; a keyed list keyed by a field of `T` ships keyed patches;
  a store with a `Computed` and a `restore` hook restores through the restorer of the instantiation; the two
  instantiations have different fingerprints; `Arc<TodoCache>` and `&TodoCache` work as ADR-040 positions;
  `TypeRefMeta::Named(<Page<Todo>>::UNDRA_TYPE_NAME)` evaluates and the fallback gives `""` for a missing
  alias. 8 tests against the real runtime. The macro crate's unit and behaviour tests pass with the patch.
  Its trybuild suite: six goldens mismatch on this machine with or without the patch (a local 1.99 against
  goldens recorded on the pinned 1.98.1), and two more with it (`e0070_second_alias`,
  `e0001_bad_type_arguments`), because the patch re-homes the tokens of **every** instantiation and that
  adds a line to `rustc`'s rendering of a record instantiation's errors. The brief therefore re-homes only
  objects and stores.
* **Proved in a scratch crate**: the hand-over of 3.4 (struct macro, block macro, a composing proc macro, an
  alias reaching the template by `use` and by path), and its failure message.
* **Found**: the hygiene failure and its fix; that the self type of an instantiation can simply be the alias
  (so the expansions need no "generic self type" mode); that `Arc<Self>` is not a method return today, so the
  template replaces `Selection<T>` by the alias rather than relying on `Self`; the scope rule of 3.5; the
  `SignalValue` bound.
* **Killed**: `never[]` as the parameter type of the TypeScript implementation signature (TS2394; a union of
  the instantiations' types compiles); a `Selection<Todo>` self type in the instantiation (an alias in
  another module that spells the template by path cannot name it; the alias itself can).
* **Measured, not chosen**: a Swift generic over a generated protocol type-checks and its diagnostics are
  clean; it is not the v1.x shape for the reasons in Alternatives.
* **Not prototyped**: generic methods, section 4's mapper change, the label, the generators, and enforcing
  "the alias stands in the template's module" (a constant assertion on `module_path!()` would be evaluated
  after name resolution, so it could not replace the unresolved-name errors it is meant to explain, and it
  would refuse the aliases that do resolve).
* **Measured**: the native shapes of section 2 compile (`swiftc -typecheck` 6.3.3, `kotlinc -Werror` 2.4.20,
  `tsc --strict` 5.9.3) and the compilers' messages for a type that is not instantiated are the ones quoted.

## Implementation brief

Three pieces, in this order; each is one worktree, one review. Read first: this ADR, ADR-042 with its
deviations, SPEC 2, 4.1, 4.3, 10, 12, 16.3, and the prototype patch (apply it to a scratch branch to see the
mechanism run; do not start from its code where this brief says otherwise). Trybuild goldens are recorded with
the pinned CI toolchain (1.98.1), not a local 1.99.

**Piece A: `generics-meta-macros` (crates `undra-meta`, `undra-macros`, `undra`).**

1. `crates/undra-meta`: `GenericOf`, `GenericArg` (`def.rs`), `FunctionDef.generic` and `MethodDef.generic`
   (`Option`, `skip_serializing_if`), the `'static` mirrors (`meta.rs`: `GenericOfMeta`, `GenericArgMeta`,
   `Option<&'static GenericOfMeta>` on `FunctionMeta` and `MethodMeta`, the `From` impls), the canonical form
   (`canonical.rs`), `without_docs`, the validation of 1.7 (`validate.rs`, `SchemaError::BadGeneric`, code
   E0072, text in the four-part shape). The new field breaks every struct literal of the four types: add
   `generic: None` in `undra-macros` (`object.rs`, `port.rs`), `undra-ports/src/background.rs`,
   `undra-cli/src/schema.rs`, `undra-bindgen` (`src/model.rs`, `tests/common/mod.rs`), `undra-meta`'s
   fixtures and the tests of `undra-runtime` (`tests/common/mod.rs`, `sync_reply.rs`, `weak_ctx.rs`); no
   behaviour changes there. Tests: every existing golden hash unchanged (`crates/undra-bindgen/tests/
   schema_hash.rs`, the hash tests of `undra-meta`); a schema with two instantiations hashes the same in
   either order and differently with a third; each validation rule; JSON round trip.
2. `crates/undra-macros`, functions and methods: `generic(T = [..])` in `expand_api` for a `fn` and as a
   method helper attribute (`attrs.rs`); `expand_generic_fn` (the prototype's shape) and the same loop inside
   `expand_impl`; `FnModel` carries the call's type arguments; the label in `FunctionMeta`/`MethodMeta`
   (`inferred` = the parameter occurs in a type position of an input); arguments wrapped as `Type::Group` so
   `Checks`' `Scope::Instance` checks exactly them; a template pass over the generic signature
   (`Scope::Template`) that reports type errors once, at the function; every diagnostic raised
   while an instantiation expands names it ("in `newest<Todo>`"). E0002 texts, E0072.
3. `crates/undra-macros`, objects: `#[undra::api(generic)]` and `#[undra::api(store, generic)]` on impl
   blocks, `#[undra::store(generic)]`; `expand_impl_template`, `expand_store_template`, the local compose
   macro, `__compose_store!` (re-exported by `crates/undra/src/lib.rs` next to `__instantiate`),
   `object_template`; **positional** placeholders (the block's parameter at position *i* of the self type's
   arguments, so `impl<U> Selection<U>` composes with `struct Selection<T>`); the self type with its own
   parameters replaced by the alias; `__instantiate!`, for an object or a store only, re-homes spans first
   (ADR-042's record goldens must not change), then runs `expand_store_as` / `expand_impl_as` in instance
   mode with `Scope::Instance` checks; the E0070 constant first in the
   instantiation's inherent impl; `__UNDRA_IS_GENERIC_STORE` on the generic struct and its assertion in a
   plain object's instantiation; E0074; the header check; a template pass that reports type errors at the
   template. Add `SignalValue` to `undra::prelude`.
4. `crates/undra-macros`, section 4: `UNDRA_TYPE_NAME` / `__UNDRA_OBJECT_NAME` on every instantiation;
   `KType::NamedOf(Type)` / `ObjectOf(Type)` whose `meta()` reads the constant; the mapper accepts a generic
   application only when one of its arguments is a parameter (template pass) or a `Type::Group` (instance);
   the fallback assertion (E0002).
5. Tests for 2 to 4: the prototype's two files, renamed and completed (`tests/generic_functions.rs`,
   `tests/generic_objects.rs`: generic methods, streams, object and callback parameters in a template, a
   `Lazy<T>` and a `DerivedList<T>` store, `Cache<K, V>` with a map-keyed signal, an object as `T`, section 4
   both ways, a template whose struct and block use different parameter names, MSRV build); expansion
   snapshots (`src/tests/snapshots.rs`); one trybuild case per row of section 7 (`tests/ui/e0072_*.rs`,
   `e0074_*.rs`, the changed `e0002_*`, `e0070_second_alias_of_an_object.rs`, `e0011_generic_store_*`); the
   catalogue (`diag.rs` table, `tests/catalogue.rs`).
6. SPEC 1.1, 2.2, 2.3, 4.1, 4.3, 12, 16.3 in the same commits; `site/scripts/build-errors.mjs` regenerated.

**Piece B: `generics-bindgen` (crate `undra-bindgen`; after A's `undra-meta` commit).**

7. `model.rs`: families (definitions sharing `generic.of`, per list of functions and per object), the native
   name and the id name of a definition, whether a family takes a token. `validate.rs`: identifiers and case
   collisions use those names (`check_identifiers`, `check_top_level_values`, the method checks); E0072
   passes through (and joins the codes SPEC 12 lists under C0007). `swift.rs`, `kotlin.rs`, `ts.rs`: `Callable` gains the native name and the token; the id
   files use the id name; TypeScript's family emitter (overload signatures, the implementation, private
   per-instantiation functions or methods); Kotlin's `@JvmName` and `KClass` import.
8. Goldens: a new case `generic_functions` (a free family with `T` in the parameters, one with `T` in the
   return only, an `async` one with an error type, a stream, a command, a family of methods on an object and
   on a store, a parameter called `type`) and a new case `generic_objects` (the `ObjectDef`s two aliases of
   an object and of a store produce, to lock "no generator change"); type-checked by `typecheck_swift.rs`,
   compiled with `-Werror` by `typecheck_kotlin.rs`, `tsc` by `typecheck_ts.rs`, run by `run_ts.rs`; the
   iOS-floor case list if stores are in it; `tests/validate.rs` and `tests/golden/diagnostics` for the new
   E0051 and E0072 messages. SPEC 10.3d.

**Piece C: `generics-playground` (after A and B).**

9. `examples/playground/core/src/selection.rs`: the `Row` trait, `newest` and `draft` over `Todo` and `Note`,
   `Selection<T>` with `TodoSelection` and `NoteSelection`, a plain generic object (`Recent<T>`, the last
   rows opened) with one alias; one real use of each on the three platform apps (the todos and notes screens
   gain multi-select through their selection store; "New" uses `draft`; a "latest" line uses `newest`).
   Regenerate the bindings (`undra bindgen -C examples/playground --docs`).
10. Contract scenario **S34 "generic functions, objects and stores"** in `contract-tests/scenarios.md` and the
    three runners: (1) `newest` of three todos and of two notes returns the right row of the right type, and
    of an empty list nothing; (2) `draft` with the type token returns a row of that type with a fresh id;
    (3) `TodoSelection` and `NoteSelection` are two stores: toggling a todo delivers one keyed `Insert` to
    the todo selection's `rows` and nothing to the note selection, toggling it again one `Remove`, `count`
    follows; (4) snapshot, restore: both selections keep their rows and handles, and their type ids differ;
    (5) through the raw API, an id that names no instantiation (`fnv1a32("fn.newest<Draft>")`) is status 5,
    `UndraCallError.refused`; (6) `Recent<Todo>` returned from a
    function is one wrapper per handle (ADR-040).
11. Bench (`bench/benches`, `bench/budgets.toml`): `dispatch/call_sync/generic_fn` beside
    `dispatch/call_sync/function`, the same body, budget 1.1 times it (the claim "an instantiation dispatches like a hand-written function" as a test, R9). Sizes:
    `scripts/wasm-size.sh` before and after; the gates of ADR-052 hold or the piece stops and reports.
12. Docs: `site/docs/types.html` ("Generic data types" becomes "Generics": functions, objects, stores, the
    layout advice, the limits table of section 6), `site/docs/errors.html` regenerated, the cookbook's
    modelling page, `docs/TESTING.md` unchanged; a devtools unit test that a call named `newest<Todo>`
    renders as text. For the integrator at merge: `README.md` ("What is not done"), `site/data/roadmap.json`
    ("now" entry `Generic functions and objects`), the post (`site/blog/why-undra-is-the-default-choice/`:
    `claims.md` rows M03-N3, O34, T22 and the two passages of `index.html`), then `node
    site/scripts/build-all.mjs`.

Quality bar for each piece (R4): `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test
--workspace`, the three runtime suites and `contract-tests/run-all.sh` for C, docs on every `pub` item, no
`unsafe`. Regenerate, never hand-edit: `UPDATE_GOLDEN=1 cargo test -p undra-bindgen --test golden`,
`UPDATE_SNAPSHOTS=1 cargo test -p undra-macros --lib`, `TRYBUILD=overwrite cargo test -p undra-macros --test
compile_fail` (on the pinned toolchain), the diagnostics goldens and `node site/scripts/build-errors.mjs`
(`docs/AGENT_WORKFLOW.md`). Out of scope everywhere: any behaviour of `undra-runtime`, `undra-wire`,
`undra-ffi`, `undra-transport`, `undra-query` and the three platform runtimes (if a step seems to need one,
stop and report: the design says it does not), `.10x/status.md`, `.10x/handoff.md`.

## Dependencies

None to start. Piece B needs A's `undra-meta` commit; piece C needs both. The pieces own `undra-meta`,
`undra-macros` and `undra-bindgen` while they run, so no other worktree may touch those crates
(`docs/AGENT_WORKFLOW.md`, parallelism rules).

## Implementation (2026-10-02, `wt/generics-fn-obj`) and deviations

The three pieces of the brief landed on one branch, in this order, each with its own commits (the record:
`.10x/decisions/sde/generics-fn-obj.md`): **A** `undra-meta`, `undra-macros` and `undra` (the label, functions and methods,
objects and stores, the application of a type parameter, the diagnostics, the catalogue); **B** `undra-bindgen` (families, native and id
names, the three generators, the goldens `generic_functions` and `generic_objects`, the diagnostics goldens); **C** the playground
(`selection.rs`, the three apps, the regenerated bindings), contract scenario **S34**, the bench row, the guide
`site/docs/generics.html` and the wording updates. The prototype patch is deleted. Deviations, each dated 2026-10-02:

1. **The new bindgen cases are `generic_functions` and `generic_objects`**, not `generics` (which ADR-042's goldens already use). Both
   are in `schema_hash.rs`'s list of cases with no hash from before the field: the label is part of the canonical form only when set.
2. **`__instantiate!` also receives the template's type arguments as an item**, `type __UndraInstanceArgs = (Todo,);`, after the
   definitions. Its only use is naming the rule of E0070: a second alias of one instantiation in one module defines the same constant
   twice, and `rustc` reports that before anything else (so the macro's branded E0070 reaches the author first). The check of an alias
   in another crate is the inherent constant, as in ADR-042. A store's E0070 has a unit test and no trybuild case (its golden is 640
   lines of cascade).
3. **A template that fails to expand leaves a stub**: a `macro_rules!` that swallows its aliases (and a stub compose macro for a
   store), so the one error is not followed by one "cannot find macro" for every alias.
4. **E0074 also covers a method of a generic block with type parameters of its own and no list** (`fn map<U>(..)`), beside a list on
   such a method; the text names the block.
5. **E0072's text counts in the singular** ("has 1 parameter"), and the schema validation names the family's schema names (`newest<Todo>`,
   `newest<Note>`) when instantiations disagree in shape.
6. **Kotlin gives every overload a `@JvmName`** (the id name: `newestTodo`), not only the clashing ones, so a family reads the same
   whatever its parameters erase to; the generated files compile with `-Werror` under kotlinc 2.4.20 and CI's 2.0.21.
7. **Scenario S34 step 6**: `Recent<Todo>` is returned by a function, `recent_todos(rows, limit)`, that makes a fresh object each call,
   so "one wrapper per handle" is checked as: a `RecentTodos` is the alias's class, a second call is another wrapper with rows of its
   own, and closing one leaves the other. (Interning of one object returned twice is S27's.) S35 stays with `reload-handles`.
8. **The ratio gate is 1.2, not 1.1**: `generic_fn_vs_function` measured 0.89 to 1.04 over six runs on the reference host, and
   `budgets.toml`'s own rule (1.15 times the largest healthy sample, rounded up to 0.1) gives 1.2; two rows of about 40 ns have more
   noise than 1.1 on a shared runner. An extra dispatch hop would be 1.3 or more.
9. **The cookbook has no modelling page** to extend: the guide `generics.html` takes that role (and `types.html` gained the
   "Generics" section with the limits table). The default-choice post's tally follows row T22 (28 solved, 5 partial) and its `claims.md`
   rows M03-N3, O34 and T22, MI04 and section 11.
10. **Measured**: the web hello core grows from 116,023 to 117,215 bytes gzipped (+1,192, the label in the JSON writer and the mirrors;
    gate 120,000; both built with Rust 1.99.0 and wasm-opt 133 by the review, the merge base `a309e9f` against the branch: the
    implementation's own note said 116,550 and +665, a main measured another way) and the up-front JavaScript stays at 22,100
    (gate 22,100: no runtime changed). More than the "well under 1 KB" the Risks expected, inside the gate (2,785 bytes left). The Swift 6.3 diagnostic for an array
    literal of an un-instantiated type is in the guide.
11. **MSRV 1.85 was not built locally** (no such toolchain on the machine); the two-macro hand-over and the single-segment re-export are
    the constructs ADR-042 already relies on, and CI's pinned toolchain builds the tests.
