# ADR-023: an unavailable port is a typed outcome, and the schema names the type the wire carries

Status: accepted (2026-09-30). Touches SPEC 2.1, 4.2, 5.7, 8, 12 and 16.3 (generated code shapes).
Origin: `.10x/reviews/2026-09-30-keel-macros-review.md`, findings H2 (proxies panic on a port outcome
the SPEC defines) and H1 (the schema can name a different type than the wire carries); the smaller
generated-shape changes that came with the Medium findings are listed at the end.

## H2: a port nobody registered must not abort the core

Context. SPEC 6.3 makes "unavailable" a normal outcome: a port that is not registered behaves as
status 2, a call can be cancelled, and a host can reply with bytes that do not decode. The generated
proxy turned every such `PortError` into `panic!`, even for a method that returns `Result<T, E>`.
SPEC 7 builds the wasm core with `panic=abort`, so on the web a platform that does not register an
optional port (SecureStore, Fs, Http in a test page) trapped the whole module instead of returning an
error the app can show. R6 says nothing escapes as a panic or an abort.

Decision. Where a method has an error channel the proxy uses it:

* `PortError::Failed(bytes)` is the encoded `E` and decodes into it, as before;
* every other outcome, and an `Ok` or `E` reply that does not decode (`PortError::Decode`), becomes
  `E::from(port_error)`.

The bound is `E: From<PortError>`, expressed through a trait the expansion defines, so a missing
impl produces `error[keel::E0033]` at the error type in the trait (what, why, fix, docs) rather than
a bare `From` bound failure. `HttpError` and `FsError` in `keel-ports` implement it: `Unavailable`
maps to `Network("the Http port has no adapter registered")` / `Io("the Fs port has no adapter
registered")`, `Cancelled` to `Cancelled` / `Io(..)`, `Decode(e)` to `Network("malformed port reply:
..")` / `Io(..)`, and an undecodable `Failed` to the same `Network` / `Io` text. The wire layouts and
the byte goldens do not change.

A method without an error channel (`fn get(&self, k: String) -> Option<Bytes>`) has no typed way to
say "unavailable". Its proxy still panics, now with a message that teaches: the port and method
("keel: the `Kv` port has no adapter registered (method `get`)"), how to bind one
(`core.registerPort(..)` in TypeScript, Kotlin and Swift, `keel_port_register` in the C ABI, a Rust
implementation or a fake), that on the web the panic traps the core, and the docs link
(`https://keel.dev/errors/E0062`). The runtime already contains the panic at the dispatch boundary on
native targets.

Alternative rejected: fail `Runtime::init` (or the first call) when an infallible port is unbound.
Binding is legitimately dynamic: a host registers ports as it starts, a dev core binds some of them
from a remote client after `init`, and tests install fakes per test. A check at init would either
reject valid setups or be a second source of truth about which ports a core needs. The schema
already lists the ports; what an app can rely on is "an unbound port answers unavailable", and a
method that wants to survive that returns a `Result`.

Consequence for existing code: a `#[keel::port]` method returning `Result<T, E>` needs
`impl From<PortError> for E`. The e2e test, the README example and the ui-runtime fixture gained one.

## H1: a schema type reference is checked against the type it resolves to

Context. The type mapper reads spellings. A user record called `Bytes` was recorded as the built-in
`bytes` while `Encode` moved the user's record (and its extra field); `use model::Item as Todo`
recorded `Todo` while the wire carried an `Item`. `keel-bindgen` validated both schemas.

Decision. Every type position the schema records gets a compile-time check emitted with the
expansion, on the user's own tokens (`keel-macros/src/impl_/check.rs`):

* Built-in mappings (scalars, `String`, `Bytes`, `Uuid`, `Timestamp`, `Duration`, and the `Vec`,
  `Option`, `HashMap`, `BTreeMap`, `Box`, `Result` constructors, level by level) are compared with
  the canonical type through a trait defined in the expansion whose `on_unimplemented` carries the
  branded message: `error[keel::E0060]`.
* Named mappings compare `<Ty>::KEEL_TYPE_ID` (an inherent constant every record and enum already
  had) with `type_id("<recorded name>")` in a const assertion: `error[keel::E0061]`. An alias or a
  renamed import has the other type's id and fails. Types that are not Keel types resolve to a
  fallback trait constant of `0` (inherent items win over trait items in `<T>::NAME`) and fail too.
* Objects carry a hidden `__KEEL_IS_OBJECT`, so an object used as a value says so (`E0064`); enums
  carry `KEEL_IS_ERROR`, required on the error side of a `Result` (`E0001`).
* `keel-wire`'s `Encode` and `Decode` carry a branded `on_unimplemented`, so any other type that
  cannot cross reads as `E0001` as well.

No new public trait surface: the marker items are hidden inherent constants on the types the macros
already define, and everything else lives inside an anonymous constant of the expansion.
`keel-runtime` is untouched.

Alternative rejected: a `KeelNamed` trait carrying the schema name. It would make the check a string
comparison, but it adds a public trait to `keel-meta` or `keel-runtime` for a property the type id
already encodes (two names with one hash are caught by the registry's collision check).

## Generated shapes that changed alongside

These follow from the Medium and Low findings and are listed so the next reader of the snapshots
knows why they moved.

* `impl StoreObject for T` is written by the `#[keel::api(store)] impl` block next to `impl
  KeelObject`, forwarding to hidden inherent members (`__keel_cell_ref`, `__keel_restore`) of the
  struct. A store without its impl block gets the branded `E0011` instead of four unsatisfied
  `KeelObject` bounds, and a failed `#[keel::store]` still defines the hidden field and stub members
  so its impl block adds no errors. A record, enum or error that fails to expand likewise keeps
  behaviour-free `Encode`, `Decode` and `KEEL_TYPE_ID` (and `Display`/`Error` for an error), so the
  new identity checks do not repeat the one real error at every use of the type.
* Generated locals that sit next to user parameter names are positional (`__keel_a0`, ..) or carry
  the `__keel_` prefix, so a parameter called `__r`, `__w` or `__ctx` no longer collides.
* A port whose snake-case name is a keyword gets a raw accessor (`r#match`); `self`, `super` and
  `crate` get a trailing underscore.
* `Option<Option<T>>` is rejected (E0063; SPEC 2.1 said "allowed", bindgen said no), `Handle` has no
  schema type (E0001), records need a field (E0007, SPEC 3.1's zero-width argument), queries may not
  return `()` or an `Option` (E0042, bindgen's existing rule), an implicit `{}` in an error message
  is E0010 (the schema keeps the text, and the platforms resolve placeholders by index or name), and
  `From<E> for keel::Error` was deleted from SPEC 4.2 (no `keel::Error` exists in v1).

Known limits, recorded rather than hidden: the macros cannot see their parent, so `#[keel::query]` on
an associated function of a plain `impl` (not `#[keel::api]`) is still reported by `rustc`; a second
`#[keel::api] impl` block for one type is reported as a duplicate definition of a constant named
after the rule; a plain object's struct carries no Keel attribute, so its docs are the impl block's
(a store merges its struct docs first).
