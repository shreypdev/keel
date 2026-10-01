//! Cost of crossing the C ABI (SPEC 6, constitution R4): what a Swift or Kotlin call pays for
//! the boundary itself, with a trivial method behind it.
//!
//! * `call_sync/add`: `undra_call_sync` of a sync method, payload prebuilt, reply buffer freed;
//! * `call_sync/unknown`: the status 5 path (decode, lookup, reply);
//! * `call/add`: `undra_call` of a sync method; the reply callback runs on this thread;
//! * `call/ready_add`: `undra_call` of an `async` method that is ready at once: the executor
//!   hop (spawn, core thread wake, poll, reply callback on the core thread);
//! * `write_observed`: a store write with one observer: `undra_call_sync` plus the change-set
//!   callback;
//! * `port_call/sum_on_host`: `undra_call_sync` of a method that makes one synchronous port call
//!   the host answers with a `malloc`ed reply: the port path (registration lookup and in-flight
//!   accounting, the callback, copying and `free`ing the reply) on top of a `call_sync`.
#![allow(unsafe_code)]

use core::ffi::c_void;
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{Criterion, criterion_group, criterion_main};
use undra::meta::ids;
use undra::prelude::Handle;
use undra::runtime::RuntimeConfig;
use undra::wire::payload::{Call, CallTarget};
use undra::wire::{Decode, Encode, Writer};
use undra_ffi::{
    UndraBuf, undra_buf_free, undra_call, undra_call_sync, undra_init, undra_observe,
    undra_port_register, undra_shutdown,
};

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

unsafe extern "C" {
    fn malloc(size: usize) -> *mut c_void;
}

/// The `Sum` port: `add(a: u32, b: u32) -> u32`, answered inline with a `malloc`ed `PortReply`.
extern "C" fn on_port(
    _user: *mut c_void,
    _port_id: u32,
    _method_id: u32,
    port_call_id: u32,
    ptr: *const u8,
    len: u32,
    out: *mut UndraBuf,
) -> u8 {
    // SAFETY: the core passes `len` readable bytes (two `u32`s) and a live `out_reply`.
    unsafe {
        let args = std::slice::from_raw_parts(ptr, len as usize);
        let sum = u32::from_le_bytes(args[..4].try_into().unwrap())
            .wrapping_add(u32::from_le_bytes(args[4..8].try_into().unwrap()));
        let block = malloc(9).cast::<u8>();
        block.copy_from_nonoverlapping(port_call_id.to_le_bytes().as_ptr(), 4);
        block.add(4).write(0); // status: ok
        block
            .add(5)
            .copy_from_nonoverlapping(sum.to_le_bytes().as_ptr(), 4);
        *out = UndraBuf {
            ptr: block,
            len: 9,
            cap: 0,
        };
    }
    0
}

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
        let buf = undra_call_sync(payload.as_ptr(), payload.len() as u32);
        let len = buf.len as usize;
        undra_buf_free(buf);
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
        let buf = undra_call_sync(p.as_ptr(), p.len() as u32);
        let reply = buf.as_slice()[5..].to_vec();
        undra_buf_free(buf);
        reply
    };
    Handle::decode_exact(&body).expect("a handle")
}

fn boundary(c: &mut Criterion) {
    let cfg = RuntimeConfig::default().encode_to_vec();
    // SAFETY: `on_port` is an `extern "C"` function that touches nothing but its arguments.
    unsafe { undra_port_register(ids::port_id("Sum"), Some(on_port), std::ptr::null_mut()) };
    // SAFETY: `cfg` is valid for its length and the callbacks are `extern "C"` functions.
    let code = unsafe {
        undra_init(
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

    let sum = payload(
        target("sum_on_host"),
        5,
        &[20_u32.encode_to_vec(), 22_u32.encode_to_vec()].concat(),
    );
    group.bench_function("port_call/sum_on_host", |b| {
        b.iter(|| call_sync(black_box(&sum)));
    });

    group.bench_function("call/add", |b| {
        let mut id = 10_u32;
        b.iter(|| {
            id = id.wrapping_add(1).max(10);
            let p = payload(target("add"), id, &args);
            let before = REPLIES.load(Ordering::Acquire);
            // SAFETY: `p` is valid for its length.
            assert_eq!(unsafe { undra_call(p.as_ptr(), p.len() as u32) }, 0);
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
            assert_eq!(unsafe { undra_call(p.as_ptr(), p.len() as u32) }, 0);
            while REPLIES.load(Ordering::Acquire) == before {
                std::hint::spin_loop();
            }
        });
    });

    let counter = construct("Counter", &[]);
    undra_observe(counter.0, u32::MAX, 1);
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
    undra_shutdown();
}

criterion_group!(benches, boundary);
criterion_main!(benches);
