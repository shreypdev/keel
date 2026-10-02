# ADR-042: transparent newtypes, named generic instantiations, a `Decimal` wire type and third-party leaf types

Status: **Accepted (implemented by `types-paging`, 2026-10-02; the deviations are at the end)** (proposed 2026-10-01, `wt/boundary-adrs`; Amendment B "boundary surface", Amendment C item 5,
which adds decimals and the `uuid`/`chrono` types to this ADR's scope). Touches SPEC 2.1 (`TypeRef::Decimal`),
2.2 (`RecordDef.transparent`), 3.1 (two encodings), 4.1 (two new accepted shapes), 10.1–10.3 (generated
shapes), 12 (E0002 and E0007 texts, one new code) and 16.3; `undra-meta`, `undra-wire`, `undra-macros`,
`undra-bindgen`, the three platform runtimes' codecs. **Wire: one new leaf encoding (`Decimal`); no change to
any existing encoding. Schema: one new `TypeRef` variant and one flag written only when set, so no existing
schema's hash moves. No C ABI or wasm ABI change.** Constitution R1 (the schema must describe every new
shape), R3 (the generated shapes) and R11 (a generated public shape changes), so it is decided here before
code.

## Context

The competitive catalogue (`.10x/specs/2026-10-01-competitive-limitations.md`) found the boundary narrower
than the tools a Rust team is leaving:

* Finding 3: "Undra has no objects as parameters or returns (E0064), no host callbacks … (E0004), no generics
  (E0002) and no newtypes (E0007)." KMP-6: "Undra is **stricter**: no generics at all (E25, E0002)." FL-B6:
  flutter_rust_bridge takes "arbitrary types", and "G4 must not ship with a narrower surface than the bridge
  it replaces."
* §11.2 lists "Newtypes stay typed" among the blueprint claims "the code does not back … as of today
  (E0007)".

The gap audit (`.10x/specs/2026-10-01-v1x-gaps.md` §2, §4.1) probed each shape:

| Gap | Shape | Today |
|---|---|---|
| TY-2 | `struct UserId(Uuid)` | E0007 "tuple struct `UserId` cannot be a record … use named fields" (`crates/undra-macros/src/impl_/record.rs:279-286`); the named-field workaround crosses as a wrapper struct/data class/interface on every platform |
| TY-3 | `struct Page<T>`, `enum Loadable<T>` | E0002 "the schema describes concrete types … declare one concrete `#[undra::api]` type per instantiation" (`crates/undra-macros/src/impl_/common.rs:11-44`), and there is no way to register an instantiation |
| TY-5 | money, `i128`/`u128` | E0001; no decimal type at all |
| TY-6 | `uuid::Uuid`, `chrono::DateTime<Utc>`, `rust_decimal::Decimal` | E0060/E0061 + E0001 + a rustc cascade of 4–6 errors: the mapper reads the spelling `Uuid` (`crates/undra-macros/src/impl_/types.rs:545-567`) and the same-type assertion then finds `uuid::Uuid` is not `undra::Uuid` |

Two facts make the newtype cheap: a record is encoded as its fields in order with no header (SPEC 3.1), so a
one-field record and its field have **identical bytes**; and the schema already writes optional flags only
when set (`SignalDef.no_coalesce`, `skip_serializing_if = "is_false"`, `crates/undra-meta/src/def.rs:201-205`),
which keeps hashes stable when a flag is added.

## Decision

### 1. Transparent newtypes

1. **Shape.** `#[undra::api]` accepts a tuple struct with exactly **one** field: `pub struct UserId(pub Uuid);`
   (the field's visibility is free). A unit struct or a tuple struct with two or more fields stays E0007, with
   the help text naming both fixes ("one field: a newtype; several: named fields").
2. **Wire.** The inner type's encoding: the same bytes as a one-field record. No length, no tag.
3. **Schema.** A `RecordDef` with `transparent: true` and exactly one `FieldDef { name: "value", ty, default:
   false }`. `transparent` is serialised only when `true`, so every existing schema hashes as before. Validation
   (`undra-meta` and `undra-bindgen`): a transparent record has one field named `value`, no `default`.
4. **Positions.** Anywhere a record may appear (fields, parameters, returns, signals, stream items, port and
   callback parameters), and additionally **as a map key** when its inner type is a valid key
   (`HashMap<UserId, User>`): `Schema::is_valid_map_key(&TypeRef)` resolves transparent records, and
   `TypeRef::is_valid_map_key` keeps its current, schema-free meaning. A keyed list may use a newtype key field
   (the key function already hashes the encoded field, ADR-027). The inner type is any value type, including
   another newtype, a record, `Vec<T>`, `Option<T>` and `Decimal`; never `Unit`, an object (ADR-040), a
   callback (ADR-041) or `Lazy<T>` (ADR-043).
5. **Rust side.** `Encode`/`Decode` delegate to `.0`; the inherent `UNDRA_TYPE_ID` as for records. The macro
   generates no `From`, `Deref` or accessor: those are the author's API decisions.
6. **Generated shapes** (examples for `/// A user's id. #[undra::api] pub struct UserId(pub Uuid);` and
   `pub struct Meters(pub f64);`):

   ```swift
   /// A user's id.
   public struct UserId: RawRepresentable, UndraRecord, Sendable, Hashable, Codable {
       public var rawValue: UUID
       public init(rawValue: UUID) { self.rawValue = rawValue }
       public init(_ rawValue: UUID) { self.rawValue = rawValue }
       public init(from decoder: any Decoder) throws { rawValue = try decoder.singleValueContainer().decode(UUID.self) }
       public func encode(to encoder: any Encoder) throws { var c = encoder.singleValueContainer(); try c.encode(rawValue) }
       public static func undraDecode(_ r: inout UndraReader) throws -> UserId { UserId(try UUID.undraDecode(&r)) }
       public func undraEncode(_ w: inout UndraWriter) { rawValue.undraEncode(&w) }
   }
   public struct Meters: RawRepresentable, UndraRecord, Sendable, Hashable, Codable, Comparable { … }  // Comparable: Double is
   ```

   ```kotlin
   /** A user's id. */
   @JvmInline
   value class UserId(val value: UUID) : UndraRecord {
       companion object : UndraCodec<UserId> { /* decode/encode delegate to the UUID codec */ }
   }
   @JvmInline
   value class Meters(val value: Double) : UndraRecord, Comparable<Meters> {
       override fun compareTo(other: Meters): Int = value.compareTo(other.value)
       companion object : UndraCodec<Meters> { … }
   }
   ```

   ```ts
   /** A user's id. */
   export type UserId = string & { readonly __brand: "UserId" };
   /** Wraps a string as a `UserId`; no check is made. */
   export function UserId(value: string): UserId { return value as UserId; }
   export const UserIdCodec: Codec<UserId> = uuidCodec as Codec<UserId>;
   ```

   `Comparable` (Swift, Kotlin) is added when the inner platform type is comparable and order is meaningful:
   integers, floats, `String`, `Timestamp`, `Duration`, `Decimal`, and a newtype of one of those. Not for
   `Uuid` (Foundation's `UUID` is `Comparable` only from iOS 17, which ADR-045 must not assume), bytes, records
   or collections. Literal conformances (`ExpressibleByStringLiteral`) are deliberately not generated: they
   would let any literal become a `UserId`, which is what the newtype exists to prevent.
7. **Persisted state (ADR-037).** `T → Newtype(T)` and back are lossless by construction (equal bytes);
   ADR-037's structural rules gain this step, so wrapping an existing field in a newtype migrates for free.

### 2. Named generic instantiations

1. **The template.** `#[undra::api(generic)]` on a struct or enum with **type** parameters
   (`pub struct Page<T> { pub items: Vec<T>, pub next: Option<String> }`, `pub enum Loadable<T> { Loading,
   Loaded(T), Failed(String) }`). Lifetimes stay E0003, const generics and `where` clauses stay E0002 (with a
   help text that says only type parameters are supported). The template **registers nothing**: it has no wire
   identity of its own. It expands to the item, generic `impl<T: Encode> Encode` / `impl<T: Decode> Decode`
   (the wire of every instantiation is the same field walk), and a hidden `#[macro_export] macro_rules!
   __undra_template_<Name>` that carries the field list with each type parameter replaced by a metavariable,
   re-exported next to the type with `#[doc(hidden)] pub use` so it can be named by the type's path.
2. **The instantiation.** `#[undra::api] pub type TodoPage = Page<Todo>;` invokes that template with the
   concrete arguments; the template calls a hidden proc macro (`undra::__instantiate!`) with the substituted
   definition, which runs the ordinary record (or enum) expansion **on concrete tokens**: the type mapper, the
   identity checks, and `RecordMeta`/`EnumMeta` for **`TodoPage`** (`type_id = fnv1a32("TodoPage")`). It also
   emits `impl Page<Todo> { pub const UNDRA_TYPE_ID: u32 = type_id("TodoPage"); }`, so a signature that spells
   `TodoPage` passes E0061 (an *unregistered* alias still fails it, as ADR-025 intends).
3. **Rules.** Signatures spell the alias. Spelling `Page<Todo>` directly is E0002 with the fix in the text
   ("declare `#[undra::api] pub type TodoPage = Page<Todo>;` and write `TodoPage`"). The alias must be declared
   in the crate that declares the template (the inherent impl of decision 2.2 is only legal there); elsewhere,
   and for a second alias of the same instantiation, the error is **E0070** (new; the inherent impl carries a
   constant named after the rule, the technique ADR-025 uses for a duplicate `#[undra::api] impl` block).
   Arguments are concrete types the mapper accepts, including other aliases and newtypes.
4. **What stays E0002.** Generic objects, stores, free functions, methods, ports, callbacks, queries and
   mutations: their dispatch would need one dispatcher per instantiation and a schema that can name a type
   parameter. The text says so and points at the named-instantiation pattern for data types.
5. **Generated shapes.** Each instantiation is a plain named record or enum (`TodoPage`), byte-for-byte what a
   hand-written `#[undra::api] struct TodoPage { items: Vec<Todo>, next: Option<String> }` produces. The
   schema contains no type parameters.

### 3. `Decimal`

1. **Wire.** A new leaf, `TypeRef::Decimal`: `mantissa i128` (16 bytes, little-endian two's complement) then
   `scale u8`, value = mantissa × 10^−scale, scale ≤ 38 (the decoder rejects a larger one with
   `WireError::InvalidTag { ty: "decimal scale" }`). Not normalised: `1.0` and `1.00` are distinct encodings
   that are numerically equal.
2. **Rust.** `undra::Decimal { mantissa: i128, scale: u8 }` in `undra-wire` (exact `FromStr`/`Display`,
   structural `Eq`, `cmp_numeric`, no arithmetic) and, behind the `rust_decimal` feature, conversions and
   `Encode`/`Decode` for `rust_decimal::Decimal` (its 96-bit mantissa and scale ≤ 28 always encode; a wire
   value outside its range decodes to a `WireError`, never a panic).
3. **Platforms.** Swift `Foundation.Decimal` (its 128-bit mantissa and exponent range hold every wire value),
   Kotlin `java.math.BigDecimal(BigInteger, scale)` (exact; note that `equals` is scale-sensitive, which Kotlin
   engineers expect), TypeScript a small immutable `Decimal` class in `@undra/runtime` (`mantissa: bigint`,
   `scale: number`, `toString()`, `static parse(text)`, `equals`, `compare`; no arithmetic — convert through
   `toString()` to the app's decimal library).
4. Not a map key (numeric versus encoded equality would disagree). `i128`/`u128` stay E0001; the help text
   points at `Decimal` (or a `String` newtype for identifiers).

### 4. Third-party leaf types (opt-in features)

1. `undra` (and `undra-wire`) gain features mapping popular types onto **existing** wire types: `uuid`
   (`uuid::Uuid` → `Uuid`), `chrono` (`DateTime<Utc>` → `Timestamp`; `TimeDelta` → `Duration`), `time`
   (`OffsetDateTime` and `UtcDateTime` → `Timestamp`, normalised to UTC; `time::Duration` → `Duration`),
   `rust_decimal` (→ `Decimal`), `bytes` (`bytes::Bytes` → `Bytes`). Each dependency is declared with
   `default-features = false` and **no clock or randomness feature** (R12; `chrono`'s `clock` and `uuid`'s
   `v4` would also pull `wasm-bindgen`/`getrandom` into wasm32), and CI builds each feature for
   `wasm32-unknown-unknown`, `aarch64-apple-ios` and `aarch64-linux-android` (CLAUDE.md dependency rule).
2. **Precision is documented, not hidden:** `Timestamp` is milliseconds; a sub-millisecond `chrono`/`time`
   value truncates toward negative infinity on encode. `DateTime<Local>`/`DateTime<FixedOffset>` are E0001
   with the help "convert to `Utc`; offsets do not cross".
3. **Identity check.** E0060's same-type assertion becomes a membership assertion: `undra-wire` defines a
   hidden marker trait `WireLeaf<K>` per leaf kind, implemented for the canonical type and, behind each feature,
   for the foreign one; the check asserts `Spelled: WireLeaf<K>`. The mapper learns the spellings `DateTime`,
   `OffsetDateTime`, `UtcDateTime`, `TimeDelta` and `Decimal`. Without the feature, the error is one E0001 that
   names the feature to enable (D1 collapses the cascade).

## Alternatives considered

* **Newtypes as one-field records (status quo with a nicer error).** Ships a `UserId { value }` wrapper class
  on every platform: not what a Swift, Kotlin or TypeScript engineer writes (R3), and keys/sets of ids need
  extra code.
* **A separate `newtypes` list in the schema.** Duplicates most of `RecordDef`'s handling (ADR-037 closures,
  bindgen's model) for a shape whose wire is already a one-field record's.
* **Erase newtypes to the inner type on the platforms.** Simplest, but throws away the type safety the author
  asked for, and "Newtypes stay typed" is the claim the post wants to make.
* **Generic platform types** (`struct Page<T>` in Swift with conditional `UndraCodec` conformance, Kotlin
  `data class Page<T>` with codec parameters, `interface Page<T>`). Nicer for one generic paging view, but the
  schema would need type parameters (`TypeRef::Param`, `TypeRef::Apply`), every runtime's codec model would
  take codec arguments (Kotlin erases `T`), and ADR-037's closures would have to substitute. Revisit in v2.
* **Infer instantiations from use** (`fn page(&self) -> Page<Todo>` registers `PageTodo` automatically). The
  generated name would be invented, collide across crates, and change when a signature changes.
* **`i128` as a wire type instead of `Decimal`.** Swift has no `Int128` before iOS 18 / Swift 6's
  availability, TypeScript would need `bigint` everywhere, and money still needs a scale.
* **Feature-free foreign types via `From` conversions in user code.** Every team writes the same wrappers; the
  mapping is mechanical and belongs in one place.

## Consequences

* Newtypes, named generic instantiations, `Decimal` and the five foreign leaf types cross. The post can say
  "newtypes stay typed" (catalogue §11.2) once this lands.
* Existing schemas hash unchanged (`transparent` is written only when true; `Decimal` is a new kind).
* The generated namespace gains branded types in TypeScript (`UserId` is both a type and a function) and value
  classes in Kotlin; both are idiomatic.
* `@undra/runtime` gains a `Decimal` class (it stays free of runtime dependencies), the Kotlin runtime a
  `BigDecimal` codec, the Swift runtime a `Decimal` codec.

## Risks

* **Kotlin value classes and Java interop.** Functions taking a value class get mangled JVM names; a Java
  caller of generated Kotlin cannot call them. Undra targets Kotlin; noted in the docs.
* **Template macro hygiene.** The `macro_rules!` template is exported at the crate root; two generic types
  with one name in one crate collide (they would collide in the schema anyway, E0050). The proc macro must
  substitute type parameters inside nested generics (`Vec<Option<T>>`) and leave same-named paths in other
  namespaces alone; covered by expansion tests.
* **Precision surprises** with `chrono`/`time` (milliseconds) and `Decimal` scale-sensitive equality in Kotlin.
  Documented on each type and in the cookbook.

## Implementation brief

1. `crates/undra-meta`: `RecordDef.transparent` (+ `RecordMeta`), `TypeRef::Decimal` (+ `TypeRefMeta`,
   `Display` `decimal`), validation rules (decisions 1.3, 1.4, 3.4), `Schema::is_valid_map_key`; canonical-JSON
   tests proving an unflagged record and a schema without `Decimal` hash exactly as before.
2. `crates/undra-wire`: `Decimal` (+ proptest round trips, a fuzz case for the scale check), the `WireLeaf<K>`
   markers, features `uuid`, `chrono`, `time`, `rust_decimal`, `bytes` with their `Encode`/`Decode` impls and
   `default-features = false`; the facade forwards the features.
3. `crates/undra-macros`: `record.rs` (one-field tuple struct → transparent record; E0007 text), a `generic`
   argument on `#[undra::api]` for structs and enums (template expansion, `macro_rules!` emission), `#[undra::api]`
   on type aliases and the hidden `__instantiate!` proc macro, E0070 (`impl_/diag.rs`), `types.rs` (new leaf
   spellings, `Page<Todo>`-style generic spelling → E0002 with the alias help), `check.rs` (`WireLeaf`
   membership instead of same-type for leaves). UI tests (trybuild) for every new diagnostic.
4. `crates/undra-bindgen`: transparent records in `swift.rs`, `kotlin.rs`, `ts.rs` (decision 1.6), `Decimal`
   mapping, map-key validation through transparent records. Golden cases `newtypes` (newtype of a scalar, of a
   record, nested, as a map key, as a keyed-list key) and `generics` (a struct and an enum template, two
   instantiations each) and `decimal`; the Swift golden is type-checked, Kotlin compiled with `-Werror`, TS
   type-checked, as for the existing cases.
5. Runtimes: Swift `Decimal: UndraCodec`; Kotlin `BigDecimal` codec; TS `Decimal` class + `decimalCodec`
   (exported from `@undra/runtime`, documented, unit-tested).
6. `contract-tests/wire-vectors.json`: `decimal` vectors (zero, negative, scale 0 and 38, `i128::MIN`, a scale
   of 39 rejected); a contract scenario **S20 "newtypes and generic instantiations"** (provisional number; ADR-039 holds S19): a method takes and
   returns `UserId`, a `HashMap<UserId, Todo>`, and a `TodoPage`; a `Decimal` round-trips exactly.
7. Docs: SPEC 2.1, 2.2, 3.1, 4.1, 10, 12 (E0002, E0007 texts; E0070); the errors page (D1); the cookbook's
   "modelling your domain" page (newtypes, `Page<T>`, money).
8. No budget row: no boundary hot path changes; `bench` gains `wire/decimal` encode/decode rows for the
   record.

## Dependencies

None to start. ADR-037's structural rules gain the newtype step (decision 1.7) if ADR-037 lands first,
otherwise ADR-037's implementer adds it. ADR-043 does not need generic instantiations (its `Page<T, C>` is a
query-only shape the query macro recognises; see ADR-043).

## Implementation (2026-10-02, `wt/types-paging`) and deviations

All eight items of the brief landed (the records: `.10x/decisions/sde/types-paging.md`, `types-paging-macros.md`,
`types-paging-bindgen.md`, `types-paging-swift.md`, `types-paging-kotlin.md`, `types-paging-ts.md`). The contract scenario is **S31**
(S29 and S30 went to ADR-046), the playground module `ledger.rs`, the guide `site/docs/types.html`. Deviations, each dated 2026-10-02:

1. **`MapKey` is one blanket impl over a hidden `Newtype` trait**, not a conditional impl per newtype (decision 1.4): `impl MapKey for UserId
   where Uuid: MapKey {}` is rejected by rustc at the definition when false, so `struct Price(Decimal)` would not compile at all. The macro
   emits `impl Newtype for UserId { type Inner = Uuid; }`; `impl<T: Newtype> MapKey for T where T::Inner: MapKey` does the rest. A
   `HashMap<Price, _>` is still E0006 at the map, at compile time.
2. **The template macro is re-exported under the type's own name** in the macro namespace (`pub use __undra_template_Page as Page;`), so
   `use model::Page;` finds both; an instantiation checks only its type arguments (a `macro_rules!` resolves names where it is invoked).
3. **E0070 for an alias in another crate** is found by the macro (the template embeds `CARGO_CRATE_NAME`), so the message is branded; a
   second alias of one instantiation is `rustc`'s duplicate-constant error pointed at both aliases.
4. **Feature mirror.** `undra-macros` has its own copy of each leaf feature, turned on by `undra`'s: with a feature off, a spelling only that
   crate has (`DateTime<Utc>`, `OffsetDateTime`, `TimeDelta`, `uuid::Uuid`, ..) is exactly one E0001 naming the feature; a bare `Uuid`,
   `Decimal`, `Bytes` or `Duration` may be either type, so without the feature it is the compiler's three errors, which agree on the fix.
5. **`Decimal` in Rust** has public `mantissa`/`scale` fields, `new` (panics above scale 38), `try_new`, `ZERO`, `is_zero`, `cmp_numeric`/
   `eq_numeric` (exact for every pair) and `ParseDecimalError`; the `rust_decimal` feature converts through the wire encoding.
6. **What each platform's `Decimal` does at its edges** (decoding is exact everywhere): Swift saturates a value of 2^127 or more, encodes NaN as
   zero and cuts digits past the 38th after the point; Kotlin rounds those digits half up, turns a negative scale into whole digits and
   saturates a mantissa of more than 128 bits; TypeScript's `Decimal` constructor throws `RangeError` for what does not fit. Documented in SPEC 17.
7. **Generated shapes**: a Kotlin newtype of `Bytes` is a plain class with content equality (a value class cannot override `equals`); a TypeScript
   newtype of a newtype is branded on what the inner one wraps and a newtype of an option brands the payload; non-primitive TypeScript newtype
   codecs delegate lazily (use-before-assignment at module load); `Option<N>` where `N` wraps an option is E0001 (nested optionality, E0063's
   reason); `Decimal`, `BigDecimal`, `UndraLazyList`, `UndraLazyListObject`, `LazyList` and `InfiniteQuery` are reserved type names.
8. **Persisted state (decision 1.7)**: the structural migration wraps and unwraps a newtype in both the streamed and the tree conversion
   (`ClosureRecord.transparent`), and wrapping a persisted field in a newtype needs no hook (tested).
9. **Numbers.** `wire/decimal/roundtrip` is in the `wire` group with a budget of 250 ns (measured 45 ns: 16 bytes of mantissa, a scale checked on decode). The foundation costs the hello-world
   web core bytes (schema serialisation of the new fields, the persisted-state arms); see ADR-043's size note, where they were paid back.

