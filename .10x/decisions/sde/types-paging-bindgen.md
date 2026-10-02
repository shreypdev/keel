# types-paging-bindgen: the generators for newtypes, polling, infinite queries and lazy lists (ADR-042, ADR-043)

Sub-piece `tp-bindgen` of `types-paging` (the contract is `.10x/decisions/sde/types-paging.md`). Branch `wt/tp-bindgen`,
worktree `/Users/shrey/Desktop/src/.work/tp-bindgen`. Owns `crates/undra-bindgen/**`; `undra-cli` needed no change beyond
regenerating the committed bindings (it names no type that moved).

## What landed

### `model.rs` (shared by the three emitters)

* The handle of every query is synthesized with `set_poll_interval(interval: Option<Duration>)`
  (`ids::SET_POLL_INTERVAL_METHOD_ID`, re-exported as `QUERY_SET_POLL_INTERVAL_ID`), after `refetch` and `invalidate`. A
  handle of an `infinite` query has `data` as `Vec<T>` keyed by `item_key` (not `Option`), the signals `5 has_next_page` and
  `6 fetching_next_page`, and `fetch_next_page()` (`QUERY_FETCH_NEXT_PAGE_ID`) first among its methods. The error type is
  read from `returns` as for every query, so both `Vec<T>` and `Result<Vec<T>, E>` work (see "Open items" 1).
* Helpers: `Model::newtype`/`newtype_inner`/`resolve_newtypes` (a newtype and its inner type share their bytes; the cycle of
  two newtypes stops after 16 steps), `is_ordered` (ADR-042's `Comparable` rule: integers, floats, `String`, `Timestamp`,
  `Duration`, `Decimal`, and a newtype of one), `is_query_handle`, `infinite(handle)` (item record and key field),
  `identifiable_items()` (the item records whose key field is named `id`).

### `validate.rs`

* The `Lazy<T>` refusal is gone; `Lazy<T>` signals may be keyed or computed, the item is checked like a `Vec<T>`'s, and a key
  is legal on a `Vec` or a `Lazy` only. E0006 (invalid map key, newtypes of valid keys allowed, decimals not) and E0007
  (transparent shape) and E0073 (infinite shape) come from `Schema::validate`; their texts are locked in the diagnostics
  goldens.
* New E0001: `Option<N>` where `N` is a newtype (however nested) that wraps an option, also as the result of a query. It is a
  nested option on the wire, which Kotlin and TypeScript cannot tell apart, and a TypeScript brand cannot hold `null`.
* Reserved type names: `Decimal`, `BigDecimal`, `UndraLazyList`, `UndraLazyListObject`, `LazyList`, `InfiniteQuery` (a schema
  type with one of those would shadow the runtime's). `naming::RESERVED_ENTRIES` gained the five `UndraLazy*` names the Swift
  and Kotlin runtimes declare (their test fails otherwise).

### Swift (`swift.rs`)

* Newtype: `public struct X: RawRepresentable, UndraRecord, Sendable, Hashable[, Codable][, Comparable]` with `rawValue`,
  `init(rawValue:)`, `init(_:)`, a single-value `Codable` (only when the inner type is `Codable`, so a newtype of `Duration`
  is not), `<` where ordered, `undraDecode`/`undraEncode` that delegate to the inner type through the same `read_expr` /
  `write_stmt` paths as a field (bytes and options keep their special paths). No literal conformances. Zero value
  `X(<inner zero>)`.
* `Identifiable` on an infinite query's row record when its key field is `id`.
* Query handles: `setPollInterval(_ interval: Duration?)` (unlabeled, `UndraDuration?` below iOS 16); infinite handles get
  `fetchNextPage()`, `hasNextPage`, `fetchingNextPage` and
  `loadMore(ifNeededFor item: Post, threshold: Int = 5)`, which is
  `guard hasNextPage, !fetchingNextPage, data.suffix(max(threshold, 0)).contains(where: { $0.<key> == item.<key> }) else { return }`
  then `fetchNextPage()`. ADR-043 says "find the index by key"; the `suffix` form is the same predicate (the item is within
  `threshold` rows of the end) without scanning the whole list for every row that appears, and `max` keeps a negative
  threshold from trapping.
* `Lazy<T>` signal: `public let books: UndraLazyList<Book>` (`UndraLazyListObject` in the `ObservableObject` mode, with a doc
  line that it must be observed directly: a nested `ObservableObject` does not publish through its owner), initialised before
  `super.init`; `apply`: `.fullValue` -> `try books.applyFull(&reader)`, `.lazyListInvalidated` -> `try
  books.applyInvalidated(&reader)`, `.keyedPatch` -> `break`.

### Kotlin (`kotlin.rs`)

* Newtype: `@JvmInline value class X(val value: T) : UndraRecord[, Comparable<X>]` with the delegating `companion object :
  UndraCodec<X>`. **Deviation:** a newtype of `Bytes` / `Option<Bytes>` is an ordinary `class` with `equals`/`hashCode` over
  the content (`equals` and `hashCode` are "reserved for future releases" in a value class, in 2.0.21 and 2.4.20, and a
  `ByteArray` compares by identity).
* `setPollInterval(interval: Duration?)`. An infinite handle `: UndraStore(core, handle), InfiniteQuery` with `override val
  hasNextPage`, `override val fetchingNextPage` and `override fun fetchNextPage()`.
* `Lazy<T>`: `val books: UndraLazyList<Book> = UndraLazyList(core, Book)` (the constructor's `core`); `apply` calls
  `applyFull(reader)` / `applyInvalidated(reader)`; a store with lazy lists overrides `close()` to close them first
  (`UndraLazyList` is `AutoCloseable`: "stops requesting pages and drops the cache").

### TypeScript (`ts.rs`)

* Newtype: `export type X = <T> & { readonly __brand: "X" }`, `export function X(value)` (a cast, no check) and `XCodec`. The
  codec is `codecs.<inner> as Codec<X>` when the wrapped type has a codec of the runtime; otherwise (a named type, an option, a
  list, a map) an object whose methods look the inner codec up when they run, because `const XCodec = ZedCodec as ...` would
  read `ZedCodec` before it is assigned when `Zed` is declared further down the file (TDZ). **Deviations from the ADR text,
  forced by TypeScript:** a newtype of a newtype is branded on what the inner one wraps (a brand on a brand is two `__brand`
  literals in one intersection, which TS reduces to `never`), its constructor taking the inner newtype and casting through
  the unbranded type; a newtype of an option brands the payload, `(string & Brand) | null` (`null & Brand` is `never`).
* `setPollInterval(ms: Duration | null)` (the parameter is named `ms`, a `Duration` is milliseconds).
* `Lazy<T>`: `readonly books: LazyList<Book> = new LazyList(this.core, BookCodec)` (a field initializer; the base class has set
  `core`), `_apply` hands `new UndraReader(value)` to `applyFull` / `applyInvalidated`, and the list is left out of `_signals`.

### Tests and goldens

* Golden cases `newtypes`, `generics`, `decimal`, `polling`, `infinite` and `lazy` (Swift also in the `ObservableObject` mode
  for `newtypes`, `polling`, `infinite`, `lazy`); every golden of a query handle (`queries`, `full`, `stdlib`, ...) changed by
  exactly the `setPollInterval` method, its id and, in Kotlin and TypeScript, one hoisted `Option<Duration>` codec / import.
* Diagnostics goldens: E0001 (the lazy message is gone, the nested-option message is new), E0006 (new text, a decimal key, a
  newtype of a float), E0007, E0073.
* `generators.rs`: structure of every new shape (comparable newtypes and the rest, Swift/Kotlin/TS newtype forms, brands never
  doubled, handles, `Identifiable`, lazy lists, decimal). `model.rs` unit tests. `validate.rs` tests.
* Execution: Swift `swift-run/newtypes.swift` (wire bytes equal the inner's, `Hashable`, `Comparable`, `Codable`); Kotlin
  `kotlin-run/{newtypes,infinite,lazy}` over a shared `support/FakeCore.kt`; TypeScript `ts-run/{newtypes,paging,lazy}.mjs`
  and `newtypes_are_nominal_in_typescript` (each `@ts-expect-error` must be an error: a `TodoId` is not a `UserId`, a nested
  newtype is not its inner, a plain string is not a newtype).
* `tests/fixtures/ts-base/index.d.ts` declares a stand-in `LazyList<T>` against the fixture's own `UndraCore`/`Signal`
  (the real class is `runtimes/ts/@undra/runtime/src/lazy.ts`).
* The committed bindings of the playground, cookbook, two-cores a/b and ios15-sample are regenerated (only `setPollInterval`
  and its id); `undra bindgen --check [--docs]` passes on all six, Fieldbook has no query.

## Open items for the integrator

1. **`Schema::validate` and the query macro disagree about an infinite query's `returns`.** E0073 requires `Vec<T>`
   (`infinite_problem` in `undra-meta`), but `#[undra::query]` requires `Result<_, E>`. With `Vec<T>` the handle's `error` is
   `String?`. Bindgen reads the error type from `Result<Vec<T>, E>` too (a model test pins it), so accepting that shape in
   `infinite_problem` is all that is needed; the goldens use the bare `Vec<T>` because that is what validates today.
2. **E0073 is not in SPEC section 12**, so `cargo test -p undra-macros --test catalogue` fails twice (it was failing once
   before: the code is emitted by `undra-meta`; my `tests/golden/diagnostics/E0073.txt` is the second assertion). Adding
   E0073 (and its docs rows) to SPEC 12 / `docs/ERRORS.md` / `site/docs/errors.html` fixes both.
3. `naming::RESERVED_ENTRIES` must be extended once more if the Swift/Kotlin/TS runtimes declare further `Undra...` types
   when they land (its test scans their sources); re-run `cargo test -p undra-bindgen --lib` after the final merge.
4. The Swift `UndraLazyListObject` is a nested `ObservableObject`, which does not publish through the store: a view must
   observe the list itself (the generated doc says so). If the runtime wants the store to forward it, that is a change to the
   generated `init` (a Combine sink) and an ADR-045 note.
5. `docs/SPEC.md` (sections 10.x for the new shapes, 12) is not edited by this sub-piece.
6. The E0006 message prints the type in the schema's compact form (`named:Meters`); it is `undra-meta`'s `Display`.

## Checks run (worktree `wt/tp-bindgen`, after merging `wt/tp-swift`, `wt/tp-kotlin`, `wt/tp-ts`; env as in the preamble)

* `cargo test -p undra-bindgen`: lib 26, diagnostics 2, generators 48, golden 21, run_ts 13, schema_hash 2, stdlib 16,
  typecheck_kotlin 1 (brew Kotlin 2.4.20 and CI's 2.0.21, `-Werror`, plus the execution mains), typecheck_swift 4 (the host, and
  the iOS 15.0 and 16.0 simulator triples, Swift 6 mode, plus the `recursive` and `newtypes` executables), typecheck_ts 21,
  validate 31, doc tests 8: 193 passed, 0 failed.
* `cargo test -p undra-cli`: 408 passed, 0 failed, 2 ignored; `cargo test -p undra-cli --test schema_docs -- --ignored`: 1 passed
  (the library route and the dev runner's route write the same bindings).
* `cargo clippy -p undra-bindgen -p undra-cli --all-targets -- -D warnings`: clean. `cargo fmt --check`: clean.
  `RUSTDOCFLAGS="-D warnings" cargo doc -p undra-bindgen --no-deps`: clean.
* `scripts/ios-floor.sh golden` and `scripts/ios-floor.sh apps` (the playground's and Fieldbook's bindings for an iOS 15 floor
  build for the iOS 15.0 simulator): pass.
* `undra bindgen -C <p> --check --docs` on the playground, Fieldbook, cookbook and two-cores a and b, and `--check` on
  ios15-sample: up to date after the regeneration committed here.
* `cargo test -p undra-ports --test schema --test opt_in`, `cargo test -p undra --test schema_diagnostics --test e2e_todo`:
  pass. `cargo test -p undra-macros --test catalogue` fails twice on E0073 (open item 2).
