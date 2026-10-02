# ports-v2: what each runtime implements (the brief behind ADR-047 and ADR-048)

The binding text is `.10x/adrs/ADR-047-websocket-and-sse-ports.md` and `ADR-048-db-port.md`. This brief is the
detail every runtime (Swift, Kotlin JVM + Android, TypeScript main + worker, React Native) implements the same
way. The Rust side is done and is the reference: `crates/undra-ports/src/{ws,sse,db}.rs` (port traits, records,
errors), `crates/undra-ports/src/fakes/{ws,sse,db}.rs` (the reference semantics of a binding), the playground core
`examples/playground/core/src/{live,notes}.rs`, the generated playground bindings `examples/playground/generated/`
(already regenerated: they import the new standard types from the runtimes, which do not have them yet).

## 1. Ids (FNV-1a, SPEC 1.1)

| Port | port id | methods |
|---|---|---|
| WebSocket | `0x7388b95f` | connect `0x83477638`, send `0x117b2158`, receive `0x8f31f08f`, close `0x60154b86` |
| Sse | `0x75d2ef19` | open `0xc0033c14`, next `0x4035cbed`, close `0x5bfe2c88` |
| Db | `0x559eda82` | open `0xee6f26db`, execute `0xffac2f0a`, query `0x3a4deefd`, begin `0xae2ba428`, commit `0xf866d5ae`, rollback `0x3e7b24b3`, close `0xde3dc7ed` |

Type ids: WsOpened `0x93640662`, WsMessage `0x9f2d9b9e`, WsError `0xc4e7cc8f`, SseEvent `0xa89828ce`, SseError
`0x2e7801f4`, DbMigration `0x36b31925`, DbOpened `0xaf76040e`, DbValue `0x48f74ac0`, DbExecuted `0x41a1a3a6`,
DbRows `0xffd12f2e`, DbConstraint `0x856f0900`, DbError `0x1dfc036b`. Kotlin `StandardPorts.kt` must carry the
port and method ids as `public object WebSocket { const val PORT_ID: UInt = 0x7388b95fu; const val CONNECT ...}`
(`crates/undra-ports/tests/ids.rs` parses them: object name = port name, constant = method name upper-cased).

## 2. The twelve standard types (every runtime exports all of them, whatever a core enables)

Wire layouts (little-endian, SPEC 3.1; enum = `u16` index + fields):

```
WsOpened     { conn: u32, protocol: String }
WsMessage    Text(String)=0 | Binary(Bytes)=1
WsError      Refused { status: Option<u16>, message: String }=0 | Network(String)=1 | Protocol(String)=2 | Closed { code: u16, reason: String }=3
SseEvent     { id: Option<String>, event: String, data: String, retry_ms: Option<u32> }
SseError     Refused { status: Option<u16>, message: String }=0 | Network(String)=1 | Protocol(String)=2 | Ended=3
DbMigration  { version: u32, sql: String }
DbOpened     { db: u32, version: u32 }
DbValue      Null=0 | Integer(i64)=1 | Real(f64)=2 | Text(String)=3 | Blob(Bytes)=4
DbExecuted   { changes: u64, last_insert_id: i64 }
DbRows       { columns: Vec<String>, rows: Vec<Vec<DbValue>> }
DbConstraint Unique=0 | NotNull=1 | ForeignKey=2 | Check=3 | Other=4
DbError      Busy=0 | Constraint { kind: DbConstraint, message: String }=1 | Corrupt(String)=2 | Full=3 | Unavailable(String)=4 | Sql { message: String }=5 | Migration { version: u32, message: String }=6
```

Shapes and spellings: exactly what `undra-bindgen` emits for these types with `emit_standard_library`
(reference output: `/private/tmp/claude-501/-Users-shrey-Desktop-src-keel/acdcf20d-e5e0-4e55-baef-372c013afd72/scratchpad/gen/{swift,kotlin,ts}`),
adapted the way each runtime's existing `StandardRecords` adapted the eight v1 types (public, documented, codecs
named as generated code expects: Swift `UndraRecord`/`UndraEnum`/`UndraError` conformances, `undraDecode`,
`Hashable`, `Codable` where it can be; Kotlin classes in `dev.undra.runtime.adapters` whose companion is the
`UndraCodec`; TS types + `<Name>Codec` exported from `@undra/runtime`'s main entry, errors as `UndraError`
subclasses with variant classes like `HttpError`). The generated golden `crates/undra-bindgen/tests/golden/stdlib`
references `WsMessage`, `SseEvent`, `DbValue`, `DbRows`, `DbError`, `SseError`, `WsError` from the runtimes, and
`cargo test -p undra-bindgen --test typecheck_{swift,kotlin,ts}` compiles it against the runtimes: it must pass.
Error messages (Display) are the Rust `#[error]` texts in `crates/undra-ports/src/{ws,sse,db}.rs`. Add the 12 types
to each runtime's wire-vector / codec tests (exact bytes are in the Rust unit tests of those files).

## 3. The adapter interfaces (native idiom, R3) and the binding

Each runtime exposes **an adapter interface** an app can implement (to replace the default) and **a binding**
that turns an adapter into the port's `PortImpl` and owns ids, the pull and the read-ahead. Names below are the
Swift ones; Kotlin and TS use the same names with their idioms (`suspend`, `Flow`, `Promise`, `AsyncIterable`).

```swift
public protocol WebSocketAdapter: Sendable {
    func connect(url: String, protocols: [String], headers: [Header]) async throws(WsError) -> any WebSocketConnection
}
public protocol WebSocketConnection: Sendable {
    var negotiatedProtocol: String { get }                 // "" for none
    var messages: AsyncThrowingStream<WsMessage, Error> { get }   // pull-based (`unfolding:`); ends by throwing a WsError, or finishing after the core's close
    func send(_ message: WsMessage) async throws(WsError)
    func close(code: UInt16, reason: String) async
}
public protocol SseAdapter: Sendable {
    func open(url: String, headers: [Header], lastEventId: String?) async throws(SseError) -> any SseStream
}
public protocol SseStream: Sendable { var events: AsyncThrowingStream<SseEvent, Error> { get }; func close() async }
public protocol DbAdapter: Sendable { func open(name: String) async throws(DbError) -> any DbConnection }
public protocol DbConnection: Sendable {   // every method runs on the adapter's own thread for that database
    func execute(_ sql: String, _ params: [DbValue]) async throws(DbError) -> DbExecuted   // exactly one statement
    func query(_ sql: String, _ params: [DbValue]) async throws(DbError) -> DbRows         // exactly one statement
    func executeScript(_ sql: String) async throws(DbError)                                 // several statements (migrations)
    func close() async
}
```
Kotlin: `interface WebSocketAdapter { suspend fun connect(url: String, protocols: List<String>, headers: List<Header>): WebSocketConnection }`,
`interface WebSocketConnection { val protocol: String; val messages: Flow<WsMessage>; suspend fun send(message: WsMessage); suspend fun close(code: Int, reason: String) }`,
failures thrown as the `WsError` exception classes. TS: `connect(...): Promise<WebSocketConnection>`, `messages(): AsyncIterable<WsMessage>`,
rejections are `WsError` instances. Db likewise (`suspend fun execute(sql: String, params: List<DbValue>): DbExecuted` ...).

### WebSocket binding (`webSocketPort(adapter)` / `WebSocketPortAdapter`)
* `connect`: call the adapter; register the connection under the next id (from 1, never reused); answer
  `WsOpened { conn, protocol }`. A typed adapter failure is the port's typed error (status 1); anything else
  thrown becomes `WsError.Network(description)`. A URL that is not `ws://` or `wss://` is
  `Refused { status: nil, message: "invalid URL: <url>" }` before the adapter is asked.
* A **pump** per connection pulls the adapter's `messages` into a buffer only while `buffer.count < window`,
  `window` = the `max` of the latest `receive` (16 before the first). Native adapters' streams are lazy, so a
  full buffer stops reading the socket (TCP pushes back). The pump records the terminal: the stream's thrown
  `WsError` (any other error → `Network`), or, if it finished without error and the core did not close,
  `Network("the connection ended")`.
* `receive(conn, max)`: unknown id → `Network("no WebSocket connection <id>")`; closed by the core → `[]`;
  `max` or more buffered → `max` at once; fewer → wait until `max` are there, 2 ms pass with no new
  one, or 8 ms after the first (a burst is one reply; the Swift and TypeScript bindings measured 6 pulls in S23.3
  without it); else the terminal error (sticky); else wait. At most one
  `receive` pending per connection (a second → `Protocol("a receive is already pending on connection <id>")`).
* `send(conn, m)`: unknown → `Network(..)`; after the core's close → `Closed { code, reason }` of that close; after
  the terminal → the terminal; else `await connection.send(m)`.
* `close(conn, code, reason)`: unknown → `Network(..)`; marks closed (sticky; a second close is `Ok`), answers a
  pending `receive` with `[]`, stops the pump, drops the buffer, `await connection.close(code, reason)`, `Ok`.
* Core shutdown / adapter detach: close every open connection with 1001, "".

### Default WebSocket adapters
* **Swift** `URLSessionWebSocketAdapter`: `URLSessionWebSocketTask` (protocols via `webSocketTask(with:protocols:)`
  on a `URLRequest` carrying the headers); `messages` = `AsyncThrowingStream(unfolding: { try await task.receive() })`
  (pull: no `receive()` while the binding's buffer is full); the first `receive`/`send` failure after the peer's
  close frame is `Closed { code: task.closeCode.rawValue, reason: String(decoding: task.closeReason) }`; a failed
  upgrade is `Refused { status: (task.response as? HTTPURLResponse)?.statusCode, message }`; other URLErrors
  `Network`; an invalid UTF-8 text frame `Protocol`. `send` completes on URLSession's completion handler.
* **Kotlin** (JVM and Android, the same class `ClientWebSocketAdapter` in `:runtime`): the runtime's own
  `WebSocketClient` (ADR-051) extended **without breaking `RemoteTransport`**: text frames delivered as text
  (UTF-8 validated → else close 1007 and `Protocol`), subprotocol offer + the server's choice, extra headers,
  and a read gate (the reader thread waits while the consumer's buffer is full, so TCP pushes back). A refused
  upgrade reports its HTTP status. `java.net.http.WebSocket` is not used (absent on Android at every API level).
* **TypeScript** `browserWebSocket()` / Node: the global `WebSocket` (Node 22+: `new WebSocket(url, { protocols, headers })`);
  it cannot pause, so the connection buffers up to **4,096 messages or 16 MiB** and then closes itself with
  **1008** and ends with `Closed { code: 1008, reason: "the core did not keep up" }`; `send` waits (polling per
  frame / 16 ms) while `bufferedAmount > 1 MiB`; headers on a browser (`typeof document !== "undefined"` and no
  Node `process.versions.node`) → `Refused { status: null, message: "this platform cannot send WebSocket headers: put the credential in the URL or a subprotocol" }`;
  close code 1006 without a close frame → `Network`; an `error` before `open` → `Refused { status: null }`.
* **React Native**: the TypeScript adapter over RN's `WebSocket` global, passing headers as RN's third
  constructor argument `{ headers }`.

### Sse binding and default adapters
Same pump/receive discipline (`next` ≙ `receive`, `close(stream)` ≙ `close`), errors `SseError`. `open` resolves
only after a 2xx answer whose content type is `text/event-stream` (else `Refused { status }` for non-2xx,
`Protocol("expected text/event-stream, got <type>")`); the request sends `Accept: text/event-stream`,
`Cache-Control: no-cache`, the headers, and `Last-Event-ID` when given. The body's end → `Ended`; a failure while
reading → `Network`. **One parser per runtime**, the HTML standard's algorithm: split lines on CRLF, LF or CR; a
line starting with `:` is a comment; `field: value` (one leading space after the colon dropped; a line without a
colon is a field with an empty value); `event` sets the type buffer, `data` appends value + LF to the data buffer,
`id` sets the last-event-id buffer (unless the value contains NUL), `retry` with only ASCII digits sets this
event's `retry_ms`; a blank line dispatches: if the data buffer is empty, reset data/type and dispatch nothing;
else strip one trailing LF from data, emit `{ id: lastEventIdBuffer or nil, event: type or "message", data,
retry_ms }`, reset data, type and retry (the id buffer persists). A UTF-8 BOM at the start is skipped. Invalid
UTF-8 → `Protocol`. Swift: `URLSession.bytes(for:)`; Kotlin: `java.net.http` on the JVM (`HttpURLConnection.disconnect()` does not abort a read blocked on a chunked body on JDK 17), `HttpURLConnection` on Android, each streaming on a reader thread
(no read while the buffer is full); TS: `fetch` + `body.getReader()` (no `read()` while the buffer is full);
RN: `fetch` streaming when `response.body` exists, else `XMLHttpRequest` progress events (incremental
`responseText`).

### Db binding (`dbPort(adapter)`) — the binding implements ADR-048's semantics, the adapter only runs SQL
* Ids: databases and transactions share one counter from 1, never reused.
* `open(name, migrations)`: `name` must be `":memory:"` or 1–64 of `A-Z a-z 0-9 . _ -` not starting with `.`,
  else `Unavailable("invalid database name ...")`; versions must strictly increase from 1, else
  `Migration { version, "migration versions must strictly increase, starting at 1" }`. Then `conn =
  adapter.open(name)`; run `PRAGMA foreign_keys = ON`, `PRAGMA busy_timeout = 5000` and, except on the web,
  `PRAGMA journal_mode = WAL` (ignore its result row); `current = PRAGMA user_version`; `newest` = the last
  migration's version; if migrations are non-empty and `current > newest`: close, `Migration { version:
  current, "the database is at version <current>, newer than the newest migration (<newest>)" }`. Pending =
  migrations above `current`; if any: `BEGIN IMMEDIATE`, `executeScript(sql)` each in order — a failure →
  `ROLLBACK`, close, `Migration { version, message: <the error's text> }` — then `PRAGMA user_version = <last>`,
  `COMMIT`. Answer `DbOpened { db, version }`.
* A per-database **serial queue** (one operation at a time on the connection, in arrival order) and a
  transaction slot. `begin(db)`: wait until no transaction is active (at most the busy timeout, 5 s; a test hook
  may shorten it), then `BEGIN IMMEDIATE`, answer the new tx id. `execute`/`query` on a **tx id** run at once (in
  the queue); on a **db id** while a transaction is active they wait for it to end (busy timeout, then `Busy`).
  `commit(tx)`: `COMMIT` (on failure: `ROLLBACK`, end the tx, the error); `rollback(tx)`: `ROLLBACK`; both end the
  tx and wake the waiters. Unknown/ended id → `Unavailable("no open database or transaction <id>")` (tx after
  commit: `Unavailable("transaction <id> is over")`). `close(db)`: roll back an active tx, `conn.close()`, mark
  closed; again → `Ok`; statements afterwards → `Unavailable`.
* Adapter contract (each runtime's SQLite code): `execute`/`query` prepare **one** statement — trailing
  non-whitespace, non-comment SQL → `Sql { "only one statement per call: use a migration for several" }`;
  `params.count != sqlite3_bind_parameter_count` → `Sql { "the statement has N parameters, M were given" }`; bind
  positionally (`Null`, `Integer` int64, `Real` double, `Text`, `Blob`); `query` returns the column names and each
  cell by its storage class (`sqlite3_column_type`); `execute` returns `changes` (`sqlite3_changes64` / count)
  and `last_insert_id` (`sqlite3_last_insert_rowid`). Errors map by (extended) result code, never by text
  (Android, which has no code, parses the `(code NNNN SQLITE_...)` suffix of its message, the one documented
  exception): `BUSY`(5)/`LOCKED`(6) → `Busy`; `CONSTRAINT`(19) by extended code: `UNIQUE` 2067 and `PRIMARYKEY`
  1555 → `Unique`, `NOTNULL` 1299 → `NotNull`, `FOREIGNKEY` 787 → `ForeignKey`, `CHECK` 275 → `Check`, others →
  `Other` (message = SQLite's); `CORRUPT`(11)/`NOTADB`(26) → `Corrupt(message)`; `FULL`(13) → `Full`;
  `CANTOPEN`(14)/`PERM`(3)/`READONLY`(8)/`IOERR`(10) → `Unavailable(message)`; everything else →
  `Sql { message }`.
* Files: iOS/macOS `Application Support/<bundle id>/Undra/db/<name>.sqlite` (the `Kv` adapter's root; directories created); Android
  `context.getDatabasePath("undra-<name>.sqlite")`; JVM `<dataDir>/db/<name>.sqlite`; Node `<dir>/<name>.sqlite`;
  web OPFS `undra/db/<name>`; `":memory:"` is in memory everywhere. Every default adapter takes a directory
  override (tests and S25 root it in a temporary directory).
* Default adapters: Swift `SQLiteDbAdapter` (`import SQLite3`, one serial `DispatchQueue` per connection);
  Android `AndroidDbAdapter` in `android-adapters` (`android.database.sqlite.SQLiteDatabase`, one thread per
  database; `rawQuery`/`SQLiteStatement` with typed binds, cell types via `Cursor.getType`); JVM `JdbcDbAdapter`
  in `:runtime` over `java.sql` (driver URL `jdbc:sqlite:<path>`, found through `DriverManager`; the driver,
  `org.xerial:sqlite-jdbc`, is the app's dependency, not `:runtime`'s; when no driver is on the class path every
  call is `Unavailable("no SQLite JDBC driver on the class path: add org.xerial:sqlite-jdbc")`); TS Node
  `nodeSqliteDb({ directory })` over `node:sqlite` `DatabaseSync` (`setReadBigInts(true)`: integers cross as
  `bigint`), browser `waSqliteDb()` (a dedicated worker, wa-sqlite + OPFS AccessHandlePoolVFS; pending the
  founder's download approval); RN: the binding in portable C++ (`cpp/UndraDb.{h,cpp}`, the `UndraStores` pattern: one worker
  thread per database, JS never involved) over a small backend interface: iOS (and the host test) the sqlite3 C
  API of the system `libsqlite3`; Android JNI to `android.database.sqlite` through the module's Java side (the same
  file `getDatabasePath("undra-<name>.sqlite")` as `android-adapters`, so either shell reads the other's database;
  no amalgamation, no download).

### TypeScript names (the TS and React Native pieces share them)
```ts
// "@undra/runtime" (main entry): the 12 types and their codecs (`WsMessageCodec`, ...), the error classes
// (`WsError`, `WsError.Refused`, ...; `SseError`, `DbError` likewise) and `PortIds.WebSocket/Sse/Db`. No binding code.
// "@undra/runtime/realtime":
export interface WebSocketAdapter { connect(url: string, protocols: readonly string[], headers: readonly Header[]): Promise<WebSocketConnection> }
export interface WebSocketConnection { readonly protocol: string; messages(): AsyncIterable<WsMessage>; send(message: WsMessage): Promise<void>; close(code: number, reason: string): Promise<void> }
export function webSocketPort(adapter: WebSocketAdapter): PortImpl;
export function browserWebSocket(options?: { WebSocket?: WebSocketConstructorLike; headers?: "refuse" | "pass"; maxBufferedMessages?: number; maxBufferedBytes?: number }): WebSocketAdapter;
export interface SseAdapter { open(url: string, headers: readonly Header[], lastEventId: string | null): Promise<SseStream> }
export interface SseStream { events(): AsyncIterable<SseEvent>; close(): Promise<void> }
export function ssePort(adapter: SseAdapter): PortImpl;
export function fetchSse(options?: { fetch?: typeof fetch }): SseAdapter;
export class SseParser { push(text: string): SseEvent[]; end(): void }   // the one parser (RN's XHR path reuses it)
// "@undra/runtime/db":
export interface DbAdapter { open(name: string): Promise<DbConnection> }
export interface DbConnection { execute(sql: string, params: readonly DbValue[]): Promise<DbExecuted>; query(sql: string, params: readonly DbValue[]): Promise<DbRows>; executeScript(sql: string): Promise<void>; close(): Promise<void> }
export function dbPort(adapter: DbAdapter, options?: { busyTimeoutMs?: number; wal?: boolean }): PortImpl;
export function nodeSqliteDb(options: { directory: string }): DbAdapter;
```
Registration stays one line before the first use and keeps the main entry free of these modules (the TS piece
picks the shape: a `LoadOptions.ports` map, or `core.registerPort(PortIds.WebSocket, webSocketPort(..))`).

## 4. Where they are registered
* Swift: `Adapters.platformDefault` gains `URLSessionWebSocketAdapter`, `URLSessionSseAdapter`, `SQLiteDbAdapter`
  (as `UndraAdapter`s for port ids above). Registering a port the core does not declare is harmless.
* Kotlin: `JvmAdapters.standard` gains WebSocket, Sse, Db (JDBC); `JvmAdapters.portable` stays as it is;
  `AndroidPlatformDefaults.install` registers WebSocket and Sse (the `:runtime` classes) and `AndroidDbAdapter`.
* TypeScript: **not** in the browser defaults (ADR-052's 26,000-byte JS gate): subpath exports
  `@undra/runtime/realtime` (`browserWebSocket`, `fetchSse`, `webSocketPort`, `ssePort`) and `@undra/runtime/db`
  (`dbPort`, `nodeSqliteDb`, `waSqliteDb`), and `LoadOptions.adapters` gains `webSocket`, `sse`, `db` keys
  (`AdapterOverrides`). Types and codecs are in the main entry. In `wasm-worker` mode these async ports cross to
  the main thread like every async port (ADR-049 §2).
* React Native: `reactNativeAdapters()` gains `webSocket` and `sse`; `Db` is a native default
  (`nativePlatformDefaults().ports` includes `0x559eda82`).

## 5. Tests every runtime adds
* Codec/wire-vector tests for the 12 types.
* Binding tests with a scripted in-memory adapter: ids, pull discipline (`window`, one pending receive),
  close/terminal semantics, unknown ids, shutdown closes with 1001.
* The **shared failure-injection suite** of the default adapters against
  `contract-tests/servers/realtime-server.mjs` (spawn `node <path> --port 0 --exit-on-stdin-close`, read
  `READY <port>`; TS imports `startRealtimeServer`): echo text and binary; subprotocol + headers (`/ws/headers`);
  refused upgrade with status (`/ws/deny?status=401`); peer close code and reason (`/ws/close`); abrupt drop
  (`/ws/drop` → `Network`); invalid UTF-8 (`/ws/bad-utf8` → `Protocol`); flood under a stalled reader
  (`/ws/flood?n=2000&size=65536`: where the platform can pause — Swift, Kotlin — the server's `written` count
  stays far below 2000 while the reader stalls, then everything arrives in order; where it cannot — TS/RN — the
  stream ends with `Closed(1008)` once 16 MiB are buffered); SSE parser cases (`/sse/feed`, resume with
  `Last-Event-ID`), `/sse/status?code=204|500` → `Refused(status)`, `/sse/html` → `Protocol`, body end → `Ended`,
  `/sse/hang` + close → the server sees the client leave.
* Db against the real adapter in a temp directory: each constraint kind, `Busy` (an outer statement during a
  transaction, with the busy timeout shortened by the test hook), a corrupt file (garbage written to
  `<name>.sqlite` → `Corrupt`), unknown id after close, invalid name, downgrade refused, a failed migration
  rolls everything back, typed cells incl. `i64::MIN/MAX`, NaN-free reals, empty and binary blobs, the
  one-statement rule, parameter-count mismatch, `":memory:"`.
* Contract scenarios **S23, S24, S25** exactly as `contract-tests/scenarios.md` writes them, printing
  `SCENARIO S23 PASS websocket`, ... (`contract-tests/check.sh` now requires them).

## 6. Rules
* Work only inside your directories (listed in your task); never edit `crates/`, `.10x/`, `docs/`, `site/`,
  `contract-tests/scenarios.md`, `check.sh`, `run-all.sh`, `servers/` (ask the integrator in your report).
* **Do not commit** (the integrator commits; parallel commits collide on the index lock). Leave the tree with your
  changes unstaged.
* No new dependencies (downloads need the founder's yes, pending): JDBC driver, wa-sqlite and the SQLite
  amalgamation are **not** available; build everything else, and make the pieces that need them compile and skip
  cleanly with a one-line reason.
* Docs on every public item; Swift 6 strict concurrency, no Objective-C; Kotlin stdlib + coroutines only in
  `:runtime`, compiled by both Kotlin 2.4 (brew) and 2.0.21; TS strict, ESM, no `any` in exported types.
