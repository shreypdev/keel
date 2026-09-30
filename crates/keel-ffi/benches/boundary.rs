//! Cost of crossing the C ABI (SPEC 6, constitution R4): what a Swift or Kotlin call pays for
//! the boundary itself, with a trivial method behind it.
//!
//! * `call_sync/add`: `keel_call_sync` of a sync method, payload prebuilt, reply buffer freed;
//! * `call_sync/unknown`: the status 5 path (decode, lookup, reply);
//! * `call/add`: `keel_call` of a sync method; the reply callback runs on this thread;
//! * `call/ready_add`: `keel_call` of an `async` method that is ready at once: the executor
//!   hop (spawn, core thread wake, poll, reply callback on the core thread);
//! * `write_observed`: a store write with one observer: `keel_call_sync` plus the change-set
//!   callback.
#![allow(unsafe_code)]

use core::ffi::c_void;
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{Criterion, criterion_group, criterion_main};
use keel::meta::ids;
use keel::prelude::Handle;
use keel::runtime::RuntimeConfig;
use keel::wire::payload::{Call, CallTarget};
use keel::wire::{Decode, Encode, Writer};
use keel_ffi::{keel_buf_free, keel_call, keel_call_sync, keel_init, keel_observe, keel_shutdown};

#[path = "../tests/common/core.rs"]
mod test_core;

static REPLIES: AtomicU64 = AtomicU64::new(0);
static CHANGES: AtomicU64 = AtomicU64::new(0);

extern "C" fn on_reply(_user: *mut c_void, _call_id: u32, _ptr: *const u8, _len: u32) {
    REPLIES.fetch_add(1, Ordering::Release);
}

extern "C" fn on_changes(_user: *mut c_void, _ptr: *const u8, _len: u32) {
    CHANGES.fetch_add(1, Ordering::Release);
}

extern "C" fn on_stream(_user: *mut c_void, _call_id: u32, _ptr: *const u8, _len: u32) {}

fn payload(target: CallTarget, call_id: u32, args: &[u8]) -> Vec<u8> {
    let mut w = Writer::new();
    Call {
        target,
        call_id,
        args,
    }
    .encode(&mut w);
    w.into_vec()
}

fn call_sync(payload: &[u8]) -> usize {
    // SAFETY: `payload` is valid for its length; the returned buffer is read once and freed.
    unsafe {
        let buf = keel_call_sync(payload.as_ptr(), payload.len() as u32);
        let len = buf.len as usize;
        keel_buf_free(buf);
        len
    }
}

fn construct(type_name: &str, args: &[u8]) -> Handle {
    let p = payload(
        CallTarget::Constructor {
            type_id: ids::type_id(type_name),
            method_id: ids::method_id(type_name, "new"),
        },
        1,
        args,
    );
    // SAFETY: as in `call_sync`.
    let body = unsafe {
        let buf = keel_call_sync(p.as_ptr(), p.len() as u32);
        let reply = buf.as_slice()[5..].to_vec();
        keel_buf_free(buf);
        reply
    };
    Handle::decode_exact(&body).expect("a handle")
}

fn boundary(c: &mut Criterion) {
    let cfg = RuntimeConfig::default().encode_to_vec();
    // SAFETY: `cfg` is valid for its length and the callbacks are `extern "C"` functions.
    let code = unsafe {
        keel_init(
            cfg.as_ptr(),
            cfg.len() as u32,
            Some(on_reply),
            Some(on_changes),
            Some(on_stream),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(code, 0);

    let calc = construct("Calculator", &7_i64.encode_to_vec());
    let args = [1_i64.encode_to_vec(), 2_i64.encode_to_vec()].concat();
    let target = |name: &str| CallTarget::Method {
        handle: calc,
        method_id: ids::method_id("Calculator", name),
    };
    let add = payload(target("add"), 2, &args);
    let unknown = payload(
        CallTarget::Function {
            method_id: 0xDEAD_BEEF,
        },
        3,
        &[],
    );

    let mut group = c.benchmark_group("boundary");
    group.bench_function("call_sync/add", |b| b.iter(|| call_sync(black_box(&add))));
    group.bench_function("call_sync/unknown", |b| {
        b.iter(|| call_sync(black_box(&unknown)));
    });

    group.bench_function("call/add", |b| {
        let mut id = 10_u32;
        b.iter(|| {
            id = id.wrapping_add(1).max(10);
            let p = payload(target("add"), id, &args);
            let before = REPLIES.load(Ordering::Acquire);
            // SAFETY: `p` is valid for its length.
            assert_eq!(unsafe { keel_call(p.as_ptr(), p.len() as u32) }, 0);
            assert!(
                REPLIES.load(Ordering::Acquire) > before,
                "synchronous reply"
            );
        });
    });

    group.bench_function("call/ready_add", |b| {
        let mut id = 1_000_000_u32;
        b.iter(|| {
            id = id.wrapping_add(1).max(1_000_000);
            let p = payload(target("ready_add"), id, &args);
            let before = REPLIES.load(Ordering::Acquire);
            // SAFETY: `p` is valid for its length.
            assert_eq!(unsafe { keel_call(p.as_ptr(), p.len() as u32) }, 0);
            while REPLIES.load(Ordering::Acquire) == before {
                std::hint::spin_loop();
            }
        });
    });

    let counter = construct("Counter", &[]);
    keel_observe(counter.0, u32::MAX, 1);
    let bump = payload(
        CallTarget::Method {
            handle: counter,
            method_id: ids::method_id("Counter", "bump"),
        },
        4,
        &[],
    );
    group.bench_function("write_observed", |b| {
        b.iter(|| {
            let before = CHANGES.load(Ordering::Acquire);
            black_box(call_sync(&bump));
            assert!(CHANGES.load(Ordering::Acquire) > before);
        });
    });
    group.finish();
    keel_shutdown();
}

criterion_group!(benches, boundary);
criterion_main!(benches);
