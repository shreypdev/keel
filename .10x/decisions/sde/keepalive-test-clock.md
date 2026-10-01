# SDE — `a_chatty_client_is_never_pinged` stops depending on thread timing (wt/flake, 2026-09-30)

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
