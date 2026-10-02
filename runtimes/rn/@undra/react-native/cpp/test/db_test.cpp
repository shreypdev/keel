// The `Db` port of @undra/react-native (cpp/UndraDb.*, cpp/UndraDbSqlite.cpp) on this machine, against
// the system SQLite, without a core: the binding's semantics of the ports-v2 brief (ids, migrations in one
// transaction, the transaction slot and the busy timeout, typed errors by result code, the one-statement
// rule, the parameter count), the wire bytes of the Rust unit tests, and the SQL lexer the Android backend
// relies on, checked against the sqlite3 C API on a corpus. `run.sh` builds it with AddressSanitizer and
// UndefinedBehaviorSanitizer.
//
// Prints `ok - <name>` per check and exits non-zero on the first failure.

#include <sqlite3.h>
#include <sys/stat.h>
#include <unistd.h>

#include <cfloat>
#include <chrono>
#include <climits>
#include <cmath>
#include <condition_variable>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#include "../UndraDb.h"

using namespace undra::rn;

namespace {

int g_checks = 0;

[[noreturn]] void fail(const std::string &what) {
  std::fprintf(stderr, "not ok - %s\n", what.c_str());
  std::exit(1);
}

void check(bool condition, const std::string &what) {
  if (!condition) fail(what);
}

void ok(const char *name) {
  ++g_checks;
  std::printf("ok - %s\n", name);
}

std::string g_base;

// ----- the wire ---------------------------------------------------------------------------------

using Bytes = std::vector<uint8_t>;

Bytes openArgs(const std::string &name, const std::vector<DbMigration> &migrations = {}) {
  WireWriter w;
  w.str(name).u32(static_cast<uint32_t>(migrations.size()));
  for (const DbMigration &m : migrations) w.u32(m.version).str(m.sql);
  return w.out;
}

Bytes statementArgs(uint32_t target, const std::string &sql, const std::vector<DbValue> &params = {}) {
  WireWriter w;
  w.u32(target).str(sql).u32(static_cast<uint32_t>(params.size()));
  for (const DbValue &p : params) writeDbValue(w, p);
  return w.out;
}

Bytes idArgs(uint32_t id) {
  WireWriter w;
  w.u32(id);
  return w.out;
}

/// A reply: its status and body.
struct Reply {
  uint8_t status = 0xff;
  Bytes body;
};

DbFailure readFailure(const Bytes &body) {
  WireReader r(body.data(), body.size());
  DbFailure f;
  f.kind = static_cast<DbErrorKind>(r.u16());
  switch (f.kind) {
    case DbErrorKind::Constraint:
      f.constraint = static_cast<DbConstraintKind>(r.u16());
      f.message = r.str();
      break;
    case DbErrorKind::Migration:
      f.version = r.u32();
      f.message = r.str();
      break;
    case DbErrorKind::Corrupt:
    case DbErrorKind::Unavailable:
    case DbErrorKind::Sql:
      f.message = r.str();
      break;
    default:
      break;
  }
  check(r.finish(), "a DbError decodes exactly");
  return f;
}

DbRows readRows(const Bytes &body) {
  WireReader r(body.data(), body.size());
  DbRows rows;
  const uint32_t columns = r.u32();
  for (uint32_t i = 0; i < columns; ++i) rows.columns.push_back(r.str());
  const uint32_t count = r.u32();
  for (uint32_t i = 0; i < count && r.ok(); ++i) {
    const uint32_t cells = r.u32();
    std::vector<DbValue> row;
    for (uint32_t c = 0; c < cells && r.ok(); ++c) row.push_back(readDbValue(r));
    rows.rows.push_back(std::move(row));
  }
  check(r.finish(), "DbRows decode exactly");
  return rows;
}

const char *kindName(DbErrorKind kind) {
  static const char *names[] = {"Busy", "Constraint", "Corrupt", "Full", "Unavailable", "Sql", "Migration"};
  return names[static_cast<int>(kind)];
}

std::string describe(const Reply &reply) {
  if (reply.status == 0) return "ok";
  if (reply.status == 1) {
    const DbFailure f = readFailure(reply.body);
    return std::string(kindName(f.kind)) + "(" + f.message + ")";
  }
  return "status " + std::to_string(reply.status);
}

/// A `DbPort` over the system SQLite in a directory, with the replies collected by port call id.
struct Harness {
  std::mutex mutex;
  std::condition_variable cv;
  std::map<uint32_t, Bytes> replies;
  int syncAnswers = 0;
  std::unique_ptr<DbPort> port;
  uint32_t next = 1;

  explicit Harness(const std::string &dir, DbOptions options = {}) {
    port = std::make_unique<DbPort>(
        std::shared_ptr<DbBackend>(makeSqliteDbBackend(dir)),
        [this](const uint8_t *data, std::size_t len) {
          check(len >= 5, "a reply has its header");
          std::lock_guard<std::mutex> lock(mutex);
          replies[getU32(data)] = Bytes(data, data + len);
          cv.notify_all();
        },
        DbThreadHooks{},
        options);
  }

  /// Posts a call; returns its port call id (a synchronous answer is filed like an asynchronous one).
  uint32_t post(uint32_t method, const Bytes &args) {
    const uint32_t id = next++;
    UndraBuf out{nullptr, 0, 0};
    const uint8_t rc = port->post(method, id, args.data(), static_cast<uint32_t>(args.size()), &out);
    if (rc == 0) {
      check(out.ptr != nullptr && out.len >= 5 && out.cap == 0 && getU32(out.ptr) == id, "a synchronous answer is a malloc'ed PortReply");
      std::lock_guard<std::mutex> lock(mutex);
      replies[id] = Bytes(out.ptr, out.ptr + out.len);
      ++syncAnswers;
      std::free(out.ptr);
    } else if (rc == 2) {
      std::lock_guard<std::mutex> lock(mutex);
      replies[id] = Bytes{0, 0, 0, 0, 2};
    } else {
      check(rc == 1, "post answers 0, 1 or 2");
    }
    return id;
  }

  /// Whether call `id` has been answered.
  bool answered(uint32_t id) {
    std::lock_guard<std::mutex> lock(mutex);
    return replies.count(id) != 0;
  }

  Reply wait(uint32_t id, int seconds = 10) {
    std::unique_lock<std::mutex> lock(mutex);
    check(cv.wait_for(lock, std::chrono::seconds(seconds), [&] { return replies.count(id) != 0; }), "call " + std::to_string(id) + " is answered");
    const Bytes &raw = replies[id];
    return Reply{raw[4], Bytes(raw.begin() + 5, raw.end())};
  }

  Reply call(uint32_t method, const Bytes &args) {
    return wait(post(method, args));
  }

  // The port's methods, each expected to succeed.
  uint32_t open(const std::string &name, const std::vector<DbMigration> &migrations = {}, uint32_t *version = nullptr) {
    const Reply r = call(ports::kDbOpen, openArgs(name, migrations));
    check(r.status == 0, "open " + name + ": " + describe(r));
    check(r.body.size() == 8, "DbOpened is 8 bytes");
    if (version != nullptr) *version = getU32(r.body.data() + 4);
    return getU32(r.body.data());
  }
  DbExecuted execute(uint32_t target, const std::string &sql, const std::vector<DbValue> &params = {}) {
    const Reply r = call(ports::kDbExecute, statementArgs(target, sql, params));
    check(r.status == 0, "execute '" + sql + "': " + describe(r));
    check(r.body.size() == 16, "DbExecuted is 16 bytes");
    WireReader rd(r.body.data(), r.body.size());
    DbExecuted done;
    done.changes = rd.u64();
    done.lastInsertId = static_cast<int64_t>(rd.u64());
    return done;
  }
  DbRows query(uint32_t target, const std::string &sql, const std::vector<DbValue> &params = {}) {
    const Reply r = call(ports::kDbQuery, statementArgs(target, sql, params));
    check(r.status == 0, "query '" + sql + "': " + describe(r));
    return readRows(r.body);
  }
  /// A call expected to fail with a typed error.
  DbFailure failure(uint32_t method, const Bytes &args) {
    const Reply r = call(method, args);
    check(r.status == 1, "expected a typed error, got " + describe(r));
    return readFailure(r.body);
  }
  uint32_t begin(uint32_t db) {
    const Reply r = call(ports::kDbBegin, idArgs(db));
    check(r.status == 0 && r.body.size() == 4, "begin: " + describe(r));
    return getU32(r.body.data());
  }
  void ok(uint32_t method, uint32_t id) {
    const Reply r = call(method, idArgs(id));
    check(r.status == 0 && r.body.empty(), "() expected, got " + describe(r));
  }
};

std::string freshDir(const std::string &name) {
  const std::string path = g_base + "/" + name;
  std::string error;
  check(makeDirectories(path, error), "create " + path + ": " + error);
  return path;
}

std::string textOf(const DbValue &value) {
  check(value.index() == 3, "a Text cell");
  return std::get<std::string>(value);
}

// ----- the SQL lexer against sqlite3 ------------------------------------------------------------

/// What sqlite3 says about `sql`: the end offset of each statement it prepares (in order), and the
/// parameter count of the first; `ok` false when a statement does not prepare.
struct SqliteView {
  bool ok = true;
  std::vector<std::size_t> ends;
  int firstParameters = -1;
};

SqliteView sqliteView(sqlite3 *db, const std::string &sql) {
  SqliteView view;
  const char *start = sql.c_str();
  const char *at = start;
  const char *end = start + sql.size();
  while (at < end) {
    sqlite3_stmt *stmt = nullptr;
    const char *tail = nullptr;
    if (sqlite3_prepare_v2(db, at, static_cast<int>(end - at), &stmt, &tail) != SQLITE_OK) {
      view.ok = false;
      return view;
    }
    if (stmt != nullptr) {
      if (view.firstParameters < 0) view.firstParameters = sqlite3_bind_parameter_count(stmt);
      view.ends.push_back(static_cast<std::size_t>(tail - start));
      sqlite3_finalize(stmt);
    }
    if (tail == at || tail == nullptr) break;
    at = tail;
  }
  return view;
}

void testLexer() {
  sqlite3 *db = nullptr;
  check(sqlite3_open(":memory:", &db) == SQLITE_OK, "an in-memory sqlite3");
  char *error = nullptr;
  check(sqlite3_exec(db,
            "CREATE TABLE t (a, b, c); CREATE TABLE \"we;ird\" (x); CREATE TABLE log (m);"
            "CREATE TABLE [br;acket] (y); CREATE TABLE `back;tick` (z)",
            nullptr, nullptr, &error) == SQLITE_OK,
      "the lexer's tables");
  const std::vector<std::string> corpus = {
      "SELECT 1",
      "SELECT 1;",
      "  SELECT 1 ;  ",
      "SELECT 1; SELECT 2",
      "SELECT 1;;; SELECT 2;",
      ";;SELECT 1",
      "SELECT ';' AS semi; SELECT 2",
      "SELECT 'it''s; fine', \"a\" FROM t",
      "SELECT * FROM \"we;ird\"; SELECT * FROM [br;acket]; SELECT * FROM `back;tick`",
      "SELECT 1 -- a comment; with a semicolon\n; SELECT 2",
      "SELECT 1 /* a ; comment */ ; SELECT /* x */ 2",
      "SELECT 1; -- trailing comment",
      "SELECT 1; /* trailing block */",
      "CREATE TRIGGER tr AFTER INSERT ON t BEGIN INSERT INTO log VALUES ('a;b'); INSERT INTO log VALUES (2); END; SELECT 1",
      "CREATE TEMP TRIGGER tr2 AFTER INSERT ON t BEGIN SELECT 1; SELECT 2; END",
      "create temporary trigger tr3 after delete on t begin delete from log; end; select 3",
      "EXPLAIN SELECT 1; SELECT 2",
      "SELECT ?, ?, ?",
      "SELECT ?3, ?",
      "SELECT ?2, ?1, ?",
      "SELECT :a, :a, @a, $a, :b",
      "SELECT $x::y, $x::y, $f(a), :n1",
      "SELECT ? FROM t WHERE a = '?' AND b = \"?\" -- ?\n AND c = ? /* ? */",
      "SELECT a$b FROM (SELECT 1 AS a$b)",
      "SELECT x'0a3f', ?",
      "SELECT 1e5, 2.5, ?7",
      "INSERT INTO t VALUES (?, ?, ?); SELECT ?",
      "SELECT 'unicode é;', ?",
      "WITH r AS (SELECT ? AS v) SELECT v FROM r",
      "SELECT ?1, ?1, ?",
      "SELECT :one, ?, :one, ?2",
      "SELECT #a, #a, #b",
      "SELECT :a::b, :a::b",
      "SELECT $a(xy), $a(xy), @a(b)",
      "SELECT ?, ?5, ?",
      "SELECT ?32766",
      "SELECT 'a' || :x || \"b\"",
      "SELECT \"col\"\"x\" FROM (SELECT 1 AS \"col\"\"x\")",
      "SELECT ? -- x\n",
      "SELECT 1 /* unterminated",
      "SELECT 1 -- unterminated line comment",
  };
  for (const std::string &sql : corpus) {
    const SqliteView view = sqliteView(db, sql);
    check(view.ok, "the corpus prepares: " + sql);
    const std::vector<SqlStatement> mine = splitSql(sql);
    check(mine.size() == view.ends.size(), "statement count of '" + sql + "': lexer " + std::to_string(mine.size()) + ", sqlite " + std::to_string(view.ends.size()));
    for (std::size_t i = 0; i < mine.size(); ++i) {
      // sqlite3's tail stops right after a statement's `;` (or at the end, or before the trailing blank
      // that follows a last statement without one).
      const std::string_view rest(sql.data() + mine[i].end, sql.size() - mine[i].end);
      const std::string_view sqliteRest(sql.data() + view.ends[i], sql.size() - view.ends[i]);
      check(mine[i].end == view.ends[i] || (sqlIsBlank(rest) && sqlIsBlank(sqliteRest)),
          "statement " + std::to_string(i) + " of '" + sql + "' ends at " + std::to_string(mine[i].end) + ", sqlite at " + std::to_string(view.ends[i]));
    }
    const SqlShape shape = sqlShapeOf(sql);
    check(shape.trailing == (view.ends.size() > 1), "trailing SQL of '" + sql + "'");
    check(shape.parameters == view.firstParameters,
        "parameter count of '" + sql + "': lexer " + std::to_string(shape.parameters) + ", sqlite " + std::to_string(view.firstParameters));
  }
  check(splitSql("").empty() && splitSql(" ;; -- x\n /* y */").empty(), "blank SQL has no statement");
  check(sqlIsBlank(" ; -- c\n /* d */ ;") && !sqlIsBlank("; x"), "sqlIsBlank");
  check(sqlShapeOf("SELECT 1; SELECT 'unterminated").trailing, "an unterminated last statement is trailing SQL");
  sqlite3_close(db);
  std::printf("# %zu SQL strings compared with sqlite3\n", corpus.size());
  ok("the SQL lexer splits statements and counts parameters as sqlite3 does");
}

// ----- errors -----------------------------------------------------------------------------------

void testErrors() {
  struct Case {
    int code;
    DbErrorKind kind;
    DbConstraintKind constraint;
  };
  const Case cases[] = {
      {5, DbErrorKind::Busy, DbConstraintKind::Other},
      {261, DbErrorKind::Busy, DbConstraintKind::Other}, // BUSY_RECOVERY
      {6, DbErrorKind::Busy, DbConstraintKind::Other},
      {2067, DbErrorKind::Constraint, DbConstraintKind::Unique},
      {1555, DbErrorKind::Constraint, DbConstraintKind::Unique},
      {1299, DbErrorKind::Constraint, DbConstraintKind::NotNull},
      {787, DbErrorKind::Constraint, DbConstraintKind::ForeignKey},
      {275, DbErrorKind::Constraint, DbConstraintKind::Check},
      {1811, DbErrorKind::Constraint, DbConstraintKind::Other}, // TRIGGER
      {19, DbErrorKind::Constraint, DbConstraintKind::Other},
      {11, DbErrorKind::Corrupt, DbConstraintKind::Other},
      {26, DbErrorKind::Corrupt, DbConstraintKind::Other},
      {13, DbErrorKind::Full, DbConstraintKind::Other},
      {14, DbErrorKind::Unavailable, DbConstraintKind::Other},
      {3, DbErrorKind::Unavailable, DbConstraintKind::Other},
      {8, DbErrorKind::Unavailable, DbConstraintKind::Other},
      {778, DbErrorKind::Unavailable, DbConstraintKind::Other}, // IOERR_WRITE
      {1, DbErrorKind::Sql, DbConstraintKind::Other},
      {20, DbErrorKind::Sql, DbConstraintKind::Other},
  };
  for (const Case &c : cases) {
    const DbFailure f = dbFailureOfCode(c.code, "m");
    check(f.kind == c.kind && f.constraint == c.constraint, "result code " + std::to_string(c.code) + " maps to " + kindName(c.kind));
  }
  // Android: the `(code NNNN ...)` suffix, Android's own text cut off; a class for the few without one.
  const DbFailure unique = androidDbFailure("android.database.sqlite.SQLiteConstraintException",
      "UNIQUE constraint failed: notes.id (code 1555 SQLITE_CONSTRAINT_PRIMARYKEY)");
  check(unique.kind == DbErrorKind::Constraint && unique.constraint == DbConstraintKind::Unique && unique.message == "UNIQUE constraint failed: notes.id",
      "Android's PRIMARYKEY suffix is Unique, with SQLite's message: " + unique.message);
  const DbFailure fk = androidDbFailure("android.database.sqlite.SQLiteConstraintException", "FOREIGN KEY constraint failed (code 787 SQLITE_CONSTRAINT_FOREIGNKEY)");
  check(fk.constraint == DbConstraintKind::ForeignKey && fk.message == "FOREIGN KEY constraint failed", "Android's FOREIGNKEY suffix");
  const DbFailure old = androidDbFailure("android.database.sqlite.SQLiteConstraintException", "NOT NULL constraint failed: t.a (code 1299)");
  check(old.constraint == DbConstraintKind::NotNull && old.message == "NOT NULL constraint failed: t.a", "an older Android's `(code N)`");
  const DbFailure syntax = androidDbFailure("android.database.sqlite.SQLiteException",
      "near \"SELEC\": syntax error (code 1 SQLITE_ERROR): , while compiling: SELEC 1");
  check(syntax.kind == DbErrorKind::Sql && syntax.message == "near \"SELEC\": syntax error", "Android's compile suffix is cut: " + syntax.message);
  const DbFailure corrupt = androidDbFailure("android.database.sqlite.SQLiteDatabaseCorruptException", "file is not a database (code 26 SQLITE_NOTADB)");
  check(corrupt.kind == DbErrorKind::Corrupt && corrupt.message == "file is not a database", "Android's NOTADB is Corrupt");
  check(androidDbFailure("android.database.sqlite.SQLiteDatabaseLockedException", "database is locked").kind == DbErrorKind::Busy,
      "a class without a code: SQLiteDatabaseLockedException is Busy");
  check(androidDbFailure("android.database.sqlite.SQLiteCantOpenDatabaseException", "unknown error").kind == DbErrorKind::Unavailable,
      "SQLiteCantOpenDatabaseException is Unavailable");
  check(androidDbFailure("java.lang.IllegalStateException", "Cannot perform this operation").kind == DbErrorKind::Sql, "anything else is Sql");
  // Display, as the Rust `#[error]` texts.
  check(dbErrorText(DbFailure::busy()) == "the database is busy", "Busy text");
  check(dbErrorText(DbFailure::sql("no such table: x")) == "SQL error: no such table: x", "Sql text");
  check(dbErrorText(DbFailure::migration(4, "x")) == "migration 4 failed: x", "Migration text");
  check(dbErrorText(DbFailure::constraintFailed(DbConstraintKind::Unique, "u")) == "constraint failed: u", "Constraint text");
  check(dbErrorText(DbFailure::unavailable("closed")) == "the database is unavailable: closed", "Unavailable text");
  check(dbErrorText(DbFailure::corrupt("c")) == "the database is corrupt: c", "Corrupt text");
  check(dbErrorText(DbFailure::full()) == "the database is full", "Full text");
  ok("result codes map to DbError by code (and Android's suffix, the one parsed text); Display as in Rust");
}

void testWireBytes() {
  auto bytesOf = [](const DbValue &v) {
    WireWriter w;
    writeDbValue(w, v);
    return w.out;
  };
  check(bytesOf(std::monostate{}) == Bytes({0, 0}), "Null");
  check(bytesOf(int64_t{-2}) == Bytes({1, 0, 0xfe, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff}), "Integer(-2)");
  check(bytesOf(1.0) == Bytes({2, 0, 0, 0, 0, 0, 0, 0, 0xf0, 0x3f}), "Real(1.0)");
  check(bytesOf(std::string("a")) == Bytes({3, 0, 1, 0, 0, 0, 'a'}), "Text(\"a\")");
  check(bytesOf(Bytes{9}) == Bytes({4, 0, 1, 0, 0, 0, 9}), "Blob([9])");
  WireWriter e1;
  writeDbFailure(e1, DbFailure::constraintFailed(DbConstraintKind::NotNull, ""));
  check(e1.out == Bytes({1, 0, 1, 0, 0, 0, 0, 0}), "Constraint { NotNull, \"\" }");
  WireWriter e2;
  writeDbFailure(e2, DbFailure::migration(3, ""));
  check(e2.out == Bytes({6, 0, 3, 0, 0, 0, 0, 0, 0, 0}), "Migration { 3, \"\" }");
  WireWriter e3;
  writeDbFailure(e3, DbFailure::busy());
  check(e3.out == Bytes({0, 0}), "Busy");
  for (const DbValue &v : {DbValue(std::monostate{}), DbValue(INT64_MIN), DbValue(-0.5), DbValue(std::string("é")), DbValue(Bytes{0, 1})}) {
    const Bytes b = bytesOf(v);
    WireReader r(b.data(), b.size());
    check(readDbValue(r) == v && r.finish(), "a DbValue round-trips");
  }
  const Bytes unknown{9, 0};
  WireReader r(unknown.data(), unknown.size());
  readDbValue(r);
  check(!r.ok(), "an unknown DbValue variant fails the reader");
  check(repairUtf8("a\xff" "b\xe2\x82") == "a\xef\xbf\xbd" "b\xef\xbf\xbd\xef\xbf\xbd" && repairUtf8("é") == "é", "invalid UTF-8 is repaired with U+FFFD");
  ok("the wire bytes of DbValue and DbError are the Rust unit tests'");
}

// ----- the binding ------------------------------------------------------------------------------

void testOpenAndCells() {
  Harness h(freshDir("cells"));
  const uint32_t a = h.open(":memory:");
  const uint32_t b = h.open(":memory:");
  check(a == 1 && b == 2, "ids start at 1: " + std::to_string(a) + ", " + std::to_string(b));
  h.execute(a, "CREATE TABLE cells (i INTEGER, r REAL, t TEXT, b BLOB, n TEXT)");
  const DbFailure missing = h.failure(ports::kDbQuery, statementArgs(b, "SELECT * FROM cells"));
  check(missing.kind == DbErrorKind::Sql && missing.message.find("no such table") != std::string::npos, "two :memory: databases are separate");

  const std::vector<std::vector<DbValue>> rows = {
      {INT64_MIN, DBL_MAX, std::string(""), Bytes{}, std::monostate{}},
      {INT64_MAX, -2.25e-300, std::string("héllo \xf0\x9f\x8c\x8d\n\0x", 13), Bytes{0, 1, 0xff, 0}, std::monostate{}},
      {int64_t{0}, 1.5, std::string("text"), Bytes{42}, std::string("not null")},
  };
  for (const auto &row : rows) {
    const DbExecuted done = h.execute(a, "INSERT INTO cells VALUES (?, ?, ?, ?, ?)", row);
    check(done.changes == 1, "one row inserted");
  }
  const DbRows back = h.query(a, "SELECT i, r, t, b, n, typeof(i), typeof(r), typeof(t), typeof(b), typeof(n) FROM cells ORDER BY rowid");
  check(back.columns.size() == 10 && back.columns[0] == "i" && back.columns[5] == "typeof(i)", "the column names");
  check(back.rows.size() == 3, "three rows");
  for (std::size_t i = 0; i < rows.size(); ++i) {
    for (std::size_t c = 0; c < 5; ++c) {
      check(back.rows[i][c] == rows[i][c], "row " + std::to_string(i) + " cell " + std::to_string(c) + " comes back by its storage class");
    }
  }
  check(textOf(back.rows[0][5]) == "integer" && textOf(back.rows[0][6]) == "real" && textOf(back.rows[0][7]) == "text" &&
            textOf(back.rows[0][8]) == "blob" && textOf(back.rows[0][9]) == "null",
      "an empty text is text and an empty blob is a blob, not NULL");
  const DbRows invalid = h.query(a, "SELECT CAST(x'61ff62' AS TEXT)");
  check(textOf(invalid.rows[0][0]) == "a\xef\xbf\xbd" "b", "a TEXT cell that is not UTF-8 is repaired");
  const DbRows empty = h.query(a, "SELECT * FROM cells WHERE 0");
  check(empty.columns.size() == 5 && empty.rows.empty(), "no rows, the columns all the same");
  const DbRows nan = h.query(a, "SELECT ?", {std::nan("")});
  check(nan.rows[0][0].index() == 0, "SQLite stores NaN as NULL");
  ok("open :memory:, ids from 1; typed cells round-trip (i64 extremes, reals, empty and binary text and blobs, NULL)");
}

void testStatements() {
  Harness h(freshDir("statements"));
  const uint32_t db = h.open("statements");
  h.execute(db, "CREATE TABLE p (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, n INTEGER CHECK (n >= 0))");
  h.execute(db, "CREATE TABLE c (pid INTEGER REFERENCES p (id))");
  const DbExecuted one = h.execute(db, "INSERT INTO p (name, n) VALUES (?, ?)", {std::string("a"), int64_t{1}});
  check(one.changes == 1 && one.lastInsertId == 1, "execute answers changes and the last insert id");
  const DbExecuted two = h.execute(db, "INSERT INTO p (id, name, n) VALUES (?, ?, ?)", {int64_t{41}, std::string("b"), int64_t{2}});
  check(two.lastInsertId == 41, "last insert id 41");
  check(h.execute(db, "UPDATE p SET n = n + 1").changes == 2, "an update of two rows changes 2");
  check(h.query(db, "SELECT name FROM p WHERE id = :id AND name = :n OR id = :id", {int64_t{41}, std::string("b")}).rows.size() == 1,
      "named parameters bind by position, one index per name");

  struct Broken {
    std::string sql;
    std::vector<DbValue> params;
    DbConstraintKind kind;
  };
  const Broken broken[] = {
      {"INSERT INTO p (name, n) VALUES ('a', 0)", {}, DbConstraintKind::Unique},
      {"INSERT INTO p (id, name, n) VALUES (1, 'z', 0)", {}, DbConstraintKind::Unique},
      {"INSERT INTO p (name, n) VALUES (NULL, 0)", {}, DbConstraintKind::NotNull},
      {"INSERT INTO c VALUES (?)", {int64_t{999}}, DbConstraintKind::ForeignKey},
      {"INSERT INTO p (name, n) VALUES ('neg', -1)", {}, DbConstraintKind::Check},
  };
  for (const Broken &b : broken) {
    const DbFailure f = h.failure(ports::kDbExecute, statementArgs(db, b.sql, b.params));
    check(f.kind == DbErrorKind::Constraint && f.constraint == b.kind && f.message.find("constraint failed") != std::string::npos,
        "'" + b.sql + "' breaks its constraint: " + kindName(f.kind) + " " + std::to_string(static_cast<int>(f.constraint)) + " " + f.message);
  }
  h.execute(db, "CREATE TRIGGER no_x BEFORE INSERT ON p WHEN NEW.name = 'x' BEGIN SELECT RAISE(ABORT, 'no x; never'); END");
  const DbFailure raised = h.failure(ports::kDbExecute, statementArgs(db, "INSERT INTO p (name, n) VALUES ('x', 0)"));
  check(raised.kind == DbErrorKind::Constraint && raised.constraint == DbConstraintKind::Other && raised.message == "no x; never",
      "a trigger's RAISE is Constraint { Other } (and a trigger with `;` is one statement)");

  const DbFailure two_statements = h.failure(ports::kDbExecute, statementArgs(db, "UPDATE p SET n = 0; DELETE FROM p"));
  check(two_statements.kind == DbErrorKind::Sql && two_statements.message == db_text::kOneStatement, "two statements: " + two_statements.message);
  check(h.query(db, "SELECT COUNT(*) FROM p WHERE n = 0").rows[0][0] == DbValue(int64_t{0}), "and nothing of it ran");
  check(h.query(db, "SELECT 1; -- fine\n;").rows.size() == 1, "a trailing comment and `;` are not a statement");
  const DbFailure nothing = h.failure(ports::kDbQuery, statementArgs(db, " -- only a comment"));
  check(nothing.kind == DbErrorKind::Sql && nothing.message == db_text::kNoStatement, "no statement: " + nothing.message);
  const DbFailure few = h.failure(ports::kDbQuery, statementArgs(db, "SELECT ?, ?", {int64_t{1}}));
  check(few.kind == DbErrorKind::Sql && few.message == "the statement has 2 parameters, 1 were given", few.message);
  const DbFailure many = h.failure(ports::kDbExecute, statementArgs(db, "DELETE FROM p", {int64_t{1}}));
  check(many.message == "the statement has 0 parameters, 1 were given", many.message);
  check(h.query(db, "SELECT ?3", {int64_t{1}, int64_t{2}, int64_t{3}}).rows[0][0] == DbValue(int64_t{3}), "?NNN counts to its index");
  const DbFailure syntax = h.failure(ports::kDbQuery, statementArgs(db, "SELEC 1"));
  check(syntax.kind == DbErrorKind::Sql && syntax.message.find("syntax error") != std::string::npos, "a syntax error is Sql: " + syntax.message);
  ok("execute and query: each constraint kind, the one-statement rule, the parameter count, typed SQL errors");
}

void testMigrations() {
  const std::string dir = freshDir("migrations");
  Harness h(dir);
  const std::vector<DbMigration> v1v2 = {{1, "CREATE TABLE a (x INTEGER)"}, {2, "CREATE TABLE b (y INTEGER); INSERT INTO a VALUES (1)"}};
  uint32_t version = 99;
  const uint32_t first = h.open("app", v1v2, &version);
  check(version == 2, "a fresh database runs both migrations: version " + std::to_string(version));
  check(h.query(first, "PRAGMA user_version").rows[0][0] == DbValue(int64_t{2}), "user_version is 2");
  check(textOf(h.query(first, "PRAGMA journal_mode").rows[0][0]) == "wal", "a file database is in WAL mode");
  check(h.query(first, "PRAGMA foreign_keys").rows[0][0] == DbValue(int64_t{1}), "foreign keys are on");
  h.ok(ports::kDbClose, first);
  // Reopened: nothing runs again (the insert of migration 2 would add a second row).
  const uint32_t second = h.open("app", v1v2, &version);
  check(second != first && version == 2, "reopened at version 2 with a new id");
  check(h.query(second, "SELECT COUNT(*) FROM a").rows[0][0] == DbValue(int64_t{1}), "migrations ran once");
  const std::vector<DbMigration> v3 = {v1v2[0], v1v2[1], {3, "CREATE TABLE c (z INTEGER)"}};
  h.ok(ports::kDbClose, second);
  check(h.open("app", v3, &version) > second && version == 3, "a new migration runs alone");
  // A database newer than the newest migration is refused, never downgraded.
  const DbFailure newer = h.failure(ports::kDbOpen, openArgs("app", v1v2));
  check(newer.kind == DbErrorKind::Migration && newer.version == 3 &&
            newer.message == "the database is at version 3, newer than the newest migration (2)",
      "downgrade refused: " + newer.message);
  // A failing migration rolls back everything of that open, the migrations before it included.
  const std::vector<DbMigration> broken = {{1, "CREATE TABLE a (x INTEGER)"}, {2, "CREATE TABLE b (y INTEGER); INSERT INTO nowhere VALUES (1)"}};
  const DbFailure failed = h.failure(ports::kDbOpen, openArgs("fresh", broken));
  check(failed.kind == DbErrorKind::Migration && failed.version == 2 && failed.message == "SQL error: no such table: nowhere",
      "the broken migration: " + std::to_string(failed.version) + " " + failed.message);
  const uint32_t fresh = h.open("fresh", {}, &version);
  check(version == 0, "the failed open left version 0");
  check(h.query(fresh, "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('a', 'b')").rows[0][0] == DbValue(int64_t{0}),
      "and neither table of it");
  h.ok(ports::kDbClose, fresh);
  // Versions are checked before the database is touched; names likewise.
  const DbFailure order = h.failure(ports::kDbOpen, openArgs("x", {{2, "SELECT 1"}, {2, "SELECT 1"}}));
  check(order.kind == DbErrorKind::Migration && order.version == 2 && order.message == "migration versions must strictly increase, starting at 1", order.message);
  check(h.failure(ports::kDbOpen, openArgs("x", {{0, "SELECT 1"}})).version == 0, "a migration 0 is refused");
  for (const std::string &bad : {std::string(""), std::string(".hidden"), std::string("a/b"), std::string("a b"), std::string(65, 'x'), std::string("é")}) {
    const DbFailure f = h.failure(ports::kDbOpen, openArgs(bad));
    check(f.kind == DbErrorKind::Unavailable && f.message.rfind("invalid database name \"", 0) == 0, "invalid name '" + bad + "': " + f.message);
  }
  h.open(std::string(64, 'x'));
  check(h.syncAnswers >= 8, "argument errors are answered at once, on the calling thread");
  ok("open: pragmas, migrations in one transaction, reopen, downgrade refused, a failed migration rolls back, names checked");
}

void testTransactions() {
  DbOptions quick;
  quick.busyTimeout = std::chrono::milliseconds(300);
  Harness h(freshDir("tx"), quick);
  const uint32_t db = h.open("tx", {{1, "CREATE TABLE t (v INTEGER)"}});
  const uint32_t tx = h.begin(db);
  check(tx == db + 1, "a transaction id comes from the databases' counter");
  h.execute(tx, "INSERT INTO t VALUES (1)");
  check(h.query(tx, "SELECT COUNT(*) FROM t").rows[0][0] == DbValue(int64_t{1}), "the transaction sees its insert");
  // A statement on the database id waits for the transaction: here past the busy timeout, so Busy.
  const auto started = std::chrono::steady_clock::now();
  const uint32_t outer = h.post(ports::kDbQuery, statementArgs(db, "SELECT COUNT(*) FROM t"));
  // The transaction's own statements are not held up by the waiting one.
  h.execute(tx, "INSERT INTO t VALUES (2)");
  const Reply busy = h.wait(outer);
  const auto waited = std::chrono::steady_clock::now() - started;
  check(busy.status == 1 && readFailure(busy.body).kind == DbErrorKind::Busy, "an outer statement during a transaction is Busy: " + describe(busy));
  check(waited >= std::chrono::milliseconds(250), "after the busy timeout");
  // ... and not much later: the deadline wakes the worker, nothing else has to happen on the database.
  check(waited < std::chrono::milliseconds(300 + 700),
      "Busy within the busy timeout (300 ms) plus slack, after " +
          std::to_string(std::chrono::duration_cast<std::chrono::milliseconds>(waited).count()) + " ms");
  // One that the commit releases in time runs, after the transaction's statements.
  const uint32_t waiting = h.post(ports::kDbQuery, statementArgs(db, "SELECT COUNT(*) FROM t"));
  const uint32_t secondBegin = h.post(ports::kDbBegin, idArgs(db));
  std::this_thread::sleep_for(std::chrono::milliseconds(50));
  check(!h.answered(waiting) && !h.answered(secondBegin), "they wait for the slot");
  h.ok(ports::kDbCommit, tx);
  const Reply released = h.wait(waiting);
  check(released.status == 0 && readRows(released.body).rows[0][0] == DbValue(int64_t{2}), "released by the commit, it sees both rows");
  const Reply begun = h.wait(secondBegin);
  check(begun.status == 0, "begin waited for the slot: " + describe(begun));
  const uint32_t tx2 = getU32(begun.body.data());
  check(tx2 > tx, "ids are never reused");
  h.execute(tx2, "INSERT INTO t VALUES (3)");
  h.ok(ports::kDbRollback, tx2);
  check(h.query(db, "SELECT COUNT(*) FROM t").rows[0][0] == DbValue(int64_t{2}), "a rollback discards");
  // Ended transactions and wrong ids.
  const DbFailure over = h.failure(ports::kDbCommit, idArgs(tx));
  check(over.kind == DbErrorKind::Unavailable && over.message == "transaction " + std::to_string(tx) + " is over", over.message);
  check(h.failure(ports::kDbExecute, statementArgs(tx2, "SELECT 1")).message == "transaction " + std::to_string(tx2) + " is over", "a statement on an ended tx");
  check(h.failure(ports::kDbQuery, statementArgs(777, "SELECT 1")).message == "no open database or transaction 777", "an unknown id");
  const uint32_t tx3 = h.begin(db);
  const DbFailure nested = h.failure(ports::kDbBegin, idArgs(tx3));
  check(nested.kind == DbErrorKind::Sql, "begin on a transaction id is Sql: " + nested.message);
  // A failing statement inside a transaction leaves it running.
  check(h.failure(ports::kDbExecute, statementArgs(tx3, "INSERT INTO nowhere VALUES (1)")).kind == DbErrorKind::Sql, "a failure inside");
  h.execute(tx3, "INSERT INTO t VALUES (4)");
  h.ok(ports::kDbCommit, tx3);
  check(h.query(db, "SELECT COUNT(*) FROM t").rows[0][0] == DbValue(int64_t{3}), "the transaction committed after its failed statement");
  ok("transactions: ids from the shared counter, statements inside, outer statements wait then Busy, begin waits, commit, rollback, ended ids");
}

void testCloseAndBusyFile() {
  DbOptions quick;
  quick.busyTimeout = std::chrono::milliseconds(200);
  const std::string dir = freshDir("close");
  Harness h(dir, quick);
  const uint32_t db = h.open("shared", {{1, "CREATE TABLE t (v INTEGER)"}});
  const uint32_t tx = h.begin(db);
  h.execute(tx, "INSERT INTO t VALUES (1)");
  const uint32_t parked = h.post(ports::kDbExecute, statementArgs(db, "INSERT INTO t VALUES (2)"));
  // Close rolls the transaction back; the waiting statement finds the database closed.
  h.ok(ports::kDbClose, db);
  const Reply after = h.wait(parked);
  check(after.status == 1 && readFailure(after.body).message == "no open database or transaction " + std::to_string(db),
      "a statement waiting at close: " + describe(after));
  h.ok(ports::kDbClose, db);
  check(h.failure(ports::kDbExecute, statementArgs(db, "SELECT 1")).message == "no open database or transaction " + std::to_string(db),
      "a statement after close");
  check(h.failure(ports::kDbCommit, idArgs(tx)).message == "transaction " + std::to_string(tx) + " is over", "its transaction is over");
  const uint32_t again = h.open("shared");
  check(h.query(again, "SELECT COUNT(*) FROM t").rows[0][0] == DbValue(int64_t{0}), "close rolled the transaction back");
  // Two connections to one file: SQLite's own lock, past its busy timeout, is Busy too.
  const uint32_t other = h.open("shared");
  const uint32_t holding = h.begin(again);
  const DbFailure locked = h.failure(ports::kDbBegin, idArgs(other));
  check(locked.kind == DbErrorKind::Busy, "a second connection's BEGIN IMMEDIATE is Busy");
  h.ok(ports::kDbRollback, holding);
  h.ok(ports::kDbClose, again);
  h.ok(ports::kDbClose, other);
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
  while (h.port->threadCount() != 0 && std::chrono::steady_clock::now() < deadline) std::this_thread::sleep_for(std::chrono::milliseconds(10));
  check(h.port->threadCount() == 0 && h.port->openCount() == 0, "a closed database's thread ends");
  ok("close: rolls back, answers waiting calls, twice is Ok, ids after close; two connections to one file are Busy");
}

void testFailures() {
  const std::string dir = freshDir("failures");
  Harness h(dir);
  {
    std::ofstream garbage(dir + "/broken.sqlite", std::ios::binary);
    garbage << std::string(4096, 'G');
  }
  const DbFailure corrupt = h.failure(ports::kDbOpen, openArgs("broken"));
  check(corrupt.kind == DbErrorKind::Corrupt, std::string("garbage is Corrupt: ") + kindName(corrupt.kind) + " " + corrupt.message);
  check(::mkdir((dir + "/folder.sqlite").c_str(), 0700) == 0, "a directory where the file would be");
  const DbFailure cannot = h.failure(ports::kDbOpen, openArgs("folder"));
  check(cannot.kind == DbErrorKind::Unavailable, std::string("a file that cannot be opened is Unavailable: ") + kindName(cannot.kind) + " " + cannot.message);
  const uint32_t db = h.open("small");
  h.execute(db, "CREATE TABLE big (b BLOB)");
  h.query(db, "PRAGMA max_page_count = 8");
  const DbFailure full = h.failure(ports::kDbExecute, statementArgs(db, "INSERT INTO big VALUES (?)", {Bytes(200000, 7)}));
  check(full.kind == DbErrorKind::Full, std::string("past max_page_count is Full: ") + kindName(full.kind));
  Harness nowhere(dir + "/broken.sqlite/sub"); // its directory is under a file
  const DbFailure noDir = nowhere.failure(ports::kDbOpen, openArgs("x"));
  check(noDir.kind == DbErrorKind::Unavailable, "a directory that cannot be made is Unavailable: " + noDir.message);
  ok("a corrupt file is Corrupt, an unopenable one Unavailable, a full one Full");
}

void testStop() {
  DbOptions slow;
  slow.busyTimeout = std::chrono::seconds(30);
  Harness h(freshDir("stop"), slow);
  const uint32_t db = h.open("stop", {{1, "CREATE TABLE t (v INTEGER)"}});
  const uint32_t tx = h.begin(db);
  h.execute(tx, "INSERT INTO t VALUES (1)");
  const uint32_t parked = h.post(ports::kDbQuery, statementArgs(db, "SELECT 1"));
  h.open(":memory:");
  check(h.port->threadCount() == 2, "two database threads");
  const auto started = std::chrono::steady_clock::now();
  h.port->stop();
  check(std::chrono::steady_clock::now() - started < std::chrono::seconds(5), "stop does not wait for the busy timeout");
  check(h.port->threadCount() == 0 && h.port->openCount() == 0, "every thread joined");
  check(!h.answered(parked), "a parked call is dropped (the core is gone)");
  UndraBuf out{nullptr, 0, 0};
  const Bytes args = openArgs(":memory:");
  check(h.port->post(ports::kDbOpen, 999, args.data(), static_cast<uint32_t>(args.size()), &out) == 2, "post after stop is unavailable");
  h.port->stop();
  // The transaction was rolled back by its thread before it closed.
  Harness again(g_base + "/stop");
  const uint32_t reopened = again.open("stop");
  check(again.query(reopened, "SELECT COUNT(*) FROM t").rows[0][0] == DbValue(int64_t{0}), "stop rolled back the open transaction");
  // No transaction (and no lock) dangles in the file: a new one starts at once and commits.
  const uint32_t fresh = again.begin(reopened);
  again.execute(fresh, "INSERT INTO t VALUES (7)");
  again.ok(ports::kDbCommit, fresh);
  check(again.query(reopened, "SELECT COUNT(*) FROM t").rows[0][0] == DbValue(int64_t{1}), "BEGIN IMMEDIATE after the stop succeeds");
  // Malformed arguments are unavailable, never a crash.
  const Bytes junk{1, 2, 3};
  check(again.port->post(ports::kDbExecute, 1000, junk.data(), 3, &out) == 2, "malformed arguments");
  check(again.port->post(0x1234, 1001, nullptr, 0, &out) == 2, "an unknown method");
  ok("stop joins every database thread, drops what waits, rolls back; malformed calls are unavailable");
}

void testInjectionAndLargeValues() {
  Harness h(freshDir("values"));
  const uint32_t db = h.open(":memory:");
  h.execute(db, "CREATE TABLE t (v TEXT, b BLOB)");
  // Values travel only as bound parameters: SQL in a value is a value.
  for (const std::string &evil : {std::string("'; DROP TABLE t; --"), std::string("x'); DROP TABLE t; --"), std::string("\"; DELETE FROM t; --")}) {
    h.execute(db, "INSERT INTO t (v) VALUES (?)", {evil});
    const DbRows back = h.query(db, "SELECT v FROM t WHERE v = ?", {evil});
    check(back.rows.size() == 1 && textOf(back.rows[0][0]) == evil, "an injection attempt is stored as text: " + evil);
  }
  check(h.query(db, "SELECT COUNT(*) FROM sqlite_master WHERE name = 't'").rows[0][0] == DbValue(int64_t{1}), "and the table is still there");
  check(h.query(db, "SELECT COUNT(*) FROM t").rows[0][0] == DbValue(int64_t{3}), "with its three rows");
  // Two megabytes of text (U+0000 and a supplementary character included) and of blob, there and back.
  std::string text(2 * 1024 * 1024, 'a');
  text[17] = '\0';
  text.replace(1000, 4, "\xf0\x9f\x8c\x8d");
  Bytes blob(2 * 1024 * 1024 + 3, 0);
  for (std::size_t i = 0; i < blob.size(); ++i) blob[i] = static_cast<uint8_t>(i * 31);
  h.execute(db, "INSERT INTO t VALUES (?, ?)", {text, blob});
  const DbRows big = h.query(db, "SELECT v, b, length(v), length(b) FROM t WHERE b IS NOT NULL");
  check(big.rows.size() == 1 && textOf(big.rows[0][0]) == text, "2 MiB of text with U+0000 comes back byte for byte");
  check(big.rows[0][1] == DbValue(blob), "2 MiB of blob comes back byte for byte");
  check(big.rows[0][3] == DbValue(static_cast<int64_t>(blob.size())), "the blob's length in SQL");
  ok("values only as parameters (injection text stays text); 2 MiB text with U+0000 and 2 MiB blobs round-trip");
}

void testMigrationKeepsVersion() {
  Harness h(freshDir("keep"));
  const std::vector<DbMigration> v1 = {{1, "CREATE TABLE a (x INTEGER)"}};
  uint32_t version = 0;
  const uint32_t first = h.open("keep", v1, &version);
  check(version == 1, "at version 1");
  h.execute(first, "INSERT INTO a VALUES (41)");
  h.ok(ports::kDbClose, first);
  const std::vector<DbMigration> broken = {v1[0], {2, "CREATE TABLE b (y INTEGER); INSERT INTO a VALUES (42); INSERT INTO nowhere VALUES (1)"}};
  const DbFailure failed = h.failure(ports::kDbOpen, openArgs("keep", broken));
  check(failed.kind == DbErrorKind::Migration && failed.version == 2, "the broken migration 2 of a database at 1: " + failed.message);
  const uint32_t again = h.open("keep", v1, &version);
  check(version == 1, "user_version is still 1 after the failed migration, got " + std::to_string(version));
  check(h.query(again, "PRAGMA user_version").rows[0][0] == DbValue(int64_t{1}), "PRAGMA user_version 1");
  check(h.query(again, "SELECT COUNT(*) FROM sqlite_master WHERE name = 'b'").rows[0][0] == DbValue(int64_t{0}), "no table b");
  check(h.query(again, "SELECT x FROM a").rows.size() == 1, "the row of version 1 kept, migration 2's insert gone");
  ok("a failing migration leaves user_version and the data of the versions before it unchanged");
}

void testFailedCommit() {
  Harness h(freshDir("commit"));
  const uint32_t db = h.open("commit", {{1, "CREATE TABLE parent (id INTEGER PRIMARY KEY); "
                                            "CREATE TABLE child (pid INTEGER REFERENCES parent (id) DEFERRABLE INITIALLY DEFERRED)"}});
  const uint32_t tx = h.begin(db);
  h.execute(tx, "INSERT INTO child VALUES (999)"); // deferred: only the COMMIT checks it
  const DbFailure refused = h.failure(ports::kDbCommit, idArgs(tx));
  check(refused.kind == DbErrorKind::Constraint && refused.constraint == DbConstraintKind::ForeignKey,
      std::string("a commit a deferred foreign key refuses is Constraint { ForeignKey }: ") + kindName(refused.kind) + " " + refused.message);
  check(h.failure(ports::kDbExecute, statementArgs(tx, "SELECT 1")).message == "transaction " + std::to_string(tx) + " is over",
      "the refused transaction is over");
  check(h.query(db, "SELECT COUNT(*) FROM child").rows[0][0] == DbValue(int64_t{0}), "and rolled back");
  // Nothing dangles on the connection: the next transaction begins and commits, a plain statement autocommits.
  const uint32_t next = h.begin(db);
  h.execute(next, "INSERT INTO parent VALUES (1)");
  h.ok(ports::kDbCommit, next);
  h.execute(db, "INSERT INTO parent VALUES (2)");
  h.ok(ports::kDbClose, db);
  const uint32_t reopened = h.open("commit");
  check(h.query(reopened, "SELECT COUNT(*) FROM parent").rows[0][0] == DbValue(int64_t{2}), "both later writes are in the file");
  ok("a failed commit (deferred foreign key) rolls back and ends the transaction; the next one runs");
}

void testStopWhileRunning() {
  const std::string dir = freshDir("running");
  const std::string slow = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 2000000) SELECT COUNT(*) FROM c";
  {
    Harness h(dir);
    const uint32_t db = h.open("running");
    const uint32_t running = h.post(ports::kDbQuery, statementArgs(db, slow));
    std::this_thread::sleep_for(std::chrono::milliseconds(30));
    h.port->stop();
    // The statement that was running finished on its thread, which `stop` joined: answered before `stop`
    // returned, never after it.
    check(h.answered(running), "the running statement was answered before stop returned");
    std::size_t replies = 0;
    {
      std::lock_guard<std::mutex> lock(h.mutex);
      replies = h.replies.size();
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(100));
    std::lock_guard<std::mutex> lock(h.mutex);
    check(h.replies.size() == replies, "no reply arrives after stop returned");
  }
  {
    // The host goes away while a statement runs: its destructor stops and joins, so the reply cannot reach a
    // destroyed host (AddressSanitizer would see it).
    // A transaction is open too: the thread rolls it back after the statement, before it is joined.
    Harness h(dir);
    const uint32_t db = h.open("running", {{1, "CREATE TABLE t (v INTEGER)"}});
    const uint32_t tx = h.begin(db);
    h.execute(tx, "INSERT INTO t VALUES (1)");
    h.post(ports::kDbQuery, statementArgs(tx, slow));
    std::this_thread::sleep_for(std::chrono::milliseconds(30));
  }
  Harness after(dir);
  const uint32_t db = after.open("running");
  check(after.query(db, "SELECT COUNT(*) FROM t").rows[0][0] == DbValue(int64_t{0}), "the destroyed host's transaction was rolled back");
  const uint32_t tx = after.begin(db);
  after.ok(ports::kDbRollback, tx);
  ok("stop while a statement runs: answered before stop returns, nothing after; a host destroyed mid-statement");
}

} // namespace

/// Opens of one new database from several bindings at once (two cores, two stores opening "app" at launch) all
/// succeed and migrate it once: the version is read again under BEGIN IMMEDIATE's write lock, and the switch to WAL
/// (which SQLite answers BUSY at once while another connection makes it) waits like any other lock. A migration version
/// above SQLite's user_version range (a signed 32-bit integer) is refused before anything runs.
void testConcurrentOpens() {
  const std::string dir = freshDir("concurrent");
  const std::vector<DbMigration> v1v2 = {{1, "CREATE TABLE a (x INTEGER)"}, {2, "INSERT INTO a VALUES (1)"}};
  for (int round = 0; round < 8; ++round) {
    std::vector<std::unique_ptr<Harness>> bindings;
    for (int i = 0; i < 4; ++i) bindings.push_back(std::make_unique<Harness>(dir));
    const std::string name = "together" + std::to_string(round);
    std::vector<uint32_t> calls;
    for (auto &binding : bindings) calls.push_back(binding->post(ports::kDbOpen, openArgs(name, v1v2)));
    for (std::size_t i = 0; i < bindings.size(); ++i) {
      const Reply r = bindings[i]->wait(calls[i]);
      check(r.status == 0, "concurrent open " + std::to_string(i) + " of " + name + ": " + describe(r));
      check(getU32(r.body.data() + 4) == 2, "every concurrent open answers version 2");
    }
    const uint32_t db = bindings[0]->open(name, v1v2);
    check(bindings[0]->query(db, "SELECT COUNT(*) FROM a").rows[0][0] == DbValue(int64_t{1}), "migration 2 ran once");
  }
  Harness h(dir);
  const DbFailure wide = h.failure(ports::kDbOpen, openArgs("wide", {{1, "SELECT 1"}, {3000000000u, "SELECT 1"}}));
  check(wide.kind == DbErrorKind::Migration && wide.version == 3000000000u, "a version above user_version's range: " + wide.message);
  h.open("fits", {{2147483647u, "SELECT 1"}});
  ok("concurrent opens of one new database migrate it once; versions fit user_version");
}

int main() {
  char pattern[] = "/tmp/undra-rn-db.XXXXXX";
  const char *base = ::mkdtemp(pattern);
  check(base != nullptr, "a temporary directory");
  g_base = base;
  std::printf("# sqlite %s\n", sqlite3_libversion());
  testLexer();
  testErrors();
  testWireBytes();
  testOpenAndCells();
  testStatements();
  testMigrations();
  testTransactions();
  testCloseAndBusyFile();
  testFailures();
  testStop();
  testInjectionAndLargeValues();
  testMigrationKeepsVersion();
  testFailedCommit();
  testStopWhileRunning();
  testConcurrentOpens();
  std::string cmd = "rm -rf '" + g_base + "'";
  if (std::system(cmd.c_str()) != 0) std::fprintf(stderr, "# could not remove %s\n", g_base.c_str());
  std::printf("# %d Db checks passed\n", g_checks);
  return 0;
}
