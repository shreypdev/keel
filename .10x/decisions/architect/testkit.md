# Testkit (Track F: F1 preview/fake cores, F2 port record/replay), 2026-10-01

Status: decided by the implementer, recorded as ADR-055 (`.10x/adrs/ADR-055-testkit.md`). Nothing here touches the
wire, the C ABI, the schema or a generated shape (R1, R3, R11): the kits sit beside the runtimes.

## Premise corrected

The brief says recorded change-sets exist as contract fixtures. They do not: `contract-tests/wire-vectors.json` is a table of
byte vectors (`name, type, value, hex`) and the scenarios run live. The recording format below is new; it reuses that file's
conventions (lower-case hex, deterministic order, a `note`-style informational field).

## Decisions

1. **Where recording happens: both, one format.** (a) `undra dev --record <file>` is the cheapest capture: the dev server already
   sees every envelope in both directions for iOS, Android and web alike, so one Rust tap (`ServerConfig::tap`) records the whole
   session (calls, replies, change-sets, stream items, port calls and replies, events, timer reports, observes). The dev runner
   also wraps its Rust-bound `Clock` and `Rng` so their readings are in the file (those two never cross the socket). A reload
   starts a new core, so a new file: `session.json`, `session-2.json`, ... (a recording belongs to one schema hash).
   (b) `PortRecorder` in each runtime kit wraps real adapters for sessions that are not dev ones (a device, a test) and records
   port traffic only. Replayers read `port_call`/`port_reply` only; `RecordedCore` reads `call`/`reply`/`change_set`/`stream_item`.
2. **One JSON shape, versioned** (`undra.recording`, `version: 1`): `{format, version, schema_hash:"0x..", source, platform?, events:[..]}`,
   each event `{t, kind, ...}` with `t` = whole milliseconds since the session start, payloads as lower-case hex, handles as
   `"0x.."` strings (u64 does not fit a JS number), u32 ids as numbers. Kinds: `call`, `reply`, `change_set` (entries
   `{handle, signal, op, value}`), `stream_item`, `port_call`, `port_reply`, `event`, `timer_fired`, `observe`, `release`, `cancel`.
   Port and method ids are authoritative; `name` ("Http.request") is informational for the standard ports. Canonical writer: fixed
   key order, one event per line, no wall-clock, no host paths, so equal input gives equal bytes and every platform's writer is
   golden-tested against `testkit/fixtures/*.json` byte for byte. Unknown `version` or `format` is a typed error.
3. **F1(a) in-memory real core: platform-native fakes, not Rust fakes inside the core.** A core behind the C ABI or wasm has no
   entry to install Rust fakes (adding one is an ABI change, R11), and its ports are host-implemented anyway. So each kit has the
   same fakes as `undra::ports::fakes` (FakeClock+Timer, SeededRng, FakeHttp, MemKv/MemSecureStore, MemFs, CaptureLog, scripted
   Connectivity/Lifecycle) registered as the adapters of the app's own real core, and `testkit/conformance/fakes.json`, generated
   by the Rust fakes (checked in CI), is replayed against all four implementations so they cannot drift. `PreviewCore.load(seed:)`
   takes the same seed JSON as `undra::testing::Seed` (`now_ms, rng_seed, kv, secure_store, fs, http, connectivity, lifecycle`)
   or native builders.
4. **F1(b) `RecordedCore`**: a replay transport under the unchanged `UndraCore`/mirror/generated stores. Change-sets are released by a
   manual playhead (`advance(ms)`, `playAll()`; forward only, reload for a fresh start) and, per observed signal, everything up to
   the playhead is delivered when it is observed; constructors and calls are answered by the next recorded reply for the same
   target (call ids rewritten); anything unrecorded is a typed failure naming the call. Streams replay their recorded items. It is for
   states that are costly to reach; interaction belongs to the real core.
5. **The manual clock in previews**: `preview.clock.nowMs`, `set(nowMs:)`, `advance(ms)`; `advance` fires due timers in deadline order
   with the clock reading each deadline (same as `Fakes::advance`), then waits until the core is quiet (stats `polls` stable, no
   pending port call; microtask flush on TS wasm-main). **Corrected during implementation:** `ctx.sleep` follows the preview clock on
   the web only. A native core's runtime runs its own sleeps on its timer thread in real time (`Host::timer_set` is false except for
   the wasm host; the C ABI never hands sleeps to the host, SPEC 5.8), so on Swift and Kotlin the manual clock moves what the core
   reads (`Clock`) and timers armed through the `Timer` port, and a native sleep is waited for. Documented in `docs/TESTING.md`.
6. **Runtime seams**: Swift `UndraTransport`/`UndraInbound` and the `connect` entry become `package` (same SwiftPM package, still
   hidden from apps); Kotlin gets a documented opt-in public seam (`@UndraEmbeddingApi`: `Transport`, `TransportEvents`,
   `PortOutcome`, `UndraCore.attach`); TypeScript already exports `Transport` and `UndraCore.attach`. Additive, SPEC section 17 pointer.
7. **Rust**: a new crate `undra-testkit`, re-exported as `undra::testing` (no feature: serde_json is already in every core's graph via
   `undra-meta`, unused code is stripped, checked against the wasm size gate). It holds the format, `Seed`, `Recorder` (a `Host`
   decorator plus a raw envelope sink), `Replayer` (typed `ReplayError`: mismatch, exhausted, unconsumed; installs into a
   `RecordingHost` through a new catch-all `script_port_default`), `Harness` (TestRuntime + fakes + seed) and the conformance
   generator. `undra-transport` gets only the `Tap` hook; the dev runner (generated) depends on `undra-testkit`.
8. **Storybook is not a dependency.** The playground ships Component Story Format modules (`export default {title}` plus named
   story exports, which Storybook loads unchanged) and a plain `stories.html` page that renders them with Vite alone.
9. **Replay semantics**: per-port queues in recorded order; a call is answered when the method (and, by default, the exact args)
   match; a mismatch answers `Unavailable` and is reported as a typed error (`mismatch`, `exhausted`) from `replayer.finish()`
   (`unconsumed` too). Time is metadata: replay is instant and deterministic.

## Out of scope / limits (also in docs/TESTING.md)

RecordedCore does not replay interactions it has no recording for, rewinds, or `Lazy` page calls with different offsets (matched by
offset and limit). Timer firing is not recorded in dev sessions (the runner's core owns timers); the `Clock` readings are.
Previews link the kit into Debug builds; release dead-strips it.
