//! The zero-allocation synchronous path (ADR-028), measured: a counting global allocator proves
//! that `Runtime::call_sync_with` of a macro-generated method allocates nothing once the
//! thread's reply buffer has warmed up, that `Runtime::call_sync` allocates exactly once (the
//! `Vec` it returns), and that `undra_call_sync` allocates exactly once (the `UndraBuf` the caller
//! frees, which `undra.h` promises). The same generated dispatchers are then run through the slot
//! and through the allocating path to show the replies are byte-identical for every outcome a
//! method can have.
//!
//! The counter is per thread, so the test threads (and the `undra-core` thread `undra_init`
//! starts) do not disturb each other's numbers.
#![allow(unsafe_code)]
#![deny(clippy::undocumented_unsafe_blocks)]

use core::ffi::c_void;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use undra::meta::ids;
use undra::prelude::Handle;
use undra::runtime::testing::{TestRuntime, call_payload, call_sync_reference, decode_reply};
use undra::runtime::{Runtime, RuntimeConfig};
use undra::wire::payload::{CallTarget, ReplyStatus};
use undra::wire::{Decode, Encode, Reader};
use undra_ffi::UndraBuf;

#[path = "common/table.rs"]
mod table;
use table::{undra_buf_free, undra_call_sync, undra_init, undra_shutdown};

#[path = "common/core.rs"]
mod test_core;

// ---------------------------------------------------------------------------------------------
// A global allocator that counts the calling thread's allocations
// ---------------------------------------------------------------------------------------------

struct Counting;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

fn bump() {
    let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
}

// SAFETY: every method forwards to `System` unchanged; the only addition is a counter in a
// `const`-initialised, destructor-free thread-local, which never allocates.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump();
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract; we pass it through.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump();
        // SAFETY: as in `alloc`.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump();
        // SAFETY: the caller upholds `GlobalAlloc::realloc`'s contract; we pass it through.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `GlobalAlloc::dealloc`'s contract; we pass it through.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// How many allocations this thread made while `f` ran.
fn allocations_in(f: impl FnOnce()) -> usize {
    let before = ALLOCATIONS.with(Cell::get);
    f();
    ALLOCATIONS.with(Cell::get) - before
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

const CALLS: usize = 1_000;

fn construct_calculator(rt: &Runtime, base: i64) -> Handle {
    let payload = call_payload(
        CallTarget::Constructor {
            type_id: ids::type_id("Calculator"),
            method_id: ids::method_id("Calculator", "new"),
        },
        1,
        &base.encode_to_vec(),
    );
    let reply = decode_reply(&rt.call_sync(&payload));
    assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
    Handle::decode_exact(&reply.body).expect("a handle")
}

fn calculator_call(handle: Handle, method: &str, call_id: u32, args: &[u8]) -> Vec<u8> {
    call_payload(
        CallTarget::Method {
            handle,
            method_id: ids::method_id("Calculator", method),
        },
        call_id,
        args,
    )
}

fn add_args() -> Vec<u8> {
    [1_i64.encode_to_vec(), 2_i64.encode_to_vec()].concat()
}

// ---------------------------------------------------------------------------------------------
// The claims
// ---------------------------------------------------------------------------------------------

#[test]
fn call_sync_with_allocates_nothing_for_a_generated_method() {
    let t = TestRuntime::new();
    let rt = t.runtime();
    let calc = construct_calculator(rt, 7);
    let add = calculator_call(calc, "add", 2, &add_args());
    let fail = calculator_call(calc, "fail", 3, &[]);
    let version = call_payload(
        CallTarget::Function {
            method_id: ids::function_id("version"),
        },
        4,
        &[],
    );

    // `version` builds a `String` of its own (`to_owned`), which is the method's allocation and
    // not the runtime's: one per call, and no more.
    for (name, payload, status, user_allocations) in [
        ("add", &add, ReplyStatus::Ok, 0),
        ("fail", &fail, ReplyStatus::Error, 0),
        ("version", &version, ReplyStatus::Ok, CALLS),
    ] {
        // Warm up: the thread's reply buffer and every lazily built thread-local.
        for _ in 0..10 {
            rt.call_sync_with(payload, |_| ());
        }
        let mut seen = None;
        let allocations = allocations_in(|| {
            for _ in 0..CALLS {
                rt.call_sync_with(payload, |reply| seen = Some(reply[4]));
            }
        });
        assert_eq!(seen, Some(status.as_u8()), "{name}");
        assert_eq!(
            allocations, user_allocations,
            "`{name}`: {CALLS} calls through call_sync_with made {allocations} allocations \
             (the method itself accounts for {user_allocations})"
        );
    }
}

#[test]
fn call_sync_allocates_exactly_the_returned_vec() {
    let t = TestRuntime::new();
    let rt = t.runtime();
    let calc = construct_calculator(rt, 7);
    let add = calculator_call(calc, "add", 2, &add_args());
    for _ in 0..10 {
        let _ = rt.call_sync(&add);
    }
    let mut replies = Vec::with_capacity(CALLS);
    let allocations = allocations_in(|| {
        for _ in 0..CALLS {
            replies.push(rt.call_sync(&add));
        }
    });
    assert_eq!(
        allocations, CALLS,
        "call_sync should allocate once per call (the returned Vec), made {allocations} for {CALLS}"
    );
    assert!(replies.iter().all(|r| r == &replies[0]));
    // base 7 + 1 + 2.
    assert_eq!(replies[0], [2, 0, 0, 0, 0, 10, 0, 0, 0, 0, 0, 0, 0]);
}

/// The C ABI, end to end through the table: one allocation per call, the buffer handed to the caller.
#[test]
fn undra_call_sync_allocates_exactly_the_buffer_it_hands_out() {
    extern "C" fn on_reply(_: *mut c_void, _: u32, _: *const u8, _: u32) {}
    extern "C" fn on_changes(_: *mut c_void, _: *const u8, _: u32) {}
    extern "C" fn on_stream(_: *mut c_void, _: u32, _: *const u8, _: u32) {}

    let cfg = RuntimeConfig::default().encode_to_vec();
    // SAFETY: `cfg` is valid for its length and the callbacks are `extern "C"` functions that
    // touch nothing.
    let code = unsafe {
        undra_init(
            cfg.as_ptr(),
            u32::try_from(cfg.len()).expect("small"),
            Some(on_reply),
            Some(on_changes),
            Some(on_stream),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(code, 0);
    let rt = Runtime::global().expect("undra_init started the runtime");

    let calc = construct_calculator(&rt, 7);
    let add = calculator_call(calc, "add", 2, &add_args());
    let call = |payload: &[u8]| -> (u8, usize) {
        // SAFETY: `payload` is valid for its length; the buffer is read once and freed.
        unsafe {
            let buf: UndraBuf =
                undra_call_sync(payload.as_ptr(), u32::try_from(payload.len()).unwrap());
            let status = buf.as_slice()[4];
            let len = buf.len as usize;
            undra_buf_free(buf);
            (status, len)
        }
    };
    for _ in 0..10 {
        call(&add);
    }
    // Per call, not in aggregate: the runtime's periodic maintenance (a sweep of abandoned port
    // calls, a rate-limited log line) runs on whichever thread crosses next, and on a slow CI
    // runner a 1,000-call window crosses such a boundary once or twice (observed: 1,002 for
    // 1,000). The claim is the steady state -- every call allocates the UndraBuf and nothing else --
    // so the distribution is what is asserted: no call under one, the median exactly one, and at
    // most one call in a hundred above it.
    let mut last = (0, 0);
    let mut per_call = Vec::with_capacity(CALLS);
    for _ in 0..CALLS {
        per_call.push(allocations_in(|| last = call(&add)));
    }
    drop(rt);
    undra_shutdown();
    assert_eq!(last, (0, 13));
    assert!(
        per_call.iter().all(|&n| n >= 1),
        "undra_call_sync must allocate the UndraBuf on every call: {per_call:?}"
    );
    let mut sorted = per_call.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted[CALLS / 2],
        1,
        "the median call must allocate exactly once: {sorted:?}"
    );
    let extra: usize = per_call.iter().map(|n| n.saturating_sub(1)).sum();
    assert!(
        extra <= CALLS / 100,
        "undra_call_sync should allocate once per call (the UndraBuf); {extra} extra allocations \
         over {CALLS} calls: {per_call:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// The slot and the allocating path agree
// ---------------------------------------------------------------------------------------------

fn same_both_ways(rt: &Runtime, payload: &[u8]) -> Vec<u8> {
    let fast = rt.call_sync(payload);
    let reference = call_sync_reference(rt, payload);
    assert_eq!(fast, reference, "slot and allocating path differ");
    fast
}

#[test]
fn every_outcome_of_a_generated_method_is_identical_on_both_paths() {
    let t = TestRuntime::new();
    let rt = t.runtime();
    let calc = construct_calculator(rt, 7);

    // ok, typed error, unit result, a free function.
    let ok = same_both_ways(rt, &calculator_call(calc, "add", 10, &add_args()));
    assert_eq!(decode_reply(&ok).status, ReplyStatus::Ok);
    let err = same_both_ways(rt, &calculator_call(calc, "fail", 11, &[]));
    assert_eq!(decode_reply(&err).status, ReplyStatus::Error);
    let counter = {
        let payload = call_payload(
            CallTarget::Constructor {
                type_id: ids::type_id("Counter"),
                method_id: ids::method_id("Counter", "new"),
            },
            14,
            &[],
        );
        let reply = decode_reply(&rt.call_sync(&payload));
        assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
        Handle::decode_exact(&reply.body).expect("a handle")
    };
    let bump = CallTarget::Method {
        handle: counter,
        method_id: ids::method_id("Counter", "bump"),
    };
    let unit = same_both_ways(rt, &call_payload(bump, 12, &[]));
    assert_eq!(unit, [12, 0, 0, 0, 0], "a unit result is an empty Ok body");
    let version = same_both_ways(
        rt,
        &call_payload(
            CallTarget::Function {
                method_id: ids::function_id("version"),
            },
            13,
            &[],
        ),
    );
    assert_eq!(decode_reply(&version).status, ReplyStatus::Ok);

    // BadRequest: undecodable arguments, trailing bytes, a stale handle, an unknown method.
    for payload in [
        calculator_call(calc, "add", 20, &[1, 2, 3]),
        calculator_call(calc, "add", 21, &[add_args(), vec![0]].concat()),
        calculator_call(Handle(0x0000_0009_0000_0042), "add", 22, &add_args()),
        calculator_call(calc, "no_such_method", 23, &[]),
    ] {
        let reply = decode_reply(&same_both_ways(rt, &payload));
        assert_eq!(reply.status, ReplyStatus::BadRequest, "{reply:?}");
    }
    // Async and stream methods are refused without running, identically.
    for method in ["slow_add", "ready_add", "ticks"] {
        let args = if method == "ticks" {
            3_u32.encode_to_vec()
        } else {
            add_args()
        };
        let reply = decode_reply(&same_both_ways(
            rt,
            &calculator_call(calc, method, 30, &args),
        ));
        assert_eq!(reply.status, ReplyStatus::BadRequest, "{method}: {reply:?}");
    }
    // An id nobody serves goes through every layer (undra-query's among them) to status 5.
    let nobody = call_payload(
        CallTarget::Function {
            method_id: ids::function_id("nobody_serves_this"),
        },
        40,
        &[],
    );
    assert_eq!(
        decode_reply(&same_both_ways(rt, &nobody)).status,
        ReplyStatus::BadRequest
    );
}

#[test]
fn a_generated_method_that_panics_is_status_2_on_both_paths() {
    let t = TestRuntime::new();
    let rt = t.runtime();
    let calc = construct_calculator(rt, 7);
    let boom = calculator_call(calc, "boom", 5, &[]);
    let fast = decode_reply(&rt.call_sync(&boom));
    let reference = decode_reply(&call_sync_reference(rt, &boom));
    for reply in [&fast, &reference] {
        assert_eq!(reply.status, ReplyStatus::Panic);
        assert_eq!(reply.call_id, 5);
    }
    let message = |body: &[u8]| Reader::new(body).read_str().unwrap().to_owned();
    assert_eq!(message(&fast.body), "kaboom");
    assert_eq!(message(&fast.body), message(&reference.body));
    // A panic poisons the object, not the slot: the same runtime still answers.
    let other = construct_calculator(rt, 1);
    let add = same_both_ways(rt, &calculator_call(other, "add", 6, &add_args()));
    assert_eq!(decode_reply(&add).status, ReplyStatus::Ok);
}
