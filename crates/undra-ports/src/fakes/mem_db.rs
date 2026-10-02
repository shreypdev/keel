//! [`MemDb`]: the `Db` port over a real in-memory SQLite (feature `db-fake`, tests only; ADR-048 §8).

use core::fmt;
use std::collections::BTreeMap;
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, ErrorCode, ffi};
use undra_runtime::{Port, Runtime};
use undra_wire::Bytes;

use crate::db::{
    Db, DbConstraint, DbError, DbExecuted, DbMigration, DbOpened, DbRows, DbValue, MEMORY,
    validate_migrations, validate_name,
};

/// One named database: its connection and whether a transaction runs on it.
struct Database {
    conn: Connection,
    tx: Option<u32>,
}

enum Handle {
    /// A database id: the name it was opened under (`None` once closed).
    Db(Option<String>),
    /// A transaction id: the database name it runs on.
    Tx(String),
}

#[derive(Default)]
struct State {
    next_id: u32,
    next_memory: u32,
    databases: BTreeMap<String, Database>,
    handles: BTreeMap<u32, Handle>,
}

/// The `Db` port over a real in-memory SQLite: the reference adapter. It runs the migrations,
/// transactions, error mapping and one-statement rule exactly as ADR-048 says the platform
/// adapters do, on the calling thread (deterministic, no I/O).
///
/// A database opened under a name lives as long as the `MemDb`: closing and reopening it finds
/// its rows and version (as a file would). `":memory:"` opens a fresh private database each time.
/// One difference from a platform adapter, which waits up to its busy timeout: a statement on the
/// database id while a transaction runs fails `Busy` at once (a fake has no clock to wait on).
///
/// ```
/// use undra_ports::db::{Database, Migration};
/// use undra_ports::fakes::MemDb;
/// use undra_ports::params;
/// use undra_runtime::testing::TestRuntime;
///
/// let t = TestRuntime::new();
/// MemDb::new().install(t.runtime());
/// let ctx = t.ctx();
/// let title: String = t.run_until(async move {
///     let db = Database::open(&ctx, "app", &[Migration::new(1, "CREATE TABLE t (title TEXT NOT NULL)")]).await.unwrap();
///     db.execute("INSERT INTO t VALUES (?)", params!["milk"]).await.unwrap();
///     let rows = db.query("SELECT title FROM t", params![]).await.unwrap();
///     rows.row(0).unwrap().get("title").unwrap()
/// });
/// assert_eq!(title, "milk");
/// ```
#[derive(Default)]
pub struct MemDb {
    state: Mutex<State>,
}

impl fmt::Debug for MemDb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state.lock();
        f.debug_struct("MemDb")
            .field("databases", &state.databases.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl MemDb {
    /// No database yet.
    pub fn new() -> Arc<MemDb> {
        Arc::new(MemDb::default())
    }

    /// Binds this fake as `rt`'s `Db` port (in place of `FakeDb` when [`fakes::install`] ran).
    ///
    /// [`fakes::install`]: crate::fakes::install
    pub fn install(self: &Arc<Self>, rt: &Arc<Runtime>) {
        rt.bind_dyn_port_with::<dyn Db>(
            <dyn Db as Port>::PORT_ID,
            self.clone(),
            &crate::DB_DISPATCHER,
        );
    }

    /// The names of the databases opened so far.
    pub fn names(&self) -> Vec<String> {
        self.state.lock().databases.keys().cloned().collect()
    }
}

/// SQLite's failure as a `DbError`, by its extended result code (ADR-048).
fn map_error(error: rusqlite::Error) -> DbError {
    match error {
        rusqlite::Error::SqliteFailure(failure, message) => {
            let message = message.unwrap_or_else(|| failure.to_string());
            match failure.code {
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => DbError::Busy,
                ErrorCode::ConstraintViolation => DbError::Constraint {
                    kind: match failure.extended_code {
                        ffi::SQLITE_CONSTRAINT_UNIQUE | ffi::SQLITE_CONSTRAINT_PRIMARYKEY => {
                            DbConstraint::Unique
                        }
                        ffi::SQLITE_CONSTRAINT_NOTNULL => DbConstraint::NotNull,
                        ffi::SQLITE_CONSTRAINT_FOREIGNKEY => DbConstraint::ForeignKey,
                        ffi::SQLITE_CONSTRAINT_CHECK => DbConstraint::Check,
                        _ => DbConstraint::Other,
                    },
                    message,
                },
                ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => DbError::Corrupt(message),
                ErrorCode::DiskFull => DbError::Full,
                ErrorCode::CannotOpen
                | ErrorCode::PermissionDenied
                | ErrorCode::ReadOnly
                | ErrorCode::SystemIoFailure => DbError::Unavailable(message),
                _ => DbError::Sql { message },
            }
        }
        rusqlite::Error::MultipleStatement => DbError::Sql {
            message: "only one statement per call: use a migration for several".to_owned(),
        },
        rusqlite::Error::InvalidParameterCount(given, expected) => DbError::Sql {
            message: format!("the statement has {expected} parameters, {given} were given"),
        },
        other => DbError::Sql {
            message: other.to_string(),
        },
    }
}

fn to_value(value: &DbValue) -> Value {
    match value {
        DbValue::Null => Value::Null,
        DbValue::Integer(v) => Value::Integer(*v),
        DbValue::Real(v) => Value::Real(*v),
        DbValue::Text(v) => Value::Text(v.clone()),
        DbValue::Blob(v) => Value::Blob(v.0.clone()),
    }
}

fn from_ref(value: ValueRef<'_>) -> DbValue {
    match value {
        ValueRef::Null => DbValue::Null,
        ValueRef::Integer(v) => DbValue::Integer(v),
        ValueRef::Real(v) => DbValue::Real(v),
        ValueRef::Text(v) => DbValue::Text(String::from_utf8_lossy(v).into_owned()),
        ValueRef::Blob(v) => DbValue::Blob(Bytes(v.to_vec())),
    }
}

fn prepare<'c>(
    conn: &'c Connection,
    sql: &str,
    params: &[DbValue],
) -> Result<rusqlite::Statement<'c>, DbError> {
    let mut statement = conn.prepare(sql).map_err(map_error)?;
    let expected = statement.parameter_count();
    if expected != params.len() {
        return Err(DbError::Sql {
            message: format!(
                "the statement has {expected} parameters, {} were given",
                params.len()
            ),
        });
    }
    for (i, value) in params.iter().enumerate() {
        statement
            .raw_bind_parameter(i + 1, to_value(value))
            .map_err(map_error)?;
    }
    Ok(statement)
}

fn execute(conn: &Connection, sql: &str, params: &[DbValue]) -> Result<DbExecuted, DbError> {
    let mut statement = prepare(conn, sql, params)?;
    let mut rows = statement.raw_query();
    while rows.next().map_err(map_error)?.is_some() {}
    drop(rows);
    Ok(DbExecuted {
        changes: conn.changes(),
        last_insert_id: conn.last_insert_rowid(),
    })
}

fn query(conn: &Connection, sql: &str, params: &[DbValue]) -> Result<DbRows, DbError> {
    let mut statement = prepare(conn, sql, params)?;
    let columns: Vec<String> = statement
        .column_names()
        .iter()
        .map(|c| (*c).to_owned())
        .collect();
    let width = columns.len();
    let mut out = Vec::new();
    let mut rows = statement.raw_query();
    while let Some(row) = rows.next().map_err(map_error)? {
        let mut cells = Vec::with_capacity(width);
        for i in 0..width {
            cells.push(from_ref(row.get_ref(i).map_err(map_error)?));
        }
        out.push(cells);
    }
    Ok(DbRows { columns, rows: out })
}

impl State {
    /// The database a statement on `id` runs on, and whether `id` is a transaction.
    fn target(&mut self, id: u32) -> Result<(&mut Database, bool), DbError> {
        let (name, in_tx) = match self.handles.get(&id) {
            Some(Handle::Tx(name)) => (name.clone(), true),
            Some(Handle::Db(Some(name))) => (name.clone(), false),
            _ => {
                return Err(DbError::Unavailable(format!(
                    "no open database or transaction {id}"
                )));
            }
        };
        let db = self
            .databases
            .get_mut(&name)
            .ok_or_else(|| DbError::Unavailable(format!("no database {name}")))?;
        if !in_tx && db.tx.is_some() {
            return Err(DbError::Busy);
        }
        Ok((db, in_tx))
    }

    fn end(&mut self, tx: u32, sql: &str) -> Result<(), DbError> {
        let Some(Handle::Tx(name)) = self.handles.remove(&tx) else {
            return Err(DbError::Unavailable(format!("transaction {tx} is over")));
        };
        let db = self
            .databases
            .get_mut(&name)
            .ok_or_else(|| DbError::Unavailable(format!("no database {name}")))?;
        db.tx = None;
        match db.conn.execute_batch(sql) {
            Ok(()) => Ok(()),
            Err(error) => {
                // A failed COMMIT rolls back and ends the transaction, as the adapters do.
                let _ = db.conn.execute_batch("ROLLBACK");
                Err(map_error(error))
            }
        }
    }
}

#[undra_macros::port]
impl Db for MemDb {
    async fn open(&self, name: String, migrations: Vec<DbMigration>) -> Result<DbOpened, DbError> {
        validate_name(&name)?;
        validate_migrations(&migrations)?;
        let mut state = self.state.lock();
        let key = if name == MEMORY {
            state.next_memory += 1;
            format!(":memory:#{}", state.next_memory)
        } else {
            name
        };
        if !state.databases.contains_key(&key) {
            let conn = Connection::open_in_memory().map_err(map_error)?;
            conn.execute_batch("PRAGMA foreign_keys = ON")
                .map_err(map_error)?;
            state
                .databases
                .insert(key.clone(), Database { conn, tx: None });
        }
        let db = state.databases.get_mut(&key).expect("inserted above");
        if db.tx.is_some() {
            return Err(DbError::Busy);
        }
        let current: u32 = db
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(map_error)?;
        let newest = migrations.last().map_or(0, |m| m.version);
        if !migrations.is_empty() && current > newest {
            return Err(DbError::Migration {
                version: current,
                message: format!(
                    "the database is at version {current}, newer than the newest migration ({newest})"
                ),
            });
        }
        let pending: Vec<&DbMigration> =
            migrations.iter().filter(|m| m.version > current).collect();
        let mut version = current;
        if let Some(last) = pending.last() {
            db.conn
                .execute_batch("BEGIN IMMEDIATE")
                .map_err(map_error)?;
            for migration in &pending {
                if let Err(error) = db.conn.execute_batch(&migration.sql) {
                    let _ = db.conn.execute_batch("ROLLBACK");
                    return Err(DbError::Migration {
                        version: migration.version,
                        message: map_error(error).to_string(),
                    });
                }
            }
            let finish = format!("PRAGMA user_version = {}; COMMIT", last.version);
            if let Err(error) = db.conn.execute_batch(&finish) {
                let _ = db.conn.execute_batch("ROLLBACK");
                return Err(map_error(error));
            }
            version = last.version;
        }
        state.next_id += 1;
        let id = state.next_id;
        state.handles.insert(id, Handle::Db(Some(key)));
        Ok(DbOpened { db: id, version })
    }

    async fn execute(
        &self,
        db: u32,
        sql: String,
        params: Vec<DbValue>,
    ) -> Result<DbExecuted, DbError> {
        let mut state = self.state.lock();
        let (database, _) = state.target(db)?;
        execute(&database.conn, &sql, &params)
    }

    async fn query(&self, db: u32, sql: String, params: Vec<DbValue>) -> Result<DbRows, DbError> {
        let mut state = self.state.lock();
        let (database, _) = state.target(db)?;
        query(&database.conn, &sql, &params)
    }

    async fn begin(&self, db: u32) -> Result<u32, DbError> {
        let mut state = self.state.lock();
        let name = match state.handles.get(&db) {
            Some(Handle::Db(Some(name))) => name.clone(),
            Some(Handle::Tx(_)) => {
                return Err(DbError::Sql {
                    message: "a transaction cannot begin inside a transaction".to_owned(),
                });
            }
            _ => {
                return Err(DbError::Unavailable(format!(
                    "no open database or transaction {db}"
                )));
            }
        };
        state.next_id += 1;
        let tx = state.next_id;
        let database = state
            .databases
            .get_mut(&name)
            .ok_or_else(|| DbError::Unavailable(format!("no database {name}")))?;
        if database.tx.is_some() {
            return Err(DbError::Busy);
        }
        database
            .conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(map_error)?;
        database.tx = Some(tx);
        state.handles.insert(tx, Handle::Tx(name));
        Ok(tx)
    }

    async fn commit(&self, tx: u32) -> Result<(), DbError> {
        self.state.lock().end(tx, "COMMIT")
    }

    async fn rollback(&self, tx: u32) -> Result<(), DbError> {
        self.state.lock().end(tx, "ROLLBACK")
    }

    async fn close(&self, db: u32) -> Result<(), DbError> {
        let mut state = self.state.lock();
        match state.handles.get(&db) {
            Some(Handle::Db(Some(name))) => {
                let name = name.clone();
                let running = state.databases.get(&name).and_then(|d| d.tx);
                if let Some(tx) = running {
                    let _ = state.end(tx, "ROLLBACK");
                }
                if name.starts_with(":memory:#") {
                    state.databases.remove(&name);
                }
                state.handles.insert(db, Handle::Db(None));
                Ok(())
            }
            Some(Handle::Db(None)) => Ok(()),
            _ => Err(DbError::Unavailable(format!("no database {db}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::testing::block_on;

    fn m(version: u32, sql: &str) -> DbMigration {
        DbMigration {
            version,
            sql: sql.into(),
        }
    }

    #[test]
    fn typed_cells_and_constraints() {
        let db = MemDb::default();
        let id = block_on(db.open(
            "t".into(),
            vec![m(
                1,
                "CREATE TABLE c (k INTEGER PRIMARY KEY, i INTEGER, r REAL, t TEXT NOT NULL, b BLOB, n TEXT, CHECK (i <> 13))",
            )],
        ))
        .unwrap()
        .db;
        let row = vec![
            DbValue::Integer(1),
            DbValue::Integer(i64::MIN),
            DbValue::Real(1.5),
            DbValue::Text("é😀".into()),
            DbValue::Blob(Bytes(vec![0, 255])),
            DbValue::Null,
        ];
        let done = block_on(db.execute(
            id,
            "INSERT INTO c VALUES (?, ?, ?, ?, ?, ?)".into(),
            row.clone(),
        ))
        .unwrap();
        assert_eq!((done.changes, done.last_insert_id), (1, 1));
        let rows = block_on(db.query(id, "SELECT * FROM c".into(), vec![])).unwrap();
        assert_eq!(rows.columns, ["k", "i", "r", "t", "b", "n"]);
        assert_eq!(rows.rows, std::slice::from_ref(&row));
        let dup = block_on(db.execute(id, "INSERT INTO c VALUES (?, ?, ?, ?, ?, ?)".into(), row));
        assert!(matches!(
            dup,
            Err(DbError::Constraint {
                kind: DbConstraint::Unique,
                ..
            })
        ));
        let null = block_on(db.execute(id, "INSERT INTO c (t) VALUES (NULL)".into(), vec![]));
        assert!(matches!(
            null,
            Err(DbError::Constraint {
                kind: DbConstraint::NotNull,
                ..
            })
        ));
        let check =
            block_on(db.execute(id, "INSERT INTO c (i, t) VALUES (13, 'x')".into(), vec![]));
        assert!(matches!(
            check,
            Err(DbError::Constraint {
                kind: DbConstraint::Check,
                ..
            })
        ));
        assert!(matches!(
            block_on(db.execute(id, "SELECT 1; SELECT 2".into(), vec![])),
            Err(DbError::Sql { message }) if message.contains("one statement")
        ));
        assert!(matches!(
            block_on(db.execute(id, "SELECT ?".into(), vec![])),
            Err(DbError::Sql { message }) if message.contains("1 parameters, 0 were given")
        ));
        assert!(matches!(
            block_on(db.query(id, "SELEC 1".into(), vec![])),
            Err(DbError::Sql { .. })
        ));
    }

    #[test]
    fn migrations_are_one_transaction_and_survive_a_reopen() {
        let db = MemDb::default();
        let broken = vec![
            m(1, "CREATE TABLE a (x)"),
            m(2, "INSERT INTO nowhere VALUES (1)"),
        ];
        assert!(matches!(
            block_on(db.open("m".into(), broken)),
            Err(DbError::Migration { version: 2, .. })
        ));
        let good = vec![
            m(1, "CREATE TABLE a (x)"),
            m(2, "INSERT INTO a VALUES (1); INSERT INTO a VALUES (2)"),
        ];
        let opened = block_on(db.open("m".into(), good.clone())).unwrap();
        assert_eq!(opened.version, 2);
        block_on(db.close(opened.db)).unwrap();
        block_on(db.close(opened.db)).unwrap();
        let again = block_on(db.open("m".into(), good)).unwrap();
        let rows = block_on(db.query(again.db, "SELECT COUNT(*) FROM a".into(), vec![])).unwrap();
        assert_eq!(rows.rows, [[DbValue::Integer(2)]]);
        assert!(matches!(
            block_on(db.open("m".into(), vec![m(1, "CREATE TABLE a (x)")])),
            Err(DbError::Migration { version: 2, .. })
        ));
        assert!(matches!(
            block_on(db.query(opened.db, "SELECT 1".into(), vec![])),
            Err(DbError::Unavailable(_))
        ));
    }

    #[test]
    fn transactions_commit_roll_back_and_make_outer_statements_busy() {
        let db = MemDb::default();
        let id = block_on(db.open("x".into(), vec![m(1, "CREATE TABLE t (v INTEGER)")]))
            .unwrap()
            .db;
        let tx = block_on(db.begin(id)).unwrap();
        block_on(db.execute(tx, "INSERT INTO t VALUES (1)".into(), vec![])).unwrap();
        assert_eq!(
            block_on(db.execute(id, "INSERT INTO t VALUES (2)".into(), vec![])),
            Err(DbError::Busy)
        );
        block_on(db.rollback(tx)).unwrap();
        assert!(matches!(
            block_on(db.commit(tx)),
            Err(DbError::Unavailable(_))
        ));
        let tx = block_on(db.begin(id)).unwrap();
        block_on(db.execute(tx, "INSERT INTO t VALUES (3)".into(), vec![])).unwrap();
        block_on(db.commit(tx)).unwrap();
        let rows = block_on(db.query(id, "SELECT v FROM t".into(), vec![])).unwrap();
        assert_eq!(rows.rows, [[DbValue::Integer(3)]]);
        let a = block_on(db.open(MEMORY.into(), vec![])).unwrap().db;
        block_on(db.execute(a, "CREATE TABLE z (v)".into(), vec![])).unwrap();
        let b = block_on(db.open(MEMORY.into(), vec![])).unwrap().db;
        assert!(
            matches!(
                block_on(db.query(b, "SELECT * FROM z".into(), vec![])),
                Err(DbError::Sql { .. })
            ),
            ":memory: databases are private"
        );
    }
}
