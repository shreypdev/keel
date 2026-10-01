//! The application core the benchmarks run against: real `#[undra::api]` / `#[undra::store]`
//! output, exactly what an app would write, so the numbers include the generated dispatchers,
//! codecs and change-set plumbing rather than hand-rolled stand-ins (constitution R10 in
//! spirit: bench through the public surface).
#![allow(missing_docs, dead_code)]

use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use undra::meta::ids;
use undra::prelude::*;
use undra::runtime::{Port, Stream, Subscription};
use undra::wire::{Decode, Encode};

// ---------------------------------------------------------------------------------------------
// Wire fixtures
// ---------------------------------------------------------------------------------------------

/// A small record: five fields of mixed kinds, about 50 bytes encoded.
#[undra::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Record5 {
    pub id: u64,
    pub name: String,
    pub score: f64,
    pub active: bool,
    pub created: Timestamp,
}

/// The blueprint's "1 KB record": exactly 1,024 encoded bytes (asserted in `fixtures` tests and
/// when the workloads are built).
#[undra::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Record1k {
    pub id: Uuid,
    pub title: String,
    pub body: String,
    pub views: u32,
    pub rating: f64,
    pub pinned: bool,
}

/// A data enum: unit, one-field and multi-field variants.
#[undra::api]
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Empty,
    Circle { radius: f64 },
    Rect { w: f64, h: f64 },
    Label(String),
}

/// The 5-field record the wire benches encode.
pub fn record5() -> Record5 {
    Record5 {
        id: 0x0123_4567_89ab_cdef,
        name: "Ada Lovelace".to_owned(),
        score: 98.6,
        active: true,
        created: Timestamp(1_700_000_000_000),
    }
}

/// The 1 KB record the wire and dispatch benches move around.
pub fn record1k() -> Record1k {
    let body: String = (0..963_u32)
        .map(|i| char::from(b'a' + (i % 26) as u8))
        .collect();
    Record1k {
        id: Uuid([7; 16]),
        title: "A title of twenty-four c".to_owned(),
        body,
        views: 123_456,
        rating: 4.75,
        pinned: false,
    }
}

// ---------------------------------------------------------------------------------------------
// Dispatch fixtures
// ---------------------------------------------------------------------------------------------

/// An object with the smallest interesting methods: the floor of a handle method call.
pub struct Calculator {
    base: i64,
}

#[undra::api]
impl Calculator {
    pub fn new(base: i64) -> Self {
        Calculator { base }
    }

    /// Sync, primitive arguments and return: the blueprint's "handle method call" row.
    pub fn add(&self, a: i64, b: i64) -> i64 {
        self.base.wrapping_add(a).wrapping_add(b)
    }

    /// Async, ready at once: measures the executor hop, not a timer.
    pub async fn ready_add(&self, a: i64, b: i64) -> i64 {
        self.base.wrapping_add(a).wrapping_add(b)
    }

    /// A 1 KB record in, the same record out: the "1 KB record, round trip" row at the boundary.
    pub fn echo(&self, record: Record1k) -> Record1k {
        record
    }
}

/// A free function: no handle to look up.
#[undra::api]
pub fn add_one(n: u32) -> u32 {
    n.wrapping_add(1)
}

// ---------------------------------------------------------------------------------------------
// Store fixtures
// ---------------------------------------------------------------------------------------------

/// Declares a store of `Signal<u32>` fields and a method that writes every one of them in one
/// transaction: the blueprint's "change-set with 100 dirty signals".
macro_rules! wide_store {
    ($name:ident; $($field:ident),+ $(,)?) => {
        #[undra::store]
        pub struct $name {
            $( $field: Signal<u32>, )+
        }

        #[undra::api(store)]
        #[allow(clippy::new_without_default)]
        impl $name {
            pub fn new() -> Self {
                $name { $( $field: Signal::new(0), )+ }
            }

            /// Writes every signal in one transaction.
            pub fn bump_all(&self) {
                txn(|| {
                    $( self.$field.update(|n| *n = n.wrapping_add(1)); )+
                });
            }
        }
    };
}

wide_store!(Wide100;
    s00, s01, s02, s03, s04, s05, s06, s07, s08, s09,
    s10, s11, s12, s13, s14, s15, s16, s17, s18, s19,
    s20, s21, s22, s23, s24, s25, s26, s27, s28, s29,
    s30, s31, s32, s33, s34, s35, s36, s37, s38, s39,
    s40, s41, s42, s43, s44, s45, s46, s47, s48, s49,
    s50, s51, s52, s53, s54, s55, s56, s57, s58, s59,
    s60, s61, s62, s63, s64, s65, s66, s67, s68, s69,
    s70, s71, s72, s73, s74, s75, s76, s77, s78, s79,
    s80, s81, s82, s83, s84, s85, s86, s87, s88, s89,
    s90, s91, s92, s93, s94, s95, s96, s97, s98, s99,
);

/// A list row.
#[undra::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: u64,
    pub title: String,
    pub done: bool,
}

/// A store with one keyed list: what a todo screen, a feed or a chat is.
#[undra::store]
pub struct Feed {
    #[undra(key = "id")]
    items: Signal<Vec<Item>>,
}

#[undra::api(store)]
#[allow(clippy::new_without_default)]
impl Feed {
    pub fn new() -> Self {
        Feed {
            items: Signal::new(Vec::new()),
        }
    }

    /// Replaces the list with `count` rows whose titles are `title_len` characters long; ids
    /// count up from 1 and are spaced by 2, so a new id fits anywhere.
    pub fn seed(&self, count: u32, title_len: u32) {
        let rows = (0..count)
            .map(|n| Item {
                id: u64::from(n) * 2 + 1,
                title: title_of(n, title_len),
                done: false,
            })
            .collect();
        self.items.set(rows);
    }

    /// Inserts `item` so it ends up at `index`. A recorded list operation: one `Insert` op, however
    /// long the list is (ADR-027). This is what store code written against the recorded API does.
    pub fn insert_at(&self, index: u32, item: Item) {
        self.items.insert(index as usize, item);
    }

    /// Removes the row at `index`. Recorded.
    pub fn remove_at(&self, index: u32) {
        self.items.remove(index as usize);
    }

    /// Changes the title of the row at `index`. Recorded: one `Update` op.
    pub fn rename(&self, index: u32, title: String) {
        self.items
            .update_at(index as usize, |row| row.title = title);
    }

    /// Moves the row at `from` so it ends up at `to`. Recorded: one `Move` op.
    pub fn move_item(&self, from: u32, to: u32) {
        self.items.move_item(from as usize, to as usize);
    }

    /// `rename` through the raw `update`: the list is compared with what the host has (O(list)),
    /// the path for edits no recorded operation can express.
    pub fn rename_raw(&self, index: u32, title: String) {
        self.items.update(|rows| rows[index as usize].title = title);
    }
}

/// A deterministic title of exactly `len` characters.
pub fn title_of(n: u32, len: u32) -> String {
    let mut title = format!("item {n} ");
    while title.len() < len as usize {
        title.push('x');
    }
    title.truncate(len as usize);
    title
}

// ---------------------------------------------------------------------------------------------
// Harsh-conditions fixtures (`stress/*`, `bench/common/stress.rs`, the soak binary)
// ---------------------------------------------------------------------------------------------

/// One observed `Signal<u64>`: the firehose. A change-set of a single `u64` entry is 37 bytes
/// (12 header + 17 entry header + 8 value), which the scenarios assert.
#[undra::store]
pub struct Ticker {
    value: Signal<u64>,
}

#[undra::api(store)]
#[allow(clippy::new_without_default)]
impl Ticker {
    pub fn new() -> Self {
        Ticker {
            value: Signal::new(0),
        }
    }

    /// Writes `value`: one implicit transaction, one change-set when observed. Written through
    /// `update` (in place), so the write itself allocates nothing and the allocations the gate in
    /// `crates/undra-ffi/tests/commit_alloc.rs` counts are the commit's alone.
    pub fn set(&self, value: u64) {
        self.value.update(|v| *v = value);
    }

    /// `count` implicit transactions in one call, each adding one: what a core-side producer (a
    /// socket task, a simulation) does. `count` change-sets when observed.
    pub fn burst(&self, count: u32) {
        for _ in 0..count {
            self.value.update(|v| *v = v.wrapping_add(1));
        }
    }

    /// The current value (for the scenarios' final-state checks).
    pub fn current(&self) -> u64 {
        self.value.get()
    }
}

/// What `Churn` does, in a fixed order: four updates, two inserts, two removes and two moves per
/// ten operations, so the list is back to its seeded length after every tenth.
const CHURN_CYCLE: [ChurnOp; 10] = [
    ChurnOp::Update,
    ChurnOp::Insert,
    ChurnOp::Update,
    ChurnOp::Move,
    ChurnOp::Remove,
    ChurnOp::Update,
    ChurnOp::Insert,
    ChurnOp::Move,
    ChurnOp::Update,
    ChurnOp::Remove,
];

#[derive(Clone, Copy)]
enum ChurnOp {
    Update,
    Insert,
    Remove,
    Move,
}

/// The bookkeeping of `Churn`: a seeded xorshift64 generator (no ambient randomness: a run is
/// reproducible), the position in the cycle, the next unused id and the list length.
struct ChurnState {
    rng: u64,
    step: u32,
    next_id: u64,
    len: usize,
}

impl Default for ChurnState {
    fn default() -> Self {
        ChurnState {
            rng: ChurnState::SEED,
            step: 0,
            next_id: 1,
            len: 0,
        }
    }
}

impl ChurnState {
    const SEED: u64 = 0x9E37_79B9_7F4A_7C15;

    fn next(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    /// A position in `0..n` (`n > 0`).
    fn pick(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A store with one keyed list that is changed, one recorded operation per transaction, in a
/// fixed cycle at seeded random positions: what a chat, a feed or a live board does under load.
#[undra::store]
pub struct Churn {
    #[undra(key = "id")]
    rows: Signal<Vec<Item>>,
    state: Mutex<ChurnState>,
}

#[undra::api(store)]
#[allow(clippy::new_without_default)]
impl Churn {
    pub fn new() -> Self {
        Churn {
            rows: Signal::new(Vec::new()),
            state: Mutex::new(ChurnState::default()),
        }
    }

    /// Replaces the list with `count` rows (ids `1..=count`) and restarts the generator.
    pub fn seed(&self, count: u32) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        *state = ChurnState {
            rng: ChurnState::SEED,
            step: 0,
            next_id: u64::from(count) + 1,
            len: count as usize,
        };
        let rows = (0..count)
            .map(|n| Item {
                id: u64::from(n) + 1,
                title: title_of(n, 24),
                done: false,
            })
            .collect();
        self.rows.set(rows);
    }

    /// `count` operations, each its own transaction (one change-set when observed), following
    /// the fixed cycle. Every tenth operation leaves the list at its seeded length.
    pub fn churn(&self, count: u32) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let state: &mut ChurnState = &mut guard;
        for _ in 0..count {
            let mut op = CHURN_CYCLE[(state.step % 10) as usize];
            state.step = state.step.wrapping_add(1);
            // A list too short to update, remove or move from can only grow.
            if state.len < 2 {
                op = ChurnOp::Insert;
            }
            match op {
                ChurnOp::Update => {
                    let at = state.pick(state.len);
                    self.rows.update_at(at, |row| row.done = !row.done);
                }
                ChurnOp::Insert => {
                    let at = state.pick(state.len + 1);
                    let id = state.next_id;
                    state.next_id += 1;
                    state.len += 1;
                    self.rows.insert(
                        at,
                        Item {
                            id,
                            title: title_of(id as u32, 24),
                            done: false,
                        },
                    );
                }
                ChurnOp::Remove => {
                    let at = state.pick(state.len);
                    state.len -= 1;
                    self.rows.remove(at);
                }
                ChurnOp::Move => {
                    let from = state.pick(state.len);
                    // A different position, so the move is a real change (a move onto itself
                    // commits nothing).
                    let to = (from + 1 + state.pick(state.len - 1)) % state.len;
                    self.rows.move_item(from, to);
                }
            }
        }
    }

    /// The ids of the rows, in order (for the final equality checks).
    pub fn ids(&self) -> Vec<u64> {
        self.rows
            .with(|rows| rows.iter().map(|row| row.id).collect())
    }

    /// The rows themselves, in order: what a host mirror must equal field for field (an
    /// `Update` changes a row's content and never its id, so comparing ids alone cannot see a
    /// lost one).
    pub fn rows_now(&self) -> Vec<Item> {
        self.rows.get()
    }
}

/// An object whose stream is always ready: its rate is whatever the core polls, far above any
/// push source. It counts what it produced, so a scenario can prove that the core never
/// produces more than one item beyond the consumer's credit.
pub struct Producer {
    produced: Arc<AtomicU64>,
}

#[undra::api]
#[allow(clippy::new_without_default)]
impl Producer {
    pub fn new() -> Self {
        Producer {
            produced: Arc::new(AtomicU64::new(0)),
        }
    }

    /// `0, 1, 2, ..` up to `count` (`u64::MAX` is, for practical purposes, forever).
    pub fn numbers(&self, count: u64) -> impl Stream<Item = u64> {
        Numbers {
            next: 0,
            count,
            produced: self.produced.clone(),
        }
    }

    /// How many items the core has pulled out of every stream of this object so far.
    pub fn produced(&self) -> u64 {
        self.produced.load(Ordering::Relaxed)
    }
}

struct Numbers {
    next: u64,
    count: u64,
    produced: Arc<AtomicU64>,
}

impl Stream for Numbers {
    type Item = u64;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<u64>> {
        if self.next < self.count {
            self.next += 1;
            self.produced.fetch_add(1, Ordering::Relaxed);
            Poll::Ready(Some(self.next - 1))
        } else {
            Poll::Ready(None)
        }
    }
}

/// The foreign port `Fetcher` calls. Not a `#[undra::port]` (there is no schema entry to keep in
/// step): the scenarios bind it with `Runtime::bind_foreign_port` and answer it from threads.
pub const SOURCE_PORT: u32 = 0x4B45_454C;

/// A store whose async method awaits a foreign port call and adds the reply to a signal: the
/// shape of "fetch something, then record it", run hundreds at a time.
#[undra::store]
pub struct Fetcher {
    ctx: Ctx,
    total: Signal<u64>,
}

#[undra::api(store)]
impl Fetcher {
    pub fn new(ctx: Ctx) -> Self {
        Fetcher {
            ctx,
            total: Signal::new(0),
        }
    }

    /// Asks the host for a number (`SOURCE_PORT`, method 1, argument `i`), adds it to `total`
    /// and returns it. A reply that is lost or unreadable adds nothing, so a lost completion
    /// shows up as a wrong total.
    pub async fn fetch(&self, i: u32) -> u64 {
        let reply = self.ctx.port_call(SOURCE_PORT, 1, i.encode_to_vec()).await;
        let value = reply
            .ok()
            .and_then(|body| u64::decode_exact(&body).ok())
            .unwrap_or(0);
        self.total.update(|total| *total += value);
        value
    }

    /// Adds one to `total` and returns: a synchronous write to the store the `fetch`
    /// completions write, made from a host thread through `call_sync` while they commit on the
    /// core thread (the contended completions scenario).
    pub fn bump(&self) {
        self.total.update(|total| *total += 1);
    }

    /// The running total (for the scenarios' final-state checks).
    pub fn total_now(&self) -> u64 {
        self.total.get()
    }
}

/// A host-to-core event, as a socket or a sensor feed delivers it.
#[undra::port(event)]
pub trait Ticks {
    fn tick(&self, value: u64);
}

/// The port id, method id and payload of `Ticks::tick(value)`, what a host passes to
/// `Runtime::event`.
pub fn tick_event(value: u64) -> (u32, u32, Vec<u8>) {
    (
        <dyn Ticks as Port>::PORT_ID,
        ids::port_method_id("Ticks", "tick"),
        encode_ticks_tick_event(value),
    )
}

/// A store that subscribes to `Ticks::tick` when it is built and writes each value into an
/// observed signal: the path a WebSocket or sensor feed takes into the core.
#[undra::store(restore = "Self::rebuild")]
pub struct TickSink {
    value: Signal<u64>,
    subscription: Subscription,
}

#[undra::api(store)]
impl TickSink {
    pub fn new(ctx: Ctx) -> Self {
        Self::rebuild(ctx, Signal::new(0))
    }

    fn rebuild(ctx: Ctx, value: Signal<u64>) -> Self {
        let target = value.clone();
        let subscription = on_ticks_tick(&ctx, move |_ctx, tick| target.update(|v| *v = tick));
        TickSink {
            value,
            subscription,
        }
    }

    /// The last value received (for the scenarios' final-state checks).
    pub fn current(&self) -> u64 {
        self.value.get()
    }
}
