# B3 dev-reload (ADR-053, state-preserving reload) — adversarial review

**Date:** 2026-10-02 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/dev-reload` at `12ec236` (57 files, +4,008/−331 against `main`; `main` merged at `3e8a304`, which is still `main`'s
tip at the end of this review) · **Read:** `CLAUDE.md` (R4, R6, R9, R11, R12), ADR-053 with its Decisions and "As built",
ADR-051, ADR-022/023, the SDE record `.10x/decisions/sde/dev-reload.md`, `docs/SPEC.md` 5.9, 5.10, 11.0, 17,
`docs/DEV_LOOP.md`, and the diff (`undra-transport` `server.rs`, `session.rs`, `notice.rs`, `resume.rs`, `bridge.rs`,
`conn.rs`, `tracker.rs`; `undra-cli` `reload.rs`, `runner.rs`, `commands/dev.rs`, `templates/runner/main.rs`, the
templates; the three runtimes' `onDevNotice` and the Kotlin `RemoteTransport` change; the three dev bars) · **Method:** a
test for every attack, against the real `undra dev` on a copy of the playground and against real sockets; the Kotlin race
fix reverted in a scratch copy to see its test fail; the remote Kotlin suites 20 times on a loaded machine; the device proof
re-run on the `undra` AVD · **Fixes:** `1f7943d`, `fcbf09f`.

## Verdict

**Merge after fixes; the fixes are on the branch.** The design holds: the steps happen in the order that cannot lose state
(standby before anything is touched, the restore before the new core listens), the snapshot never touches disk, the
handover is keyed on the session token and needs `undra_resume=1`, the generation floor crosses the process boundary,
every failure falls back to today's loop, and nothing in `undra-runtime`, the envelope, a payload, the ABI or the generated
code changed. The pipes cannot deadlock (the parent drains the runner's stdout on a thread of its own; the runner's
stderr is inherited), and every wait on a runner has a deadline.

What was wrong is where the ADR promised honesty and the code was silent. **A tap made during the swap vanished behind
"Reloaded, state kept"** (M1): from the freeze to the close, `Call` frames were dropped without a reply and without a
count, and a command that fails as `Unavailable` while the connection is down is only logged (ADR-051), so neither the app,
the bar nor the terminal said the write was gone. **The core could speak as the dev server** (M2): a record a core logs
under `undra::dev` went to every client and fired `onDevNotice`, against decision 1(a) and SPEC 5.10. Both are fixed with
tests that fail on `12ec236`. Two Low findings are fixed (the runner's stdout was read in unbounded lines and a non-UTF-8
byte ended `undra dev`; a call could be recorded between the settle's count and the Close). The Kotlin `Hello` race fix is
right and necessary (its test fails without it), and the envelope's schema check keeps a frame from another core out of
the window it opens. Nothing blocking is open.

## Findings

| # | Sev | Where (at `12ec236`) | Finding | Status |
|---|---|---|---|---|
| M1 | Medium | `crates/undra-transport/src/session.rs:376-387`, `:621-623`; `server.rs:489-504`; `crates/undra-cli/templates/runner/main.rs:232-235`; `crates/undra-cli/src/reload.rs` (`Outcome::describe`) | **Lost writes in the quiesce are silent.** After `suspend` sets `frozen`, a `Call` is dropped without a reply (and one that arrives after the Close is queued is dropped too); neither is counted. The client fails it as `Unavailable` at the close, and for a command (the `+` button) that failure is only logged at warning level (ADR-051: the connection state already says "reconnecting"). The window is the up-to-2 s settle whenever a call is open (an `async` load, a hanging port call), and the notice then says `Reloaded, state kept`; the snapshot does not contain the tap. Calls cancelled at the end of the settle were counted for the terminal but not in the notice. Reproduced against the real `undra dev`: a `Probe.hang` held open across the reload and a tap every 50 ms; the first unanswered tap's write is not in the restored state and nothing says so. | **Fixed** (`1f7943d`). `Suspended::dropped_calls` counts the calls not run (on the frozen path and among frames ignored after the Close); the runner answers `snapshot ok <settled> <cancelled> <not run> ..` and is handed `state <old-hash> <lost calls> ..`; the terminal adds `N calls sent during the reload were not run`, the notice reads `Reloaded, state kept (1 object not carried over; 2 calls lost in the reload)`. Semantics unchanged (not run, `Unavailable` at the close; the state is the state before them), now said. Tests: `calls_the_reload_cut_off_are_counted_and_the_notice_says_so` (real `undra dev`; at `12ec236` its `Restarted:` line has no word of the call that was not run: `state kept (1 store, 1 KiB, ...); 1 object not carried over: ...; 1 call still running when the core was replaced was cancelled`), `a_call_sent_while_the_server_settles_is_not_run_and_is_counted` (transport). ADR-053 "Review amendments", `docs/DEV_LOOP.md` (what carries over, the protocol table, a troubleshooting row), SPEC 5.10, the transport README. |
| M2 | Medium | `crates/undra-transport/src/bridge.rs:174-182` | **A core can say a dev notice.** `Host::log` forwards every runtime record to the attached client; `undra_info!(target: "undra::dev", "Reloaded, state kept")` in an app's core reaches the client as a `Log` with that target, and each runtime's `onDevNotice` fires (the runtimes cannot tell origin: the envelope is the same). Decision 1(a) and SPEC 5.10 say only `undra dev`'s server emits one. Dev only, no data exposure, but the bar can be made to lie. | **Fixed** (`1f7943d`): the bridge does not forward a `undra::dev` record to a client (the log sink, i.e. the terminal, still prints it); the server's own notices go through `Session::tell`, which does not use `Host::log`. `NOTICE_TARGET`'s docs, SPEC 5.10, `DEV_LOOP.md`. Test `the_core_cannot_say_a_dev_notice_only_the_server_can` (fails at `12ec236`: the client received `["Reloaded, state kept"]`). |
| L1 | Low | `crates/undra-cli/src/runner.rs:298-310`, `:104-109`; `commands/dev.rs:273`, `:287` | **The runner's stdout.** (a) Lines were read with `BufRead::lines()`, unbounded: a runner (or a core printing without end on stdout) grows `undra dev`'s memory without limit; the legitimate maximum is now a 32 MiB line. (b) `map_while(Result::ok)` ends the reader on the first non-UTF-8 byte, which sends `Closed`, which `undra dev` reports as "the dev server stopped" and exits: a core that writes one invalid byte to stdout kills the loop (this half is on `main` too). (c) A damaged or truncated `snapshot` answer (a runner that crashes mid-line) became a plain `Line`, printed whole to the terminal (megabytes of hex), and the swap then waited 15 s for an answer that would not come. (d) A protocol line glued to the core's `print!` output without a newline was not recognised (another 15 s wait). | **Fixed** (`1f7943d`): bounded line reads (48 MiB, three times the state limit; the rest of a longer line is dropped and the event says so), lossy UTF-8, a `snapshot` answer that does not parse or is over the bound is a failed snapshot at once (fresh state with the reason), a glued protocol line is split from the core's text, and `undra dev` checks the 16 MiB limit itself before decoding hex. Tests: `a_line_is_bounded_and_need_not_be_utf8`, `a_damaged_snapshot_answer_is_not_trusted`, `a_protocol_line_glued_to_what_the_core_printed_is_still_heard`, `a_snapshot_over_the_limit_is_refused_before_it_is_decoded` (exactly 16 MiB is carried, +1 is refused), `sixteen_mib_of_state_crosses_the_pipes_both_ways` (a fake runner prints a 16 MiB snapshot and reads a 16 MiB `state` line through real pipes: no deadlock, whole on both sides, about a second). |
| L2 | Low | `crates/undra-transport/src/session.rs:378-390` against `server.rs:489-504` | **A call between the settle's count and the Close.** The reader checked `frozen` and then recorded the call in a second critical section; `suspend` set `frozen` and then counted the open calls. A call that passed the check before the flag and was recorded after the count ran without being waited for: a sync write could land in the snapshot with its reply dropped by the closing connection (the client is told `Unavailable` for a write that is in the state), an async one was cancelled without being counted. Found by reading; the window is sub-microsecond and I could not hit it without injected delays. | **Fixed** (`fcbf09f`): `Conn::begin_call` reads the flag under the connection's lock, which `open_plain_calls` takes after the flag is set, so a call is either waited for or not run, and says why (`Begin::{Started, Frozen, Closing, Duplicate}`; a closing connection no longer logs "reuses the open call id"). Stress test `every_tap_racing_the_suspend_is_answered_or_not_run_and_only_answered_ones_are_in_the_state`: 30 rounds of pipelined taps against the start of a suspend with nothing open; the restored state holds exactly the answered taps. |
| L3 | Low | `runtimes/kotlin/.../RemoteTransport.kt:315` against `:256` | The race fix makes a frame from the connection being opened dispatchable, but `send(kind, payload)` still requires `current`: an asynchronous port reply produced inside the window (between the reader's `Hello` and the connecting thread publishing `current`) fails with `lostConnection`, is reported through `onError` ("port reply"), and the core's port call waits until the next disconnect fails it. Before the fix the port call itself was dropped, so this is strictly better; the window is the connecting thread's wake-up. | **Open** (note): send to `opening` when its handshake is done, or queue until `current` is set. |
| I1 | Info | `runtimes/swift/.../WebSocketTransport.swift:643`, `:812` | Swift adopts the server's hash from its `Hello` and routes later frames against it, so on a reconnect a frame right behind a mismatched `Hello` would reach the core before the reconnect's schema check (`:812`). Kotlin checks every post-`Hello` envelope against the *expected* hash, so its new window cannot admit another core's frame; TypeScript checks the `Hello` before it settles and is single-threaded. Unreachable with `undra-transport` (a refused client gets its `Hello` and then only the Close). Pre-existing. | Note. |
| I2 | Info | ADR-051 (one slot) | Two clients (a simulator and an emulator): only the attached client's session is handed over; the other was refused (1013) and holds nothing in this core. After a reload the first to reconnect wins the slot; if it is the other client (a new session), the carried stores are released and the first gets 4001 and loads afresh. ADR-051's semantics, not this piece's. | Pinned: `with_two_clients_only_the_attached_one_has_a_session_to_hand_over`. |
| I3 | Info | ADR-051 grace | An inherited session expires like any other: after `resume_grace` (10 min) the reaper releases the restored stores and a returning client is told 4001; a client back after the 30 s notice window finds its state and hears nothing. | Pinned: `an_inherited_session_whose_grace_passes_is_released_and_its_client_told_session_lost`, `a_client_that_comes_back_after_the_notice_window_finds_its_state_and_hears_nothing`. |
| I4 | Info | `crates/undra-cli/src/commands/dev.rs:352-368`; `reload.rs:262` | When `undra dev` gives up waiting for `restored` (30 s, or the runner closed), it does not send `reset`: a restore that finishes later still listens holding the carried session while the terminal said "state reset". With no store alive (`NothingToKeep`) the new runner is told nothing, so a resuming client gets 4001 with no notice. Both are corner cases. | Note. |
| I5 | Info | `LoadOptions.kt` | `onDevNotice` is a new last constructor parameter with a default: source compatible, but the JVM constructor signature changes (as it did for ADR-051's `reconnect`, `onConnectionChange`, `onError`). | Note, precedent. |

## The attack on each surface

**1. Lost writes in the quiesce.** The settle waits for the calls open *on the attached connection* (`Pending` in the
tracker: plain calls and constructors, not streams) and polls every 5 ms up to `SETTLE`; a call still open at the deadline
is cancelled by the teardown (`Runtime::cancel` takes the core lock and drops the future before it returns), so once
`suspend` returns no call can write, and `Runtime::snapshot` (under the core lock) sees whole task steps only. The core's
own tasks may still write; the ADR says so. A call cancelled at the deadline: not answered (the Close is ahead of its
`Cancelled` reply), counted, its partial writes in the state (ADR's known limit (a)). A call sent after the freeze, or after
the Close was queued: dropped, now counted (M1). A call recorded between the count and the Close: now impossible (L2). The
end-to-end test holds a `Probe.hang` open and taps every 50 ms through the rebuild: every answered tap is in the restored
state, the unanswered one is not, and terminal and notice say `1 call ... cancelled; 1 call ... not run` /
`(1 object not carried over; 2 calls lost in the reload)`.

**2. The runner pipe protocol.** No deadlock: the parent reads each runner's stdout on a dedicated thread into an unbounded
channel, so the old runner's 32 MiB answer never blocks it; the new runner's main thread is in its stdin loop while the
parent writes `state`, and its stdout is drained meanwhile; stderr is inherited. Every read has a deadline (standby and
listen 30 s, snapshot 15 s, restore 30 s); a runner that crashes mid-`snapshot` closes stdout, which ends the wait at once
(`Closed`) and falls back to fresh state. Unbounded reads, the non-UTF-8 exit, the printed damaged line and glued lines
were L1. Log records go to stderr (the runner's `print_log` and the bridge's sink), so only the core's own `print!` shares
stdout; `say` holds the stdout lock for a whole line, so nothing interleaves *within* a line. Cap boundary: exactly
16 MiB is carried and +1 refused on both sides (the runner's check, now also `undra dev`'s before decoding), and 16 MiB
crossed real pipes both ways in about a second.

**3. Session handover (ADR-051).** The new server seeds the registry with `(token, handles)`; adoption needs the token
**and** `undra_resume=1`; a stranger that asks to resume another token is told 4001 and does not consume the session (the
owner resumes afterwards: `a_resumed_client_is_told_the_resumed_sentence_and_a_refused_one_nothing`); a stranger that
attaches as new supersedes and releases it (no access to its objects). Two clients: I2. Generation floor across the
process boundary: after the reload a new `Probe` has a generation above the old process's stale `Probe` handle, which is
still refused (added to `a_rebuild_keeps_the_screen_the_client_was_on`). After the notice window and after the grace: I3.

**4. The Kotlin race fix.** `handshakeDone` is now set by the reader when it reads the `Hello`, and a frame from the
connection being opened is dispatched. Only the connecting thread sets `current` (the reader never does), `opening` names
one attempt at a time (initial connect and the reconnect loop never overlap: the loop starts only after `current` was
lost), and a late frame of a failed attempt matches neither. A frame cannot reach the core before the schema is accepted:
after `handshakeDone`, `Envelope.decode` checks every envelope against the *expected* hash, so a frame from a core with
another schema is a `SchemaMismatch` loss, never a dispatch; on the first connect the same check runs before `attach`
compares the `Hello`. Residual: L3. Proof that it was needed: with `main`'s `RemoteTransport.kt` in a scratch copy, the
new case `a frame the server sends right behind its Hello is not lost` fails (`timed out after 10000ms`). Stress: the
`RemoteTransport`, `ReconnectCore`, `RemoteReconnect` and `WebSocketClient` suites 20 times on a machine four other agents
were loading: 1,360 cases, 0 failed. TypeScript and Swift do not have the race: TypeScript checks the `Hello` and sets
`#open` synchronously in the message handler, before the next message event; Swift sets `handshakeDone` on the reading
callback (I1 is a different, pre-existing gap).

**5. `onDevNotice` and the notice channel.** Each runtime dispatches only from a `remote` transport (TS
`#transport.mode === "remote"`, Kotlin `Mode.REMOTE`, Swift `.remote`) and has a test that an in-process core never fires
it. There is no production remote core: the remote transport is documented dev-only in all three runtimes and the only
server is `undra-transport`'s. The spoof was M2. The notice reaches non-dev clients on purpose (it is not a devtools
record), once per session token.

**6. Failure matrix, for real.** Added against the real `undra dev`: a rebuilt core whose init hook calls
`std::process::exit` (standby never printed): "the rebuilt core did not start; still serving the previous build", the client
never disconnected and kept its state, the next good edit swapped with it
(`a_rebuilt_core_that_does_not_start_leaves_the_old_one_serving_with_its_state`); a restore the new core refuses (the
store's restore hook panics; the schema hash is unchanged, which the test asserts): `state reset: the core refused the
snapshot: restoring store 0xfc0f33c3 panicked: ...`, 4001 for the resuming client, the notice to the fresh one
(`a_snapshot_the_new_core_refuses_falls_back_to_fresh_state_and_says_why`); a second save during the rebuild: one
re-queued build, two `Restarted:` lines both `state kept`, the second from a core whose client had not come back (the
inherited session handed on), the client then resumes with its value
(`a_change_during_a_reload_is_built_next_and_the_state_survives_both_swaps`). No testing aid was needed for any of them.

**7. R12 and R6.** `undra-runtime` is untouched by the branch. The settle, the notice window, the grace and the runner
timeouts are `Instant`s in `undra-transport` and `undra-cli` (host side); the runner reads the wall clock only to print log
times and durations. The runner protocol path has no `unwrap` on input (slice patterns, `Option` decoding, `first_chunk`);
`parse_snapshot` checks the length before multiplying; a panic in `suspend` would end the runner process, which the parent
reads as `Closed` and falls back. Connection threads stay under `catch_unwind`.

**8. Everything else.** `pub` items added by the branch and the review are documented (`cargo doc -D warnings` clean); no
`println!` in `undra-transport`; SPEC 5.9, 5.10, 11.0 and 17 match the code after the M1/M2 edits; no new dependency.

## Device proof (re-run)

The `undra` AVD (emulator-5554, shared; it was running another agent's `com.example.demo`, which was put back in front
afterwards), the playground APK built from this branch (Gradle: up to date) against `undra dev --addr 127.0.0.1:7455` at
`fcbf09f`: Counter tab, nine taps (`9`, `9 changes`, odd), a comment appended to `core/src/counter.rs`: the bar read
**`Reloaded, state kept`** and the counter still `9`, `9 changes`. `logcat`: `Reconnecting(attempt=1, ... (1001 the core is
reloading))`, `Connected`, `dev server: Reloaded, state kept`; the dev server: `suspended for a reload: 0 call(s) were still
open, 0 sent meanwhile were not run, session 17e6f588 (2 object(s)) handed over`, `client reconnected ... away 1.0 s, 2
object(s) kept`. Restoring the file reloaded once more with the same result. Screenshots in the session scratchpad
(`review/android-3-counter9.png`, `review/android-4-after-reload.png`). iOS and web were not re-run (the brief asked for
one platform; the change on the device side is nil).

## Verification

* `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc
  --workspace --no-deps`: clean (at `1f7943d` and at the tip).
* `cargo test --workspace --no-fail-fast` (`UNDRA_REQUIRE_TOOLCHAINS=1`, `tsc` on `PATH`): at the tip (`fcbf09f`), 144 suites, **2,603 passed**, 0 failed, 11 ignored. At `1f7943d`:
  144 suites, 2,602 passed, 0 failed, 11 ignored (2,589 claimed at `12ec236` plus the 13 tests of that commit).
* `crates/undra-transport/tests/suspend.rs`: 11 → 17; `crates/undra-cli/tests/dev_reload.rs`: 4 → 8; `runner.rs` units +4.
* `bash contract-tests/run-all.sh`: **18 x 3 = 54/54**.
* TypeScript runtime: 1,132 passed (33 files), `npm run typecheck` clean. Kotlin runtime: 616 cases, 0 failed, 2 skipped,
  under kotlinc 2.4.20 **and** 2.0.21 (metadata `mv=[2,0,0]` checked on the 2.0.21 build). Swift runtime: 482 tests,
  0 failures.
* `crates/undra-transport/interop/run.sh ts` and `kotlin`: OK.
* `undra bindgen -C examples/playground --check --docs`: up to date (`0xddcdea47fa95a8d4`). `node
  site/scripts/build-all.mjs`: nothing to regenerate; `check-links.mjs --words`: clean.

## Open items for the integrator

1. L3: the Kotlin transport's send path during the `Hello` window (send to `opening` once its handshake is done).
2. I1: Swift should check post-`Hello` envelopes against the expected hash on a reconnect, as Kotlin does (pre-existing).
3. I4: send `reset` when `undra dev` gives up on a restore, and tell the new runner `reset no state to keep` for
   `NothingToKeep` so the bar can say it.
4. `.10x/status.md` / `handoff.md`: the runner protocol lines changed (`snapshot ok` has a `<not run>` field, `state` a
   `<lost calls>` field), `Suspended` gained `dropped_calls`, and `undra::dev` is reserved for the server (a core's record
   under it reaches the terminal only); B4 (devtools) should build on those.
