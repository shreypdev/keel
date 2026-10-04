# objects-followups: the open items O1 to O8 of the objects-callbacks review

Piece `objects-followups`. Worktree `wt/objects-followups`, from `main` `848d3a7`. The review is
`.10x/reviews/2026-10-02-objects-callbacks-review.md` (its "Open items" table); ADR-040 and ADR-041 carry a
"Follow-up notes" section each with what changed in their rules; SPEC 3.7/5.9/10.3a/11/16/17 carry the wording. This
record has the cause, the fix and the test of each item, what was decided where the review left room, what was not done,
and the counts.

One commit per item (`fix(streams)` O1, `fix(swift)` O2, `fix(restore)` O3, `fix(ts)` O4 and O7, `fix(dev)` O5,
`fix(callbacks)` O6, `fix(bindgen)` O8), plus the chores `fmt`, the proxy key type, the re-blessed testkit recordings,
the ADR notes, the size trim (`perf(runtime)`: the web gate, below) and the abort-order fix of O7 that the contract grid found
(`fix(ts)`). Every test below was run **before** the fix (or with the fix reverted in place) and fails there.

## O1 - streams that take objects or callbacks

**Cause.** Swift's stream branch wrote no `requireOwn` and no give-back (an object of another core was sent; a refused
stream's lent listener stayed in the registry, held strongly). TypeScript lent outside `lending`, so a refused stream
kept its listener. Kotlin's `flow { try { emitAll(items) } catch (e: UndraException) { giveBack } }` also caught what the
*collector* threw and what the downstream `map { decode }` threw for an item the core had sent, so a reference the core
still held was given back (and `coreTookArguments` treats a `WireException` as "not taken"). No golden, typecheck or run
test had a stream with an object or callback parameter, on any platform; nothing in the core's tests did either.

**Fix.**
* Swift: `handover(.., checks: true)` for streams (`requireOwn` and the `withExtendedLifetime` fence); the object checks
  sit in a `do`, and a stream whose call could not be made returns `core.failedStream(error, mapError:)` (it fails at its
  first element, like every stream failure). `core.stream(.., lending:)` / `openStream(.., lent:)` give the lent
  instances back when the core refuses the open (`StreamChannel.lent`, status 5 as the first reply) or the call is not
  sent (no call id, the transport refuses). A stream opens when the method is called, so it lends once.
* TypeScript: `lendingStream(core, (lend) => core.stream(..))`, an async iterable that lends and opens per iteration and
  gives back unless the stream was served (an item arrived, it ended, or the core failed it with anything but status 5).
  Object-only streams keep the eager `requireOwn` at the call (a synchronous refusal; documented in the doc comment).
* Kotlin: lend, then `val items = try { encode; core.stream(..) } catch (e: Exception) { giveBack; throw e }`, then
  `emitAll(items.catch { e -> giveBackIfRefused(e, instance); throw e })`. `Flow.catch` handles the upstream's exceptions
  only, so the collector's exception and the decoding of an item pass through; the decode failure is `mappedStream`'s.

**Tests.**
* The golden cases carry the shapes: `object_graph` (`Account.follow(target, extra?)`, `follow_checked(boxes)` as a
  `Result<Stream, MailError>`, the free function `follow_all(account)`) and `callbacks` (`Uploader.follow(watch,
  listener)`, `follow_checked(listener?)`, the free function `tail(listener)`); the three languages' goldens, typechecks
  (`typecheck_swift` Swift 6 and the iOS 15/16 floors, `typecheck_kotlin` with `-Werror`, `typecheck_ts`), and the old ad-hoc
  TypeScript typecheck test is gone (the goldens replace it).
* Run tests: TypeScript `callbacks.mjs`/`object_graph.mjs` over a fake core (lent once per iteration, nothing lent before
  the first `next()`, refused/never-opened give back, a served stream, an early `break` and a throwing loop body keep it, an
  absent optional lends nothing, a foreign object is refused with nothing lent or sent); Kotlin `CallbacksTest.kt` and
  `ObjectGraphTest.kt` (the same, plus a decoding failure and a throwing collector keep the reference: **fails with the old
  emitter** at "an item the core sent that does not decode (not a refusal) leaves the reference with it: expected 5, got 4");
  Swift `ObjectsCallbacksTests` (`testAStreamTheCoreRefusesOrNeverReceivedGivesItsCallbacksBack`: refused, served,
  cancelled by the core, a failing decode, a closed core; `testAFailedStreamThrowsTheMappedErrorAtItsFirstElement`).
* The core's half: `undra-macros/tests/callbacks.rs` (a stream with `&Watch` and `Arc<dyn UploadListener>`: a stale object
  refuses before any proxy exists; an accepted one holds the proxy until the stream drops, one release).
* End to end: S27 step 8a (a stream takes a shelf, borrowed; a closed shelf refuses the stream; `host_refs` unchanged) and
  S28 step 10 (a stream takes a reporter: held while it runs, released when it ends; a closed workshop refuses it, the
  registry is back at its baseline) on Swift, Kotlin and TypeScript, over `Workshop::tally(Arc<Shelf>, steps)` and
  `Workshop::walk(steps, Arc<dyn Reporter>)` (new, with a Rust unit test). The playground's three generated packages, the
  two-core copies and the recorded testkit sessions (their schema hash moved) were regenerated.

**Not done / limits.** Swift's stream lends once (it opens when called); a stream that is never iterated on Kotlin or
TypeScript lends nothing (it opens at the first collection). The TypeScript object-only stream refuses a foreign object
synchronously from the method, not at iteration (`lendingStream` covers streams with callbacks, where the check sits
inside the lazily run encoder).

## O2 - Swift store wrappers

**Cause.** (a) `UndraStore.init` registered with the mirror unconditionally and the mirror was last-wins, so the second
wrapper of a handle (an `Arc<Self>` `new` returning an interned store: a Swift initializer cannot return the first)
replaced the first one's registration; closing either `unregister`ed the handle for both. (b) `close()` ran
`identities.forget`, `mirror.unregister(handle)` and `core.release(handle)` (which clears the observed set and the held
mark) as three steps, none atomic with an `adopt` of the same handle on another thread: the new wrapper lost its routing
and its observed signals (the core's count was right).

**Fix.** The mirror holds one function per **owner** for a handle (`register(_:owner:)`, `unregister(_:owner:)`, the public
replacing `register` is unchanged): every wrapper receives each change (each applies a copy of the reader) and one going
away takes only its own. The identity map counts a handle's wrappers (`holders`) and `leave(_:cleanup:)` runs the
leaving wrapper's cleanup **under the map's lock** with whether it was the last holder: the wrapper's own routing always
goes, the handle's observed set and held mark only with the last. `UndraCore.release` is `forgetHandleState` plus the bare
`releaseExtraReference`, so `close()` no longer clears state another wrapper needs; `reattach` is gone.

**Tests** (`ObjectIdentityTests`): `testASecondWrapperOfAStoreDoesNotTakeTheFirstOnesRoutingOver` (both update; closing one
leaves the other routed; the registration goes with the last); `testACloseRacingAnAdoptOfTheSameHandleLeavesTheNewWrapperRoutedAndObserved`
(deterministic: a DEBUG hook, `UndraObject.testHookAfterMarkedClosed`, adopts the handle at the moment the closing wrapper is
marked closed and has not left the map; with the old unconditional cleanup it fails: registered 0 not 1, count 0 not 9,
observed cleared); `testAdoptingWhileOtherThreadsCloseTheSameHandleIsConsistent` (600 adopts on the main thread against closes
on global queues, then the invariants). **Limit:** a duplicate wrapper is not the handle's identity-map wrapper, so after the
first closes a later `adopt` makes a third; all work (each owns its reference).

## O3 - restore and object parameters

**Cause.** `cancel_calls_replaced_by_restore` checked a call's receiver only; an async free function or a stream that took
a `&Store` kept running on the pre-restore store and answered status 0 (ADR-023's M3 hazard through ADR-040's parameters).

**Fix.** The generated dispatcher of an `async` method or a stream resolves its object parameters through
`Runtime::param(call_id, handle)` (the `Runtime::object` of the other methods, which hand their parameters back before the
call returns): it remembers the handles in one per-thread slot keyed by the call id, `spawn_call` / `open_stream` move them
into the call table (`CallEntry.params`, inline, the first four; a fifth object parameter is not checked), and the restore
applies the existing "replaced or invalidated" rule to the receiver and to each (a handle released before the restore and
absent from the snapshot is left alone). The first shape of this (a collector armed by `call_from` around every dispatch)
cost the hot path a thread-local arm and take per call and the web gate 230 bytes; this one touches only the methods that
need it.

**Tests** (`crates/undra/tests/e2e_todo.rs`, real macros through the `TestRuntime`): an async free function holding a store
parameter across a restore is answered `Cancelled`, once, and the restored store serves the next call; a stream holding one
ends with one failed item (flag 3); a call whose store parameter was released before the restore (and is not in the snapshot)
carries on. The first two fail without the change (`expected exactly one, got []`).

## O4 - TypeScript crash recovery and give-backs

**Cause.** A `Release` sent while the core restarts was kept in a `Set` and replayed as `core.release(handle)`, which
unregisters the mirror, forgets the handle and its observed signals and releases: for a give-back (a superseded wrapper's
finalizer, an `adopt` of a handle a wrapper holds) that killed the live newer wrapper and released the restored entry's only
reference. Two releases of one handle collapsed into one. A finalizer from before a restart also gave back through
`_giveBack` against a newer wrapper.

**Fix.** The recovery layer counts the releases it holds back per handle, replays each as the bare wire release it was (the
local bookkeeping ran when the wrapper closed), and **drops** those of a handle an open wrapper holds (the restored core
counts what the snapshot held, which may not include an extra reference issued after it: a leaked reference until the core
closes is the lesser harm). `UndraCore._restarts` counts restarts, the `Leak` record carries it, and `collected(core,
handle, epoch)` gives nothing back for a pre-restart wrapper while a newer wrapper holds the handle.

**Tests** (`recovery.test.ts`): give-backs of a live wrapper's handle during the restart are not sent, its routing,
observation and updates survive, and another handle's two releases go out twice; a pre-restart finalizer leaks rather than
frees the live wrapper, a post-restart one gives back normally, and with no wrapper at all it releases. Both fail with the old
replay and the old finalizer.

## O5 - `undra dev` sessions

**Cause.** (1) A constructor reply was recorded in the connection's `constructed` *set* and, for an `Arc<Self>`
constructor, again in the origin ledger: a disconnect gave back one reference more than the client held (an in-process
embedder's reference to the same singleton was lost) and a constructor called twice counted once on one side. (2) One
`Release` wiped a handle's tracking: a store constructed and also returned leaked a reference. (3) Callback proxies were
interned by `(port, instance)` while every client numbers its instances from 1: a left session's proxy (held by an object that
outlived it) was taken for the next client's instance 1 and its duplicate reference was given back to that client, as a
release of its own listener. (4) TypeScript and Kotlin dropped their callback registry when the connection was lost although a
resumed session keeps its proxies; Swift kept it.

**Fix.** Constructor references live in the connection, per handle (`HashMap<u64, u32>`, a handle once per reference in
retained sessions, `adopt` and `KeptSession.handles`); `call_from` serves a constructor with the ledger flag off
(`CallOrigin { origin, ledger }`) so the origin ledger skips what it issues. A `Release` gives back one reference of either
kind (the constructor's first: `conn.release` says which; else `release_from`), and the client's observation ends only when
`host_refs_of(handle)` is gone. Proxies are interned per `(origin, port, instance)`; `Runtime::set_client_origin` (set by the
session after it claims the slot, cleared by `clear_client_origin` before its teardown drops anything) gates a proxy's calls,
`__release` and `__cancel`: a proxy of another origin is silent. The three runtimes keep their callback registry across a lost
connection (aborting what ran for it: `connectionLost`) and drop it when the core closes.

**Tests.** `undra-transport/tests/objects.rs` (the fixture gained a `Hub` singleton and a `Listener` callback):
`a_disconnect_gives_back_one_reference_per_constructor_call_and_no_more` (an embedder's reference survives a client's two
constructor references), `a_release_gives_back_one_reference_of_an_object_constructed_and_returned` (no leak),
`a_session_that_left_has_no_say_over_the_next_sessions_instances` (the second client hears one note for its own instance 1 and
receives exactly one release, its own); tracker unit tests; all three fail on the old code. Registries:
`callbacks-remote.test.ts` (the TypeScript case that pinned "dropped with the connection" now pins kept-then-dropped-with-the-core),
Kotlin `RemoteReconnectTests` (new case over real sockets) and `CallbackTests` (the case that pinned the old rule),
Swift `testALostConnectionKeepsTheRegistryAndClosingTheCoreDropsIt` (also: the running call is cancelled).
**Limit:** after a hot reload the new core has no proxies for the old instance ids, so a client's registry keeps entries the
new core never releases until the client closes (dev only).

## O6 - a constructor that fails after its body

**Cause.** A constructor with callback parameters has made its proxies by the time `issue_constructed` or
`__undra_attach_all` fails; the unpublished value drops them (`__release`), yet the call answered status 5 ("refused: owns
nothing") so the host gave its references back too: a double release.

**Fix.** `DispatchResult::Failed(String)` (status 2, no unwinding: the runtime reports it through the panic path, reason as
the message, no backtrace, so the Diagnostics port and the stats see it as a failed call; sync, async and stream paths); the
macro uses it for the failure after the body of a constructor that took callbacks, and keeps status 5 for one that took none.
A constructor's own reference is committed with `IssueScope::commit_constructed` (it is the client's one reference, counted by
the transport, not recorded for the origin: O5).

**Tests.** `undra-macros/tests/callbacks.rs` (`a_constructor_that_fails_after_making_its_proxies_is_not_a_refusal`: a store
whose second construction cannot attach; fails with the old macro: "expected a failed call, got BadRequest"; the proxy is
released once), `undra-runtime/tests/layers.rs` (`Failed` is status 2 with the reason, sync and async).

## O7 - an abort racing a successful reply

**Cause.** `_send`'s abort handler sent `Cancel` and deleted the pending entry, so a success reply already on its way
(worker, remote and React Native transports; `wasm-main` answers inside the cancel) found no entry and its object references
were owned by nobody until the core closed.

**Fix.** `UndraCore.call(.., signal, orphan?)`: an aborted call with an `orphan` keeps its entry as that answer (the entry's
`resolve` becomes the orphan, its `reject` a no-op); `reclaim(core, shape)` (identity.ts) reads the reply's handles (one,
optional, list) and gives each back. Generated code passes it for an `async` method that returns objects (`ts.rs`); calls
without one behave as before. The same code serves the worker, remote and React Native transports.
The entry is abandoned (deleted, or turned into the orphan) **before** the `Cancel` is sent: a transport that answers a cancel
inside the send (`wasm-main`) must find the call already abandoned, or the caller sees the core's `CancelledByCore` instead of
its abort reason. The first version of this fix sent the cancel first and broke contract S06, S27, S28 and S30 on TypeScript; the
contract grid found it, and `core.test.ts` now has the transport that answers inside the send (with and without an orphan).

**Tests.** `core.test.ts` (kept until the answer, given back for each shape, a cancelled answer gives nothing, a call without
an orphan is unchanged: fails without the tombstone: `pendingCalls` 0 not 1) and the React Native `objects-callbacks.test.ts`
(over the stand-in native module: the abandoned reply's reference goes back by its two halves). **Not done:** Kotlin and Swift
keep their documented limit (the same race, in process: a hook on `UndraCore.call` would fix each).

## O8 - Kotlin `invoke` for every `new`, and the Swift weak wrapper

**Kotlin.** `operator fun invoke(<params>, ctx = ..)` is generated for every `new` (parameters, `suspend`, fallible), as a
forwarder to `create`; a golden `Vault` (suspending, fallible, takes a parameter) is in `object_graph`, and the run fixture calls
`Uploader(listener, core)`.

**Swift.** Typed throws can throw only the method's own error, so the weak wrapper's gone path throws the error type's
`Unavailable` variant when it has one (unit, or one `String`: the idiom `From<PortError>` maps to) and calls
`UndraCallbacks.targetGone()` otherwise, which now stops the process with a message instead of awaiting for ever (the
core's path never calls it: the runtime resolves the target first and answers unavailable). The `callbacks` golden's
`WeakUploadListener` throws `PromptError.unavailable("WeakUploadListener's target is gone")`; `WeakTokenProvider` (an error
type without one) keeps `targetGone()`. A Swift **run check** of the golden (`fixtures/swift-run/callbacks.swift`, built with
the real runtime) drops a weak wrapper's target and asks it a question.

## The web size gate

The hello-wasm gate is `min(120,000, record + 5%)` and `main` sits at 119,654 (its own path). The piece's net change in the
module is about **+90 bytes of code after wasm-opt** (a stream of small things: `Failed` in the classification, the call
entry's parameters, the call table's wider entry; the restore got smaller, the per-receiver iterator chain gone). The gzipped
number moves with where the checkout lives (the panic locations embed it; ADR-052), by more than the change itself:

| path | `main` `a4c7cb2` | this piece |
|---|---|---|
| sibling of the main checkout (a name as long as `keel`) | 119,611 | **119,856** (+245, 144 under the budget) |
| `.work/objects-followups` (this worktree) | 119,928 | **120,031** (+103, 31 over the budget) |

So the gate passes where the repository is checked out and fails by 31 bytes at the longer path of this worktree; the
integrator's `--record` on `main` is the measurement that counts. The first version of the O3 change (a collector armed and
taken around every dispatch) measured +230 bytes at the long path; the trim (commit `50f6743`, `perf(runtime)`) moved the cost to
the methods that need it: object parameters of an `async` method or a stream go through `Runtime::param`, which records them
in one per-thread slot (two `Cell`s: the call id they belong to and the handles), `spawn_call` / `open_stream` take them into
the entry, a failed call is reported through the existing panic path. JavaScript: 22,044 of 22,100 (record 22,005).
Neither budget was restated.

## What the review said and this piece did not do

* O9 (the wasm budget) and O10 (the playground's tab bar) are not in the brief.
* Verification of the Swift weak-wrapper `fatalError` path is by reading (a process stop cannot be asserted in-process).

## Counts

Run once on the merged tree (`main` `a4c7cb2` is an ancestor), the toolchains required (`UNDRA_REQUIRE_TOOLCHAINS=1`):

| Suite | Result |
|---|---|
| `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy -p undra-ffi --target wasm32-unknown-unknown -- -D warnings`, `cargo doc --workspace --no-deps` | clean |
| `cargo test --workspace` | 3,239 pass, 18 ignored. One failure on the first run, `undra-signals` `derived_delivery::concurrent_writers_and_a_re_observer_never_strand_an_op` ("update_at at index 13, but the list has 13"): a race inside that test (it picks an index from a length it read earlier), in a crate this piece does not touch; it passed on three reruns |
| bench `budgets` (release) | 6 pass, 1 ignored |
| wasm harness | 22 + 36 pass |
| C / JNI / Swift-ABI harnesses | `c smoke`, `c lifetime`, `c two cores` ok / 16 pass / 6 pass |
| Swift runtime (`swift test`) | 778 pass |
| Kotlin runtime, Kotlin 2.4.20 and CI's 2.0.21 | 811 cases in 46 suites, 0 failed, 2 skipped; the testkit 32 in 5 suites; both compilers |
| TypeScript runtime | 1,686 pass (61 files), `typecheck` clean |
| React Native | 105 pass (9 files), `typecheck` clean, `cpp/test/run.sh` 30 checks |
| Contract grid (`rm -rf contract-tests/swift/.build`, `run-all.sh`) | 86/86 (TypeScript 30, Kotlin 28, Swift 28). The first run found the O7 ordering bug above (TS S06/S27/S28/S30, now fixed with a test) and a Kotlin S30 timing step ("within 100 ms of Background") that passed on the rerun |
| Interop (`crates/undra-transport/interop/run.sh`) | OK |
| `undra bindgen --check --docs`: playground, two-cores a and b, cookbook, fieldbook; `--check`: ios15-sample | all up to date (the playground's hash is `0x25ca013a060a93ce`: it gained `Workshop::tally` and `walk`) |
| `scripts/wasm-size.sh` | wasm 120,031 at this path (see the gate section: 119,856 at a `keel`-length path; `main` 119,928 / 119,611); JS 22,044 of 22,100 |
| site `build-all.mjs`, `check-links.mjs --words` | clean; the reference pages and `llms-full.txt` regenerated; landing prose 342 of 350 words |
