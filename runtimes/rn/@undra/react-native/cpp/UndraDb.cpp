#include "UndraDb.h"

#include <pthread.h>

#include <algorithm>
#include <cstdlib>
#include <cstring>
#include <iterator>
#include <new>
#include <utility>

namespace undra::rn {

static_assert(ports::kDb == 0x559eda82u, "Db port id (the brief, section 1)");
static_assert(ports::kDbOpen == 0xee6f26dbu, "Db.open method id");
static_assert(ports::kDbExecute == 0xffac2f0au, "Db.execute method id");
static_assert(ports::kDbQuery == 0x3a4deefdu, "Db.query method id");
static_assert(ports::kDbBegin == 0xae2ba428u, "Db.begin method id");
static_assert(ports::kDbCommit == 0xf866d5aeu, "Db.commit method id");
static_assert(ports::kDbRollback == 0x3e7b24b3u, "Db.rollback method id");
static_assert(ports::kDbClose == 0xde3dc7edu, "Db.close method id");

// ----- errors -----------------------------------------------------------------------------------

namespace {

// SQLite's primary result codes the mapping names (sqlite3.h).
constexpr int kSqlitePerm = 3;
constexpr int kSqliteBusy = 5;
constexpr int kSqliteLocked = 6;
constexpr int kSqliteReadOnly = 8;
constexpr int kSqliteIoErr = 10;
constexpr int kSqliteCorrupt = 11;
constexpr int kSqliteFull = 13;
constexpr int kSqliteCantOpen = 14;
constexpr int kSqliteConstraint = 19;
constexpr int kSqliteNotADb = 26;
// Its extended constraint codes.
constexpr int kConstraintCheck = 275;
constexpr int kConstraintForeignKey = 787;
constexpr int kConstraintNotNull = 1299;
constexpr int kConstraintPrimaryKey = 1555;
constexpr int kConstraintUnique = 2067;

std::string_view trimmed(std::string_view text) {
  while (!text.empty() && (text.front() == ' ' || text.front() == '\n' || text.front() == '\t')) text.remove_prefix(1);
  while (!text.empty() && (text.back() == ' ' || text.back() == '\n' || text.back() == '\t' || text.back() == ':' || text.back() == ','))
    text.remove_suffix(1);
  return text;
}

} // namespace

DbFailure dbFailureOfCode(int code, std::string message) {
  switch (code & 0xff) {
    case kSqliteBusy:
    case kSqliteLocked:
      return DbFailure::busy();
    case kSqliteConstraint:
      switch (code) {
        case kConstraintUnique:
        case kConstraintPrimaryKey:
          return DbFailure::constraintFailed(DbConstraintKind::Unique, std::move(message));
        case kConstraintNotNull:
          return DbFailure::constraintFailed(DbConstraintKind::NotNull, std::move(message));
        case kConstraintForeignKey:
          return DbFailure::constraintFailed(DbConstraintKind::ForeignKey, std::move(message));
        case kConstraintCheck:
          return DbFailure::constraintFailed(DbConstraintKind::Check, std::move(message));
        default:
          return DbFailure::constraintFailed(DbConstraintKind::Other, std::move(message));
      }
    case kSqliteCorrupt:
    case kSqliteNotADb:
      return DbFailure::corrupt(std::move(message));
    case kSqliteFull:
      return DbFailure::full();
    case kSqliteCantOpen:
    case kSqlitePerm:
    case kSqliteReadOnly:
    case kSqliteIoErr:
      return DbFailure::unavailable(std::move(message));
    default:
      return DbFailure::sql(std::move(message));
  }
}

DbFailure androidDbFailure(std::string_view className, std::string_view message) {
  // `<SQLite's message> (code 2067 SQLITE_CONSTRAINT_UNIQUE)[: , while compiling: <sql>]`: Android's
  // `throw_sqlite3_exception` (older releases write `(code 2067)`).
  const std::size_t at = message.find("(code ");
  if (at != std::string_view::npos) {
    std::size_t i = at + 6;
    int code = 0;
    bool digits = false;
    while (i < message.size() && message[i] >= '0' && message[i] <= '9' && code < 100000) {
      code = code * 10 + (message[i] - '0');
      digits = true;
      ++i;
    }
    if (digits) {
      std::string_view text = trimmed(message.substr(0, at));
      return dbFailureOfCode(code, std::string(text.empty() ? message : text));
    }
  }
  // No code: the exception's class says which (the few Android throws without SQLite's message).
  std::string_view simple = className;
  if (const std::size_t dot = simple.rfind('.'); dot != std::string_view::npos) simple = simple.substr(dot + 1);
  static const std::pair<std::string_view, int> kClasses[] = {
      {"SQLiteDatabaseLockedException", kSqliteBusy},
      {"SQLiteTableLockedException", kSqliteLocked},
      {"SQLiteConstraintException", kSqliteConstraint},
      {"SQLiteDatabaseCorruptException", kSqliteCorrupt},
      {"SQLiteFullException", kSqliteFull},
      {"SQLiteCantOpenDatabaseException", kSqliteCantOpen},
      {"SQLiteAccessPermException", kSqlitePerm},
      {"SQLiteReadOnlyDatabaseException", kSqliteReadOnly},
      {"SQLiteDiskIOException", kSqliteIoErr},
  };
  std::string text(message.empty() ? className : message);
  for (const auto &[name, code] : kClasses) {
    if (simple == name) return dbFailureOfCode(code, std::move(text));
  }
  return DbFailure::sql(std::move(text));
}

std::string dbErrorText(const DbFailure &failure) {
  switch (failure.kind) {
    case DbErrorKind::Busy:
      return "the database is busy";
    case DbErrorKind::Constraint:
      return "constraint failed: " + failure.message;
    case DbErrorKind::Corrupt:
      return "the database is corrupt: " + failure.message;
    case DbErrorKind::Full:
      return "the database is full";
    case DbErrorKind::Unavailable:
      return "the database is unavailable: " + failure.message;
    case DbErrorKind::Sql:
      return "SQL error: " + failure.message;
    case DbErrorKind::Migration:
      return "migration " + std::to_string(failure.version) + " failed: " + failure.message;
  }
  return failure.message;
}

void writeDbFailure(WireWriter &w, const DbFailure &failure) {
  w.u16(static_cast<uint16_t>(failure.kind));
  switch (failure.kind) {
    case DbErrorKind::Busy:
    case DbErrorKind::Full:
      break;
    case DbErrorKind::Constraint:
      w.u16(static_cast<uint16_t>(failure.constraint)).str(failure.message);
      break;
    case DbErrorKind::Migration:
      w.u32(failure.version).str(failure.message);
      break;
    case DbErrorKind::Corrupt:
    case DbErrorKind::Unavailable:
    case DbErrorKind::Sql:
      w.str(failure.message);
      break;
  }
}

void writeDbValue(WireWriter &w, const DbValue &value) {
  w.u16(static_cast<uint16_t>(value.index()));
  switch (value.index()) {
    case 1:
      w.u64(static_cast<uint64_t>(std::get<int64_t>(value)));
      break;
    case 2: {
      uint64_t bits = 0;
      const double real = std::get<double>(value);
      std::memcpy(&bits, &real, sizeof(bits));
      w.u64(bits);
      break;
    }
    case 3:
      w.str(std::get<std::string>(value));
      break;
    case 4:
      w.bytes(std::get<std::vector<uint8_t>>(value));
      break;
    default:
      break;
  }
}

DbValue readDbValue(WireReader &r) {
  switch (r.u16()) {
    case 0:
      return std::monostate{};
    case 1:
      return static_cast<int64_t>(r.u64());
    case 2: {
      const uint64_t bits = r.u64();
      double real = 0;
      std::memcpy(&real, &bits, sizeof(real));
      return real;
    }
    case 3:
      return r.str();
    case 4:
      return r.bytes();
    default:
      r.fail(); // an unknown variant: the arguments are malformed
      return std::monostate{};
  }
}

void writeDbRows(WireWriter &w, const DbRows &rows) {
  w.strings(rows.columns);
  w.u32(static_cast<uint32_t>(rows.rows.size()));
  for (const std::vector<DbValue> &row : rows.rows) {
    w.u32(static_cast<uint32_t>(row.size()));
    for (const DbValue &cell : row) writeDbValue(w, cell);
  }
}

std::string repairUtf8(std::string_view bytes) {
  if (validUtf8(bytes)) return std::string(bytes);
  std::string out;
  out.reserve(bytes.size() + 8);
  std::size_t i = 0;
  while (i < bytes.size()) {
    const auto c = static_cast<unsigned char>(bytes[i]);
    std::size_t len = c < 0x80 ? 1 : (c & 0xe0) == 0xc0 ? 2 : (c & 0xf0) == 0xe0 ? 3 : (c & 0xf8) == 0xf0 ? 4 : 0;
    if (len != 0 && i + len <= bytes.size() && validUtf8(bytes.substr(i, len))) {
      out.append(bytes.substr(i, len));
      i += len;
    } else {
      out.append("\xef\xbf\xbd");
      ++i;
    }
  }
  return out;
}

namespace db_text {
std::string parameterCount(int expected, std::size_t given) {
  return "the statement has " + std::to_string(expected) + " parameters, " + std::to_string(given) + " were given";
}
} // namespace db_text

// ----- the SQL lexer ----------------------------------------------------------------------------

namespace {

/// The token classes of `sqlite3_complete` (SQLite's complete.c).
enum Tok : uint8_t { kSemi = 0, kWs = 1, kOther = 2, kExplain = 3, kCreate = 4, kTemp = 5, kTrigger = 6, kEnd = 7 };

/// Its state machine: 0 INVALID, 1 START, 2 NORMAL, 3 EXPLAIN, 4 CREATE, 5 TRIGGER, 6 SEMI, 7 END. A `;`
/// that moves to START ends a statement; inside `CREATE TRIGGER ... BEGIN ... END` it does not.
constexpr uint8_t kTrans[8][8] = {
    /*            SEMI WS OTHER EXPLAIN CREATE TEMP TRIGGER END */
    /* 0 INVALID */ {1, 0, 2, 3, 4, 2, 2, 2},
    /* 1 START   */ {1, 1, 2, 3, 4, 2, 2, 2},
    /* 2 NORMAL  */ {1, 2, 2, 2, 2, 2, 2, 2},
    /* 3 EXPLAIN */ {1, 3, 3, 2, 4, 2, 2, 2},
    /* 4 CREATE  */ {1, 4, 2, 2, 2, 4, 5, 2},
    /* 5 TRIGGER */ {6, 5, 5, 5, 5, 5, 5, 5},
    /* 6 SEMI    */ {6, 6, 5, 5, 5, 5, 5, 7},
    /* 7 END     */ {1, 7, 5, 5, 5, 5, 5, 5},
};

/// SQLite's `IdChar`: letters, digits, `_`, `$` and every byte of a multi-byte character.
bool idChar(char ch) {
  const auto c = static_cast<unsigned char>(ch);
  return c >= 0x80 || (c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c == '_' || c == '$';
}

bool isKeyword(std::string_view word, std::string_view lower) {
  if (word.size() != lower.size()) return false;
  for (std::size_t i = 0; i < word.size(); ++i) {
    char c = word[i];
    if (c >= 'A' && c <= 'Z') c = static_cast<char>(c - 'A' + 'a');
    if (c != lower[i]) return false;
  }
  return true;
}

/// The end of the quoted text or comment that starts at `at` (SQLite's tokenizer: an unterminated
/// quote runs to the end, an error when prepared; an unterminated block comment is whitespace to the end).
std::size_t skipQuoted(std::string_view s, std::size_t at, char close) {
  const std::size_t found = s.find(close, at + 1);
  return found == std::string_view::npos ? s.size() : found + 1;
}

/// Reads the token at `at` (`at < s.size()`) into `end`; returns its class.
Tok lexToken(std::string_view s, std::size_t at, std::size_t &end) {
  const char c = s[at];
  const bool hasNext = at + 1 < s.size();
  switch (c) {
    case ';':
      end = at + 1;
      return kSemi;
    case ' ':
    case '\r':
    case '\t':
    case '\n':
    case '\f':
      end = at + 1;
      return kWs;
    case '/':
      if (hasNext && s[at + 1] == '*') {
        const std::size_t close = s.find("*/", at + 2);
        end = close == std::string_view::npos ? s.size() : close + 2;
        return kWs;
      }
      end = at + 1;
      return kOther;
    case '-':
      if (hasNext && s[at + 1] == '-') {
        const std::size_t newline = s.find('\n', at + 2);
        end = newline == std::string_view::npos ? s.size() : newline + 1;
        return kWs;
      }
      end = at + 1;
      return kOther;
    case '[':
      end = skipQuoted(s, at, ']');
      return kOther;
    case '`':
    case '"':
    case '\'':
      end = skipQuoted(s, at, c);
      return kOther;
    default:
      break;
  }
  if (!idChar(c)) {
    end = at + 1;
    return kOther;
  }
  std::size_t j = at + 1;
  while (j < s.size() && idChar(s[j])) ++j;
  end = j;
  const std::string_view word = s.substr(at, j - at);
  if (isKeyword(word, "create")) return kCreate;
  if (isKeyword(word, "trigger")) return kTrigger;
  if (isKeyword(word, "temp") || isKeyword(word, "temporary")) return kTemp;
  if (isKeyword(word, "end")) return kEnd;
  if (isKeyword(word, "explain")) return kExplain;
  return kOther;
}

} // namespace

std::vector<SqlStatement> splitSql(std::string_view sql) {
  std::vector<SqlStatement> out;
  uint8_t state = 0;
  std::size_t begin = 0;
  bool content = false;
  std::size_t at = 0;
  while (at < sql.size()) {
    std::size_t end = at;
    const Tok tok = lexToken(sql, at, end);
    if (tok != kWs && tok != kSemi && !content) {
      content = true;
      begin = at;
    }
    const uint8_t next = kTrans[state][tok];
    if (tok == kSemi && next == 1) {
      if (content) out.push_back({begin, end});
      content = false;
    }
    state = next;
    at = end;
  }
  if (content) out.push_back({begin, sql.size()});
  return out;
}

bool sqlIsBlank(std::string_view text) {
  std::size_t at = 0;
  while (at < text.size()) {
    std::size_t end = at;
    const Tok tok = lexToken(text, at, end);
    if (tok != kWs && tok != kSemi) return false;
    at = end;
  }
  return true;
}

int sqlParameterCount(std::string_view s) {
  int count = 0;
  std::vector<std::string_view> names;
  std::size_t i = 0;
  while (i < s.size()) {
    const char c = s[i];
    const bool hasNext = i + 1 < s.size();
    if (c == '\'' || c == '"' || c == '`') {
      i = skipQuoted(s, i, c);
    } else if (c == '[') {
      i = skipQuoted(s, i, ']');
    } else if (c == '-' && hasNext && s[i + 1] == '-') {
      const std::size_t newline = s.find('\n', i + 2);
      i = newline == std::string_view::npos ? s.size() : newline + 1;
    } else if (c == '/' && hasNext && s[i + 1] == '*') {
      const std::size_t close = s.find("*/", i + 2);
      i = close == std::string_view::npos ? s.size() : close + 2;
    } else if (c == '?') {
      // `?NNN` is index NNN; `?` the next one after the largest so far.
      std::size_t j = i + 1;
      long long n = 0;
      bool digits = false;
      while (j < s.size() && s[j] >= '0' && s[j] <= '9') {
        if (n < 1000000) n = n * 10 + (s[j] - '0');
        digits = true;
        ++j;
      }
      if (!digits) {
        ++count;
      } else if (n > count) {
        count = static_cast<int>(std::min<long long>(n, 1000000));
      }
      i = j;
    } else if (c == ':' || c == '@' || c == '$' || c == '#') {
      // `:AAA`, `@AAA`, `$AAA` (with SQLite's Tcl forms `::` and a `(...)` suffix): one index per name.
      std::size_t j = i + 1;
      int n = 0;
      while (j < s.size()) {
        const char d = s[j];
        if (idChar(d)) {
          ++n;
          ++j;
        } else if (d == '(' && n > 0) {
          ++j;
          while (j < s.size() && s[j] != ')' && s[j] != ' ' && s[j] != '\t' && s[j] != '\n' && s[j] != '\r' && s[j] != '\f') ++j;
          if (j < s.size() && s[j] == ')') ++j;
          break;
        } else if (d == ':' && j + 1 < s.size() && s[j + 1] == ':') {
          j += 2;
        } else {
          break;
        }
      }
      if (n > 0) {
        const std::string_view name = s.substr(i, j - i);
        if (std::find(names.begin(), names.end(), name) == names.end()) {
          names.push_back(name);
          ++count;
        }
        i = j;
      } else {
        i = i + 1;
      }
    } else if (idChar(c)) {
      while (i < s.size() && idChar(s[i])) ++i;
    } else {
      ++i;
    }
  }
  return count;
}

SqlShape sqlShapeOf(std::string_view sql) {
  SqlShape shape;
  const std::vector<SqlStatement> statements = splitSql(sql);
  if (statements.empty()) return shape;
  shape.first = statements.front();
  shape.trailing = statements.size() > 1;
  shape.parameters = sqlParameterCount(sql.substr(shape.first.begin, shape.first.end - shape.first.begin));
  return shape;
}

// ----- the binding ------------------------------------------------------------------------------

/// One port call, from `post` to the database's thread.
struct DbPort::Job {
  uint32_t method = 0;
  uint32_t portCallId = 0;
  /// The database or transaction id the call named.
  uint32_t target = 0;
  /// Whether `target` was a transaction when the call arrived.
  bool viaTx = false;
  std::string name;
  std::vector<DbMigration> migrations;
  std::string sql;
  std::vector<DbValue> params;
  /// Set the first time the call is parked: when it fails `Busy`.
  bool parked = false;
  std::chrono::steady_clock::time_point deadline{};
};

/// One database: its thread, its queue and its connection.
struct DbPort::Database {
  std::mutex mutex;
  std::condition_variable wake;
  /// Under `mutex`: calls to run, in arrival order, and calls waiting for the transaction slot.
  std::deque<Job> queue;
  std::deque<Job> parked;
  /// Under `mutex`: whether `post` may still queue here; whether the thread is to end now (`stop`);
  /// whether it ends once its queue is empty (closed, or its open failed).
  bool accepting = true;
  bool stopping = false;
  bool retiring = false;
  std::thread thread;
  std::atomic<bool> exited{false};
  // The thread's own.
  std::unique_ptr<DbConnection> conn;
  uint32_t id = 0;
  uint32_t tx = 0;
  bool closed = false;
};

namespace {

constexpr uint8_t kOk = 0;
constexpr uint8_t kTypedError = 1;
constexpr uint8_t kUnavailable = 2;

std::vector<uint8_t> replyOf(uint32_t portCallId, uint8_t status, const std::vector<uint8_t> &body = {}) {
  WireWriter w;
  w.out.reserve(5 + body.size());
  w.u32(portCallId).u8(status);
  w.out.insert(w.out.end(), body.begin(), body.end());
  return std::move(w.out);
}

std::vector<uint8_t> failureReply(uint32_t portCallId, const DbFailure &failure) {
  WireWriter w;
  w.u32(portCallId).u8(kTypedError);
  writeDbFailure(w, failure);
  return std::move(w.out);
}

/// Answers synchronously: `reply` into a `malloc`ed block the core frees (docs/SPEC.md section 6.3).
uint8_t answerNow(UndraBuf *out, const std::vector<uint8_t> &reply) noexcept {
  if (out == nullptr || reply.size() < 5 || reply.size() > UINT32_MAX) return kUnavailable;
  auto *block = static_cast<uint8_t *>(std::malloc(reply.size()));
  if (block == nullptr) return kUnavailable;
  std::memcpy(block, reply.data(), reply.size());
  out->ptr = block;
  out->len = static_cast<uint32_t>(reply.size());
  out->cap = 0;
  return 0;
}

std::string noSuchId(uint32_t id) {
  return "no open database or transaction " + std::to_string(id);
}

std::string txOver(uint32_t id) {
  return "transaction " + std::to_string(id) + " is over";
}

/// `name` is `":memory:"` or 1 to 64 of `A-Z a-z 0-9 . _ -` not starting with `.`.
bool validName(const std::string &name) {
  if (name == ":memory:") return true;
  if (name.empty() || name.size() > 64 || name.front() == '.') return false;
  return std::all_of(name.begin(), name.end(), [](char c) {
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '.' || c == '_' || c == '-';
  });
}

/// `name` as Rust's `{:?}` writes it (quotes, `\"`, `\\`, `\n`, ...).
std::string debugQuoted(const std::string &name) {
  std::string out = "\"";
  for (char c : name) {
    switch (c) {
      case '"':
        out += "\\\"";
        break;
      case '\\':
        out += "\\\\";
        break;
      case '\n':
        out += "\\n";
        break;
      case '\r':
        out += "\\r";
        break;
      case '\t':
        out += "\\t";
        break;
      case '\0':
        out += "\\0";
        break;
      default:
        out += c;
    }
  }
  return out + "\"";
}

} // namespace

DbPort::DbPort(std::shared_ptr<DbBackend> backend, Reply reply, DbThreadHooks hooks, DbOptions options)
    : backend_(std::move(backend)), reply_(std::move(reply)), hooks_(std::move(hooks)), options_(options) {}

DbPort::~DbPort() {
  stop();
}

std::size_t DbPort::openCount() const {
  std::lock_guard<std::mutex> lock(mutex_);
  return dbs_.size();
}

std::size_t DbPort::threadCount() const {
  std::lock_guard<std::mutex> lock(mutex_);
  return static_cast<std::size_t>(std::count_if(threads_.begin(), threads_.end(), [](const auto &db) { return !db->exited.load(); }));
}

void DbPort::send(const std::vector<uint8_t> &reply) noexcept {
  try {
    if (reply_) reply_(reply.data(), reply.size());
  } catch (...) {
    // The reply is lost; nothing may unwind out of a database's thread.
  }
}

void DbPort::reapLocked() {
  for (auto it = threads_.begin(); it != threads_.end();) {
    Database &db = **it;
    if (db.exited.load() && db.thread.joinable() && db.thread.get_id() != std::this_thread::get_id()) {
      db.thread.join();
      it = threads_.erase(it);
    } else {
      ++it;
    }
  }
}

uint8_t DbPort::post(uint32_t methodId, uint32_t portCallId, const uint8_t *args, uint32_t len, UndraBuf *out) noexcept {
  try {
    WireReader r(args, len);
    Job job;
    job.method = methodId;
    job.portCallId = portCallId;
    if (methodId == ports::kDbOpen) {
      job.name = r.str();
      const uint32_t count = r.u32();
      if (count > len) return kUnavailable; // more migrations than bytes: malformed
      for (uint32_t i = 0; i < count && r.ok(); ++i) {
        DbMigration m;
        m.version = r.u32();
        m.sql = r.str();
        job.migrations.push_back(std::move(m));
      }
      if (!r.finish()) return kUnavailable;
      if (!validName(job.name)) {
        return answerNow(out, failureReply(portCallId, DbFailure::unavailable("invalid database name " + debugQuoted(job.name) +
                                                                     ": use 1 to 64 of A-Z a-z 0-9 . _ - (not starting with .), or \":memory:\"")));
      }
      uint32_t last = 0;
      for (const DbMigration &m : job.migrations) {
        if (m.version <= last) {
          return answerNow(out, failureReply(portCallId, DbFailure::migration(m.version, "migration versions must strictly increase, starting at 1")));
        }
        last = m.version;
      }
      auto db = std::make_shared<Database>();
      db->queue.push_back(std::move(job));
      std::lock_guard<std::mutex> lock(mutex_);
      if (stopped_) return kUnavailable;
      reapLocked();
      threads_.push_back(db);
      try {
        db->thread = std::thread([this, db] { run(db); });
      } catch (...) {
        threads_.pop_back();
        return kUnavailable;
      }
      return 1;
    }

    const bool statement = methodId == ports::kDbExecute || methodId == ports::kDbQuery;
    if (!statement && methodId != ports::kDbBegin && methodId != ports::kDbCommit && methodId != ports::kDbRollback &&
        methodId != ports::kDbClose) {
      return kUnavailable;
    }
    job.target = r.u32();
    if (statement) {
      job.sql = r.str();
      const uint32_t count = r.u32();
      if (count > len) return kUnavailable;
      for (uint32_t i = 0; i < count && r.ok(); ++i) job.params.push_back(readDbValue(r));
    }
    if (!r.finish()) return kUnavailable;

    // Which database the id names, or the answer when it names none.
    std::shared_ptr<Database> db;
    std::optional<std::vector<uint8_t>> now;
    {
      std::lock_guard<std::mutex> lock(mutex_);
      if (stopped_) return kUnavailable;
      const uint32_t id = job.target;
      const uint8_t kind = id < kinds_.size() ? kinds_[id] : 0;
      const auto liveDb = dbs_.find(id);
      const auto liveTx = txs_.find(id);
      const bool isDb = liveDb != dbs_.end();
      const bool isTx = liveTx != txs_.end();
      if (statement) {
        if (isTx) {
          db = liveTx->second;
          job.viaTx = true;
        } else if (isDb) {
          db = liveDb->second;
        } else {
          now = failureReply(portCallId, DbFailure::unavailable(kind == 2 ? txOver(id) : noSuchId(id)));
        }
      } else if (methodId == ports::kDbBegin) {
        if (isDb) {
          db = liveDb->second;
        } else if (isTx) {
          now = failureReply(portCallId, DbFailure::sql("a transaction cannot begin inside a transaction"));
        } else {
          now = failureReply(portCallId, DbFailure::unavailable(kind == 2 ? txOver(id) : noSuchId(id)));
        }
      } else if (methodId == ports::kDbCommit || methodId == ports::kDbRollback) {
        if (isTx) {
          db = liveTx->second;
          job.viaTx = true;
        } else {
          now = failureReply(portCallId, DbFailure::unavailable(kind != 0 ? txOver(id) : noSuchId(id)));
        }
      } else { // close
        if (isDb) {
          db = liveDb->second;
        } else if (kind == 1) {
          now = replyOf(portCallId, kOk); // closed already: closing twice is not an error
        } else {
          now = failureReply(portCallId, DbFailure::unavailable(noSuchId(id)));
        }
      }
    }
    if (now) return answerNow(out, *now);
    const uint32_t target = job.target;
    const bool viaTx = job.viaTx;
    {
      std::lock_guard<std::mutex> lock(db->mutex);
      if (db->accepting) {
        db->queue.push_back(std::move(job));
        now.reset();
      } else {
        // The database closed between the lookup and here.
        now = failureReply(portCallId, DbFailure::unavailable(viaTx ? txOver(target) : noSuchId(target)));
      }
    }
    if (now) return answerNow(out, *now);
    db->wake.notify_one();
    return 1;
  } catch (...) {
    return kUnavailable; // out of memory
  }
}

void DbPort::run(const std::shared_ptr<Database> &shared) {
  Database &db = *shared;
  // At most 15 characters: Linux and Android refuse a longer thread name.
#if defined(__APPLE__)
  pthread_setname_np("undra-db");
#else
  pthread_setname_np(pthread_self(), "undra-db");
#endif
  if (hooks_.started) {
    try {
      hooks_.started("undra-db");
    } catch (...) {
    }
  }
  std::unique_lock<std::mutex> lock(db.mutex);
  while (!db.stopping) {
    const auto now = std::chrono::steady_clock::now();
    std::vector<Job> expired;
    for (auto it = db.parked.begin(); it != db.parked.end();) {
      if (it->deadline <= now) {
        expired.push_back(std::move(*it));
        it = db.parked.erase(it);
      } else {
        ++it;
      }
    }
    if (!expired.empty()) {
      lock.unlock();
      for (const Job &job : expired) send(failureReply(job.portCallId, DbFailure::busy()));
      lock.lock();
      continue;
    }
    if (!db.queue.empty()) {
      Job job = std::move(db.queue.front());
      db.queue.pop_front();
      lock.unlock();
      runJob(shared, job);
      lock.lock();
      continue;
    }
    if (db.retiring && db.parked.empty()) break;
    if (db.parked.empty()) {
      db.wake.wait(lock);
    } else {
      auto earliest = db.parked.front().deadline;
      for (const Job &job : db.parked) earliest = std::min(earliest, job.deadline);
      db.wake.wait_until(lock, earliest);
    }
  }
  db.accepting = false;
  lock.unlock();
  // The connection is this thread's: an open transaction is rolled back and it is closed here.
  if (db.conn != nullptr) {
    try {
      if (db.tx != 0) {
        DbExecuted ignored;
        (void)db.conn->execute("ROLLBACK", {}, ignored);
      }
    } catch (...) {
    }
    db.conn->close();
    db.conn.reset();
  }
  if (hooks_.ended) {
    try {
      hooks_.ended();
    } catch (...) {
    }
  }
  db.exited.store(true);
}

bool DbPort::parkIfBusy(Database &db, Job &job) {
  if (db.closed || db.tx == 0) return false;
  if (!job.parked) {
    job.parked = true;
    job.deadline = std::chrono::steady_clock::now() + options_.busyTimeout;
  }
  std::lock_guard<std::mutex> lock(db.mutex);
  db.parked.push_back(std::move(job));
  return true;
}

void DbPort::endTransaction(Database &db) {
  const uint32_t tx = db.tx;
  db.tx = 0;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    txs_.erase(tx);
  }
  // The calls that waited for the slot run next, before anything that arrived after them.
  std::lock_guard<std::mutex> lock(db.mutex);
  db.queue.insert(db.queue.begin(), std::make_move_iterator(db.parked.begin()), std::make_move_iterator(db.parked.end()));
  db.parked.clear();
}

void DbPort::runJob(const std::shared_ptr<Database> &shared, Job &job) {
  Database &db = *shared;
  std::vector<uint8_t> reply;
  bool built = false;
  const uint32_t portCallId = job.portCallId;
  try {
    const bool waits = !job.viaTx && (job.method == ports::kDbExecute || job.method == ports::kDbQuery || job.method == ports::kDbBegin);
    if (waits && parkIfBusy(db, job)) return;
    if (job.method == ports::kDbOpen) {
      reply = runOpen(shared, job);
    } else if (job.method == ports::kDbExecute || job.method == ports::kDbQuery) {
      reply = runStatement(db, job);
    } else if (job.method == ports::kDbBegin) {
      reply = runBegin(shared, job);
    } else if (job.method == ports::kDbCommit) {
      reply = runEnd(db, job, true);
    } else if (job.method == ports::kDbRollback) {
      reply = runEnd(db, job, false);
    } else {
      reply = runClose(db, job);
    }
    built = true;
  } catch (...) {
    // Out of memory (or a backend that threw): answered "unavailable" below, without allocating.
  }
  if (built) {
    send(reply);
    return;
  }
  uint8_t unavailable[5];
  putU32(unavailable, portCallId);
  unavailable[4] = kUnavailable;
  try {
    if (reply_) reply_(unavailable, sizeof(unavailable));
  } catch (...) {
  }
}

std::vector<uint8_t> DbPort::runOpen(const std::shared_ptr<Database> &shared, Job &job) {
  Database &db = *shared;
  const auto retire = [&] {
    std::lock_guard<std::mutex> lock(db.mutex);
    db.retiring = true;
  };
  const auto fail = [&](const DbFailure &failure) {
    if (db.conn != nullptr) {
      db.conn->close();
      db.conn.reset();
    }
    retire();
    return failureReply(job.portCallId, failure);
  };
  DbFailure failure = DbFailure::unavailable("the database could not be opened");
  db.conn = backend_ ? backend_->open(job.name, failure) : nullptr;
  if (db.conn == nullptr) return fail(failure);

  DbRows rows;
  for (const std::string &pragma : {std::string("PRAGMA foreign_keys = ON"),
                                    "PRAGMA busy_timeout = " + std::to_string(options_.busyTimeout.count()),
                                    std::string(options_.wal ? "PRAGMA journal_mode = WAL" : "")}) {
    if (pragma.empty()) continue;
    if (auto f = db.conn->query(pragma, {}, rows)) return fail(*f);
  }
  if (auto f = db.conn->query("PRAGMA user_version", {}, rows)) return fail(*f);
  uint32_t version = 0;
  if (!rows.rows.empty() && !rows.rows.front().empty()) {
    if (const auto *v = std::get_if<int64_t>(&rows.rows.front().front())) version = static_cast<uint32_t>(*v);
  }
  if (!job.migrations.empty()) {
    const uint32_t newest = job.migrations.back().version;
    if (version > newest) {
      return fail(DbFailure::migration(version, "the database is at version " + std::to_string(version) +
                                                   ", newer than the newest migration (" + std::to_string(newest) + ")"));
    }
  }
  std::vector<const DbMigration *> pending;
  for (const DbMigration &m : job.migrations) {
    if (m.version > version) pending.push_back(&m);
  }
  if (!pending.empty()) {
    DbExecuted ignored;
    if (auto f = db.conn->execute("BEGIN IMMEDIATE", {}, ignored)) return fail(*f);
    const auto rollBack = [&] {
      DbExecuted none;
      (void)db.conn->execute("ROLLBACK", {}, none);
    };
    for (const DbMigration *m : pending) {
      if (auto f = db.conn->executeScript(m->sql)) {
        rollBack();
        return fail(DbFailure::migration(m->version, dbErrorText(*f)));
      }
    }
    const uint32_t last = pending.back()->version;
    if (auto f = db.conn->execute("PRAGMA user_version = " + std::to_string(last), {}, ignored)) {
      rollBack();
      return fail(DbFailure::migration(last, dbErrorText(*f)));
    }
    if (auto f = db.conn->execute("COMMIT", {}, ignored)) {
      rollBack();
      return fail(DbFailure::migration(last, dbErrorText(*f)));
    }
    version = last;
  }
  {
    std::lock_guard<std::mutex> lock(mutex_);
    db.id = ++nextId_;
    if (kinds_.size() <= db.id) kinds_.resize(db.id + 1, 0);
    kinds_[db.id] = 1;
    if (!stopped_) dbs_[db.id] = shared;
  }
  WireWriter body;
  body.u32(db.id).u32(version);
  return replyOf(job.portCallId, kOk, body.out);
}

std::vector<uint8_t> DbPort::runStatement(Database &db, Job &job) {
  if (job.viaTx ? (db.closed || db.tx != job.target) : db.closed) {
    return failureReply(job.portCallId, DbFailure::unavailable(job.viaTx ? txOver(job.target) : noSuchId(job.target)));
  }
  WireWriter body;
  if (job.method == ports::kDbExecute) {
    DbExecuted done;
    if (auto f = db.conn->execute(job.sql, job.params, done)) return failureReply(job.portCallId, *f);
    body.u64(done.changes).u64(static_cast<uint64_t>(done.lastInsertId));
  } else {
    DbRows rows;
    if (auto f = db.conn->query(job.sql, job.params, rows)) return failureReply(job.portCallId, *f);
    writeDbRows(body, rows);
  }
  return replyOf(job.portCallId, kOk, body.out);
}

std::vector<uint8_t> DbPort::runBegin(const std::shared_ptr<Database> &shared, Job &job) {
  Database &db = *shared;
  if (db.closed) return failureReply(job.portCallId, DbFailure::unavailable(noSuchId(job.target)));
  DbExecuted ignored;
  if (auto f = db.conn->execute("BEGIN IMMEDIATE", {}, ignored)) return failureReply(job.portCallId, *f);
  uint32_t tx = 0;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    tx = ++nextId_;
    if (kinds_.size() <= tx) kinds_.resize(tx + 1, 0);
    kinds_[tx] = 2;
    if (!stopped_) txs_[tx] = shared;
  }
  db.tx = tx;
  WireWriter body;
  body.u32(tx);
  return replyOf(job.portCallId, kOk, body.out);
}

std::vector<uint8_t> DbPort::runEnd(Database &db, Job &job, bool commit) {
  if (db.closed || db.tx != job.target) return failureReply(job.portCallId, DbFailure::unavailable(txOver(job.target)));
  DbExecuted ignored;
  std::optional<DbFailure> failure;
  if (commit) {
    failure = db.conn->execute("COMMIT", {}, ignored);
    // A failed commit leaves SQLite's transaction open: it is rolled back, and the transaction is over.
    if (failure) (void)db.conn->execute("ROLLBACK", {}, ignored);
  } else {
    failure = db.conn->execute("ROLLBACK", {}, ignored);
  }
  endTransaction(db);
  if (failure) return failureReply(job.portCallId, *failure);
  return replyOf(job.portCallId, kOk);
}

std::vector<uint8_t> DbPort::runClose(Database &db, Job &job) {
  if (db.closed) return replyOf(job.portCallId, kOk);
  if (db.tx != 0) {
    DbExecuted ignored;
    (void)db.conn->execute("ROLLBACK", {}, ignored);
    endTransaction(db);
  }
  db.conn->close();
  db.conn.reset();
  db.closed = true;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    dbs_.erase(db.id);
  }
  {
    std::lock_guard<std::mutex> lock(db.mutex);
    db.retiring = true;
  }
  return replyOf(job.portCallId, kOk);
}

void DbPort::stop() noexcept {
  std::vector<std::shared_ptr<Database>> all;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    stopped_ = true;
    all.swap(threads_);
    dbs_.clear();
    txs_.clear();
  }
  for (const auto &db : all) {
    {
      std::lock_guard<std::mutex> lock(db->mutex);
      db->stopping = true;
      db->accepting = false;
      db->queue.clear();
      db->parked.clear();
    }
    db->wake.notify_all();
  }
  for (const auto &db : all) {
    if (!db->thread.joinable()) continue;
    if (db->thread.get_id() == std::this_thread::get_id()) {
      db->thread.detach(); // never happens: a database's own call does not stop the port
    } else {
      db->thread.join();
    }
  }
}

} // namespace undra::rn
