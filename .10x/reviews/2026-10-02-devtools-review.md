# Devtools (B4, ADR-054) — adversarial review

**Date:** 2026-10-02 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:** `wt/devtools`
at `ddc8d36` (`main` merged at the end) · **Read:** `CLAUDE.md` (R1, R2, R6, R9, R11, R12), ADR-054 and
`.10x/decisions/sde/devtools.md`, ADR-053 (what a restore carries), SPEC 5.10 / 13 / 16.2, and `git diff main...HEAD` for
`crates/undra-transport` (`devtools/{http,serve,hub,ring,proto,mod}.rs`, the hooks in `bridge`, `conn`, `session`,
`tracker`, `server`), `crates/undra-runtime` (`ext.rs`, `runtime.rs`), `crates/undra-query` (`inspect.rs`, `shared.rs`),
`crates/undra-cli` (`devtools.rs`, `runner.rs`, `commands/dev.rs`, the runner template, `tests/dev_devtools.rs`) and
`runtimes/ts/devtools` · **Scope:** the five surfaces of the brief; nothing else · **Fixes:** `e324ff8`, `13b9533`, `2342e49`,
and the counts with this record; then `main` merged.

## Verdict

**Sound with fixes; merge.** No High. Three Medium findings, all fixed with a test that fails without the fix: the
WebSocket upgrade at `/devtools/ws` answered a missing or wrong token with the WebSocket library's own bare `404` (no
headers), not the one `404` every other `/devtools` request gets, so a prober could tell a server with devtools on from one
with them off, and the connection state was allocated before the token was looked at (M1); a panicking inspector was
contained but never reported, never counted and asked again at every sample, each time paying for a backtrace (M2); and the
query cache was sampled at every pass of the worker's loop, not once a second (four times a second idle, up to one in ten
milliseconds under a commit burst), with the cache's lock held while the whole document was formatted (44 ms on 10,000
entries in a debug build, 3.8 MB of JSON) (M3). Four Lows fixed (a snapshot's cost was unbounded on a big state, the app
was not told how many stores a time travel dropped, a dead worker left the app unfiltered, an unchanged state over the
4 MiB bound was listed again at every capture). One Low open (a step's `through_seq` label can lag its content during a
storm).

The rest held against every attack I could aim at it, and the attacks are tests now: the token is 128 bits from
`/dev/urandom`, never in a command line, stdout or stderr (asserted on the real `undra dev`); the app client receives
exactly what it observed before, during and after a page, with two pages, with a reconnect while a page is attached and
across a reload; a restore converges the shipped TypeScript runtime's mirror on the core; the ring's three bounds are exact at
their edges; the worker and the observation exist only while a page is attached; a commit storm at 60,000 commits a second
with a page attached costs one step per 10 ms and nothing is dropped from a page that reads, and a page that stops reading is
dropped without the core or the app waiting for it.

## Findings

**M1 — The socket's refusal was not the one `404`, and its state came before its token (fixed, `e324ff8`;
`serve.rs:56`).** ADR-054 section 6: a missing or wrong token, an unknown path, devtools off and a method other than `GET`
are "the *same* `404` (same status, headers and body), so the endpoint does not announce itself". For `/devtools`,
`/devtools/<asset>` and anything else that goes through `http::respond_to` that held. For `/devtools/ws` the token was
checked inside the upgrade callback, so a refusal was tungstenite's `ErrorResponse`: `HTTP/1.1 404 Not Found\r\n\r\nNot
Found`, with none of the eight headers the page's `404` has; and a plain `GET /devtools/ws` with no upgrade headers got no
answer at all. With devtools off, `/devtools/ws` got the full `404`. So one request told a server with the page on from one
with it off. The `Conn`, its queue and its registry slot were also allocated before the token was read. Fixed: the token is
in the request line, which is already peeked, so `serve::run` checks it first and answers a request without it with
`http::serve_get`, the very code that answers every other refusal; nothing is allocated for it. (A holder of the token that
sends an origin that is not allowed still gets the library's refusal: it holds the token, nothing is announced to it.)
Test: `every_refusal_is_the_same_bytes_and_a_server_with_devtools_off_answers_the_same`, 15 request shapes (no token, wrong
token, a token that is too long or too short, `..` and `%2e%2e`, `POST`, `HEAD`, upgrade or plain, bad origin) against a
server with devtools on and one with them off: every answer is byte-for-byte the same. It fails on the old code at
`GET /devtools/ws`. Timing: 1,500 requests per case, medians 64 to 97 microseconds with no token, a wrong first character, a
wrong last character, a good token and no asset, and the two upgrade forms; the spread between runs of one case is larger than
the spread between cases (`token_matches` reads the whole expected token whatever differs).

**M2 — A panicking inspector was silent (fixed, `e324ff8`; `ext.rs:67`, `runtime.rs:840`).** `Inspectors::inspect` ran
the inspector under `guarded` and threw the `PanicReport` away with `.ok()`. R6 is met (nothing escapes), but nothing was
logged, `panics` in `undra_stats_json` did not count it, and the hub's worker asked again at every sample, so a broken
inspector cost a backtrace (the guard formats one with `force_capture`) several times a second for as long as the page was
open. Fixed: the first panic is logged once at level 5 (`undra::panic`, with the backtrace, like every other guarded
place), counted, and the inspector is skipped until a new one is registered under its name. Tests:
`a_panicking_inspector_is_contained_reported_once_and_not_asked_again` (fails on the old code: asked five times, no record,
`panics` 0) and `an_inspector_that_asks_the_runtime_for_a_snapshot_or_another_inspector_does_not_deadlock`.

**M3 — The query cache was sampled up to a hundred times a second, under its lock (fixed, `e324ff8`; `hub.rs:744`,
`inspect.rs:130`).** `sample_queries` ran on every pass of the worker's loop that had no snapshot pending: every 250 ms
idle and, during a commit burst, once after every snapshot (up to 100 a second). `describe` formatted every entry while
holding the cache's `state` lock, so every query operation waited behind it. Measured on 10,000 entries with a value each:
3.8 MB of JSON in 44 to 87 ms (debug), 8 ms (release), the lock held for all of it. Fixed twice: the lock is held only to
copy a row (a `String` and some numbers, the values are `Arc`s), 3.5 ms debug and 1.4 ms release for the same cache; and
the sampler runs at most four times a second and nine times as far apart as the last sample took (so one tenth of a core at
most; about once a second on this cache in debug). Test: `sampling_a_cache_of_ten_thousand_entries_stays_within_a_budget`
(a 1.5 s bound on the document, debug); the cadence is the hub's own and is exercised by the storm test.

**L1 — A snapshot's cost was unbounded on a big state (fixed, `e324ff8`; `hub.rs:59`, `hub.rs:536`).** A snapshot holds the
core lock. The worker took one per 10 ms burst whatever it cost, so a state whose snapshot takes 30 ms would hold the core
for three quarters of every second while a page is open, and the 4 MiB bound only decides what is *kept*, not what is
*taken*. Now the next snapshot waits for `max(10 ms, 9 x the last one's duration)`, at most 2 s: the snapshots take a
tenth of the worker's time at most. Small states are unaffected (the storm test is unchanged at one step per 10 ms).

**L2 — The app was not told how many stores a time travel dropped (fixed, `e324ff8`; `hub.rs:669`).** The brief and
ADR-053's reload notice say `(N objects not carried over)`; the time-travel notice said `time travel: step N` only, and the
count went to the page alone. It is now `time travel: step 1 (1 store(s) built since are gone)` when a store was dropped
(unchanged otherwise). Test: the dropped-store test now reads the notice from the app's session. SPEC 5.10 and DEV_LOOP say so.

**L3 — A dead worker left the app unfiltered (fixed, `2342e49`; `hub.rs:491`).** The worker's panic guard set `active` to
false and left the pages attached: the hub still observed every store, but the bridge stopped filtering, so the app client
was sent every entry the runtime emitted, including stores it never observed, until the pages left. Now the pages are
closed (1001), each leaves through `detach`, and the last one undoes the observation as it always does; a page that comes
back gets a new worker. Test: `a_dead_worker_closes_the_pages_and_leaves_the_hub_to_undo_its_observation` (the policy; the
panic itself cannot be provoked from outside, since the inspector and the restore are guarded below it).

**L4 — A state over 4 MiB was listed again at every capture (fixed, `2342e49`; `ring.rs`).** The dedupe compared with the
newest step's bytes, and a step over the per-step bound has none, so an unchanged big state (a commit that changes only a
query handle, which a snapshot does not hold) pushed a new non-restorable step every time and evicted the restorable ones.
A step that was not kept now carries its length and a hash. Test: `the_same_state_is_not_listed_twice_whether_or_not_it_was_kept`.

**L5 — A step's `through_seq` can lag its content (open).** `capture` reads the change-set sequence before it takes the
snapshot, so a commit between the two is in the state and not in the label; the next capture finds the same bytes and adds
no step. Only visible under a storm (a commit every 16 microseconds against a snapshot of 50 to 100): the timeline would
show that commit after a step that already contains it. The label cannot be read after (it would claim commits the state
does not have) and cannot be read under the core lock from the transport; fixing it needs the runtime to return the
sequence with the snapshot. Left, noted in the sde record's open items.

**I1 — The token is in the runner's environment for as long as the runner lives.** Not in any `argv` (asserted for every
process on the machine), not in stdout or stderr (asserted), written once in the banner. `UNDRA_DEVTOOLS_TOKEN` stays in
the runner's environment, so a process the core spawns inherits it, and `ps eww` shows it to the same user and root
(argv is shown to everyone). The runner is generated and `forbid(unsafe_code)` is not set in it, but `remove_var` is
`unsafe` in edition 2024; left. A hardened variant would hand it over on stdin.

**I2 — The page's CSP says `connect-src 'self' ws: wss:`** which lets a script connect to any WebSocket host (Safari's
older `'self'` does not cover `ws:`). Inert today: `script-src 'self'` stops inline script and the page builds its DOM with
`createElement`/`textContent` (no `innerHTML`, `insertAdjacentHTML`, `eval` or `new Function` in `runtimes/ts/devtools/src`).

**I3 — `HEAD` is answered with the body (a `404` with `Not Found`);** and a plain `GET` of any path that is not `/devtools*`
is hung up on without an answer, so the listener can be told from a web server whichever way the page is configured. Both
harmless.

## The attacks

**1. The token and exposure — holds (M1 fixed).** *Identical 404:* above; the same matrix against a server with devtools
off. *128 bits from the OS:* `new_token` reads 16 bytes from `/dev/urandom`; the only fallback (no such file, i.e. Windows)
takes the keys of `RandomState`, which the OS seeds, and mixes the clock in addition, never as the only source; `a_token_is_32_hex_digits_and_not_the_same_twice`.
It is made once per `undra dev` (`dev.rs:63`), so a reload keeps the page's address. *Never logged:* the hub's and the
server's log lines never carry it (`DevtoolsConfig`'s `Debug` prints `(hidden)`; the refusal line prints the origin, not the
URL); `the_page_is_served_behind_its_token_and_time_travel_restores_the_app` now reads `ps -ww -ax -o args=` for the whole
machine (the token is on no line), and the stdout and stderr of `undra dev` after the banner. *The runner's argv:* `--devtools`
only; the token is `Command::env`. *Auto is off on a non-loopback `--addr`:* `enabled()` is true for `localhost` and
loopback IPs only (`127.1`, `[::ffff:127.0.0.1]`, an empty host and a bare port fail closed); unit test over thirteen
addresses; the banner says why it is off. *The upgrade checks the token before allocating anything:* M1 (a `Conn` was
allocated first; now nothing is, and the hub is touched only after the handshake). *Same origin policy as the app's:*
`config.origin_policy` is the one the app socket uses; `https://evil.example` is refused with the page's token in hand (`null` is not a
local origin either: `origin.rs` unit tests) (`a_page_from_an_origin_that_is_not_allowed_is_refused`). *A page cannot send app-level envelopes:*
`a_page_cannot_reach_the_core_through_its_socket_whatever_it_sends` sends a real `Call` envelope (it would add 100), all 256
tag bytes with no body, a short body and a restore-sized body (768 messages), a text frame and a 200 KiB message, each on a
fresh page: every one that is not a valid `Resync` or `Restore` closes the page, the core's `calls` counter moved only
for the app's own read, the counter is where it was, no log line says `panicked`, and a new page still attaches.

**2. Observe-all routing — holds.** `the_app_gets_exactly_what_it_observed_before_during_and_after_a_page`: the app watches
store A's `count` only; B changes before the page (nothing), with it (the page sees it, the app nothing; A's unobserved
`label` neither), after it leaves (nothing), and when the app then observes B it is sent B's current value (103) and no
other. `two_pages_share_the_observation_and_the_last_one_out_restores_the_app`: the first page leaving does not release
the hub or blind the second; the second leaving does. `an_app_that_reconnects_with_a_page_attached_is_sent_only_what_it_asks_for`
(ADR-051 resume while a page is attached: the page is told the app left and came back; the resumed connection is sent
nothing until it observes, then only A, and still only A after the page leaves).
`a_reload_with_a_page_attached_hands_the_app_session_over_without_the_hubs_observations`: `Server::suspend` with a page
attached hands over the app's handles and nothing the hub took (`KeptSession` holds handles only; the hub never writes to
the app's tracker); the successor core, restored from the snapshot, sends the resumed app nothing until it observes again
and then only what it asks for; its first page has another `core_epoch`. A race I could not close in a test and read
instead: between `observe(off)` and the re-`observe(on)` of `release_observation` an app that observes or unobserves at the
same instant can leave one signal observed that it just dropped, which is one unobserved entry sent to the app; Info only.

**3. Time travel and bounds — holds (L1, L2, L4 fixed).** *Convergence on the TypeScript runtime:*
`crates/undra-transport/interop/ts-devtools.mjs` (run by `interop/run.sh`) drives the shipped, unmodified `RemoteTransport`
as the app, observes a counter, and a plain WebSocket speaks the page's `[tag][body]` messages: attach (step 1 = 5),
`add(3)` (mirror 8), `Restore {step 1}` (answer `ok`, 0 dropped), the mirror converges on 5, `add(0)` through the core
returns 5, the dev notice `time travel: step 1` arrives, the app carries on (7) and keeps following after the page leaves.
*The ring's bounds:* `the_documented_bounds_hold_at_their_edges` (200 kept, the 201st evicts only the oldest; 4 MiB kept,
4 MiB + 1 listed with its size and not kept, weighing nothing; eight 4 MiB steps fill 32 MiB exactly, the ninth evicts one;
one byte over the total evicts) and the per-step test through the server (`a_state_over_the_per_step_limit_...`).
*Memory freed:* `clear()` drops every snapshot when the last page leaves (`a_step_that_was_not_kept_is_evicted_...`);
`the_worker_and_the_observation_exist_only_while_a_page_is_attached` (unit test on the hub: no worker, no events channel, no
held store, no step before the first page; a worker after it; one still after the first of two leaves; none after the last).
*No worker when no page:* the same test, and `nothing_is_recorded_or_counted_while_no_page_is_attached` (300 commits with no
page, then a page: its counters start at 0 commits and 1 step; after it leaves and 50 more commits, again).
*A commit storm:* the decision is **both a cap and back-pressure**. Steps are capped by time (one per 10 ms, farther apart
when a snapshot is slow, L1) and the page's own queue is the back-pressure: past `max_queued_bytes` (64 MiB) the page is
dropped, never waited for. `a_commit_storm_costs_steps_by_time_not_by_commit_and_the_ring_stays_bounded`: 20,000 pipelined
`add` calls at about 60,000 a second with a page attached: the page received all 20,000 change-sets and 33 steps (the bound
asserted is one per 10 ms of wall time plus 20), the newest step covers the last change-set, the ring is within 200 steps and
32 MiB, the core holds 20,000, and a restore of a step from the storm works. `a_page_that_stops_reading_is_dropped_...`:
a page that never reads (queue bound 128 KiB) against 40,000 commits: the app was not held up, the page was dropped, the hub
released, the app's observed set is what it asked for. *Stale handles and the notice:* L2; the existing
`restoring_a_step_from_before_a_store_existed_...` shows status 5 for the dropped store's handle and now the notice.

**4. The runtime seam — holds (M2, M3 fixed).** *Panic:* M2. *Thread and lock:* an inspector runs on the thread that
asks (the hub's worker; the doc comment says so now) with none of the runtime's locks held by `inspect` (the slot is cloned
out of the list first, so registering from inside one works); `undra-query`'s takes only the cache's own lock, and
since M3 only to copy rows. Re-entrancy: an inspector that calls `rt.inspect(..)` and `rt.snapshot()` is fine from a test
thread, from another thread and from a task on the core (`snapshot` is read-only and enters the core only if it can), and the
core's entry points fail `E_REENTRANT` rather than deadlock. No thread that holds the core lock ever waits for the worker (the
core's threads only enqueue events), so no cycle. *Cost at 10,000 entries:* M3.

## Verification

Fixes and tests are in `e324ff8`, `13b9533` and `2342e49`. Counts after the final merge of `main` (`ee73ddc`):

| Check | Result |
|---|---|
| `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings` | clean |
| `cargo test --workspace --no-fail-fast` (tsc on PATH, `UNDRA_REQUIRE_TOOLCHAINS=1`) | 2,790 tests (2,789 passed, 14 ignored); the one failure was `dev_reload` under load, see Open: it fails the same way on `main` |
| `undra-transport` / `undra-runtime` / `undra-query` | 228 / 335 / 194 pass |
| the 3 real `undra dev` tests (`dev_devtools.rs`) | 3 pass (page and token matrix with the `ps` check, `--devtools off`, a rebuild with a page open) |
| `bash contract-tests/run-all.sh` | 60 of 60 cells pass (S01 to S19 and S26 on ts, kotlin, swift) |
| interop `run.sh` (TypeScript incl. the devtools script, Kotlin) | `interop OK`: TypeScript 9 checks, the new devtools script 5, Kotlin 8 |
| TS runtime `npm test` / typecheck | 1133 / clean |
| devtools page `npm test` / typecheck / `build.sh --check` | 71 / clean / assets equal a rebuild, 17,395 bytes gzipped (budget 153,600) |
| `undra bindgen -C examples/playground --check --docs` | up to date (hash 0xc5f05c376fde398c) |
| site `build-all` / `check-links --words` | no change to `site/` / clean, landing prose 342 words |
| browser pane, one screenshot | the page against `undra dev` on a copy of the playground: scrubbed back one step with the app attached; the page says `restored step 5`, the Counter store shows 2, the timeline has `restore to step 5` as step 7; the app's mirror followed (3 to 2) and its dev notice read `time travel: step 5` (see Open for what was the app) |

## Not verified, and open

* L5 above. The mirror's drains and merges never reach the dev server (the sde record says so).
* `main` carried a stale golden: `crates/undra-cli/tests/golden/stores/**` (`a32d4ec` changed the generator's wording for a
  derived list and updated every golden but this copy), so `cargo test --workspace` failed on `bindgen_schema` before
  this review touched anything. `main` fixed it in `ee73ddc` while the review ran; my identical change merged without a
  conflict.
* `dev_reload` (8 real `undra dev` reload tests) is flaky when the machine is loaded (load average 10 to 20 here, other
  agents building): different tests fail on different runs at the same two assertions (`dev_reload.rs:145`, a `Close` frame
  expected, and `:172`, a socket closed under a call; the client's error is `ResetWithoutClosingHandshake`). It is **not
  caused by this branch**: the same file on `main` (`1b9b605`, no devtools) failed in 3 of 4 runs at the same load, 5, 2 and 5 of
  8 tests, at the same two lines, and passed in the fourth. On this branch it failed in 4 of 5 full-file runs (1 or 2 of 8 tests each) and
  passed the fifth, and the failing tests pass alone. I did not find the cause (a close handshake lost when the old runner or the connection is
  torn down while the test's thread is starved is my guess); it belongs to ADR-053's tests.
* The screenshot's app was the shipped TypeScript runtime under Node (`@undra/runtime`, platform `web`), not the
  playground's browser tab: the Browser pane was at its tab cap and `window.open` is blocked, so the page and the playground
  could not be open at once, and Claude in Chrome was not connected. The playground tab did connect to `undra dev`
  (`?undra=ws://...`, green bar, Counter tab) before I replaced it with the page. The 10,000-row screen and the stress
  screen at 10k/s were not driven in a browser; the storm is the transport test above (60,000 commits a second).
* An OS-level thread count: the tests' process shares its threads with every other test, so "no worker" is asserted on the
  hub's own handle, not on `ps -M`.
