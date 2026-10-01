//! [`FakeDb`]: a scripted `Db` port (ADR-048 §8): replies chosen by SQL, failure injection, every
//! call recorded, migrations and transactions tracked the way the adapters run them.

use core::fmt;
use std::collections::{BTreeMap, VecDeque};

use parking_lot::Mutex;

use crate::db::{
    Db, DbError, DbExecuted, DbMigration, DbOpened, DbRows, DbValue, validate_migrations,
    validate_name,
};

/// What a [`DbCall`] was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DbCallKind {
    /// `open`.
    Open,
    /// One migration `open` ran (`sql` is the migration's).
    Migrate,
    /// `execute`.
    Execute,
    /// `query`.
    Query,
    /// `begin`.
    Begin,
    /// `commit`.
    Commit,
    /// `rollback`.
    Rollback,
    /// `close`.
    Close,
}

/// One call the core made, recorded by [`FakeDb`].
#[derive(Clone, Debug, PartialEq)]
pub struct DbCall {
    /// Which method.
    pub kind: DbCallKind,
    /// The database or transaction id the call named (for `Open` and `Migrate`, the database
    /// that was opened, or 0 when the open failed).
    pub target: u32,
    /// The SQL (the name for `Open`, empty for the transaction calls).
    pub sql: String,
    /// The bound parameters.
    pub params: Vec<DbValue>,
    /// Whether the statement ran inside a transaction.
    pub in_transaction: bool,
}

enum Reply {
    Rows(DbRows),
    Executed(DbExecuted),
    Fail(DbError),
}

struct Rule {
    contains: String,
    reply: Reply,
}

struct Database {
    name: String,
    open: bool,
    tx: Option<u32>,
}

#[derive(Default)]
struct State {
    next_id: u32,
    dbs: BTreeMap<u32, Database>,
    txs: BTreeMap<u32, u32>,
    versions: BTreeMap<String, u32>,
    rules: Vec<Rule>,
    fail_next: VecDeque<DbError>,
    calls: Vec<DbCall>,
}

/// A scripted database: no SQL engine, so tests say what each statement answers.
///
/// * [`respond_rows`](FakeDb::respond_rows) / [`respond_executed`](FakeDb::respond_executed)
///   / [`fail_on`](FakeDb::fail_on) answer the first statement whose SQL contains a fragment
///   (the first matching rule wins); an unmatched `execute` answers `changes 0`, an unmatched
///   `query` no rows.
/// * [`fail_next`](FakeDb::fail_next) fails the next call of any kind, once.
/// * Migrations run as adapters run them: versions are checked, every one above the stored
///   version is recorded as a `Migrate` call, a rule that fails one turns into
///   `Migration { version, .. }` and leaves the version unchanged; versions survive a reopen of
///   the same name; a database newer than the newest migration is refused.
/// * Transactions: `begin` hands out an id from the same space as database ids; a statement on
///   the database id while a transaction runs fails `Busy` **at once** (an adapter would wait
///   up to its busy timeout first), as does a second `begin`.
#[derive(Default)]
pub struct FakeDb {
    state: Mutex<State>,
}

impl fmt::Debug for FakeDb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state.lock();
        f.debug_struct("FakeDb")
            .field("databases", &state.dbs.len())
            .field("rules", &state.rules.len())
            .field("calls", &state.calls.len())
            .finish()
    }
}

impl FakeDb {
    /// An empty fake.
    pub fn new() -> FakeDb {
        FakeDb::default()
    }

    /// Queries whose SQL contains `fragment` answer `rows`.
    pub fn respond_rows(&self, fragment: impl Into<String>, rows: DbRows) -> &FakeDb {
        self.rule(fragment, Reply::Rows(rows))
    }

    /// Statements whose SQL contains `fragment` answer `executed`.
    pub fn respond_executed(&self, fragment: impl Into<String>, executed: DbExecuted) -> &FakeDb {
        self.rule(fragment, Reply::Executed(executed))
    }

    /// Statements (and migrations) whose SQL contains `fragment` fail with `error`.
    pub fn fail_on(&self, fragment: impl Into<String>, error: DbError) -> &FakeDb {
        self.rule(fragment, Reply::Fail(error))
    }

    fn rule(&self, fragment: impl Into<String>, reply: Reply) -> &FakeDb {
        self.state.lock().rules.push(Rule {
            contains: fragment.into(),
            reply,
        });
        self
    }

    /// The next call, whatever it is, fails with `error` (queued: call twice for two).
    pub fn fail_next(&self, error: DbError) -> &FakeDb {
        self.state.lock().fail_next.push_back(error);
        self
    }

    /// Sets the stored version of database `name`, as if an earlier run had migrated it.
    pub fn set_version(&self, name: impl Into<String>, version: u32) {
        self.state.lock().versions.insert(name.into(), version);
    }

    /// The stored version of database `name` (0 if it was never opened).
    pub fn version(&self, name: &str) -> u32 {
        self.state.lock().versions.get(name).copied().unwrap_or(0)
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<DbCall> {
        self.state.lock().calls.clone()
    }

    /// The calls of one kind.
    pub fn calls_of(&self, kind: DbCallKind) -> Vec<DbCall> {
        self.state
            .lock()
            .calls
            .iter()
            .filter(|c| c.kind == kind)
            .cloned()
            .collect()
    }

    /// Whether a transaction is running on database `db`.
    pub fn in_transaction(&self, db: u32) -> bool {
        self.state
            .lock()
            .dbs
            .get(&db)
            .is_some_and(|d| d.tx.is_some())
    }

    /// Whether database `db` is open.
    pub fn is_open(&self, db: u32) -> bool {
        self.state.lock().dbs.get(&db).is_some_and(|d| d.open)
    }
}

impl State {
    fn record(&mut self, kind: DbCallKind, target: u32, sql: &str, params: &[DbValue], tx: bool) {
        self.calls.push(DbCall {
            kind,
            target,
            sql: sql.to_owned(),
            params: params.to_vec(),
            in_transaction: tx,
        });
    }

    fn reply_for(&self, sql: &str) -> Option<&Reply> {
        self.rules
            .iter()
            .find(|rule| sql.contains(rule.contains.as_str()))
            .map(|rule| &rule.reply)
    }

    /// Resolves a statement's target: `(database, in_transaction)`.
    fn target(&self, id: u32) -> Result<(u32, bool), DbError> {
        if let Some(&db) = self.txs.get(&id) {
            return Ok((db, true));
        }
        match self.dbs.get(&id) {
            Some(d) if d.open && d.tx.is_some() => Err(DbError::Busy),
            Some(d) if d.open => Ok((id, false)),
            _ => Err(DbError::Unavailable(format!(
                "no open database or transaction {id}"
            ))),
        }
    }

    fn end_tx(&mut self, tx: u32) -> Result<(), DbError> {
        let Some(db) = self.txs.remove(&tx) else {
            return Err(DbError::Unavailable(format!("transaction {tx} is over")));
        };
        if let Some(d) = self.dbs.get_mut(&db) {
            d.tx = None;
        }
        Ok(())
    }
}

#[undra_macros::port]
impl Db for FakeDb {
    async fn open(&self, name: String, migrations: Vec<DbMigration>) -> Result<DbOpened, DbError> {
        let mut state = self.state.lock();
        if let Some(error) = state.fail_next.pop_front() {
            state.record(DbCallKind::Open, 0, &name, &[], false);
            return Err(error);
        }
        validate_name(&name)?;
        validate_migrations(&migrations)?;
        let current = state.versions.get(&name).copied().unwrap_or(0);
        let newest = migrations.last().map_or(0, |m| m.version);
        if current > newest && !migrations.is_empty() {
            state.record(DbCallKind::Open, 0, &name, &[], false);
            return Err(DbError::Migration {
                version: current,
                message: format!(
                    "the database is at version {current}, newer than the newest migration ({newest})"
                ),
            });
        }
        state.next_id += 1;
        let db = state.next_id;
        state.record(DbCallKind::Open, db, &name, &[], false);
        let mut version = current;
        for migration in migrations.iter().filter(|m| m.version > current) {
            state.record(DbCallKind::Migrate, db, &migration.sql, &[], true);
            if let Some(Reply::Fail(error)) = state.reply_for(&migration.sql) {
                let message = error.to_string();
                return Err(DbError::Migration {
                    version: migration.version,
                    message,
                });
            }
            version = migration.version;
        }
        state.versions.insert(name.clone(), version);
        state.dbs.insert(
            db,
            Database {
                name,
                open: true,
                tx: None,
            },
        );
        Ok(DbOpened { db, version })
    }

    async fn execute(
        &self,
        db: u32,
        sql: String,
        params: Vec<DbValue>,
    ) -> Result<DbExecuted, DbError> {
        let mut state = self.state.lock();
        let in_tx = state.txs.contains_key(&db);
        state.record(DbCallKind::Execute, db, &sql, &params, in_tx);
        if let Some(error) = state.fail_next.pop_front() {
            return Err(error);
        }
        state.target(db)?;
        match state.reply_for(&sql) {
            Some(Reply::Fail(error)) => Err(error.clone()),
            Some(Reply::Executed(executed)) => Ok(*executed),
            Some(Reply::Rows(_)) | None => Ok(DbExecuted {
                changes: 0,
                last_insert_id: 0,
            }),
        }
    }

    async fn query(&self, db: u32, sql: String, params: Vec<DbValue>) -> Result<DbRows, DbError> {
        let mut state = self.state.lock();
        let in_tx = state.txs.contains_key(&db);
        state.record(DbCallKind::Query, db, &sql, &params, in_tx);
        if let Some(error) = state.fail_next.pop_front() {
            return Err(error);
        }
        state.target(db)?;
        match state.reply_for(&sql) {
            Some(Reply::Fail(error)) => Err(error.clone()),
            Some(Reply::Rows(rows)) => Ok(rows.clone()),
            Some(Reply::Executed(_)) | None => Ok(DbRows::default()),
        }
    }

    async fn begin(&self, db: u32) -> Result<u32, DbError> {
        let mut state = self.state.lock();
        state.record(DbCallKind::Begin, db, "", &[], false);
        if let Some(error) = state.fail_next.pop_front() {
            return Err(error);
        }
        let (db, in_tx) = state.target(db)?;
        if in_tx {
            return Err(DbError::Sql {
                message: "a transaction cannot begin inside a transaction".to_owned(),
            });
        }
        state.next_id += 1;
        let tx = state.next_id;
        state.txs.insert(tx, db);
        if let Some(d) = state.dbs.get_mut(&db) {
            d.tx = Some(tx);
        }
        Ok(tx)
    }

    async fn commit(&self, tx: u32) -> Result<(), DbError> {
        let mut state = self.state.lock();
        state.record(DbCallKind::Commit, tx, "", &[], true);
        if let Some(error) = state.fail_next.pop_front() {
            // A failed commit rolls back and ends the transaction, as the adapters do.
            let _ = state.end_tx(tx);
            return Err(error);
        }
        state.end_tx(tx)
    }

    async fn rollback(&self, tx: u32) -> Result<(), DbError> {
        let mut state = self.state.lock();
        state.record(DbCallKind::Rollback, tx, "", &[], true);
        if let Some(error) = state.fail_next.pop_front() {
            let _ = state.end_tx(tx);
            return Err(error);
        }
        state.end_tx(tx)
    }

    async fn close(&self, db: u32) -> Result<(), DbError> {
        let mut state = self.state.lock();
        state.record(DbCallKind::Close, db, "", &[], false);
        if let Some(error) = state.fail_next.pop_front() {
            return Err(error);
        }
        let Some(d) = state.dbs.get_mut(&db) else {
            return Err(DbError::Unavailable(format!("no database {db}")));
        };
        d.open = false;
        if let Some(tx) = d.tx.take() {
            state.txs.remove(&tx);
        }
        Ok(())
    }
}

impl fmt::Debug for Database {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Database")
            .field("name", &self.name)
            .field("open", &self.open)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbConstraint;
    use crate::fakes::testing::block_on;

    fn m(version: u32, sql: &str) -> DbMigration {
        DbMigration {
            version,
            sql: sql.into(),
        }
    }

    #[test]
    fn migrations_run_once_and_survive_a_reopen() {
        let db = FakeDb::new();
        let first =
            block_on(db.open("app".into(), vec![m(1, "CREATE a"), m(2, "CREATE b")])).unwrap();
        assert_eq!(first.version, 2);
        assert_eq!(db.calls_of(DbCallKind::Migrate).len(), 2);
        let again = block_on(db.open(
            "app".into(),
            vec![m(1, "CREATE a"), m(2, "CREATE b"), m(3, "CREATE c")],
        ))
        .unwrap();
        assert_eq!(again.version, 3);
        assert_eq!(
            db.calls_of(DbCallKind::Migrate).len(),
            3,
            "only migration 3 ran"
        );
        assert!(matches!(
            block_on(db.open("app".into(), vec![m(1, "CREATE a")])),
            Err(DbError::Migration { version: 3, .. })
        ));
    }

    #[test]
    fn a_failing_migration_leaves_the_version() {
        let db = FakeDb::new();
        db.fail_on(
            "BROKEN",
            DbError::Sql {
                message: "syntax".into(),
            },
        );
        assert!(matches!(
            block_on(db.open("x".into(), vec![m(1, "OK"), m(2, "BROKEN")])),
            Err(DbError::Migration { version: 2, .. })
        ));
        assert_eq!(db.version("x"), 0);
        assert!(matches!(
            block_on(db.open("bad/name".into(), vec![])),
            Err(DbError::Unavailable(_))
        ));
    }

    #[test]
    fn statements_transactions_and_failures() {
        let db = FakeDb::new();
        db.respond_rows(
            "SELECT",
            DbRows {
                columns: vec!["n".into()],
                rows: vec![vec![DbValue::Integer(1)]],
            },
        )
        .fail_on(
            "INSERT INTO dup",
            DbError::Constraint {
                kind: DbConstraint::Unique,
                message: "UNIQUE constraint failed: dup.id".into(),
            },
        );
        let id = block_on(db.open(":memory:".into(), vec![])).unwrap().db;
        assert_eq!(
            block_on(db.query(id, "SELECT 1".into(), vec![]))
                .unwrap()
                .len(),
            1
        );
        assert!(matches!(
            block_on(db.execute(id, "INSERT INTO dup VALUES (1)".into(), vec![])),
            Err(DbError::Constraint {
                kind: DbConstraint::Unique,
                ..
            })
        ));
        let tx = block_on(db.begin(id)).unwrap();
        assert!(db.in_transaction(id));
        assert_eq!(
            block_on(db.execute(id, "UPDATE t".into(), vec![])),
            Err(DbError::Busy)
        );
        assert_eq!(block_on(db.begin(id)), Err(DbError::Busy));
        block_on(db.execute(tx, "UPDATE t SET a = ?".into(), vec![DbValue::Integer(2)])).unwrap();
        block_on(db.commit(tx)).unwrap();
        assert!(matches!(
            block_on(db.commit(tx)),
            Err(DbError::Unavailable(_))
        ));
        let updates = db.calls_of(DbCallKind::Execute);
        assert!(updates.last().unwrap().in_transaction);
        assert_eq!(updates.last().unwrap().params, [DbValue::Integer(2)]);
        db.fail_next(DbError::Full);
        assert_eq!(
            block_on(db.execute(id, "x".into(), vec![])),
            Err(DbError::Full)
        );
        block_on(db.close(id)).unwrap();
        assert!(!db.is_open(id));
        assert!(matches!(
            block_on(db.query(id, "SELECT 1".into(), vec![])),
            Err(DbError::Unavailable(_))
        ));
    }
}
