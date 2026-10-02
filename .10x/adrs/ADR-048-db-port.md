# ADR-048: a `Db` standard port over SQLite: typed cells, bound parameters, adapter-run migrations, interactive transactions without blocking the core

Status: **Accepted** (2026-10-02, after the adversarial review of `wt/ports-v2`,
`.10x/reviews/2026-10-02-ports-review.md`; see "Implementation notes" at the end for the deviations). Proposed
2026-10-01 (piece G3 of the v1.x plan, `wt/ports-v2`; the founder approved the bet in
Amendment A of `.10x/specs/2026-10-01-v1x-default-choice-design.md`). Touches SPEC 0 (the "Rust-owned SQLite" line
stays out of scope: SQLite stays foreign), 8, 10.5, 11, 17; `undra-ports`, `undra` (feature), `undra-bindgen`
(`stdlib`), the four runtimes. **No wire, C ABI or wasm ABI change; no existing schema hash moves.** Sits beside
ADR-049's `StorageError` (same style: `Unavailable(String)`, `Full`, `Corrupt(String)`). R2, R5, R6, R11, R12.

## Context

Structured local data is matrix row 11: KMP has Room/SQLDelight, RN `expo-sqlite`, Flutter drift; Undra has `Kv`.
ADR-014 kept SQLite out of the core: `wasm32-unknown-unknown` has no libc, a Rust SQLite would add ~1 MB to every
core, and file I/O on the core thread would break §5.1's model. A port keeps the core small and deterministic and
uses the SQLite every platform already ships (iOS, Android) or a vetted build (JVM, web).

## Decision

1. **Opt-in** behind cargo feature `db` (`undra-ports`, forwarded by `undra`), off by default, for ADR-047's reason:
   no other core's schema, hash or wasm changes.
2. **The port** (async, every method typed):
   ```rust
   #[undra::port] pub trait Db {
       async fn open(&self, name: String, migrations: Vec<DbMigration>) -> Result<DbOpened, DbError>;
       async fn execute(&self, db: u32, sql: String, params: Vec<DbValue>) -> Result<DbExecuted, DbError>;
       async fn query(&self, db: u32, sql: String, params: Vec<DbValue>) -> Result<DbRows, DbError>;
       async fn begin(&self, db: u32) -> Result<u32, DbError>;     // a transaction id, usable as `db` above
       async fn commit(&self, tx: u32) -> Result<(), DbError>;
       async fn rollback(&self, tx: u32) -> Result<(), DbError>;
       async fn close(&self, db: u32) -> Result<(), DbError>;
   }
   DbMigration { version: u32, sql: String }          DbOpened { db: u32, version: u32 }
   DbValue { Null = 0, Integer(i64) = 1, Real(f64) = 2, Text(String) = 3, Blob(Bytes) = 4 }   // SQLite's storage classes
   DbExecuted { changes: u64, last_insert_id: i64 }    DbRows { columns: Vec<String>, rows: Vec<Vec<DbValue>> }
   DbError { Busy = 0, Constraint { kind: DbConstraint, message: String } = 1, Corrupt(String) = 2, Full = 3,
             Unavailable(String) = 4, Sql { message: String } = 5, Migration { version: u32, message: String } = 6 }
   DbConstraint { Unique = 0, NotNull = 1, ForeignKey = 2, Check = 3, Other = 4 }    // PRIMARY KEY is Unique
   ```
   Codes, never text, pick the variant: `SQLITE_BUSY`/`LOCKED` (after the adapter's 5 s busy timeout) → `Busy`;
   `SQLITE_CONSTRAINT_*` extended codes → `Constraint` (Android's `SQLiteConstraintException` carries no code: its
   documented `(code 2067 SQLITE_CONSTRAINT_UNIQUE)` suffix is the one place a message is parsed, tested per kind);
   `CORRUPT`/`NOTADB` → `Corrupt`; `FULL` (and quota) → `Full`; no adapter, a closed or unknown id, an invalid name,
   a file that cannot be opened → `Unavailable`; everything else → `Sql`. `From<PortError>`: `Unavailable` →
   `Unavailable("the Db port has no adapter registered (E0062: ..)")`, `Cancelled` → `Unavailable("cancelled")`,
   `Decode` → `Sql { "malformed port reply: .." }`, `Failed` → the decoded error.
3. **One statement per call, values only as parameters.** `execute`/`query` prepare exactly one statement and bind
   `params` positionally (`?`, `?NNN`); trailing SQL is `Sql`. The Rust surface takes the SQL as `&'static str`, so
   values cannot be formatted into it by accident; `execute_dynamic`/`query_dynamic(String, ..)` exist for SQL built
   at run time (an `IN (?, ?, ?)` list) and are named so they stand out in review. `params![a, b]` builds the
   `Vec<DbValue>` (`From` for integers, `f64`, `bool`, `&str`, `String`, `Vec<u8>`, `Bytes`, `Option<T>`);
   `DbRow::get::<T>(column)` reads a typed cell (a wrong class is `Sql`, never a panic, R6).
4. **Migrations are versioned SQL run by the adapter** at `open`, all pending ones in **one** transaction:
   versions must strictly increase from 1 (checked in Rust before the call and again by the adapter); the adapter
   reads `PRAGMA user_version`, runs every migration above it in order (a migration's `sql` may hold several
   statements), sets `user_version` to the last and commits; any failure rolls everything back and is
   `Migration { version, message }`. A database newer than the newest migration is refused (`Migration`), never
   downgraded. `DbOpened.version` is the version after migrating.
5. **Transactions without blocking the core.** Every open database has one connection and one serial worker owned
   by the adapter (a thread, queue or worker); the core only awaits port replies. `begin` waits for any running
   transaction, runs `BEGIN IMMEDIATE` and returns a transaction id; statements on that id run inside it;
   statements on the plain database id wait until it ends, at most the busy timeout, then fail `Busy` (a core that
   awaits the outer handle inside its own transaction gets a typed error, not a deadlock). Rust:
   `db.transaction(|tx| async move { tx.execute(..).await?; Ok(v) }).await` commits on `Ok`, rolls back on `Err`;
   a transaction dropped mid-way (its task cancelled) is rolled back through a `WeakCtx` (ADR-034).
6. **Open options are fixed:** `foreign_keys = ON`, `busy_timeout = 5000`, `journal_mode = WAL` where the VFS has it
   (not the web). `name` is `[A-Za-z0-9._-]{1,64}` not starting with `.`; `":memory:"` opens a private in-memory
   database (tests, caches). Files: iOS `Application Support/<bundle id>/Undra/db/<name>.sqlite` (the `Kv` adapter's root), Android
   `getDatabasePath("undra-<name>.sqlite")`, JVM `<root>/db/<name>.sqlite`, web OPFS `undra/db/<name>`; React
   Native uses the native shell's path on each OS so either shell reads the other's database. SQL time functions
   read the adapter's clock, not the core's: deterministic code binds `ctx.clock()` values (R12, documented).
7. **Platforms** (`DbAdapter.open(..) -> DbConnection`, natively: Swift `async throws(DbError)`, Kotlin `suspend`, TS
   `Promise`; the binding owns ids and the per-database serial worker):
   | | implementation | dependency, licence |
   |---|---|---|
   | iOS / macOS | the SQLite3 C API from Swift (`import SQLite3`), one `DispatchQueue` per database | system library |
   | Android | `android.database.sqlite.SQLiteDatabase` in `android-adapters`, one thread per database | platform |
   | JVM | `java.sql` (JDBC, in the JDK), driver supplied by the app: `org.xerial:sqlite-jdbc` (Apache-2.0, bundles SQLite) | none in `:runtime`; the driver at test/run time |
   | Node | `node:sqlite` (`DatabaseSync`, Node 22.5+) | built in |
   | web | **wa-sqlite** (MIT) sync build in a dedicated worker with OPFS `AccessHandlePoolVFS` (no COOP/COEP needed) | opt-in entry `@undra/runtime/db` + `db-worker` |
   | React Native | the binding in portable C++, the `UndraStores` pattern (one worker thread per database, JS never involved), over the system `libsqlite3` on iOS and JNI to `android.database.sqlite` on Android (the file `android-adapters` uses, so either shell reads the other's database) | none |
   **Size:** wa-sqlite's sync wasm is 558,343 bytes, 272,993 gzipped; with its worker script 299,165 bytes gzipped
   (`bench/RESULTS.md`, "Opt-in ports"; informational, not a gate); it is a separate opt-in bundle loaded by its own worker, never part of the
   core's wasm or the hello-world JS, so ADR-052's two gates are untouched. sql.js was rejected: in-memory only, a
   persisted database is the whole file re-exported per commit. The official `@sqlite.org/sqlite-wasm` is larger.
8. **The Rust fake.** `fakes::MemDb` is an in-memory SQLite (`rusqlite` with `bundled`, behind `undra-ports`'
   feature `db-fake`, which only `dev-dependencies` and tests enable): real SQL, deterministic, migrations and
   transactions exactly as an adapter runs them (it is the reference adapter the platform suites compare with).
   CLAUDE.md's rule is that a dependency must build for wasm32, iOS and Android: `rusqlite`/`libsqlite3-sys`
   build for iOS and Android (C via `cc`) and not for `wasm32-unknown-unknown`, which is why it is never a
   dependency of a core: no shipped crate enables `db-fake` (a test in `undra-ports` asserts the feature is not
   reachable from `undra`'s default or `db` features). `FakeDb` (scripted results and failure injection:
   `fail_next(DbError)`, records every statement) needs nothing and ships with `db`.
9. **`stdlib`**: the port and its seven types, pinned; goldens with the feature on.

## Alternatives considered

* **SQLite inside the core** (`rusqlite` in every core): ~1 MB per core, no wasm32-unknown-unknown build, I/O under
  the core lock; ADR-014's rejection stands.
* **Batch-only transactions** (`transaction(statements)`, one crossing): no read-modify-write; kept as a later
  addition, the interactive form is what apps write first.
* **A connection per transaction** (SQLite's own locking instead of the queue): `BUSY` storms under WAL writers and
  a file handle per transaction on the web, where OPFS access handles are exclusive.
* **`sqlite-jdbc` inside `:runtime`**: breaks "stdlib + kotlinx-coroutines only"; JDBC keeps the module clean.
* **The SQLite amalgamation in the React Native module** (and on Android generally): 1 MB per ABI for a library
  Android already has, a 9 MB vendored source, and a second SQLite whose file the native shell would not share.

## Consequences and proof (R4)

* Contract scenario **S25** (open with migrations, insert/query typed cells, constraint and SQL errors typed,
  transaction commit and rollback, migration failure rolls back, a reopened database keeps its rows and version)
  on the Swift, Kotlin and TypeScript columns with the real default adapter of each (TypeScript: `node:sqlite`).
* Each runtime's failure-injection suite: busy (an outer statement during a transaction), each constraint kind,
  corrupt file, unknown id after close, invalid name, migration downgrade, cancelled transaction rolled back.
* Bench rows `db/insert_1k` (1,000 inserts in one transaction) and `db/query_10k` (10,000 rows of 3 cells decoded)
  through the port path with `MemDb`, budgets in `bench/budgets.toml`.
* SPEC 8, 10.5, 11, 17 and `site/docs/db.html`. Hello-world wasm and JS: unchanged within tolerance.

## Implementation notes (2026-10-02, accepted after the review)

* **Android turns its own WAL pool off** (one connection per database, which `changes()` and `last_insert_rowid()`
  need) and the binding sets `journal_mode = WAL` itself. Android exposes neither the parameter count nor the end of
  a statement, so a tokenizer (`SqlText`) gives both; the review made it count every variable form SQLite's does
  (`#name`, `::` and `(...)` suffixes: `UPDATE .. SET x = #v WHERE id = ?` had bound the value to `#v`). A single row
  larger than Android's cursor window (about 2 MB) is a typed `Sql`/`Full` failure, never a crash (measured on the
  `undra` AVD). Since the review BEGIN, COMMIT and ROLLBACK go to SQLite itself (a leading `;`), not Android's
  transaction stack, which drops a transaction SQLite refused to commit (a deferred foreign key) and then refuses
  the binding's ROLLBACK: every later `begin` failed. The React Native module had the same code.
* **wa-sqlite 1.0.0 bugs are worked around** in `db/wa-sqlite-engine.ts`: its `bind_text` binds a NUL-terminated copy
  and `column_text` reads up to the first U+0000 (text is bound and read as bytes with their length), and its
  `bind_blob` binds a NULL pointer for an empty array, which SQLite stores as NULL (blobs are bound through
  `sqlite3_bind_blob` with their length); each workaround has a test that fails without it. The web has no WAL (`dbPort(waSqliteDb(), { wal: false })`).
* **Open, decided by the review on every binding** (Swift found it; Kotlin, TypeScript, React Native and the Rust
  check followed): `user_version` is read again under `BEGIN IMMEDIATE`'s write lock, so two opens of one new file at
  once (two stores opening `"app"` at launch, two cores) migrate it once instead of the second failing
  `Migration { 1, "table already exists" }`; the switch to WAL, which SQLite answers `BUSY` at once while another
  connection makes it, is retried until the busy timeout; a migration version must be at most 2,147,483,647
  (`user_version` is a signed 32-bit integer and would store a larger one as 0, re-running migration 1 on every
  open; `undra_ports::db::validate_migrations` refuses it before the call).
* **A transaction whose `begin` was still crossing when its task was cancelled** is rolled back when the platform's
  late answer names it (decision 5's "dropped mid-way" now covers the `begin` itself; the same for `open`).
* **Two cores (ADR-044) that open the same name share its file by design**, each through its own binding; SQLite's
  locks keep them apart (a contended write is `Busy` after the busy timeout; on Node `DatabaseSync` waits on the
  calling thread). **On the web one tab holds the database**: `AccessHandlePoolVFS` takes every file of the origin's
  pool exclusively, so a second tab's (or a second `waSqliteDb()`'s) `open` is `Unavailable` with the browser's
  reason. A shared worker or Web Locks hand-off is future work.
* **Not decided here (open):** a migration whose SQL holds its own `COMMIT` (or `BEGIN`/`ROLLBACK`) ends the
  binding's transaction early, so a later failing migration leaves a partial schema with `user_version` unchanged;
  refusing transaction control inside migrations (an authorizer on `SQLITE_TRANSACTION`/`SAVEPOINT`, or the
  tokenizer) is the candidate rule for every binding.

