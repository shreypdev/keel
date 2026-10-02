# The opt-in `WebSocket`, `Sse` (ADR-047) and `Db` (ADR-048) ports — adversarial review

**Date:** 2026-10-02 · **Reviewer:** adversarial reviewer (`docs/AGENT_WORKFLOW.md` section 3), with four platform
sub-reviewers (Swift; Kotlin JVM + Android; TypeScript; React Native) whose findings were each re-checked here and
committed per platform · **Piece:** `wt/ports-v2` at `9cece1b` · **Read:** `CLAUDE.md` (R1, R2, R3, R5, R6, R9, R11,
R12), ADR-047, ADR-048, ADR-034, ADR-044, ADR-049, ADR-052, `.10x/specs/2026-10-01-ports-v2-runtime-brief.md`,
`.10x/decisions/sde/ports-v2.md`, SPEC §8.1 and §17, and the diff: `undra-ports` (`ws.rs`, `sse.rs`, `db.rs`, the
fakes, `MemDb`), bindgen `stdlib`, the five runtimes' bindings and default adapters, `realtime-server.mjs`, the
playground's `live.rs` / `notes.rs` and its web, iOS, Android and React Native views. **Merges:** `main` `e3573f7`
(persistence: `StorageError`, dispatchers by use, S20–S22) at `5413a5c`; `main` `8cbfc0f` (testkit, docs-v1x: the
cookbook and Fieldbook) at `5ee9069`, which also turns the cookbook's `realtime` and Fieldbook's `presence` features on
(the ports land with this branch), runs them in CI, and repairs main's `ci.yml` wasm32 step (two nested `for` lines,
one `done`). **Fixes:** `e23301e`, `385f0f6`, `f3b110a`, `adab13f`, `0bd736a`, `a6ffa1e`,
`48f7eda`, `22fbbe2`, `c40953a`, `c9f4da9`, `26dad2a`, `97a5a89`, `173a731`; ADRs `eee8656`.

## Verdict

**Sound after the fixes; merge.** The design holds under attack on every platform: the pull really is the credit
(a stalled core holds the window and no more: URLSession, the Kotlin client and the SSE body streams stop reading
and the server stalls far below its flood; the browser, Node-global and React Native `WebSocket`s stop at exactly
4,096 messages and end `Closed(1008, "the core did not keep up")`), one pull is in flight per connection (a second
is the typed `Protocol`), a burst is one reply without starving a lone message (median 3 ms on Swift, Kotlin and
TypeScript), every end is typed, values only ever travel as bound parameters (`'; DROP TABLE t; --` is data on all
five), every `DbValue` round-trips (U+0000 in text, NUL blobs, empty blob vs NULL, `i64` extremes, 2 MB rows; over
Android's cursor window a typed `Sql`, never a crash), a statement outside a running transaction is `Busy` at the
deadline while the transaction keeps going, and a transaction left open at shutdown is rolled back (a fresh
connection takes the write lock at once and sees none of it) on every platform.

What did not hold, all fixed with a test that fails without the fix:

* **B1 (blocking, size)** — after the cross-merge the hello-world JS was **26,132 B gz, 132 over ADR-052's gate**:
  the opt-in ports had put `PortIds.WebSocket/Sse/Db`, codec residue and the dispose plumbing into the main entry.
  Now 25,996 (ids in `OptInPortIds` of the subpaths, pure codecs, `dispose` called on close only from the main
  entry, from `recovery.ts` at a restart).
* **H1 (resource leak, every platform's Rust surface)** — a task cancelled while `WebSocket.connect`, `Sse.open`,
  `Db.open` or **`Db.begin`** crossed left what the platform opened with no handle in the core; an orphaned
  transaction makes every later statement on its database `Busy` until shutdown.
* **H2 (web recovery)** — a crash-restarted wasm core (ADR-049) kept the trapped instance's connections and its
  open transaction: the restarted core's `begin` was `Busy` for ever.
* **H3 (every `Db` binding)** — two opens of one new database at once (two stores opening `"app"` at launch, two
  cores) both migrated; the second failed `Migration { 1, "table already exists" }`. The WAL switch was `Busy`
  in ~0.1 s instead of the busy timeout, and a version above `i32::MAX` bricked the database.
* **H4 (Android, `android-adapters` and React Native)** — a `COMMIT` SQLite refused (a deferred foreign key) left
  the database stuck in a transaction: every later `begin` failed until the database closed. Proven on the `undra`
  AVD before and after the fix.

## Findings

### Blocking

**B1 — the hello-world JS over ADR-052's gate (fixed, `c40953a`).** `scripts/wasm-size.sh` on the merged tree:
`web/hello-runtime-js` 26,132 B gz against `<= 26,000` (main's record 25,984). Diffing the unminified hello bundle
against main's: `PortIds` gained the three opt-in entries (72 B gz), `/* @__PURE__ */ codecs.option(codecs.u16)`
left `codecs.u16; codecs.string; codecs.string;` statements behind (12 B), and `PortImpl.dispose` plumbing
(`#disposePorts` with a `Set`, a handler and replacement disposal) the rest. Fix:
`runtimes/ts/@undra/runtime/src/adapters/opt-in-ids.ts:9` (`OptInPortIds`, exported by `@undra/runtime/realtime`
and `@undra/runtime/db`; every use updated: the runtime, its tests, the TS contract column, the web playground,
React Native), the three codecs built in pure IIFEs (`adapters/codecs.ts`), `core.ts:875` (the close loop, no
handler: `dispose` must not throw), `recovery.ts:689` (the restart's disposal, out of the hello bundle). After:
**hello wasm 116,575 B gz** (gate 120,000), **hello JS 25,996 B gz** (gate 26,000), recorded. `registerPort` no
longer disposes the port it replaces (documented in `port.ts` and SPEC §17).

### High

**H1 — `connect` / `open` / `begin` cancelled while crossing leaves what the platform opened (fixed, `e23301e`).**
`crates/undra-ports/src/{ws.rs,sse.rs,db.rs}`: the platform finishes an abandoned port call (SPEC 5.1: the host is
not told), so a cancelled `Database::transaction` during `begin` left `BEGIN IMMEDIATE` open on the platform — every
later statement `Busy` — and a cancelled `connect`/`open` left a live connection, stream or database
(`SseEvents::drop` returned on `State::Opening`). Fix: `crates/undra-ports/src/owned.rs` runs those four calls in a
task of their own that closes (1001), closes, or rolls back what the late answer names when the caller is gone
(`db.rs:379`, `db.rs:457`, `ws.rs:321`, `sse.rs:178`). Tests (`tests/opt_in.rs`):
`a_transaction_cancelled_while_begin_crosses_is_rolled_back_when_the_platform_answers`,
`a_connect_or_open_cancelled_while_it_crosses_closes_what_the_platform_opened` — both fail without the fix. The
platform-side twin (a connect/open finishing after the core detached) was fixed in Kotlin (`WebSocketPort.kt:127`,
`SsePort.kt:108`, `DbPort.kt:157`, `22fbbe2`) and TypeScript (the bindings' epoch, `0bd736a`); a cancelled Kotlin
connect now abandons the half-open client (`ClientWebSocketAdapter.kt:216`) and a cancelled Kotlin Db open closes
its file under `NonCancellable`.

**H2 — a crash restart kept what the trapped instance held (fixed, `0bd736a`, then `c40953a`).** The restart did
not dispose the ports: old WebSocket and SSE connections stayed open (`nodeWebSocket` paused for ever, the browser
adapter buffered to 1008) and an open transaction kept SQLite's write lock, so every `begin` of the restarted core
was `Busy` after 5 s (reproduced with `nodeSqliteDb`). The bindings' terminal `disposed` flag would also have
refused every later call. Fix: the restart disposes every registered port (`recovery.ts:689`), the worker its
ports module's (`worker.ts:205`), and the bindings count disposals (an epoch) so the same instance serves the new
core. Tests: `recovery.test.ts` "a restart closes the WebSocket connections…", "a restart rolls back the
transaction…", "wasm-worker: the ports of the worker's ports module release…" (each fails without the fix). The
web playground then had to re-create what a restart does not restore: the Live view kept a stale `Live` ("stale
handle") and the Notes view a store whose database was gone ("no database is open") — both remount after a restart
(`examples/playground/web/src/App.tsx:64-65`, `f3b110a`, `173a731`; checked in the browser pane: connect, crash,
connect and echo; add, crash, add, reload, both notes kept).

**H3 — concurrent opens migrate twice; the WAL switch does not wait; a version above `i32::MAX` bricks the
database (fixed on every binding).** The brief prescribed reading `user_version` before `BEGIN IMMEDIATE`. Swift
(`a6ffa1e`, `DbPort.swift:300`) found it (2 of 3 concurrent opens failed `Migration{1, "table notes already
exists"}`); reproduced on the React Native C++ binding over the host SQLite (4 bindings, one new file:
`Migration(SQL error: table a already exists)`), and the Kotlin and TypeScript bindings follow the same sequence.
Fix everywhere: the version is read again under the write lock and what is pending recomputed
(`DbPort.kt:162`, `db/binding.ts:282`, `UndraDb.cpp:952`); `PRAGMA journal_mode = WAL`, which SQLite answers `BUSY`
at once while another connection switches the same file, is retried until the busy timeout (`DbPort.swift:347`,
`DbPort.kt:209`, `binding.ts` `walOn`, `UndraDb.cpp`); `user_version` is a signed 32-bit integer, so a version above
2,147,483,647 was stored as 0 and every later open re-ran migration 1 — refused before the call in Rust
(`db.rs:284`, `48f7eda`) and again by every binding. Tests that fail without the fix: Swift
`testConcurrentOpensOfOneNewDatabaseAllSucceedAndMigrateOnce`, `testTheSwitchToWalWaitsForTheBusyTimeoutThenIsBusy`,
`testAVersionBeyondUserVersionsRangeIsRefusedAndTheLargestFittingOneIsKept`; Kotlin `PortsV2BindingTests` (three
cases and the stricter open sequence); TypeScript `db-binding.test.ts` (three); React Native `db_test`
`testConcurrentOpens`; Rust `db_checks_names_and_migrations_before_crossing`. `MemDb` holds one lock across `open`,
so it has no race.

**H4 — Android: a refused `COMMIT` sticks (fixed, `c9f4da9`, `97a5a89`).** `AndroidDbAdapter.kt:169` and the React
Native module's `UndraDatabase.java` ran `BEGIN`/`COMMIT`/`ROLLBACK` through `SQLiteDatabase`'s transaction stack.
When SQLite refuses a `COMMIT` (a deferred foreign key), Android has already popped the transaction while SQLite
keeps it open; the binding's `ROLLBACK` is then refused ("no transaction is active") and every later `begin` fails
"cannot start a transaction within a transaction". Found by the React Native sub-review from AOSP; the new
`DbOnDeviceTest.a_commit_sqlite_refuses_rolls_the_transaction_back_and_the_next_one_begins` failed on the `undra`
AVD with exactly that error and passes after the fix (the three statements go to SQLite itself with a leading `;`,
`UndraDatabase.java:193`); RN19 checks it on both devices.

### Medium (fixed)

* **Kotlin's `SqlText` miscounted parameters** (`SqlText.kt`, `22fbbe2`): `#name` and the `::` / `(...)` suffixes
  SQLite's tokenizer accepts were not variables, so on Android `UPDATE t SET x = #v WHERE id = ?` bound the value to
  `#v` and ran `?` as NULL. A 20,000-statement fuzz against real SQLite now agrees.
* **A raw `WsError`/`SseError`/`DbError` from its own port answered status 2 on Kotlin** (`PortRegistry.kt:234`):
  ADR-049's rule listed only `StorageError`, `FsError`, `HttpError`.
* **A 7th SSE stream to one host hung in `open`** for the 24 h request timeout on Swift (URLSession's 6 connections
  per host; `URLSessionSseAdapter.swift:36`, `a6ffa1e`).
* **TypeScript adapters** (`0bd736a`): `browserWebSocket` closed with 1008 on one message larger than its byte limit
  with nothing queued (`browser-websocket.ts:150`, the limit bounds a backlog); `fetchSse` made `fetch` throw on a
  `Last-Event-ID` outside Latin-1 (every resume `Network`; now UTF-8 bytes, `fetch-sse.ts:22`) and stripped two byte
  order marks; the bindings had no `PortImpl.name`, so failures named `port 0x7388b95f`.

### Minor / documentation (done)

* **Two cores, one database name: shared by design** (each core its own binding over the same file; SQLite's locks
  keep them apart) — `runtimes/ts/@undra/runtime/test/db-two-cores.test.ts` (committed rows seen, uncommitted not,
  a contended write `Busy` after 5 s, concurrent first opens migrate once). On Node `DatabaseSync` waits on the
  calling thread.
* **The web's `Db` is one tab's**: `AccessHandlePoolVFS` holds every file of the origin's pool, so a second tab's
  open is `DbError.Unavailable` (checked in the browser pane; typed, no data lost). SPEC §8.1, `site/docs/db.html`.
* **React Native**: a lone message reaches the core after one frame (~16.7 ms; RN timers are frame-aligned); every
  failure before a socket opens is `Refused { status: null }`; a drop is `Network`, or on iOS sometimes
  `Closed(1001, "Stream end encountered")` — `docs/REACT_NATIVE.md`, ADR-047 notes; RN20 accepts what each OS gives.
* `contract-tests/scenarios.md` S23 named Node's global `WebSocket` for the TS column, which runs `nodeWebSocket()`.
* SPEC §8.1 and §17 follow every fix above; ADR-047 and ADR-048 are Accepted with dated implementation notes.

## Each attack and its result

**1. Backpressure as credit (ADR-047 §3).**
* *Flood while the core does not pull.* Swift: `/ws/flood?n=100000&size=512`, the server's `written` stayed at 1,709
  (1 s and 1.5 s), the binding held 32 (16 delivered + 16 read ahead), then all 100,000 in order and
  `Closed{1000,"end"}`; SSE stalled at ~10k small events (about 5 MB in URLSession and the kernel), 84 of 2,000 for
  64 KiB events. Kotlin JVM: binding 16 + adapter 1; `written` froze at 83,514 of 100,000 for 16-byte frames (kernel
  buffers), 857 for 1 KiB; SSE 41 of 2,000 for 64 KiB; Android device test: floods of 500 × 64 KiB stall below n/2.
  TypeScript: `browserWebSocket` over Node's global peaked at exactly 4,096, then `Closed(1008)`; `nodeWebSocket`
  pauses but only after parsing one socket read (≤ 64 KiB; open item). React Native (device, RN20): a stalled flood
  of 6,000 kept 4,112 (4,096 + the 16 read ahead) and ended `Closed(1008, "the core did not keep up")` on iOS and
  Android. **Holds.**
* *One pull in flight.* A second concurrent `receive` is `Protocol("a receive is already pending …")` and the first
  is still answered, on all four. **Holds.**
* *Burst coalescing vs a lone message.* Swift median 3.0 ms (max 4.3 ms, 40 rounds), Kotlin p50 3.29 / p95 3.42 ms,
  TypeScript ~3 ms on both adapters; a trickle every 0.5 ms answers at the 8 ms cap. React Native ~16.7 ms (one
  frame; documented). **Holds** (RN by its platform).
* *`close` during a pending `receive`.* Answered `[]` exactly once, both race orders, 300 rounds on Swift, 30 on
  Kotlin; shutdown/detach during a pending receive answers `[]` and the server sees 1001 (a browser sends no code:
  script cannot send 1001). **Holds** after H1/H2.
* *SSE.* `Last-Event-ID` sent from the core's id, `retry:` surfaced as `retry_ms`, body end `Ended`, `/sse/hang` +
  close: the server sees the client leave, on Swift, the JVM (`java.net.http`), Android (`HttpURLConnection`),
  TypeScript (`fetch`) and React Native (`fetch`/XHR, RN21). **Holds** after the TS header fix.

**2. Db integrity and safety (ADR-048).**
* *Busy deadline.* With a shortened busy timeout: Swift `Busy` at 404–422 ms for 400 (5.004 s through the bridge with
  the default), Kotlin 405 ms for 400 (20 statements of the transaction ran meanwhile), TypeScript ≥ 190 ms and
  < 1.2 s for 200 on `node:sqlite` and wa-sqlite, React Native within the deadline (upper bound now asserted).
  **Holds.**
* *No dangling transaction after shutdown or restore.* Swift `core.shutdown()` with three uncommitted rows: a raw
  sqlite3 connection gets `BEGIN IMMEDIATE`, only the committed row remains, no file descriptor of the database is
  left. Kotlin JDBC and Android: a fresh adapter (busy timeout 100 ms) begins at once and sees only the committed
  row. TypeScript: after `dispose` a raw connection with `busy_timeout = 0` gets the write lock. React Native host:
  rolled back, a new `BEGIN IMMEDIATE` succeeds, a statement running during `stop` is answered before `stop`
  returns. Restore and crash restart: H1, H2. **Holds** after the fixes.
* *Migrations in one transaction.* A failing migration leaves `user_version`, the schema and the rows as they were;
  a newer database is refused, on all five. **Holds**, except H3 (fixed) and the `COMMIT`-inside-a-migration open
  item.
* *SQL injection.* The only values formatted into SQL anywhere are internal integers (`PRAGMA busy_timeout`,
  `PRAGMA user_version`); every value is bound (`sqlite3_bind_*`, `SQLiteStatement.bind*`, JDBC `setX`,
  `node:sqlite` parameters, wa-sqlite binds); `'; DROP TABLE t; --` as a text parameter is stored as text and the
  table survives on all five (RN18 on both devices). Android's tokenizer agrees with SQLite after the `SqlText` fix.
  **Holds.**
* *Every `DbValue`.* U+0000 inside text, NUL blobs, empty blob ≠ NULL ≠ empty text, `i64::MIN/MAX` (as `bigint` in
  JS), ±inf, 2 MB blobs and 3 M-character text round-trip on Swift, JDBC, Node, wa-sqlite and the RN host/iOS; JNI
  carries standard UTF-8 both ways (no modified UTF-8, no CESU-8). Android: a row over the cursor window is
  `DbError.Sql("Row too big to fit into CursorWindow …")`, never a crash (DbOnDeviceTest; RN18 on the AVD).
  **Holds.**
* *Two cores on one file.* Shared by design (above). **Holds**, documented.
* *wa-sqlite workarounds.* Removing each (`bind_text` cut at U+0000; an empty blob bound as NULL) makes its new test
  fail. A reload mid-transaction, simulated on wa-sqlite's in-memory VFS: the hot journal is rolled back, integrity
  ok, only committed rows. Real OPFS in the browser pane: notes survive reloads and a crash restart; a second tab is
  `Unavailable`. **Holds.** (In the first browser session, while the TypeScript runtime was being edited under the
  dev server, two notes added before a crash were missing after a later reload; the same sequences on the final code
  keep every note. Not reproduced; recorded as an open item.)

**3. Typed ends and errors.**
* *401 upgrade.* `Refused{401}` on Swift, Kotlin (JVM, Android) and `nodeWebSocket`; `Refused{null}` on the browser
  and React Native (both OSes agree), which hide the status (documented). *DNS failure.* `Network` on Swift, Kotlin,
  Node; `Refused{null}` on the browser and React Native (no status, no reason exposed before `open`). *Peer close
  1000.* `Closed{1000}` everywhere. *Drop.* `Network` everywhere, except iOS React Native, which can report
  `Closed(1001, "Stream end encountered")`. *Bad UTF-8.* `Protocol` on Swift, Kotlin, Node; the browser reports a
  drop. *Headers on a browser* — refused with the typed message, no socket made. *SSE* 401 → `Refused{401}`, DNS →
  `Network`, HTML → `Protocol` on all. **Holds** (platform differences documented in ADR-047's notes).
* *R3, Kotlin `reason` vs `message`.* The goldens and the playground read natively: `WsError.Closed(code, reason)`,
  `Refused(status, reason)` like `HttpError.Network(reason)`; `message` stays the Display text. (A Rust-style doc link
  `` [`disconnect`](Live::disconnect) `` survives in the generated Kotlin KDoc of `Objects.kt` — bindgen's, open.)

**4. Hashes and size (R1, ADR-052).** Standard schema with the features off: **`0xbbf6f70d0c567f47`**, main's
(`cargo test -p undra-ports --test schema` with default features; the hello core builds `undra-ports` with default
features only; the brief's `0x35fae635f80025f2` predates ADR-049). With the features on: the opt-in schema golden
**`0x716fc678df98087c`** (re-blessed for `StorageError`), the **playground `0xb5b7b1dc29182a9d`** (playground,
both two-core packages; cookbook `0x88127919dcf53b11`, Fieldbook `0x7bfb0229a00c20ed`, unchanged by the features'
flip). The three ports use `#[undra::port(dispatcher_by_use)]` (`WEB_SOCKET_DISPATCHER`, `SSE_DISPATCHER`,
`DB_DISPATCHER`, bound by `fakes::install` and `MemDb::install`). Size: B1. The Db worker bundle stays
informational (`bench/RESULTS.md`, 299,165 B gz).

## The matrix, once (after both merges, at `5ee9069`)

| Check | Result |
|---|---|
| `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, wasm32 clippy (`undra-ffi`; `undra-ports` with the three features), `cargo doc -D warnings` | clean |
| `cargo test --workspace --no-fail-fast` | **3,052 passed, 2 failed, 16 ignored** (169 suites); the 2 are `dev_reload` (below) |
| `cargo test -p undra-cli --test dev_reload`, alone | default parallelism: 4 passed / 5 failed, then 5 / 4; `--test-threads=3`: **9/9**; each failing test passes on its own. Load average 11–12 (the review's own builds and two emulators). Every failure is `expect_close` at `:145` ("a Close frame": the connection dropped without one), the signature `.10x/decisions/sde/testkit.md` finding 6 measured on a pristine main (3–5 of 8 failing at the same load). The branch does not touch the dev server; its CLI-test playground copy builds the three features (`crates/undra-cli/tests/common/mod.rs`) |
| `undra-ports`: features on with `db-fake` off / default features | 221 / 175 passed, 0 failed (standard hash `0xbbf6f70d0c567f47` with the features off) |
| `schema_docs -- --ignored`; `undra bindgen --check --docs` (playground, two-cores a and b, cookbook, Fieldbook) | ok; all up to date (`0xb5b7b1dc29182a9d`, `0x88127919dcf53b11`, `0x7bfb0229a00c20ed`) |
| `cookbook --features realtime`, `fieldbook-core --features presence` (flipped on by the second merge) | 44 / 22 passed |
| `cargo test -p undra-bench --test budgets --release` | ok; `ports/ws_roundtrip` 417 ns (budget 2.1 µs), `db/insert_1k` p50 0.99 ms (6.5 ms), `db/query_10k` p50 1.24 ms (9 ms); `sync_alloc` 5 and `commit_alloc` 2 pass in debug and release |
| `scripts/wasm-size.sh` | hello wasm 116,578 B gz (gate 120,000), hello JS **25,996** B gz (gate 26,000); recorded |
| Swift `swift test` | **668 tests, 0 failures** (640 runtime incl. the 21 review tests, plus main's testkit) |
| Kotlin `test-local.sh`, brew 2.4.20 and CI's 2.0.21 (JDBC driver set, `UNDRA_REQUIRE_TOOLCHAINS=1`) | **753 cases, 0 failed, 2 skipped** (the native-library smoke cases) + the kit's 30, both compilers |
| `android-adapters:test` (debug + release); `connectedDebugAndroidTest` on the `undra` AVD (realtime port given; run at `c9f4da9`, the second merge touched neither module) | 142 + 142, 0 failed, 1 skipped each; **142, 0 failed, 1 skipped** (network toggle); `DbOnDeviceTest` 14/14 |
| TypeScript `npm run typecheck`, `vitest run` | clean; **1,432 tests** in 44 files; `@undra/testkit` 32 |
| React Native `npm test`, `typecheck`, `cpp/test/run.sh` (ASan/UBSan, JSI and SQLite required), `android/test/run.sh` | 87; clean; stores 15, **Db 15**, host 33 + 33, JSI and iOS SDK compile; 7 |
| `scripts/rn-device-checks.sh` (run at `97a5a89`, after the React Native fixes; the second merge touched no React Native, TypeScript runtime or playground-core source) | iPhone 17 Pro simulator **`UNDRA-RN CHECKS 24/24`** (21/21 in the app); `undra-rn` AVD **`25/25`** (21/21): RN18 a 2 MiB row round-trips on iOS, is `DbError.Sql("Row too big to fit into CursorWindow …")` on Android; RN19 the refused commit; RN20 the flood (6,000 sent, 4,112 kept, `Closed(1008)`) |
| `bash contract-tests/run-all.sh` | **74/74** (ts 26, kotlin 24, swift 24: S01–S20, S23–S26, and S21–S22 on ts) |
| interop `run.sh ts`, `run.sh kotlin` | OK, OK |
| Android playground on the `undra` AVD (`smoke.sh` through its Notes step; the airplane-mode half not run on the shared emulator) | a note written to SQLite (`AndroidDbAdapter`) is there after a killed process; 0 crash markers |
| Web playground in the browser pane (OPFS, `browserWebSocket`) | Notes: add, reload, crash restart, add, reload — every note kept; a second tab `Unavailable`; Live: connect, echo, crash, connect, echo |
| Site `build-all`, `check-links --words` | clean |

## Open items (not fixed here)

* **A migration holding its own `COMMIT`/`BEGIN`/`ROLLBACK`** ends the binding's transaction early; a later failing
  migration then leaves a partial schema with `user_version` unchanged (every binding and `MemDb`). Candidate rule:
  refuse transaction control in migrations (an authorizer on `SQLITE_TRANSACTION`/`SAVEPOINT`, or the tokenizer).
* `nodeWebSocket` parses a whole socket read (≤ 64 KiB on the wire, up to ~13,000 tiny frames) before it pauses:
  bounded, TCP pushes back, but more than `max` (parse lazily, keep a received close frame). `fetchSse` likewise.
* React Native's lone-message latency is one frame (~16.7 ms): a scheduler the RN package could hand the binding.
* Bookkeeping grows: Swift `endedTransactions` and the bindings' `closed` sets, TypeScript `dbPort`'s closed
  databases. Derive "ended" from the id counter.
* An SSE `open` whose server accepts and never answers waits for the 24 h request timeout (Swift; shutdown cannot
  cancel it). JDBC/Android adapter-level open cancellation (during `getConnection`/`openDatabase`) still leaks the
  connection (no deterministic hook). Kotlin `awaitCancellable` can drop a just-completed SSE response without
  closing its body. `PulledStream` waits the 2 ms quiet period even when items are already buffered (~3 ms).
* SSE `id:` with an empty value (the standard's reset) is indistinguishable from no id, so a reconnect resends a
  stale `Last-Event-ID`; `sse.rs`'s doc example drops `retry_ms`.
* React Native: the platform's failure text before `open` is lost (RN puts it in the 1006 close's reason); iOS
  reports bad UTF-8 as `Closed(0, "")`, Android substitutes U+FFFD; a close code OkHttp rejects is only logged by
  RN's module (the socket may stay open). The web's one-tab `Db` (a shared worker or Web Locks hand-off).
* `dev_reload` (not touched by this branch; its playground copy now builds the three features): load-sensitive as on main (the matrix row above); passes 9/9 with three test threads.
* The first browser session's two missing notes (above), not reproduced.
* `.10x/decisions/sde/ports-v2.md` still states the pre-persistence hashes (`0x35fae635…`, `0xdb07a090…`,
  `0x88d07d5f…`); this review's numbers supersede them.
