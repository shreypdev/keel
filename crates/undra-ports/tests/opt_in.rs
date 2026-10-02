//! The opt-in ports (ADR-047 `WebSocket` and `Sse`, ADR-048 `Db`): their schema locked, their
//! Rust surface against the fakes inside a `TestRuntime` (through the accessor and through the
//! proxy, i.e. the whole encode/port-table/decode path), and properties of the pull discipline.
//!
//! Built only with all three features: `cargo test -p undra-ports --features websocket,sse,db`
//! (a workspace build has them, the playground turns them on).
#![cfg(all(feature = "websocket", feature = "sse", feature = "db"))]

use std::sync::{Arc, Mutex};

use proptest::prelude::*;
use undra_bindgen::Generator;
use undra_meta::{Schema, TypeRef, collect_schema, ids};
use undra_ports::db::{
    Database, DbConstraint, DbError, DbExecuted, DbMigration, DbOpened, DbRows, DbValue, Migration,
};
use undra_ports::fakes::{self, DbCallKind, Fakes};
use undra_ports::sse::{self, SseError, SseEvent};
use undra_ports::ws::{self, WsConnection, WsError, WsMessage, WsOpened, WsOptions};
use undra_ports::{Db, DbProxy, Header, Sse, SseProxy, WebSocket, WebSocketProxy, params};
use undra_runtime::testing::{TestRuntime, port_reply};
use undra_wire::payload::PortStatus;
use undra_wire::{Bytes, Decode, Encode};

/// `Schema::hash()` of the standard ports with all three opt-in ports.
const SCHEMA_HASH: u64 = 0x716f_c678_df98_087c;

const GOLDEN: &str = "tests/golden/schema-opt-in.json";

fn schema() -> Schema {
    let _ = undra_ports::HttpMethod::Get;
    collect_schema("undra-ports")
}

fn rig() -> (TestRuntime, Fakes) {
    let t = TestRuntime::new();
    let fakes = fakes::install(&t);
    (t, fakes)
}

// ---- schema --------------------------------------------------------------------------------------

#[test]
fn the_opt_in_schema_is_locked() {
    let schema = schema();
    schema
        .validate()
        .unwrap_or_else(|e| panic!("invalid schema: {e:#?}"));
    let json = schema.canonical_json();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN);
    if std::env::var_os("UNDRA_BLESS").is_some() {
        std::fs::write(&path, format!("{json}\n")).expect("write the golden schema");
    }
    let golden = std::fs::read_to_string(&path).expect("the opt-in golden schema is missing");
    assert_eq!(
        json,
        golden.trim_end(),
        "the opt-in ports' schema changed: the four runtimes hard-code it (ADR-047, ADR-048); \
         regenerate with UNDRA_BLESS=1 if it is intended"
    );
    assert_eq!(
        schema.hash(),
        SCHEMA_HASH,
        "schema hash changed (now {:#x}); update SCHEMA_HASH with the golden",
        schema.hash()
    );
}

#[test]
fn type_ids_are_hard_coded() {
    let schema = schema();
    for (name, id) in [
        ("WsOpened", 0x9364_0662),
        ("WsMessage", 0x9f2d_9b9e),
        ("WsError", 0xc4e7_cc8f),
        ("SseEvent", 0xa898_28ce),
        ("SseError", 0x2e78_01f4),
        ("DbMigration", 0x36b3_1925),
        ("DbOpened", 0xaf76_040e),
        ("DbValue", 0x48f7_4ac0),
        ("DbExecuted", 0x41a1_a3a6),
        ("DbRows", 0xffd1_2f2e),
        ("DbConstraint", 0x856f_0900),
        ("DbError", 0x1dfc_036b),
    ] {
        assert_eq!(ids::type_id(name), id, "{name}");
        let registered = schema
            .records
            .iter()
            .map(|r| (r.name.as_str(), r.type_id))
            .chain(schema.enums.iter().map(|e| (e.name.as_str(), e.type_id)))
            .find(|(n, _)| *n == name)
            .map(|(_, id)| id);
        assert_eq!(registered, Some(id), "{name} is registered");
    }
}

/// `(port, method, params, returns)`.
type Signature = (
    &'static str,
    &'static str,
    Vec<(&'static str, TypeRef)>,
    TypeRef,
);

#[test]
fn method_signatures_are_the_adrs() {
    let schema = schema();
    let s = TypeRef::String;
    let n = |name: &str| TypeRef::named(name);
    let r = |t: TypeRef, e: &str| TypeRef::Result(Box::new(t), Box::new(TypeRef::named(e)));
    let v = |t: TypeRef| TypeRef::Vec(Box::new(t));
    let o = |t: TypeRef| TypeRef::Option(Box::new(t));
    let expected: &[Signature] = &[
        (
            "WebSocket",
            "connect",
            vec![
                ("url", s.clone()),
                ("protocols", v(s.clone())),
                ("headers", v(n("Header"))),
            ],
            r(n("WsOpened"), "WsError"),
        ),
        (
            "WebSocket",
            "send",
            vec![("conn", TypeRef::U32), ("message", n("WsMessage"))],
            r(TypeRef::Unit, "WsError"),
        ),
        (
            "WebSocket",
            "receive",
            vec![("conn", TypeRef::U32), ("max", TypeRef::U32)],
            r(v(n("WsMessage")), "WsError"),
        ),
        (
            "WebSocket",
            "close",
            vec![
                ("conn", TypeRef::U32),
                ("code", TypeRef::U16),
                ("reason", s.clone()),
            ],
            r(TypeRef::Unit, "WsError"),
        ),
        (
            "Sse",
            "open",
            vec![
                ("url", s.clone()),
                ("headers", v(n("Header"))),
                ("last_event_id", o(s.clone())),
            ],
            r(TypeRef::U32, "SseError"),
        ),
        (
            "Sse",
            "next",
            vec![("stream", TypeRef::U32), ("max", TypeRef::U32)],
            r(v(n("SseEvent")), "SseError"),
        ),
        (
            "Sse",
            "close",
            vec![("stream", TypeRef::U32)],
            r(TypeRef::Unit, "SseError"),
        ),
        (
            "Db",
            "open",
            vec![("name", s.clone()), ("migrations", v(n("DbMigration")))],
            r(n("DbOpened"), "DbError"),
        ),
        (
            "Db",
            "execute",
            vec![
                ("db", TypeRef::U32),
                ("sql", s.clone()),
                ("params", v(n("DbValue"))),
            ],
            r(n("DbExecuted"), "DbError"),
        ),
        (
            "Db",
            "query",
            vec![
                ("db", TypeRef::U32),
                ("sql", s.clone()),
                ("params", v(n("DbValue"))),
            ],
            r(n("DbRows"), "DbError"),
        ),
        (
            "Db",
            "begin",
            vec![("db", TypeRef::U32)],
            r(TypeRef::U32, "DbError"),
        ),
        (
            "Db",
            "commit",
            vec![("tx", TypeRef::U32)],
            r(TypeRef::Unit, "DbError"),
        ),
        (
            "Db",
            "rollback",
            vec![("tx", TypeRef::U32)],
            r(TypeRef::Unit, "DbError"),
        ),
        (
            "Db",
            "close",
            vec![("db", TypeRef::U32)],
            r(TypeRef::Unit, "DbError"),
        ),
    ];
    for (port, method, params, returns) in expected {
        let def = schema
            .ports
            .iter()
            .find(|p| p.name == *port)
            .and_then(|p| p.methods.iter().find(|m| m.name == *method))
            .unwrap_or_else(|| panic!("{port}.{method} is not registered"));
        assert!(def.is_async, "{port}.{method} is async");
        let got: Vec<(String, TypeRef)> = def
            .params
            .iter()
            .map(|p| (p.name.clone(), p.ty.clone()))
            .collect();
        let want: Vec<(String, TypeRef)> = params
            .iter()
            .map(|(n, t)| ((*n).to_owned(), t.clone()))
            .collect();
        assert_eq!(got, want, "{port}.{method} params");
        assert_eq!(&def.returns, returns, "{port}.{method} returns");
    }
}

#[test]
fn bindgen_generates_the_opt_in_ports_in_three_languages() {
    let schema = schema();
    let mut generator = Generator::for_crate("undra-ports");
    generator.emit_standard_library = true;
    for (language, files) in [
        ("Swift", generator.swift(&schema).expect("Swift")),
        ("Kotlin", generator.kotlin(&schema).expect("Kotlin")),
        (
            "TypeScript",
            generator.typescript(&schema).expect("TypeScript"),
        ),
    ] {
        let all: String = files.iter().map(|f| f.contents.as_str()).collect();
        for needle in [
            "WebSocket",
            "WsMessage",
            "SseEvent",
            "DbValue",
            "DbError",
            "DbConstraint",
        ] {
            assert!(
                all.contains(needle),
                "{language}: generated code lacks {needle}"
            );
        }
    }
}

// ---- WebSocket -----------------------------------------------------------------------------------

/// Runs `consume` as a task and returns what it recorded, after the runtime went idle.
fn spawn_collect<F, Fut>(t: &TestRuntime, consume: F) -> Arc<Mutex<Vec<String>>>
where
    F: FnOnce(undra_runtime::Ctx, Arc<Mutex<Vec<String>>>) -> Fut,
    Fut: core::future::Future<Output = ()> + Send + 'static,
{
    let log = Arc::new(Mutex::new(Vec::new()));
    t.ctx().spawn(consume(t.ctx(), log.clone()));
    t.run_pending();
    log
}

fn describe(item: &Result<WsMessage, WsError>) -> String {
    match item {
        Ok(WsMessage::Text(text)) => text.clone(),
        Ok(WsMessage::Binary(bytes)) => format!("bin{:?}", bytes.0),
        Err(error) => format!("err:{error}"),
    }
}

#[test]
fn echo_through_the_connection() {
    let (t, fakes) = rig();
    fakes.web_socket.echo(true);
    let log = spawn_collect(&t, |ctx, log| async move {
        let conn = WsConnection::connect(
            &ctx,
            "wss://echo.test/",
            WsOptions::default().with_protocol("v1"),
        )
        .await
        .unwrap();
        log.lock().unwrap().push(conn.protocol().to_owned());
        conn.send_text("a").await.unwrap();
        conn.send_binary(vec![1, 2]).await.unwrap();
        let mut messages = conn.messages();
        for _ in 0..2 {
            let item = undra_ports::next(&mut messages).await.unwrap();
            log.lock().unwrap().push(describe(&item));
        }
        conn.close(1000, "done").await.unwrap();
        let end = format!("{:?}", undra_ports::next(&mut messages).await);
        log.lock().unwrap().push(end);
    });
    assert_eq!(*log.lock().unwrap(), ["v1", "a", "bin[1, 2]", "None"]);
    let conn = fakes.web_socket.last_conn().unwrap();
    assert_eq!(
        fakes.web_socket.connections()[0].closed_by_core,
        Some((1000, "done".into()))
    );
    assert_eq!(fakes.web_socket.sent(conn).len(), 2);
}

#[test]
fn a_reader_that_stops_stops_the_pulls() {
    let (t, fakes) = rig();
    let stop = Arc::new(Mutex::new(5usize));
    let held: Arc<Mutex<Option<ws::WsMessages>>> = Arc::new(Mutex::new(None));
    let (limit, keep) = (stop.clone(), held.clone());
    let log = spawn_collect(&t, |ctx, log| async move {
        let conn = WsConnection::connect(&ctx, "ws://flood.test", WsOptions::default())
            .await
            .unwrap();
        let mut messages = conn.messages();
        let n = *limit.lock().unwrap();
        for _ in 0..n {
            let item = undra_ports::next(&mut messages).await.unwrap();
            log.lock().unwrap().push(describe(&item));
        }
        *keep.lock().unwrap() = Some(messages);
    });
    // The server floods before the core reads anything.
    let conn = fakes.web_socket.last_conn().unwrap();
    assert!(
        fakes.web_socket.pulling(conn),
        "the stream pulled as soon as it was polled"
    );
    for i in 0..1000 {
        fakes.web_socket.push(conn, format!("{i}"));
    }
    t.run_pending();
    assert_eq!(log.lock().unwrap().len(), 5);
    let delivered = fakes.web_socket.delivered(conn);
    assert!(
        delivered <= 5 + u64::from(ws::CREDIT) + ws::LOW_WATER as u64,
        "a stalled reader was handed {delivered} messages"
    );
    assert_eq!(fakes.web_socket.waiting(conn) as u64, 1000 - delivered);
    assert!(
        fakes
            .web_socket
            .pulls(conn)
            .iter()
            .all(|&max| max == ws::CREDIT)
    );
    // Resume: everything arrives, in order.
    let mut messages = held.lock().unwrap().take().unwrap();
    fakes.web_socket.close_from_server(conn, 1000, "end");
    let rest = t.run_until(async move {
        let mut rest = Vec::new();
        while let Some(item) = undra_ports::next(&mut messages).await {
            rest.push(describe(&item));
        }
        rest
    });
    assert_eq!(rest.len(), 996);
    assert_eq!(rest[0], "5");
    assert_eq!(rest[994], "999");
    assert_eq!(rest[995], "err:the WebSocket was closed (1000): end");
}

#[test]
fn every_end_is_typed() {
    let (t, fakes) = rig();
    fakes.web_socket.refuse(
        "wss://auth.test",
        WsError::Refused {
            status: Some(401),
            message: "unauthorized".into(),
        },
    );
    let ctx = t.ctx();
    let refused = t.run_until(async move {
        WsConnection::connect(&ctx, "wss://auth.test/x", WsOptions::default())
            .await
            .map(|_| ())
    });
    assert_eq!(
        refused,
        Err(WsError::Refused {
            status: Some(401),
            message: "unauthorized".into()
        })
    );
    for (end, expected) in [
        ("drop", "err:WebSocket network error: reset"),
        ("protocol", "err:WebSocket protocol error: masked"),
        ("close", "err:the WebSocket was closed (4001): kicked"),
    ] {
        let ctx = t.ctx();
        let conn = t.run_until(async move {
            WsConnection::connect(&ctx, "ws://x.test", WsOptions::default())
                .await
                .unwrap()
        });
        match end {
            "drop" => fakes.web_socket.drop_connection(conn.id(), "reset"),
            "protocol" => fakes.web_socket.break_protocol(conn.id(), "masked"),
            _ => fakes
                .web_socket
                .close_from_server(conn.id(), 4001, "kicked"),
        }
        let items = t.run_until(async move {
            let mut messages = conn.messages();
            let mut items = Vec::new();
            while let Some(item) = undra_ports::next(&mut messages).await {
                items.push(describe(&item));
            }
            items
        });
        assert_eq!(items, [expected], "{end}");
    }
}

#[test]
fn a_dropped_connection_is_closed_going_away_and_a_second_stream_is_refused() {
    let (t, fakes) = rig();
    let ctx = t.ctx();
    let second = t.run_until(async move {
        let conn = WsConnection::connect(&ctx, "ws://x.test", WsOptions::default())
            .await
            .unwrap();
        let _first = conn.messages();
        let mut second = conn.messages();
        undra_ports::next(&mut second).await
    });
    assert!(matches!(second, Some(Err(WsError::Protocol(_)))));
    t.run_pending();
    assert_eq!(
        fakes.web_socket.connections()[0].closed_by_core,
        Some((ws::GOING_AWAY, String::new())),
        "dropping the connection and its streams closed it"
    );
}

#[test]
fn the_proxy_carries_every_method_through_the_port_table() {
    let (t, fakes) = rig();
    fakes.web_socket.echo(true);
    let proxy = WebSocketProxy::new(t.ctx());
    let outcome = t.run_until(async move {
        let opened = proxy
            .connect(
                "wss://p.test".into(),
                vec!["x".into()],
                vec![Header::new("k", "v")],
            )
            .await?;
        proxy.send(opened.conn, WsMessage::Text("é".into())).await?;
        let got = proxy.receive(opened.conn, 16).await?;
        proxy.close(opened.conn, 1000, String::new()).await?;
        let after = proxy.receive(opened.conn, 16).await?;
        Ok::<_, WsError>((opened, got, after))
    });
    let (opened, got, after) = outcome.unwrap();
    assert_eq!(
        opened,
        WsOpened {
            conn: 1,
            protocol: "x".into()
        }
    );
    assert_eq!(got, [WsMessage::Text("é".into())]);
    assert!(after.is_empty());
    assert_eq!(
        fakes.web_socket.connections()[0].headers,
        [Header::new("k", "v")]
    );
}

#[test]
fn without_an_adapter_every_method_is_a_typed_error() {
    let t = TestRuntime::new();
    let ctx = t.ctx();
    let (ws, sse, db) = t.run_until(async move {
        let ws = WsConnection::connect(&ctx, "ws://x", WsOptions::default())
            .await
            .map(|_| ());
        let mut events = sse::subscribe(&ctx, "https://x", Vec::new(), None);
        let sse = undra_ports::next(&mut events).await;
        let db = Database::open(&ctx, "app", &[]).await.map(|_| ());
        (ws, sse, db)
    });
    assert!(matches!(ws, Err(WsError::Network(text)) if text.contains("E0062")));
    assert!(matches!(sse, Some(Err(SseError::Network(text))) if text.contains("E0062")));
    assert!(matches!(db, Err(DbError::Unavailable(text)) if text.contains("E0062")));
}

// ---- Sse -----------------------------------------------------------------------------------------

#[test]
fn sse_resumes_from_the_last_id_after_the_server_ends() {
    let (t, fakes) = rig();
    let log = spawn_collect(&t, |ctx, log| async move {
        let mut last: Option<String> = None;
        for _ in 0..2 {
            let mut events = sse::subscribe(
                &ctx,
                "https://feed.test/",
                vec![Header::new("a", "b")],
                last.clone(),
            );
            while let Some(item) = undra_ports::next(&mut events).await {
                match item {
                    Ok(event) => {
                        log.lock()
                            .unwrap()
                            .push(format!("{}:{}", event.event, event.data));
                        last = event.id.or(last);
                    }
                    Err(error) => {
                        log.lock().unwrap().push(format!("err:{error}"));
                        break;
                    }
                }
            }
        }
    });
    let first = fakes.sse.last_stream().unwrap();
    fakes.sse.push(first, SseEvent::message("a").with_id("1"));
    fakes.sse.push(
        first,
        SseEvent::message("b").with_event("tick").with_id("2"),
    );
    fakes.sse.end(first);
    t.run_pending();
    let second = fakes.sse.last_stream().unwrap();
    assert_ne!(first, second);
    fakes.sse.fail(second, SseError::Network("gone".into()));
    t.run_pending();
    assert_eq!(
        *log.lock().unwrap(),
        [
            "message:a",
            "tick:b",
            "err:the server ended the event stream",
            "err:event stream network error: gone"
        ]
    );
    let streams = fakes.sse.streams();
    assert_eq!(streams[0].last_event_id, None);
    assert_eq!(streams[1].last_event_id.as_deref(), Some("2"));
    assert_eq!(streams[0].headers, [Header::new("a", "b")]);
}

#[test]
fn sse_refusal_is_the_only_item_and_close_ends_cleanly() {
    let (t, fakes) = rig();
    fakes.sse.refuse(
        "https://stop.test",
        SseError::Refused {
            status: Some(204),
            message: "no content".into(),
        },
    );
    let ctx = t.ctx();
    let items = t.run_until(async move {
        let mut events = sse::subscribe(&ctx, "https://stop.test/", Vec::new(), None);
        let mut items = Vec::new();
        while let Some(item) = undra_ports::next(&mut events).await {
            items.push(item);
        }
        items
    });
    assert_eq!(
        items,
        [Err(SseError::Refused {
            status: Some(204),
            message: "no content".into()
        })]
    );
    let ctx = t.ctx();
    let sse_fake = fakes.sse.clone();
    let after = t.run_until(async move {
        let mut events = sse::subscribe(&ctx, "https://ok.test/", Vec::new(), None);
        // Open it: push one event so the first item resolves.
        let first = undra_ports::next(&mut events);
        let _ = (first, sse_fake);
        events.close().await.unwrap();
        undra_ports::next(&mut events).await
    });
    assert_eq!(after, None);
    assert!(fakes.sse.streams().last().unwrap().closed_by_core);
    let proxy = SseProxy::new(t.ctx());
    let opened = t.run_until(async move {
        proxy
            .open("https://p.test".into(), vec![], Some("9".into()))
            .await
    });
    assert!(opened.is_ok());
}

// ---- Db ------------------------------------------------------------------------------------------

const MIGRATIONS: &[Migration] = &[
    Migration::new(
        1,
        "CREATE TABLE todos (id INTEGER PRIMARY KEY, title TEXT NOT NULL)",
    ),
    Migration::new(
        2,
        "ALTER TABLE todos ADD COLUMN done INTEGER NOT NULL DEFAULT 0",
    ),
];

#[test]
fn db_opens_migrates_and_runs_bound_statements() {
    let (t, fakes) = rig();
    fakes
        .db
        .respond_executed(
            "INSERT",
            DbExecuted {
                changes: 1,
                last_insert_id: 7,
            },
        )
        .respond_rows(
            "SELECT",
            DbRows {
                columns: vec!["title".into()],
                rows: vec![vec![DbValue::Text("milk".into())]],
            },
        );
    let ctx = t.ctx();
    let (version, id, title) = t.run_until(async move {
        let db = Database::open(&ctx, "app", MIGRATIONS).await.unwrap();
        let row = db
            .execute("INSERT INTO todos (title) VALUES (?)", params!["milk"])
            .await
            .unwrap();
        let rows = db
            .query(
                "SELECT title FROM todos WHERE id = ?",
                params![row.last_insert_id],
            )
            .await
            .unwrap();
        let title: String = rows.row(0).unwrap().get("title").unwrap();
        (db.version(), row.last_insert_id, title)
    });
    assert_eq!((version, id, title.as_str()), (2, 7, "milk"));
    let migrations = fakes.db.calls_of(DbCallKind::Migrate);
    assert_eq!(migrations.len(), 2);
    let query = fakes.db.calls_of(DbCallKind::Query);
    assert_eq!(query[0].params, [DbValue::Integer(7)]);
    t.run_pending();
    assert_eq!(
        fakes.db.calls().last().unwrap().kind,
        DbCallKind::Close,
        "dropping the database closed it"
    );
}

#[test]
fn db_transactions_commit_roll_back_and_roll_back_when_dropped() {
    let (t, fakes) = rig();
    fakes.db.fail_on(
        "dup",
        DbError::Constraint {
            kind: DbConstraint::Unique,
            message: "UNIQUE constraint failed: t.id".into(),
        },
    );
    let ctx = t.ctx();
    let db = t.run_until(async move { Database::open(&ctx, "tx", &[]).await.unwrap() });
    let d = db.clone();
    let committed = t.run_until(async move {
        d.transaction(|tx| async move {
            tx.execute("UPDATE t SET a = 1", params![]).await?;
            Ok(41 + 1)
        })
        .await
    });
    assert_eq!(committed, Ok(42));
    let d = db.clone();
    let failed = t.run_until(async move {
        d.transaction(|tx| async move {
            tx.execute("INSERT INTO t VALUES (1)", params![]).await?;
            tx.execute("INSERT dup", params![]).await?;
            Ok(())
        })
        .await
    });
    assert!(matches!(
        failed,
        Err(DbError::Constraint {
            kind: DbConstraint::Unique,
            ..
        })
    ));
    let kinds: Vec<DbCallKind> = fakes.db.calls().iter().map(|c| c.kind).collect();
    assert_eq!(
        kinds,
        [
            DbCallKind::Open,
            DbCallKind::Begin,
            DbCallKind::Execute,
            DbCallKind::Commit,
            DbCallKind::Begin,
            DbCallKind::Execute,
            DbCallKind::Execute,
            DbCallKind::Rollback
        ]
    );
    // An outer statement inside a running transaction is Busy, not a deadlock.
    let d = db.clone();
    let busy = t.run_until(async move {
        let outer = d.clone();
        d.transaction(|_tx| async move { outer.execute("UPDATE t SET a = 2", params![]).await })
            .await
    });
    assert_eq!(busy, Err(DbError::Busy));
    // A transaction whose task is dropped half-way is rolled back.
    let d = db.clone();
    let task = t.ctx().spawn(async move {
        let _ = d
            .transaction(|_tx| async move {
                core::future::pending::<()>().await;
                Ok(())
            })
            .await;
    });
    t.run_pending();
    assert!(fakes.db.in_transaction(db.id()));
    t.runtime().cancel_task(task);
    t.run_pending();
    assert!(
        !fakes.db.in_transaction(db.id()),
        "the dropped transaction was rolled back"
    );
    assert_eq!(fakes.db.calls().last().unwrap().kind, DbCallKind::Rollback);
}

/// The `Db` port and the methods these tests answer by hand (ADR-048's pinned ids).
const DB_PORT: u32 = 0x559e_da82;
const DB_OPEN: u32 = 0xee6f_26db;
const DB_BEGIN: u32 = 0xae2b_a428;
const DB_COMMIT: u32 = 0xf866_d5ae;
const DB_ROLLBACK: u32 = 0x3e7b_24b3;

/// A platform `Db` (no Rust binding: every call crosses the port table) that opens database 1 at
/// once, answers `begin` only when the test says so, and acknowledges `commit` and `rollback`.
fn platform_db() -> (TestRuntime, Database) {
    let t = TestRuntime::new();
    let opened = DbOpened { db: 1, version: 0 };
    t.host()
        .script_port_ok(DB_PORT, DB_OPEN, opened.encode_to_vec());
    t.host().script_port_async(DB_PORT, DB_BEGIN);
    t.host().script_port_ok(DB_PORT, DB_COMMIT, Vec::new());
    t.host().script_port_ok(DB_PORT, DB_ROLLBACK, Vec::new());
    let ctx = t.ctx();
    let db = t.run_until(async move { Database::open(&ctx, "app", &[]).await.unwrap() });
    (t, db)
}

/// The `(method, args)` of the calls the platform received for `method`.
fn db_calls(t: &TestRuntime, method: u32) -> Vec<Vec<u8>> {
    t.host()
        .port_calls()
        .into_iter()
        .filter(|c| c.port_id == DB_PORT && c.method_id == method)
        .map(|c| c.args)
        .collect()
}

#[test]
fn a_transaction_cancelled_while_begin_crosses_is_rolled_back_when_the_platform_answers() {
    // ADR-048 §5: a transaction dropped mid-way is rolled back. The platform runs `BEGIN IMMEDIATE`
    // whether or not the core still waits for the reply (a cancelled port call is abandoned, the
    // host is not told, SPEC 5.1), so a task cancelled during `begin` must not leave the
    // transaction it started open: every later statement on the database would be `Busy`.
    let (t, db) = platform_db();
    let d = db.clone();
    let task = t.ctx().spawn(async move {
        let _ = d.transaction(|_tx| async move { Ok(()) }).await;
    });
    t.run_pending();
    let begin = t
        .host()
        .port_calls()
        .into_iter()
        .find(|c| c.method_id == DB_BEGIN)
        .expect("begin crossed to the platform");
    t.runtime().cancel_task(task);
    t.run_pending();
    // The platform began transaction 7 anyway; its answer arrives after the core stopped waiting.
    t.runtime().port_reply(&port_reply(
        begin.port_call_id,
        PortStatus::Ok,
        &7u32.to_le_bytes(),
    ));
    t.run_pending();
    assert_eq!(
        db_calls(&t, DB_ROLLBACK),
        [7u32.to_le_bytes().to_vec()],
        "the transaction the platform began for a cancelled task is rolled back"
    );
    assert!(db_calls(&t, DB_COMMIT).is_empty());
}

#[test]
fn a_connect_or_open_cancelled_while_it_crosses_closes_what_the_platform_opened() {
    // ADR-047 §7 / ADR-048 §5: dropping without closing closes. The platform opens whether or not
    // the core still waits (an abandoned port call is not reported to the host), so a task
    // cancelled while `connect` / `open` crosses must close what the late answer names.
    const WS: u32 = 0x7388_b95f;
    const WS_CONNECT: u32 = 0x8347_7638;
    const WS_CLOSE: u32 = 0x6015_4b86;
    const SSE: u32 = 0x75d2_ef19;
    const SSE_OPEN: u32 = 0xc003_3c14;
    const SSE_CLOSE: u32 = 0x5bfe_2c88;
    const DB_CLOSE: u32 = 0xde3d_c7ed;
    let t = TestRuntime::new();
    t.host().script_port_async(WS, WS_CONNECT);
    t.host().script_port_ok(WS, WS_CLOSE, Vec::new());
    t.host().script_port_async(SSE, SSE_OPEN);
    t.host().script_port_ok(SSE, SSE_CLOSE, Vec::new());
    t.host().script_port_async(DB_PORT, DB_OPEN);
    t.host().script_port_ok(DB_PORT, DB_CLOSE, Vec::new());
    let ctx = t.ctx();
    let ws_task = t.ctx().spawn(async move {
        let _ = WsConnection::connect(&ctx, "wss://x", WsOptions::default()).await;
    });
    let ctx = t.ctx();
    let sse_task = t.ctx().spawn(async move {
        let mut events = sse::subscribe(&ctx, "https://x", Vec::new(), None);
        let _ = undra_ports::next(&mut events).await;
    });
    let ctx = t.ctx();
    let db_task = t.ctx().spawn(async move {
        let _ = Database::open(&ctx, "app", &[]).await;
    });
    t.run_pending();
    let call = |port: u32, method: u32| {
        t.host()
            .port_calls()
            .into_iter()
            .find(|c| c.port_id == port && c.method_id == method)
            .unwrap_or_else(|| panic!("{method:#x} crossed"))
            .port_call_id
    };
    let (connect, open, db_open) = (
        call(WS, WS_CONNECT),
        call(SSE, SSE_OPEN),
        call(DB_PORT, DB_OPEN),
    );
    for task in [ws_task, sse_task, db_task] {
        t.runtime().cancel_task(task);
    }
    t.run_pending();
    let opened = WsOpened {
        conn: 3,
        protocol: String::new(),
    };
    t.runtime().port_reply(&port_reply(
        connect,
        PortStatus::Ok,
        &opened.encode_to_vec(),
    ));
    t.runtime()
        .port_reply(&port_reply(open, PortStatus::Ok, &4u32.to_le_bytes()));
    let db = DbOpened { db: 5, version: 0 };
    t.runtime()
        .port_reply(&port_reply(db_open, PortStatus::Ok, &db.encode_to_vec()));
    t.run_pending();
    let args = |port: u32, method: u32| -> Vec<Vec<u8>> {
        t.host()
            .port_calls()
            .into_iter()
            .filter(|c| c.port_id == port && c.method_id == method)
            .map(|c| c.args)
            .collect()
    };
    let mut going_away = 3u32.to_le_bytes().to_vec();
    going_away.extend(1001u16.to_le_bytes());
    going_away.extend(String::new().encode_to_vec());
    assert_eq!(
        args(WS, WS_CLOSE),
        [going_away],
        "the orphaned connection is closed with 1001"
    );
    assert_eq!(
        args(SSE, SSE_CLOSE),
        [4u32.to_le_bytes().to_vec()],
        "the orphaned stream is closed"
    );
    assert_eq!(
        args(DB_PORT, DB_CLOSE),
        [5u32.to_le_bytes().to_vec()],
        "the orphaned database is closed"
    );
}

#[test]
fn a_transaction_whose_begin_answers_in_time_runs_and_commits_once() {
    let (t, db) = platform_db();
    let d = db.clone();
    let slot: Arc<Mutex<Option<Result<u32, DbError>>>> = Arc::default();
    let sink = slot.clone();
    t.ctx().spawn(async move {
        let r = d.transaction(|tx| async move { Ok(tx.id()) }).await;
        *sink.lock().unwrap() = Some(r);
    });
    t.run_pending();
    let begin = t
        .host()
        .port_calls()
        .into_iter()
        .find(|c| c.method_id == DB_BEGIN)
        .expect("begin crossed");
    t.runtime().port_reply(&port_reply(
        begin.port_call_id,
        PortStatus::Ok,
        &9u32.to_le_bytes(),
    ));
    t.run_pending();
    assert_eq!(slot.lock().unwrap().take(), Some(Ok(9)));
    assert_eq!(db_calls(&t, DB_COMMIT), [9u32.to_le_bytes().to_vec()]);
    assert!(db_calls(&t, DB_ROLLBACK).is_empty());
}

#[test]
fn db_checks_names_and_migrations_before_crossing() {
    let (t, fakes) = rig();
    let ctx = t.ctx();
    let (name, order, wide) = t.run_until(async move {
        let name = Database::open(&ctx, "../etc", &[]).await.map(|_| ());
        let order = Database::open(
            &ctx,
            "ok",
            &[Migration::new(2, "a"), Migration::new(1, "b")],
        )
        .await
        .map(|_| ());
        // SQLite keeps the version in `PRAGMA user_version`, a signed 32-bit integer: a larger one
        // would be stored as 0 and every later open would run the migrations again.
        let wide = Database::open(
            &ctx,
            "ok",
            &[Migration::new(1, "a"), Migration::new(3_000_000_000, "b")],
        )
        .await
        .map(|_| ());
        (name, order, wide)
    });
    assert!(matches!(name, Err(DbError::Unavailable(_))));
    assert!(matches!(order, Err(DbError::Migration { version: 1, .. })));
    assert!(
        matches!(wide, Err(DbError::Migration { version: 3_000_000_000, ref message }) if message.contains("2147483647")),
        "{wide:?}"
    );
    assert_eq!(
        undra_ports::db::validate_migrations(&[DbMigration {
            version: i32::MAX as u32,
            sql: String::new(),
        }]),
        Ok(()),
        "the largest version that fits is kept"
    );
    assert!(fakes.db.calls().is_empty(), "nothing crossed the boundary");
    let proxy = DbProxy::new(t.ctx());
    let migrated = t.run_until(async move {
        proxy
            .open(
                "p".into(),
                vec![DbMigration {
                    version: 1,
                    sql: "CREATE".into(),
                }],
            )
            .await
    });
    assert_eq!(migrated.map(|o| o.version), Ok(1));
}

// ---- properties ----------------------------------------------------------------------------------

fn ws_message() -> impl Strategy<Value = WsMessage> {
    prop_oneof![
        ".{0,8}".prop_map(WsMessage::Text),
        prop::collection::vec(any::<u8>(), 0..8).prop_map(|b| WsMessage::Binary(Bytes(b))),
    ]
}

fn db_value() -> impl Strategy<Value = DbValue> {
    prop_oneof![
        Just(DbValue::Null),
        any::<i64>().prop_map(DbValue::Integer),
        any::<f64>()
            .prop_filter("NaN is not equal to itself", |f| !f.is_nan())
            .prop_map(DbValue::Real),
        ".{0,6}".prop_map(DbValue::Text),
        prop::collection::vec(any::<u8>(), 0..6).prop_map(|b| DbValue::Blob(Bytes(b))),
    ]
}

proptest! {
    #[test]
    fn records_round_trip(
        message in ws_message(),
        values in prop::collection::vec(db_value(), 0..6),
        id in prop::option::of(".{0,4}"),
        retry in prop::option::of(any::<u32>()),
        status in prop::option::of(any::<u16>()),
        code in any::<u16>(),
    ) {
        prop_assert_eq!(WsMessage::decode_exact(&message.encode_to_vec()), Ok(message));
        let rows = DbRows { columns: vec!["c".into(); values.len()], rows: vec![values.clone(), values] };
        prop_assert_eq!(DbRows::decode_exact(&rows.encode_to_vec()), Ok(rows));
        let event = SseEvent { id, event: "e".into(), data: "d\nd".into(), retry_ms: retry };
        prop_assert_eq!(SseEvent::decode_exact(&event.encode_to_vec()), Ok(event));
        for error in [WsError::Refused { status, message: "m".into() }, WsError::Closed { code, reason: "r".into() }] {
            prop_assert_eq!(WsError::decode_exact(&error.encode_to_vec()), Ok(error));
        }
    }

    /// Whatever the server sends and however much the core reads, the core is handed at most one
    /// pull more than it consumed, and what it consumed is what was sent, in order.
    #[test]
    fn the_pull_bounds_the_read_ahead(sent in 0usize..200, read in 0usize..200) {
        let (t, fakes) = rig();
        let consumed = Arc::new(Mutex::new(Vec::new()));
        let keep: Arc<Mutex<Option<ws::WsMessages>>> = Arc::new(Mutex::new(None));
        let (sink, held, ctx) = (consumed.clone(), keep.clone(), t.ctx());
        t.ctx().spawn(async move {
            let conn = WsConnection::connect(&ctx, "ws://p.test", WsOptions::default()).await.unwrap();
            let mut messages = conn.messages();
            for _ in 0..read {
                match undra_ports::next(&mut messages).await {
                    Some(Ok(WsMessage::Text(text))) => sink.lock().unwrap().push(text),
                    _ => break,
                }
            }
            *held.lock().unwrap() = Some(messages);
        });
        t.run_pending();
        let conn = fakes.web_socket.last_conn().unwrap();
        for i in 0..sent {
            fakes.web_socket.push(conn, format!("{i}"));
            t.run_pending();
        }
        let consumed = consumed.lock().unwrap().clone();
        prop_assert_eq!(consumed.len(), sent.min(read));
        for (i, text) in consumed.iter().enumerate() {
            prop_assert_eq!(text, &format!("{i}"));
        }
        let delivered = fakes.web_socket.delivered(conn) as usize;
        prop_assert!(delivered <= consumed.len() + ws::CREDIT as usize + ws::LOW_WATER);
        prop_assert_eq!(delivered + fakes.web_socket.waiting(conn), sent);
    }
}

// ---- the dev-only real-SQLite fake -----------------------------------------------------------------

/// `db-fake` pulls `rusqlite`, which does not build for wasm32-unknown-unknown: no feature a core
/// ships with may reach it (ADR-048 §8).
#[test]
fn no_shipped_feature_reaches_db_fake() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let ports = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let facade = std::fs::read_to_string(root.join("../undra/Cargo.toml")).unwrap();
    let feature_line = |manifest: &str, name: &str| -> String {
        manifest
            .lines()
            .find(|l| l.trim_start().starts_with(&format!("{name} =")))
            .unwrap_or_default()
            .to_owned()
    };
    for name in ["default", "websocket", "sse", "db"] {
        assert!(
            !feature_line(&ports, name).contains("db-fake"),
            "undra-ports/{name}"
        );
        assert!(
            !feature_line(&ports, name).contains("rusqlite"),
            "undra-ports/{name}"
        );
        assert!(
            !feature_line(&facade, name).contains("db-fake"),
            "undra/{name}"
        );
    }
    assert!(
        ports.contains(
            "rusqlite = { version = \"0.40\", features = [\"bundled\"], optional = true }"
        )
    );
}

#[cfg(feature = "db-fake")]
#[test]
fn the_reference_adapter_runs_the_database_surface_on_real_sqlite() {
    use undra_ports::fakes::MemDb;
    let t = TestRuntime::new();
    let _fakes = fakes::install(&t);
    MemDb::new().install(t.runtime());
    let ctx = t.ctx();
    let outcome = t.run_until(async move {
        let db = Database::open(&ctx, "ref", MIGRATIONS).await?;
        assert_eq!(db.version(), 2);
        let id = db
            .transaction(|tx| async move {
                let a = tx
                    .execute("INSERT INTO todos (title) VALUES (?)", params!["a"])
                    .await?;
                tx.execute("INSERT INTO todos (title) VALUES (?)", params!["b"])
                    .await?;
                Ok(a.last_insert_id)
            })
            .await?;
        let failed = db
            .transaction(|tx| async move {
                tx.execute("INSERT INTO todos (title) VALUES (?)", params!["c"])
                    .await?;
                tx.execute(
                    "INSERT INTO todos (title) VALUES (?)",
                    params![None::<String>],
                )
                .await?;
                Ok(())
            })
            .await;
        assert!(matches!(
            failed,
            Err(DbError::Constraint {
                kind: DbConstraint::NotNull,
                ..
            })
        ));
        let rows = db
            .query("SELECT id, title, done FROM todos ORDER BY id", params![])
            .await?;
        assert_eq!(rows.len(), 2, "the failed transaction rolled back");
        let first = rows.row(0).unwrap();
        assert_eq!(first.get::<i64>("id")?, id);
        assert!(!first.get::<bool>("done")?);
        db.close().await?;
        Ok::<_, DbError>(())
    });
    assert_eq!(outcome, Ok(()));
}
