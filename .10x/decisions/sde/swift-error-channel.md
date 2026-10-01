# SDE: the Swift error channel (ADR-032, wt/swift-errors, 2026-09-30)

Implements ADR-032 from `.10x/specs/2026-09-30-swift-error-channel-impl.md`. After this piece generated Swift never
traps on the outcome of a call (constitution R6 for the Swift host): a call throws exactly one of the method's own
`E`, `CancellationError` or `UndraCallError`, and a command (a synchronous method that returns `()` and has no error
type) reports through `LoadOptions.onError` and returns. The ADR's status is Accepted, with the integrator's choices
recorded at its end (commands report; the debug default of `onError` is log-only; the `UndraCore.shared`
placeholder ships here; no migration note pre-1.0).

## What was built

* **Swift runtime** (`runtimes/swift/UndraRuntime`): `Core/CallError.swift` (`UndraCallError` with its five cases and
  the texts of the brief, the four `mapped` functions, `UndraUnhandledError`), `LoadOptions.onError` (last parameter of
  `init`, `.inproc`, `.remote`), `UndraCore.report(_:operation:)` (log at error level, then `onError`, recursion guarded by a
  task-local), `UndraCore.shared` returning a shut-down placeholder over the new internal `Core/UnloadedTransport.swift`
  (`current` stays `nil`), doc sentences on `call`/`callSync`, `Errors.swift` header, README. 53 new tests
  (`CallErrorTests.swift`): every row of the `mapped` table, domain mapping, streams (E body, `"cancelled: ..."`, other
  String, garbage; with and without a domain; the String-or-E ambiguity pinned both ways), end to end through
  `UndraCore` on `FakeTransport` (statuses 2, 3, 5, garbage; cancel before send and while waiting; shutdown in flight and
  after; remote timeout; constructor), `report` (handler once, no handler, no recursion, calling thread, 8 threads x 25),
  and the placeholder.
* **Generator** (`crates/undra-bindgen/src/swift.rs`): `undraUnexpected` and every `undraFromReply` are gone; calls are
  one `do`/`catch` that throws `UndraCallError.mapped(error[, domain: E.self])`; commands call
  `<core>.report(error, operation: "<Owner>.<method>")` (free functions: `ctx.report`, operation without backticks);
  streams always pass `mapError: { UndraCallError.mapped(streamFailure: $0[, domain: E.self]) }`; every constructor is
  `throws` / `async throws`; store `apply` reports an undecodable change and skips it (the `PatchError` re-observe stays);
  `throws_doc` writes the `- Throws:` lines of the brief; `swift_typed_throws` now governs port requirements only
  (`port_throws_clause`). README, `lib.rs` doc and the `undra.toml` template comment follow.
* **Generator tests** (`tests/generators.rs`): `swift_typed_throws_only_changes_port_requirements` (replaces the old
  test), `swift_generated_code_never_stops_the_process` (all nine cases, both option values: no `fatalError`,
  `undraUnexpected`, `preconditionFailure`, `precondition(`, `assertionFailure`, `assert(`, `try!`, `as!`),
  `swift_call_shapes_follow_adr_032`, `swift_free_function_commands_report_through_their_context`,
  `swift_store_apply_reports_undecodable_changes`; the stream and async tests were updated.
* **Goldens**: nine bindgen Swift trees, the CLI's `GoldenStores` Swift tree and `examples/playground/generated/swift`
  (`undra bindgen --docs`), all regenerated through `UPDATE_GOLDEN=1` / the CLI, never by hand. `git diff --name-only`
  over the golden trees and the playground output lists only `/swift/` files. All nine bindgen Swift trees also compile
  against the runtime under Swift 6 (a scratch package, see Verification).
* **Contracts**: `scenarios.md` first (S05.6, S06.6, S15.9, S17.1/2 sentences, S17.5, S17.6, wasm S17.5, platform notes),
  then the runners. Swift: untyped `checkThrows` siblings (sync and async) and `callError` helpers in `Support.swift`;
  16 `outcome { () throws(E) }` sites moved; about 30 `try`s; `Fixture.shared.unhandled` records `onError`;
  `CapturingLog.onNextRecord(where:run:)`; S17.1 and S17.2 through the generated `explode` / `explodeLater`; the NOTES
  workaround is gone. Kotlin and TypeScript: the same steps as new coverage, generated output untouched.
  `ApplyReportTests.swift` (a generated store skips and reports a change it cannot decode) is an extra Swift-only test.
* **Playground iOS app**: `BigListScreen.run` takes `() throws -> Void` and the four closures lost `throws(ListError)`;
  `UndraBootstrap` installs an `onError` that logs through `os.Logger` (`dev.undra.playground`), the reference use (R10).
* **Docs**: `docs/SPEC.md` 10.1 and 17.3, `docs/SWIFT_ERRORS.md` (new, the guide for a Swift engineer), the runtime README,
  the bindgen README, the config template comment, `contract-tests/{swift,kotlin,ts}/NOTES.md`, and the typed-throws claims
  in `docs/blueprint.html` (six lines).

## Verification

| Check | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` (bindgen goldens included) | 2,114 passed, 0 failed, 9 ignored (2,110 at the base per the blog fact-check; +4 new bindgen tests) |
| `swift test` in `runtimes/swift/UndraRuntime` | 381 tests, 0 failures (328 before, 53 new) |
| `bash crates/undra-ffi/tests/swift/run.sh` | 1 test, passes |
| `bash contract-tests/run-all.sh` | 51/51 PASS (17 scenarios x ts, kotlin, swift), with the new steps; Swift also runs `ApplyReportTests` (18 XCTests) |
| Kotlin runtime `test-local.sh` | 454 cases, 0 failed, 2 skipped (native smoke, as before) |
| TypeScript runtime `npm test` | 26 files, 897 tests, passing; unchanged |
| `contract-tests/ts` `npm run typecheck` | clean |
| iOS: `undra build --platform ios` and `smoke.sh` | app built for the simulator; four screens alive, no fault or `dev.undra` error lines in the app's log, no crash reports; XCUITest tour 5 tests, 0 failures; `** TEST SUCCEEDED **` |
| Kotlin / TypeScript generated trees and goldens | byte-identical (`git diff --name-only f1890c9..HEAD` lists no Kotlin or TypeScript generated or runtime file) |
| Greps (DoD 1, 2) | no `fatalError`, `undraUnexpected`, `assertionFailure`, `preconditionFailure`, `precondition(` in the bindgen goldens, the CLI golden or the playground's generated Swift; no `fatalError` in the runtime sources (the codec preconditions named in the ADR stay) |

A one-off scratch Swift package (not committed) compiled every bindgen golden Swift tree (nine, `PlaygroundCore` included)
against the runtime with Swift 6 strict concurrency: only the three existing "keyword does not need to be escaped"
warnings of `Errors.swift` and `Types.swift`, none new.

## Deviations from the brief, and why

1. **The site pages are not edited** (`site/docs/api-swift.html`, `site/docs/cli.html`, `site/index.html`). `main` has moved
   37 commits since this worktree last merged it, including the site-v2 rewrite of those exact pages and their generated
   files (search index, `llms-full.txt`). Editing the old versions would only conflict. `docs/SWIFT_ERRORS.md` is the docs
   deliverable (the task message allows it). What the integrator should land after merging `main`: the Errors, Objects and
   Stores sections of `api-swift.html` (three outcomes, commands and `onError`, typed throws on ports; listings as in SPEC
   10.1), the `swift_typed_throws` comment of `cli.html` (`# port requirements: \`throws(E)\`; false emits plain \`throws\``),
   the two `async throws(TodoError)` mentions of `index.html`, blog post 4 (it describes the abort), then rebuild the
   generated site files. The page text can be lifted from `docs/SWIFT_ERRORS.md`.
2. **The CLI golden** `crates/undra-cli/tests/golden/stores/swift/.../Stores.swift` also changes (the brief lists only the
   bindgen goldens and the playground output); regenerated with `UPDATE_GOLDEN=1 cargo test -p undra-cli --test bindgen_schema`.
3. **`docs/blueprint.html`** (not in the brief): six typed-throws claims (`throws(E)`, "typed throws") would have been false;
   changed to the new shapes. It is identical on `main`, so it merges cleanly.
4. **The brief's golden check is unreliable**: `git diff --stat ... | grep -Ev '/swift/'` shows `.../Sources/...` for long
   paths, which never contains `/swift/`. I used `git diff --name-only ... | grep -Ev '/swift/'`.
5. **Kotlin S17.5 asserts a different refusal**: the Kotlin runtime's `InprocTransport` knows it is inside a callback and
   throws an `UndraException` ("called from inside a core callback") before the core can answer `E_REENTRANT`. Swift gets the
   core's bad request (`.refused`, reason contains `E_REENTRANT`), which is also the proof that the Log hook runs on the
   thread that holds the core lock (no `ManualClock` fallback was needed). `scenarios.md` S17.5 now says both.
6. **`UndraCore.registerPort` ignores a shut-down core** (like `release`, `event` and `timerFired` already did), so the
   placeholder does not accumulate registrations. This is a small behaviour change for real shut-down cores too: a late
   registration used to be stored in a dead core.
7. **Swift runner helpers**: the old, unused `checkThrows(_:_:...)` was removed (its name collides with the brief's
   untyped sibling); `callError(_:_:)` (sync and async) was added to assert an `UndraCallError` whose reason is the core's
   text; the four `success(await outcome { ... })` sites of S12/S13 became plain `try await` calls and S14's
   `Result<RemoteTodo, RemoteError>?` became `Result<RemoteTodo, any Error>?`.
8. **Extra tests** beyond the brief: `swift_free_function_commands_report_through_their_context` (no golden case has a
   free-function command, so the test extends the `objects` schema) and `ApplyReportTests` (decision 6 end to end).
9. **Operation strings** drop the backticks that escape a keyword (`Calculator.open`, not ``Calculator.`open` ``).
10. **`smoke.sh` could not save its screenshots here**: `xcrun simctl io ... screenshot` is refused with "Operation not
    permitted" when the target is the tracked `examples/playground/.proof/` under `~/Desktop` (the simulator service has no
    access there), and the first attempt overwrote the tracked `ios-smoke.log` (restored with `git checkout`). The full
    script was run from a copy with `PROOF` pointing at the scratchpad: everything else identical, PASS. No tracked proof
    file changed.

## Things worth knowing

* **`onError` and Swift tasks.** The recursion guard is a task-local, as specified. A `Task { }` started *inside* the
  handler inherits it, so reports made from that child task are only logged; `Task.detached` does not inherit it. A handler
  is meant to be short and synchronous; the `LoadOptions.onError` doc says so.
* **The apply report's wording.** A `WireError` maps to `.malformed("the reply does not decode: ...")` (the brief's text),
  which reads slightly odd for a change-set; the operation (`Counter.apply(signal: 0)`) says what failed.
* **The stream tie-break** (ADR-032, Risks) is pinned by two tests: an `E` whose bytes also read as one `String` resolves as
  `E` (an empty panic message is the four bytes of `.code(0)` for a variant-0 `UInt16` payload). A distinct wire flag for
  core-ended streams stays a v2 item.
* **`UndraCore.shared` after `shutdown()`** returns the placeholder, so `UndraLifecycle()` and `ConnectivityAdapter`
  (default `core: .shared`) no longer trap before a load either.

## What is left

* The adversarial review (`.10x/reviews/`, DoD 7) is the reviewer's, not part of this record.
* The site pages and blog post 4 (deviation 1), and the shared state files (`.10x/status.md`, `.10x/handoff.md`), are the
  integrator's.
* Merging `main` brings ADR-033 (the envelope magic) and site v2: this piece does not touch the wire, the Envelope or the
  site, so the expected conflicts are `site/*` only (none, since they are untouched here) and possibly `contract-tests/`
  line noise. Regenerate the goldens rather than editing them if a rename script or merge touches generated Swift.
* A gated `typecheck_swift` test in `undra-bindgen` (like `typecheck_kotlin`) would keep the nine golden trees compiling
  against the runtime in CI; the scratch package that proved it today is a few lines of `Package.swift`.
