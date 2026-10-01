//! A seeded, deterministic workload over three derived views (ADR-039), shared by
//! `tests/derived_seeded.rs` (which checks it) and `examples/derived_vectors.rs` (which records it
//! for the platform runtimes: contract scenario S19 replays the recorded change-sets with each
//! runtime's own patch decoder and applier).
//!
//! The source is a `Signal<Vec<Row>>` of up to a few hundred rows; the views are
//!
//! * `open`: `filter(!done)`, unsorted;
//! * `ranked`: `filter_with(&cut, rank >= cut)` then `sort_by_key(rank % 4)`: heavy ties;
//! * `labels`: `map(Row -> Label)` then `sort_by_key(text)`: a mapped record, a `String` key.
//!
//! A SplitMix64 generator picks every operation from the seed: the recorded operations, with
//! occasional raw writes and parameter changes, in transactions of one to four operations. After
//! **every operation** each view's `get()` is checked against `filter + stable sort` of the source
//! (inside the transaction: a Rust read replays the operations so far), and after every commit a
//! host mirror that applies the delivered change-sets is checked against the same reference.
//!
//! # The recording (`UDV1`)
//!
//! Little-endian. `"UDV1"`, `views u32` (3), `records u32`, then per change-set the host receives:
//! `len u32`, the change-set payload (SPEC 3.5; the views are signals 0, 1 and 2 of one store;
//! `txn_id` renumbered 1, 2, 3, .. so the bytes depend on the seed only), and
//! `views x u64`: the FNV-1a 64 hash of each view's encoding (`Vec<Row>` / `Vec<Label>`, SPEC 3.1)
//! after applying it. The first record is the initial observe (full values).
#![allow(dead_code)]

use std::sync::Arc;

use undra_signals::testing::CaptureSink;
use undra_signals::{ALL_SIGNALS, DerivedList, Signal, StoreCell, next_txn_id, txn, with_sink};
use undra_wire::payload::{ChangeOp, ChangeSet};
use undra_wire::{Decode, Encode, KeyedPatch, Reader, WireError, Writer};

/// A source row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: u32,
    pub title: String,
    pub done: bool,
    pub rank: u32,
}

impl Encode for Row {
    fn encode(&self, w: &mut Writer) {
        self.id.encode(w);
        self.title.encode(w);
        self.done.encode(w);
        self.rank.encode(w);
    }
}

impl Decode for Row {
    const MIN_ENCODED_LEN: usize = 13;
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Row {
            id: u32::decode(r)?,
            title: String::decode(r)?,
            done: bool::decode(r)?,
            rank: u32::decode(r)?,
        })
    }
}

/// A mapped row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    pub id: u32,
    pub text: String,
}

impl Encode for Label {
    fn encode(&self, w: &mut Writer) {
        self.id.encode(w);
        self.text.encode(w);
    }
}

impl Decode for Label {
    const MIN_ENCODED_LEN: usize = 8;
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Label {
            id: u32::decode(r)?,
            text: String::decode(r)?,
        })
    }
}

pub fn label_of(row: &Row) -> Label {
    Label {
        id: row.id,
        text: format!("{}-{}", row.title, row.id % 7),
    }
}

/// The references.
pub fn ref_open(rows: &[Row]) -> Vec<Row> {
    rows.iter().filter(|r| !r.done).cloned().collect()
}

pub fn ref_ranked(rows: &[Row], cut: u32) -> Vec<Row> {
    let mut v: Vec<Row> = rows.iter().filter(|r| r.rank >= cut).cloned().collect();
    v.sort_by_key(|r| r.rank % 4);
    v
}

pub fn ref_labels(rows: &[Row]) -> Vec<Label> {
    let mut v: Vec<Label> = rows.iter().map(label_of).collect();
    v.sort_by_key(|l| l.text.clone());
    v
}

/// FNV-1a 64 of `bytes` (what the recording hashes each view's encoding with).
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub fn hash_of<T: Encode>(value: &T) -> u64 {
    let mut w = Writer::new();
    value.encode(&mut w);
    fnv1a64(w.as_slice())
}

/// SplitMix64: deterministic, seeded, no dependency.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (`n > 0`).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// What a run did, for the report.
#[derive(Debug, Default)]
pub struct Summary {
    pub ops: usize,
    pub transactions: usize,
    pub change_sets: usize,
    pub patches: u64,
    pub full_values: u64,
    pub rebuilds: u64,
    pub records: Vec<u8>,
}

const TITLES: [&str; 5] = ["alpha", "beta", "gamma", "delta", "eps"];

struct Views {
    cell: Arc<StoreCell>,
    rows: Signal<Vec<Row>>,
    cut: Signal<u32>,
    open: DerivedList<Row>,
    ranked: DerivedList<Row>,
    labels: DerivedList<Label>,
}

fn views() -> Views {
    let cell = StoreCell::new(0x5EED);
    cell.set_handle(0x0000_0001_0000_0019);
    let rows = Signal::new(Vec::<Row>::new());
    let cut = Signal::new(1_u32);
    let open = rows.derive().filter(|r: &Row| !r.done).build();
    let ranked = rows
        .derive()
        .filter_with(&cut, |cut, r: &Row| r.rank >= *cut)
        .sort_by_key(|r: &Row| r.rank % 4)
        .build();
    let labels = rows
        .derive()
        .map(label_of)
        .sort_by_key(|l: &Label| l.text.clone())
        .build();
    cell.attach_derived(&open, 0, |r: &Row| u64::from(r.id))
        .unwrap();
    cell.attach_derived(&ranked, 1, |r: &Row| u64::from(r.id))
        .unwrap();
    cell.attach_derived(&labels, 2, |l: &Label| u64::from(l.id))
        .unwrap();
    Views {
        cell,
        rows,
        cut,
        open,
        ranked,
        labels,
    }
}

/// The host: three mirrored views.
#[derive(Default)]
struct Host {
    open: Vec<Row>,
    ranked: Vec<Row>,
    labels: Vec<Label>,
}

fn apply<T: Decode + Clone>(list: &mut Vec<T>, op: ChangeOp, value: &[u8]) {
    match op {
        ChangeOp::Full => *list = Vec::<T>::decode_exact(value).expect("a full value decodes"),
        ChangeOp::KeyedPatch => {
            let mut r = Reader::new(value);
            let patch = KeyedPatch::<T>::decode(&mut r).expect("a patch decodes");
            r.finish().expect("no trailing bytes");
            patch
                .apply(list)
                .expect("a patch applies to what the host has");
        }
        other => panic!("unexpected {other:?}"),
    }
}

impl Host {
    fn apply_set(&mut self, set: &ChangeSet) {
        for e in &set.entries {
            match e.signal_id {
                0 => apply(&mut self.open, e.op, &e.value),
                1 => apply(&mut self.ranked, e.op, &e.value),
                2 => apply(&mut self.labels, e.op, &e.value),
                other => panic!("unexpected signal {other}"),
            }
        }
    }
}

/// Runs `ops` seeded operations; checks the views after every one and the host after every
/// commit; returns what happened and, when `record`, the recording.
pub fn run(seed: u64, ops: usize, record: bool) -> Summary {
    let v = views();
    let mut rng = Rng::new(seed);
    let mut host = Host::default();
    let mut summary = Summary::default();
    let mut next_id = 1_u32;
    let mut records = Vec::new();

    let write_record = |payload: &[u8], host: &Host, records: &mut Vec<u8>, number: u32| {
        if !record {
            return;
        }
        records.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
        // Transaction ids are process-wide; the recording numbers its change-sets from 1 instead,
        // so it is the same bytes for the same seed.
        records.extend_from_slice(&u64::from(number).to_le_bytes());
        records.extend_from_slice(&payload[8..]);
        for hash in [
            hash_of(&host.open),
            hash_of(&host.ranked),
            hash_of(&host.labels),
        ] {
            records.extend_from_slice(&hash.to_le_bytes());
        }
    };

    // The initial observe, as the change-set a runtime would hand its host.
    let mut entries = Writer::new();
    let count = v.cell.observe(ALL_SIGNALS, true, &mut entries);
    let mut payload = Writer::new();
    payload.write_u64(next_txn_id());
    payload.write_u32(count);
    payload.write_raw(entries.as_slice());
    let initial = undra_signals::testing::decode(payload.as_slice()).unwrap();
    host.apply_set(&initial);
    let mut record_count = 1_u32;
    write_record(payload.as_slice(), &host, &mut records, record_count);

    let sink = CaptureSink::new();
    let check = |v: &Views, when: &str| {
        let rows = v.rows.get();
        let cut = v.cut.get();
        assert_eq!(v.open.get(), ref_open(&rows), "open {when}");
        assert_eq!(v.ranked.get(), ref_ranked(&rows, cut), "ranked {when}");
        assert_eq!(v.labels.get(), ref_labels(&rows), "labels {when}");
    };

    while summary.ops < ops {
        let in_txn = 1 + rng.below(4);
        with_sink(sink.clone(), || {
            txn(|| {
                for _ in 0..in_txn {
                    one_op(&v, &mut rng, &mut next_id);
                    summary.ops += 1;
                    check(&v, &format!("after op {}", summary.ops));
                }
            })
        });
        summary.transactions += 1;
        for bytes in sink.take() {
            let set = undra_signals::testing::decode(&bytes).unwrap();
            host.apply_set(&set);
            summary.change_sets += 1;
            record_count += 1;
            write_record(&bytes, &host, &mut records, record_count);
        }
        let rows = v.rows.get();
        assert_eq!(
            host.open,
            ref_open(&rows),
            "host open after op {}",
            summary.ops
        );
        assert_eq!(
            host.ranked,
            ref_ranked(&rows, v.cut.get()),
            "host ranked after op {}",
            summary.ops
        );
        assert_eq!(
            host.labels,
            ref_labels(&rows),
            "host labels after op {}",
            summary.ops
        );
    }

    for stats in [v.open.stats(), v.ranked.stats(), v.labels.stats()] {
        summary.patches += stats.patches;
        summary.full_values += stats.full_values;
        summary.rebuilds += stats.rebuilds;
    }
    if record {
        let mut out = Vec::with_capacity(records.len() + 12);
        out.extend_from_slice(b"UDV1");
        out.extend_from_slice(&3_u32.to_le_bytes());
        out.extend_from_slice(&record_count.to_le_bytes());
        out.extend_from_slice(&records);
        summary.records = out;
    }
    summary
}

/// One operation picked by `rng`: mostly recorded, with rare raw writes and parameter changes.
fn one_op(v: &Views, rng: &mut Rng, next_id: &mut u32) {
    let len = v.rows.with(Vec::len);
    let mut row = |rng: &mut Rng| {
        let id = *next_id;
        *next_id += 1;
        Row {
            id,
            title: TITLES[rng.below(TITLES.len())].into(),
            done: rng.below(4) == 0,
            rank: u32::try_from(rng.below(8)).unwrap(),
        }
    };
    // Keep the list between a handful and a few hundred rows.
    let grow = if len < 20 {
        6
    } else if len > 250 {
        1
    } else {
        3
    };
    let pick = rng.below(1000);
    match pick {
        0..=1 => v.rows.clear(),
        2..=4 => {
            // A raw write: every view rebuilds and is sent whole.
            let n = 10 + rng.below(60);
            let fresh = (0..n).map(|_| row(rng)).collect();
            v.rows.replace(fresh);
        }
        5..=7 if len > 0 => {
            let at = rng.below(len);
            v.rows.update(|l| l[at].done = !l[at].done);
        }
        8..=12 => v.cut.set(u32::try_from(rng.below(8)).unwrap()),
        _ => {
            let kind = rng.below(10 + grow);
            if len == 0 || kind >= 10 {
                let at = rng.below(len + 1);
                if rng.below(2) == 0 {
                    v.rows.insert(at, row(rng));
                } else {
                    v.rows.push(row(rng));
                }
                return;
            }
            match kind {
                0..=2 => {
                    v.rows.remove(rng.below(len));
                }
                3..=7 => {
                    let at = rng.below(len);
                    let change = rng.below(5);
                    let title = TITLES[rng.below(TITLES.len())];
                    let rank = u32::try_from(rng.below(8)).unwrap();
                    v.rows.update_at(at, |r| match change {
                        0 | 1 => r.done = !r.done,
                        2 => r.rank = rank,
                        3 => r.title = title.into(),
                        _ => {
                            r.done = !r.done;
                            r.rank = rank;
                            r.title = title.into();
                        }
                    });
                }
                _ => {
                    let (from, to) = (rng.below(len), rng.below(len));
                    v.rows.move_item(from, to);
                }
            }
        }
    }
}
