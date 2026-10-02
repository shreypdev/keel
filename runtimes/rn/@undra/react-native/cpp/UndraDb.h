// The `Db` port of @undra/react-native (ADR-048; the runtime brief of ports-v2): a native default, the
// `UndraStores` pattern of ADR-038 amendment B. JavaScript is never involved.
//
// Two halves:
//  * `DbPort`, the binding, in portable C++: it owns the ids (databases and transactions share one
//    counter from 1, never reused), one serial worker thread per database (every operation on a
//    connection runs on its database's thread, in arrival order), the transaction slot and the busy
//    timeout, migrations in one transaction under `PRAGMA user_version`, the open pragmas and the typed
//    errors. It never blocks the core: a port call is decoded, queued and answered later with
//    `undra_port_reply` (or answered at once, synchronously, when it names nothing that exists);
//  * a small `DbBackend` / `DbConnection` interface that only runs SQL: the sqlite3 C API of the system
//    `libsqlite3` on iOS and in the host test (`UndraDbSqlite.cpp`), JNI to `android.database.sqlite` on
//    Android (`UndraPlatformAndroid.cpp`). A backend maps SQLite's result codes to `DbFailure` with
//    `dbFailureOfCode`, never by text (Android, which reports no code, parses the `(code NNNN ...)`
//    suffix of its messages: `androidDbFailure`, the one documented exception).
//
// The SQL lexer below (`splitSql`, `sqlShapeOf`) is SQLite's own `sqlite3_complete` state machine and
// tokenizer rules, so the Android backend can keep the one-statement rule and count parameters (Android's
// API prepares only the first statement of a string and exposes no parameter count); the host test checks
// it against the sqlite3 C API on a corpus.
#pragma once

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <deque>
#include <functional>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <string_view>
#include <thread>
#include <variant>
#include <vector>

#include "UndraHost.h"
#include "UndraStores.h"

namespace undra::rn {

/// Ids of the `Db` port (docs/SPEC.md section 1.1; the brief, section 1).
namespace ports {
inline constexpr uint32_t kDb = fnv1a32("port.Db");
inline constexpr uint32_t kDbOpen = fnv1a32("Db.open");
inline constexpr uint32_t kDbExecute = fnv1a32("Db.execute");
inline constexpr uint32_t kDbQuery = fnv1a32("Db.query");
inline constexpr uint32_t kDbBegin = fnv1a32("Db.begin");
inline constexpr uint32_t kDbCommit = fnv1a32("Db.commit");
inline constexpr uint32_t kDbRollback = fnv1a32("Db.rollback");
inline constexpr uint32_t kDbClose = fnv1a32("Db.close");
} // namespace ports

// ----- the port's types (wire order of ADR-048) -------------------------------------------------

/// One SQLite value, by storage class, in the wire order of `DbValue`: `Null` 0, `Integer` 1, `Real` 2,
/// `Text` 3, `Blob` 4 (the variant's index is the wire index).
using DbValue = std::variant<std::monostate, int64_t, double, std::string, std::vector<uint8_t>>;

/// What `execute` answers.
struct DbExecuted {
  /// Rows changed by the statement (`sqlite3_changes64`).
  uint64_t changes = 0;
  /// The rowid of the last insert on the connection (`sqlite3_last_insert_rowid`).
  int64_t lastInsertId = 0;
};

/// What `query` answers: the column names and each row's cells.
struct DbRows {
  std::vector<std::string> columns;
  std::vector<std::vector<DbValue>> rows;
};

/// One schema migration.
struct DbMigration {
  uint32_t version = 0;
  std::string sql;
};

/// The variants of `DbError`, in wire order.
enum class DbErrorKind : uint16_t { Busy = 0, Constraint = 1, Corrupt = 2, Full = 3, Unavailable = 4, Sql = 5, Migration = 6 };

/// The variants of `DbConstraint`, in wire order.
enum class DbConstraintKind : uint16_t { Unique = 0, NotNull = 1, ForeignKey = 2, Check = 3, Other = 4 };

/// A `DbError`: `kind` and the fields its variant has (`constraint` for `Constraint`, `version` for
/// `Migration`, `message` for every variant but `Busy` and `Full`).
struct DbFailure {
  DbErrorKind kind = DbErrorKind::Sql;
  DbConstraintKind constraint = DbConstraintKind::Other;
  uint32_t version = 0;
  std::string message;

  static DbFailure busy() { return {DbErrorKind::Busy, DbConstraintKind::Other, 0, {}}; }
  static DbFailure full() { return {DbErrorKind::Full, DbConstraintKind::Other, 0, {}}; }
  static DbFailure sql(std::string message) { return {DbErrorKind::Sql, DbConstraintKind::Other, 0, std::move(message)}; }
  static DbFailure corrupt(std::string message) { return {DbErrorKind::Corrupt, DbConstraintKind::Other, 0, std::move(message)}; }
  static DbFailure unavailable(std::string message) {
    return {DbErrorKind::Unavailable, DbConstraintKind::Other, 0, std::move(message)};
  }
  static DbFailure constraintFailed(DbConstraintKind kind, std::string message) {
    return {DbErrorKind::Constraint, kind, 0, std::move(message)};
  }
  static DbFailure migration(uint32_t version, std::string message) {
    return {DbErrorKind::Migration, DbConstraintKind::Other, version, std::move(message)};
  }
};

/// The `DbFailure` of an (extended) SQLite result code (the brief's table): `BUSY`/`LOCKED` are `Busy`;
/// `CONSTRAINT` by its extended code (`UNIQUE` 2067 and `PRIMARYKEY` 1555 are `Unique`, `NOTNULL` 1299,
/// `FOREIGNKEY` 787, `CHECK` 275, the rest `Other`); `CORRUPT`/`NOTADB` are `Corrupt`; `FULL` is `Full`;
/// `CANTOPEN`/`PERM`/`READONLY`/`IOERR` are `Unavailable`; everything else is `Sql`. `message` is SQLite's.
DbFailure dbFailureOfCode(int code, std::string message);

/// The `DbFailure` of an exception Android's `android.database.sqlite` threw: its `(code NNNN ...)` suffix
/// (Android's SQLite reports extended codes there), else the exception's class for the few that carry
/// no code, else `Sql`. The message is SQLite's, without Android's suffix (`, while compiling: ...`).
DbFailure androidDbFailure(std::string_view className, std::string_view message);

/// The `Display` text of the Rust `DbError` (crates/undra-ports/src/db.rs), which a failed migration's
/// `Migration { message }` carries.
std::string dbErrorText(const DbFailure &failure);

/// Writes `failure` as a wire `DbError`.
void writeDbFailure(WireWriter &w, const DbFailure &failure);
/// Writes a wire `DbValue`.
void writeDbValue(WireWriter &w, const DbValue &value);
/// Reads a wire `DbValue`; sets the reader's failure on an unknown variant.
DbValue readDbValue(WireReader &r);
/// Writes a wire `DbRows`.
void writeDbRows(WireWriter &w, const DbRows &rows);

/// The text of `bytes` as UTF-8, every byte of an invalid sequence replaced by U+FFFD (a `TEXT` cell
/// SQLite holds is not always valid UTF-8; the wire's `String` must be).
std::string repairUtf8(std::string_view bytes);

/// The error texts the brief fixes.
namespace db_text {
inline constexpr char kOneStatement[] = "only one statement per call: use a migration for several";
inline constexpr char kNoStatement[] = "the SQL holds no statement";
/// "the statement has N parameters, M were given".
std::string parameterCount(int expected, std::size_t given);
} // namespace db_text

// ----- the SQL lexer (SQLite's own rules) -------------------------------------------------------

/// One statement of a string of SQL: `[begin, end)`, its terminating `;` included when it has one.
struct SqlStatement {
  std::size_t begin = 0;
  std::size_t end = 0;
};

/// The statements of `sql` as SQLite runs them one after the other (the `sqlite3_complete` state
/// machine: a `;` inside a `CREATE TRIGGER ... END` body does not end the statement; quotes, `[...]`
/// and comments are skipped). Empty statements (only whitespace, comments, `;`) are left out; text
/// after the last `;` that is not blank is the last statement.
std::vector<SqlStatement> splitSql(std::string_view sql);

/// Whether `text` holds only whitespace, comments and `;`.
bool sqlIsBlank(std::string_view text);

/// `sqlite3_bind_parameter_count` of the one statement `statement`: `?` takes the next index, `?NNN`
/// index NNN, `:name` / `@name` / `$name` one index per distinct name; the count is the largest index.
int sqlParameterCount(std::string_view statement);

/// What the one-statement rule and the parameter check need to know about the SQL of `execute`/`query`.
struct SqlShape {
  /// The first statement, leading empty statements skipped (`begin == end` when there is none).
  SqlStatement first;
  /// Whether anything but whitespace, comments and `;` follows it.
  bool trailing = false;
  /// Its parameter count.
  int parameters = 0;
};

/// The shape of `sql` (see `SqlShape`).
SqlShape sqlShapeOf(std::string_view sql);

// ----- the backend ------------------------------------------------------------------------------

/// One open connection, used only from its database's worker thread.
class DbConnection {
 public:
  virtual ~DbConnection() = default;
  /// Runs exactly one statement with `params` bound positionally (the one-statement rule and the
  /// parameter count are the backend's to check, before anything runs).
  virtual std::optional<DbFailure> execute(const std::string &sql, const std::vector<DbValue> &params, DbExecuted &out) = 0;
  /// Runs exactly one query and returns every row, each cell by its storage class.
  virtual std::optional<DbFailure> query(const std::string &sql, const std::vector<DbValue> &params, DbRows &out) = 0;
  /// Runs several statements without parameters (a migration).
  virtual std::optional<DbFailure> executeScript(const std::string &sql) = 0;
  /// Closes the connection. Called once, on the database's thread.
  virtual void close() noexcept = 0;
};

/// A platform's SQLite. Thread-safe: each database's thread opens its own connection.
class DbBackend {
 public:
  virtual ~DbBackend() = default;
  /// Opens (creating it if needed) database `name`, `":memory:"` or a name `DbPort` validated, on the
  /// calling thread (the database's worker). Null with `failure` set when it cannot.
  virtual std::unique_ptr<DbConnection> open(const std::string &name, DbFailure &failure) = 0;
  /// Where the files are (for `platformDefaults()`).
  virtual std::string describe() const = 0;
};

/// The sqlite3 C API backend over `<directory>/<name>.sqlite` (`UndraDbSqlite.cpp`: iOS, macOS and the
/// host test; Android has no public sqlite3 for C).
std::unique_ptr<DbBackend> makeSqliteDbBackend(std::string directory);

// ----- the binding ------------------------------------------------------------------------------

/// What `DbPort` is configured with.
struct DbOptions {
  /// How long a statement on a database waits for its running transaction (and `begin` for another
  /// one) before it fails `Busy`; also SQLite's own `PRAGMA busy_timeout`. 5 s; tests shorten it.
  std::chrono::milliseconds busyTimeout{5000};
  /// `PRAGMA journal_mode = WAL` at open (every native platform has it).
  bool wal = true;
};

/// Runs on every database thread as it starts (with its name) and as it ends: Android attaches the
/// thread to the Java VM and detaches it. Either may be empty.
struct DbThreadHooks {
  std::function<void(const char *name)> started;
  std::function<void()> ended;
};

/// The `Db` port's binding (see the file comment and the brief, "Db binding").
///
/// Every database has one connection and one serial worker thread; `open` runs on the new database's
/// thread (Android binds a transaction to the thread that began it). A statement on a transaction id
/// runs at once (in the queue); one on the database id while a transaction runs waits, parked, until the
/// transaction ends or the busy timeout passes (`Busy`); `begin` likewise waits for the slot. The waiting
/// never holds the worker, so the transaction's own statements and its `commit` still run.
class DbPort {
 public:
  /// Hands a complete `PortReply` payload to the core (`undra_port_reply`), from a database's thread.
  using Reply = std::function<void(const uint8_t *data, std::size_t len)>;

  DbPort(std::shared_ptr<DbBackend> backend, Reply reply, DbThreadHooks hooks = {}, DbOptions options = {});
  /// Stops (see `stop`).
  ~DbPort();
  DbPort(const DbPort &) = delete;
  DbPort &operator=(const DbPort &) = delete;

  /// A `Db` port call from the core (any thread, possibly under the core lock); never waits for I/O.
  /// Returns 1 when it was queued on a database's thread (answered later through `reply`), 0 when it
  /// was answered at once into `out` (a `malloc`ed `PortReply`: an argument the binding refuses, an id
  /// that names nothing open), 2 when the port is stopped or memory ran out.
  uint8_t post(uint32_t methodId, uint32_t portCallId, const uint8_t *args, uint32_t len, UndraBuf *out) noexcept;

  /// Ends every database: queued and parked calls are dropped (nobody waits for them once the core is
  /// shut down), a running one finishes, then each thread rolls back its open transaction, closes its
  /// connection and is joined. Idempotent; `post` answers 2 afterwards.
  void stop() noexcept;

  /// How many databases are open (for tests).
  std::size_t openCount() const;
  /// How many database threads have not been joined yet (for tests).
  std::size_t threadCount() const;

 private:
  struct Job;
  struct Database;

  void run(const std::shared_ptr<Database> &db);
  /// Runs `job` on `db`'s thread: answers it, or parks it (statements on the database id while a
  /// transaction runs, `begin` while one runs).
  void runJob(const std::shared_ptr<Database> &db, Job &job);
  std::vector<uint8_t> runOpen(const std::shared_ptr<Database> &db, Job &job);
  std::vector<uint8_t> runStatement(Database &db, Job &job);
  std::vector<uint8_t> runBegin(const std::shared_ptr<Database> &db, Job &job);
  std::vector<uint8_t> runEnd(Database &db, Job &job, bool commit);
  std::vector<uint8_t> runClose(Database &db, Job &job);
  /// The transaction of `db` ended: forget its id and run the parked calls next, in their order.
  void endTransaction(Database &db);
  /// Whether `job` must wait for `db`'s transaction (and parks it if so).
  bool parkIfBusy(Database &db, Job &job);
  void send(const std::vector<uint8_t> &reply) noexcept;
  /// Joins the threads of databases that have ended (under `mutex_`).
  void reapLocked();

  std::shared_ptr<DbBackend> backend_;
  Reply reply_;
  DbThreadHooks hooks_;
  DbOptions options_;

  mutable std::mutex mutex_;
  bool stopped_ = false;
  uint32_t nextId_ = 0;
  /// What each id handed out was: 0 never handed out, 1 a database, 2 a transaction.
  std::vector<uint8_t> kinds_;
  std::map<uint32_t, std::shared_ptr<Database>> dbs_;
  std::map<uint32_t, std::shared_ptr<Database>> txs_;
  /// Every database whose thread has not been joined.
  std::vector<std::shared_ptr<Database>> threads_;
};

} // namespace undra::rn
