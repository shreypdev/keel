//! Behaviour tests for `#[undra::api]` on impl blocks and free functions: the generated
//! dispatchers are looked up in the registry (as the real runtime does) and *called*.
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Mutex;

use undra::meta::{Registration, TypeRef, collect_schema, ids};
use undra::runtime::{Ctx, UndraObject, Stream};
use undra::wire::{Decode, Encode, Handle, Writer};
use undra_macros as k;

mod support;
use support::Runtime;
use support::testing::stream_of;

fn args(encode: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    encode(&mut w);
    w.into_vec()
}

#[k::error]
#[derive(Clone, PartialEq)]
pub enum CalcError {
    #[error("division by zero")]
    DivideByZero,
    #[error("negative start {0}")]
    NegativeStart(i64),
}

/// A calculator.
pub struct Calculator {
    ctx: Ctx,
    base: i64,
    log: Mutex<Vec<String>>,
}

/// A calculator.
#[k::api]
impl Calculator {
    /// Creates one with a starting value.
    pub fn new(ctx: &Ctx, base: i64) -> Self {
        Calculator {
            ctx: ctx.clone(),
            base,
            log: Mutex::new(Vec::new()),
        }
    }

    pub fn from_owned_ctx(ctx: Ctx) -> Self {
        Self::new(&ctx, 100)
    }

    pub fn without_ctx() -> Self {
        Calculator {
            ctx: Runtime::new().ctx(),
            base: 0,
            log: Mutex::new(Vec::new()),
        }
    }

    pub fn checked(base: i64) -> Result<Self, CalcError> {
        if base < 0 {
            return Err(CalcError::NegativeStart(base));
        }
        Ok(Self::new(&Runtime::new().ctx(), base))
    }

    /// Adds to the base.
    pub fn add(&self, a: i64, b: i64) -> i64 {
        self.log.lock().unwrap().push(format!("add {a} {b}"));
        self.base + a + b
    }

    pub fn divide(&self, a: i64, b: i64) -> Result<i64, CalcError> {
        if b == 0 {
            Err(CalcError::DivideByZero)
        } else {
            Ok(a / b)
        }
    }

    pub fn reset(&self) {
        self.log.lock().unwrap().clear();
    }

    pub fn log_len(&self) -> u32 {
        self.log.lock().unwrap().len() as u32
    }

    pub fn describe(&self, names: Vec<String>, flags: HashMap<String, bool>) -> String {
        let mut keys: Vec<_> = flags.into_iter().collect();
        keys.sort();
        format!("{}:{keys:?}:{}", names.join(","), self.base)
    }

    pub async fn slow_add(&self, a: i64) -> i64 {
        std::future::ready(()).await;
        self.base + a
    }

    pub async fn slow_divide(&self, a: i64, b: i64) -> Result<i64, CalcError> {
        std::future::ready(()).await;
        self.divide(a, b)
    }

    pub async fn slow_unit(&self) {
        std::future::ready(()).await;
        self.log.lock().unwrap().push("slow".into());
    }

    pub fn counts(&self, up_to: u32) -> impl Stream<Item = u32> + Send + 'static {
        stream_of((0..up_to).collect())
    }

    pub fn try_counts(&self, up_to: i64) -> Result<impl Stream<Item = u32> + Send, CalcError> {
        if up_to < 0 {
            Err(CalcError::NegativeStart(up_to))
        } else {
            Ok(stream_of((0..up_to as u32).collect()))
        }
    }

    pub async fn later_counts(&self, up_to: u32) -> impl Stream<Item = u32> + Send {
        std::future::ready(()).await;
        stream_of((0..up_to).collect())
    }

    pub async fn try_later_counts(
        &self,
        up_to: i64,
    ) -> Result<impl Stream<Item = u32> + Send, CalcError> {
        std::future::ready(()).await;
        if up_to < 0 {
            Err(CalcError::NegativeStart(up_to))
        } else {
            Ok(stream_of((0..up_to as u32).collect()))
        }
    }

    #[deprecated(note = "kept to prove the dispatcher does not warn about deprecated methods")]
    pub fn old_add(&self, a: i64) -> i64 {
        self.base + a
    }

    #[cfg(test)]
    #[allow(dead_code)]
    fn only_in_tests(&self) -> i64 {
        self.base
    }

    /// Not exposed: private.
    #[allow(dead_code)]
    fn helper(&self) -> i64 {
        self.ctx_is_alive() as i64
    }

    #[allow(dead_code)]
    pub(crate) fn crate_visible(&self) -> i64 {
        self.helper()
    }

    #[allow(dead_code)]
    fn ctx_is_alive(&self) -> bool {
        let _ = &self.ctx;
        true
    }
}

/// A second object, to check that handles are typed.
#[derive(Default)]
pub struct Other;

#[k::api]
impl Other {
    pub fn new() -> Self {
        Other
    }
    pub fn ping(&self) -> u8 {
        1
    }
}

fn runtime_with_calculator() -> (Runtime, u64) {
    let rt = Runtime::new();
    let reply = rt
        .call_object("Calculator", "new", 0, &args(|w| 10_i64.encode(w)))
        .sync_ok();
    let handle = Handle::decode_exact(&reply).unwrap().0;
    (rt, handle)
}

#[test]
fn constructors_insert_the_object_and_reply_with_its_handle() {
    let (rt, handle) = runtime_with_calculator();
    assert_ne!(handle, 0);
    let calc = rt.object::<Calculator>(handle).unwrap();
    assert_eq!(calc.base, 10);
    // The constructor received a usable context.
    let _ = calc.ctx.clone();
}

#[test]
fn the_ctx_parameter_may_be_owned_or_absent() {
    let rt = Runtime::new();
    let owned = rt
        .call_object("Calculator", "from_owned_ctx", 0, &[])
        .sync_ok();
    let owned = Handle::decode_exact(&owned).unwrap().0;
    assert_eq!(rt.object::<Calculator>(owned).unwrap().base, 100);
    let none = rt
        .call_object("Calculator", "without_ctx", 0, &[])
        .sync_ok();
    assert_eq!(
        rt.object::<Calculator>(Handle::decode_exact(&none).unwrap().0)
            .unwrap()
            .base,
        0
    );
}

#[test]
fn fallible_constructors_reply_with_a_handle_or_the_typed_error() {
    let rt = Runtime::new();
    let ok = rt.call_object("Calculator", "checked", 0, &args(|w| 5_i64.encode(w)));
    assert!(Handle::decode_exact(&ok.sync_ok()).is_ok());
    let err = rt
        .call_object("Calculator", "checked", 0, &args(|w| (-5_i64).encode(w)))
        .sync_err();
    assert_eq!(
        CalcError::decode_exact(&err).unwrap(),
        CalcError::NegativeStart(-5)
    );
}

#[test]
fn sync_methods_decode_arguments_in_order_and_encode_the_result() {
    let (rt, handle) = runtime_with_calculator();
    let out = rt
        .call_object(
            "Calculator",
            "add",
            handle,
            &args(|w| {
                1_i64.encode(w);
                2_i64.encode(w);
            }),
        )
        .sync_ok();
    assert_eq!(i64::decode_exact(&out).unwrap(), 13);
    let len = rt
        .call_object("Calculator", "log_len", handle, &[])
        .sync_ok();
    assert_eq!(u32::decode_exact(&len).unwrap(), 1);
    // A unit result is an empty body.
    let unit = rt.call_object("Calculator", "reset", handle, &[]).sync_ok();
    assert!(unit.is_empty());
    let len = rt
        .call_object("Calculator", "log_len", handle, &[])
        .sync_ok();
    assert_eq!(u32::decode_exact(&len).unwrap(), 0);
}

#[test]
fn collection_arguments_round_trip() {
    let (rt, handle) = runtime_with_calculator();
    let out = rt
        .call_object(
            "Calculator",
            "describe",
            handle,
            &args(|w| {
                vec!["a".to_owned(), "b".to_owned()].encode(w);
                HashMap::from([("x".to_owned(), true)]).encode(w);
            }),
        )
        .sync_ok();
    assert_eq!(
        String::decode_exact(&out).unwrap(),
        "a,b:[(\"x\", true)]:10"
    );
}

#[test]
fn result_methods_split_into_ok_and_typed_error() {
    let (rt, handle) = runtime_with_calculator();
    let ok = rt
        .call_object(
            "Calculator",
            "divide",
            handle,
            &args(|w| {
                9_i64.encode(w);
                3_i64.encode(w);
            }),
        )
        .sync_ok();
    assert_eq!(i64::decode_exact(&ok).unwrap(), 3);
    let err = rt
        .call_object(
            "Calculator",
            "divide",
            handle,
            &args(|w| {
                9_i64.encode(w);
                0_i64.encode(w);
            }),
        )
        .sync_err();
    assert_eq!(
        CalcError::decode_exact(&err).unwrap(),
        CalcError::DivideByZero
    );
}

#[test]
fn malformed_requests_are_bad_requests_with_a_reason_not_panics() {
    let (rt, handle) = runtime_with_calculator();
    let good = args(|w| {
        1_i64.encode(w);
        2_i64.encode(w);
    });
    // Too short: the reason names the argument and the method.
    let reason = rt
        .call_object("Calculator", "add", handle, &good[..15])
        .bad_request();
    assert!(
        reason.contains("cannot decode argument `b` of `Calculator.add`"),
        "{reason}"
    );
    // Too long.
    let mut long = good.clone();
    long.push(0);
    let reason = rt
        .call_object("Calculator", "add", handle, &long)
        .bad_request();
    assert!(
        reason.contains("cannot decode the arguments of `Calculator.add`"),
        "{reason}"
    );
    // Empty.
    let reason = rt
        .call_object("Calculator", "add", handle, &[])
        .bad_request();
    assert!(reason.contains("argument `a`"), "{reason}");
    // A method id the dispatcher does not implement is the "unknown" answer.
    assert!(
        rt.call_object_raw("Calculator", 0xdead_beef, handle, &good)
            .is_unknown()
    );
    // Null, stale and wrongly typed handles say which.
    let reason = rt.call_object("Calculator", "add", 0, &good).bad_request();
    assert!(reason.contains("null handle"), "{reason}");
    let reason = rt
        .call_object("Calculator", "add", Handle::new(99, 1).0, &good)
        .bad_request();
    assert!(
        reason.contains("cannot call `Calculator.add`") && reason.contains("unknown handle"),
        "{reason}"
    );
    let other = rt.call_object("Other", "new", 0, &[]).sync_ok();
    let other = Handle::decode_exact(&other).unwrap().0;
    let reason = rt
        .call_object("Calculator", "add", other, &good)
        .bad_request();
    assert!(reason.contains("not a"), "{reason}");
    // Constructors with bad arguments do not insert anything.
    let reason = rt.call_object("Calculator", "new", 0, &[1]).bad_request();
    assert!(reason.contains("Calculator.new"), "{reason}");
}

#[test]
fn async_methods_return_a_future_over_ok_or_error_bytes() {
    let (rt, handle) = runtime_with_calculator();
    let out = rt
        .call_object("Calculator", "slow_add", handle, &args(|w| 5_i64.encode(w)))
        .run_async()
        .unwrap();
    assert_eq!(i64::decode_exact(&out).unwrap(), 15);

    let ok = rt
        .call_object(
            "Calculator",
            "slow_divide",
            handle,
            &args(|w| {
                8_i64.encode(w);
                2_i64.encode(w);
            }),
        )
        .run_async()
        .unwrap();
    assert_eq!(i64::decode_exact(&ok).unwrap(), 4);
    let err = rt
        .call_object(
            "Calculator",
            "slow_divide",
            handle,
            &args(|w| {
                8_i64.encode(w);
                0_i64.encode(w);
            }),
        )
        .run_async()
        .unwrap_err();
    assert_eq!(
        CalcError::decode_exact(&err).unwrap(),
        CalcError::DivideByZero
    );

    let unit = rt
        .call_object("Calculator", "slow_unit", handle, &[])
        .run_async()
        .unwrap();
    assert!(unit.is_empty());
    let len = rt
        .call_object("Calculator", "log_len", handle, &[])
        .sync_ok();
    assert_eq!(u32::decode_exact(&len).unwrap(), 1);
}

fn u32_items(items: Vec<Result<Vec<u8>, Vec<u8>>>) -> Vec<Result<u32, CalcError>> {
    items
        .into_iter()
        .map(|item| match item {
            Ok(bytes) => Ok(u32::decode_exact(&bytes).unwrap()),
            Err(bytes) => Err(CalcError::decode_exact(&bytes).unwrap()),
        })
        .collect()
}

#[test]
fn stream_methods_map_every_item() {
    let (rt, handle) = runtime_with_calculator();
    let items = rt
        .call_object("Calculator", "counts", handle, &args(|w| 3_u32.encode(w)))
        .run_stream();
    assert_eq!(u32_items(items), [Ok(0), Ok(1), Ok(2)]);
}

#[test]
fn result_stream_methods_fail_before_the_stream_opens() {
    let (rt, handle) = runtime_with_calculator();
    let items = rt
        .call_object(
            "Calculator",
            "try_counts",
            handle,
            &args(|w| 2_i64.encode(w)),
        )
        .run_stream();
    assert_eq!(u32_items(items), [Ok(0), Ok(1)]);
    let err = rt
        .call_object(
            "Calculator",
            "try_counts",
            handle,
            &args(|w| (-1_i64).encode(w)),
        )
        .sync_err();
    assert_eq!(
        CalcError::decode_exact(&err).unwrap(),
        CalcError::NegativeStart(-1)
    );
}

#[test]
fn async_stream_methods_open_lazily() {
    let (rt, handle) = runtime_with_calculator();
    let items = rt
        .call_object(
            "Calculator",
            "later_counts",
            handle,
            &args(|w| 2_u32.encode(w)),
        )
        .run_stream();
    assert_eq!(u32_items(items), [Ok(0), Ok(1)]);

    let items = rt
        .call_object(
            "Calculator",
            "try_later_counts",
            handle,
            &args(|w| 2_i64.encode(w)),
        )
        .run_stream();
    assert_eq!(u32_items(items), [Ok(0), Ok(1)]);
    // An error after `await` becomes the single error item of the stream.
    let items = rt
        .call_object(
            "Calculator",
            "try_later_counts",
            handle,
            &args(|w| (-3_i64).encode(w)),
        )
        .run_stream();
    assert_eq!(u32_items(items), [Err(CalcError::NegativeStart(-3))]);
}

#[test]
fn private_and_crate_visible_functions_are_not_exposed() {
    let schema = collect_schema("objects-test");
    let calc = schema
        .objects
        .iter()
        .find(|o| o.name == "Calculator")
        .unwrap();
    let methods: Vec<&str> = calc.methods.iter().map(|m| m.name.as_str()).collect();
    assert!(!methods.contains(&"helper"));
    assert!(!methods.contains(&"crate_visible"));
    assert!(!methods.contains(&"ctx_is_alive"));
    let ctors: Vec<&str> = calc.constructors.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(ctors, ["new", "from_owned_ctx", "without_ctx", "checked"]);
    assert!(
        methods.contains(&"old_add"),
        "deprecated methods stay part of the API"
    );
    assert!(!methods.contains(&"only_in_tests"));
}

#[test]
fn object_meta_describes_signatures() {
    let schema = collect_schema("objects-test");
    let calc = schema
        .objects
        .iter()
        .find(|o| o.name == "Calculator")
        .unwrap();
    assert_eq!(calc.type_id, ids::type_id("Calculator"));
    assert_eq!(calc.docs, "A calculator.");
    assert!(calc.store.is_none());
    assert_eq!(
        <Calculator as UndraObject>::TYPE_ID,
        ids::type_id("Calculator")
    );
    assert_eq!(<Calculator as UndraObject>::NAME, "Calculator");

    let method = |name: &str| calc.methods.iter().find(|m| m.name == name).unwrap();
    let add = method("add");
    assert_eq!(add.method_id, ids::method_id("Calculator", "add"));
    assert_eq!(add.docs, "Adds to the base.");
    assert_eq!(
        add.params
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(add.returns, TypeRef::I64);
    assert!(!add.is_async && !add.takes_ctx);
    assert_eq!(
        method("divide").returns,
        TypeRef::result(TypeRef::I64, TypeRef::named("CalcError"))
    );
    assert_eq!(method("reset").returns, TypeRef::Unit);
    assert!(method("slow_add").is_async);
    assert_eq!(method("counts").returns, TypeRef::stream(TypeRef::U32));
    assert_eq!(
        method("try_later_counts").returns,
        TypeRef::result(TypeRef::stream(TypeRef::U32), TypeRef::named("CalcError"))
    );
    assert_eq!(
        method("describe").params[1].ty,
        TypeRef::map(TypeRef::String, TypeRef::Bool)
    );

    let ctor = |name: &str| calc.constructors.iter().find(|m| m.name == name).unwrap();
    assert!(ctor("new").takes_ctx);
    assert_eq!(ctor("new").params.len(), 1, "Ctx is not a schema parameter");
    assert_eq!(ctor("new").returns, TypeRef::named("Calculator"));
    assert_eq!(ctor("new").docs, "Creates one with a starting value.");
    assert!(ctor("from_owned_ctx").takes_ctx);
    assert!(!ctor("without_ctx").takes_ctx);
    assert_eq!(
        ctor("checked").returns,
        TypeRef::result(TypeRef::named("Calculator"), TypeRef::named("CalcError"))
    );
}

#[test]
fn dispatchers_are_registered_by_name() {
    let names: Vec<&str> = undra::meta::registrations()
        .filter_map(|r| match r {
            Registration::Object(o) => Some(o.name),
            _ => None,
        })
        .collect();
    assert!(names.contains(&"Calculator"));
    assert!(names.contains(&"Other"));
}

// ---------------------------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------------------------

/// Says hello.
#[k::api]
pub fn greet(name: String) -> String {
    format!("hello {name}")
}

#[k::api]
pub async fn greet_later(ctx: &Ctx, name: String, shout: bool) -> Result<String, CalcError> {
    let _ = ctx.clone();
    std::future::ready(()).await;
    if name.is_empty() {
        return Err(CalcError::DivideByZero);
    }
    Ok(if shout { name.to_uppercase() } else { name })
}

#[k::api]
pub fn ticks(ctx: Ctx, n: u32) -> impl Stream<Item = String> + Send {
    let _ = ctx;
    stream_of((0..n).map(|i| format!("tick {i}")).collect())
}

#[k::api]
pub fn noop() {}

#[test]
fn free_functions_dispatch() {
    let rt = Runtime::new();
    let out = rt
        .call_function("greet", &args(|w| "undra".to_owned().encode(w)))
        .sync_ok();
    assert_eq!(String::decode_exact(&out).unwrap(), "hello undra");
    assert!(rt.call_function("noop", &[]).sync_ok().is_empty());
    let reason = rt.call_function("greet", &[]).bad_request();
    assert!(reason.contains("argument `name` of `greet`"), "{reason}");
}

#[test]
fn async_free_functions_get_the_context() {
    let rt = Runtime::new();
    let ok = rt
        .call_function(
            "greet_later",
            &args(|w| {
                "abc".to_owned().encode(w);
                true.encode(w);
            }),
        )
        .run_async()
        .unwrap();
    assert_eq!(String::decode_exact(&ok).unwrap(), "ABC");
    let err = rt
        .call_function(
            "greet_later",
            &args(|w| {
                String::new().encode(w);
                false.encode(w);
            }),
        )
        .run_async()
        .unwrap_err();
    assert_eq!(
        CalcError::decode_exact(&err).unwrap(),
        CalcError::DivideByZero
    );
}

#[test]
fn stream_free_functions_dispatch() {
    let rt = Runtime::new();
    let items = rt
        .call_function("ticks", &args(|w| 2_u32.encode(w)))
        .run_stream();
    let decoded: Vec<String> = items
        .into_iter()
        .map(|i| String::decode_exact(&i.unwrap()).unwrap())
        .collect();
    assert_eq!(decoded, ["tick 0", "tick 1"]);
}

#[test]
fn function_meta_describes_signatures() {
    let schema = collect_schema("objects-test");
    let function = |name: &str| schema.functions.iter().find(|f| f.name == name).unwrap();
    let greet = function("greet");
    assert_eq!(greet.method_id, ids::function_id("greet"));
    assert_eq!(greet.docs, "Says hello.");
    assert_eq!(greet.returns, TypeRef::String);
    assert!(!greet.takes_ctx && !greet.is_async);
    let later = function("greet_later");
    assert!(later.takes_ctx && later.is_async);
    assert_eq!(later.params.len(), 2);
    assert_eq!(function("ticks").returns, TypeRef::stream(TypeRef::String));
    assert!(function("ticks").takes_ctx);
    assert_eq!(function("noop").returns, TypeRef::Unit);
}

#[test]
fn the_registered_schema_validates() {
    // Ctx-free, resolvable, well-formed: the macros only emit what `Schema::validate` accepts.
    collect_schema("objects-test")
        .validate()
        .unwrap_or_else(|errors| panic!("{errors:#?}"));
}
