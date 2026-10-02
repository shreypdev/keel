# SDE — `dev_reload.rs` fails under load: the test client was not a client (wt/dev-reload-flake, 2026-10-01)

`crates/undra-cli/tests/dev_reload.rs` (ADR-053's nine integration tests: the real `undra dev` on a copy of the
playground and a raw WebSocket client) failed under parallel load: at `:145` (`expect_close`: `a Close frame`) and `:172`
(`call`: `closed while waiting for a reply: None`), 3 to 5 of 9 in four runs at load 11 to 29, each passing alone. Measured
again here on a clean `main` at load 17: 5 of 9 (`a_rebuild_keeps_the_screen_the_client_was_on`,
`a_rebuilt_core_that_does_not_start_leaves_the_old_one_serving_with_its_state`,
`no_keep_state_starts_every_rebuilt_core_fresh`, `a_snapshot_the_new_core_refuses_...`,
`a_schema_change_the_state_cannot_follow_...`).

## The cause (reproduced on a quiet connection, not guessed)

It is none of the three hypotheses in the brief. The server drops the client as dead; the test client never knew it was
being asked to answer.

* The dev runner serves with `ServerConfig::default()`: `ping_interval` 5 s. The writer pings a peer that has been silent
  for one interval and **drops it, with no Close frame, after three** (15 s): `Verdict::Dead` in `writer.rs`, a
  `shutdown(Both)`. The doc comment on `ping_interval` says why it is safe: "every client answers pings without any code of
  its own". A browser, OkHttp and `URLSessionWebSocketTask` do: their reader runs for as long as the socket lives.
* The test's `Client` was a bare tungstenite `WebSocket` read on demand. tungstenite answers a Ping *inside* `read()`, and
  the test calls `read()` only when it expects a frame. Every test then waits for a rebuild (`dev.wait_log`,
  `dev.wait_line("Restarted: ...")`, up to 600 s) with its client connected and **reading nothing**. A rebuild of the
  playground core takes a few seconds alone and well over 15 s on a loaded machine; the server pinged at 5 s and 10 s,
  got no Pong, and dropped the client at 15 s. The test woke up, found a socket that ended without a Close frame
  (tungstenite: `ResetWithoutClosingHandshake`, which the helper maps to `Got::Closed(None)`) and failed at 145
  (`expect_close`) or at 172 (`call` on the "old core still answers" probe, a client dropped during the broken-edit rebuild).
* Why "passes alone": a quiet machine rebuilds inside the window. Why "3 to 5 of 9": the tests that wait for two rebuilds
  or for a slow one cross 15 s first.

Reproduction without any load (a throwaway test, removed): connect to `undra dev`, send `Hello`, then read nothing and write
nothing. The dev runner's own log: `02:56:41.161 client connected` ... `02:56:56.169 client disconnected`: 15.008 s, no
Close frame; the first read afterwards fails.

### Hypotheses (a), (b), (c)

* (b) the server tears the socket down before the Close frame is flushed: **no**. The writer writes the Close frame
  (`ws.close` writes and flushes) and only then starts the linger; the forced `shutdown(Both)` comes after it, and the reader's
  `drain` keeps a client that is still sending from being reset. Pinned anyway by a new transport test (below): a client that
  keeps sending through the close and reads nothing for longer than `close_timeout` still finds Close 1001 first.
* (a) the close-handshake deadline expires and the client sees a reset: **no, and it could not hide the frame** for the
  same reason. (It does mean a suspend waits `close_timeout`, 2 s, for a client that is not reading: another effect of the
  test client not reading; a real client answers at once.)
* (c) a short per-frame deadline: **a second, smaller bug in the same helper**. `await_count` set the socket's read timeout
  to 200 ms and restored it only on the failure path; on success it returned with 200 ms left, so every later `read()` of that
  client (the next `call`, `expect_close`) waited 200 ms for the server instead of 30 s. `a_rebuild_keeps_the_screen_...`
  and `a_rebuilt_core_that_does_not_start_...` both call `await_count` and then wait for more frames. Fixed by the same change.

## Which side is wrong: the test

The server did what it documents. Dropping a peer that answers no ping is the point of the keepalive (a half-open connection
holds the one client slot and the relaunched app is refused); a test that cannot answer is a dead peer as far as anyone can
tell. Widening the server's window (a longer `ping_interval` for `undra dev`, or a dev-only environment variable) would only
move the load at which the tests fail, and would make a phone that left the Wi-Fi hold the slot longer. So the server is
unchanged and the harness became a client.

## What changed

* `crates/undra-cli/tests/dev_reload.rs` (`Client`): the socket is serviced by a thread of its own (`pump`) for as long as the
  client lives, which is what a platform runtime's reader is. It sends what the test queues (`send` no longer touches the
  socket), reads with a 10 ms tick, lets tungstenite answer Pings and Pongs, flushes the echo of a Close, and hands everything
  else to the test through a channel in order. `read()` is `recv_timeout` on that channel (patience 30 s, as before);
  `read_within(wait)` is a read with another patience for that read only (`await_count`, `notices_after`), so no timeout is
  left behind. Dropping the client ends the thread and closes the socket. The semantics the tests use are unchanged:
  `Got::Frame` / `Closed(frame)` / `Closed(None)` for a connection that ended without a Close / `Silence`.
* `crates/undra-transport/tests/suspend.rs`: `a_client_that_keeps_sending_and_reads_late_still_finds_the_close_frame`, the
  invariant the dev tests depend on and that a late reader exercises: a client sends a call every 20 ms through the suspend,
  reads nothing while the server waits out `close_timeout` and closes the socket, keeps sending into the closed socket, and
  then reads: Close 1001 `the core is reloading` is the first thing it finds, and the suspend counted the calls it was sent
  (`dropped_calls > 0`).
* No server, wire, ABI, schema or generated-code change.

## Evidence

* Before (the old `Client`, the same machine): 5 of 9 failing at load 17 and 6 of 9 at load 19 (the six: the five above and
  `an_additive_schema_change_keeps_the_state`).
* After: `cargo test -p undra-cli --test dev_reload` run **10 times in a row, 90 of 90 passed**, other agents' builds
  and test suites running on the machine the whole time (load average at the start of each run: 35, 83, 54, 38, 29, 22, 62,
  41, 25, 26; one run took 37 s and the slowest 86 s). The file is the same code both times; only the harness changed.
* `cargo test -p undra-transport`: all 17 binaries green, `suspend` 18 (the new one included; it also passed 40 of 40 run
  back to back). `cargo clippy -p undra-cli -p undra-transport --all-targets -- -D warnings` clean, `cargo fmt --check` clean.
* `bash contract-tests/run-all.sh`: 74 of 74 (ts 26, kotlin 24, swift 24). A fresh worktree has no `wa-sqlite` under
  `runtimes/ts/@undra/runtime/node_modules` (the lockfile's 1.0.0, an optional peer the TypeScript column's S25 imports), so a
  first run read `ts S25 MISSING`; it was copied from a sibling worktree (gitignored, no network) and the run repeated.
  Worth a line in `docs/ONBOARDING.md` for the next fresh worktree: `npm ci` in `runtimes/ts/@undra/runtime` before the
  contract suite.

## Not done, on purpose

* The server says nothing when it drops a peer as dead (`client disconnected (0 calls cancelled, ...)` is the only line).
  A reason in that line (`it answered none of the pings sent in 15 s`) would have made this a two-minute diagnosis; it is a
  change to `writer.rs`, `conn.rs` and `session.rs` (a flag the writer sets and the teardown reads) and left for the
  integrator to decide, since three branches are touching the transport.
* `tests/dev_devtools.rs::a_rebuild_with_a_page_open_...` holds an app client and a page idle through a rebuild the same way.
  It does not read them afterwards (it connects again), so it cannot fail on this; the harness there is the old shape.
