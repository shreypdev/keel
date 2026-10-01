//! The opt-in ports (ADR-047, ADR-048) seen from the core: what a call through a port proxy costs
//! the core (argument encoding, the port table, the reply's decoding) with a Rust fake answering
//! in place of the platform.
//!
//! These are host rows: the core side of the path, and for `db/*` an in-memory SQLite in place of
//! the platform's file (whose disk time is the platform's, measured by each runtime).
//!
//! * `ports/ws_roundtrip`: one 64-byte text message sent and the echo received, through
//!   `WebSocketProxy` (`send`, then `receive` with the stream's credit of 16).
//! * `db/insert_1k`: one transaction of 1,000 bound `INSERT`s through `DbProxy` (`begin`, 1,000
//!   `execute` with three parameters, `commit`) into a real in-memory SQLite (`MemDb`, the
//!   reference adapter): the port path plus SQLite's own work, the table emptied between runs.
//! * `db/query_10k`: one `query` of 10,000 rows of 3 cells (integer, text, real) from the same
//!   SQLite, encoded and decoded into `DbRows`.
#![allow(missing_docs, dead_code)]

use std::hint::black_box;
use std::rc::Rc;
use std::sync::Arc;

use undra::ports::db::{Db, DbMigration, DbValue};
use undra::ports::fakes::{Fakes, MemDb};
use undra::ports::ws::{WebSocket, WsMessage};
use undra::ports::{DbProxy, WebSocketProxy};
use undra::runtime::testing::TestRuntime;
use undra_bench::workload::{Workload, plain, with_reset};

fn rig() -> (TestRuntime, Fakes) {
    let t = TestRuntime::new();
    let fakes = undra::ports::fakes::install(&t);
    (t, fakes)
}

/// The workloads of the `ports` group.
pub fn ports() -> Vec<Workload> {
    vec![Workload::new("ports/ws_roundtrip", || {
        let (t, fakes) = rig();
        fakes.web_socket.echo(true);
        let proxy = Arc::new(WebSocketProxy::new(t.ctx()));
        let p = proxy.clone();
        let conn = t
            .run_until(async move { p.connect("ws://bench.test/".into(), vec![], vec![]).await })
            .expect("the fake accepts")
            .conn;
        let message = WsMessage::Text("x".repeat(64));
        let p = proxy.clone();
        let m = message.clone();
        let echoed = t.run_until(async move {
            p.send(conn, m).await?;
            p.receive(conn, 16).await
        });
        assert_eq!(echoed, Ok(vec![message.clone()]), "the echo comes back");
        let server = fakes.web_socket.clone();
        with_reset(
            move || {
                let p = proxy.clone();
                let m = message.clone();
                black_box(t.run_until(async move {
                    p.send(conn, m).await?;
                    p.receive(conn, 16).await
                }))
                .ok();
            },
            move || server.clear_history(),
        )
    })]
}

/// A runtime whose `Db` port is a real in-memory SQLite ([`MemDb`]) with an `items` table.
fn sqlite_rig() -> (TestRuntime, Arc<DbProxy>, u32) {
    let t = TestRuntime::new();
    let _fakes = undra::ports::fakes::install(&t);
    MemDb::new().install(t.runtime());
    let proxy = Arc::new(DbProxy::new(t.ctx()));
    let p = proxy.clone();
    let db = t
        .run_until(async move {
            p.open(
                "bench".into(),
                vec![DbMigration {
                    version: 1,
                    sql: "CREATE TABLE items (id INTEGER PRIMARY KEY, title TEXT NOT NULL, score REAL)"
                        .into(),
                }],
            )
            .await
        })
        .expect("the database opens")
        .db;
    (t, proxy, db)
}

fn item(i: i64) -> Vec<DbValue> {
    vec![
        DbValue::Integer(i),
        DbValue::Text(format!("item {i}")),
        DbValue::Real(i as f64 / 2.0),
    ]
}

/// The workloads of the `db` group: the port path and a real in-memory SQLite.
pub fn db() -> Vec<Workload> {
    vec![
        Workload::new("db/insert_1k", || {
            let (t, proxy, db) = sqlite_rig();
            let insert_1k = move |t: &TestRuntime, proxy: &Arc<DbProxy>| {
                let p = proxy.clone();
                t.run_until(async move {
                    let tx = p.begin(db).await?;
                    for i in 0..1_000_i64 {
                        p.execute(
                            tx,
                            "INSERT INTO items (id, title, score) VALUES (?, ?, ?)".to_owned(),
                            item(i),
                        )
                        .await?;
                    }
                    p.commit(tx).await
                })
            };
            let clear = move |t: &TestRuntime, proxy: &Arc<DbProxy>| {
                let p = proxy.clone();
                t.run_until(
                    async move { p.execute(db, "DELETE FROM items".to_owned(), vec![]).await },
                )
            };
            insert_1k(&t, &proxy).expect("1,000 inserts commit");
            let count = {
                let p = proxy.clone();
                t.run_until(async move {
                    p.query(db, "SELECT COUNT(*) FROM items".to_owned(), vec![])
                        .await
                })
                .expect("count")
            };
            assert_eq!(
                count.rows,
                [[DbValue::Integer(1_000)]],
                "every insert landed"
            );
            let t = Rc::new(t);
            let (t2, proxy2) = (t.clone(), proxy.clone());
            with_reset(
                move || {
                    black_box(insert_1k(&t, &proxy)).ok();
                },
                move || {
                    clear(&t2, &proxy2).expect("the table empties");
                },
            )
        }),
        Workload::new("db/query_10k", || {
            let (t, proxy, db) = sqlite_rig();
            let p = proxy.clone();
            t.run_until(async move {
                let tx = p.begin(db).await?;
                for i in 0..10_000_i64 {
                    p.execute(
                        tx,
                        "INSERT INTO items (id, title, score) VALUES (?, ?, ?)".to_owned(),
                        item(i),
                    )
                    .await?;
                }
                p.commit(tx).await
            })
            .expect("10,000 rows seeded");
            let query = move |t: &TestRuntime, proxy: &Arc<DbProxy>| {
                let p = proxy.clone();
                t.run_until(async move {
                    p.query(db, "SELECT id, title, score FROM items".to_owned(), vec![])
                        .await
                })
            };
            let first = query(&t, &proxy).expect("the query answers");
            assert_eq!(first.len(), 10_000, "10,000 rows come back");
            assert_eq!(first.columns.len(), 3, "of 3 cells");
            assert_eq!(first.rows[9_999], item(9_999));
            plain(move || {
                black_box(query(&t, &proxy)).ok();
            })
        }),
    ]
}
