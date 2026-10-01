//! ADR-034 for the query layer: its tasks hold a `WeakCtx`, so neither hydration (which retries
//! for seconds while a platform registers `Kv` late) nor a pending garbage collection (5 minutes by
//! default) keeps a runtime alive after its owner lets go. Before the change both did (gap LC-3:
//! an idle runtime lived 6 to 8 s past its owner, a released handle pinned it for `gc_ms`).
//!
//! The thread count is process-wide, so the tests here take [`SERIAL`].

use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use undra::runtime::testing::{RecordingHost, live_threads};
use undra::runtime::{Ctx, Runtime, RuntimeConfig};
use undra_query::{BoxFuture, CtxQuery, QueryDef};

static SERIAL: Mutex<()> = Mutex::new(());

struct Answer;

impl QueryDef for Answer {
    const ID: u32 = 0x5157_0001;
    const KEY: &'static str = "lifecycle:answer";
    const STALE_MS: Option<u64> = Some(60_000);
    const PERSIST: bool = false;
    const RETRY: u32 = 0;
    type Params = ();
    type Output = u32;
    type Error = String;
    fn fetch(_: Ctx, _: ()) -> BoxFuture<Result<u32, String>> {
        Box::pin(async { Ok(42) })
    }
}

/// A threaded runtime with no ports at all: `Kv` is unavailable, so hydration keeps retrying.
fn threaded() -> Arc<Runtime> {
    Runtime::new(
        RuntimeConfig {
            platform: "test".to_owned(),
            mode: "inproc".to_owned(),
            core_threads: 1,
            blocking_threads: 1,
            log_level: 0,
        },
        Arc::new(RecordingHost::new()),
    )
    .expect("a runtime")
}

fn drop_owner_and_expect_release(rt: Arc<Runtime>, threads_before: usize) {
    let weak: Weak<Runtime> = Arc::downgrade(&rt);
    drop(rt);
    let deadline = Instant::now() + Duration::from_millis(100);
    while weak.upgrade().is_some() || live_threads() > threads_before {
        assert!(
            Instant::now() < deadline,
            "the runtime is still alive 100 ms after its owner let go (alive: {}, threads: {})",
            weak.upgrade().is_some(),
            live_threads()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn lc3_an_idle_runtime_still_hydrating_is_released_when_its_owner_lets_go() {
    let _serial = SERIAL.lock();
    let threads_before = live_threads();
    let rt = threaded();
    // Hydration is retrying an unavailable `Kv` (every 100 ms for about five seconds).
    std::thread::sleep(Duration::from_millis(30));
    drop_owner_and_expect_release(rt, threads_before);
}

#[test]
fn lc3_a_released_query_handle_with_its_collection_pending_does_not_pin_the_runtime() {
    let _serial = SERIAL.lock();
    let threads_before = live_threads();
    let rt = threaded();
    let ctx = rt.ctx();
    let handle = ctx.query().observe::<Answer>(());
    let deadline = Instant::now() + Duration::from_secs(5);
    while handle.data().get().is_none() {
        assert!(Instant::now() < deadline, "the fetch never landed");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(handle.data().get(), Some(42));
    // Unobserved: the entry is collected `gc_ms` (5 minutes) from now, by a task that sleeps.
    drop(handle);
    assert_eq!(ctx.query().cached_entries(), 1);
    drop(ctx);
    drop_owner_and_expect_release(rt, threads_before);
}

#[test]
fn a_query_handle_keeps_working_while_the_runtime_lives_and_goes_quiet_after() {
    let _serial = SERIAL.lock();
    let rt = threaded();
    let handle = rt.ctx().query().observe::<Answer>(());
    let deadline = Instant::now() + Duration::from_secs(5);
    while handle.data().get().is_none() {
        assert!(Instant::now() < deadline, "the fetch never landed");
        std::thread::sleep(Duration::from_millis(1));
    }
    handle.refetch();
    handle.invalidate();
    rt.shutdown();
    // After shutdown the handle holds no runtime to reach: these are quiet no-ops.
    handle.refetch();
    handle.invalidate();
    drop(handle);
}
