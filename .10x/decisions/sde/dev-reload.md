# SDE — dev-reload: a rebuild keeps the state (wt/dev-reload, 2026-10-01)

Track B3 of the v1.x design (`.10x/specs/2026-10-01-v1x-default-choice-design.md`). The decision is ADR-053 (written
first, R11, accepted by the integrator with four decisions that are in its "Decisions" section; "As built" lists what
differs). `docs/DEV_LOOP.md` is the user-facing page; this note is the record for the integrator.

## What landed

Editing Rust no longer loses the screen. When a rebuild succeeds, `undra dev` starts the new core in **standby** (runtime
built, not listening), **suspends** the old core's server, takes its `Runtime::snapshot`, stops it, hands the snapshot to the
new core (`Runtime::restore`, before it listens) and tells it to listen holding the old client's session (ADR-051), so the
client reconnects, resumes, observes again and gets the restored values first. Everything that cannot be carried falls back to
the loop as it was (4001 / schema mismatch) and says why. No envelope, payload, ABI, schema, generated-code or
`undra-runtime` change.

* `undra-transport`: `Server::suspend(settle) -> Suspended { session, settled, cancelled_calls }` (no new connection and the
  listener closed, no new `Call` from the client, a wait of at most `settle` for open calls, Close 1001 `the core is
  reloading`, the retained session handed back un-released), `ServerConfig::inherited_session` (a session held from the first
  instant), `ServerConfig::attach_notices` / `AttachNotices` / `NOTICE_TARGET` (a `Log` record with the target `undra::dev`
  to every client that attaches within the window, once per session token: one sentence for a resumed client, one for a new
  one), `KeptSession`. `notice.rs` is new; `server.rs`, `session.rs`, `resume.rs`, `conn.rs`, `tracker.rs`, `bridge.rs` change
  by a few lines each.
* `undra-cli`: `reload.rs` (new: the swap, its decisions and order, behind an `Ops` trait, so the failure matrix is unit
  tests with a recording fake), `runner.rs` (the runner's stdout/stdin line protocol: `standby`, `snapshot`, `state`, `reset`,
  `listen`, `restored`; hex), `commands/dev.rs` (`ProcOps`, the loop), `templates/runner/main.rs` (builds the runtime, then binds;
  `--standby`), `--no-keep-state`, the playground's `Restarted:` line says what became of the state.
* The three runtimes: an optional `onDevNotice` option (TS `AttachOptions`, Kotlin `LoadOptions` last parameter, Swift
  `LoadOptions` and `.remote(...)`), dispatched from the **remote** transport only (a test per runtime asserts an in-process
  core never fires it), on a thread of the runtime's own for Kotlin and Swift. One Kotlin transport fix (below).
* The three dev bars (playground and `undra init` templates): `Reloaded, state kept` for four seconds over the usual line; the
  schema-mismatch text says `The schema changed, state reset: ...`.
* Docs: ADR-053, SPEC 5.9, 5.10, 11.0, 17.1-17.3, `docs/DEV_LOOP.md` (the save section, what carries over, the runner
  protocol, troubleshooting rows, a stale query handle: run the query again), `undra-transport`'s README and crate docs,
  the site's `cli.html` and `getting-started.html` (one sentence each) and the generated site files, the READMEs, `undra init`'s.

## Evidence

**CLI integration tests** (`crates/undra-cli/tests/dev_reload.rs`, the real `undra dev` on a *copy* of the playground, a raw
client with a session token, edits of `core/src`):

* `a_rebuild_keeps_the_screen_the_client_was_on`: a counter at 7, a 10,000-row list with a row removed, a plain object; a
  broken edit (the old core keeps serving, state intact); a good edit; after the swap the client resumes (`reload-keep-token`),
  observes, and the first change-sets say `count 7, changes 3, 9,999 rows`; `increment` works on the restored handle; the plain
  object's handle is status 5; the notice is `Reloaded, state kept (1 object not carried over)`; the terminal line reads
  `state kept (2 stores, 205 KiB, restored in 1.6 ms); 1 object not carried over: ...`.
* `a_schema_change_resets_the_state_and_says_so`: a new method changes the hash; `state reset: schema changed (was 0x.., now
  0x..)`; an old-bindings client is told the new hash in the `Hello` and closed 1008; a new client has `count 0` and the notice
  `Reloaded, state reset: schema changed ...`.
* `a_state_over_the_limit_falls_back_to_fresh_state_and_says_so` (`UNDRA_DEV_STATE_LIMIT_BYTES=50000`): `state reset: snapshot
  over 50000 bytes`, 4001 for the resuming client, the notice to the fresh one.
* `no_keep_state_starts_every_rebuilt_core_fresh`.
* `tests/dev.rs` (the existing three) is unchanged in meaning; its `Dev` helper moved to `tests/common/devserver.rs`.

**Transport tests** (`crates/undra-transport/tests/suspend.rs`, 11, real sockets): the quiesce order (the listener is closed
afterwards; the session comes back un-released and survives a later `shutdown`; a call open at the freeze finishes within the
settle and its effect is in the snapshot taken after; one that does not is cancelled and counted; a stream is not waited for;
no `resume_grace`: nothing handed over), the successor serving on the *same address* with the snapshot restored before it
listens and the client resuming (same handle, same value, first change-set), the inherited session that cannot be held is
released, and the notices (each kind to its own client, once per session, not after the window, to a `mode = "prod"` client,
none to a refused client, none by default). Unit tests beside the code: `reload.rs` 9 (the order of the steps and every row of
the failure matrix), `runner.rs` (the protocol lines, damaged answers, hex), `notice.rs` 4, the session's freeze.

**Runtimes:** TypeScript 4 new (a remote core is told, after a reconnect too, a throwing callback is reported, an in-process
core never fires), Kotlin 4 new (the three plus the `Hello` race), Swift 2 new.

**Devices** (screenshots in the scratchpad: `android-1-before.png`, `android-2-counter7.png`, `android-3-after-reload.png`,
`ios-1-before.png`, `ios-2-counter5.png`, `ios-3-after-reload.png`; the logs `undra-dev.final.out`, `undra-dev.final.err`):

* **Android, the `undra` AVD (emulator-5554), remote mode** (`ws://10.0.2.2:7443`): tapped `+` seven times, appended a comment to
  `counter.rs`: the bar read **`Reloaded, state kept`** and the counter still said `7`, `7 changes`. `logcat`:
  `UndraApp: connection: Reconnecting(attempt=1, cause=... (1001 the core is reloading))`, then `dev server: Reloaded, state
  kept`, `connection: Connected`.
* **iPhone 17 Pro simulator** (`ws://127.0.0.1:7443`): five taps, an edit: **`Reloaded, state kept (1 object not carried over)`**
  and the counter still `5`, `5 changes`.
* **Web** (real browser, Vite dev server, the TypeScript runtime): the same, the bar's history was `Reconnecting (attempt 1)`
  at +5974 ms, `Dev server`, `Reloaded, state kept (1 object not carried over)` 171 ms later, back to `Dev server` four seconds
  after; a schema change turned the bar red `The schema changed, state reset: run undra bindgen, then reload`, the terminal
  `state reset: schema changed (was 0xddcdea47fa95a8d4, now 0x8de7f5135dcd755b)`.
* The dev server's log (`undra-dev.final.err`): `suspended for a reload: 0 call(s) were still open, session a892e3a5 (4
  object(s)) handed over`, `holding session a892e3a5 (3 object(s)) for its client`, `client reconnected: ... (session a892e3a5,
  away 0.1 s, 3 object(s) kept)`; 74 ms from the suspend to the new core listening.

**Numbers:** playground snapshot 209,008 B (204 KiB); `Runtime::snapshot` 68 us, `restore` 189 us (release, in process, best of
20); 1.6 ms inside the runner (a debug core); the swap from the client's close to the new core listening 74 ms; an incremental
rebuild 0.5 s. The runtime's rows are already budgeted (`snapshot/encode_100kb` 13.8 us, `snapshot/restore_1mb` 260 us); no
hot path or boundary entry changed, so no new budget row.

**Matrix (on the tip, after merging main twice: tooling at `aa66ce9`, wasm-size at `3e8a304`):** `cargo fmt --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean;
`cargo test --workspace --no-fail-fast` (with `UNDRA_REQUIRE_TOOLCHAINS=1` and `tsc` on `PATH`) 2,589 passed, 0 failed (11 ignored); `contract-tests/run-all.sh` 54/54 (18 x 3); TypeScript 1,132
(`npm run typecheck` clean); Kotlin 616 cases, 0 failed, under kotlinc 2.4.20 **and** 2.0.21 (2 skipped: no native library);
Swift 482; `crates/undra-transport/interop/run.sh` (the shipped TS and Kotlin transports against the real server) OK; `undra
bindgen -C examples/playground --check --docs` up to date (hash `0xddcdea47fa95a8d4`); `node site/scripts/build-all.mjs` and
`check-links.mjs --words` clean.

## Findings and deviations

1. **A Kotlin transport race, found on the emulator, fixed.** A frame right behind the server's `Hello` was dropped: the
   connecting thread sets `handshakeDone` and publishes `current` after it wakes from the `Hello`, and the reader thread had
   already read the next frame. The first run on the AVD showed the right counter and no notice; `logcat` had no `dev server:`
   line. Fixed in `RemoteTransport.dispatch`; a Kotlin test fails without it. Swift and TypeScript set the flag on the reading
   thread. (The same race could have dropped a `PortCall` the core issued right after attach: now it cannot.)
2. **Query handles do not survive a reload.** `RemoteTodosQueryHandle` is transient (SPEC 5.9): the playground's Remote tab keeps
   its last values and `refetch` fails with `Refused` (stale handle) until the query is constructed again. This is the
   integrator's decision 2; `docs/DEV_LOOP.md` says what to do. The cheapest real fix is for the server to record the constructor
   call of a transient object and re-run it into the same handle after the restore; that needs an ADR (it changes what restore
   promises) and is not done here.
3. **Not as the brief wrote it:** the TypeScript-runtime-over-a-real-WebSocket check against `undra dev` is the browser run above,
   not a node script (the integration tests speak the envelope with a raw client; the TS suite pins `onDevNotice`). No new
   contract scenario: a reload is ADR-051's reconnect plus the change-sets S15 specifies (argued in the ADR); S01-S18 still
   pass on all three.
4. **Standby is one extra process start** at the swap (the price of "a core that cannot start leaves the old one serving"; it
   cannot be tested end to end without a core that fails at start, so that row is a unit test of `swap` with a fake, plus
   the real path exercised by every integration test).
5. `Server::suspend` leaves the runtime running (the runner calls `Runtime::shutdown` after the snapshot, as before): the core's
   own tasks may write for the milliseconds until `undra dev` closes stdin; the snapshot is the state when it was taken.
6. The notice is sent at most once per session token and a client with no token is told per connection; a client that reconnects
   again inside the window is not told twice.

## What the integrator owns

Nothing in `.10x/status.md` or `handoff.md` was touched. After merging: the status line for B3, the handoff note that B4
(devtools) can build on `Server::suspend`/`undra::dev` notices and on the runner protocol, and the open follow-up of re-creating
transient handles (finding 2). Track A's shapes (`UndraCore.kt`, `core.ts`, `UndraCore.swift`) were merged first; the runtime-side
diff is additive.
