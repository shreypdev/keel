//! The lab: small functions and one object that exercise every part of the boundary.
//!
//! The apps show a few of them on a diagnostics screen, and the contract tests (`contract-tests/`)
//! call all of them from Swift, Kotlin and TypeScript to prove the platforms agree:
//!
//! * [`echo_primitives`], [`echo_composite`] and [`echo_figure`] return what they are given, so a
//!   value makes the round trip through each language's codec;
//! * [`add`], [`greet`], [`area`] and [`parse_count`] are synchronous calls, the last two with a
//!   typed error;
//! * [`add_later`] and [`fail_later`] are asynchronous (they wait on the `Timer` port);
//! * [`explode`] and [`explode_later`] panic, to show the panic is contained at the boundary;
//! * [`Probe`] counts what the core sees of cancelled calls and of streams with backpressure.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::task::{Context, Poll};

use undra::prelude::*;
use undra::runtime::Stream;

/// One value of every primitive the wire has, so a single call covers them all.
#[undra::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Primitives {
    /// A boolean.
    pub flag: bool,
    /// An `i8`.
    pub tiny: i8,
    /// An `i16`.
    pub small: i16,
    /// An `i32`.
    pub int: i32,
    /// An `i64`: a `bigint` in TypeScript.
    pub long: i64,
    /// A `u8`.
    pub byte: u8,
    /// A `u16`.
    pub word: u16,
    /// A `u32`.
    pub dword: u32,
    /// A `u64`: a `bigint` in TypeScript.
    pub qword: u64,
    /// An `f32`.
    pub single: f32,
    /// An `f64`.
    pub double: f64,
    /// A string, UTF-8 on the wire.
    pub text: String,
    /// Raw bytes.
    pub blob: Bytes,
    /// A span of time, whole nanoseconds on the wire.
    pub span: Duration,
    /// A moment, milliseconds since the Unix epoch on the wire.
    pub at: Timestamp,
    /// A UUID, sixteen bytes on the wire.
    pub id: Uuid,
}

/// A shape: an enum with named fields, a tuple variant and a unit variant.
#[undra::api]
#[derive(Clone, Debug, PartialEq)]
pub enum Figure {
    /// A circle.
    Circle {
        /// Distance from the centre to the edge.
        radius: f64,
    },
    /// A rectangle.
    Rect {
        /// Horizontal extent.
        width: f64,
        /// Vertical extent.
        height: f64,
    },
    /// A shape that is only a name.
    Label(String),
    /// No shape at all.
    Empty,
}

/// A record that nests the other kinds: lists, options, maps, enums with data and a map with
/// integer keys.
#[undra::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Composite {
    /// A name.
    pub name: String,
    /// Several strings.
    pub tags: Vec<String>,
    /// Maybe a figure.
    pub figure: Option<Figure>,
    /// Figures, in order.
    pub history: Vec<Figure>,
    /// A map keyed by string.
    pub scores: BTreeMap<String, i32>,
    /// A map keyed by integer.
    pub names: BTreeMap<u32, String>,
    /// Maybe a number.
    pub limit: Option<u32>,
}

/// Why a lab call failed.
#[undra::error]
#[derive(Clone, Debug, PartialEq)]
pub enum LabError {
    /// Nothing was given.
    #[error("nothing was given")]
    Empty,
    /// The text is longer than `max` characters.
    #[error("longer than {max} characters")]
    TooLong {
        /// The longest accepted length.
        max: u32,
    },
    /// The text is not a number.
    #[error("`{0}` is not a number")]
    NotANumber(String),
    /// A failure chosen by the caller.
    #[error("rejected with code {code}: {reason}")]
    Rejected {
        /// The code the caller asked for.
        code: i32,
        /// A reason.
        reason: String,
    },
}

/// The name and version of the core.
#[undra::api]
pub fn version() -> String {
    "undra playground 1".to_owned()
}

/// Returns `value` unchanged.
#[undra::api]
pub fn echo_primitives(value: Primitives) -> Primitives {
    value
}

/// Returns `value` unchanged.
#[undra::api]
pub fn echo_composite(value: Composite) -> Composite {
    value
}

/// Returns `value` unchanged.
#[undra::api]
pub fn echo_figure(value: Figure) -> Figure {
    value
}

/// Does nothing and returns nothing: a call with neither arguments nor a result.
#[undra::api]
pub fn ping() {}

/// Adds two numbers, wrapping on overflow: a synchronous call with primitive arguments.
#[undra::api]
pub fn add(a: i32, b: i32) -> i32 {
    a.wrapping_add(b)
}

/// A greeting.
#[undra::api]
pub fn greet(name: String) -> String {
    format!("Hello, {name}, from the playground core")
}

/// The area of a circle or a rectangle. A label and an empty figure have none.
#[undra::api]
pub fn area(figure: Figure) -> Result<f64, LabError> {
    match figure {
        Figure::Circle { radius } => Ok(std::f64::consts::PI * radius * radius),
        Figure::Rect { width, height } => Ok(width * height),
        Figure::Label(name) => Err(LabError::Rejected {
            code: 1,
            reason: format!("`{name}` has no area"),
        }),
        Figure::Empty => Err(LabError::Empty),
    }
}

/// Reads an unsigned number of at most nine digits.
#[undra::api]
pub fn parse_count(text: String) -> Result<u32, LabError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(LabError::Empty);
    }
    if text.chars().count() > 9 {
        return Err(LabError::TooLong { max: 9 });
    }
    text.parse()
        .map_err(|_| LabError::NotANumber(text.to_owned()))
}

/// Adds two numbers after `delay_ms` milliseconds: an asynchronous call that waits on the `Timer`
/// port.
#[undra::api]
pub async fn add_later(ctx: &Ctx, a: i32, b: i32, delay_ms: u32) -> i32 {
    ctx.sleep(Duration::from_millis(u64::from(delay_ms))).await;
    a.wrapping_add(b)
}

/// Fails with `LabError::Rejected { code, .. }` after `delay_ms` milliseconds: an asynchronous
/// call with a typed error.
#[undra::api]
pub async fn fail_later(ctx: &Ctx, delay_ms: u32, code: i32) -> Result<u32, LabError> {
    ctx.sleep(Duration::from_millis(u64::from(delay_ms))).await;
    Err(LabError::Rejected {
        code,
        reason: "on purpose".to_owned(),
    })
}

/// Panics with `reason`. The boundary turns the panic into a reply (status 2), never into a
/// crash, on the platforms that can unwind (R6).
#[undra::api]
pub fn explode(reason: String) -> u32 {
    panic!("{reason}")
}

/// Panics with `reason` after `delay_ms` milliseconds, inside an asynchronous call.
#[undra::api]
pub async fn explode_later(ctx: &Ctx, delay_ms: u32, reason: String) -> u32 {
    ctx.sleep(Duration::from_millis(u64::from(delay_ms))).await;
    panic!("{reason}")
}

/// What a [`Probe`] has seen.
#[undra::api]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProbeCounters {
    /// Calls that started running in the core.
    pub started: u32,
    /// Calls that ran to the end.
    pub completed: u32,
    /// Calls the core dropped before they ended, because the platform cancelled them.
    pub cancelled: u32,
    /// Items the core produced for streams (whether or not the platform has read them yet).
    pub produced: u32,
}

/// The counters a probe and its in-flight calls share.
#[derive(Default)]
struct Shared {
    started: AtomicU32,
    completed: AtomicU32,
    cancelled: AtomicU32,
    produced: AtomicU32,
}

/// Counts a call as cancelled when it is dropped before [`Flight::land`].
struct Flight {
    shared: Arc<Shared>,
    landed: bool,
}

impl Flight {
    /// A call that just started.
    fn take_off(shared: &Arc<Shared>) -> Flight {
        shared.started.fetch_add(1, Ordering::SeqCst);
        Flight {
            shared: shared.clone(),
            landed: false,
        }
    }

    /// The call ran to its end.
    fn land(mut self) {
        self.landed = true;
        self.shared.completed.fetch_add(1, Ordering::SeqCst);
    }
}

impl Drop for Flight {
    fn drop(&mut self) {
        if !self.landed {
            self.shared.cancelled.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// An object that reports how the core saw the calls made on it: what cancellation and stream
/// backpressure do inside the core, which no amount of watching from the platform can show.
pub struct Probe {
    ctx: Ctx,
    shared: Arc<Shared>,
}

#[undra::api]
impl Probe {
    /// A probe with every counter at zero.
    pub fn new(ctx: Ctx) -> Self {
        Probe {
            ctx,
            shared: Arc::new(Shared::default()),
        }
    }

    /// Waits forever. The only way it ends is cancellation, which the `cancelled` counter shows.
    pub async fn hang(&self) -> u32 {
        let flight = Flight::take_off(&self.shared);
        std::future::pending::<()>().await;
        flight.land();
        0
    }

    /// Waits `ms` milliseconds and returns it. Cancelled before that, it counts as cancelled.
    pub async fn wait(&self, ms: u32) -> u32 {
        let flight = Flight::take_off(&self.shared);
        self.ctx.sleep(Duration::from_millis(u64::from(ms))).await;
        flight.land();
        ms
    }

    /// A stream of the numbers `0..count`, produced one per poll. The runtime sends an item only
    /// against credit the platform granted and polls at most one item ahead of it, so `produced`
    /// stays within one of what the platform has asked for, however large `count` is.
    pub fn ticks(&self, count: u32) -> impl Stream<Item = u32> {
        Ticks {
            next: 0,
            count,
            shared: self.shared.clone(),
        }
    }

    /// What the probe has seen so far.
    pub fn counters(&self) -> ProbeCounters {
        ProbeCounters {
            started: self.shared.started.load(Ordering::SeqCst),
            completed: self.shared.completed.load(Ordering::SeqCst),
            cancelled: self.shared.cancelled.load(Ordering::SeqCst),
            produced: self.shared.produced.load(Ordering::SeqCst),
        }
    }

    /// Sets every counter back to zero.
    pub fn reset(&self) {
        for counter in [
            &self.shared.started,
            &self.shared.completed,
            &self.shared.cancelled,
            &self.shared.produced,
        ] {
            counter.store(0, Ordering::SeqCst);
        }
    }
}

/// The stream behind [`Probe::ticks`].
struct Ticks {
    next: u32,
    count: u32,
    shared: Arc<Shared>,
}

impl Stream for Ticks {
    type Item = u32;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<u32>> {
        if self.next < self.count {
            let item = self.next;
            self.next += 1;
            self.shared.produced.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(Some(item))
        } else {
            Poll::Ready(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use undra::meta::ids;
    use undra::runtime::testing::TestRuntime;
    use undra::wire::payload::{CallTarget, ReplyStatus, StreamFlag};
    use undra::wire::{Decode, Encode};

    use super::*;

    fn function(name: &str) -> CallTarget {
        CallTarget::Function {
            method_id: ids::function_id(name),
        }
    }

    fn primitives() -> Primitives {
        Primitives {
            flag: true,
            tiny: -8,
            small: -16_000,
            int: -2_000_000_000,
            long: -9_000_000_000_000_000_000,
            byte: 255,
            word: 65_535,
            dword: 4_000_000_000,
            qword: 18_000_000_000_000_000_000,
            single: 1.5,
            double: -2.25e100,
            text: "héllo, wörld ✓".to_owned(),
            blob: Bytes(vec![0, 1, 2, 254, 255]),
            span: Duration::from_nanos(1_500_000_123),
            at: Timestamp(1_700_000_000_123),
            id: Uuid([
                0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 1, 2, 3, 4, 5, 6, 7, 8,
            ]),
        }
    }

    fn composite() -> Composite {
        Composite {
            name: "everything".to_owned(),
            tags: vec!["a".to_owned(), "".to_owned(), "ü".to_owned()],
            figure: Some(Figure::Rect {
                width: 2.0,
                height: 3.5,
            }),
            history: vec![
                Figure::Circle { radius: 1.0 },
                Figure::Label("x".to_owned()),
                Figure::Empty,
            ],
            scores: BTreeMap::from([("high".to_owned(), 99), ("low".to_owned(), -3)]),
            names: BTreeMap::from([(7, "seven".to_owned()), (1, "one".to_owned())]),
            limit: Some(7),
        }
    }

    /// Calls the free function `name` with encoded `args`.
    fn call(t: &TestRuntime, name: &str, args: &[u8]) -> (ReplyStatus, Vec<u8>) {
        let reply = t.call_sync(function(name), 1, args);
        (reply.status, reply.body)
    }

    #[test]
    fn values_make_the_round_trip() {
        let t = TestRuntime::new();
        let (status, body) = call(&t, "echo_primitives", &primitives().encode_to_vec());
        assert_eq!(status, ReplyStatus::Ok);
        assert_eq!(Primitives::decode_exact(&body).unwrap(), primitives());
        let (status, body) = call(&t, "echo_composite", &composite().encode_to_vec());
        assert_eq!(status, ReplyStatus::Ok);
        assert_eq!(Composite::decode_exact(&body).unwrap(), composite());
        for figure in [
            Figure::Empty,
            Figure::Label(String::new()),
            Figure::Circle {
                radius: f64::INFINITY,
            },
        ] {
            let (_, body) = call(&t, "echo_figure", &figure.encode_to_vec());
            assert_eq!(Figure::decode_exact(&body).unwrap(), figure);
        }
    }

    #[test]
    fn sync_calls_compute() {
        let t = TestRuntime::new();
        let mut args = 40i32.encode_to_vec();
        args.extend(2i32.encode_to_vec());
        let (status, body) = call(&t, "add", &args);
        assert_eq!(
            (status, i32::decode_exact(&body).unwrap()),
            (ReplyStatus::Ok, 42)
        );
        let mut wrap = i32::MAX.encode_to_vec();
        wrap.extend(1i32.encode_to_vec());
        assert_eq!(
            i32::decode_exact(&call(&t, "add", &wrap).1).unwrap(),
            i32::MIN
        );
        let (_, body) = call(&t, "greet", &"Ada".to_owned().encode_to_vec());
        assert!(
            String::decode_exact(&body)
                .unwrap()
                .starts_with("Hello, Ada")
        );
        assert_eq!(call(&t, "ping", &[]), (ReplyStatus::Ok, vec![]));
    }

    #[test]
    fn errors_are_typed_values() {
        let t = TestRuntime::new();
        let parse = |text: &str| call(&t, "parse_count", &text.to_owned().encode_to_vec());
        assert_eq!(u32::decode_exact(&parse("42").1).unwrap(), 42);
        let (status, body) = parse("");
        assert_eq!(
            (status, LabError::decode_exact(&body).unwrap()),
            (ReplyStatus::Error, LabError::Empty)
        );
        let (_, body) = parse("1234567890");
        assert_eq!(
            LabError::decode_exact(&body).unwrap(),
            LabError::TooLong { max: 9 }
        );
        let (_, body) = parse("4x2");
        assert_eq!(
            LabError::decode_exact(&body).unwrap(),
            LabError::NotANumber("4x2".into())
        );

        let (status, body) = call(&t, "area", &Figure::Label("hat".into()).encode_to_vec());
        assert_eq!(status, ReplyStatus::Error);
        assert!(matches!(
            LabError::decode_exact(&body).unwrap(),
            LabError::Rejected { code: 1, .. }
        ));
        let (_, body) = call(
            &t,
            "area",
            &Figure::Rect {
                width: 2.0,
                height: 4.0,
            }
            .encode_to_vec(),
        );
        assert_eq!(f64::decode_exact(&body).unwrap(), 8.0);
    }

    #[test]
    fn async_calls_wait_on_the_timer() {
        let t = TestRuntime::new();
        let mut args = 20i32.encode_to_vec();
        args.extend(22i32.encode_to_vec());
        args.extend(1_000u32.encode_to_vec());
        assert_eq!(t.call(function("add_later"), 5, &args), 0);
        t.run_pending();
        assert!(t.take_replies().is_empty(), "the timer has not fired");
        t.advance(Duration::from_secs(1));
        let replies = t.take_replies();
        assert_eq!(replies.len(), 1);
        assert_eq!(i32::decode_exact(&replies[0].body).unwrap(), 42);

        let mut args = 10u32.encode_to_vec();
        args.extend(7i32.encode_to_vec());
        assert_eq!(t.call(function("fail_later"), 6, &args), 0);
        t.advance(Duration::from_millis(10));
        let replies = t.take_replies();
        assert_eq!(replies[0].status, ReplyStatus::Error);
        assert_eq!(
            LabError::decode_exact(&replies[0].body).unwrap(),
            LabError::Rejected {
                code: 7,
                reason: "on purpose".into()
            }
        );
    }

    #[test]
    fn panics_are_replies_and_the_core_lives_on() {
        let t = TestRuntime::new();
        let (status, body) = call(&t, "explode", &"kaboom".to_owned().encode_to_vec());
        assert_eq!(status, ReplyStatus::Panic);
        assert!(String::from_utf8_lossy(&body).contains("kaboom"));
        let mut args = 1i32.encode_to_vec();
        args.extend(1i32.encode_to_vec());
        assert_eq!(i32::decode_exact(&call(&t, "add", &args).1).unwrap(), 2);

        let mut args = 5u32.encode_to_vec();
        args.extend("later".to_owned().encode_to_vec());
        assert_eq!(t.call(function("explode_later"), 9, &args), 0);
        t.advance(Duration::from_millis(5));
        let replies = t.take_replies();
        assert_eq!(replies[0].status, ReplyStatus::Panic);
    }

    fn probe(t: &TestRuntime) -> Handle {
        let reply = t.call_sync(
            CallTarget::Constructor {
                type_id: ids::type_id("Probe"),
                method_id: ids::method_id("Probe", "new"),
            },
            1,
            &[],
        );
        Handle::decode_exact(&reply.body).unwrap()
    }

    fn method(handle: Handle, name: &str) -> CallTarget {
        CallTarget::Method {
            handle,
            method_id: ids::method_id("Probe", name),
        }
    }

    fn counters(t: &TestRuntime, probe: Handle) -> ProbeCounters {
        let reply = t.call_sync(method(probe, "counters"), 99, &[]);
        ProbeCounters::decode_exact(&reply.body).unwrap()
    }

    #[test]
    fn a_cancelled_call_is_dropped_in_the_core() {
        let t = TestRuntime::new();
        let probe = probe(&t);
        assert_eq!(t.call(method(probe, "hang"), 7, &[]), 0);
        t.run_pending();
        assert_eq!(counters(&t, probe).started, 1);
        assert_eq!(counters(&t, probe).cancelled, 0);
        t.runtime().cancel(7);
        t.run_pending();
        let seen = counters(&t, probe);
        assert_eq!((seen.started, seen.completed, seen.cancelled), (1, 0, 1));
        assert_eq!(t.take_replies()[0].status, ReplyStatus::Cancelled);
    }

    #[test]
    fn a_call_that_finishes_is_not_cancelled() {
        let t = TestRuntime::new();
        let probe = probe(&t);
        assert_eq!(t.call(method(probe, "wait"), 7, &250u32.encode_to_vec()), 0);
        t.advance(Duration::from_millis(250));
        let seen = counters(&t, probe);
        assert_eq!((seen.started, seen.completed, seen.cancelled), (1, 1, 0));
        assert_eq!(u32::decode_exact(&t.take_replies()[0].body).unwrap(), 250);
    }

    #[test]
    fn a_stream_is_produced_only_as_fast_as_credit_allows() {
        let t = TestRuntime::new();
        let probe = probe(&t);
        assert_eq!(
            t.call(method(probe, "ticks"), 3, &1_000u32.encode_to_vec()),
            0
        );
        t.run_pending();
        // Initial credit is zero. The runtime polls before it checks credit, so the stream is
        // at most one item ahead of what was granted, and nothing has been sent.
        assert_eq!(counters(&t, probe).produced, 1);
        assert!(t.host().take_stream_items().is_empty());

        t.runtime().stream_credit(3, 16);
        t.run_pending();
        assert_eq!(counters(&t, probe).produced, 17);
        let items = t.host().take_stream_items();
        assert_eq!(items.len(), 16);
        assert_eq!(items[15].flag, StreamFlag::Item);
        assert_eq!(u32::decode_exact(&items[15].body).unwrap(), 15);

        // Granting more releases more, and only that much.
        t.runtime().stream_credit(3, 4);
        t.run_pending();
        assert_eq!(counters(&t, probe).produced, 21);
        assert_eq!(t.host().take_stream_items().len(), 4);
    }

    #[test]
    fn a_short_stream_ends() {
        let t = TestRuntime::new();
        let probe = probe(&t);
        assert_eq!(t.call(method(probe, "ticks"), 3, &3u32.encode_to_vec()), 0);
        t.runtime().stream_credit(3, 16);
        t.run_pending();
        let items = t.host().take_stream_items();
        let flags: Vec<StreamFlag> = items.iter().map(|i| i.flag).collect();
        assert_eq!(
            flags,
            [
                StreamFlag::Item,
                StreamFlag::Item,
                StreamFlag::Item,
                StreamFlag::End
            ]
        );
        assert_eq!(counters(&t, probe).produced, 3);
    }

    #[test]
    fn reset_zeroes_the_counters() {
        let t = TestRuntime::new();
        let probe = probe(&t);
        t.call(method(probe, "hang"), 7, &[]);
        t.run_pending();
        assert_eq!(
            t.call_sync(method(probe, "reset"), 8, &[]).status,
            ReplyStatus::Ok
        );
        assert_eq!(counters(&t, probe), ProbeCounters::default());
    }
}
