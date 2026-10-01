//! Benchmark hooks: the state and methods the blueprint section 14 budget rows are measured with.
//!
//! One store holds everything the rows need, so a harness builds one object, observes it and
//! calls the `bench_*` methods:
//!
//! | Budget row | What to call |
//! |---|---|
//! | Handle method call, primitive arguments and return | [`Bench::bench_add`] |
//! | 1 KB record, round trip | [`Bench::bench_echo_bytes`] with a 1,024-byte payload |
//! | Change-set with 100 dirty signals | [`Bench::bench_touch_signals`] with `k = 100` |
//! | Keyed patch on a 10,000-item list, one insert | [`Bench::bench_list_insert`] |
//! | ADR-031 drain: 1,667 one-operation keyed patches in one frame | [`Bench::bench_list_update_burst`] |
//!
//! The store has [`SIGNALS`] counters (`s000` .. `s127`) and a keyed list of [`ROWS`] rows. The
//! counters are written by position, so the signal table is generated from one list of names.

use std::sync::atomic::{AtomicU32, Ordering};

use undra::prelude::*;

use crate::biglist::Item;

/// How many counters the bench store has (`s000` .. `s127`).
pub const SIGNALS: u32 = 128;

/// How many rows the bench list starts with.
pub const ROWS: u32 = 10_000;

/// Expands to `0` for every counter name, so the constructor can repeat it once per counter.
macro_rules! zero {
    ($counter:ident) => {
        0
    };
}

/// Declares the `Bench` store with one `Signal<u32>` per name; the list is spelled out once.
macro_rules! bench_store {
    ($($counter:ident)*) => {
        /// The benchmark store: 128 counters, a 10,000-row keyed list and three methods that
        /// exercise the boundary.
        #[undra::store(restore = "Self::assemble")]
        pub struct Bench {
            ctx: Ctx,
            next_id: AtomicU32,
            #[undra(key = "id")]
            rows: Signal<Vec<Item>>,
            $($counter: Signal<u32>,)*
        }

        #[undra::api(store)]
        impl Bench {
            /// A store with every counter at zero and [`ROWS`] rows numbered from 1.
            pub fn new(ctx: Ctx) -> Self {
                Self::assemble(
                    ctx,
                    Signal::new((1..=ROWS).map(Item::numbered).collect()),
                    $(Signal::new(zero!($counter)),)*
                )
            }

            // Used by `new` and, through `restore = ".."`, to rebuild the store from a snapshot:
            // the hook takes one signal per field, so it has as many parameters as the store has
            // signals.
            #[allow(clippy::too_many_arguments)]
            fn assemble(ctx: Ctx, rows: Signal<Vec<Item>>, $($counter: Signal<u32>,)*) -> Self {
                let next = rows.with(|list| list.iter().map(|item| item.id).max().unwrap_or(0));
                Self {
                    ctx,
                    next_id: AtomicU32::new(next.saturating_add(1)),
                    rows,
                    $($counter,)*
                }
            }

            /// Adds two numbers: the cheapest call there is, for the handle-call row.
            pub fn bench_add(&self, a: u32, b: u32) -> u32 {
                a.wrapping_add(b)
            }

            /// Returns `data` unchanged: a payload of `data.len()` bytes crosses the boundary
            /// twice.
            pub fn bench_echo_bytes(&self, data: Bytes) -> Bytes {
                data
            }

            /// Adds one to the first `k` counters (at most [`SIGNALS`]) inside one transaction:
            /// `k` dirty signals, one change-set.
            pub fn bench_touch_signals(&self, k: u32) {
                let counters: [&Signal<u32>; SIGNALS as usize] = [$(&self.$counter,)*];
                self.ctx.txn(|| {
                    for counter in counters.iter().take(k as usize) {
                        counter.update(|n| *n = n.wrapping_add(1));
                    }
                });
            }

            /// Inserts one new row so that it ends at position `i` (a position past the end
            /// appends): one keyed `Insert` on a list of about 10,000 rows. The list grows by
            /// one row per call; `bench_list_reset` starts over.
            pub fn bench_list_insert(&self, i: u32) {
                let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                // The recorded insert keeps the change-set O(change) (ADR-027): this hook is
                // what the device benchmarks time.
                let at = (i as usize).min(self.rows.with(Vec::len));
                self.rows.insert(at, Item::numbered(id));
            }

            /// Updates `n` rows of the list, **one transaction each**: `n` change-sets, each a
            /// keyed patch of a single `Update` (the row's `version` goes up by one), the way a
            /// socket or a sensor feed that writes a row at a time reaches the platform. The rows
            /// are spread over the list (consecutive updates are far apart) and the same
            /// positions come out for the same list length, so a run is repeatable. This is what
            /// the device benchmarks drain (ADR-031: 100,000 patches a second is 1,667 a frame).
            pub fn bench_list_update_burst(&self, n: u32) {
                let len = self.rows.with(Vec::len);
                if len == 0 {
                    return;
                }
                for step in 0..n as usize {
                    // 7,919 is prime, so the positions visit every row of a list whose length it
                    // does not divide before any repeats.
                    let at = step.wrapping_mul(7_919) % len;
                    self.rows
                        .update_at(at, |item| item.version = item.version.wrapping_add(1));
                }
            }

            /// Puts the list back to its [`ROWS`] starting rows and writes zero to every counter,
            /// in one transaction: one change-set with the list and all [`SIGNALS`] counters.
            pub fn bench_list_reset(&self) {
                let counters: [&Signal<u32>; SIGNALS as usize] = [$(&self.$counter,)*];
                self.ctx.txn(|| {
                    self.next_id.store(ROWS + 1, Ordering::Relaxed);
                    self.rows.set((1..=ROWS).map(Item::numbered).collect());
                    for counter in counters {
                        counter.set(0);
                    }
                });
            }
        }
    };
}

bench_store! {
    s000 s001 s002 s003 s004 s005 s006 s007
    s008 s009 s010 s011 s012 s013 s014 s015
    s016 s017 s018 s019 s020 s021 s022 s023
    s024 s025 s026 s027 s028 s029 s030 s031
    s032 s033 s034 s035 s036 s037 s038 s039
    s040 s041 s042 s043 s044 s045 s046 s047
    s048 s049 s050 s051 s052 s053 s054 s055
    s056 s057 s058 s059 s060 s061 s062 s063
    s064 s065 s066 s067 s068 s069 s070 s071
    s072 s073 s074 s075 s076 s077 s078 s079
    s080 s081 s082 s083 s084 s085 s086 s087
    s088 s089 s090 s091 s092 s093 s094 s095
    s096 s097 s098 s099 s100 s101 s102 s103
    s104 s105 s106 s107 s108 s109 s110 s111
    s112 s113 s114 s115 s116 s117 s118 s119
    s120 s121 s122 s123 s124 s125 s126 s127
}

#[cfg(test)]
mod tests {
    use undra::meta::ids;
    use undra::runtime::testing::TestRuntime;
    use undra::signals::ALL_SIGNALS;
    use undra::wire::payload::{CallTarget, ChangeOp, ChangeSet, ReplyStatus};
    use undra::wire::{Decode, Encode, KeyedPatch, PatchOp, Reader};

    use super::*;

    /// The row list is signal 0; counter `sNNN` is signal `1 + NNN`.
    const ROWS_SIGNAL: u32 = 0;

    struct App {
        t: TestRuntime,
        store: Handle,
    }

    impl App {
        /// A bench store that is being observed; the initial change-set was consumed.
        fn new() -> App {
            let t = TestRuntime::new();
            let reply = t.call_sync(
                CallTarget::Constructor {
                    type_id: ids::type_id("Bench"),
                    method_id: ids::method_id("Bench", "new"),
                },
                1,
                &[],
            );
            assert_eq!(reply.status, ReplyStatus::Ok);
            let store = Handle::decode_exact(&reply.body).unwrap();
            t.take_change_sets();
            t.runtime().observe(store.0, ALL_SIGNALS, true);
            let initial = t.host().take_decoded_change_sets();
            assert_eq!(initial.len(), 1);
            assert_eq!(initial[0].entries.len(), 1 + SIGNALS as usize);
            App { t, store }
        }

        fn call(&self, method: &str, args: &[u8]) -> Vec<u8> {
            let reply = self.t.call_sync(
                CallTarget::Method {
                    handle: self.store,
                    method_id: ids::method_id("Bench", method),
                },
                2,
                args,
            );
            assert_eq!(reply.status, ReplyStatus::Ok, "Bench.{method}: {reply:?}");
            reply.body
        }

        fn change_sets(&self) -> Vec<ChangeSet> {
            self.t.host().take_decoded_change_sets()
        }
    }

    #[test]
    fn the_primitive_call_adds() {
        let app = App::new();
        let mut args = 40u32.encode_to_vec();
        args.extend(2u32.encode_to_vec());
        assert_eq!(
            u32::decode_exact(&app.call("bench_add", &args)).unwrap(),
            42
        );
        let mut wrap = u32::MAX.encode_to_vec();
        wrap.extend(2u32.encode_to_vec());
        assert_eq!(u32::decode_exact(&app.call("bench_add", &wrap)).unwrap(), 1);
    }

    #[test]
    fn a_kilobyte_makes_the_round_trip() {
        let app = App::new();
        let payload = Bytes((0..1_024u32).map(|n| (n % 251) as u8).collect());
        let body = app.call("bench_echo_bytes", &payload.encode_to_vec());
        assert_eq!(Bytes::decode_exact(&body).unwrap(), payload);
    }

    #[test]
    fn touching_k_signals_is_one_change_set_of_k_entries() {
        let app = App::new();
        for k in [1u32, 100, SIGNALS] {
            app.call("bench_touch_signals", &k.encode_to_vec());
            let sets = app.change_sets();
            assert_eq!(sets.len(), 1, "k = {k}: one transaction, one change-set");
            assert_eq!(sets[0].entries.len(), k as usize, "k = {k}");
            assert!(sets[0].entries.iter().all(|e| e.op == ChangeOp::Full));
        }
        // Asking for more than there are touches them all and no more.
        app.call("bench_touch_signals", &1_000u32.encode_to_vec());
        assert_eq!(app.change_sets()[0].entries.len(), SIGNALS as usize);
    }

    #[test]
    fn touched_counters_count() {
        let app = App::new();
        app.call("bench_touch_signals", &3u32.encode_to_vec());
        app.call("bench_touch_signals", &3u32.encode_to_vec());
        let sets = app.change_sets();
        let values: Vec<u32> = sets[1]
            .entries
            .iter()
            .map(|e| u32::decode_exact(&e.value).unwrap())
            .collect();
        assert_eq!(values, [2, 2, 2]);
        // The first three counters are signals 1, 2 and 3 (signal 0 is the list).
        let ids: Vec<u32> = sets[1].entries.iter().map(|e| e.signal_id).collect();
        assert_eq!(ids, [1, 2, 3]);
    }

    #[test]
    fn one_insert_is_one_patch_operation() {
        let app = App::new();
        app.call("bench_list_insert", &5_000u32.encode_to_vec());
        let sets = app.change_sets();
        assert_eq!(sets.len(), 1);
        let entry = &sets[0].entries[0];
        assert_eq!(
            (entry.signal_id, entry.op),
            (ROWS_SIGNAL, ChangeOp::KeyedPatch)
        );
        let mut r = Reader::new(&entry.value);
        let patch = KeyedPatch::<Item>::decode(&mut r).unwrap();
        assert_eq!(patch.ops.len(), 1);
        assert!(
            matches!(&patch.ops[0], PatchOp::Insert { index: 5_000, item } if item.id == ROWS + 1)
        );
        assert!(
            entry.value.len() < 64,
            "{} bytes for a 10,000-row list",
            entry.value.len()
        );

        // A position past the end appends.
        app.call("bench_list_insert", &u32::MAX.encode_to_vec());
        let sets = app.change_sets();
        let mut r = Reader::new(&sets[0].entries[0].value);
        let patch = KeyedPatch::<Item>::decode(&mut r).unwrap();
        assert!(matches!(&patch.ops[0], PatchOp::Insert { index, .. } if *index == ROWS + 1));
    }

    #[test]
    fn an_update_burst_is_one_change_set_of_one_update_per_step() {
        let app = App::new();
        app.call("bench_list_update_burst", &0u32.encode_to_vec());
        assert!(app.change_sets().is_empty(), "no steps, no change-sets");

        app.call("bench_list_update_burst", &5u32.encode_to_vec());
        let sets = app.change_sets();
        assert_eq!(sets.len(), 5, "one transaction per update");
        for (step, set) in sets.iter().enumerate() {
            assert_eq!(set.entries.len(), 1);
            let entry = &set.entries[0];
            assert_eq!((entry.signal_id, entry.op), (ROWS_SIGNAL, ChangeOp::KeyedPatch));
            let mut r = Reader::new(&entry.value);
            let patch = KeyedPatch::<Item>::decode(&mut r).unwrap();
            assert_eq!(patch.ops.len(), 1);
            let want = (step as u32 * 7_919) % ROWS;
            assert!(
                matches!(&patch.ops[0], PatchOp::Update { index, item }
                    if *index == want && item.id == want + 1 && item.version == 1),
                "step {step}: {:?}",
                patch.ops
            );
            assert!(entry.value.len() < 64, "{} bytes", entry.value.len());
        }

        // The same positions again: every row was touched once, so the versions are now 2.
        app.call("bench_list_update_burst", &1u32.encode_to_vec());
        let sets = app.change_sets();
        let mut r = Reader::new(&sets[0].entries[0].value);
        let patch = KeyedPatch::<Item>::decode(&mut r).unwrap();
        assert!(matches!(&patch.ops[0], PatchOp::Update { index: 0, item } if item.version == 2));
    }

    #[test]
    fn a_reset_puts_everything_back_in_one_transaction() {
        let app = App::new();
        app.call("bench_touch_signals", &10u32.encode_to_vec());
        app.call("bench_list_insert", &0u32.encode_to_vec());
        app.change_sets();
        app.call("bench_list_reset", &[]);
        let sets = app.change_sets();
        assert_eq!(sets.len(), 1);
        // Every counter was written (a write is a change even when the value stays), and the
        // list is the fresh one again: the patch removes the inserted row.
        assert_eq!(sets[0].entries.len(), 1 + SIGNALS as usize);
        assert!(
            sets[0].entries[1..]
                .iter()
                .all(|e| e.value == 0u32.to_le_bytes())
        );
        let mut r = Reader::new(&sets[0].entries[0].value);
        let patch = KeyedPatch::<Item>::decode(&mut r).unwrap();
        assert_eq!(patch.ops, [PatchOp::Remove { index: 0 }]);
    }

    #[test]
    fn a_restore_rebuilds_all_hundred_and_twenty_nine_signals() {
        let app = App::new();
        app.call("bench_touch_signals", &SIGNALS.encode_to_vec());
        app.call("bench_list_insert", &1u32.encode_to_vec());
        let snapshot = app.t.runtime().snapshot();
        app.call("bench_touch_signals", &SIGNALS.encode_to_vec());
        app.change_sets();
        app.t.runtime().restore(&snapshot).expect("restores");
        let sets = app.change_sets();
        let set = sets.last().expect("the restore re-sends what is observed");
        assert_eq!(set.entries.len(), 1 + SIGNALS as usize);
        assert_eq!(
            u32::decode_exact(&set.entries[1].value).unwrap(),
            1,
            "the counters are as snapshotted"
        );
        let rows = Vec::<Item>::decode_exact(&set.entries[0].value).unwrap();
        assert_eq!(rows.len(), ROWS as usize + 1);
        // Identities continue above the largest one in the snapshot.
        app.call("bench_list_insert", &0u32.encode_to_vec());
        let sets = app.change_sets();
        let mut r = Reader::new(&sets[0].entries[0].value);
        let patch = KeyedPatch::<Item>::decode(&mut r).unwrap();
        assert!(matches!(&patch.ops[0], PatchOp::Insert { item, .. } if item.id == ROWS + 2));
    }
}
