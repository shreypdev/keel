# `types-paging` (ADR-042 newtypes, generic instantiations, leaf types, `Decimal`; ADR-043 polling, infinite queries, `Lazy<T>`) - adversarial review

**Date:** 2026-10-02 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:** `wt/types-paging` at `22e3a44`
(94 commits; `main` `a4c7cb2` already an ancestor, and `main` had not moved when the review merged it) · **Read:** `CLAUDE.md` (R1, R3, R4, R5,
R8, R9, R11, R12), ADR-042 and ADR-043 with their deviation sections, ADR-031's amendment, ADR-052's notes, `.10x/decisions/sde/types-paging.md`
and the seven sub-piece records, SPEC 3.1/3.3/3.5, 10.3c, 11.1, 16.3, 17, the guides `site/docs/{types,paging,polling}.html`, and the diff:
the macros (newtypes, `generic`, `__instantiate!`, the leaf spellings, `Lazy`, `infinite`/`interval`), `undra-meta`, the bindgen emitters and
goldens, `undra-wire` (`Decimal`, the leaf features, the lazy payloads), `undra-signals` (`Lazy`, `Lazy::over`, `sort_ids`), `undra-runtime`
(the page server), `undra-query` (`paged.rs`, `poll.rs`), the three runtimes' mirrors, lazy lists, decimal codecs and React hooks,
`undra-compose`, the playground's `ledger.rs`/`paging.rs` and screens, S31-S33 · **Fixes:** four `fix(types-paging): review fixes` commits,
two review-test commits, one docs commit, then this record.

## Verdict

**Sound with fixes; merge.** The two ADRs are implemented whole and the riskiest change, ADR-031's amended fold rule, is right in all three
mirrors: the review put op 2 into the Kotlin and Swift model harnesses (TypeScript's already had it), ran 30,000 / 20,000 / 20,000 random
histories clean, and reverting the amendment in any one mirror fails its model within a few seeds. One **High**, fixed: on Kotlin and Swift a
`Decimal` whose mantissa passed 127 bits only because of digits after the point saturated to 1.7 x 10^38 at scale 0, so `10 / 3` divided to
scale 38 (Kotlin) or a 39-digit parse (Swift) crossed the wire as 170141183460469231731687303715884105727: a silent change of 38 orders of
magnitude in a money type (H1). Two **Medium**, fixed: every platform let a lazy list's `pageSize` go far past the core's cut of a page call at
4,096 rows, and then every page came back short, was reported as a protocol error and never loaded (M1); and the TypeScript mirror applied a
lazy invalidation to a signal waiting for a full value, where Swift and Kotlin drop it as the amendment says (M2). Each fix has a test that
fails without it. Low findings are open items, not blockers (L2-L6). Sizes hold: the hello wasm is 116,480 (gate 120,000) and the JavaScript
up front 22,068 against its 22,100 gate, restated with a dated ADR-052 note.

## Findings

| # | Sev | Where | Finding | Status |
|---|---|---|---|---|
| H1 | High | `runtimes/kotlin/undra-runtime/runtime/src/main/kotlin/dev/undra/runtime/wire/Codecs.kt:171` (`decimal.encode`), `runtimes/swift/UndraRuntime/Sources/UndraRuntime/Wire/DecimalCodec.swift:115` (`undraEncode`) | Both encoders saturated **any** mantissa of more than 127 bits to `i128::MAX`/`MIN` at scale 0. A value whose whole part is small but whose digits after the point make the mantissa long - `BigDecimal.TEN.divide(BigDecimal(3), 38, HALF_UP)` (3.33.., a 39-digit mantissa), `BigDecimal(2).setScale(38)`, Foundation's `Decimal(string: "3.40282366920938463463374607431768211455")` (it keeps 39 digits at exponent -38) - crossed as 1.7 x 10^38. The ADR documented "saturates a mantissa of more than 128 bits", which described the code, not a sane result. | **Fixed** (`890aa7b`): such a mantissa loses its last digits after the point (Kotlin rounds half up once, from the original value; Swift cuts, as each already did past the 38th) and only a whole part of 2^127 or more saturates. New Kotlin `DecimalCodecTests` (every scale with zero, ±1, 10 and both extreme mantissas; 2,000 random round trips; negative scale; rounding past the 38th; the long mantissa; saturation) and a Swift case, both failing before. SPEC 17, the types guide and ADR-042 deviation 6 updated. |
| M1 | Medium | `runtimes/ts/@undra/runtime/src/lazy.ts:29`, `runtimes/kotlin/.../UndraLazyList.kt:536`, `runtimes/swift/.../Lazy/UndraLazyListEngine.swift:72`; the core's cut at `crates/undra-runtime/src/lazy.rs:137` | SPEC 3.3 cuts a page call's `limit` to 4,096; the settable `pageSize` went to 65,535 (TS), 65,536 (Kotlin) and 1,048,576 (Swift). With 5,000 against a core that cuts, measured: row 4,500 stays `undefined` and `onError` hears "the core sent 4096 rows for page 0 of a list of 10000, which holds 5000" for every page. | **Fixed** (`b94c4c0`): 4,096 on all three (TS `RangeError`, Kotlin `IllegalArgumentException`, Swift clamps, each platform's existing style); the TS fake page server now cuts like the core and a new case reads a whole 4,096-row page (fails before: the setter accepted 4,097). ADR-043 deviation 5 updated. |
| M2 | Medium | `runtimes/ts/@undra/runtime/src/mirror.ts:585` (`_fold`) | The amendment says "a signal waiting for a full value drops an invalidation like a patch"; Swift (`Mirror.swift` `!= .fullValue`) and Kotlin (`op == PATCH_CODE \|\| op == INVALIDATED_CODE`) do, TypeScript dropped only patches, so an op 2 cleared the wait and was applied against a page server the host had not been told about. Reachable only after a resync of a lazy signal (a malformed patch, a compaction drop), so the impact is small, but the three mirrors disagreed on the rule the whole piece hinges on. | **Fixed** (`32129ff`): `!== FullValue`, as Swift; a coalesce case fails before. |
| L1 | Low | `crates/undra-bindgen/src/model.rs:781` | `setPollInterval` silently clamps to 1 s .. 7 days in the core (ADR-043 deviation 3), but the generated Swift/Kotlin/TS docs did not say so: a 100 ms override polls at 1 s with nothing in the API to tell. | **Fixed** (`09e04a8`): one doc sentence; goldens, the five example packages and the site's API reference regenerated (docs only, no hash moves). |
| L2 | Low | `runtimes/kotlin/.../UndraLazyList.kt:433` (`requestFailed`), `runtimes/swift/.../UndraLazyListEngine.swift:410` (`failed`) vs `runtimes/ts/@undra/runtime/src/lazy.ts:401` | A status-5 refusal of a page call (the page server is gone: a restore's new handle not delivered yet, or a store closed with a page asked for on an asynchronous transport) is quiet on TypeScript and **reported through `onError`** on Kotlin and Swift. All three recover without app code (the next full value resets the list; Kotlin/Swift also drop in-flight replies of an older epoch), so the cost is a spurious error report, not a stuck list. | Open: make Kotlin and Swift treat `BadRequest` on a page call as TypeScript does. |
| L3 | Low | `lazy.ts:27` (`MAX_STALE_REPLIES = 2`), `UndraLazyList.kt:539` (4), `UndraLazyListEngine.swift:74` (3) | The bound on "a page answered below the list's version" differs per platform. On an ordered transport a stale reply needs a change-set to overtake a reply, which only the microtask between a reply's drain and its continuation allows; under a list that changes every round trip the page is marked failed and a protocol error reported until the next change re-asks it. | Open: one number on the three, and a churn case in each suite. |
| L4 | Low | `crates/undra-query/src/poll.rs:213` (`reschedule_poll`, `Base::Keep` with no armed timer) | On `Active` a fresh entry gets a timer a **full interval from the resume**, even when its last fetch ended more than an interval ago (the timer was cancelled on `Background`); a stale entry fetches at once. That is one reading of ADR-043 1.2 ("resume with the existing triggers, then reschedule from the end of that fetch") and the doc comment says so, but a polled query with a long `stale` can show data up to two intervals old after a short trip to the background. Both transitions are tested as implemented (S33, `poll` tests). | Open: decide in ADR-043 whether resume keeps the old base (`max(0, base + interval - now)`). |
| L5 | Low | `.github/workflows/ci.yml` | `undra-compose`'s JVM unit tests (11) and instrumented tests (2) run nowhere in CI; its main sources compile only through the playground's `:app:assembleDebug` (Kotlin 2.0.21 + the Compose plugin). Same as `:android-adapters` today. Both pass here. | Open: a CI step `./gradlew :undra-compose:testDebugUnitTest`. |
| L6 | Low | `crates/undra-wire/src/leaf.rs:266` | A core typed with `rust_decimal::Decimal` refuses a wire value past its 96-bit mantissa or scale 28, as ADR-042 says - but a Swift `Decimal(1) / Decimal(3)` has scale 38, so the ordinary Swift result of a division is a refused call there. | Documented (the types guide now says so); open: consider rounding to scale 28 on decode instead of refusing. |
| T1 | Coverage | `CoalesceModelTests.kt`, `CoalesceModelTests.swift` | The Kotlin and Swift ADR-031 model harnesses had no op 2 (only unit cases); TypeScript's had. | **Added** (`32129ff`): a `Lazy` signal per store (invalidations, page-server restarts, one entry per signal per change-set) in both; mutants killed on all three. |
| T2 | Coverage | `crates/undra-signals/src/lib.rs:112` (`sort_ids`) | A hand-written Shell's sort replaced three `slice::sort` instantiations for size, with one doc test. | **Added** (`1ef938b`): proptests against `sort_unstable` (any length, few distinct values) and reversed/nearly-sorted lists of every length to 200. |
| T3 | Coverage | `crates/undra-signals/tests/lazy.rs` | No case for a `Lazy::over(&derived)` whose index rebuilds (past the 4,096 recorded ops of one commit) while a page is out. | **Added** (`e812115`): 5,000 pushes in one transaction; the announcement's version is newer than a page read before it, and pages after it are at that version and match the model. |

## The attack, surface by surface

**1. ADR-031's amended fold rule (highest risk).** Read the three implementations side by side: TypeScript keeps `full` as `[Full]`, `[Inv]` or
`[Full, Inv]`; Swift and Kotlin keep `full` and `invalidated` slots, applied full -> patches -> invalidation (Swift) and full -> invalidation ->
patches (Kotlin) - equivalent, since a signal is a keyed list or a lazy list, never both. Compaction and requeue carry both entries on all three.
The **models**: TypeScript already alternated invalidations and page-server restarts; the review added the same alphabet to Kotlin and Swift
(the core sends a full value when a restart and an invalidation fall in one transaction, as a real core does) and ran **30,000** (TS),
**20,000** (Kotlin, under 2.4.20 and 2.0.21) and **20,000** (Swift) histories: clean. **Mutants:** `setFull` for op 2 in Kotlin's fold, Swift's
fold and TypeScript's `Slot.setFull` each fail the model (seeds 3, 5 and 3: the host keeps the old handle) and the unit cases. The specific
shapes are unit-tested on all three: `[Full, Inv]`, `[Full, Inv, Inv]`, `[Inv, Full]`, `[Full(h1), Inv, Full(h2), Inv]`, `no_coalesce`, a
compaction keeping the pair, and - after M2 - an invalidation while waiting for a full value. **Stale pages** (the list side): a reply whose
version is below the list's is dropped and re-asked on all three (TS `lazy.ts:432`, Kotlin `:469`, Swift `:454`), with cases for a reply
overtaken by an invalidation, an invalidation inside a `callSync` drain, two invalidations folded into one while a page is in flight (the page
is re-asked once at the newest version), and a first value plus an invalidation in one drain (the handle survives). Bounds differ (L3).

**2. `Decimal`.** Rust: proptests round-trip every `i128` at every scale, numeric order is scale-independent, scale 39-255 rejected; the vectors
(`decimal_*`, 17 entries) are identical in `contract-tests/` and Swift's resources and the Kotlin `WireVectors.kt` regenerates unchanged.
Equality: Rust, Kotlin (`BigDecimal.equals`) and TypeScript (`Decimal.equals`) are structural (`1.0 != 1.00`), Swift's `Foundation.Decimal`
is numeric for both `==` and hashing (probed: `Set([1.0, 1.00, 1]).count == 1`); `Decimal` is never a map key, so no platform's hash crosses a
boundary, and the guide and SPEC 17 now say Swift differs. **Edges, enumerated:** TS throws `RangeError` for an out-of-range mantissa or scale
(no wrap); decoding is exact everywhere (Swift's 128-bit mantissa holds `i128::MIN`, `-0` cannot be encoded, a zero mantissa is never Foundation's
NaN); Swift encodes NaN as zero (documented), cuts digits past the 38th; Kotlin rounds them half up and turns a negative scale into whole digits;
both saturated far too eagerly (H1, fixed). `rust_decimal` decodes outside its range as `WireError`, never a panic (L6). `wire/decimal/roundtrip`
measured **43 ns** (budget 250).

**3. Newtypes and generics.** A newtype adds a `RecordDef { transparent: true }`, so a core using `Price(u64)` does **not** hash like one using
`u64` - intended (R1: the schema describes the type; the wire bytes are equal, ADR-042 1.2/1.3), and an unflagged schema hashes as before.
Generated shapes read native (Swift `RawRepresentable` structs, `Hashable`, `Comparable` only for ordered inners; Kotlin `@JvmInline value
class`; TS branded types) and the goldens type-check (Swift), compile with `-Werror` (Kotlin) and `tsc` under `UNDRA_REQUIRE_TOOLCHAINS=1`.
Instantiation names are the alias's (`TodoPage`), so two builds hash the same; a second alias of one instantiation is E0070 (trybuild golden);
an instantiation named like a user type is **E0050** (duplicate type name) at schema validation, not E0070 - correct, worth knowing. A
`HashMap<Price(Decimal), _>` is E0006 at compile time (`e0006_newtype_keys` golden). **Leaf features:** the CI `leaf-features` job, run here:
every feature on, `undra-wire` 244 / `undra` 41 / macros 240 tests; each feature builds for wasm32, `aarch64-apple-ios`, `aarch64-linux-android`;
`cargo tree` with all five on wasm32 has no `getrandom`, `wasm-bindgen`, `js-sys` or `iana-time-zone` (R12). Hello wasm with all features off:
**116,480**, as claimed.

**4. Lazy lists and infinite queries.** A page call on a stale handle is status 5 (`runtime.rs:1839`); each platform recovers on the next full
value (L2 for the report). `Lazy::over` rebuilds are announced at a newer version than any page read before (T3). Memory: at most
`maxCachedPages` x `pageSize` rows on the host (24 x 50 by default; eviction is tested on all three; the window is never evicted before a page
outside it); the core encodes a page on request (`lazy/page_50_of_100k` **256 ns**, ratio to 10k 1.01). Browser pane, playground web: the
Library scrolled to row 10,000, then "Add 100 rows" and "Rename": length 10,100, version 101, rows 10,086-10,100 loaded; the Feed went 50 -> 250
rows over four scrolls of `useLoadMore`; the Ticker polled 2 -> 8 in six seconds at 1 s; no console errors. StrictMode: `useLoadMore`'s observer
is disconnected by the first cleanup before it can fire and `fetch_next_page` is a no-op while anything is in flight (`paged.rs:615`); both have
cases. `undra-compose`: Gradle (Kotlin 2.0.21 + the Compose plugin; brew's 2.4.20 is not used by Gradle): 11 JVM unit tests and 2 instrumented
`LazyColumn` tests on the `undra` AVD pass (L5). `persist_pages` across builds: migrated by name or dropped, tested (`infinite.rs:1228`, `:1286`).

**5. Polling.** The timer is a task on the Timer port holding a `WeakCtx`; times come from the Clock port; nothing in `undra-query` or
`undra-signals` reads `std::time` outside tests (R12). Paused on `Background`/`Inactive` (unless `poll_in_background`) and offline through one
function (`poll.rs:149`); resume semantics in L4. The interval is clamped, not refused, by the accepted deviation; the docs now say so (L1).
**Linking:** polling is in every core with a query (about 1.35 KB) because `set_poll_interval` is a fixed method of every query handle, so any
observer may poll any query and no static use-analysis can drop it; acceptable, the hello web core is 3.5 KB under its gate.

**6. Sizes and the matrix (once, on the fixed tree).** `scripts/wasm-size.sh`: **web/hello-wasm 116,480** gzipped (gate 120,000; record
119,654), **web/hello-runtime-js 22,068** (gate 22,100; record 22,005). The JavaScript is within 39 bytes of its gate, so ADR-052 has a dated
review note (the +63 bytes are the amended fold, the page-call target in `UndraCore.call`/`callSync`, and M2's fix; `LazyList`, the hooks and
`Decimal` are not in the hello chunk) and keeps the budget; the integrator re-records on `main`'s path.

## Counts (this worktree, after the fixes)

| Suite | Result |
|---|---|
| `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `clippy -p undra-ffi --target wasm32`, `cargo doc -D warnings` | clean |
| `cargo test --workspace` | **3,518** passed, 0 failed, 21 ignored (+3 `sort_ids`; the T3 case ran in `undra-signals --test lazy`, 24 passed) |
| write rule in release | 9 + 9 |
| Swift runtime (`swift test`) | **862** (+1) |
| `scripts/ios-floor.sh` (ObservableObject mode) | runtime for iOS 15.0 and macOS 12, every golden case at iOS 15.0/16.0, the iOS 15 sample, the playground's and Fieldbook's floor bindings, the Swift grid on floor bindings **31/31**, the testkit 5, the observation probe; the iOS 15/16 simulator step skips (no such runtime installed) |
| Kotlin runtime (`test-local.sh`) | **880** cases, 0 failed, 2 skipped (+4 `DecimalCodecTests`) + testkit 32, under brew's 2.4.20 **and** CI's 2.0.21 |
| `undra-compose` | 11 JVM unit + 2 instrumented (`undra` AVD) |
| TypeScript runtime | **1,847** (+1 mirror, +2 page size, one per transport mode); typecheck clean |
| React Native | 109 unit, typecheck clean, contract 24 + 2 skipped (S17, S29 app-tested), `cpp/test/run.sh` 30 checks |
| Contract grid (`contract-tests/run-all.sh`) | **95/95** (33 TS, 31 Kotlin, 31 Swift; S31-S33 pass on all three) |
| Model harnesses | TS 30,000, Kotlin 20,000, Swift 20,000 histories with op 2; three mutants killed |
| Interop | wasm harness 22 + 36, C (smoke, lifetime, two cores), Swift over the C ABI 6, JNI 16 |
| `undra bindgen --check --docs` | playground, cookbook, fieldbook, two-cores a and b current; ios15-sample current (`--check`) |
| `schema_retention`, `schema_docs` (`--ignored`) | 1 + 1 |
| Budgets (`--release`) | pass; new rows: `wire/decimal/roundtrip` 43 ns, `lazy/page_50_of_100k` 256 ns, `lazy/view_page_50_of_100k` 2.7 us, `lazy/invalidate` 157 ns, `query/infinite_append_page_50` 6.3 us vs `query/keyed_push_50` 4.9 us (ratio 1.29, gate 2) |
| Leaf features (CI job's steps) | all green; no clock or randomness on wasm32 |
| Playground web | 135 tests, `npm run build`; Library/Feed/Ticker in the browser pane as above |
| Android | playground `:app:assembleDebug` builds |
| Site | `build-all` regenerated (API reference, search index, llms-full), `check-links --words` passes (landing 342 of 350) |
| Playground iOS (iPhone 17 Pro, iOS 26.5) | `undra build --platform ios`, then the XCUITest tour's `testLibrary`, `testFeed`, `testTicker`: 3 passed; by hand, the Library's `LazyVStack` scrolled through rows 1-52 with every row filled as its page arrived |

**Not run, and why:** Miri - no nightly toolchain on this machine, and no `unsafe` changed (`undra-ffi` gained only the `lazy_alloc` test; the
wire code is `forbid(unsafe_code)`). The iOS Library was **not** scrolled to row 10,000: the simulator's swipes move about a dozen rows each and
the scroll indicator would not take a drag, so only the web Library was taken to the end (row 10,100). The `smoke.sh` wrapper was not used
because it shuts the simulator down at the end; its UI tests for the three new screens were run directly instead.

## Open items

1. L2: Kotlin and Swift should treat a refused page call (status 5) as TypeScript does, without an `onError` report.
2. L3: one stale-reply bound on the three platforms, and a churn case in each suite.
3. L4: ADR-043 should say whether a resumed poll counts from the last fetch or from the resume.
4. L5: CI runs `:undra-compose:testDebugUnitTest` (and `:android-adapters:test`, which it does not either).
5. L6: decide whether a `rust_decimal` core rounds a wire value with scale above 28 instead of refusing it.
6. The integrator re-records `web/hello-runtime-js` and `web/hello-wasm` on `main`'s path; 32 bytes of JavaScript headroom remain.
