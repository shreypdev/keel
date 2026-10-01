//! Timers and sleeping: the manual clock, chained sleeps, host-owned timers, cancellation.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use undra_runtime::testing::TestRuntime;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Spawns a task that sleeps for each duration in turn and records `(tag, step)` as it wakes.
fn sleeper(
    t: &TestRuntime,
    tag: &'static str,
    delays: Vec<Duration>,
    log: &Arc<Mutex<Vec<(&'static str, usize)>>>,
) {
    let ctx = t.ctx();
    let log = log.clone();
    t.ctx().spawn(async move {
        for (step, delay) in delays.into_iter().enumerate() {
            ctx.sleep(delay).await;
            log.lock().push((tag, step));
        }
    });
}

#[test]
fn sleep_completes_only_when_time_is_advanced_past_its_deadline() {
    let t = TestRuntime::new();
    let woke = Arc::new(AtomicU32::new(0));
    let (ctx, w) = (t.ctx(), woke.clone());
    t.ctx().spawn(async move {
        ctx.sleep(ms(100)).await;
        w.fetch_add(1, Ordering::SeqCst);
    });
    t.run_pending();
    assert!(t.runtime().stats_json().contains("\"pending_timers\":1"));
    assert_eq!(t.advance(ms(99)), 0);
    assert_eq!(woke.load(Ordering::SeqCst), 0);
    assert_eq!(t.advance(ms(1)), 1);
    assert_eq!(woke.load(Ordering::SeqCst), 1);
    assert_eq!(
        t.advance(Duration::from_secs(10)),
        0,
        "nothing left to fire"
    );
}

#[test]
fn timers_fire_in_deadline_order_regardless_of_arming_order() {
    let t = TestRuntime::new();
    let log = Arc::new(Mutex::new(Vec::new()));
    sleeper(&t, "slow", vec![ms(300)], &log);
    sleeper(&t, "fast", vec![ms(100)], &log);
    sleeper(&t, "mid", vec![ms(200)], &log);
    t.run_pending();
    t.advance(ms(1000));
    let order: Vec<_> = log.lock().iter().map(|e| e.0).collect();
    assert_eq!(order, ["fast", "mid", "slow"]);
}

#[test]
fn equal_deadlines_fire_in_arming_order() {
    let t = TestRuntime::new();
    let log = Arc::new(Mutex::new(Vec::new()));
    for tag in ["a", "b", "c", "d"] {
        sleeper(&t, tag, vec![ms(50)], &log);
    }
    t.run_pending();
    t.advance(ms(50));
    let order: Vec<_> = log.lock().iter().map(|e| e.0).collect();
    assert_eq!(order, ["a", "b", "c", "d"]);
}

#[test]
fn chained_sleeps_are_served_within_one_advance() {
    let t = TestRuntime::new();
    let log = Arc::new(Mutex::new(Vec::new()));
    sleeper(&t, "loop", vec![ms(10), ms(10), ms(10), ms(10)], &log);
    sleeper(&t, "other", vec![ms(25)], &log);
    t.run_pending();
    // 35 ms: the loop wakes at 10, 20, 30 (each re-arms relative to its own wake time),
    // "other" at 25.
    t.advance(ms(35));
    assert_eq!(
        *log.lock(),
        [("loop", 0), ("loop", 1), ("other", 0), ("loop", 2)]
    );
    t.advance(ms(5));
    assert_eq!(log.lock().last(), Some(&("loop", 3)));
}

#[test]
fn a_zero_sleep_is_immediate() {
    let t = TestRuntime::new();
    let ctx = t.ctx();
    t.run_until(async move { ctx.sleep(Duration::ZERO).await });
    assert!(t.runtime().stats_json().contains("\"pending_timers\":0"));
}

#[test]
fn a_sleep_is_measured_from_its_creation_not_from_the_first_poll() {
    let t = TestRuntime::new();
    let sleep = t.ctx().sleep(ms(10));
    t.advance(ms(10)); // due before anybody polled it
    t.run_until(sleep);
}

#[test]
fn dropping_a_sleep_or_cancelling_its_task_cancels_the_timer() {
    let t = TestRuntime::new();
    let sleep = t.ctx().sleep(ms(10));
    assert!(t.runtime().stats_json().contains("\"pending_timers\":1"));
    drop(sleep);
    assert!(t.runtime().stats_json().contains("\"pending_timers\":0"));

    let ctx = t.ctx();
    let woke = Arc::new(AtomicU32::new(0));
    let w = woke.clone();
    let id = t.ctx().spawn(async move {
        ctx.sleep(ms(10)).await;
        w.fetch_add(1, Ordering::SeqCst);
    });
    t.run_pending();
    assert!(t.runtime().stats_json().contains("\"pending_timers\":1"));
    t.ctx().cancel_task(id);
    assert!(t.runtime().stats_json().contains("\"pending_timers\":0"));
    t.advance(ms(100));
    assert_eq!(woke.load(Ordering::SeqCst), 0);
}

#[test]
fn many_concurrent_sleeps_all_complete_with_distinct_timer_ids() {
    let t = TestRuntime::new();
    t.host().set_own_timers(true);
    let woke = Arc::new(AtomicU32::new(0));
    for i in 0..200_u64 {
        let (ctx, w) = (t.ctx(), woke.clone());
        t.ctx().spawn(async move {
            ctx.sleep(ms(1 + i)).await;
            w.fetch_add(1, Ordering::SeqCst);
        });
    }
    t.run_pending();
    let sets = t.host().take_timer_sets();
    assert_eq!(sets.len(), 200);
    let mut ids: Vec<u32> = sets.iter().map(|s| s.0).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 200, "timer ids are unique");
    assert!(ids.iter().all(|&id| id != 0));
    for &(id, _) in &sets {
        t.runtime().timer_fired(id);
    }
    t.run_pending();
    assert_eq!(woke.load(Ordering::SeqCst), 200);
}

// ----- host-owned timers (wasm) -----------------------------------------------------------

#[test]
fn a_host_that_owns_timers_gets_timer_set_and_completes_sleeps_with_timer_fired() {
    let t = TestRuntime::new();
    t.host().set_own_timers(true);
    let woke = Arc::new(AtomicU32::new(0));
    let (ctx, w) = (t.ctx(), woke.clone());
    t.ctx().spawn(async move {
        ctx.sleep(Duration::from_micros(250_500)).await;
        w.fetch_add(1, Ordering::SeqCst);
    });
    t.run_pending();

    // Rounded up to whole milliseconds, and the internal timer was not armed.
    let sets = t.host().take_timer_sets();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].1, 251);
    t.advance(Duration::from_secs(3600));
    assert_eq!(woke.load(Ordering::SeqCst), 0, "the host owns this timer");

    t.runtime().timer_fired(sets[0].0);
    assert_eq!(
        woke.load(Ordering::SeqCst),
        0,
        "timer_fired only wakes; the task runs next turn"
    );
    t.run_pending();
    assert_eq!(woke.load(Ordering::SeqCst), 1);
    // Firing twice, or an unknown id, is harmless.
    t.runtime().timer_fired(sets[0].0);
    t.runtime().timer_fired(424_242);
    t.run_pending();
    assert_eq!(woke.load(Ordering::SeqCst), 1);
}

#[test]
fn a_sleep_with_a_host_timer_that_was_dropped_ignores_the_late_fire() {
    let t = TestRuntime::new();
    t.host().set_own_timers(true);
    let sleep = t.ctx().sleep(ms(5));
    let id = t.host().take_timer_sets()[0].0;
    drop(sleep);
    t.runtime().timer_fired(id);
    assert!(t.runtime().stats_json().contains("\"pending_timers\":0"));
}

#[test]
fn executor_free_functions_use_the_current_runtime() {
    let t = TestRuntime::new();
    let done = Arc::new(AtomicU32::new(0));
    let d = done.clone();
    let _scope = t.ctx().enter();
    undra_runtime::executor::spawn(async move {
        undra_runtime::executor::sleep(ms(5)).await;
        d.fetch_add(1, Ordering::SeqCst);
    });
    t.run_pending();
    t.advance(ms(5));
    assert_eq!(done.load(Ordering::SeqCst), 1);
}

#[test]
fn schedule_is_requested_once_per_turn_by_the_executor() {
    let t = TestRuntime::new();
    let before = t.host().schedule_count();
    for _ in 0..10 {
        t.ctx().spawn(async {});
    }
    assert_eq!(
        t.host().schedule_count(),
        before + 1,
        "ten spawns, one schedule request"
    );
    t.run_pending();
    t.ctx().spawn(async {});
    assert_eq!(
        t.host().schedule_count(),
        before + 2,
        "a new turn asks again"
    );
    t.run_pending();
}

#[test]
fn poll_runs_one_bounded_batch_and_asks_for_more() {
    let t = TestRuntime::new();
    let n = 64 + 10;
    let ran = Arc::new(AtomicU32::new(0));
    for _ in 0..n {
        let r = ran.clone();
        t.ctx().spawn(async move {
            r.fetch_add(1, Ordering::SeqCst);
        });
    }
    let asked = t.host().schedule_count();
    t.runtime().poll();
    assert_eq!(
        ran.load(Ordering::SeqCst),
        64,
        "one turn is at most 64 polls"
    );
    assert_eq!(
        t.host().schedule_count(),
        asked + 1,
        "more work: the host is asked to poll again"
    );
    t.runtime().poll();
    assert_eq!(ran.load(Ordering::SeqCst), n);
    assert_eq!(
        t.host().schedule_count(),
        asked + 1,
        "nothing left, no request"
    );
}
