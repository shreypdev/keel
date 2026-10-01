//! The smaller message kinds and cross-cutting behaviour: logs, events, timers, restore,
//! ordering under concurrent writers.
#![cfg(feature = "server")]

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use undra::runtime::log;
use undra::wire::payload::{CallTarget, Log, ReplyStatus};
use undra::wire::{Kind, Reader, Writer};

fn logs_of(client: &TestClient) -> Vec<(u8, String, String)> {
    client
        .frames_of(Kind::Log)
        .into_iter()
        .map(|frame| {
            let record = Log::decode(&mut Reader::new(&frame.payload)).unwrap();
            (
                record.level,
                record.target.to_owned(),
                record.message.to_owned(),
            )
        })
        .collect()
}

// ----- logs (SPEC 5.10) -------------------------------------------------------------------------

#[test]
fn dev_clients_receive_the_cores_devtools_records() {
    let f = start();
    let mut client = f.client(); // mode = "dev"
    let handle = client.new_counter(0);
    client.observe(handle, u32::MAX, true);
    client.method(handle, ADD, &enc(&1_i32));
    client.method(handle, GET, &[]);
    let records = logs_of(&client);
    assert!(
        records
            .iter()
            .any(|(_, target, message)| target == "undra::devtools"
                && message.starts_with("commit txn=")),
        "a commit record: {records:?}"
    );
}

#[test]
fn clients_that_did_not_ask_for_dev_mode_do_not_receive_devtools_records() {
    let f = start();
    let mut client = TestClient::connect_raw(&f.url(), f.schema());
    client.handshake("web", "prod");
    let handle = client.new_counter(0);
    client.observe(handle, u32::MAX, true);
    client.method(handle, ADD, &enc(&1_i32));
    client.method(handle, GET, &[]);
    let records = logs_of(&client);
    assert!(
        !records
            .iter()
            .any(|(_, target, _)| target == "undra::devtools"),
        "no devtools records for a prod client: {records:?}"
    );
    assert!(
        records
            .iter()
            .any(|(_, target, message)| target == "undra::transport"
                && message.contains("client connected")),
        "ordinary records still arrive: {records:?}"
    );
}

#[test]
fn the_cores_log_records_reach_the_client_and_the_sink() {
    let f = start();
    let mut client = f.client();
    f.rt.log(log::WARN, "app::sync", "careful");
    let frame = loop {
        let frame = client.recv_kind(Kind::Log);
        if Log::decode(&mut Reader::new(&frame.payload))
            .unwrap()
            .message
            == "careful"
        {
            break frame;
        }
    };
    let record = Log::decode(&mut Reader::new(&frame.payload)).unwrap();
    assert_eq!((record.level, record.target), (3, "app::sync"));
    assert!(f.log_lines().contains(&"3 app::sync: careful".to_owned()));
}

#[test]
fn a_record_from_a_panicking_call_is_forwarded_too() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    let (status, _) = client.method(handle, BOOM, &[]);
    assert_eq!(status, ReplyStatus::Panic);
    let records = logs_of(&client);
    assert!(
        records
            .iter()
            .any(|(level, _, message)| *level == 5 && message.contains("kaboom")),
        "a fatal record: {records:?}"
    );
}

#[test]
fn the_first_message_is_always_the_hello_even_while_the_core_is_logging() {
    let f = start();
    let stop = Arc::new(AtomicBool::new(false));
    let noisy = {
        let (rt, stop) = (f.rt.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                rt.log(log::INFO, "noise", "hello?");
            }
        })
    };
    for _ in 0..25 {
        let mut client = TestClient::connect_raw(&f.url(), f.schema());
        client.send_hello(f.schema(), "test", "dev");
        let first = client.expect_any();
        assert_eq!((first.kind, first.seq), (Kind::Hello, 0));
        // Everything after it is in order too.
        for _ in 0..5 {
            let next = client.expect_any();
            assert_eq!(next.seq, client.seen.len() as u32 - 1);
        }
        drop(client);
        f.eventually("the slot is free", |f| !f.bridge.is_connected());
    }
    stop.store(true, Ordering::Relaxed);
    noisy.join().unwrap();
}

// ----- events, timers, restore --------------------------------------------------------------------

#[test]
fn an_event_reaches_the_subscribers_of_its_port_and_method() {
    let f = start();
    let received: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
    let sink = received.clone();
    f.rt.events()
        .subscribe(
            7,
            9,
            Box::new(move |payload| sink.lock().unwrap().push(payload.to_vec())),
        )
        .detach();
    let mut client = f.client();
    let mut w = Writer::new();
    undra::wire::payload::Event {
        port_id: 7,
        method_id: 9,
        payload: &[1, 2, 3],
    }
    .encode(&mut w);
    client.send(Kind::Event, w.as_slice());
    // A round trip proves it was processed.
    client.call(
        CallTarget::Function { method_id: SUM },
        &[enc(&1_i32), enc(&1_i32)].concat(),
    );
    assert_eq!(*received.lock().unwrap(), [vec![1, 2, 3]]);
}

#[test]
fn a_timer_fired_for_an_unknown_timer_is_ignored() {
    let f = start();
    let mut client = f.client();
    client.send(Kind::TimerFired, &enc(&12345_u32));
    let (status, _) = client.call(
        CallTarget::Function { method_id: SUM },
        &[enc(&1_i32), enc(&1_i32)].concat(),
    );
    assert_eq!(status, ReplyStatus::Ok);
}

#[test]
fn a_restore_rebuilds_the_stores_and_keeps_the_handles_valid() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(5);
    client.method(handle, ADD, &enc(&2_i32));
    client.observe(handle, COUNT_SIGNAL, true);
    client.recv_kind(Kind::ChangeSet);

    let snapshot = f.rt.snapshot();
    client.method(handle, ADD, &enc(&100_i32));
    client.send(Kind::Restore, &snapshot);

    // The same handle, the value from the snapshot, and the observer is told.
    let (status, body) = client.method(handle, GET, &[]);
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 7));
    let counts: Vec<i32> = change_sets(&client)
        .iter()
        .filter_map(|cs| cs.entries.iter().find(|e| e.signal_id == COUNT_SIGNAL))
        .map(|e| dec::<i32>(&e.value))
        .collect();
    assert_eq!(counts.last(), Some(&7), "{counts:?}");
}

#[test]
fn a_restore_that_is_refused_is_reported_and_the_connection_carries_on() {
    let f = start();
    let mut client = f.client();
    client.send(Kind::Restore, &[1, 0, 0, 0, 9]); // one store, cut short
    let (status, _) = client.call(
        CallTarget::Function { method_id: SUM },
        &[enc(&1_i32), enc(&1_i32)].concat(),
    );
    assert_eq!(status, ReplyStatus::Ok);
    f.eventually("the refusal was noted", |f| {
        f.log_lines()
            .iter()
            .any(|l| l.contains("the client's Restore was refused"))
    });
    let records = logs_of(&client);
    assert!(
        records
            .iter()
            .any(|(level, _, m)| *level == 4 && m.contains("Restore was refused")),
        "{records:?}"
    );
}

// ----- ordering --------------------------------------------------------------------------------------

#[test]
fn change_sets_from_a_busy_core_thread_arrive_complete_and_in_order() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    client.observe(handle, COUNT_SIGNAL, true);
    client.recv_kind(Kind::ChangeSet);

    // The writer is a task on the core thread: it commits 500 times, yielding between commits,
    // so the client's calls (taking the core lock on the connection's thread) interleave.
    let (done, finished) = std::sync::mpsc::channel();
    let counter = f.rt.object::<Counter>(handle).unwrap();
    f.rt.ctx().spawn(async move {
        for _ in 0..500 {
            counter.add(1);
            undra::runtime::executor::yield_now().await;
        }
        let _ = done.send(());
    });
    // Calls from the client interleave with the writer's commits.
    for _ in 0..50 {
        client.method(handle, GET, &[]);
    }
    finished
        .recv_timeout(Duration::from_secs(10))
        .expect("the writer finished");
    while change_sets(&client).len() < 501 {
        match client.recv_within(Duration::from_secs(5)) {
            Received::Frame(_) => {}
            other => panic!("not all 500 commits were forwarded: {other:?}"),
        }
    }

    let txns: Vec<u64> = change_sets(&client).iter().map(|cs| cs.txn_id).collect();
    assert!(
        txns.windows(2).all(|w| w[0] < w[1]),
        "txn ids strictly increase"
    );
    let seqs: Vec<u32> = client.seen.iter().map(|frame| frame.seq).collect();
    assert_eq!(
        seqs,
        (0..seqs.len() as u32).collect::<Vec<_>>(),
        "gapless, in queue order"
    );
    let counts: Vec<i32> = change_sets(&client)
        .iter()
        .map(|cs| dec::<i32>(&cs.entries[0].value))
        .collect();
    assert_eq!(counts, (0..=500).collect::<Vec<_>>());
}
