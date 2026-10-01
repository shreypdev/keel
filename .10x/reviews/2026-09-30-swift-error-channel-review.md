# Swift error channel (ADR-032) — adversarial review

**Date:** 2026-09-30 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/swift-errors` at `39cd257`, merged with `main` (`8820442`: ADR-033 wire magic, site v2; conflicts only in
`.10x/decisions/sde/_index.md`, both lines kept; `docs/blueprint.html` merged cleanly, only this branch edits it) ·
**Read:** `CLAUDE.md`, ADR-032 (accepted, with the integrator's choices), the implementation brief
`.10x/specs/2026-09-30-swift-error-channel-impl.md`, the SDE record `.10x/decisions/sde/swift-error-channel.md`,
`docs/SPEC.md` 6, 10.1, 17.3, `docs/SWIFT_ERRORS.md`, the runtime sources, `crates/undra-bindgen/src/swift.rs`, the nine
bindgen Swift goldens, the CLI golden, `examples/playground/generated/swift`, `contract-tests/scenarios.md` and the three
runners, `CallErrorTests.swift`.

## Verdict

The piece does what ADR-032 decides: no generated Swift and no runtime path reachable from generated code traps on the
outcome of a call, and every failure a Swift app can observe is a typed value. Two places broke the narrower promise that a
call throws exactly one of `E`, `CancellationError` or `UndraCallError` (M1, M2), both on the remote transport and both
fixed here; the rest are Low and fixed. No High or Medium finding is open.

## How the paths were enumerated (attack 1)

Every way a failure reaches generated code or the runtime's public surface, and where it ends:

| Source | Raised at | Reaches generated code as | Ends as |
|---|---|---|---|
| reply status 1, 2, 3, 5, `streamOpened`, ok-as-failure | `UndraCore.unwrap` (`UndraCore.swift`, `unwrap`), `onReply` stream branch | `UndraReplyError` | `E`, `.panicked`, `.cancelledByCore`, `.refused`, `.malformed` (`CallError.swift` `fromReply`) |
| `undra_call` returns non-zero (in process) | `send(call:) == false` | `UndraReplyError(.badRequest)` from `rejection()` | `.refused` |
| send refused by a closed remote transport, or after `shutdown()` raced the call | `send(call:) == false` | was `.refused`; now `UndraTransportError.closed` (`notSent()`, M2) | `.unavailable(.closed)` |
| this core shut down / placeholder | `reserveCallId` | `UndraTransportError.closed` | `.unavailable(.closed)` |
| in-flight calls at `shutdown()` | `failAllPending` | `UndraTransportError.closed` | `.unavailable(.closed)` |
| remote timeout / connection lost | `blockingCall`, `WebSocketTransport.connectionLost` | `UndraTransportError` | `.unavailable` |
| remote core reloaded with another schema | `WebSocketTransport.swift:267` `onDisconnect(UndraSchemaMismatchError)` | `UndraSchemaMismatchError` | was returned **unchanged**; now `.unavailable(.connectionLost)` (M1) |
| undecodable reply / stream item | `decodeReply`, `onReply`, `onStreamItem` | `UndraProtocolError` | `.malformed` |
| undecodable result, handle, `E`, stream item body | generated `undraDecoded`, `mapped(_:domain:)` | `WireError` | `.malformed` |
| null handle | `construct` (sync); async constructors now too (L2) | `UndraProtocolError.nullHandle` | `.malformed` |
| Swift task cancelled | `Task.checkCancellation()`, `CallSlot.cancel` | `CancellationError` | itself, never wrapped |
| stream consumer cancelled / dropped | `StreamChannel.consumerGone` | `nil` (end) | loop ends quietly |
| two concurrent `next()` on one iterator | `StreamChannel.decide` | `UndraTransportError.closed` | `.unavailable(.closed)` |
| remote `callSync` inline | `WebSocketTransport.callSync` | `UndraModeError` (unreachable: remote never takes the inline path) | `.refused` |
| undecodable change-set entry | generated `apply` | `WireError` | skipped and reported (`report`), now skipped whole (L1) |
| `UndraCore.shared` with nothing loaded | `UndraCore.swift` `shared` | placeholder core | `.unavailable(.closed)` |

Greps over the runtime sources, the nine bindgen goldens, the CLI golden and the playground's generated Swift for
`fatalError`, `preconditionFailure`, `precondition(`, `assertionFailure`, `assert(`, `try!`, `as!`, `unsafelyUnwrapped` and
force unwraps (`x!`, `)!`, `]!`): **the generated trees have none**; the runtime has only the items below.
`swift_generated_code_never_stops_the_process` enforces the generated half for every golden case and both values of
`swift_typed_throws`.

**Codec preconditions and asserts that remain (judged):**

| Site | Guard | Reachable from a reply or malformed input? |
|---|---|---|
| `Wire/UndraWriter.swift:144` `writeLen` | length fits in `u32` | No. Host-side encoding only: an app would have to pass a single argument over 4 GiB. Host misuse, like Swift's own `Array` limits. |
| `Wire/UndraReader.swift:254`, `:264`, `:281`, `:290` `readRaw`/`readSlice`/`skip`/`readSubReader` | `count >= 0` | No. Every runtime caller passes a `readLen()` result, which is a `u32` already checked against `remaining` (`Envelope.swift:139-140`, `Payloads.swift:345-346`, `:728-729`). |
| `Wire/UndraTypes.swift:209` `UndraUUID.byte(at:)` | index in `0..<16` | No. Called with the constants 0 to 15 only. |
| `Core/UndraCore.swift:657` `checkMethod` | `assert(id == method)` (debug only) | No. Generated code always passes the same id twice. |

Also checked for hostile-input traps the grep does not see: `readLen` converts a `u32` already bounded by `remaining`
(`UndraReader.swift:201-208`), so no `Int` conversion can trap on 32-bit; keyed patches validate every index before any
`Int(...)` conversion or mutation (`KeyedPatch.swift:133-176`); `UndraTimestamp.init(_ date:)` saturates. The fuzz suites
cover the rest.

## Findings

| # | Sev | Area | Where | Finding | Status |
|---|---|---|---|---|---|
| M1 | Medium | Mapping | `Adapters/WebSocketTransport.swift:267`; `Core/CallError.swift:78-106` (`mapped`) | When `undra dev` comes back with another schema, the remote transport fails every pending call with `UndraSchemaMismatchError`. `mapped` returned it unchanged, so a generated call threw a fourth type (breaking SPEC 10.1's "exactly one of three") and a command reported it as `.malformed("UndraSchemaMismatchError(...)")`, which the docs call an Undra bug. Reached on every schema-changing hot reload. | **Fixed**: `mapped` turns it into `.unavailable(.connectionLost(reason: <the mismatch text>))` (`CallError.swift:91-95`); the raw API is unchanged. Test `testASchemaChangeOfTheRemoteCoreIsUnavailable` (with and without a domain). SPEC 17.3 and `docs/SWIFT_ERRORS.md` name the case. |
| M2 | Medium | Mapping | `Core/UndraCore.swift` `call` (`:294`), `openStream` (`:386`), `blockingCall` (`:613`) | Every `send(call:) == false` became `rejection()`, an `UndraReplyError(.badRequest)` "the core rejected the call without a reply (malformed, duplicate call id, or shut down)". The remote transport returns `false` only once its connection is closed, so after a dev-server restart every later call failed as `.refused` while the call in flight at the same moment was `.unavailable(.connectionLost)`. ADR-032 maps a closed remote connection to `.unavailable`, as Kotlin (`UndraException` "closed") and TypeScript (`UndraTransportError("closed")`) do. The same misclassification hit an in-process call that raced `shutdown()` between `reserveCallId` and `send`. | **Fixed**: `notSent()` (`UndraCore.swift:700-709`) answers `UndraTransportError.closed` when the transport is remote or the core is shut down, `rejection()` otherwise. The raw remote `callSync`/`call` now throw `.closed` there (documented `- Throws:` already allowed it). `CoreCallTests.testBlockingCallRejectedByTheCoreThrowsBadRequest` asserted the old, unreachable case and is now `testBlockingCallTheRemoteTransportCannotSendThrowsClosed`; new `testACallTheRemoteTransportCannotSendIsUnavailableNotRefused` (async, sync, stream). |
| L1 | Low | Mirror | `crates/undra-bindgen/src/swift.rs` `store_apply` (`:1146-1156`) | A full value was assigned to the `@Observable` property **before** `reader.finish()`; a change with trailing bytes was stored and then reported, not skipped as ADR-032 decision 6 says. | **Fixed**: decode into `value`, `finish()`, then store. All Swift goldens, the CLI golden and the playground regenerated; `ApplyReportTests.testAChangeWithTrailingBytesIsSkippedWholeNotHalfApplied` (fails on the old shape); generator test asserts the order; post 4's verbatim `apply` excerpt updated. |
| L2 | Low | Mapping | `swift.rs` `constructor` (`:967-970`) | Async constructors decoded the handle without the null check `UndraCore.construct` makes for sync ones, so `.malformed` (ADR-032's "the null handle") held only for sync constructors. Pre-existing. | **Fixed**: `if handle.isNull { throw UndraProtocolError.nullHandle }` inside the `do`, mapped like every other failure. |
| L3 | Low | R3 docs | `swift.rs` `callable` (`:1041`, `:1074`), `COMMAND_DOC`, `stream_doc` (`:1486-1503`) | Methods named their three failures in `- Throws:`, but a stream method said nothing about what iterating throws, and a command (`toggle(id:)`) gave no hint that failures go to `onError`; a Swift engineer reading a non-throwing signature assumes it cannot fail. | **Fixed**: `- Note: Iterating throws ``E``, or ``UndraCallError`` …; cancelling the iterating task ends the loop quietly.` and `- Note: A failure is logged and passed to `LoadOptions.onError`; the method does not throw.` Asserted in `swift_call_shapes_follow_adr_032`. |
| L4 | Low | R3 API | `Core/CallError.swift:192` | `UndraUnhandledError` was not `LocalizedError`, so `localizedDescription` (what an app would show) read "The operation couldn't be completed. (UndraRuntime.UndraUnhandledError error 1.)". | **Fixed**: `LocalizedError`, `errorDescription == description`; tested; SPEC 17.3 updated. |
| L5 | Low | Blast radius | `Core/UndraCore.swift:484-497` `registerPort` (deviation 6) | Ignoring a registration on a shut-down core is safe for the real shutdown path (`isShutDown` is terminal, the only runtime caller is `install` on a fresh core, and the transports already dropped registrations after their own shutdown, so the old code stored an impl nothing could call). What it could hide is an app registering on `UndraCore.shared` before `load`: silently ignored, the port then answers `unavailable`. | **Fixed**: the ignored registration logs a warning naming the port and the remedy; doc comment and SPEC 17.3 say so. |
| L6 | Low | Docs / tests | `Core/UndraCore.swift:139-141` (`current`), `docs/SWIFT_ERRORS.md` | `current == nil` while `.shared` succeeds is by design (the placeholder) but only `shared`'s doc said so. The task-local recursion guard is inherited by a `Task { }` started inside the handler (reports from it are only logged; `Task.detached` starts clean): documented on `LoadOptions.onError`, absent from the guide, untested. | **Fixed**: `current` says to check it rather than `shared`; the guide states both behaviours; `testATaskStartedFromTheHandlerInheritsTheGuardButADetachedOneDoesNot`. |
| L7 | Low | CLI docs | `crates/undra-cli/src/config.rs:103-105` | The `swift_typed_throws` field doc still said "Emit `throws(E)` in Swift". (The template comment, `lib.rs` and the bindgen README were already right; no `--swift-typed-throws` CLI flag exists, and SPEC 10.1 no longer claims one.) | **Fixed**: "on Swift port requirements (calls always use plain `throws`)". |
| L8 | Low | Docs | `docs/blueprint.html:296` | The comparison table's Undra cell still listed "typed throws" (deviation 3 fixed six other lines). | **Fixed**: "`async throws` with your error enums". |
| L9 | Low | Playground | `examples/playground/ios/PlaygroundApp/Screens/BigListScreen.swift:114-116` | The `run` comment said a sync call can fail with "a cancelled task … `CancellationError`"; a synchronous call cannot. | **Fixed**. |
| L10 | Low | Site | `site/docs/api-swift.html`, `site/docs/cli.html`, `site/index.html`, posts 1 and 4 (deviation 1) | The site still documented typed throws on calls and the abort; post 1 (`undra-vs-kotlin-multiplatform`) also showed `async throws(TodoError)` and "typed `throws(TodoError)` enums", which the brief did not list. Site v2 had already removed the landing's two `throws(TodoError)` mentions. | **Fixed** in `docs(site): Swift error channel — …` (Errors / Objects / Stores / loading sections, the CLI comment, the landing's catch comment naming the three outcomes, post 4's paragraph and excerpt, post 1's excerpt and sentence, the claims-ledger note on P1). Landing prose 332/350. |
| I1 | Info | Wire | ADR-032 Risks | A stream error item is `E` or a `String` with no flag between them; `mapped(streamFailure:domain:)` tries `E` first. Pinned both ways by tests. | Open by design (v2 wire item, R7). |
| I2 | Info | CI | `crates/undra-bindgen` | No gated test compiles the nine Swift goldens against the runtime (the SDE proved it once with a scratch package). The contract runner compiles the playground's tree only. | Open: recommend a `typecheck_swift` test like `typecheck_kotlin`. |

## The ten deviations of the SDE record

1. Site pages left to the integrator: **accepted** (they had been rewritten on `main`); landed by this review (L10).
2. CLI golden regenerated: **accepted**; it is generated by the same Swift generator, and it changed again here through `UPDATE_GOLDEN=1 cargo test -p undra-cli --test bindgen_schema`.
3. `docs/blueprint.html`: **accepted**; one more line fixed (L8).
4. The brief's `git diff --stat | grep` check is unreliable: **accepted** (`--stat` abbreviates long paths). `git diff main --name-only` over every generated and golden tree and both runtimes lists no Kotlin or TypeScript file.
5. Kotlin S17.5 sees the runtime's own guard (`InprocTransport.kt:181`) instead of `E_REENTRANT`: **accepted**; verified in the Kotlin runtime, and `scenarios.md` S17.5 states both refusals and why; the Swift column's `E_REENTRANT` is also the proof that the Log hook runs under the core lock.
6. `registerPort` ignores a shut-down core: **accepted**, with a warning added (L5).
7. Swift runner helpers: **accepted**; `checkThrows` checks the dynamic type and the value, `callError` rejects `CancellationError` and the method's own `E`.
8. Extra tests: **accepted**.
9. Operation strings without keyword backticks (`Calculator.open`): **accepted**; that is how a Swift engineer writes the name in prose.
10. `smoke.sh` screenshots: **accepted** (environmental). Not re-run by this review: the app's only change here is a comment, and the playground's generated Swift compiles in the contract runner.

## Attacks 2 to 8, briefly

* **Mapping (2).** Every row of the brief's table holds (`CallErrorMappingTests`). Status 1 with an `E` that does not decode is `.malformed("a <E> that does not decode (n bytes)")`; a typed reply on a method without `E` is `.malformed`; `CancellationError` is returned by identity and the generated `catch` rethrows what `mapped` returns, so it is never wrapped or swallowed. Streams: `mapError` is always passed; consumer cancellation resumes the parked `next()` with `nil` (`StreamChannel.consumerGone`), so the loop ends quietly; `E` survives through `domain:`.
* **Commands (3).** A command's `catch` calls `core.report` and returns; `report` never throws or traps, logs at error level, calls `onError` once on the calling thread (`testTheHandlerRunsOnTheCallingThread`), and a nested report on the same thread or task is only logged. A command refused because the core is shut down reports `.unavailable(.closed)`, exactly what a throwing call on that core throws (S17.6 asserts both). On the placeholder a command only logs, which `shared`'s doc states.
* **Ordering (4).** Unchanged: generated stores write properties only in `apply`; mapping happens after `callSync`/`call` returns; no generated path flushes or skips the mirror; S05.3 still sees zero change-sets after a refused `add`.
* **Native review (5).** Signatures are `throws` / `async throws` with `- Throws:` naming the three kinds; `UndraCallError` is `Error, Sendable, Equatable, CustomStringConvertible, LocalizedError`; the playground calls commands from `Button` and a `Binding` setter without `try`; `swift_typed_throws` now only reaches `port_throws_clause`.
* **Contracts (6).** S05.6, S06.6, S15.9, S17.1 (generated `explode`, NOTES workaround gone), S17.5 and S17.6 exercise the paths on Swift and Kotlin, the wasm S17.5 on TypeScript; nothing is skipped; the Swift runner's 17 scenarios plus `ApplyReportTests` (2) pass.
* **Placeholder (7).** `UndraCore.current == nil` while `.shared` returns the placeholder: by design, now documented on both (L6). The placeholder never becomes `current`, never reaches `undra_*` (`UnloadedTransport`), and keeps no registrations.
* **Docs (8).** SPEC 10.1 and 17.3 match the code after the edits above; `docs/SWIFT_ERRORS.md` reads as a guide for a Swift engineer; the blueprint's typed-throws claims are now true.

## Verification (after the fixes)

| Check | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | 2,114 passed, 0 failed, 9 ignored |
| `swift test` in `runtimes/swift/UndraRuntime` | 384 tests, 0 failures (381 + 3 new) |
| `bash crates/undra-ffi/tests/swift/run.sh` | 1 test, passes |
| `bash contract-tests/run-all.sh` | 51/51 (17 x ts, kotlin, swift); the Swift runner executes 19 XCTests (17 scenarios, 2 `ApplyReportTests`), 0 failures |
| `undra bindgen -C examples/playground --docs --check` | up to date |
| Kotlin and TypeScript generated trees, goldens and runtimes vs `main` | byte-identical |
| `node site/scripts/build-all.mjs`, `check-links.mjs`, `check-links.mjs --words` | 20 pages OK; landing 332 words (budget 350); a second `build-all` changes nothing |

## R6 for the Swift host

R6 now holds for Swift. Every boundary entry was already guarded on the core side; what changes is the host: no reply
status, transport failure, schema change, cancellation or undecodable byte reaches a trap from generated code or from any
runtime path generated code can take, in debug or release, and each one surfaces as a typed value: the method's own `E`,
`CancellationError`, an `UndraCallError` case, or, for the one shape that cannot throw (commands and store `apply`), an
`UndraUnhandledError` handed to `LoadOptions.onError` after an error-level log. The remaining preconditions in the codec
guard host-side misuse of the encoding API and are not reachable from anything the core sends, which is the same line
Kotlin's `require` draws. The contract suite proves the panic path end to end through the generated `explode` on all three
platforms.
