//! The synchronous reply slot (ADR-028): `call_sync` answers through a thread-local buffer, and
//! everything the buffer cannot serve falls back to the allocating path. The two must be
//! indistinguishable on the wire, so every scenario here is run both ways and compared byte for
//! byte (`call_sync_reference` keeps the slot busy, which is exactly the allocating path).
//!
//! The dispatchers are hand-written the way the macros generate them: `rt.sync_ok(&value,
//! Encode::encode)` for a success, `rt.sync_err(..)` for a typed error.

use std::any::Any;
use std::sync::{Arc, Mutex};

use undra_meta::{
    DispatchCall, DispatchOutcome, FunctionMeta, MethodMeta, ObjectMeta, ParamMeta, Registration,
    TypeRefMeta, ids,
};
use undra_runtime::testing::{TestRuntime, call_payload, call_sync_reference, decode_reply};
use undra_runtime::{DispatchLayer, DispatchResult, Handle, Runtime, UndraObject, inventory};
use undra_wire::payload::{CallTarget, ReplyStatus};
use undra_wire::{Decode, Encode, Reader, Writer};

// ----- the fixture: an object, a function and a layer that answer through sync_ok/sync_err ----

struct Fast;

impl UndraObject for Fast {
    const TYPE_ID: u32 = ids::type_id("Fast");
    const NAME: &'static str = "Fast";
}

macro_rules! ids_of {
    ($($name:ident = $s:literal;)*) => {
        $(const $name: u32 = ids::method_id("Fast", $s);)*
    };
}

ids_of! {
    NEW = "new";
    DOUBLE = "double";
    NOTHING = "nothing";
    FAIL = "fail";
    BAD = "bad";
    BOOM = "boom";
    BOOM_ENCODE = "boom_encode";
    BIG = "big";
    REENTER = "reenter";
    NESTED_RUNTIME = "nested_runtime";
    SLOW = "slow";
    CALL_ID = "call_id";
}

const FN_TRIPLE: u32 = ids::function_id("fast_triple");
/// Served by the layer below, never registered statically.
const LAYER_ONLY: u32 = ids::function_id("layer_only_fast");
const LAYER_FAIL: u32 = ids::function_id("layer_only_fail");

/// A value whose encoding panics, to prove a failure after the slot took the call is contained.
struct EncodeBomb;

impl Encode for EncodeBomb {
    fn encode(&self, _: &mut Writer) {
        panic!("the encoder blew up");
    }
}

/// The second runtime a method calls into while the first is dispatching.
static OTHER: Mutex<Option<Arc<Runtime>>> = Mutex::new(None);

fn decode_args<T: Decode>(call: &DispatchCall<'_>) -> Result<T, DispatchOutcome> {
    T::decode_exact(call.args).map_err(|e| {
        DispatchOutcome::new(DispatchResult::BadRequest(format!("bad arguments: {e}")))
    })
}

fn fast_dispatch(rt: &dyn Any, call: DispatchCall<'_>) -> DispatchOutcome {
    let Some(rt) = rt.downcast_ref::<Runtime>() else {
        return DispatchOutcome::new(DispatchResult::Unknown);
    };
    if call.method_id == NEW {
        let handle = rt.insert_object(Arc::new(Fast));
        return rt.sync_ok(&handle, Encode::encode);
    }
    if rt.object::<Fast>(call.handle).is_err() {
        return DispatchOutcome::new(DispatchResult::BadRequest("no such Fast".to_owned()));
    }
    match call.method_id {
        DOUBLE => match decode_args::<u32>(&call) {
            Ok(n) => rt.sync_ok(&(n * 2), Encode::encode),
            Err(bad) => bad,
        },
        NOTHING => rt.sync_ok(&(), Encode::encode),
        FAIL => match decode_args::<u16>(&call) {
            Ok(code) => rt.sync_err(&code, Encode::encode),
            Err(bad) => bad,
        },
        BAD => DispatchOutcome::new(DispatchResult::BadRequest("the method refuses".to_owned())),
        BOOM => panic!("kaboom"),
        BOOM_ENCODE => rt.sync_ok(&EncodeBomb, |bomb, w| bomb.encode(w)),
        BIG => match decode_args::<u32>(&call) {
            Ok(n) => rt.sync_ok(&vec![0xAB_u8; n as usize], Encode::encode),
            Err(bad) => bad,
        },
        CALL_ID => rt.sync_ok(&call.call_id, Encode::encode),
        // A host calling back into the runtime from inside a method: refused, and the method
        // reports the status byte of the refusal it got.
        REENTER => {
            let inner = call_payload(
                CallTarget::Method {
                    handle: Handle(call.handle),
                    method_id: DOUBLE,
                },
                99,
                &1_u32.encode_to_vec(),
            );
            let refusal = rt.call_sync(&inner);
            rt.sync_ok(&refusal[4], Encode::encode)
        }
        // A method that calls another runtime's `call_sync` on this very thread, then answers
        // with what it got: the slot is armed for this runtime, so the inner call must neither
        // steal nor clobber it.
        NESTED_RUNTIME => {
            let other = OTHER.lock().unwrap().clone().expect("the test set it");
            let payload = call_payload(
                CallTarget::Function {
                    method_id: FN_TRIPLE,
                },
                77,
                &7_u32.encode_to_vec(),
            );
            let inner = other.call_sync(&payload);
            rt.sync_ok(&inner, Encode::encode)
        }
        // Declared `async` in the metadata: `call_sync` must refuse it without running it.
        SLOW => {
            panic!("an async method must not run under call_sync");
        }
        _ => DispatchOutcome::new(DispatchResult::Unknown),
    }
}

fn fn_dispatch(rt: &dyn Any, call: DispatchCall<'_>) -> DispatchOutcome {
    let Some(rt) = rt.downcast_ref::<Runtime>() else {
        return DispatchOutcome::new(DispatchResult::Unknown);
    };
    match u32::decode_exact(call.args) {
        Ok(n) => rt.sync_ok(&(n * 3), Encode::encode),
        Err(e) => DispatchOutcome::new(DispatchResult::BadRequest(e.to_string())),
    }
}

/// A layer that adopts the same fast path for ids no registration names.
fn layer_dispatch(rt: &dyn Any, call: DispatchCall<'_>) -> DispatchOutcome {
    let Some(rt) = rt.downcast_ref::<Runtime>() else {
        return DispatchOutcome::new(DispatchResult::Unknown);
    };
    match call.method_id {
        LAYER_ONLY => rt.sync_ok(&String::from("from the layer"), Encode::encode),
        LAYER_FAIL => rt.sync_err(&7_u8, Encode::encode),
        _ => DispatchOutcome::new(DispatchResult::Unknown),
    }
}

const fn method(
    name: &'static str,
    method_id: u32,
    params: &'static [ParamMeta],
    returns: TypeRefMeta,
    is_async: bool,
) -> MethodMeta {
    MethodMeta {
        name,
        method_id,
        params,
        returns,
        is_async,
        takes_ctx: false,
        coalesce: false,
        docs: "",
    }
}

const U32_ARG: &[ParamMeta] = &[ParamMeta {
    name: "n",
    ty: TypeRefMeta::U32,
}];

static FAST_META: ObjectMeta = ObjectMeta {
    name: "Fast",
    type_id: ids::type_id("Fast"),
    constructors: &[method("new", NEW, &[], TypeRefMeta::Named("Fast"), false)],
    methods: &[
        method("double", DOUBLE, U32_ARG, TypeRefMeta::U32, false),
        method("nothing", NOTHING, &[], TypeRefMeta::Unit, false),
        method(
            "fail",
            FAIL,
            &[ParamMeta {
                name: "code",
                ty: TypeRefMeta::U16,
            }],
            TypeRefMeta::Result(&TypeRefMeta::Unit, &TypeRefMeta::U16),
            false,
        ),
        method("bad", BAD, &[], TypeRefMeta::Unit, false),
        method("boom", BOOM, &[], TypeRefMeta::Unit, false),
        method("boom_encode", BOOM_ENCODE, &[], TypeRefMeta::U8, false),
        method("big", BIG, U32_ARG, TypeRefMeta::Bytes, false),
        method("reenter", REENTER, &[], TypeRefMeta::U8, false),
        method(
            "nested_runtime",
            NESTED_RUNTIME,
            &[],
            TypeRefMeta::Bytes,
            false,
        ),
        method("slow", SLOW, &[], TypeRefMeta::U32, true),
        method("call_id", CALL_ID, &[], TypeRefMeta::U32, false),
    ],
    store: None,
    docs: "",
    dispatch: fast_dispatch,
};

static TRIPLE_META: FunctionMeta = FunctionMeta {
    name: "fast_triple",
    method_id: FN_TRIPLE,
    params: U32_ARG,
    returns: TypeRefMeta::U32,
    is_async: false,
    takes_ctx: false,
    docs: "",
    dispatch: fn_dispatch,
};

inventory::submit! { Registration::Object(&FAST_META) }
inventory::submit! { Registration::Function(&TRIPLE_META) }
inventory::submit! { DispatchLayer { name: "fast-layer", dispatch: layer_dispatch } }

// ----- helpers --------------------------------------------------------------------------------

fn construct(t: &TestRuntime) -> Handle {
    let reply = t.call_sync(
        CallTarget::Constructor {
            type_id: FAST_META.type_id,
            method_id: NEW,
        },
        1,
        &[],
    );
    assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
    Handle::decode_exact(&reply.body).unwrap()
}

fn method_call(handle: Handle, method_id: u32, call_id: u32, args: &[u8]) -> Vec<u8> {
    call_payload(CallTarget::Method { handle, method_id }, call_id, args)
}

/// Runs `payload` through the slot and through the allocating path and requires the same bytes.
fn same_both_ways(rt: &Runtime, payload: &[u8]) -> Vec<u8> {
    let fast = rt.call_sync(payload);
    let reference = call_sync_reference(rt, payload);
    assert_eq!(
        fast, reference,
        "the reply of the slot and the allocating path differ"
    );
    fast
}

// ----- the two paths agree ----------------------------------------------------------------------

#[test]
fn an_ok_value_is_byte_identical_and_has_the_reply_layout() {
    let t = TestRuntime::new();
    let h = construct(&t);
    let reply = same_both_ways(
        t.runtime(),
        &method_call(h, DOUBLE, 0x0102_0304, &21_u32.encode_to_vec()),
    );
    // call_id u32 LE, status 0, the value.
    assert_eq!(reply, [0x04, 0x03, 0x02, 0x01, 0, 42, 0, 0, 0]);
    let unit = same_both_ways(t.runtime(), &method_call(h, NOTHING, 5, &[]));
    assert_eq!(unit, [5, 0, 0, 0, 0]);
    // A constructor answers its handle the same way.
    let ctor = call_payload(
        CallTarget::Constructor {
            type_id: FAST_META.type_id,
            method_id: NEW,
        },
        6,
        &[],
    );
    let a = t.runtime().call_sync(&ctor);
    let b = call_sync_reference(t.runtime(), &ctor);
    assert_eq!(a[..5], b[..5]);
    assert_eq!(a.len(), 5 + 8);
}

#[test]
fn a_typed_error_is_byte_identical_with_status_1() {
    let t = TestRuntime::new();
    let h = construct(&t);
    let reply = same_both_ways(
        t.runtime(),
        &method_call(h, FAIL, 9, &513_u16.encode_to_vec()),
    );
    assert_eq!(reply, [9, 0, 0, 0, 1, 0x01, 0x02]);
}

#[test]
fn a_bad_request_is_byte_identical_with_status_5() {
    let t = TestRuntime::new();
    let h = construct(&t);
    // The dispatcher's own refusal, undecodable arguments, a stale receiver and a payload that
    // is not a call at all.
    for payload in [
        method_call(h, BAD, 3, &[]),
        method_call(h, DOUBLE, 4, &[1]),
        method_call(Handle(0xdead_0000_0001), DOUBLE, 5, &[]),
        vec![1, 2, 3],
    ] {
        let reply = same_both_ways(t.runtime(), &payload);
        assert_eq!(decode_reply(&reply).status, ReplyStatus::BadRequest);
    }
    // An unknown method id on a known object is status 5 too.
    let unknown = same_both_ways(
        t.runtime(),
        &method_call(h, ids::method_id("Fast", "nope"), 6, &[]),
    );
    assert_eq!(decode_reply(&unknown).status, ReplyStatus::BadRequest);
}

#[test]
fn a_panic_is_status_2_on_both_paths_and_leaves_the_slot_usable() {
    let t = TestRuntime::new();
    let h = construct(&t);
    for method_id in [BOOM, BOOM_ENCODE] {
        let payload = method_call(h, method_id, 8, &[]);
        let fast = decode_reply(&t.runtime().call_sync(&payload));
        let reference = decode_reply(&call_sync_reference(t.runtime(), &payload));
        assert_eq!(fast.status, ReplyStatus::Panic);
        assert_eq!(reference.status, ReplyStatus::Panic);
        assert_eq!(fast.call_id, 8);
        // The body is `String message, String backtrace`; the messages match, the backtraces
        // are each path's own.
        let message = |body: &[u8]| Reader::new(body).read_str().unwrap().to_owned();
        assert_eq!(message(&fast.body), message(&reference.body));
    }
    // The next call through the slot is unaffected (the buffer the encoder panicked in is gone,
    // a new one is grown).
    let ok = same_both_ways(
        t.runtime(),
        &method_call(h, DOUBLE, 10, &4_u32.encode_to_vec()),
    );
    assert_eq!(ok, [10, 0, 0, 0, 0, 8, 0, 0, 0]);
    assert_eq!(t.runtime().objects().live(), 1, "the object survived");
}

#[test]
fn an_async_method_is_refused_before_it_runs() {
    let t = TestRuntime::new();
    let h = construct(&t);
    let reply = same_both_ways(t.runtime(), &method_call(h, SLOW, 2, &[]));
    let reply = decode_reply(&reply);
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    let reason = Reader::new(&reply.body).read_str().unwrap().to_owned();
    assert!(reason.contains("`slow` is asynchronous"), "{reason}");
}

#[test]
fn functions_and_layers_answer_through_the_same_path() {
    let t = TestRuntime::new();
    let function = call_payload(
        CallTarget::Function {
            method_id: FN_TRIPLE,
        },
        1,
        &5_u32.encode_to_vec(),
    );
    assert_eq!(
        same_both_ways(t.runtime(), &function),
        [1, 0, 0, 0, 0, 15, 0, 0, 0]
    );

    // The static table has no entry for these ids: the layer serves them, adopting sync_ok.
    let layer_ok = call_payload(
        CallTarget::Function {
            method_id: LAYER_ONLY,
        },
        2,
        &[],
    );
    let reply = decode_reply(&same_both_ways(t.runtime(), &layer_ok));
    assert_eq!(reply.status, ReplyStatus::Ok);
    assert_eq!(String::decode_exact(&reply.body).unwrap(), "from the layer");
    let layer_err = call_payload(
        CallTarget::Function {
            method_id: LAYER_FAIL,
        },
        3,
        &[],
    );
    let reply = decode_reply(&same_both_ways(t.runtime(), &layer_err));
    assert_eq!(reply.status, ReplyStatus::Error);
    assert_eq!(reply.body, [7]);

    // An id nobody claims falls through every layer to status 5.
    let nobody = call_payload(
        CallTarget::Function {
            method_id: ids::function_id("nobody_serves_this"),
        },
        4,
        &[],
    );
    let reply = decode_reply(&same_both_ways(t.runtime(), &nobody));
    assert_eq!(reply.status, ReplyStatus::BadRequest);
}

#[test]
fn the_reply_echoes_every_call_id() {
    let t = TestRuntime::new();
    let h = construct(&t);
    for call_id in [1_u32, 255, 65_536, u32::MAX] {
        let reply = same_both_ways(t.runtime(), &method_call(h, CALL_ID, call_id, &[]));
        let reply = decode_reply(&reply);
        assert_eq!(reply.call_id, call_id);
        assert_eq!(u32::decode_exact(&reply.body).unwrap(), call_id);
    }
}

#[test]
fn a_reply_bigger_than_the_kept_buffer_is_served_whole() {
    let t = TestRuntime::new();
    let h = construct(&t);
    for n in [0_u32, 1, 70_000, 1 << 20] {
        let reply = same_both_ways(t.runtime(), &method_call(h, BIG, 2, &n.encode_to_vec()));
        assert_eq!(reply.len(), 5 + 4 + n as usize);
        assert!(reply[9..].iter().all(|&b| b == 0xAB));
    }
    // And small calls still work afterwards.
    let small = same_both_ways(
        t.runtime(),
        &method_call(h, DOUBLE, 3, &1_u32.encode_to_vec()),
    );
    assert_eq!(small, [3, 0, 0, 0, 0, 2, 0, 0, 0]);
}

// ----- re-entrancy and nesting ------------------------------------------------------------------

#[test]
fn a_method_that_calls_the_runtime_is_refused_and_the_outer_reply_is_intact() {
    let t = TestRuntime::new();
    let h = construct(&t);
    let reply = same_both_ways(t.runtime(), &method_call(h, REENTER, 4, &[]));
    // The inner call (the host re-entering) was answered with status 5; the method reported it.
    assert_eq!(reply, [4, 0, 0, 0, 0, ReplyStatus::BadRequest.as_u8()]);
}

#[test]
fn the_reader_may_call_the_runtime_and_does_not_disturb_the_bytes_it_reads() {
    let t = TestRuntime::new();
    let rt = t.runtime();
    let h = construct(&t);
    let outer = method_call(h, DOUBLE, 11, &10_u32.encode_to_vec());
    let inner = method_call(h, DOUBLE, 12, &20_u32.encode_to_vec());
    let expected_outer = [11, 0, 0, 0, 0, 20, 0, 0, 0];
    let expected_inner = [12, 0, 0, 0, 0, 40, 0, 0, 0];

    let seen = rt.call_sync_with(&outer, |reply| {
        let before = reply.to_vec();
        // The slot is busy while the reply is lent out, so this call takes the allocating path,
        // and answers correctly.
        let nested = rt.call_sync(&inner);
        assert_eq!(nested, expected_inner);
        // Through the reference path too, and via `call_sync_with` nested in `call_sync_with`.
        assert_eq!(call_sync_reference(rt, &inner), expected_inner);
        rt.call_sync_with(&inner, |inner_reply| {
            assert_eq!(inner_reply, expected_inner)
        });
        assert_eq!(reply, &before[..], "the lent bytes did not move");
        reply.to_vec()
    });
    assert_eq!(seen, expected_outer);
    // The slot is free again: a plain call after it is served normally.
    assert_eq!(rt.call_sync(&inner), expected_inner);
}

#[test]
fn a_panicking_reader_releases_the_slot() {
    let t = TestRuntime::new();
    let rt = t.runtime();
    let h = construct(&t);
    let payload = method_call(h, DOUBLE, 2, &3_u32.encode_to_vec());
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rt.call_sync_with(&payload, |_| panic!("the reader panicked"));
    }));
    assert!(caught.is_err());
    assert_eq!(same_both_ways(rt, &payload), [2, 0, 0, 0, 0, 6, 0, 0, 0]);
}

#[test]
fn a_method_calling_another_runtime_on_the_same_thread_steals_nothing() {
    let t = TestRuntime::new();
    let other = TestRuntime::new();
    *OTHER.lock().unwrap() = Some(other.runtime().clone());
    let h = construct(&t);
    let reply = same_both_ways(t.runtime(), &method_call(h, NESTED_RUNTIME, 21, &[]));
    let reply = decode_reply(&reply);
    assert_eq!(reply.status, ReplyStatus::Ok);
    assert_eq!(reply.call_id, 21);
    // The body is the bytes the *other* runtime answered (`Vec<u8>` encoding: a length, then the
    // other reply): call id 77, status 0, 7 * 3.
    let inner = Vec::<u8>::decode_exact(&reply.body).unwrap();
    assert_eq!(inner, [77, 0, 0, 0, 0, 21, 0, 0, 0]);
    *OTHER.lock().unwrap() = None;
}

#[test]
fn every_thread_has_its_own_slot() {
    let t = Arc::new(TestRuntime::new());
    let h = construct(&t);
    let rt = t.runtime().clone();
    let workers: Vec<_> = (0..8_u32)
        .map(|worker| {
            let rt = rt.clone();
            std::thread::spawn(move || {
                for i in 0..500_u32 {
                    let n = worker * 1000 + i;
                    let call_id = 1 + i;
                    let payload = method_call(h, DOUBLE, call_id, &n.encode_to_vec());
                    let mut expected = call_id.to_le_bytes().to_vec();
                    expected.push(0);
                    expected.extend_from_slice(&(n * 2).to_le_bytes());
                    assert_eq!(rt.call_sync(&payload), expected);
                    rt.call_sync_with(&payload, |reply| assert_eq!(reply, &expected[..]));
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
}

#[test]
fn call_sync_with_hands_out_the_same_bytes_as_call_sync() {
    let t = TestRuntime::new();
    let h = construct(&t);
    for payload in [
        method_call(h, DOUBLE, 1, &9_u32.encode_to_vec()),
        method_call(h, FAIL, 2, &3_u16.encode_to_vec()),
        method_call(h, BAD, 3, &[]),
        vec![0xff; 3],
    ] {
        let owned = t.runtime().call_sync(&payload);
        let lent = t.runtime().call_sync_with(&payload, <[u8]>::to_vec);
        assert_eq!(owned, lent);
    }
}

#[test]
fn a_call_made_from_a_shut_down_runtime_is_still_answered() {
    let t = TestRuntime::new();
    let h = construct(&t);
    t.runtime().shutdown();
    let reply =
        decode_reply(
            &t.runtime()
                .call_sync(&method_call(h, DOUBLE, 1, &1_u32.encode_to_vec())),
        );
    assert_eq!(reply.status, ReplyStatus::BadRequest);
}
