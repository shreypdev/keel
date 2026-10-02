//! A list kept in SQLite through the opt-in `Db` port (ADR-048): [`Notes`], a store whose keyed
//! list mirrors a table, and three functions the contract runners (S25) use to prove each
//! platform's adapter: [`db_cells`] (every storage class there and back), [`db_run`] (one
//! statement, for typed SQL errors) and [`db_migrate`] (a migration that fails rolls back).
//!
//! The database belongs to the platform's adapter, which owns its thread; the core awaits port
//! replies and never blocks (ADR-048 §5). Values reach SQL only as bound parameters.

use std::sync::{Mutex, MutexGuard, PoisonError};

use undra::ports::db::{Database, DbError, DbValue, Migration};
use undra::ports::params;
use undra::prelude::*;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The notes database's schema, version by version.
pub const MIGRATIONS: &[Migration] = &[
    Migration::new(
        1,
        "CREATE TABLE notes (id INTEGER PRIMARY KEY, title TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0)",
    ),
    Migration::new(
        2,
        "CREATE INDEX notes_done ON notes (done); \
         CREATE TABLE tags (note INTEGER NOT NULL REFERENCES notes (id) ON DELETE CASCADE, tag TEXT NOT NULL)",
    ),
];

/// One note, a row of the `notes` table.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    /// The row id.
    pub id: i64,
    /// The text.
    pub title: String,
    /// Whether it is ticked.
    pub done: bool,
}

/// The notes of one database, mirrored: `notes` changes only after the database did.
#[undra::store(restore = "Self::assemble")]
pub struct Notes {
    /// Weak (ADR-034): the store is owned by the runtime.
    ctx: WeakCtx,
    db: Mutex<Option<Database>>,
    #[undra(key = "id")]
    notes: Signal<Vec<Note>>,
    version: Signal<u32>,
}

#[undra::api(store)]
impl Notes {
    /// No database open yet: call [`open`](Notes::open).
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(ctx, Signal::new(Vec::new()), Signal::new(0))
    }

    // A restored store has its notes and version and no open database: `open` again.
    fn assemble(ctx: Ctx, notes: Signal<Vec<Note>>, version: Signal<u32>) -> Self {
        Notes {
            ctx: ctx.downgrade(),
            db: Mutex::new(None),
            notes,
            version,
        }
    }

    /// Opens (creating it if needed) database `name`, migrates it and loads every note.
    /// Returns its version.
    pub async fn open(&self, name: String) -> Result<u32, DbError> {
        let Ok(ctx) = self.ctx.upgrade() else {
            return Err(DbError::Unavailable("the runtime is gone".to_owned()));
        };
        let db = Database::open(&ctx, &name, MIGRATIONS).await?;
        drop(ctx);
        let notes = load(&db).await?;
        let version = db.version();
        *lock(&self.db) = Some(db);
        undra::signals::txn(|| {
            self.notes.set(notes);
            self.version.set(version);
        });
        Ok(version)
    }

    /// Adds a note at the end.
    pub async fn add(&self, title: String) -> Result<Note, DbError> {
        let db = self.database()?;
        let row = db
            .execute("INSERT INTO notes (title) VALUES (?)", params![&title])
            .await?;
        let note = Note {
            id: row.last_insert_id,
            title,
            done: false,
        };
        self.notes.update(|list| list.push(note.clone()));
        Ok(note)
    }

    /// Adds a note with a chosen id (an id in use is `Constraint { kind: Unique }`).
    pub async fn add_with_id(&self, id: i64, title: String) -> Result<Note, DbError> {
        let db = self.database()?;
        db.execute(
            "INSERT INTO notes (id, title) VALUES (?, ?)",
            params![id, &title],
        )
        .await?;
        let note = Note {
            id,
            title,
            done: false,
        };
        self.notes.update(|list| {
            list.push(note.clone());
            list.sort_by_key(|n| n.id);
        });
        Ok(note)
    }

    /// Adds every title in one transaction: all or none. A `None` title breaks `NOT NULL` and
    /// rolls the whole batch back. Returns how many were added.
    pub async fn add_all(&self, titles: Vec<Option<String>>) -> Result<u32, DbError> {
        let db = self.database()?;
        let added = db
            .transaction(|tx| async move {
                let mut added = Vec::with_capacity(titles.len());
                for title in titles {
                    let row = tx
                        .execute(
                            "INSERT INTO notes (title) VALUES (?)",
                            params![title.clone()],
                        )
                        .await?;
                    added.push(Note {
                        id: row.last_insert_id,
                        title: title.unwrap_or_default(),
                        done: false,
                    });
                }
                Ok(added)
            })
            .await?;
        let count = added.len() as u32;
        self.notes.update(|list| list.extend(added));
        Ok(count)
    }

    /// Ticks or unticks note `id`.
    pub async fn toggle(&self, id: i64) -> Result<(), DbError> {
        let db = self.database()?;
        db.execute("UPDATE notes SET done = 1 - done WHERE id = ?", params![id])
            .await?;
        self.notes.update(|list| {
            if let Some(note) = list.iter_mut().find(|n| n.id == id) {
                note.done = !note.done;
            }
        });
        Ok(())
    }

    /// Removes note `id`.
    pub async fn remove(&self, id: i64) -> Result<(), DbError> {
        let db = self.database()?;
        db.execute("DELETE FROM notes WHERE id = ?", params![id])
            .await?;
        self.notes.update(|list| list.retain(|n| n.id != id));
        Ok(())
    }

    /// How many notes the database holds (read from it, not from the mirror).
    pub async fn count(&self) -> Result<u32, DbError> {
        let db = self.database()?;
        let rows = db
            .query("SELECT COUNT(*) AS n FROM notes", params![])
            .await?;
        rows.row(0).map_or(Ok(0), |row| row.get::<u32>("n"))
    }

    /// Closes the database; the list keeps what it showed (`close` is every store's own: it
    /// releases the handle).
    pub async fn close_database(&self) -> Result<(), DbError> {
        let db = lock(&self.db).take();
        match db {
            Some(db) => db.close().await,
            None => Ok(()),
        }
    }

    fn database(&self) -> Result<Database, DbError> {
        lock(&self.db)
            .clone()
            .ok_or_else(|| DbError::Unavailable("no database is open: call open first".to_owned()))
    }
}

async fn load(db: &Database) -> Result<Vec<Note>, DbError> {
    let rows = db
        .query("SELECT id, title, done FROM notes ORDER BY id", params![])
        .await?;
    rows.iter()
        .map(|row| {
            Ok(Note {
                id: row.get("id")?,
                title: row.get("title")?,
                done: row.get("done")?,
            })
        })
        .collect()
}

/// One value of each SQLite storage class, as [`db_cells`] read it back.
#[undra::api]
#[derive(Clone, Debug, PartialEq)]
pub struct DbCells {
    /// The `INTEGER`.
    pub int: i64,
    /// The `REAL`.
    pub real: f64,
    /// The `TEXT`.
    pub text: String,
    /// The `BLOB`.
    pub blob: Bytes,
    /// The column that held `NULL` (or the text given for it).
    pub none: Option<String>,
    /// SQLite's `typeof()` of the five columns, in order.
    pub types: Vec<String>,
}

/// Writes one value of each storage class to an in-memory database and reads them back.
#[undra::api]
pub async fn db_cells(
    ctx: &Ctx,
    int: i64,
    real: f64,
    text: String,
    blob: Bytes,
    none: Option<String>,
) -> Result<DbCells, DbError> {
    let db = Database::open(ctx, ":memory:", &[]).await?;
    db.execute(
        "CREATE TABLE cells (i INTEGER, r REAL, t TEXT, b BLOB, n TEXT)",
        params![],
    )
    .await?;
    db.execute(
        "INSERT INTO cells VALUES (?, ?, ?, ?, ?)",
        params![int, real, text, blob, none],
    )
    .await?;
    let rows = db
        .query(
            "SELECT i, r, t, b, n, typeof(i) AS ti, typeof(r) AS tr, typeof(t) AS tt, typeof(b) AS tb, typeof(n) AS tn FROM cells",
            params![],
        )
        .await?;
    let row = rows.row(0).ok_or_else(|| DbError::Sql {
        message: "the row did not come back".to_owned(),
    })?;
    let cells = DbCells {
        int: row.get("i")?,
        real: row.get("r")?,
        text: row.get("t")?,
        blob: row.get("b")?,
        none: row.get("n")?,
        types: ["ti", "tr", "tt", "tb", "tn"]
            .iter()
            .map(|c| row.get::<String>(*c))
            .collect::<Result<_, _>>()?,
    };
    db.close().await?;
    Ok(cells)
}

/// Runs one statement (SQL given at run time, no parameters) on database `name` and returns the
/// rows it changed: how a platform's adapter reports SQL and constraint errors.
#[undra::api]
pub async fn db_run(ctx: &Ctx, name: String, sql: String) -> Result<u64, DbError> {
    let db = Database::open(ctx, &name, &[]).await?;
    let outcome = db.execute_dynamic(sql, Vec::<DbValue>::new()).await;
    db.close().await?;
    outcome.map(|done| done.changes)
}

/// Opens database `name` with two migrations, the second broken when `broken`: the adapter runs
/// them in one transaction, so a broken second one leaves the database at the version it had.
/// Returns the version reached.
#[undra::api]
pub async fn db_migrate(ctx: &Ctx, name: String, broken: bool) -> Result<u32, DbError> {
    const GOOD: &[Migration] = &[
        Migration::new(1, "CREATE TABLE a (x INTEGER)"),
        Migration::new(2, "CREATE TABLE b (y INTEGER); INSERT INTO a VALUES (1)"),
    ];
    const BROKEN: &[Migration] = &[
        Migration::new(1, "CREATE TABLE a (x INTEGER)"),
        Migration::new(
            2,
            "CREATE TABLE b (y INTEGER); INSERT INTO nowhere VALUES (1)",
        ),
    ];
    let db = Database::open(ctx, &name, if broken { BROKEN } else { GOOD }).await?;
    let version = db.version();
    db.close().await?;
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use undra::ports::db::{DbExecuted, DbRows};
    use undra::ports::fakes::{self, DbCallKind};
    use undra::runtime::testing::TestRuntime;

    #[test]
    fn notes_mirror_the_database() {
        let t = TestRuntime::new();
        let fakes = fakes::install(&t);
        fakes.db.respond_executed(
            "INSERT",
            DbExecuted {
                changes: 1,
                last_insert_id: 5,
            },
        );
        fakes.db.respond_rows(
            "COUNT",
            DbRows {
                columns: vec!["n".into()],
                rows: vec![vec![DbValue::Integer(1)]],
            },
        );
        let notes = Arc::new(Notes::new(t.ctx()));
        let n = notes.clone();
        let (version, note, count) = t.run_until(async move {
            let version = n.open("notes".into()).await.unwrap();
            let note = n.add("milk".into()).await.unwrap();
            n.toggle(note.id).await.unwrap();
            (version, note, n.count().await.unwrap())
        });
        assert_eq!((version, note.id, count), (2, 5, 1));
        assert_eq!(fakes.db.calls_of(DbCallKind::Migrate).len(), 2);
        let update = &fakes.db.calls_of(DbCallKind::Execute)[1];
        assert_eq!(update.params, [DbValue::Integer(5)]);
    }

    #[test]
    fn notes_on_real_sqlite() {
        use undra::ports::fakes::MemDb;
        let t = TestRuntime::new();
        let _fakes = fakes::install(&t);
        MemDb::new().install(t.runtime());
        let notes = Arc::new(Notes::new(t.ctx()));
        let n = notes.clone();
        let outcome = t.run_until(async move {
            assert_eq!(n.open("notes".into()).await?, 2);
            let milk = n.add("milk".into()).await?;
            n.add("eggs".into()).await?;
            n.toggle(milk.id).await?;
            let dup = n.add_with_id(milk.id, "dup".into()).await;
            assert!(matches!(
                dup,
                Err(DbError::Constraint {
                    kind: undra::ports::db::DbConstraint::Unique,
                    ..
                })
            ));
            let rolled_back = n.add_all(vec![Some("a".into()), None]).await;
            assert!(matches!(rolled_back, Err(DbError::Constraint { .. })));
            assert_eq!(n.count().await?, 2);
            assert_eq!(
                n.add_all(vec![Some("a".into()), Some("b".into())]).await?,
                2
            );
            n.close_database().await?;
            assert_eq!(n.open("notes".into()).await?, 2);
            n.count().await
        });
        assert_eq!(outcome, Ok(4));
        let ctx = t.ctx();
        let (cells, migrate_broken, migrate_good, sql) = t.run_until(async move {
            let cells = db_cells(
                &ctx,
                -9_007_199_254_740_993,
                1.5,
                "é😀".into(),
                Bytes(vec![0, 255, 7]),
                None,
            )
            .await;
            let broken = db_migrate(&ctx, "m".into(), true).await;
            let good = db_migrate(&ctx, "m".into(), false).await;
            let sql = db_run(
                &ctx,
                ":memory:".into(),
                "INSERT INTO missing VALUES (1)".into(),
            )
            .await;
            (cells, broken, good, sql)
        });
        let cells = cells.unwrap();
        assert_eq!(cells.int, -9_007_199_254_740_993);
        assert_eq!(cells.types, ["integer", "real", "text", "blob", "null"]);
        assert!(matches!(
            migrate_broken,
            Err(DbError::Migration { version: 2, .. })
        ));
        assert_eq!(migrate_good, Ok(2));
        assert!(matches!(sql, Err(DbError::Sql { .. })));
    }

    #[test]
    fn a_failed_batch_changes_nothing() {
        let t = TestRuntime::new();
        let fakes = fakes::install(&t);
        let notes = Arc::new(Notes::new(t.ctx()));
        let n = notes.clone();
        t.run_until(async move { n.open("batch".into()).await.unwrap() });
        fakes.db.fail_next(DbError::Full);
        let n = notes.clone();
        let failed =
            t.run_until(async move { n.add_all(vec![Some("a".into()), Some("b".into())]).await });
        assert_eq!(failed, Err(DbError::Full));
        assert_eq!(
            fakes.db.calls().last().map(|c| c.kind),
            Some(DbCallKind::Begin)
        );
    }
}
