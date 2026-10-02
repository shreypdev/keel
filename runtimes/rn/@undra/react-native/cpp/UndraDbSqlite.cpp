// The `Db` backend over the sqlite3 C API of the system `libsqlite3` (ADR-048; the brief, "Default
// adapters": iOS, and the host test on macOS and Linux). Android has no public sqlite3 for C (its
// backend is JNI to `android.database.sqlite`, in UndraPlatformAndroid.cpp), so this file is not part of
// the Android build (android/CMakeLists.txt); the pod links `libsqlite3` (UndraReactNative.podspec).
//
// A connection is used from one thread only (its database's worker in `DbPort`), so it is opened with
// `SQLITE_OPEN_NOMUTEX`. Errors map by extended result code (`dbFailureOfCode`), never by text.
#include <sqlite3.h>

#include <cerrno>
#include <cstring>
#include <memory>
#include <string>
#include <vector>

#include "UndraDb.h"

namespace undra::rn {

namespace {

/// The failure SQLite reports on `db` for result code `rc`.
DbFailure failureOf(sqlite3 *db, int rc) {
  const int code = db != nullptr ? sqlite3_extended_errcode(db) : rc;
  const char *message = db != nullptr ? sqlite3_errmsg(db) : sqlite3_errstr(rc);
  // `sqlite3_extended_errcode` is the last failure on the connection; `rc` is this one's (the same
  // when extended result codes are on, unless a misuse left no error on the connection).
  return dbFailureOfCode((code & 0xff) == (rc & 0xff) ? code : rc, message != nullptr ? message : sqlite3_errstr(rc));
}

/// Finalizes a statement when it goes out of scope.
struct Finalizer {
  sqlite3_stmt *stmt = nullptr;
  ~Finalizer() {
    if (stmt != nullptr) sqlite3_finalize(stmt);
  }
};

class SqliteConnection final : public DbConnection {
 public:
  explicit SqliteConnection(sqlite3 *db) : db_(db) {}
  ~SqliteConnection() override {
    close();
  }

  std::optional<DbFailure> execute(const std::string &sql, const std::vector<DbValue> &params, DbExecuted &out) override {
    Finalizer statement;
    if (auto failure = prepare(sql, params, statement.stmt)) return failure;
    int rc = SQLITE_ROW;
    while (rc == SQLITE_ROW) rc = sqlite3_step(statement.stmt); // rows of a statement that has some are not returned
    if (rc != SQLITE_DONE) return failureOf(db_, rc);
    out.changes = static_cast<uint64_t>(sqlite3_changes64(db_));
    out.lastInsertId = sqlite3_last_insert_rowid(db_);
    return std::nullopt;
  }

  std::optional<DbFailure> query(const std::string &sql, const std::vector<DbValue> &params, DbRows &out) override {
    out.columns.clear();
    out.rows.clear();
    Finalizer statement;
    if (auto failure = prepare(sql, params, statement.stmt)) return failure;
    const int columns = sqlite3_column_count(statement.stmt);
    out.columns.reserve(static_cast<std::size_t>(columns));
    for (int i = 0; i < columns; ++i) {
      const char *name = sqlite3_column_name(statement.stmt, i);
      out.columns.push_back(name != nullptr ? repairUtf8(name) : std::string());
    }
    while (true) {
      const int rc = sqlite3_step(statement.stmt);
      if (rc == SQLITE_DONE) return std::nullopt;
      if (rc != SQLITE_ROW) return failureOf(db_, rc);
      std::vector<DbValue> row;
      row.reserve(static_cast<std::size_t>(columns));
      for (int i = 0; i < columns; ++i) row.push_back(cell(statement.stmt, i));
      out.rows.push_back(std::move(row));
    }
  }

  std::optional<DbFailure> executeScript(const std::string &sql) override {
    char *error = nullptr;
    const int rc = sqlite3_exec(db_, sql.c_str(), nullptr, nullptr, &error);
    if (rc == SQLITE_OK) return std::nullopt;
    const int code = sqlite3_extended_errcode(db_);
    std::string message = error != nullptr ? error : sqlite3_errstr(rc);
    sqlite3_free(error);
    return dbFailureOfCode((code & 0xff) == (rc & 0xff) ? code : rc, std::move(message));
  }

  void close() noexcept override {
    if (db_ != nullptr) {
      sqlite3_close_v2(db_);
      db_ = nullptr;
    }
  }

 private:
  /// Prepares the one statement of `sql` and binds `params`: the brief's adapter contract (one
  /// statement, the parameter count, positional typed binds).
  std::optional<DbFailure> prepare(const std::string &sql, const std::vector<DbValue> &params, sqlite3_stmt *&stmt) {
    const char *at = sql.c_str();
    const char *end = sql.c_str() + sql.size();
    const char *tail = at;
    // Leading empty statements (`;`, comments) prepare to nothing: skip them.
    while (true) {
      const int rc = sqlite3_prepare_v2(db_, at, static_cast<int>(end - at), &stmt, &tail);
      if (rc != SQLITE_OK) return failureOf(db_, rc);
      if (stmt != nullptr || tail == nullptr || tail >= end || tail == at) break;
      at = tail;
    }
    if (stmt == nullptr) return DbFailure::sql(db_text::kNoStatement);
    if (tail != nullptr && tail < end && !sqlIsBlank(std::string_view(tail, static_cast<std::size_t>(end - tail)))) {
      return DbFailure::sql(db_text::kOneStatement);
    }
    const int expected = sqlite3_bind_parameter_count(stmt);
    if (static_cast<std::size_t>(expected) != params.size()) return DbFailure::sql(db_text::parameterCount(expected, params.size()));
    for (std::size_t i = 0; i < params.size(); ++i) {
      const int index = static_cast<int>(i) + 1;
      const DbValue &value = params[i];
      int rc = SQLITE_OK;
      switch (value.index()) {
        case 0:
          rc = sqlite3_bind_null(stmt, index);
          break;
        case 1:
          rc = sqlite3_bind_int64(stmt, index, std::get<int64_t>(value));
          break;
        case 2:
          rc = sqlite3_bind_double(stmt, index, std::get<double>(value));
          break;
        case 3: {
          // `c_str()` is never null, so an empty string binds as empty text (a null pointer would be NULL).
          const std::string &text = std::get<std::string>(value);
          rc = sqlite3_bind_text64(stmt, index, text.c_str(), text.size(), SQLITE_TRANSIENT, SQLITE_UTF8);
          break;
        }
        default: {
          const std::vector<uint8_t> &blob = std::get<std::vector<uint8_t>>(value);
          // An empty blob through `sqlite3_bind_blob` with a null pointer would be NULL.
          rc = blob.empty() ? sqlite3_bind_zeroblob(stmt, index, 0) : sqlite3_bind_blob64(stmt, index, blob.data(), blob.size(), SQLITE_TRANSIENT);
          break;
        }
      }
      if (rc != SQLITE_OK) return failureOf(db_, rc);
    }
    return std::nullopt;
  }

  /// Cell `i` of the current row, by its storage class.
  static DbValue cell(sqlite3_stmt *stmt, int i) {
    switch (sqlite3_column_type(stmt, i)) {
      case SQLITE_INTEGER:
        return static_cast<int64_t>(sqlite3_column_int64(stmt, i));
      case SQLITE_FLOAT:
        return sqlite3_column_double(stmt, i);
      case SQLITE_TEXT: {
        const auto *text = reinterpret_cast<const char *>(sqlite3_column_text(stmt, i));
        const int bytes = sqlite3_column_bytes(stmt, i);
        return repairUtf8(std::string_view(text != nullptr ? text : "", text != nullptr ? static_cast<std::size_t>(bytes) : 0));
      }
      case SQLITE_BLOB: {
        const auto *data = static_cast<const uint8_t *>(sqlite3_column_blob(stmt, i));
        const int bytes = sqlite3_column_bytes(stmt, i);
        if (data == nullptr || bytes <= 0) return std::vector<uint8_t>();
        return std::vector<uint8_t>(data, data + bytes);
      }
      default:
        return std::monostate{};
    }
  }

  sqlite3 *db_;
};

class SqliteBackend final : public DbBackend {
 public:
  explicit SqliteBackend(std::string directory) : directory_(std::move(directory)) {}

  std::unique_ptr<DbConnection> open(const std::string &name, DbFailure &failure) override {
    std::string path = name;
    if (name != ":memory:") {
      std::string error;
      if (!makeDirectories(directory_, error)) {
        failure = DbFailure::unavailable("cannot create " + directory_ + ": " + error);
        return nullptr;
      }
      path = directory_ + "/" + name + ".sqlite";
    }
    sqlite3 *db = nullptr;
    const int rc = sqlite3_open_v2(path.c_str(), &db, SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_NOMUTEX, nullptr);
    if (rc != SQLITE_OK) {
      failure = failureOf(db, rc);
      if (failure.kind == DbErrorKind::Unavailable && failure.message.find(path) == std::string::npos) {
        failure.message += " (" + path + ")";
      }
      sqlite3_close_v2(db);
      return nullptr;
    }
    sqlite3_extended_result_codes(db, 1);
    return std::make_unique<SqliteConnection>(db);
  }

  std::string describe() const override {
    return directory_ + "/<name>.sqlite";
  }

 private:
  std::string directory_;
};

} // namespace

std::unique_ptr<DbBackend> makeSqliteDbBackend(std::string directory) {
  return std::make_unique<SqliteBackend>(std::move(directory));
}

} // namespace undra::rn
