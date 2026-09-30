//! The threaded runtime: concurrent callers, the `keel-core` thread, the blocking pool, the
//! timer thread, re-entrancy detection and shutdown. Every test that could deadlock runs
//! under a watchdog.

mod common;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use common::*;
use keel_runtime::testing::{
    RecordingHost, ReplyRecord, call_payload, decode_reply, port_reply_ok,
};
use keel_runtime::{Host, PortCallOutcome, Runtime, RuntimeConfig};
use keel_wire::payload::{CallTarget, ReplyStatus};
use keel_wire::{Decode, Encode, Reader};
use parking_lot::Mutex;

const LONG: Duration = Duration::from_secs(60);

fn threaded() -> (Arc<Runtime>, Arc<RecordingHost>) {
    let host = Arc::new(RecordingHost::new());
    let rt = Runtime::new(
        RuntimeConfig {
            platform: "test".into(),
            log_level: 0,
            ..RuntimeConfig::default()
        },
        host.clone(),
    )
    .unwrap();
    (rt, host)
}

#[test]
fn eight_threads_call_sync_concurrently_without_deadlock_and_with_correct_replies() {
    const THREADS: u32 = 8;
    const CALLS: u32 = 250;
    let (rt, _host) = threaded();
    let shared = new_counter_rt(&rt, 0, "shared");
    let start = Arc::new(std::sync::Barrier::new(THREADS as usize));

    let results = with_timeout("8 threads x call_sync", LONG, {
        let rt = rt.clone();
        move || {
            let handles: Vec<_> = (0..THREADS)
                .map(|thread| {
                    let (rt, start) = (rt.clone(), start.clone());
                    std::thread::spawn(move || {
                        let own = new_counter_rt(&rt, 1000 * thread as i32, "own");
                        start.wait();
                        let mut shared_values = Vec::new();
                        for i in 0..CALLS {
                            let call_id = thread * 10_000 + i + 2;
                            // The shared store: every add returns a distinct running total.
                            let reply = call_sync_rt(
                                &rt,
                                counter_target(shared, ADD),
                                call_id,
                                &enc(&1_i32),
                            );
                            assert_eq!(reply.call_id, call_id, "reply belongs to this call");
                            shared_values.push(decode_body::<i32>(&reply));
                            // A private store: exact expected value, and a pure function.
                            let mine =
                                call_sync_rt(&rt, counter_target(own, ADD), call_id, &enc(&1_i32));
                            assert_eq!(
                                decode_body::<i32>(&mine),
                                1000 * thread as i32 + i as i32 + 1
                            );
                            let sum = call_sync_rt(
                                &rt,
                                function_target(SUM),
                                call_id,
                                &args(|w| {
                                    (thread as i32).encode(w);
                                    (i as i32).encode(w);
                                }),
                            );
                            assert_eq!(decode_body::<i32>(&sum), (thread + i) as i32);
                        }
                        shared_values
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect::<Vec<i32>>()
        }
    });

    // One mutator at a time: the 2000 running totals are exactly 1..=2000, each seen once.
    let total = (THREADS * CALLS) as i32;
    let seen: BTreeSet<i32> = results.iter().copied().collect();
    assert_eq!(results.len(), total as usize);
    assert_eq!(seen.len(), total as usize, "no lost or duplicated update");
    assert_eq!(seen.first(), Some(&1));
    assert_eq!(seen.last(), Some(&total));
    rt.shutdown();
}

#[test]
fn async_calls_run_on_the_core_thread_and_reply_from_it() {
    let (rt, host) = threaded();
    let h = new_counter_rt(&rt, 0, "");
    let calls = 40_u32;
    with_timeout("async calls from 8 threads", LONG, {
        let rt = rt.clone();
        move || {
            let handles: Vec<_> = (0..8_u32)
                .map(|thread| {
                    let rt = rt.clone();
                    std::thread::spawn(move || {
                        for i in 0..calls / 8 {
                            let call_id = 100 + thread * 10 + i;
                            let payload = counter_call_payload(h, SLOW_ADD, call_id, &enc(&1_i32));
                            assert_eq!(rt.call(&payload), 0);
                        }
                    })
                })
                .collect();
            for handle in handles {
                handle.join().unwrap();
            }
        }
    });
    assert!(
        host.wait_for_replies(calls as usize, LONG),
        "all {calls} replies arrive (10 ms real sleeps on the timer thread, polled on the core thread)"
    );
    let replies = host.take_replies();
    let ids: BTreeSet<u32> = replies.iter().map(|r| r.call_id).collect();
    assert_eq!(ids.len(), calls as usize, "one reply per call");
    let totals: BTreeSet<i32> = replies.iter().map(decode_body::<i32>).collect();
    assert_eq!(
        totals,
        (1..=calls as i32).collect(),
        "task polls are serialised by the core lock"
    );
    rt.shutdown();
}

#[test]
fn async_bodies_run_on_keel_core_and_blocking_closures_on_the_pool() {
    let (rt, host) = threaded();
    let h = new_counter_rt(&rt, 0, "");
    assert_eq!(rt.call(&counter_call_payload(h, THREADS, 7, &[])), 0);
    assert!(host.wait_for_replies(1, LONG));
    let reply = host.take_replies().remove(0);
    let text: String = decode_body(&reply);
    let parts: Vec<&str> = text.split('|').collect();
    assert_eq!(parts[0], "keel-core", "{text}");
    assert!(parts[1].starts_with("keel-blocking-"), "{text}");
    assert_eq!(
        parts[2], "keel-core",
        "resumed on the core after the blocking work: {text}"
    );
    let stats: serde_json::Value = serde_json::from_str(&rt.stats_json()).unwrap();
    assert!(stats["blocking_threads"]["started"].as_u64().unwrap() >= 1);
    assert!(stats["blocking_threads"]["max"].as_u64().unwrap() <= 4);
    rt.shutdown();
}

#[test]
fn a_busy_core_thread_does_not_starve_host_calls() {
    let (rt, _host) = threaded();
    let h = new_counter_rt(&rt, 0, "");
    let stop = Arc::new(AtomicBool::new(false));
    // A task that keeps re-waking itself: the core thread is polling all the time.
    let spins = Arc::new(AtomicU32::new(0));
    {
        let (stop, spins) = (stop.clone(), spins.clone());
        rt.spawn(async move {
            while !stop.load(Ordering::SeqCst) {
                spins.fetch_add(1, Ordering::SeqCst);
                keel_runtime::executor::yield_now().await;
            }
        });
    }
    assert!(
        wait_until(LONG, || spins.load(Ordering::SeqCst) > 100),
        "the core thread is spinning"
    );

    let done = with_timeout("sync calls while the core thread is busy", LONG, {
        let rt = rt.clone();
        move || {
            let handles: Vec<_> = (0..4_u32)
                .map(|t| {
                    let rt = rt.clone();
                    std::thread::spawn(move || {
                        for i in 0..100_u32 {
                            let r = call_sync_rt(
                                &rt,
                                counter_target(h, ADD),
                                t * 1000 + i + 2,
                                &enc(&1_i32),
                            );
                            assert_eq!(r.status, ReplyStatus::Ok);
                        }
                    })
                })
                .collect();
            handles.into_iter().for_each(|h| h.join().unwrap());
            true
        }
    });
    assert!(done);
    stop.store(true, Ordering::SeqCst);
    assert_eq!(
        decode_body::<i32>(&call_sync_rt(&rt, counter_target(h, GET), 9, &[])),
        400
    );
    assert!(wait_until(LONG, || rt.stats_json().contains("\"tasks\":0")));
    rt.shutdown();
}

#[test]
fn cancel_credit_and_port_replies_from_other_threads_wake_the_core() {
    let (rt, host) = threaded();
    let h = new_counter_rt(&rt, 0, "");

    // cancel() from another thread.
    assert_eq!(rt.call(&counter_call_payload(h, FOREVER, 10, &[])), 0);
    with_timeout("cancel", LONG, {
        let rt = rt.clone();
        move || rt.cancel(10)
    });
    assert!(host.wait_for_replies(1, LONG));
    assert_eq!(host.take_replies()[0].status, ReplyStatus::Cancelled);

    // stream_credit() from another thread.
    assert_eq!(
        rt.call(&counter_call_payload(h, TICKS, 11, &enc(&4_u32))),
        0
    );
    assert!(host.wait_for_replies(1, LONG));
    assert_eq!(host.take_replies()[0].status, ReplyStatus::StreamOpened);
    with_timeout("credit", LONG, {
        let rt = rt.clone();
        move || rt.stream_credit(11, 100)
    });
    assert!(
        host.wait_for_stream_items(5, LONG),
        "4 items and the end marker"
    );

    // port_reply() from another thread, some time after the call.
    host.script_port_async(TEST_PORT, ASK);
    assert_eq!(
        rt.call(&counter_call_payload(h, ASK_PORT, 12, &enc(&1_i32))),
        0
    );
    assert!(wait_until(LONG, || !host.port_calls().is_empty()));
    let id = host.port_calls()[0].port_call_id;
    let replier = {
        let rt = rt.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            rt.port_reply(&port_reply_ok(id, &enc(&99_i32)));
        })
    };
    assert!(host.wait_for_replies(1, LONG));
    replier.join().unwrap();
    assert_eq!(decode_body::<i32>(&host.take_replies()[0]), 99);
    rt.shutdown();
}

#[test]
fn a_task_may_spawn_from_a_blocking_thread() {
    let (rt, _host) = threaded();
    let ran = Arc::new(AtomicU32::new(0));
    let (ctx, r) = (rt.ctx(), ran.clone());
    rt.spawn(async move {
        let inner = ctx.clone();
        ctx.spawn_blocking(move || {
            // On a pool thread `Ctx::current()` works and `spawn` needs no core lock.
            let here = keel_runtime::Ctx::current();
            assert_eq!(here.runtime().id(), inner.runtime().id());
            inner.spawn(async move {
                r.fetch_add(1, Ordering::SeqCst);
            });
        })
        .await;
    });
    assert!(wait_until(LONG, || ran.load(Ordering::SeqCst) == 1));
    rt.shutdown();
}

#[test]
fn a_hundred_tasks_run_across_several_core_turns() {
    let (rt, _host) = threaded();
    let ran = Arc::new(AtomicU32::new(0));
    for _ in 0..200 {
        let r = ran.clone();
        rt.spawn(async move {
            keel_runtime::executor::yield_now().await;
            r.fetch_add(1, Ordering::SeqCst);
        });
    }
    assert!(wait_until(LONG, || ran.load(Ordering::SeqCst) == 200));
    let stats: serde_json::Value = serde_json::from_str(&rt.stats_json()).unwrap();
    assert!(
        stats["turns"].as_u64().unwrap() >= 200 * 2 / 64,
        "batches of at most 64 polls"
    );
    rt.shutdown();
}

// ----- re-entrancy ------------------------------------------------------------------------

/// A host that calls back into the runtime from its callbacks: forbidden by SPEC 5.1.
struct Reentrant {
    rt: OnceLock<Weak<Runtime>>,
    seen: Mutex<Vec<(String, ReplyRecord)>>,
    entry_points: Mutex<Vec<String>>,
}

impl Host for Reentrant {
    fn reply(&self, call_id: u32, _payload: &[u8]) {
        let Some(rt) = self.rt.get().and_then(Weak::upgrade) else {
            return;
        };
        let thread = std::thread::current().name().unwrap_or("?").to_owned();
        // Every entry point the host must not use from a callback: each must be refused, not
        // deadlock, on the thread that holds the core lock.
        let payload = call_payload(
            function_target(SUM),
            4242,
            &args(|w| {
                1_i32.encode(w);
                1_i32.encode(w);
            }),
        );
        let reply = decode_reply(&rt.call_sync(&payload));
        self.seen.lock().push((thread, reply));
        assert_eq!(rt.call(&payload), 5);
        rt.cancel(call_id);
        rt.observe(1, u32::MAX, true);
        rt.release(1);
        rt.event(1, 2, &[]);
        rt.poll();
        assert_eq!(rt.run_pending(), 0);
        self.entry_points.lock().push("done".to_owned());
    }
    fn change_set(&self, _: &[u8]) {}
    fn stream_item(&self, _: u32, _: &[u8]) {}
    fn port_call(&self, _: u32, _: u32, _: u32, _: &[u8]) -> PortCallOutcome {
        PortCallOutcome::Unavailable
    }
    fn log(&self, _: u8, _: &str, _: &str) {}
}

#[test]
fn calls_from_inside_host_callbacks_are_refused_with_e_reentrant() {
    let host = Arc::new(Reentrant {
        rt: OnceLock::new(),
        seen: Mutex::new(Vec::new()),
        entry_points: Mutex::new(Vec::new()),
    });
    let rt = Runtime::new(RuntimeConfig::default(), host.clone()).unwrap();
    host.rt.set(Arc::downgrade(&rt)).unwrap();

    // (a) a sync result: `reply` runs on the caller's thread inside `call`.
    // (b) an async result: `reply` runs on the keel-core thread inside a task poll.
    with_timeout("re-entrant host callbacks", LONG, {
        let rt = rt.clone();
        move || {
            let h_reply = call_sync_rt(
                &rt,
                function_target(SUM),
                1,
                &args(|w| {
                    1_i32.encode(w);
                    2_i32.encode(w);
                }),
            );
            assert_eq!(
                h_reply.status,
                ReplyStatus::Ok,
                "call_sync itself is not a callback"
            );
            let payload = call_payload(
                function_target(SUM),
                2,
                &args(|w| {
                    3_i32.encode(w);
                    4_i32.encode(w);
                }),
            );
            assert_eq!(rt.call(&payload), 0);
            assert_eq!(rt.call(&call_payload(function_target(SLOW_FN), 3, &[])), 0);
        }
    });
    assert!(
        wait_until(LONG, || host.entry_points.lock().len() == 2),
        "both callbacks finished"
    );

    let seen = host.seen.lock();
    assert_eq!(seen.len(), 2);
    for (thread, reply) in seen.iter() {
        assert_eq!(reply.status, ReplyStatus::BadRequest, "on {thread}");
        let mut r = Reader::new(&reply.body);
        assert!(r.read_str().unwrap().contains("E_REENTRANT"), "on {thread}");
    }
    let threads: BTreeSet<&str> = seen.iter().map(|(t, _)| t.as_str()).collect();
    assert!(
        threads.contains("keel-core"),
        "one refusal happened on the core thread: {threads:?}"
    );
    drop(seen);
    // Nothing is stuck: the runtime still works.
    let reply = call_sync_rt(
        &rt,
        function_target(SUM),
        9,
        &args(|w| {
            5_i32.encode(w);
            6_i32.encode(w);
        }),
    );
    assert_eq!(decode_body::<i32>(&reply), 11);
    rt.shutdown();
}

// ----- lifecycle --------------------------------------------------------------------------

#[test]
fn shutdown_stops_every_thread_and_is_safe_to_call_concurrently() {
    let (rt, host) = threaded();
    let h = new_counter_rt(&rt, 0, "");
    // Leave work in flight: a sleeping call (timer thread), a blocking job (pool), a stream.
    assert_eq!(
        rt.call(&counter_call_payload(h, SLOW_ADD, 2, &enc(&1_i32))),
        0
    );
    assert_eq!(
        rt.call(&counter_call_payload(h, BLOCKING_SQUARE, 3, &enc(&3_i32))),
        0
    );
    assert_eq!(
        rt.call(&counter_call_payload(h, TICKS, 4, &enc(&100_u32))),
        0
    );
    assert!(
        host.wait_for_replies(2, LONG),
        "the blocking call and the stream opening"
    );

    with_timeout("shutdown", LONG, {
        let rt = rt.clone();
        move || {
            let a = {
                let rt = rt.clone();
                std::thread::spawn(move || rt.shutdown())
            };
            rt.shutdown();
            a.join().unwrap();
        }
    });
    assert!(rt.is_shut_down());
    assert_eq!(
        Arc::strong_count(&rt),
        1,
        "no thread or task keeps the runtime alive"
    );
    assert_eq!(rt.call(&counter_call_payload(h, GET, 5, &[])), 5);
    assert_eq!(
        call_sync_rt(&rt, counter_target(h, GET), 6, &[]).status,
        ReplyStatus::BadRequest
    );
}

#[test]
fn an_idle_runtime_is_freed_and_its_threads_stopped_when_the_last_reference_drops() {
    // No object holds a `Ctx`, no task is in flight: dropping the last `Arc` ends everything.
    let (rt, host) = threaded();
    assert_eq!(rt.call(&call_payload(function_target(SLOW_FN), 2, &[])), 0);
    assert!(
        host.wait_for_replies(1, LONG),
        "an async call ran on keel-core and finished"
    );
    drop(rt.spawn_blocking(|| 1)); // starts a pool thread
    let weak = Arc::downgrade(&rt);
    with_timeout("dropping the runtime", LONG, move || drop(rt));
    // The pool job holds a `Ctx` until it has run, so the last reference may die on the pool
    // thread a moment later; the runtime must then tear itself down from that thread.
    assert!(wait_until(LONG, || weak.upgrade().is_none()));
}

#[test]
fn stores_and_tasks_hold_a_ctx_so_shutdown_is_what_releases_the_runtime() {
    // `Ctx` is an `Arc<Runtime>`: a store or an in-flight task that holds one is a reference
    // cycle with the runtime that owns it. `shutdown` drops both and breaks it.
    let (rt, host) = threaded();
    let h = new_counter_rt(&rt, 0, "");
    assert_eq!(
        rt.call(&counter_call_payload(h, SLOW_ADD, 2, &enc(&1_i32))),
        0
    );
    assert_eq!(
        rt.call(&counter_call_payload(h, BLOCKING_SQUARE, 3, &enc(&3_i32))),
        0
    );
    assert!(host.wait_for_replies(1, LONG));
    let weak = Arc::downgrade(&rt);
    let for_shutdown = rt.clone();
    drop(rt);
    assert!(
        weak.upgrade().is_some(),
        "the counter's Ctx and the sleeping task keep it alive"
    );
    with_timeout("shutdown", LONG, move || for_shutdown.shutdown());
    assert!(
        wait_until(LONG, || weak.upgrade().is_none()),
        "freed once shutdown broke the cycles"
    );
}

#[test]
fn a_task_holding_the_last_reference_can_drop_the_runtime_from_the_core_thread() {
    // Drop runs on keel-core itself here; it must not try to join itself.
    let (rt, host) = threaded();
    let weak = Arc::downgrade(&rt);
    let slot = Arc::new(Mutex::new(Some(rt)));
    {
        let slot = slot.clone();
        let rt = slot.lock().as_ref().unwrap().clone();
        rt.spawn(async move {
            // Give up the test's reference, then ours: the last one dies on keel-core.
            drop(slot.lock().take());
        });
        drop(rt);
    }
    assert!(
        wait_until(LONG, || weak.upgrade().is_none()),
        "runtime freed"
    );
    drop(host);
}

#[test]
fn the_default_runtime_is_configured_for_one_core_thread_and_a_bounded_pool() {
    let (rt, _host) = threaded();
    let stats: serde_json::Value = serde_json::from_str(&rt.stats_json()).unwrap();
    let max = stats["blocking_threads"]["max"].as_u64().unwrap();
    assert!((1..=4).contains(&max), "min(4, cores) = {max}");
    rt.shutdown();

    let host = Arc::new(RecordingHost::new());
    let rt = Runtime::new(
        RuntimeConfig {
            blocking_threads: 2,
            ..RuntimeConfig::default()
        },
        host,
    )
    .unwrap();
    let stats: serde_json::Value = serde_json::from_str(&rt.stats_json()).unwrap();
    assert_eq!(stats["blocking_threads"]["max"], 2);
    rt.shutdown();
}

#[test]
fn panics_on_the_core_thread_do_not_kill_it() {
    let (rt, host) = threaded();
    let h = new_counter_rt(&rt, 0, "");
    for i in 0..5_u32 {
        assert_eq!(
            rt.call(&counter_call_payload(h, PANIC_ASYNC, 10 + i, &[])),
            0
        );
    }
    assert!(host.wait_for_replies(5, LONG));
    assert!(
        host.take_replies()
            .iter()
            .all(|r| r.status == ReplyStatus::Panic)
    );
    // The core thread survived every panic and still serves async work.
    assert_eq!(
        rt.call(&counter_call_payload(h, BLOCKING_SQUARE, 20, &enc(&5_i32))),
        0
    );
    assert!(host.wait_for_replies(1, LONG));
    assert_eq!(decode_body::<i32>(&host.take_replies()[0]), 25);
    rt.shutdown();
}

#[test]
fn calls_racing_shutdown_never_leave_state_behind() {
    // A call that slips in around `shutdown` must either be refused or be torn down with the
    // rest; nothing may stay registered in a runtime that is shut down. (A smoke test: the
    // window is a few microseconds wide. The re-check of the shutdown flag under the core
    // lock in `call` and `call_sync` is what closes it.)
    for round in 0..25_u32 {
        let (rt, _host) = threaded();
        let h = new_counter_rt(&rt, 0, "");
        let hammer = {
            let rt = rt.clone();
            std::thread::spawn(move || {
                let mut accepted = 0_u32;
                for i in 0..200_u32 {
                    let id = round * 1000 + i + 1;
                    if rt.call(&counter_call_payload(h, FOREVER, id, &[])) == 0 {
                        accepted += 1;
                    }
                    if i % 7 == 0 {
                        std::thread::yield_now();
                    }
                }
                accepted
            })
        };
        std::thread::sleep(Duration::from_micros(200 * u64::from(round % 5)));
        with_timeout("shutdown racing calls", LONG, {
            let rt = rt.clone();
            move || rt.shutdown()
        });
        let _accepted = hammer.join().unwrap();
        let stats: serde_json::Value = serde_json::from_str(&rt.stats_json()).unwrap();
        assert_eq!(stats["active_calls"], 0, "round {round}: {stats}");
        assert_eq!(stats["tasks"], 0, "round {round}: {stats}");
        assert_eq!(stats["live_handles"], 0, "round {round}: {stats}");
    }
}

// ----- races and randomised stress --------------------------------------------------------

#[test]
fn exactly_one_of_many_racing_calls_with_the_same_id_is_accepted() {
    let (rt, host) = threaded();
    let h = new_counter_rt(&rt, 0, "");
    let accepted = with_timeout("racing duplicate call ids", LONG, {
        let rt = rt.clone();
        move || {
            let barrier = Arc::new(std::sync::Barrier::new(8));
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let (rt, barrier) = (rt.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        barrier.wait();
                        rt.call(&counter_call_payload(h, FOREVER, 77, &[]))
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|t| t.join().unwrap())
                .filter(|&code| code == 0)
                .count()
        }
    });
    assert_eq!(accepted, 1);
    rt.cancel(77);
    assert!(host.wait_for_replies(1, LONG));
    assert_eq!(
        host.take_replies().len(),
        1,
        "only the accepted call is answered"
    );
    rt.shutdown();
}

/// A tiny deterministic PRNG so failures reproduce.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[derive(Default)]
struct Issued {
    plain: Vec<u32>,
    streams: Vec<u32>,
    cancelled: Vec<u32>,
}

/// One thread of the stress test: a seeded random walk over every host entry point.
fn stress_thread(rt: &Runtime, shared: &[keel_runtime::Handle], thread: u32, ops: usize) -> Issued {
    // `KEEL_STRESS_SEED` varies the walk for soak runs; the default keeps CI reproducible.
    let seed = std::env::var("KEEL_STRESS_SEED")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0x9e37_79b9_7f4a_7c15);
    let mut rng = XorShift(seed ^ u64::from(thread + 1).wrapping_mul(0x2545_f491_4f6c_dd1d));
    let mut next_id = thread * 1_000_000 + 1;
    let mut mine = Issued::default();
    let mut private = new_counter_rt(rt, 0, "private");
    let mut fresh_id = || {
        next_id += 1;
        next_id
    };
    for _ in 0..ops {
        let target = shared[rng.below(shared.len())];
        match rng.below(13) {
            0 => {
                let r = call_sync_rt(rt, counter_target(target, ADD), fresh_id(), &enc(&1_i32));
                assert_eq!(r.status, ReplyStatus::Ok);
            }
            1 => {
                let r = call_sync_rt(
                    rt,
                    counter_target(target, ADD_TWICE),
                    fresh_id(),
                    &enc(&1_i32),
                );
                assert_eq!(r.status, ReplyStatus::Ok);
            }
            2 => {
                let id = fresh_id();
                if rt.call(&counter_call_payload(target, SLOW_ADD, id, &enc(&1_i32))) == 0 {
                    mine.plain.push(id);
                }
            }
            3 => {
                let id = fresh_id();
                if rt.call(&counter_call_payload(target, FOREVER, id, &[])) == 0 {
                    mine.plain.push(id);
                }
            }
            4 | 5 => {
                // Cancel one of this thread's calls, possibly racing its completion.
                if !mine.plain.is_empty() {
                    let id = mine.plain[rng.below(mine.plain.len())];
                    rt.cancel(id);
                    mine.cancelled.push(id);
                }
            }
            6 => {
                let id = fresh_id();
                let n = rng.below(12) as u32;
                if rt.call(&counter_call_payload(target, TICKS, id, &enc(&n))) == 0 {
                    mine.streams.push(id);
                }
            }
            7 => {
                if !mine.streams.is_empty() {
                    let id = mine.streams[rng.below(mine.streams.len())];
                    rt.stream_credit(id, rng.below(5) as u32);
                }
            }
            8 => {
                if !mine.streams.is_empty() && rng.below(3) == 0 {
                    let id = mine.streams[rng.below(mine.streams.len())];
                    rt.cancel(id);
                    mine.cancelled.push(id);
                }
            }
            9 => {
                let signal = match rng.below(3) {
                    2 => u32::MAX,
                    n => n as u32,
                };
                rt.observe(target.0, signal, true);
            }
            10 => {
                // Churn a private, unobserved store: release and recreate, snapshot.
                rt.release(private.0);
                private = new_counter_rt(rt, 0, "private");
                let _ = rt.snapshot();
            }
            11 => {
                let id = fresh_id();
                let call = if rng.below(2) == 0 {
                    BLOCKING_SQUARE
                } else {
                    PANIC_ASYNC
                };
                if rt.call(&counter_call_payload(private, call, id, &enc(&3_i32))) == 0 {
                    mine.plain.push(id);
                }
            }
            _ => {
                let id = fresh_id();
                if rt.call(&counter_call_payload(private, ASK_PORT, id, &enc(&1_i32))) == 0 {
                    mine.plain.push(id);
                }
            }
        }
    }
    rt.release(private.0);
    mine
}

#[test]
fn randomised_mixed_operations_answer_every_call_exactly_once_and_keep_mirrors_exact() {
    const THREADS: u32 = 6;
    let ops: usize = std::env::var("KEEL_STRESS_OPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);
    let (rt, host) = threaded();
    host.script_port_ok(TEST_PORT, ASK, enc(&5_i32));
    let shared: Vec<_> = (0..4).map(|i| new_counter_rt(&rt, i, "shared")).collect();
    for h in &shared {
        rt.observe(h.0, u32::MAX, true);
    }

    let issued = with_timeout("randomised stress", LONG, {
        let (rt, shared) = (rt.clone(), shared.clone());
        move || {
            let handles: Vec<_> = (0..THREADS)
                .map(|thread| {
                    let (rt, shared) = (rt.clone(), shared.clone());
                    std::thread::spawn(move || stress_thread(&rt, &shared, thread, ops))
                })
                .collect();
            let mut all = Issued::default();
            for handle in handles {
                let mine = handle.join().unwrap();
                all.plain.extend(mine.plain);
                all.streams.extend(mine.streams);
                all.cancelled.extend(mine.cancelled);
            }
            all
        }
    });

    // Wind everything down: cancel what never ends, give every stream unlimited credit.
    for &id in &issued.plain {
        rt.cancel(id);
    }
    for &id in &issued.streams {
        rt.stream_credit(id, u32::MAX);
    }
    assert!(
        wait_until(LONG, || {
            let stats: serde_json::Value = serde_json::from_str(&rt.stats_json()).unwrap();
            stats["active_calls"] == 0 && stats["tasks"] == 0 && stats["pending_port_calls"] == 0
        }),
        "everything winds down: {}",
        rt.stats_json()
    );

    // Exactly one terminal reply per accepted plain call; one status 4 per stream.
    let mut by_id: std::collections::BTreeMap<u32, Vec<ReplyStatus>> = Default::default();
    for r in host.take_replies() {
        by_id.entry(r.call_id).or_default().push(r.status);
    }
    for &id in &issued.plain {
        let statuses = &by_id[&id];
        assert_eq!(statuses.len(), 1, "call {id} was answered {statuses:?}");
        assert_ne!(statuses[0], ReplyStatus::StreamOpened);
    }
    for &id in &issued.streams {
        assert_eq!(by_id[&id], [ReplyStatus::StreamOpened], "stream {id}");
    }
    assert_eq!(
        by_id.len(),
        issued.plain.len() + issued.streams.len(),
        "no reply for a call that was never accepted"
    );
    // Streams that were not cancelled ended exactly once; cancelled ones at most once.
    let items = host.take_stream_items();
    for &id in &issued.streams {
        let terminals = items
            .iter()
            .filter(|i| i.call_id == id && i.flag != keel_wire::payload::StreamFlag::Item)
            .count();
        if issued.cancelled.contains(&id) {
            assert!(terminals <= 1, "stream {id}");
        } else {
            assert_eq!(terminals, 1, "stream {id} ends exactly once");
        }
    }

    // The mirror a host builds by applying change-sets in arrival order is exact.
    let mut mirror: std::collections::HashMap<(u64, u32), Vec<u8>> = Default::default();
    for cs in host.take_decoded_change_sets() {
        for e in cs.entries {
            mirror.insert((e.handle.0, e.signal_id), e.value);
        }
    }
    for h in &shared {
        let counter = rt.object::<Counter>(h.0).unwrap();
        assert_eq!(
            mirror[&(h.0, COUNT_SIGNAL)],
            enc(&counter.count.get()),
            "count of {h:?}"
        );
        if let Some(label) = mirror.get(&(h.0, LABEL_SIGNAL)) {
            assert_eq!(label, &enc(&counter.label.get()), "label of {h:?}");
        }
    }
    rt.shutdown();
}

// Silence unused-import lints for helpers only some tests use.
#[allow(dead_code)]
fn _uses(_: CallTarget, _: fn(&mut Reader<'_>) -> Result<u8, keel_wire::WireError>) {
    let _ = <u8 as Decode>::decode;
}
