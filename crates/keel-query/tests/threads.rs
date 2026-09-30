//! The client is shared state on a runtime whose calls arrive from several threads (host
//! threads, the core thread): hammer it from many threads at once and check nothing deadlocks,
//! panics or loses count.

mod common;

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use common::*;
use keel::runtime::testing::RecordingHost;
use keel::runtime::{Runtime, RuntimeConfig};
use keel_ports::fakes::Fakes;
use keel_query::{CtxQuery, QueryStatus};

fn threaded_runtime() -> (Arc<Runtime>, Fakes) {
    let host = Arc::new(RecordingHost::new());
    let rt = Runtime::new(
        RuntimeConfig {
            platform: "test".to_owned(),
            mode: "inproc".to_owned(),
            core_threads: 1,
            blocking_threads: 1,
            log_level: 0,
        },
        host,
    )
    .unwrap();
    let fakes = Fakes::new();
    fakes.install(&rt);
    (rt, fakes)
}

/// Runs `f` on this thread with a watchdog: a deadlock fails the test instead of hanging it.
fn within(seconds: u64, f: impl FnOnce() + Send + 'static) {
    let (done, finished) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        f();
        let _ = done.send(());
    });
    match finished.recv_timeout(Duration::from_secs(seconds)) {
        Ok(()) => worker.join().unwrap(),
        Err(_) => panic!("no progress in {seconds} s: a deadlock in the query client"),
    }
}

#[test]
fn many_threads_observing_invalidating_and_releasing_at_once() {
    let (rt, fakes) = threaded_runtime();
    for page in 0..4 {
        fakes.http.respond(
            format!("{API}/todos?page={page}"),
            ok(&Page {
                items: vec![todo(page as u8, "x")],
                total: 1,
            }),
        );
    }
    let ctx = rt.ctx();
    let rt2 = rt.clone();
    within(60, move || {
        let threads: Vec<_> = (0..8_u32)
            .map(|t| {
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let query = ctx.query();
                    for i in 0..150_u32 {
                        let page = (t + i) % 4;
                        let handle = query.observe::<TodosQuery>((page,));
                        match i % 5 {
                            0 => handle.refetch(),
                            1 => query.invalidate(format!("todos:{page}")),
                            2 => query.invalidate("todos"),
                            _ => {}
                        }
                        let _ = handle.status().get();
                        let _ = handle.data().get();
                        if i % 7 == 0 {
                            drop(handle);
                        } else {
                            // Keep a few alive across iterations, drop the rest in a burst.
                            std::mem::forget(handle);
                        }
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        // Let the core thread finish what it was given.
        for _ in 0..200 {
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(rt2);
    });
    let stats = rt.stats_json();
    assert!(stats.contains("\"panics\":0"), "{stats}");
    rt.shutdown();
}

#[test]
fn observers_on_other_threads_see_the_result_of_a_fetch_the_core_ran() {
    let (rt, fakes) = threaded_runtime();
    fakes.http.respond(
        format!("{API}/todos?page=0"),
        ok(&page(vec![todo(1, "milk")])),
    );
    let ctx = rt.ctx();
    within(30, move || {
        let handle = ctx.query().observe::<TodosQuery>((0,));
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while handle.status().get() != QueryStatus::Success {
            assert!(
                std::time::Instant::now() < deadline,
                "the fetch never finished"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(handle.data().get(), Some(page(vec![todo(1, "milk")])));
    });
    rt.shutdown();
}
