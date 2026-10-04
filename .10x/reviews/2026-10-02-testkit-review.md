# Testing kit (F1/F2, ADR-055) - adversarial review

**Date:** 2026-10-02 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:** `wt/testkit`
at `ea5cc38`, then `main` (`8fbb6ce`: devtools, persistence) merged in at the start · **Read:** `CLAUDE.md` (R1, R3, R4, R6, R11,
R12), ADR-055 and `.10x/decisions/sde/testkit.md`, `docs/TESTING.md`, SPEC 17.5, and `git diff main...HEAD` for
`crates/undra-testkit`, `crates/undra-transport` (`tap.rs`, the hook in `conn`/`session`), `crates/undra-ports/src/fakes`,
`crates/undra-cli` (`--record`, the runner template), the three kits and the runtime seams · **Scope:** the four surfaces of the
brief (replay determinism, the fakes conformance and the manual clock, `undra dev --record`, the public surface); nothing else ·
**Fixes:** `fix(testkit): review fixes` (one commit, listed below), then the record.

## Verdict

**Sound with fixes; merge.** No High. The design holds where it matters: a tap that reads and never reorders, a conformance
file that every kit fails against when one expected reply is changed, seams that cannot be reached from app code without an
opt-in, a kit with no dependency beyond its peer. Four Medium findings, all fixed with a test that fails without the fix:
`advance` on the manual clock never returned for a timer that re-arms itself without time passing, in all four implementations
(M1); a recording of a dev session carried every `SecureStore` value in the clear, hex-encoded, in a file whose stated purpose is
to be shared and committed (M2); the dev runner's `Clock` and `Rng` recorders both counted call ids from zero, so a recording
held two calls with one id, and a reader pairs a reply with the call of its id (M3); and a recording file that could not be
written was retried silently into a `eprintln!`, the in-memory recording grew without bound behind it, and the app was never
told that the recording had stopped (M4). Four Lows fixed (a missing reply was invisible to `finish()`; the native-`ctx.sleep` note
said what happens, not what a preview author does; `cargo doc -D warnings` failed on a private intra-doc link; the cross-merge
with persistence broke the kit's conformance generator). Open items below, none blocking.

The attacks that did not find anything are in the list too: the tap is observational (a session's change-set sequence is
identical with and without it), the harness has teeth on all four implementations, the opt-in is enforced by the compiler.

## Findings

**M1 - `advance` could never return (fixed; `fakes.ts:149`, `preview.ts:101`, `Fakes.swift:154`, `PreviewCore.swift:108`,
`Fakes.kt:142`, `PreviewCore.kt:80`, `clock.rs:118`, `fakes/mod.rs:230`).** The brief's attack: a timer that re-arms itself inside
`advance`. Every implementation loops "while a timer is due within the window", and a timer armed with a delay of zero from the
firing callback is due again at the same instant, so the loop never ends (a periodic 1 ms timer across a long window is the same
loop, only slower: `PreviewCore.advance` settles the core after each firing, about 20 ms natively). In TS and Rust `FakeClock.advance`
is synchronous, so a test hung the process; in `PreviewCore` it was an `await` that never resolved. Fix: a cap per call
(`maxTimers`: 1,000 for `PreviewCore`, 100,000 for a bare clock) and a typed failure when the cap is hit with another timer still due:
`TimerStormError` (TS, Swift; Swift `advance` is now `throws`), `TimerStormException` (Kotlin), a panic with the same sentence in
the Rust fakes (test support, documented under `# Panics`). The clock stays at the last deadline that fired and the timers still
armed stay armed. Tests: `ClockTests` in each kit (a re-arming timer stops at 50 with `fired`, `timerId`, `atMs`; a periodic timer is
fine until its window holds more firings than the cap; deadline order with ties in arming order and a timer armed inside the window
fires in the same call), and `fakes::clock::tests` / `fakes::tests` in Rust. Without the fix the TS test did not finish (8 s of the
runner's timeout, then killed).

**M2 - Secrets in a recording (fixed; `recorder.rs:85,93,131`, `main.rs` runner template, `cli.rs:355`).** `undra dev --record`
records every port call and reply the app answers, and `SecureStore.get`/`set` carry the secret as the argument or the reply. The
file is hex, not encrypted, and the docs invite committing it. Decision: **redact by default**. The calls of the `SecureStore` port
stay in the file (port, method, call id, status) with empty `args` and `body`, so the file is still a valid `undra.recording` that
every reader reads and every writer round-trips byte for byte (no new field: the format did not change), and `source` says
`"dev-server (SecureStore payloads left out)"`. `--record-secrets` keeps them; `Recorder::redact_secrets()` /
`redact_ports(&[..])` is the library seam (off unless asked for, because a Rust test that plays the platform records fake data).
HTTP headers (an `Authorization` header included) and bodies, `Kv` values and file contents are **not** redacted: they are the
app's own traffic and a replay needs them. TESTING.md, DEV_LOOP.md, the site page and `undra dev --help` now say so in one
paragraph, and say that a `Replayer` cannot answer a redacted call (it reports `mismatch`). Tests:
`redacted_ports_keep_their_events_and_lose_their_payloads` (a reply pairs with its call by id, so an unrelated reply that reuses a
finished call's id is not emptied), the `--record` integration test (`source`), `record_args` (the flag reaches the runner, and
only when asked).

**M3 - Two native recorders, one call id (fixed; `recorder.rs:336-350`).** `RecordingClock` and `RecordingRng` each had a counter
from zero (`0x8000_0000 + n`), and each wrote its call and its reply with two separate lock acquisitions. A recording could hold
`Clock.now_ms` call `0x80000000` and `Rng.fill` call `0x80000000`, with the two replies interleaved; readers pair a reply with the
call of its id, so one reply lands on the wrong call and the other is dropped. Fix: one process-wide counter, and the call and its
reply are appended under one lock (`push_all`). Test: `a_clock_and_an_rng_never_write_the_same_call_id` (two ids before, three
now).

**M4 - A recording that cannot be written (fixed; runner template `write_recording`, `Recorder::stop`).** The brief's attack: a full
disk. Before: one `eprintln!` per failed flush (a flush is attempted whenever the event count changed, so under traffic it repeats
twice a second), the recorder kept appending to memory for the rest of the session (a core that reads the clock a lot makes that
unbounded), a half-written `NAME.json.tmp` stayed behind, and nothing told the developer that the recording had stopped. After: the
first failure logs one WARN through the runner's own log (`cannot write the recording …; the recording stops here (what was written
before is kept), the dev server keeps serving`), removes the temporary file, stops the recorder (`Recorder::stop`: later events are
dropped, the tap and the native `Clock`/`Rng` wrappers stop appending) and the flusher exits; the server never stopped serving.
Test: `a_recording_that_cannot_be_written_warns_and_the_server_keeps_serving` (a directory that does not exist, on the real `undra
dev`: one warning, a call still answered, no file) and `a_stopped_recorder_keeps_what_it_has_and_drops_the_rest`.

**L1 - A recorded call with no recorded reply was invisible (fixed; `replayer.rs:84,258`, `ports.ts:104`, `Ports.swift`,
`Ports.kt`).** The brief: a missing port reply must be a typed error naming the port and the method, not a silent answer. The
replayers answered `Unavailable` (no hang, no crash) and `finish()` passed, with a test that said so on purpose ("it was made, the
answer was just empty"). A recording cut mid-call (the dev session ended while a request was in flight, a hand edit, a truncated
file) therefore replayed "clean". Now a fourth `ReplayError`, `unanswered { port, called, nth }` (`"Kv.get"`, not an id), is
recorded when the call is consumed and `finish()` fails with "call 0 of the port, Kv.get, has no reply in the recording; it was
answered unavailable". Replies are still paired with their call by id, not by position: a test pins the interleaved case (an Http
reply that lands after a Kv call and reply). Tests in all four kits; TESTING.md, SPEC 17.5, the site page and ADR-055's
amendment list it.

**L2 - The native `ctx.sleep` note did not say what to do (fixed; `docs/TESTING.md`, `testing.html`, both native `PreviewCore`
docs).** It said a native `ctx.sleep` "is waited for, not advanced". A preview author needs the consequence: `advance(5_000)`
returns at once, fires no timer for that sleep, and the task wakes 5 real seconds after it went to sleep; `settle()` does not wait
for it (a sleeping core is idle). The note now lists what to do (make the delay a parameter the preview sets to a few
milliseconds; wait the real duration and then `settle()`; preview the state after the wait with `RecordedCore` or by scripting
the ports; or run the story on the web, where sleeps follow the clock) and what keeps working natively (`Clock` reads such as
staleness). The Kotlin `advance` summary still said "completing a `ctx.sleep`"; fixed.

**L3 - `cargo doc --workspace --no-deps` with `-D warnings` failed (fixed; `replayer.rs:162`).** `Replayer`'s summary linked
`[module documentation](self)`, and the module is private (`rustdoc::private_intra_doc_links`). R4 puts docs on every `pub` item
and CI builds them with warnings denied.

**L4 - The cross-merge with persistence broke the kit (fixed in the merge commit; `conformance.rs`).** `Kv` now returns
`Result<_, StorageError>` and `FsError` has `Full` and `Unavailable`, so the conformance generator did not compile. Fixed (the
file it generates is byte-identical: success paths only). The Swift, Kotlin and TypeScript kits compiled and passed unchanged.
The playground's schema hash moved to `0xfa536b9ac6f06149`, so the two playground recordings were re-blessed
(`UNDRA_BLESS=1 cargo test -p playground-core --test testkit`); the conformance file did not change. Conflicts in the merge: the
runner template and `undra dev` (both pieces thread an option through `spawn`; now one `RunOptions`), `ServerConfig`
(`tap` and `devtools`), `Conn` (the tap sits where the frame is built, before `enqueue_locked`), SPEC, DEV_LOOP, CI, the Swift
`onLog` and the site (regenerated with `build-all`).

## Attacks and results

| Attack | Result |
|---|---|
| A recording replayed twice, change-sets byte for byte | TS: identical bytes (`ReplayTransport` driven directly, new test). Swift and Kotlin share the transport's design (a port of the same loop, no clock, no randomness, no map iteration order in the release path); not re-proved at the byte level. Rust has no `RecordedCore`; its replay of the playground session is `a_session_recorded_on_one_runtime_replays_on_another` (passes, re-blessed) |
| Equal `t` | Stable: change-sets are released in file order and `t` only gates them (test: three sets at `t = 20` arrive 2, 3, 4). A reader does not reject a `t` that goes backwards; file order wins (open item 3) |
| Port reply missing | L1 |
| Port reply out of order / interleaved across ports | Paired by call id; passes, test added (Rust, TS) |
| Reply with no call | Ignored by every reader (open item 2) |
| Recording of another schema | TS `UndraSchemaMismatchError`, message carries both hashes (test); Kotlin message carries both (test); Swift's error carries `expected` and `got` (test). Rust `Replayer` does not check (open item 1) |
| `fakes.json`, one Kv result and one Http status mutated | **Rust (stale-file test), TS (2 failures), Swift (2), Kotlin (2) all fail**; restored, all pass |
| `advance` and a timer that re-arms itself | M1 |
| Native sleep documentation | L2 |
| Tap changes timing or ordering | No. New test: the same session (28 calls and an observe) against a tapped and an untapped server, every envelope compared (kind, sequence number, replies, change-set signals and values; transaction numbers, handles and the log lines that name them are process-wide counters and differ), and the recording's transaction sequence equals what the client saw. The tap runs before dispatch for host-to-core frames and where the frame is built for core-to-host ones |
| The file is atomic, closed on reload, `NAME-2.json` starts clean | Write is temp + rename; a reload is a new runner process with its own recorder (`record_path`), so the second file starts empty; the first is closed by the final flush when its stdin closes. A standby runner writes its (empty) file as soon as it is built, so a swap that is abandoned leaves an empty `NAME-N.json` (cosmetic) |
| Disk full | M4 |
| Secrets | M2 |
| Generated code unchanged | `undra bindgen -C examples/playground --check --docs`: up to date, `0xfa536b9ac6f06149`; `build-reference` up to date for all three runtimes |
| Public runtime API widened | Swift: every new symbol is `package` (invisible to an app target; `site/reference/swift.html` unchanged). Kotlin: `Transport`, `TransportEvents`, `PortOutcome` (and its nested classes) and `attachTransport` carry `@UndraEmbeddingApi`; a file that uses any of them without the opt-in fails to compile (checked with `kotlinc` against the built runtime: six errors "this is the embedding API of the Undra runtime"). TS: nothing new |
| `@undra/testkit` has no runtime dependency | `npm pack`: no `dependencies`; the only import in `dist/*.js` outside the package is `@undra/runtime`, a peer; no `node:` import |
| Every `pub` item documented | `cargo doc --workspace --no-deps` with `-D warnings`: clean after L3 |

## Counts

Run once, on the tree after the fixes (`5682d3a`) merged with `main` at `8fbb6ce`.

| Check | Result |
|---|---|
| `cargo fmt --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` | clean (clippy flagged `too_many_arguments` on `spawn` and `start` after `--record-secrets` made eight; the two options are one `RunOptions` now) |
| `cargo test --workspace --no-fail-fast` | 2,921 passed, 19 failed: all 19 were "no TypeScript compiler (tsc) found" (8 in `undra-bindgen --test run_ts`, 11 in its typecheck suite; this shell had no `tsc` on `PATH`); `cargo test -p undra-bindgen` with the kit's `tsc` on `PATH`: all pass, so **2,940 passed, 0 failed**. `dev_reload` passed in this run |
| New Rust tests | 11: `fakes` 3 (re-arm, periodic inside the cap, a sleeping task), `recorder` 4 (redaction, stop, ids, and the existing ones kept), `replayer` 2 (unanswered, interleaved pairing), transport `tap` 1 (the order with and without a tap), `undra-cli` 2 (`record_args`, the unwritable recording) |
| Swift `swift test` (UndraRuntime) | 581 tests, 0 failures (3 `ClockTests` and 1 `PortTests` are new) |
| Kotlin `scripts/test-local.sh` | Kotlin 2.4.20 and 2.0.21 (the CI compiler, separate build dir, `UNDRA_FORCE=1`): 652 runtime cases + 30 kit cases (`ClockTests` 3 and a `PortTests` case are new), 0 failed, 2 `NativeSmoke` skipped as before |
| TS runtime `npm test`; kit `npm test`, `npm run typecheck` | 1,264 tests; kit 32 tests (6 new), typecheck clean |
| `bash contract-tests/run-all.sh` | **65/65**: S01..S20 and S26 on ts, kotlin, swift (63), S21 and S22 on ts (2) |
| The kit against the real core (inside `run-all.sh`) | TS 5, Kotlin T1-T4, Swift 4, all pass |
| Fakes conformance with a mutated Kv result and Http status | Rust, TS, Swift, Kotlin all fail on both; restored, all pass |
| `undra bindgen -C examples/playground --check --docs` | up to date, `0xfa536b9ac6f06149` |
| Playground builds | Xcode Debug (generic iOS Simulator) exit 0; `./gradlew --offline :app:assembleDebug` BUILD SUCCESSFUL; web `npm run build` OK |
| Site | `build-all` (nothing stale after the regeneration), `sync-chrome --check`, `check-links --words` exit 0; landing 342 words |
| Not run | React Native (see open item 7) |

## Open items

1. **`Replayer` does not check the recording's schema hash** (Rust, Swift, Kotlin, TS); only `RecordedCore.load` does. A port
   recording of another schema replays and fails by `mismatch` only if the arguments differ. A constructor option
   `expectedSchemaHash` would close it in four places.
2. **A reply whose call is not in the recording** is dropped by every reader without a word; a reader could report it.
3. **`t` is not validated to be non-decreasing** at read. The writers guarantee it (the recorder clamps), a hand edit can break it, and
   then the file order wins. Rejecting it is a reader change in four kits and a note in the format section.
4. **The Swift, Kotlin and TypeScript fakes have no fault injection.** The Rust fakes gained `MemStore::fail`, `fail_times`, `heal`,
   `FailingKv` and the `StorageError` and `FsError::Full/Unavailable` variants with persistence; the conformance file does not cover
   them, so the kits cannot drift only because they have nothing to drift from. TESTING.md now says "Rust only for now".
5. **`PortRecorder` (all three kits) and `Recorder` in Rust record a wrapped `SecureStore` in the clear** unless asked
   (`Recorder::redact_secrets`); only `undra dev --record` redacts by default. A `PortRecorder` on a device is the case that
   matters; the docs say so, the default is not changed.
6. **Swift's `Replayer` answers "unavailable" without counting** for a method id of a custom port that its recording does not
   hold (a fixed method table, documented in the class); Rust, Kotlin and TS report it as a deviation.
7. Not run here (as in the SDE record): React Native (no `node_modules` offline; no file under `runtimes/rn` or the runtime's own
   sources differs from main except the seams), Android Studio preview rendering (compiled, not rendered), Gradle's JUnit
   engine (the kit runs through its own `TestMain` under both compilers).
8. `crates/undra-cli/tests/dev_reload.rs` is load-sensitive on this machine (the SDE record's finding 6; the same signature as in
   the rn-adapters review); see the counts for this run.
