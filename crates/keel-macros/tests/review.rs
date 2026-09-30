//! Regression tests for the adversarial review of `keel-macros`
//! (`.10x/reviews/2026-09-30-keel-macros-review.md`): the findings that are about what the
//! generated code *does* (the ones about what it *says* are the UI tests, `tests/ui/`).
//!
//! Every test is named after its finding: `l1_` keyword names, `l2_` hygiene, `l4_` parenthesised
//! return types, `l6_` docs, `m1_`..`m5_`, `h1_`, `h2_`.
#![forbid(unsafe_code)]
// The fixtures are deliberately awkward: parentheses around a return type, parameters named like
// the generated locals, more parameters than clippy likes.
#![allow(
    unused_parens,
    clippy::too_many_arguments,
    clippy::needless_arbitrary_self_type,
    clippy::new_without_default,
    non_snake_case
)]

use std::sync::{Arc, Mutex};

use keel::meta::collect_schema;
use keel::prelude::{Ctx, Signal};
use keel::query::{MutationDef, QueryDef};
use keel::runtime::{Port, PortDispatch, PortError};
use keel::wire::{Decode, Encode, Handle};
use keel_macros as k;

mod support;
use support::Runtime;
use support::testing::block_on;

#[k::error]
#[derive(Clone, Debug, PartialEq)]
pub enum HyError {
    #[error("unavailable")]
    Unavailable,
}

impl From<PortError> for HyError {
    fn from(_: PortError) -> Self {
        HyError::Unavailable
    }
}

fn id(type_name: &str, method: &str) -> u32 {
    keel::meta::ids::port_method_id(type_name, method)
}

fn u32s(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

// ---------------------------------------------------------------------------------------------
// L2: parameters named like the generated locals
// ---------------------------------------------------------------------------------------------

#[k::api]
pub fn l2_function(
    __r: u32,
    __w: u32,
    __ctx: u32,
    __rt: u32,
    __call: u32,
    __obj: u32,
    __params: u32,
    __fut: u32,
) -> u32 {
    __r + 2 * __w + 3 * __ctx + 4 * __rt + 5 * __call + 6 * __obj + 7 * __params + 8 * __fut
}

pub struct L2Object;

#[k::api]
impl L2Object {
    /// The context parameter is injected, whatever it is called.
    pub fn new(__ctx: &Ctx, __r: u32) -> Self {
        let _ = (__ctx, __r);
        L2Object
    }

    pub fn go(&self, __r: u32, __w: u32, __obj: u32, __rt: u32, __call: u32) -> u32 {
        __r + 10 * __w + 100 * __obj + 1_000 * __rt + 10_000 * __call
    }
}

#[test]
fn l2_functions_and_methods_accept_parameters_named_like_generated_locals() {
    let rt = Runtime::new();
    let reply = rt
        .call_function("l2_function", &u32s(&[1, 2, 3, 4, 5, 6, 7, 8]))
        .sync_ok();
    // 1 + 4 + 9 + 16 + 25 + 36 + 49 + 64
    assert_eq!(u32::decode_exact(&reply).unwrap(), 204);

    let handle = Handle::decode_exact(&rt.call_object("L2Object", "new", 0, &u32s(&[9])).sync_ok())
        .unwrap()
        .0;
    let reply = rt
        .call_object("L2Object", "go", handle, &u32s(&[1, 2, 3, 4, 5]))
        .sync_ok();
    assert_eq!(
        u32::decode_exact(&reply).unwrap(),
        1 + 20 + 300 + 4_000 + 50_000
    );
}

#[k::port]
pub trait L2Port {
    async fn send(&self, __w: u32, __args: u32, __ctx: u32, __reply: u32) -> u32;
    fn sum(&self, __w: u32, __bytes: u32) -> Result<u32, HyError>;
}

#[derive(Default)]
struct L2Fake;

#[k::port]
impl L2Port for L2Fake {
    async fn send(&self, __w: u32, __args: u32, __ctx: u32, __reply: u32) -> u32 {
        __w + __args + __ctx + __reply
    }

    fn sum(&self, __w: u32, __bytes: u32) -> Result<u32, HyError> {
        Ok(__w + __bytes)
    }
}

#[test]
fn l2_port_proxies_and_dispatchers_accept_parameters_named_like_generated_locals() {
    // Through the proxy to a "platform" that adds the arguments up.
    let rt = Runtime::new();
    rt.bind_foreign(<dyn L2Port as Port>::PORT_ID, |method, args| {
        let values: Vec<u32> = args
            .chunks(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert!(method == id("L2Port", "send") || method == id("L2Port", "sum"));
        Ok(values.iter().sum::<u32>().encode_to_vec())
    });
    let ctx = rt.ctx();
    assert_eq!(block_on(l2_port(&ctx).send(1, 2, 3, 4)), 10);
    assert_eq!(l2_port(&ctx).sum(5, 6), Ok(11));

    // Through the Rust-side dispatcher of a fake.
    let fake: Arc<dyn L2Port> = Arc::new(L2Fake);
    let reply = match __keel_port_dispatch_L2Port(&fake, id("L2Port", "send"), &u32s(&[1, 2, 3, 4]))
    {
        PortDispatch::Sync(bytes) => bytes,
        PortDispatch::Async(future) => block_on(future),
    };
    assert_eq!((reply[0], u32::decode_exact(&reply[1..]).unwrap()), (0, 10));
}

#[k::port(event)]
pub trait L2Events {
    fn fired(&self, __w: u8, __r: u8, __payload: u8);
}

#[test]
fn l2_event_helpers_accept_parameters_named_like_generated_locals() {
    let rt = Runtime::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let _subscription = on_l2_events_fired(&rt.ctx(), move |w, r, payload| {
        sink.lock().unwrap().push((w, r, payload));
    });
    rt.event(
        <dyn L2Events as Port>::PORT_ID,
        id("L2Events", "fired"),
        &encode_l2_events_fired_event(1, 2, 3),
    );
    assert_eq!(*seen.lock().unwrap(), [(1, 2, 3)]);
}

#[k::query(key = "l2:{__params}:{__fut}")]
pub async fn l2_query(__ctx: &Ctx, __params: u32, __fut: u32) -> Result<u32, HyError> {
    let _ = __ctx;
    Ok(__params * 10 + __fut)
}

#[k::mutation]
pub async fn l2_mutation(__ctx: Ctx, __params: u32) -> Result<u32, HyError> {
    let _ = __ctx;
    Ok(__params + 1)
}

#[test]
fn l2_queries_and_mutations_accept_parameters_named_like_generated_locals() {
    let ctx = Runtime::new().ctx();
    assert_eq!(block_on(L2QueryQuery::fetch(ctx.clone(), (4, 2))), Ok(42));
    assert_eq!(block_on(L2MutationMutation::execute(ctx, (9,))), Ok(10));
}

// ---------------------------------------------------------------------------------------------
// L1: a port whose snake-case name is a keyword
// ---------------------------------------------------------------------------------------------

#[k::port(sync)]
pub trait Match {
    fn go(&self) -> u8;
}

#[k::port(sync)]
pub trait Type {
    fn kind(&self) -> u8;
}

#[k::port(sync)]
pub trait Loop {
    fn spin(&self) -> u8;
}

#[k::port(sync)]
pub trait Super {
    fn up(&self) -> u8;
}

#[test]
fn l1_ports_named_like_keywords_get_raw_accessors() {
    let rt = Runtime::new();
    rt.bind_foreign(
        <dyn Match as Port>::PORT_ID,
        |_, _| Ok(1_u8.encode_to_vec()),
    );
    rt.bind_foreign(<dyn Type as Port>::PORT_ID, |_, _| Ok(2_u8.encode_to_vec()));
    rt.bind_foreign(<dyn Loop as Port>::PORT_ID, |_, _| Ok(3_u8.encode_to_vec()));
    rt.bind_foreign(
        <dyn Super as Port>::PORT_ID,
        |_, _| Ok(4_u8.encode_to_vec()),
    );
    let ctx = rt.ctx();
    assert_eq!(r#match(&ctx).go(), 1);
    assert_eq!(r#type(&ctx).kind(), 2);
    assert_eq!(r#loop(&ctx).spin(), 3);
    // `super` cannot be a raw identifier: the accessor gets a trailing underscore.
    assert_eq!(super_(&ctx).up(), 4);
}

// ---------------------------------------------------------------------------------------------
// L4: parenthesised return types
// ---------------------------------------------------------------------------------------------

#[k::query(key = "l4")]
pub async fn l4_query(ctx: &Ctx) -> (Result<u8, HyError>) {
    let _ = ctx;
    Ok(4)
}

pub struct L4Object;

#[k::api]
impl L4Object {
    pub fn new() -> Self {
        L4Object
    }

    pub fn paren(&self) -> (Result<u8, HyError>) {
        Ok(5)
    }
}

#[k::port(sync)]
pub trait L4Port {
    fn paren(&self) -> (Result<u8, HyError>);
}

#[test]
fn l4_parenthesised_return_types_are_read_through() {
    let rt = Runtime::new();
    // The query used to fail with "keel: internal error: a validated query has no Result type".
    assert_eq!(block_on(L4QueryQuery::fetch(rt.ctx(), ())), Ok(4));

    let handle = Handle::decode_exact(&rt.call_object("L4Object", "new", 0, &[]).sync_ok())
        .unwrap()
        .0;
    let reply = rt.call_object("L4Object", "paren", handle, &[]).sync_ok();
    assert_eq!(u8::decode_exact(&reply).unwrap(), 5);

    // The proxy decodes the reply as a `u8`, not as a `Result`, and maps an unavailable port.
    rt.bind_foreign(<dyn L4Port as Port>::PORT_ID, |_, _| {
        Ok(7_u8.encode_to_vec())
    });
    assert_eq!(l4_port(&rt.ctx()).paren(), Ok(7));
    let unbound = Runtime::new();
    assert_eq!(l4_port(&unbound.ctx()).paren(), Err(HyError::Unavailable));
}

// ---------------------------------------------------------------------------------------------
// L6: docs
// ---------------------------------------------------------------------------------------------

/// Struct docs of a store.
#[k::store]
pub struct L6Store {
    #[allow(dead_code)]
    ctx: Ctx,
    n: Signal<u32>,
}

/// Impl docs of a store.
#[k::api(store)]
impl L6Store {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            n: Signal::new(0),
        }
    }
}

// A store whose struct has no docs.
#[k::store]
pub struct L6Bare {
    #[allow(dead_code)]
    ctx: Ctx,
    n: Signal<u32>,
}

#[k::api(store)]
impl L6Bare {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            n: Signal::new(0),
        }
    }
}

/// Docs of a struct that has no Keel attribute: the impl block cannot see them.
pub struct L6Plain;

/// Docs written on the impl block.
#[k::api]
impl L6Plain {
    pub fn new() -> Self {
        L6Plain
    }

    /// Before.
    #[doc = include_str!("support/doc_fragment.md")]
    /// After.
    #[cfg_attr(test, doc(hidden))]
    pub fn included(&self) -> u8 {
        1
    }
}

/// Before.
#[doc = include_str!("support/doc_fragment.md")]
/// After.
#[k::api]
pub fn l6_function() {}

#[test]
fn l6_a_stores_docs_are_its_struct_docs_then_its_impl_docs() {
    let schema = collect_schema("review");
    let docs = |name: &str| {
        schema
            .objects
            .iter()
            .find(|o| o.name == name)
            .unwrap_or_else(|| panic!("no object {name}"))
            .docs
            .clone()
    };
    assert_eq!(
        docs("L6Store"),
        "Struct docs of a store.\n\nImpl docs of a store."
    );
    assert_eq!(docs("L6Bare"), "");
    // A plain object's struct carries no Keel attribute, so only the impl block's docs exist.
    assert_eq!(docs("L6Plain"), "Docs written on the impl block.");
}

#[test]
fn l6_non_literal_docs_are_skipped_without_dropping_their_siblings() {
    let schema = collect_schema("review");
    // `#[doc = include_str!(..)]` cannot be evaluated by a macro that sees tokens.
    let function = schema
        .functions
        .iter()
        .find(|f| f.name == "l6_function")
        .unwrap();
    assert_eq!(function.docs, "Before.\nAfter.");
    let plain = schema.objects.iter().find(|o| o.name == "L6Plain").unwrap();
    let method = plain.methods.iter().find(|m| m.name == "included").unwrap();
    assert_eq!(method.docs, "Before.\nAfter.");
}

// ---------------------------------------------------------------------------------------------
// M4/M5 behaviour next to the UI tests
// ---------------------------------------------------------------------------------------------

/// A constructor taking `ctx: Ctx` by value and an object that stores nothing still works after
/// the receiver checks moved to `&Type`-style receivers.
pub struct M5Object;

#[k::api]
impl M5Object {
    pub fn new() -> Self {
        M5Object
    }

    /// `self: &Self` is the same as `&self`.
    pub fn typed(self: &Self) -> u8 {
        9
    }
}

#[test]
fn m5_self_ref_self_is_still_a_method() {
    let rt = Runtime::new();
    let handle = Handle::decode_exact(&rt.call_object("M5Object", "new", 0, &[]).sync_ok())
        .unwrap()
        .0;
    let reply = rt.call_object("M5Object", "typed", handle, &[]).sync_ok();
    assert_eq!(u8::decode_exact(&reply).unwrap(), 9);
}
