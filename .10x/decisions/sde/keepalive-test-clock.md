# SDE — two flaky `undra-transport` tests (wt/flake, 2026-09-30)

## 1. `a_chatty_client_is_never_pinged` stops depending on thread timing

Two agents saw `a_chatty_client_is_never_pinged` fail during full `cargo test --workspace` runs
under heavy concurrent builds; it passed alone.

## What it depended on

It was a real-socket test: a 150 ms `ping_interval`, a raw client that sent every 50 ms with
`thread::sleep`. The keepalive measured silence with `Instant`, so a test thread (or the server's
reader) descheduled for more than 100 ms made a talking client look silent for a whole interval and
the writer sent a Ping. A stalled thread is indistinguishable from a silent peer in real time, so no
margin could make that test exact.

## What changed

`undra-transport` only; no public item, wire shape or `ServerConfig` field.

* `src/conn.rs`: the keepalive reads time through an injected `Clock` (`Arc<dyn Fn() -> Duration>`)
  that defaults to the real one. `Conn::new` is unchanged; `Conn::with_clock` is `pub(crate)`.
  `silent_for()` became `silent_at(now)`: silence is judged against one reading of the clock, taken
  before the timestamp is loaded, which also makes the answer exact if a thread is descheduled
  between the two. A `#[cfg(test)]` `ManualClock` holds time still until `advance`.
* `src/writer.rs`: the schedule (when the next check is due, Quiet / Ping / Dead at one and three
  intervals) moved out of the loop into a `Keepalive` that is told what time it is. The writer
  thread's behaviour is the same. The schedule is built in `spawn`, on the caller's thread.
* `tests/lifecycle.rs` -> `src/writer.rs`: the test is now a unit test (same name) over a real
  `ReadHalf` on a loopback socket and a `ManualClock`: the peer sends every 50 ms of fake time for
  600 ms and the checks that fall due at 150/300/450/600 ms all find it alive, no Ping, no drop. The
  test thread plays the clock and the writer's tick, so the result does not depend on scheduling.
  Companions in the same rig keep it honest: a silent peer is pinged at one interval and dropped at
  three; a peer that speaks after a ping is spared; zero switches keepalive off. Removing the
  `touch()` call from `ReadHalf::read` makes the chatty test fail (checked by hand).
* `conn.rs`'s `silence_is_measured_from_the_last_bytes_received` had the same shape (`sleep(40ms)`,
  `< 30 ms` after a touch); it is exact now.

## What stays real-time

`a_client_that_goes_silent_is_pinged_and_then_dropped_so_it_cannot_hold_the_slot` and
`a_client_that_answers_pings_is_kept` (tests/lifecycle.rs) still run the whole server on the real
clock; they keep the end-to-end proof that `ServerConfig::ping_interval` reaches the writer and a
Ping goes out on the wire. The first only needs silence, so a stall cannot break it; the second
needs the client to read within three intervals (300 ms) and could in principle flake under a
stall of that size.

## 2. `byte_fuzz_before_the_upgrade_never_hurts_the_server`

Failed 1-2 times in 50 full runs under load, always as `Connection reset by peer` from the final
`f.client()` in `assert_healthy`; passed alone.

### Root cause (reproduced, not guessed)

The server refused the well-formed client. `Shared::spawn_connection` (`src/server.rs`, the
`registry.conns.len() >= max_connections` check) drops a socket on accept when 16 are open, with the
client's upgrade request unread, so the kernel answers with a reset. `max_connections` counts every
socket "upgrading, attached or closing" from accept until its connection thread has finished (`Shared::detach`, after
the socket is already closed), and the test opens 150 junk sockets in a tight loop and drops each at once:
under load the connection threads fall behind the accept loop, the table holds 16 not-yet-reaped junk
sessions, and the next connection is refused. Confirmed by logging the peer port on the refuse path
and matching it to the good client's local port: one failing run in 80 under CPU load had the good
client's own port in `dropping a connection: 16 are already open`; passing runs never had it. (The
junk phase itself is refused a few times in many passing runs, which also made the fuzz's coverage
depend on the scheduler.) The test's `assert_healthy` waits only for `bridge.is_connected()`, which junk
never sets, so it did not wait for the reap at all.

### Which side is wrong: the test

The limit is documented as counting upgrading sockets, and it is a flood guard (threads and file
descriptors per socket): counting only upgraded sessions would remove it for exactly the connections
that cost a thread and have proved nothing. Releasing a failed upgrade's slot before closing its socket
would not help either (the junk clients never wait for the close). `connections_beyond_the_limit_are_dropped_on_accept`
pins the behaviour. So the server is unchanged (its doc comment now says when a socket stops counting) and the
test no longer lets junk reach the limit: `max_connections` is `JUNK_CONNECTIONS + 16`, so every junk
socket and the fresh client fit at once. That is deterministic by construction; waiting for the reap would
have needed a new public accessor for the registry size, and polling the good client's connect would only
have retried the refusal. 0 failures in 200 runs under 8 busy loops (it was 3 in 60 before).

### Ephemeral-port exhaustion when run back to back

Each junk client dropped its socket first, so each connection's port stayed in TIME_WAIT (macOS:
2 x 15 s). The test takes ~70 ms and made 150 connections: ~100 runs in under 10 s took all 16384 ephemeral
ports (TIME_WAIT 16.4k) and every later connect, in any test or process on the machine, failed with `Can't assign
requested address` for ~30 s. The fix keeps all 150 payloads (fewer would not have helped: the exhaustion
is about rate) and has each client read, for up to 200 ms, until the server closes first. Garbage makes the server give up and
close at once (one corpus case, an unterminated header block, is legitimately waited on and the client
closes first as before), so the TIME_WAIT is on the server's single listening port and the clients'
ports are free at once. Measured: 300 back-to-back runs from 3 parallel loops (TIME_WAIT peaked at 45.8k,
nearly three times the ephemeral range) with 0 failures; before, the 95th run failed. `SO_LINGER(0)` would also work but
needs `socket2` or `unsafe` (R2), not worth a dependency for a test.
