//! The `Db` port (ADR-048; feature `db`): SQLite owned by the platform, used from the core with
//! bound parameters, adapter-run migrations and transactions that never block the core.
//!
//! # Wire contract
//!
//! | Type | Wire form |
//! |---|---|
//! | [`DbMigration`] | `version u32, sql String` |
//! | [`DbOpened`] | `db u32, version u32` |
//! | [`DbValue`] | `u16` index: `Null` 0, `Integer(i64)` 1, `Real(f64)` 2, `Text(String)` 3, `Blob(Bytes)` 4 |
//! | [`DbExecuted`] | `changes u64, last_insert_id i64` |
//! | [`DbRows`] | `columns Vec<String>, rows Vec<Vec<DbValue>>` |
//! | [`DbConstraint`] | `u16` index: `Unique` 0, `NotNull` 1, `ForeignKey` 2, `Check` 3, `Other` 4 |
//! | [`DbError`] | `u16` index: `Busy` 0, `Constraint { kind, message }` 1, `Corrupt(String)` 2, `Full` 3, `Unavailable(String)` 4, `Sql { message }` 5, `Migration { version, message }` 6 |
//!
//! # Use
//!
//! ```no_run
//! use undra_ports::db::{Database, DbError, Migration};
//! use undra_ports::params;
//! use undra_runtime::Ctx;
//!
//! const MIGRATIONS: &[Migration] = &[
//!     Migration::new(1, "CREATE TABLE todos (id INTEGER PRIMARY KEY, title TEXT NOT NULL, done INTEGER NOT NULL)"),
//!     Migration::new(2, "CREATE INDEX todos_done ON todos (done)"),
//! ];
//!
//! async fn add(ctx: &Ctx, title: &str) -> Result<i64, DbError> {
//!     let db = Database::open(ctx, "app", MIGRATIONS).await?;
//!     let id = db
//!         .transaction(|tx| async move {
//!             let row = tx.execute("INSERT INTO todos (title, done) VALUES (?, 0)", params![title]).await?;
//!             tx.execute("UPDATE todos SET done = 0 WHERE id = ?", params![row.last_insert_id]).await?;
//!             Ok(row.last_insert_id)
//!         })
//!         .await?;
//!     let rows = db.query("SELECT title, done FROM todos WHERE id = ?", params![id]).await?;
//!     let done: bool = rows.row(0).expect("inserted").get("done")?;
//!     assert!(!done);
//!     Ok(id)
//! }
//! ```
//!
//! SQL is `&'static str`, so a value can only reach a statement as a parameter; SQL built at run
//! time (an `IN (?, ?, ?)` list) goes through [`Database::execute_dynamic`] /
//! [`Database::query_dynamic`], named so it stands out in review. SQLite's time functions read
//! the adapter's clock, not the core's: deterministic code binds `ctx.clock()` values instead.

use core::fmt;
use core::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::owned::owned;

use undra_runtime::{Ctx, PortError, WeakCtx};
use undra_wire::{Bytes, Decode};

/// One schema migration, as the port carries it.
#[undra_macros::api]
#[undra(crate = "crate::root")]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DbMigration {
    /// The version this migration brings the database to; versions strictly increase from 1.
    pub version: u32,
    /// The SQL, possibly several statements.
    pub sql: String,
}

/// What `open` answers: the database's id and its version after migrating.
#[undra_macros::api]
#[undra(crate = "crate::root")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DbOpened {
    /// The database's id, chosen by the adapter; never reused by it.
    pub db: u32,
    /// `PRAGMA user_version` after the migrations ran (0 for a database without any).
    pub version: u32,
}

/// One SQLite value: the five storage classes.
#[undra_macros::api]
#[undra(crate = "crate::root")]
#[derive(Clone, Debug, PartialEq)]
pub enum DbValue {
    /// `NULL`.
    Null,
    /// A 64-bit signed integer.
    Integer(i64),
    /// A 64-bit float.
    Real(f64),
    /// UTF-8 text.
    Text(String),
    /// Bytes.
    Blob(Bytes),
}

/// What `execute` answers.
#[undra_macros::api]
#[undra(crate = "crate::root")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DbExecuted {
    /// Rows inserted, updated or deleted by the statement (`sqlite3_changes64`).
    pub changes: u64,
    /// The rowid of the last insert on the connection (`sqlite3_last_insert_rowid`).
    pub last_insert_id: i64,
}

/// What `query` answers: the column names and the rows, each a cell per column.
#[undra_macros::api]
#[undra(crate = "crate::root")]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DbRows {
    /// The result's column names, in order.
    pub columns: Vec<String>,
    /// The rows, each with one value per column.
    pub rows: Vec<Vec<DbValue>>,
}

/// Which constraint a statement broke.
#[undra_macros::api]
#[undra(crate = "crate::root")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DbConstraint {
    /// `UNIQUE` or `PRIMARY KEY`.
    Unique,
    /// `NOT NULL`.
    NotNull,
    /// `FOREIGN KEY` (enforced: every adapter opens with `foreign_keys = ON`).
    ForeignKey,
    /// `CHECK`.
    Check,
    /// Any other constraint (a trigger's `RAISE`, ...).
    Other,
}

/// Why a database operation failed (ADR-048). The variant follows SQLite's result code, never the
/// message text (Android, which reports no code, is the documented exception).
#[undra_macros::error]
#[undra(crate = "crate::root")]
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum DbError {
    /// The database stayed locked past the adapter's busy timeout (5 s): another connection, or a
    /// statement made outside a running transaction on the same database.
    #[error("the database is busy")]
    Busy,
    /// The statement broke a constraint; `message` is SQLite's (it names the columns).
    #[error("constraint failed: {message}")]
    Constraint {
        /// Which kind of constraint.
        kind: DbConstraint,
        /// SQLite's message, for example `UNIQUE constraint failed: todos.id`.
        message: String,
    },
    /// The file is not a database or is damaged.
    #[error("the database is corrupt: {0}")]
    Corrupt(String),
    /// The disk or the storage quota is full.
    #[error("the database is full")]
    Full,
    /// No adapter, a database or transaction that is closed or unknown, an invalid name, a file
    /// that cannot be opened.
    #[error("the database is unavailable: {0}")]
    Unavailable(String),
    /// Anything else SQLite refused: a syntax error, a missing table, a wrong cell type, more
    /// than one statement where one is expected.
    #[error("SQL error: {message}")]
    Sql {
        /// SQLite's message (or the adapter's).
        message: String,
    },
    /// A migration failed (everything it and the migrations before it in the same open did is
    /// rolled back), or the database is at a version newer than the newest migration.
    #[error("migration {version} failed: {message}")]
    Migration {
        /// The version that failed, or the database's version when it is too new.
        version: u32,
        /// What went wrong.
        message: String,
    },
}

/// A port that cannot answer is an ordinary outcome (SPEC 6.3):
///
/// | `PortError` | `DbError` |
/// |---|---|
/// | `Unavailable` | `Unavailable("the Db port has no adapter registered (E0062: ..)")` |
/// | `Cancelled` | `Unavailable("the Db call was cancelled")` |
/// | `Decode(e)` | `Sql { message: "malformed port reply: <e>" }` |
/// | `Failed(bytes)` | the decoded `DbError`, else `Sql { .. }` |
impl From<PortError> for DbError {
    fn from(error: PortError) -> Self {
        if let PortError::Failed(bytes) = &error {
            return DbError::decode_exact(bytes).unwrap_or_else(|_| DbError::Sql {
                message: "the Db port reported an error that does not decode".to_owned(),
            });
        }
        match error {
            PortError::Unavailable => DbError::Unavailable(
                "the Db port has no adapter registered (E0062: register one, see https://shreypdev.github.io/undra/docs/errors.html#E0062)"
                    .to_owned(),
            ),
            PortError::Cancelled => DbError::Unavailable("the Db call was cancelled".to_owned()),
            PortError::Decode(why) => DbError::Sql {
                message: format!("malformed port reply: {why}"),
            },
            other => DbError::Unavailable(format!("the Db port call failed: {other}")),
        }
    }
}

/// SQLite on the platform (ADR-048). Use [`Database`] rather than calling it directly.
///
/// Every open database has one connection and one serial worker owned by the adapter; the core
/// only awaits replies. Statements on a transaction id run inside it; statements on the database
/// id wait for a running transaction to end (at most the busy timeout, then `Busy`).
#[undra_macros::port(dispatcher_by_use)]
#[undra(crate = "crate::root")]
pub trait Db {
    /// Opens (creating it if needed) the database `name` and runs, in one transaction, every
    /// migration above its `user_version`.
    async fn open(&self, name: String, migrations: Vec<DbMigration>) -> Result<DbOpened, DbError>;
    /// Runs one statement with `params` bound positionally; `db` is a database or transaction id.
    async fn execute(
        &self,
        db: u32,
        sql: String,
        params: Vec<DbValue>,
    ) -> Result<DbExecuted, DbError>;
    /// Runs one query with `params` bound positionally and returns every row.
    async fn query(&self, db: u32, sql: String, params: Vec<DbValue>) -> Result<DbRows, DbError>;
    /// Waits for any running transaction on `db`, starts one (`BEGIN IMMEDIATE`) and returns its id.
    async fn begin(&self, db: u32) -> Result<u32, DbError>;
    /// Commits transaction `tx` and ends it (a failed commit rolls back and ends it too).
    async fn commit(&self, tx: u32) -> Result<(), DbError>;
    /// Rolls transaction `tx` back and ends it.
    async fn rollback(&self, tx: u32) -> Result<(), DbError>;
    /// Closes database `db` (a running transaction is rolled back). Closing twice is not an error.
    async fn close(&self, db: u32) -> Result<(), DbError>;
}

/// One schema migration, written as a constant: `Migration::new(1, "CREATE TABLE ..")`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Migration {
    /// The version it brings the database to; versions strictly increase from 1.
    pub version: u32,
    /// The SQL, possibly several statements.
    pub sql: &'static str,
}

impl Migration {
    /// A migration to `version`.
    pub const fn new(version: u32, sql: &'static str) -> Migration {
        Migration { version, sql }
    }
}

/// The longest database name.
pub const MAX_NAME_LEN: usize = 64;

/// The name that opens a private in-memory database.
pub const MEMORY: &str = ":memory:";

/// Checks a database name: [`MEMORY`], or 1 to 64 of `A-Z a-z 0-9 . _ -` not starting with `.`.
pub fn validate_name(name: &str) -> Result<(), DbError> {
    let valid = name == MEMORY
        || (!name.is_empty()
            && name.len() <= MAX_NAME_LEN
            && !name.starts_with('.')
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')));
    if valid {
        Ok(())
    } else {
        Err(DbError::Unavailable(format!(
            "invalid database name {name:?}: use 1 to 64 of A-Z a-z 0-9 . _ - (not starting with .), or \":memory:\""
        )))
    }
}

/// Checks that migration versions strictly increase from 1 (what every adapter checks again).
pub fn validate_migrations(migrations: &[DbMigration]) -> Result<(), DbError> {
    let mut last = 0u32;
    for migration in migrations {
        if migration.version <= last {
            return Err(DbError::Migration {
                version: migration.version,
                message: "migration versions must strictly increase, starting at 1".to_owned(),
            });
        }
        last = migration.version;
    }
    Ok(())
}

struct Shared {
    port: Arc<dyn Db>,
    db: u32,
    weak: Option<WeakCtx>,
    closed: AtomicBool,
}

impl Drop for Shared {
    /// A database nobody closed is closed when the last handle goes, fire and forget.
    fn drop(&mut self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let Some(ctx) = self.weak.as_ref().and_then(|weak| weak.upgrade().ok()) else {
            return;
        };
        let (port, db) = (self.port.clone(), self.db);
        ctx.spawn(async move {
            let _ = port.close(db).await;
        });
    }
}

/// An open database (ADR-048). Cheap to clone; closed when the last clone is dropped.
#[derive(Clone)]
pub struct Database {
    shared: Arc<Shared>,
    version: u32,
}

impl fmt::Debug for Database {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Database")
            .field("db", &self.shared.db)
            .field("version", &self.version)
            .finish()
    }
}

impl Database {
    /// Opens database `name` through the runtime's `Db` port (a fake if one is bound, else the
    /// platform) and brings it to the newest of `migrations`.
    pub async fn open(
        ctx: &Ctx,
        name: &str,
        migrations: &[Migration],
    ) -> Result<Database, DbError> {
        let migrations: Vec<DbMigration> = migrations
            .iter()
            .map(|m| DbMigration {
                version: m.version,
                sql: m.sql.to_owned(),
            })
            .collect();
        Database::open_with(crate::db(ctx), Some(ctx.downgrade()), name, migrations).await
    }

    /// Opens database `name` on `port`; `weak` is where a dropped database is closed from.
    pub async fn open_with(
        port: Arc<dyn Db>,
        weak: Option<WeakCtx>,
        name: &str,
        migrations: Vec<DbMigration>,
    ) -> Result<Database, DbError> {
        validate_name(name)?;
        validate_migrations(&migrations)?;
        // In a task of its own: a caller cancelled while `open` crosses must not leave the
        // database the platform opened open until shutdown.
        let (opener, closer, name) = (port.clone(), port.clone(), name.to_owned());
        let opened = owned(
            weak.as_ref(),
            async move { opener.open(name, migrations).await },
            move |opened: DbOpened| {
                Box::pin(async move {
                    let _ = closer.close(opened.db).await;
                })
            },
        )
        .await?;
        Ok(Database {
            shared: Arc::new(Shared {
                port,
                db: opened.db,
                weak,
                closed: AtomicBool::new(false),
            }),
            version: opened.version,
        })
    }

    /// The adapter's id of this database.
    pub fn id(&self) -> u32 {
        self.shared.db
    }

    /// The database's version after the migrations ran.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Runs one statement with `params` bound positionally (`?`, `?NNN`).
    pub async fn execute(
        &self,
        sql: &'static str,
        params: Vec<DbValue>,
    ) -> Result<DbExecuted, DbError> {
        self.execute_dynamic(sql.to_owned(), params).await
    }

    /// Runs one query with `params` bound positionally.
    pub async fn query(&self, sql: &'static str, params: Vec<DbValue>) -> Result<DbRows, DbError> {
        self.query_dynamic(sql.to_owned(), params).await
    }

    /// [`execute`](Database::execute) with SQL built at run time. Values still go in `params`.
    pub async fn execute_dynamic(
        &self,
        sql: String,
        params: Vec<DbValue>,
    ) -> Result<DbExecuted, DbError> {
        self.shared.port.execute(self.shared.db, sql, params).await
    }

    /// [`query`](Database::query) with SQL built at run time. Values still go in `params`.
    pub async fn query_dynamic(
        &self,
        sql: String,
        params: Vec<DbValue>,
    ) -> Result<DbRows, DbError> {
        self.shared.port.query(self.shared.db, sql, params).await
    }

    /// Runs `body` in a transaction: committed when it returns `Ok`, rolled back when it returns
    /// `Err` (which is returned) or when its future is dropped before finishing.
    ///
    /// Inside, use the [`Transaction`] it is given: a statement on the `Database` itself waits
    /// for the transaction to end and fails `Busy` after the adapter's busy timeout.
    pub async fn transaction<F, Fut, R>(&self, body: F) -> Result<R, DbError>
    where
        F: FnOnce(Transaction) -> Fut + Send,
        Fut: Future<Output = Result<R, DbError>> + Send,
        R: Send,
    {
        let port = self.shared.port.clone();
        // In a task of its own: a caller cancelled while `begin` crosses must not leave the
        // transaction the platform began open (every later statement would be `Busy`).
        let (begin, undo, db) = (port.clone(), port.clone(), self.shared.db);
        let id = owned(
            self.shared.weak.as_ref(),
            async move { begin.begin(db).await },
            move |tx| {
                Box::pin(async move {
                    let _ = undo.rollback(tx).await;
                })
            },
        )
        .await?;
        let mut guard = RollbackOnDrop {
            port: port.clone(),
            tx: id,
            weak: self.shared.weak.clone(),
            armed: true,
        };
        let outcome = body(Transaction {
            port: port.clone(),
            id,
        })
        .await;
        guard.armed = false;
        match outcome {
            Ok(value) => {
                port.commit(id).await?;
                Ok(value)
            }
            Err(error) => {
                let _ = port.rollback(id).await;
                Err(error)
            }
        }
    }

    /// Closes the database. Other clones see `Unavailable` afterwards.
    pub async fn close(self) -> Result<(), DbError> {
        self.shared.closed.store(true, Ordering::Release);
        self.shared.port.close(self.shared.db).await
    }
}

/// Rolls a transaction back when the future that runs it is dropped half-way.
struct RollbackOnDrop {
    port: Arc<dyn Db>,
    tx: u32,
    weak: Option<WeakCtx>,
    armed: bool,
}

impl Drop for RollbackOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Some(ctx) = self.weak.as_ref().and_then(|weak| weak.upgrade().ok()) else {
            return;
        };
        let (port, tx) = (self.port.clone(), self.tx);
        ctx.spawn(async move {
            let _ = port.rollback(tx).await;
        });
    }
}

/// A running transaction, given to the body of [`Database::transaction`].
#[derive(Clone)]
pub struct Transaction {
    port: Arc<dyn Db>,
    id: u32,
}

impl fmt::Debug for Transaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transaction").field("tx", &self.id).finish()
    }
}

impl Transaction {
    /// The adapter's id of this transaction.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Runs one statement inside the transaction.
    pub async fn execute(
        &self,
        sql: &'static str,
        params: Vec<DbValue>,
    ) -> Result<DbExecuted, DbError> {
        self.execute_dynamic(sql.to_owned(), params).await
    }

    /// Runs one query inside the transaction.
    pub async fn query(&self, sql: &'static str, params: Vec<DbValue>) -> Result<DbRows, DbError> {
        self.query_dynamic(sql.to_owned(), params).await
    }

    /// [`execute`](Transaction::execute) with SQL built at run time.
    pub async fn execute_dynamic(
        &self,
        sql: String,
        params: Vec<DbValue>,
    ) -> Result<DbExecuted, DbError> {
        self.port.execute(self.id, sql, params).await
    }

    /// [`query`](Transaction::query) with SQL built at run time.
    pub async fn query_dynamic(
        &self,
        sql: String,
        params: Vec<DbValue>,
    ) -> Result<DbRows, DbError> {
        self.port.query(self.id, sql, params).await
    }
}

// ----- values in -------------------------------------------------------------------------------

macro_rules! integer_into {
    ($($ty:ty),*) => {$(
        impl From<$ty> for DbValue {
            fn from(value: $ty) -> DbValue {
                DbValue::Integer(i64::from(value))
            }
        }
    )*};
}

integer_into!(i8, i16, i32, i64, u8, u16, u32);

impl From<bool> for DbValue {
    fn from(value: bool) -> DbValue {
        DbValue::Integer(i64::from(value))
    }
}

impl From<f64> for DbValue {
    fn from(value: f64) -> DbValue {
        DbValue::Real(value)
    }
}

impl From<f32> for DbValue {
    fn from(value: f32) -> DbValue {
        DbValue::Real(f64::from(value))
    }
}

impl From<&str> for DbValue {
    fn from(value: &str) -> DbValue {
        DbValue::Text(value.to_owned())
    }
}

impl From<String> for DbValue {
    fn from(value: String) -> DbValue {
        DbValue::Text(value)
    }
}

impl From<&String> for DbValue {
    fn from(value: &String) -> DbValue {
        DbValue::Text(value.clone())
    }
}

impl From<Vec<u8>> for DbValue {
    fn from(value: Vec<u8>) -> DbValue {
        DbValue::Blob(Bytes(value))
    }
}

impl From<&[u8]> for DbValue {
    fn from(value: &[u8]) -> DbValue {
        DbValue::Blob(Bytes(value.to_vec()))
    }
}

impl From<Bytes> for DbValue {
    fn from(value: Bytes) -> DbValue {
        DbValue::Blob(value)
    }
}

impl<T: Into<DbValue>> From<Option<T>> for DbValue {
    fn from(value: Option<T>) -> DbValue {
        value.map_or(DbValue::Null, Into::into)
    }
}

/// The positional parameters of a statement: `params![title, done, id]` is a `Vec<DbValue>`.
///
/// ```
/// use undra_ports::db::DbValue;
/// use undra_ports::params;
///
/// let none: Option<i64> = None;
/// assert_eq!(
///     params!["milk", 2_i64, true, 1.5, none],
///     vec![
///         DbValue::Text("milk".into()),
///         DbValue::Integer(2),
///         DbValue::Integer(1),
///         DbValue::Real(1.5),
///         DbValue::Null,
///     ]
/// );
/// assert!(params![].is_empty());
/// ```
#[macro_export]
macro_rules! params {
    () => { ::std::vec::Vec::<$crate::db::DbValue>::new() };
    ($($value:expr),+ $(,)?) => { ::std::vec![$($crate::db::DbValue::from($value)),+] };
}

// ----- values out ------------------------------------------------------------------------------

impl DbValue {
    /// The storage class's name, as SQLite's `typeof()` spells it.
    pub fn type_name(&self) -> &'static str {
        match self {
            DbValue::Null => "null",
            DbValue::Integer(_) => "integer",
            DbValue::Real(_) => "real",
            DbValue::Text(_) => "text",
            DbValue::Blob(_) => "blob",
        }
    }
}

/// A type a cell can be read as ([`DbRow::get`]).
pub trait FromDbValue: Sized {
    /// Reads `value`; `None` when the storage class does not fit.
    fn from_db_value(value: &DbValue) -> Option<Self>;
    /// What the type expects, for the error message.
    const EXPECTED: &'static str;
}

impl FromDbValue for i64 {
    const EXPECTED: &'static str = "integer";
    fn from_db_value(value: &DbValue) -> Option<i64> {
        match value {
            DbValue::Integer(v) => Some(*v),
            _ => None,
        }
    }
}

macro_rules! integer_out {
    ($($ty:ty),*) => {$(
        impl FromDbValue for $ty {
            const EXPECTED: &'static str = concat!("integer in the range of ", stringify!($ty));
            fn from_db_value(value: &DbValue) -> Option<$ty> {
                match value {
                    DbValue::Integer(v) => <$ty>::try_from(*v).ok(),
                    _ => None,
                }
            }
        }
    )*};
}

integer_out!(i32, u32, u64, i16, u16, i8, u8);

impl FromDbValue for bool {
    const EXPECTED: &'static str = "integer 0 or 1";
    fn from_db_value(value: &DbValue) -> Option<bool> {
        match value {
            DbValue::Integer(0) => Some(false),
            DbValue::Integer(1) => Some(true),
            _ => None,
        }
    }
}

impl FromDbValue for f64 {
    const EXPECTED: &'static str = "real or integer";
    fn from_db_value(value: &DbValue) -> Option<f64> {
        match value {
            DbValue::Real(v) => Some(*v),
            // SQLite stores a REAL column's integral value as an integer when it can.
            #[allow(clippy::cast_precision_loss)]
            DbValue::Integer(v) => Some(*v as f64),
            _ => None,
        }
    }
}

impl FromDbValue for String {
    const EXPECTED: &'static str = "text";
    fn from_db_value(value: &DbValue) -> Option<String> {
        match value {
            DbValue::Text(v) => Some(v.clone()),
            _ => None,
        }
    }
}

impl FromDbValue for Vec<u8> {
    const EXPECTED: &'static str = "blob";
    fn from_db_value(value: &DbValue) -> Option<Vec<u8>> {
        match value {
            DbValue::Blob(v) => Some(v.0.clone()),
            _ => None,
        }
    }
}

impl FromDbValue for Bytes {
    const EXPECTED: &'static str = "blob";
    fn from_db_value(value: &DbValue) -> Option<Bytes> {
        match value {
            DbValue::Blob(v) => Some(v.clone()),
            _ => None,
        }
    }
}

impl FromDbValue for DbValue {
    const EXPECTED: &'static str = "any value";
    fn from_db_value(value: &DbValue) -> Option<DbValue> {
        Some(value.clone())
    }
}

impl<T: FromDbValue> FromDbValue for Option<T> {
    const EXPECTED: &'static str = T::EXPECTED;
    fn from_db_value(value: &DbValue) -> Option<Option<T>> {
        match value {
            DbValue::Null => Some(None),
            other => T::from_db_value(other).map(Some),
        }
    }
}

/// How [`DbRow::get`] names a column: its position (`usize`) or its name (`&str`).
pub trait ColumnIndex: fmt::Debug {
    /// The position of the column in `columns`.
    fn position(&self, columns: &[String]) -> Option<usize>;
}

impl ColumnIndex for usize {
    fn position(&self, columns: &[String]) -> Option<usize> {
        (*self < columns.len()).then_some(*self)
    }
}

impl ColumnIndex for &str {
    fn position(&self, columns: &[String]) -> Option<usize> {
        columns.iter().position(|c| c == self)
    }
}

/// One row of a [`DbRows`].
#[derive(Clone, Copy, Debug)]
pub struct DbRow<'a> {
    columns: &'a [String],
    cells: &'a [DbValue],
}

impl<'a> DbRow<'a> {
    /// The cells, one per column.
    pub fn cells(&self) -> &'a [DbValue] {
        self.cells
    }

    /// The cell in `column` read as `T`: a missing column or a cell of another storage class is
    /// `Sql`, never a panic.
    pub fn get<T: FromDbValue>(&self, column: impl ColumnIndex) -> Result<T, DbError> {
        let Some(at) = column.position(self.columns) else {
            return Err(DbError::Sql {
                message: format!("no column {column:?} in the result"),
            });
        };
        let cell = self.cells.get(at).unwrap_or(&DbValue::Null);
        T::from_db_value(cell).ok_or_else(|| DbError::Sql {
            message: format!(
                "column {column:?} is {}, expected {}",
                cell.type_name(),
                T::EXPECTED
            ),
        })
    }
}

impl DbRows {
    /// How many rows.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether there are no rows.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Row `index`, if there is one.
    pub fn row(&self, index: usize) -> Option<DbRow<'_>> {
        self.rows.get(index).map(|cells| DbRow {
            columns: &self.columns,
            cells,
        })
    }

    /// The rows, in order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = DbRow<'_>> + '_ {
        self.rows.iter().map(|cells| DbRow {
            columns: &self.columns,
            cells,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use undra_wire::Encode;

    #[test]
    fn exact_bytes_of_the_records() {
        assert_eq!(DbValue::Null.encode_to_vec(), [0, 0]);
        assert_eq!(
            DbValue::Integer(-2).encode_to_vec(),
            [1, 0, 0xfe, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
        );
        assert_eq!(
            DbValue::Real(1.0).encode_to_vec(),
            [2, 0, 0, 0, 0, 0, 0, 0, 0xf0, 0x3f]
        );
        assert_eq!(
            DbValue::Text("a".into()).encode_to_vec(),
            [3, 0, 1, 0, 0, 0, b'a']
        );
        assert_eq!(
            DbValue::Blob(Bytes(vec![9])).encode_to_vec(),
            [4, 0, 1, 0, 0, 0, 9]
        );
        assert_eq!(
            DbOpened { db: 1, version: 2 }.encode_to_vec(),
            [1, 0, 0, 0, 2, 0, 0, 0]
        );
        assert_eq!(
            DbError::Constraint {
                kind: DbConstraint::NotNull,
                message: String::new()
            }
            .encode_to_vec(),
            [1, 0, 1, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            DbError::Migration {
                version: 3,
                message: String::new()
            }
            .encode_to_vec(),
            [6, 0, 3, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn errors_round_trip_and_display() {
        for error in [
            DbError::Busy,
            DbError::Constraint {
                kind: DbConstraint::Unique,
                message: "UNIQUE constraint failed: t.id".into(),
            },
            DbError::Corrupt("file is not a database".into()),
            DbError::Full,
            DbError::Unavailable("closed".into()),
            DbError::Sql {
                message: "no such table: x".into(),
            },
            DbError::Migration {
                version: 2,
                message: "boom".into(),
            },
        ] {
            assert_eq!(DbError::decode_exact(&error.encode_to_vec()), Ok(error));
        }
        assert_eq!(DbError::Busy.to_string(), "the database is busy");
        assert_eq!(
            DbError::Migration {
                version: 4,
                message: "x".into()
            }
            .to_string(),
            "migration 4 failed: x"
        );
    }

    #[test]
    fn port_errors_map_onto_db_errors() {
        assert!(matches!(
            DbError::from(PortError::Unavailable),
            DbError::Unavailable(text) if text.contains("E0062") && text.contains("Db port")
        ));
        assert_eq!(
            DbError::from(PortError::Failed(DbError::Full.encode_to_vec())),
            DbError::Full
        );
        assert!(matches!(
            DbError::from(PortError::Failed(vec![99])),
            DbError::Sql { .. }
        ));
    }

    #[test]
    fn names_and_migrations_are_checked() {
        for good in ["app", "a.b_c-1", MEMORY, &"x".repeat(64)] {
            assert_eq!(validate_name(good), Ok(()), "{good}");
        }
        for bad in ["", ".hidden", "a/b", "a b", "..", &"x".repeat(65), "é"] {
            assert!(
                matches!(validate_name(bad), Err(DbError::Unavailable(_))),
                "{bad}"
            );
        }
        let m = |version| DbMigration {
            version,
            sql: String::new(),
        };
        assert_eq!(validate_migrations(&[]), Ok(()));
        assert_eq!(validate_migrations(&[m(1), m(2), m(5)]), Ok(()));
        assert!(matches!(
            validate_migrations(&[m(0)]),
            Err(DbError::Migration { version: 0, .. })
        ));
        assert!(matches!(
            validate_migrations(&[m(2), m(2)]),
            Err(DbError::Migration { version: 2, .. })
        ));
    }

    #[test]
    fn typed_cells() {
        let rows = DbRows {
            columns: vec!["id".into(), "title".into(), "done".into(), "note".into()],
            rows: vec![vec![
                DbValue::Integer(7),
                DbValue::Text("milk".into()),
                DbValue::Integer(1),
                DbValue::Null,
            ]],
        };
        let row = rows.row(0).unwrap();
        assert_eq!(row.get::<i64>("id"), Ok(7));
        assert_eq!(row.get::<u32>(0), Ok(7));
        assert_eq!(row.get::<String>("title"), Ok("milk".into()));
        assert_eq!(row.get::<bool>("done"), Ok(true));
        assert_eq!(row.get::<Option<String>>("note"), Ok(None));
        assert_eq!(row.get::<f64>("id"), Ok(7.0));
        assert!(
            matches!(row.get::<String>("id"), Err(DbError::Sql { message }) if message.contains("integer, expected text"))
        );
        assert!(matches!(
            row.get::<i64>("missing"),
            Err(DbError::Sql { .. })
        ));
        assert!(matches!(row.get::<i64>(9), Err(DbError::Sql { .. })));
        assert!(matches!(row.get::<u8>("title"), Err(DbError::Sql { .. })));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows.iter().count(), 1);
        assert!(DbRows::default().is_empty());
    }
}
