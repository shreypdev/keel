//! The promises `keel.h` makes to a C host about its callbacks and its `user` pointers, checked
//! from the host's side (constitution R6; ADR-025):
//!
//! * a port callback never runs, and its `user` is never read, after `keel_port_register(id, NULL)`,
//!   a replacing `keel_port_register` or `keel_shutdown` has returned (the review's H1 use after
//!   free); the same tests are the AddressSanitizer regression when this binary is built with
//!   `-Z sanitizer=address`, because the host really frees `user` the moment the call returns;
//! * callbacks may run concurrently, on the threads that produced the event;
//! * `keel_init` does not overtake a shutdown in progress (L5);
//! * an answer to the fire-and-forget Log call (port call id 0) is dropped, never logged (M4).
//!
//! The runtime is one per process, so the tests take turns (`Turn`). Nothing here needs a core:
//! the Log port is reached through the runtime's own warnings (a malformed `keel_port_reply`),
//! which also keeps this binary free of the `inventory` constructors other test binaries link.
#![deny(clippy::undocumented_unsafe_blocks)]

use core::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use keel_ffi::{
    KeelBuf, init_code, keel_init, keel_port_register, keel_port_reply, keel_shutdown,
    keel_stats_json,
};

/// `fnv1a32("port.Log")`: the standard Log port (SPEC 1.1, 8).
const LOG_PORT: u32 = 0x575f_f24a;

/// What a host keeps behind its `user` pointer.
#[derive(Default)]
struct Host {
    /// Invocations of the Log port callback that have started / finished.
    entered: AtomicU32,
    finished: AtomicU32,
    /// Invocations running right now, and the most there ever were at once.
    running: AtomicU32,
    max_running: AtomicU32,
    /// How long each callback holds (milliseconds): the window a late unregister races against.
    hold_ms: AtomicU32,
    /// What the callback answers (`0`, `1` or `2`; `0` needs no reply body: it is the test's
    /// business only for port call id 0, whose answer the core ignores).
    answer: AtomicU32,
    /// Set by the test once it treats the host as gone; a callback that notices it is a use after
    /// release. (Tests that really free `user` cannot look at a flag inside it.)
    released: AtomicBool,
    violations: AtomicU32,
    /// A gate the first invocation waits at until the test opens it.
    gate: Gate,
    /// When set, the callback unregisters its own port (a contract violation, tested below).
    unregister_self: AtomicBool,
}

#[derive(Default)]
struct Gate {
    armed: AtomicBool,
    open: Mutex<bool>,
    changed: Condvar,
}

impl Gate {
    fn arm(&self) {
        *self.open.lock().unwrap_or_else(PoisonError::into_inner) = false;
        self.armed.store(true, Ordering::Release);
    }

    fn open(&self) {
        *self.open.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.changed.notify_all();
    }

    /// Blocks while the gate is armed and closed; only the first caller is held.
    fn pass(&self) {
        if !self.armed.swap(false, Ordering::AcqRel) {
            return;
        }
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        while !*open {
            open = self
                .changed
                .wait(open)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// The Log port callback of a test host.
extern "C" fn log_port(
    user: *mut c_void,
    port_id: u32,
    _method_id: u32,
    _port_call_id: u32,
    _ptr: *const u8,
    _len: u32,
    _out: *mut KeelBuf,
) -> u8 {
    // SAFETY: `user` is the `Host` the test registered; the contract under test is precisely that
    // it stays valid until the registration's removal has returned.
    let host = unsafe { &*user.cast::<Host>() };
    if port_id != LOG_PORT {
        return 2;
    }
    host.entered.fetch_add(1, Ordering::AcqRel);
    let now = host.running.fetch_add(1, Ordering::AcqRel) + 1;
    host.max_running.fetch_max(now, Ordering::AcqRel);
    if host.released.load(Ordering::Acquire) {
        host.violations.fetch_add(1, Ordering::AcqRel);
    }
    host.gate.pass();
    if host.unregister_self.swap(false, Ordering::AcqRel) {
        // SAFETY: a null callback only removes the registration (and is what is being tested).
        unsafe { keel_port_register(LOG_PORT, None, core::ptr::null_mut()) };
    }
    let hold = host.hold_ms.load(Ordering::Acquire);
    if hold > 0 {
        thread::sleep(Duration::from_millis(u64::from(hold)));
    }
    if host.released.load(Ordering::Acquire) {
        host.violations.fetch_add(1, Ordering::AcqRel);
    }
    host.running.fetch_sub(1, Ordering::AcqRel);
    host.finished.fetch_add(1, Ordering::AcqRel);
    // `answer` is 1 or 2 here: a Log answer of 0 would need a reply block, and the core ignores it.
    host.answer.load(Ordering::Acquire) as u8
}

extern "C" fn on_reply(_: *mut c_void, _: u32, _: *const u8, _: u32) {}
extern "C" fn on_changes(_: *mut c_void, _: *const u8, _: u32) {}
extern "C" fn on_stream(_: *mut c_void, _: u32, _: *const u8, _: u32) {}

static SERIAL: Mutex<()> = Mutex::new(());

/// One test's turn at the process-global runtime; shuts it down on the way out.
struct Turn(#[allow(dead_code)] MutexGuard<'static, ()>);

impl Turn {
    fn take() -> Turn {
        let turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        keel_shutdown();
        Turn(turn)
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        keel_shutdown();
    }
}

/// `RuntimeConfig { platform: "test", mode: "inproc", core_threads: 1, blocking_threads: 1,
/// log_level: 2 }` in the wire format (SPEC 3.1): two `String`s and three bytes. The test links
/// no core, so it needs no more of the workspace than the library itself.
fn config() -> Vec<u8> {
    let mut cfg = Vec::new();
    for text in ["test", "inproc"] {
        cfg.extend_from_slice(&u32::try_from(text.len()).expect("small").to_le_bytes());
        cfg.extend_from_slice(text.as_bytes());
    }
    cfg.extend_from_slice(&[1, 1, 2]);
    cfg
}

fn init(user: *mut c_void) -> u32 {
    let cfg = config();
    // SAFETY: `cfg` is valid for its length; the callbacks are `extern "C"` functions that touch
    // nothing, so any `user` is fine.
    unsafe {
        keel_init(
            cfg.as_ptr(),
            u32::try_from(cfg.len()).expect("small"),
            Some(on_reply),
            Some(on_changes),
            Some(on_stream),
            user,
        )
    }
}

fn register_log(host: *const Host) {
    // SAFETY: `log_port` is an `extern "C"` function; the test keeps `host` valid as the contract
    // under test requires (until the registration is removed or `keel_shutdown` returns).
    unsafe { keel_port_register(LOG_PORT, Some(log_port), host.cast_mut().cast()) };
}

fn unregister_log() {
    // SAFETY: a null callback only removes the registration.
    unsafe { keel_port_register(LOG_PORT, None, core::ptr::null_mut()) };
}

/// Makes the runtime log a warning (a malformed port reply), which reaches the Log port
/// callback on the calling thread, without the core lock.
fn provoke_a_log_record() {
    let junk = [1_u8, 2];
    // SAFETY: `junk` is valid for its length.
    unsafe { keel_port_reply(junk.as_ptr(), 2) };
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(1));
    }
}

/// Runs `remove` (which must not return before the callback running on another thread has)
/// while a Log callback is mid-flight, frees the host the instant it returns, and checks the
/// callback had finished.
fn the_host_may_free_user_once_removal_returns(remove: impl FnOnce()) {
    let host = Box::into_raw(Box::new(Host::default()));
    // SAFETY: just created by `Box::into_raw`, freed only below.
    unsafe { (*host).hold_ms.store(200, Ordering::Release) };
    let _turn = Turn::take();
    register_log(host);
    assert_eq!(init(core::ptr::null_mut()), init_code::OK);

    let caller = thread::spawn(provoke_a_log_record);
    // SAFETY: `host` is live until the `Box::from_raw` below.
    wait_until("the callback to start", || unsafe {
        (*host).entered.load(Ordering::Acquire) >= 1
    });

    remove();

    // The removal returned: nothing may still be running the callback or read the host again.
    // SAFETY: `host` is still live; the read happens before it is freed.
    let (finished, running) = unsafe {
        (
            (*host).finished.load(Ordering::Acquire),
            (*host).running.load(Ordering::Acquire),
        )
    };
    // SAFETY: from `Box::into_raw` above, freed exactly once. The callback of the old code is
    // still asleep at this point and touches the freed host when it wakes: what ASan reports.
    drop(unsafe { Box::from_raw(host) });
    // Joined before the assertion, so the old code's late write reaches the freed host first.
    caller.join().expect("the caller thread finished");
    assert_eq!(
        (finished, running),
        (1, 0),
        "the removal returned while the callback was still running"
    );
}

#[test]
fn unregistering_a_port_waits_for_its_running_callback() {
    the_host_may_free_user_once_removal_returns(unregister_log);
}

#[test]
fn replacing_a_port_waits_for_the_old_callback() {
    let replacement = Box::leak(Box::new(Host::default()));
    replacement.answer.store(2, Ordering::Release);
    let replacement: &Host = replacement;
    the_host_may_free_user_once_removal_returns(|| register_log(replacement));
    // The replacement is live from then on (leaked on purpose; the runtime is shut down by `Turn`).
}

#[test]
fn shutdown_waits_for_running_port_callbacks() {
    the_host_may_free_user_once_removal_returns(|| keel_shutdown());
}

/// Four threads race registrations against callbacks: a callback that starts after the
/// registration is gone, or that is still running when the host marks itself released, is a
/// violation. (The hosts are never freed here, so a violation is an assertion rather than UB.)
#[test]
fn no_callback_outlives_its_registration_under_load() {
    let _turn = Turn::take();
    assert_eq!(init(core::ptr::null_mut()), init_code::OK);
    let stop = AtomicBool::new(false);
    let hosts: Vec<&'static Host> = (0..40)
        .map(|_| &*Box::leak(Box::new(Host::default())))
        .collect();
    thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                while !stop.load(Ordering::Acquire) {
                    provoke_a_log_record();
                }
            });
        }
        for host in &hosts {
            host.answer.store(2, Ordering::Release);
            host.hold_ms.store(1, Ordering::Release);
            register_log(*host);
            thread::sleep(Duration::from_millis(3));
            unregister_log();
            // The host is gone from the contract's point of view.
            host.released.store(true, Ordering::Release);
            thread::sleep(Duration::from_millis(2));
        }
        stop.store(true, Ordering::Release);
    });
    let used = hosts
        .iter()
        .filter(|host| host.entered.load(Ordering::Acquire) > 0)
        .count();
    assert!(used > 5, "the load did reach the callbacks ({used} of 40)");
    for host in hosts {
        assert_eq!(host.violations.load(Ordering::Acquire), 0);
        assert_eq!(
            host.entered.load(Ordering::Acquire),
            host.finished.load(Ordering::Acquire)
        );
    }
}

/// Unregistering from inside the registration's own callback would wait for itself. Debug builds
/// refuse loudly (the contained assertion of ADR-025); release builds skip the wait. Either way
/// it returns, and the registration is gone.
#[test]
fn unregistering_from_inside_its_own_callback_returns_instead_of_deadlocking() {
    let _turn = Turn::take();
    let host = Box::leak(Box::new(Host::default()));
    host.answer.store(2, Ordering::Release);
    host.unregister_self.store(true, Ordering::Release);
    register_log(host);
    assert_eq!(init(core::ptr::null_mut()), init_code::OK);

    let (tx, rx) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        provoke_a_log_record();
        tx.send(()).expect("the test is listening");
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("the callback returned: no self-deadlock");
    worker.join().expect("no panic escaped the boundary");
    assert_eq!(host.finished.load(Ordering::Acquire), 1);

    // The registration is gone: a further record does not reach the host.
    provoke_a_log_record();
    assert_eq!(host.entered.load(Ordering::Acquire), 1);
}

/// `keel.h` says callbacks run concurrently and must be thread-safe. Hold four of them at once.
#[test]
fn port_callbacks_run_concurrently_on_the_threads_that_produced_the_event() {
    let _turn = Turn::take();
    let host = Box::leak(Box::new(Host::default()));
    host.answer.store(2, Ordering::Release);
    host.hold_ms.store(150, Ordering::Release);
    register_log(host);
    assert_eq!(init(core::ptr::null_mut()), init_code::OK);
    thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(provoke_a_log_record);
        }
    });
    assert_eq!(host.finished.load(Ordering::Acquire), 4);
    assert!(
        host.max_running.load(Ordering::Acquire) >= 2,
        "the callbacks were serialized: {} at a time",
        host.max_running.load(Ordering::Acquire)
    );
}

/// L5: a `keel_init` that arrives while a shutdown is still draining waits for it, so a host
/// that registers its ports after `keel_init` returns never has them wiped by that shutdown.
#[test]
fn init_waits_for_a_shutdown_that_is_draining_port_callbacks() {
    let _turn = Turn::take();
    let old = Box::leak(Box::new(Host::default()));
    old.answer.store(2, Ordering::Release);
    old.gate.arm();
    register_log(old);
    assert_eq!(init(core::ptr::null_mut()), init_code::OK);

    // A callback is now in flight, held at the gate.
    let caller = thread::spawn(provoke_a_log_record);
    wait_until("the callback to start", || {
        old.entered.load(Ordering::Acquire) >= 1
    });

    let shutdown_done = AtomicBool::new(false);
    let init_done = AtomicBool::new(false);
    thread::scope(|scope| {
        scope.spawn(|| {
            keel_shutdown();
            shutdown_done.store(true, Ordering::Release);
        });
        thread::sleep(Duration::from_millis(150));
        assert!(
            !shutdown_done.load(Ordering::Acquire),
            "shutdown returned with a callback in flight"
        );
        scope.spawn(|| {
            assert_eq!(init(core::ptr::null_mut()), init_code::OK);
            init_done.store(true, Ordering::Release);
        });
        thread::sleep(Duration::from_millis(150));
        assert!(
            !init_done.load(Ordering::Acquire),
            "keel_init overtook a shutdown that had not finished"
        );
        old.gate.open();
    });
    caller.join().expect("the caller finished");
    assert!(shutdown_done.load(Ordering::Acquire) && init_done.load(Ordering::Acquire));

    // A port registered after that init is live: it is not wiped by the earlier shutdown.
    let new = Box::leak(Box::new(Host::default()));
    new.answer.store(2, Ordering::Release);
    register_log(new);
    provoke_a_log_record();
    assert_eq!(new.entered.load(Ordering::Acquire), 1);
    assert_eq!(
        old.entered.load(Ordering::Acquire),
        1,
        "the old host is gone"
    );
}

/// L5 as a race: init-and-register against shutdown, many times. Nothing deadlocks, nothing
/// panics, and afterwards a fresh init with a registration works.
#[test]
fn init_racing_shutdown_leaves_a_consistent_process() {
    let _turn = Turn::take();
    let hosts: Vec<&'static Host> = (0..200)
        .map(|_| &*Box::leak(Box::new(Host::default())))
        .collect();
    let stop = AtomicBool::new(false);
    thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(Ordering::Acquire) {
                keel_shutdown();
                thread::yield_now();
            }
        });
        for host in &hosts {
            host.answer.store(2, Ordering::Release);
            let code = init(core::ptr::null_mut());
            assert!(
                code == init_code::OK || code == init_code::ALREADY_INITIALIZED,
                "init answered {code}"
            );
            register_log(*host);
            provoke_a_log_record();
        }
        stop.store(true, Ordering::Release);
    });
    keel_shutdown();
    let host = Box::leak(Box::new(Host::default()));
    host.answer.store(2, Ordering::Release);
    assert_eq!(init(core::ptr::null_mut()), init_code::OK);
    register_log(host);
    provoke_a_log_record();
    assert_eq!(host.entered.load(Ordering::Acquire), 1);
}

/// M4: the Log port is fire and forget (port call id 0). A host that answers it asynchronously
/// and later replies with id 0 must not make the core log "no port call 0 is pending", which
/// used to be another Log call, answered the same way, without end.
#[test]
fn a_late_answer_to_a_log_record_is_dropped_not_logged() {
    let _turn = Turn::take();
    let host = Box::leak(Box::new(Host::default()));
    host.answer.store(1, Ordering::Release); // "I will reply later"
    register_log(host);
    assert_eq!(init(core::ptr::null_mut()), init_code::OK);

    provoke_a_log_record();
    assert_eq!(
        host.entered.load(Ordering::Acquire),
        1,
        "the warning itself"
    );
    // The late reply to port call 0: status ok, empty body.
    let late = [0_u8, 0, 0, 0, 0];
    // SAFETY: `late` is valid for its length.
    unsafe { keel_port_reply(late.as_ptr(), 5) };
    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        host.entered.load(Ordering::Acquire),
        1,
        "the reply to id 0 produced another Log call"
    );
    // A reply to an id that really is unknown is still a warning (it was not silenced globally).
    let unknown = [0xEF_u8, 0xBE, 0xAD, 0xDE, 0];
    // SAFETY: `unknown` is valid for its length.
    unsafe { keel_port_reply(unknown.as_ptr(), 5) };
    assert_eq!(host.entered.load(Ordering::Acquire), 2);
}

/// The registration made by a host before `keel_init` applies once the runtime is up, and stats
/// stay readable from any thread while all this goes on (a smoke test of the plain path).
#[test]
fn a_port_registered_before_init_is_served_after_it() {
    let _turn = Turn::take();
    let host = Box::leak(Box::new(Host::default()));
    host.answer.store(2, Ordering::Release);
    register_log(host);
    assert_eq!(init(core::ptr::null_mut()), init_code::OK);
    provoke_a_log_record();
    assert_eq!(host.entered.load(Ordering::Acquire), 1);
    let stats = keel_stats_json();
    // SAFETY: a buffer the core returned, read and freed once.
    unsafe {
        assert!(stats.as_slice().starts_with(b"{"));
        keel_ffi::keel_buf_free(stats);
    }
}
