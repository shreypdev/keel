# Adversarial review: keel-macros (2026-09-30)

Scope: `crates/keel-macros/src/**` read in full, plus how the emitted code is used in keel-ports, keel-query, keel-runtime, keel-meta and keel-bindgen (checked with `validate`). Method: I wrote about 45 adversarial inputs in a scratch crate, `/private/tmp/kmr`, which depends on the workspace by path. `examples/d*.rs`, `e*.rs` and `f*.rs` hold the inputs and `out/*.txt` holds the compiler output. `tests/{schema,runtime,alias}.rs` cover the schema dump, the wire layout, dispatch edge cases and the alias checks. Baseline: `cargo test -p keel-macros --lib --tests` passes (138 unit tests plus the behaviour and trybuild tests).
Labels: **CONFIRMED** means I ran the input and saw the output quoted. **THEORY** means the claim comes from reading the code path.

## High

**H1. The schema can describe a different type than the one on the wire, and nothing catches it (R1).** CONFIRMED.
`types.rs:498-706` maps types by the *last path segment's spelling* only. The leaf names are matched at `:525-547`. Any other bare name becomes `Named(name)` at `:699`. No generated code checks that the Rust type behind the spelling is the type the schema names.
- Repro A (`f03_leaf_shadow.rs`): user record `Bytes { data: Vec<u8>, mime: String }` and `Doc { payload: Bytes, n: u32 }`.
  - Schema: `Doc.payload: bytes`. `keel_bindgen::validate`: **OK**.
  - Wire: `[2,0,0,0,1,2, 1,0,0,0,97, 9,0,0,0]`, which includes a `mime` field the schema does not have. Every platform decodes `n` from the wrong bytes.
  - `Uuid` is affected the same way. `Duration`, `Timestamp` and `String` are saved only because bindgen reserves those names.
- Repro B (`tests/alias.rs`): `mod legacy { use crate::v2::Item as Todo; #[keel::api] pub fn legacy_echo(t: Todo) -> Todo }`.
  - Schema: `legacy_echo(t: Todo) -> Todo`. This resolves to the real `crate::Todo`, so it validates.
  - A platform sending a schema `Todo` gets `BadRequest "cannot decode the arguments of legacy_echo: 10 trailing byte(s)"`.
  - The reply `[7]` does not decode as `Todo`.
  - A `type Todo = v2::Item;` alias does the same.
- Expected: a compile error in the user crate.
- Fix: for every schema type reference, emit a compile-time identity check at the reference's span.
  - For leaf types, emit `const _: fn(Ty) -> ::keel::wire::Bytes = |x| x;`, using the canonical type in each case (`core::time::Duration`, `keel::wire::Uuid`, and so on).
  - For `Named`, emit `const _: () = assert!(str_eq(<Ty as ::keel::meta::KeelNamed>::KEEL_NAME, "Todo"), "error[keel::E0001]: ..")`, where records, enums, errors and objects implement `KeelNamed`.
  - This also rejects `Handle` and the aliases in M3 at compile time.

**H2. Generated port proxies panic on a port outcome the SPEC defines. On wasm that aborts the core (R6).** CONFIRMED on native. The wasm consequence is THEORY via SPEC §7.
- `port.rs:458-476` and `:492-496` turn every `PortError` into `panic!`: `Unavailable`, `Decode`, and `Cancelled` when it is not a typed `Failed`.
- The panic fires even when the method returns `Result<T, E>`.
- SPEC §6.3 says "a port that is not registered behaves as 2 (unavailable)". SPEC §7 builds wasm with `panic=abort`.
- Repro (`f02_port_unavailable.rs`):
  - Setup: `#[keel::port] trait Kv2 { fn get(&self, k: String) -> Result<Option<String>, KvErr>; }` and `#[keel::api] fn read(ctx:&Ctx,k:String)->Result<..,KvErr>{ kv2(ctx).get(k) }`, with no binding.
  - Actual: the reply is `Panic "keel: port call `Kv2.get` failed: Unavailable"`.
  - Expected: a typed value. On web, any platform that does not register an optional port (SecureStore, Fs) traps the whole module.
- Fix:
  - For `Result<T, E>` port methods, require `E: From<keel::runtime::PortError>`. Use a teaching E0030-family error when it is missing, and map Unavailable, Decode and Cancelled into `Err`.
  - For infallible methods, either check at `Runtime::init` that they are bound, or mark ports optional in the schema.
  - Generated code should never panic.

## Medium

**M1. Any error on a `#[keel::store]` struct produces a false E0011 plus E0560.** CONFIRMED (`e01`, `e02`, `d10`, and my own fixture when I forgot `restore`).
- Repro: a store with a `Computed` field and no restore hook, next to its `#[keel::api(store)] impl`.
- Actual:
  1. `E0013`
  2. `E0560 struct Todos has no field named __keel_cell`
  3. `E0080 .. error[keel::E0011]: Todos is implemented with #[keel::api(store)] but the struct has no #[keel::store]`, which is false.
- Cause: the fallback in `mod.rs:36-55` re-emits the stripped struct without the hidden field or `__KEEL_IS_STORE`, so the probe at `object.rs:1159-1172` fires.
- This contradicts `lib.rs:23-25` ("no cascade"). The UI tests `e0013_store_restore` and `e0001_lazy_signal` omit the impl block, so this path is untested.
- Fix: on error, the store fallback should still append `__keel_cell` and emit `__KEEL_IS_STORE = true` plus stub `__keel_attach_all`, `__keel_set_handle` and `__KEEL_STORE_META`. Add UI tests that include the impl.

**M2. A store struct with no `#[keel::api(store)] impl` gets no E0011.** CONFIRMED (`d24`).
- Actual: four copies of `E0277 the trait bound S: KeelObject is not satisfied`, each with "help: the trait `KeelObject` is implemented for `LazyList`".
- SPEC §12 assigns this case to E0011. It is the first thing a user hits when writing the struct before the impl.
- Fix: the store emits a bound on a marker trait that the `api(store)` impl implements. Put `#[diagnostic::on_unimplemented(message = "error[keel::E0011]: ..", note = .., label = ..)]` on that trait (stable since 1.78, so within MSRV 1.85). Use the same attribute on `keel_wire::Encode`/`Decode` for M5.

**M3. The macro accepts shapes that bindgen then rejects.** The rejection comes late, has no span, and is never a macro error (R8). CONFIRMED (`f01`, `tests/alias.rs`, `tests/schema.rs`).
- `-> Result<u8, String>`: bindgen E0001, "E must be a #[keel::error] enum".
- A query returning `Result<(), E>` or `Result<Option<T>, E>`: bindgen E0001.
- An `Option<Option<..>>` record field: bindgen E0001, "cannot tell Some(None) from None". SPEC §2.1 says "`Option<Option<T>>` allowed", so the SPEC and bindgen contradict each other.
- `#[error("pair {} and {}")]`:
  - Rust `Display` works and prints "pair 1 and 2".
  - The schema stores the raw `{}`, because `error.rs:115-120` stores `lit.value()`, not the normalized template.
  - bindgen then fails with E0010, because `model.rs::parse_message` has no implicit positions.
- `Handle`: the prelude documents it as a "wire scalar a public signature may use", yet the macro emits `Named("Handle")`, which bindgen reports as an unresolved type. `type Id = u64` fails the same way.
- Fix: put one rule table in keel-meta and use it from both `types.rs` and bindgen. Write `{N}` into the schema message. Decide on `Handle`: `TypeRef` has no handle kind, so either reject it or add one (needs an ADR, R11). Settle the nested-Option rule in the SPEC.

**M4. Unit and empty braced structs are accepted as records, although SPEC §12 E0007 says to reject them.** CONFIRMED (`d01` compiles).
- `record.rs:129`: `Fields::Unit => Vec::new()`.
- The schema gets record `Marker` with `fields: []`. bindgen accepts it.
- `Holder { markers: Vec<Marker> }` decodes **1000** items from a 4-byte input. The only limit is `MAX_ZERO_SIZED_COUNT` = 65,536, and only in Rust.
- This is the same "zero-width items defeat length validation" case that SPEC §3.1 uses to reject `Vec<()>`.
- Fix: E0007 for both `struct X;` and `struct X {}`, or amend the SPEC and bound zero-width counts in every platform codec.

**M5. Common mistakes get rustc's generic errors (cryptic cascades), not Keel diagnostics.** CONFIRMED.
- `#[keel::api]` on a method inside an impl (`d04`):
  - Actual: six errors, starting with "associated `static` items are not allowed" and "`const` items in this context need a name".
  - Cause: `expand_fn` (`object.rs:1247`) never checks `analysis.has_receiver`.
- `#[keel::query]` on an associated fn (`d05`): eight errors, starting with "struct is not supported in `trait`s or `impl`s".
- Two `#[keel::api] impl Calc` blocks (`d03`):
  - Actual: five E0428/E0119 errors on `__KeelStoreProbe_Calc`, `__keel_dispatch_Calc` and `KeelObject`.
  - The one-block rule appears only in `lib.rs:45-46`.
- A method returning another `#[keel::api]` object (`d07`):
  - Actual: `E0277 Child: Encode`, with a list of "AppState, Arc<T> and 34 others".
  - bindgen already has the teaching text for this ("object handles cannot cross as values yet").
- A store with a non-`Default`, non-`Ctx` state field and no hook (`d08`): `E0277 Config: Default`, pointed at the attribute (`store.rs:401-421`).
- Fix:
  - Receiver check in `expand_fn` and query: "put `#[keel::api]` on the impl block" (E0007).
  - Rename the duplicate sentinel, for example `__keel_one_api_impl_block_per_type_Calc`, or support several blocks.
  - `on_unimplemented` on Encode/Decode naming the object-as-value rule.
  - A spanned `Default` probe per state field with an E0013-style note.

## Low

- **L1. Keyword accessor.** A port whose snake-case name is a keyword breaks expansion. `#[keel::port] pub trait Match` emits `pub fn match(..)`. rustc then says "expected identifier, found keyword `match`" and suggests the nonsense fix `r##[keel::port]` (`d02`, `port.rs:435`). The same happens for `Type`, `Loop`, `Move`, `Ref`, `Use`, `Gen`, `Box` and others. Fix: `Ident::new_raw` or a suffix.
- **L2. Double-underscore parameter names.** Generated locals share the user's hygiene context. Parameters named `__w` (port, `e04`), `__r` (fn, `e05`) or `__ctx` (constructor or query, `e06`) fail with type errors that point at generated locals. Ordinary names are safe; `edge_cases.rs` covers those and I confirmed a shadowing module (`e07`). Fix: `Span::mixed_site()` for generated locals.
- **L3. Raw syn errors for malformed helper values.** These produce syn errors without a Keel code:
  - `#[keel(default = true)]` → "expected `,`" (`d09`)
  - `#[keel(key = 5)]` → "expected string literal" (`d10`)
  - `retry = "3"` → "expected integer literal" (`d11`)
  - `#[keel::api(crate)]` → "expected `=`" (`d16`)
  - Locations: `attrs.rs:177,192` and `query.rs:111,134`. Wrap them as E0008/E0040.
- **L4. Parenthesised return types.** `map_type` unwraps `Type::Paren`, but `query.rs:478` (`result_arguments`) and `port.rs:690` (`result_types`) do not.
  - `-> (Result<u8, E>)` on a query gives "keel: internal error: a validated query has no Result type" (`d17`, `query.rs:309-314`).
  - On a port, the proxy would decode the reply as a `Result` while the Rust dispatcher writes status + `T` (THEORY).
- **L5. Misleading wording.**
  - `self: &Arc<Self>` is accepted (`object.rs:202-206`) and then fails with E0308 "expected `&Arc<C>`, found `&C`" (`d06`).
  - `self: Pin<&Self>` is reported as "self by value" (`d20`).
  - A `R<Self>` alias constructor is reported as "neither a method nor a constructor" (`d13`).
  - `Pin<Box<dyn Stream>>` gets the advice "declare a concrete #[keel::api] type" rather than "return impl Stream" (`d14`).
  - `-> Vec<Result<..>>` says "`Result` cannot be used as a return type" (`d27`).
  - `#[error]` on an `#[keel::api]` enum gives "cannot find attribute `error`" with no hint to use `#[keel::error]` (`d15`).
  - A `key` naming a missing field gives E0609 on the attribute, not on the literal (`d12`).
  - User types named `Lazy`, `Signal`, `Instant`, `Path` or `Cell` are rejected with the message meant for the std/keel type (`types.rs:627-698`).
- **L6. Docs.**
  - `ObjectDef.docs` is read from the impl block (`object.rs:993`), so a documented struct gets `docs: ""` (`Calc` in the fixture).
  - `#[doc = include_str!(..)]` and `concat!` docs are silently dropped (`attrs.rs:277-289`, THEORY).
  - A doc-only `#[cfg_attr(..)]` on a public method is rejected as E0008.
- **L7. SPEC and doc drift.**
  - §4.2 promises `impl From<E> for keel::Error`, but no `keel::Error` exists and nothing is emitted (`error.rs:320`: `_root` is unused).
  - The module doc at `object.rs:10-11` says bad requests answer `Unknown`; the code answers `BadRequest`.

## What holds (verified)

- **No proc-macro panic** across roughly 45 hostile inputs. Every `.expect()` is guarded by an earlier shape check. No emitted path hard-codes `::keel` outside `Root`: grep of `impl_/*.rs` finds hits only in tests, and keel-ports builds with `crate = "crate::root"`.
- **Ids match SPEC §1.1** for type, method, `fn.`, `port.`, port method, `query.` and `mutation.`, including the `QUERY_ID`/`MUTATION_ID` constants. `QueryRegistration` uses the same `Q::ID` as `QueryMeta`.
- **Wire layout.** Record fields, variant indices and signal ids follow declaration order. For `ctx, all, label, filter, count, tail`, signal ids come out as 0, 1, 2, 3 with `label` skipped. A `#[keel(default)]` field stays on the wire (exact bytes checked). A 20-field variant and deep `Option<Vec<Map<..>>>` nesting both round-trip. The attach order follows SPEC §16.1: `attach` / `attach_keyed` / `attach_computed` / `set_no_coalesce` with `?`, then insert, then `set_handle`. `KeyFn` is `fnv1a64` of the encoded key. Snapshot and restore round-trip.
- **Dispatch.** Truncated args, trailing bytes, unknown methods and stale handles all return status 5, with reasons that name ``Calc.add``. A fallible constructor returns status 1 with the typed error.
- **Hygiene** holds against a module that shadows `Result`, `Ok`, `Err`, `Option`, `Some`, `None`, `Box`, `Vec`, `Default`, `Runtime`, `Writer`, `Reader`, `Arc`, `Encode`, `Decode`, `Send`, `Sync`, `Signal`, `StoreCell`, `Stream`, `Future` and `Pin` (`e07` compiles).
- **Teaching diagnostics.** Every case in the brief's list below produces a code, what, why, help and docs link, underlined on the right tokens:
  - generics, lifetimes, `&mut self`, `self`
  - `dyn` and `Fn`
  - `Result` in a param or field, and `Stream` outside a return
  - `Lazy<T>`, struct map keys
  - a missing `#[error]`
  - async in a sync port, event ports returning a value or `async`
  - a query without `key`, a mutation with `stale` or `persist`
  - unknown `#[keel(..)]` options and macro arguments
  - `#[cfg]` on a field, tuple structs, empty enums, explicit discriminants
- **Performance.** The only quadratic step is the id-collision scan over methods (`object.rs:1093`, `port.rs:313`), which is negligible.

## Verdict

**Not sound for v1 as-is.** The dispatch, id and ordering machinery is solid, and the diagnostic catalogue covers the brief well. Two problems remain:
- The type mapper trusts spellings, so a compiling core can publish a schema that disagrees with its own wire layout, and bindgen passes it (H1).
- Generated proxies panic on a SPEC-legal port outcome, which traps the core on wasm (H2).

The diagnostics fall short of R8 wherever the error comes from rustc rather than the macro: the store cascade, a store with no impl, misplaced attributes, and shapes that only bindgen rejects.

**Three fixes first:**
1. Compile-time type-identity assertions for every schema type reference: `KeelNamed::KEEL_NAME` checks for named types and `fn(Ty) -> Canonical` coercions for leaves. This closes H1 and moves `Handle` and alias failures (M3) to compile time.
2. Non-panicking port proxies: map `PortError` into `E: From<PortError>` for `Result` methods, and make unbound infallible ports an init-time error (H2).
3. Store diagnostics:
   - A store fallback that keeps `__keel_cell` and the store consts (M1).
   - `#[diagnostic::on_unimplemented]` branding for a missing store impl (M2) and for `Encode`/`Decode`/`Default` (M5).
   - UI tests that include the impl block.
