# SDE - ports-v2: the WebSocket, Sse and Db ports (wt/ports-v2, 2026-10-01)

Pieces G2 and G3 of the v1.x design (`.10x/specs/2026-10-01-v1x-default-choice-design.md`, Track G; the founder
approved both bets in Amendment A). The decisions are ADR-047 (WebSocket + SSE) and ADR-048 (Db), written first
(R11); the cross-runtime detail every runtime implemented is `.10x/specs/2026-10-01-ports-v2-runtime-brief.md`. The
runtimes were built in parallel by four agents (Swift, Kotlin, TypeScript, React Native) against that brief and
integrated here.

## What landed

* **`undra-ports`**: three opt-in ports behind cargo features `websocket`, `sse`, `db` (forwarded by `undra`), off by
  default, so no existing core's schema, hash or wasm changes (the standard schema test strips the opt-in items and
  still hashes `0x35fae635f80025f2`; with them the schema is `0xdb07a090521a1971`, `tests/golden/schema-opt-in.json`).
  Twelve types, `From<PortError>` for the three errors, the Rust surface (`ws::WsConnection` / `WsMessages`,
  `sse::subscribe` / `SseEvents`, `db::Database` / `Transaction` / `DbRow` / `params!`), `Backoff`, `next()`, and
  the fakes `FakeWebSocket`, `FakeSse`, `FakeDb` (scripted, failure injection, pull/backpressure accounting) plus
  `MemDb` (a real in-memory SQLite, dev-only feature `db-fake`).
* **No wire change.** A port call cannot return a stream (SPEC 3.6), so inbound is a batched pull:
  `receive(conn, max)` / `next(stream, max)`; `max` is the credit (16, re-pull below 8, one pull in flight), the
  binding reads ahead at most `max`, and a pull answers a burst as one reply (`max`, 2 ms quiet, 8 ms linger: the
  Swift runner measured 6 pulls in S23.3 without it). Reconnection is the core's (`Backoff`, `WeakCtx::sleep`).
* **`undra-bindgen`**: the `stdlib` table pins the 3 ports and 12 types; a standard enum with data
  (`WsMessage`, `DbValue`) is classified as a data enum (it was assumed unit); the `stdlib` golden references the new
  types, and the three typecheck tests compile it against the runtimes.
* **Runtimes** (each: the 12 types, adapter interfaces in the native idiom, the bindings, default adapters):
  Swift `URLSessionWebSocketAdapter`, `URLSessionSseAdapter`, `SQLiteDbAdapter` (files under the Kv adapter's root,
  `Application Support/<bundle id>/Undra/db`); Kotlin the runtime's own RFC 6455 client (text frames, UTF-8 checked
  on every text frame, subprotocols, headers, a read gate, the refused status), SSE over `java.net.http` on the JVM
  and `HttpURLConnection` on Android, `JdbcDbAdapter` (driver on the app's class path) and `AndroidDbAdapter`;
  TypeScript `@undra/runtime/realtime` (`browserWebSocket`, `nodeWebSocket`, `fetchSse`, `SseParser`) and
  `@undra/runtime/db` (`dbPort`, `nodeSqliteDb`, `waSqliteDb` + the `db-worker` entry on OPFS), registered through
  `LoadOptions.ports`, never in the main entry; React Native `Db` natively (a C++ binding over the system SQLite on
  iOS, JNI to `android.database.sqlite` on Android, the native shells' files), WebSocket and SSE through the TS
  bindings. `PortImpl` gains an optional `detach` (Kotlin) / `dispose` (TS) the core calls on close or replacement.
* **Contract scenarios S23 (WebSocket), S24 (SSE), S25 (Db)** against the platforms' real adapters and the shared
  server `contract-tests/servers/realtime-server.mjs` (Node, no dependencies: WebSocket and SSE behaviours by path,
  `/stats`), which every runtime's failure-injection suite and the React Native device checks also use.
* **Playground**: `live.rs` (`ws_echo`, `Live`, `sse_follow`) and `notes.rs` (the `Notes` store, `db_cells`,
  `db_run`, `db_migrate`); a Notes tab or view on iOS, Android, the web (OPFS) and React Native, a Live view on the
  web; RN device checks RN17..RN21.
* **Docs**: SPEC 0, 8.1 (new), 10.5, 11, 13, 14, 17; `site/docs/realtime.html`, `site/docs/db.html`, the ports page
  points at them; the React Native guide; `undra-ports` README; `bench/RESULTS.md` "Opt-in ports".

## Dependencies (CLAUDE.md asks why, licence, where it builds)

* **`rusqlite` 0.40 + `libsqlite3-sys` 0.38 (bundled)**, MIT (SQLite itself public domain): `fakes::MemDb`, the real
  SQLite fake a core's tests and the `db/*` benches use. Optional, behind `db-fake`, which only `[dev-dependencies]`
  enable (the playground core, the bench crate); `tests/opt_in.rs` fails if `undra`'s default or `db` features reach
  it. Builds for the host, iOS and Android (C via `cc`); not for `wasm32-unknown-unknown`, which is why it may never
  be a core dependency.
* **`org.xerial:sqlite-jdbc` 3.53.4.0**, Apache-2.0: the JDBC driver of the JVM `Db` tests and the Kotlin S25
  column, on test class paths only (`UNDRA_SQLITE_JDBC`, `scripts/env.sh`, CI downloads it with its SHA-1); the
  runtime module takes no dependency (`java.sql` is the JDK's).
* **`wa-sqlite` 1.0.0**, MIT: the browser `Db` adapter, an optional peer of `@undra/runtime` (dev dependency for its
  tests) imported only by `@undra/runtime/db-worker`; 299,165 bytes gzipped with its worker, never in the hello world.
* Declined: vendoring the SQLite amalgamation (React Native Android uses the platform's SQLite through JNI).
* Approval: I asked the coordinator before any download. The coordinator relayed a yes; I fetched (a) and (b) on it,
  then stopped, because a relayed approval is not the user's own; the user then answered directly in the
  coordinator's chat ("Yes to all three") and the coordinator installed wa-sqlite itself.

## Verification (after merging `main` at 5f5c3fb: derived-lists S19, abi-table S26 and the per-core namespaces)

`main` then took devtools (70fda02); it merged without a conflict and touched no runtime, binding or schema, so
the Rust gates were run again on it (fmt, clippy, rustdoc, `cargo test --workspace --no-fail-fast`: 2,848
passed, 0 failed, 15 ignored, 156 suites), with `undra bindgen --check --docs`, the site build and the TypeScript
contract column (23/23); the table below is the run after the first merge.

| Check | Result |
|---|---|
| `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, wasm32 clippy of `undra-ffi`, `cargo doc -D warnings` | clean |
| `cargo test --workspace --no-fail-fast` | 2,776 passed, 0 failed, 15 ignored (152 suites) |
| Swift `swift test` | 593 tests, 0 failures |
| Kotlin `test-local.sh` (brew 2.4.20 and CI's 2.0.21, JDBC driver set, `UNDRA_REQUIRE_TOOLCHAINS=1`) | 717 cases, 0 failed, 2 skipped (the native-library smoke cases), both |
| TypeScript `npm run typecheck`, `vitest run` | clean; 1,259 tests in 40 files |
| React Native `npm test`, `test:contract`, `cpp/test/run.sh` (ASan/UBSan, JSI and SQLite required), `android/test/run.sh` | 84 passed; 18 passed + S17 skipped (as before); stores 15, Db 10, host 33 + 33, JSI and iOS SDK compiles ok; 6 |
| Contract columns | ts 23/23, kotlin 23/23 (both compilers), swift 23/23 (S01..S19, S23..S26) |
| `undra bindgen --check --docs` | playground and both two-core packages up to date, `0x88d07d5fc9a2d4b4` |
| `scripts/wasm-size.sh` | hello wasm 102,537 B gz (record 102,722; gate 107,858), hello JS 25,062 B gz (gate 26,000) |
| `cargo test -p undra-bench --test budgets --release` | ok; `ports/ws_roundtrip` 417 ns (budget 2.1 µs), `db/insert_1k` 0.98-1.29 ms (6.5 ms), `db/query_10k` 1.23-1.78 ms (9 ms) |
| Site `build-all`, `check-links --words` | clean (landing prose 342/350 words) |
| React Native on the iPhone 17 Pro simulator, after the merge (`scripts/rn-device-checks.sh ios`) | `UNDRA-RN CHECKS 21/21` in the app, `24/24` with the script's own (RN17..RN21: Db natively, WebSocket and SSE against the realtime server) |

Device evidence from the runtime pieces (before the merge): iOS playground XCUITest tour 6 tests incl. `testNotes`
on "iPhone 17"; Android Notes on the `undra` AVD against real SQLite (survives a killed process), android-adapters
instrumented 122 pass + 3 skipped (two realtime cases pass when given the host server's port); React Native
`UNDRA-RN CHECKS 24/24` on the iPhone 17 Pro simulator and `25/25` on the `undra` emulator.

## Deviations from the ADRs and the brief

* Every pull answers a burst as one reply (added to ADR-047 §3 and SPEC 8.1 during the work).
* SSE on the JVM uses `java.net.http`: `HttpURLConnection.disconnect()` does not abort a read blocked on a chunked
  body on JDK 17 (S24.3 could not pass); Android keeps `HttpURLConnection`, where it does.
* TypeScript registers the ports through `LoadOptions.ports`, not adapter keys (adapter keys would pull binding code
  into the main entry); S23 runs on `nodeWebSocket()` (Node's `http` upgrade with the runtime's framing) because
  Node's global `WebSocket` hides a refusal's status, refuses to send 1001 and reports bad UTF-8 as a drop.
* The SSE parsers start their last-event-id from the request's `Last-Event-ID` (S24.2 needs it).
* Kotlin error fields are `reason` (exceptions cannot have `message`), like `HttpError.Network(reason)`.
* Android `Db`: Android's own WAL pool is turned off (one connection per database, which `changes()` and
  `last_insert_rowid()` need) and the binding sets `journal_mode = WAL` itself; the parameter count and the
  one-statement rule come from a tokenizer (Android exposes neither); a single row over Android's cursor window
  (about 2 MB) fails.
* wa-sqlite 1.0.0 bugs worked around: `bind_text` stops at U+0000, an empty blob binds as NULL.
* The `web/db-adapter` size is recorded in `bench/RESULTS.md`, not `web-size.jsonl` (every line there must be a gate).

## Open items for the integrator

* `persistence-v2` (S20..S22, `StorageError`) had not landed: its merge crosses `contract-tests/check.sh`,
  `run-all.sh`, `scenarios.md` (union of ids) and SPEC 8.
* The Android playground `smoke.sh` full run (it toggles airplane mode on the shared emulator) was not run; its Notes
  step is in it.
* `UndraPlatform.java` (React Native, pre-existing) fails `javac -Xlint:all -Werror` on an unused-resource warning;
  `android/test/run.sh` does not compile it, so nothing fails.
* `typecheck_ts` needs `tsc` on PATH (`runtimes/ts/@undra/runtime/node_modules/.bin`); `scripts/env.sh` does not
  provide it.
* The iOS `smoke.sh` screenshot step cannot write into `examples/playground/.proof` from this machine (macOS Desktop
  folder protection for the simulator service); the Swift piece ran its steps by hand.
