//! The opt-in ports (ADR-047, ADR-048) seen from the core: what a call through a port proxy costs
//! the core (argument encoding, the port table, the reply's decoding) with a Rust fake answering
//! in place of the platform.
//!
//! These are host rows of the **core side** of the path. SQLite's own time and the platform's
//! socket are the platform's, measured by each runtime (bench/RESULTS.md, "Opt-in ports").
//!
//! * `ports/ws_roundtrip`: one 64-byte text message sent and the echo received, through
//!   `WebSocketProxy` (`send`, then `receive` with the stream's credit of 16).
//! * `db/insert_1k`: one transaction of 1,000 bound `INSERT`s through `DbProxy` (`begin`, 1,000
//!   `execute` with three parameters, `commit`): what the core spends to write 1,000 rows.
//! * `db/query_10k`: one `query` whose reply is 10,000 rows of 3 cells (integer, text, real),
//!   decoded into `DbRows`: what the core spends to read 10,000 rows.
#![allow(missing_docs, dead_code)]

use std::hint::black_box;
use std::sync::Arc;

use undra::ports::db::{Db, DbExecuted, DbRows, DbValue};
use undra::ports::fakes::Fakes;
use undra::ports::ws::{WebSocket, WsMessage};
use undra::ports::{DbProxy, WebSocketProxy};
use undra::runtime::testing::TestRuntime;
use undra_bench::workload::{Workload, with_reset};

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

/// The workloads of the `db` group.
pub fn db() -> Vec<Workload> {
    vec![
        Workload::new("db/insert_1k", || {
            let (t, fakes) = rig();
            fakes.db.respond_executed(
                "INSERT",
                DbExecuted {
                    changes: 1,
                    last_insert_id: 1,
                },
            );
            let proxy = Arc::new(DbProxy::new(t.ctx()));
            let p = proxy.clone();
            let db = t
                .run_until(async move { p.open("bench".into(), vec![]).await })
                .expect("the fake opens")
                .db;
            let insert_1k = move |t: &TestRuntime, proxy: &Arc<DbProxy>| {
                let p = proxy.clone();
                t.run_until(async move {
                    let tx = p.begin(db).await?;
                    for i in 0..1_000_i64 {
                        p.execute(
                            tx,
                            "INSERT INTO items (id, title, score) VALUES (?, ?, ?)".to_owned(),
                            vec![
                                DbValue::Integer(i),
                                DbValue::Text(format!("item {i}")),
                                DbValue::Real(i as f64 / 2.0),
                            ],
                        )
                        .await?;
                    }
                    p.commit(tx).await
                })
            };
            insert_1k(&t, &proxy).expect("1,000 inserts commit");
            let executes = fakes
                .db
                .calls_of(undra::ports::fakes::DbCallKind::Execute)
                .len();
            assert_eq!(executes, 1_000, "every insert crossed");
            let server = fakes.db.clone();
            with_reset(
                move || {
                    black_box(insert_1k(&t, &proxy)).ok();
                },
                move || server.clear_calls(),
            )
        }),
        Workload::new("db/query_10k", || {
            let (t, fakes) = rig();
            let rows = DbRows {
                columns: vec!["id".into(), "title".into(), "score".into()],
                rows: (0..10_000_i64)
                    .map(|i| {
                        vec![
                            DbValue::Integer(i),
                            DbValue::Text(format!("item {i}")),
                            DbValue::Real(i as f64 / 2.0),
                        ]
                    })
                    .collect(),
            };
            fakes.db.respond_rows("SELECT", rows);
            let proxy = Arc::new(DbProxy::new(t.ctx()));
            let p = proxy.clone();
            let db = t
                .run_until(async move { p.open("bench".into(), vec![]).await })
                .expect("the fake opens")
                .db;
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
            let server = fakes.db.clone();
            with_reset(
                move || {
                    black_box(query(&t, &proxy)).ok();
                },
                move || server.clear_calls(),
            )
        }),
    ]
}
