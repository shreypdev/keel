//! The application core the benchmarks run against: real `#[keel::api]` / `#[keel::store]`
//! output, exactly what an app would write, so the numbers include the generated dispatchers,
//! codecs and change-set plumbing rather than hand-rolled stand-ins (constitution R10 in
//! spirit: bench through the public surface).
#![allow(missing_docs, dead_code)]

use keel::prelude::*;

// ---------------------------------------------------------------------------------------------
// Wire fixtures
// ---------------------------------------------------------------------------------------------

/// A small record: five fields of mixed kinds, about 50 bytes encoded.
#[keel::api]
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
#[keel::api]
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
#[keel::api]
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

#[keel::api]
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
#[keel::api]
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
        #[keel::store]
        pub struct $name {
            $( $field: Signal<u32>, )+
        }

        #[keel::api(store)]
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
#[keel::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: u64,
    pub title: String,
    pub done: bool,
}

/// A store with one keyed list: what a todo screen, a feed or a chat is.
#[keel::store]
pub struct Feed {
    #[keel(key = "id")]
    items: Signal<Vec<Item>>,
}

#[keel::api(store)]
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

    /// Inserts `item` so it ends up at `index`.
    pub fn insert_at(&self, index: u32, item: Item) {
        self.items.update(|rows| rows.insert(index as usize, item));
    }

    /// Removes the row at `index`.
    pub fn remove_at(&self, index: u32) {
        self.items.update(|rows| {
            rows.remove(index as usize);
        });
    }

    /// Changes the title of the row at `index`.
    pub fn rename(&self, index: u32, title: String) {
        self.items.update(|rows| rows[index as usize].title = title);
    }

    /// Moves the row at `from` so it ends up at `to`.
    pub fn move_item(&self, from: u32, to: u32) {
        self.items.update(|rows| {
            let row = rows.remove(from as usize);
            rows.insert(to as usize, row);
        });
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
