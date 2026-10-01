//! Every benchmarked operation, described once.
//!
//! The criterion benches (`benches/*.rs`) and the budgets test (`tests/budgets.rs`) both take
//! their operations from here, so a number in `RESULTS.md` and a line in `budgets.toml` always
//! name the same code. Names are `group/operation[/variant]`.
//!
//! Each operation checks, while it is being built, that it does what its name says (a keyed
//! insert really ships a patch, not the whole list; a 100-signal write really makes one
//! change-set of 100 entries; the "1 KB" record really is 1,024 bytes). A benchmark that
//! silently measures the wrong thing is worse than none.
#![allow(missing_docs, dead_code)]

use std::collections::HashMap;
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use undra::meta::ids;
use undra::runtime::testing::{call_payload, drive_from_this_thread};
use undra::runtime::{Runtime, RuntimeConfig};
use undra::signals::{ALL_SIGNALS, ChangeSink, Computed, Signal, StoreCell, txn, with_sink};
use undra::wire::payload::{CallTarget, ChangeSetRef};
use undra::wire::{Bytes, Decode, Encode, Handle, KeyedPatch, Reader, Timestamp, Uuid, Writer};
use undra_bench::workload::{Bench, Workload, plain, with_reset};

use super::fixtures::{self, Item, Shape};
use super::host::{Core, CountingHost, call_ok, construct, method_call, runtime};

/// Every operation the budgets test gates: one per wire type (the round trip), plus dispatch,
/// signals, snapshot and the per-operation rows of the harsh-conditions scenarios (`stress`).
pub fn all() -> Vec<Workload> {
    let mut all = wire();
    all.extend(dispatch());
    all.extend(signals());
    all.extend(snapshot());
    all.extend(super::stress::workloads());
    all.extend(super::ports::ports());
    all.extend(super::ports::db());
    all
}

/// The operations of one group (`wire`, `dispatch`, `signals`, `snapshot`, `stress`, `ports`, `db`).
pub fn group(name: &str) -> Vec<Workload> {
    match name {
        "wire" => wire(),
        "dispatch" => dispatch(),
        "signals" => signals(),
        "snapshot" => snapshot(),
        "stress" => super::stress::workloads(),
        "ports" => super::ports::ports(),
        "db" => super::ports::db(),
        other => panic!("no benchmark group `{other}`"),
    }
}

fn enc<T: Encode>(value: &T) -> Vec<u8> {
    value.encode_to_vec()
}

// ---------------------------------------------------------------------------------------------
// wire
// ---------------------------------------------------------------------------------------------

/// Encode, decode and round trip of each wire type; the budgets test gates the round trips.
pub fn wire() -> Vec<Workload> {
    let mut all = wire_types(false);
    all.extend(keyed_patch());
    all
}

/// Each wire type's encode and decode on their own (criterion only).
pub fn wire_halves() -> Vec<Workload> {
    wire_types(true)
}

fn wire_types(halves: bool) -> Vec<Workload> {
    let mut list = Vec::new();
    let out = &mut list;
    add_wire(out, halves, "bool", || true);
    add_wire(out, halves, "u8", || 200_u8);
    add_wire(out, halves, "u32", || 0x1234_5678_u32);
    add_wire(out, halves, "i64", || -1_234_567_890_123_i64);
    add_wire(out, halves, "f64", || 1234.5678_f64);
    add_wire(out, halves, "string_short", || "hello, undra".to_owned());
    add_wire(out, halves, "string_1kb", || "k".repeat(1024));
    add_wire(out, halves, "bytes_1kb", || {
        Bytes((0..1024_u32).map(|i| (i * 31) as u8).collect())
    });
    add_wire(out, halves, "option_some", || Some(7_u32));
    add_wire(out, halves, "option_none", || None::<u32>);
    add_wire(out, halves, "vec_u32_1k", || {
        (0..1000_u32).collect::<Vec<_>>()
    });
    add_wire(out, halves, "map100_string_u32", || {
        (0..100_u32)
            .map(|i| (format!("key-{i:03}"), i))
            .collect::<HashMap<_, _>>()
    });
    add_wire(out, halves, "map100_u32_u32", || {
        (0..100_u32).map(|i| (i * 7, i)).collect::<HashMap<_, _>>()
    });
    add_wire(out, halves, "duration", || Duration::new(3, 500_000_000));
    add_wire(out, halves, "timestamp", || Timestamp(1_700_000_000_123));
    add_wire(out, halves, "uuid", || {
        Uuid([
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc,
            0xde, 0xf0,
        ])
    });
    add_wire(out, halves, "record5", fixtures::record5);
    add_wire(out, halves, "record1k", || {
        let record = fixtures::record1k();
        assert_eq!(
            record.encode_to_vec().len(),
            1024,
            "the 1 KB record must be 1,024 bytes"
        );
        record
    });
    add_wire(out, halves, "enum_rect", || Shape::Rect { w: 3.0, h: 4.5 });
    add_wire(out, halves, "enum_label", || {
        Shape::Label("a label".to_owned())
    });
    add_wire(out, halves, "result_ok", || Ok::<u32, String>(42));
    add_wire(out, halves, "result_err", || {
        Err::<u32, String>("the server said no".to_owned())
    });
    list
}

/// The keyed patch (`undra-wire`) on its own, for a 10,000-row list that gains one row in the
/// middle: the diff with a cheap key and `PartialEq` (the fallback that a raw `set` / `update`
/// takes, see `signals/keyed_10k/raw_update_diff`; recorded list operations do not run it), the
/// patch's own round trip, and the host-side replay.
fn keyed_patch() -> Vec<Workload> {
    const ROWS: u32 = 10_000;
    fn rows() -> Vec<Item> {
        (0..ROWS)
            .map(|n| Item {
                id: u64::from(n) * 2 + 1,
                title: fixtures::title_of(n, 24),
                done: false,
            })
            .collect()
    }
    fn gained() -> Item {
        Item {
            id: 10_000_000,
            title: fixtures::title_of(ROWS, 24),
            done: false,
        }
    }
    fn patch_of(old: &[Item]) -> KeyedPatch<Item> {
        let mut new = old.to_vec();
        new.insert(old.len() / 2, gained());
        KeyedPatch::diff(old, &new, |i| i.id, |a, b| a == b).expect("a patch, not the full list")
    }
    vec![
        Workload::new("wire/keyed_patch_10k/diff", || {
            let old = rows();
            let mut new = old.clone();
            new.insert(old.len() / 2, gained());
            plain(move || {
                black_box(KeyedPatch::diff(
                    black_box(&old),
                    black_box(&new),
                    |i| i.id,
                    |a, b| a == b,
                ));
            })
        }),
        Workload::new("wire/keyed_patch_10k/roundtrip", || {
            let patch = patch_of(&rows());
            assert_eq!(patch.ops.len(), 1, "one insert is one op");
            plain(move || {
                let mut w = Writer::new();
                black_box(&patch).encode(&mut w);
                let bytes = w.into_vec();
                let mut r = Reader::new(black_box(&bytes));
                black_box(KeyedPatch::<Item>::decode(&mut r)).ok();
            })
        }),
        Workload::new("wire/keyed_patch_10k/apply", || {
            let old = rows();
            let patch = patch_of(&old);
            let middle = old.len() / 2;
            let list = std::rc::Rc::new(std::cell::RefCell::new(old));
            let undo = list.clone();
            with_reset(
                move || {
                    patch
                        .apply(&mut list.borrow_mut())
                        .expect("the patch applies");
                },
                move || {
                    undo.borrow_mut().remove(middle);
                },
            )
        }),
    ]
}

/// Registers `wire/<name>/roundtrip`, and with `halves` also `/encode` and `/decode`.
///
/// * `encode` writes into a reused buffer: the codec's own cost, no allocation.
/// * `decode` reads from a fixed byte string into an owned value.
/// * `roundtrip` is what a call argument or a return value pays: `encode_to_vec` (one buffer
///   allocation) and `decode_exact`.
fn add_wire<T>(out: &mut Vec<Workload>, halves: bool, name: &str, make: fn() -> T)
where
    T: Encode + Decode + 'static,
{
    // Prove once, at registration, that the value survives its own encoding.
    {
        let value = make();
        let bytes = value.encode_to_vec();
        let back = T::decode_exact(&bytes).expect("a fixture decodes");
        assert_eq!(
            back.encode_to_vec(),
            bytes,
            "wire/{name} does not round trip"
        );
    }
    if halves {
        out.push(Workload::new(format!("wire/{name}/encode"), move || {
            let value = make();
            let mut w = Writer::with_capacity(4096);
            plain(move || {
                w.clear();
                value.encode(&mut w);
                black_box(w.as_slice());
            })
        }));
        out.push(Workload::new(format!("wire/{name}/decode"), move || {
            let bytes = make().encode_to_vec();
            plain(move || {
                let mut r = Reader::new(black_box(&bytes));
                black_box(T::decode(&mut r)).ok();
            })
        }));
    } else {
        out.push(Workload::new(format!("wire/{name}/roundtrip"), move || {
            let value = make();
            plain(move || {
                let bytes = black_box(&value).encode_to_vec();
                black_box(T::decode_exact(black_box(&bytes))).ok();
            })
        }));
    }
}

// ---------------------------------------------------------------------------------------------
// dispatch
// ---------------------------------------------------------------------------------------------

/// `undra_call_sync`'s path without the C ABI: payload decode, handle lookup, the guarded
/// generated dispatcher, reply encode.
pub fn dispatch() -> Vec<Workload> {
    vec![
        Workload::new("dispatch/call_sync/add", || {
            let (rt, _host) = runtime();
            let calc = construct(&rt, "Calculator", &enc(&7_i64));
            let args = [enc(&1_i64), enc(&2_i64)].concat();
            let payload = method_call(calc, "Calculator", "add", 2, &args);
            call_ok(&rt, &payload);
            plain(move || {
                black_box(rt.call_sync(black_box(&payload)));
            })
        }),
        Workload::new("dispatch/call_sync_with/add", || {
            let (rt, _host) = runtime();
            let calc = construct(&rt, "Calculator", &enc(&7_i64));
            let args = [enc(&1_i64), enc(&2_i64)].concat();
            let payload = method_call(calc, "Calculator", "add", 2, &args);
            call_ok(&rt, &payload);
            plain(move || {
                black_box(rt.call_sync_with(black_box(&payload), |reply| black_box(reply.len())));
            })
        }),
        Workload::new("dispatch/call_sync/function", || {
            let (rt, _host) = runtime();
            let payload = call_payload(
                CallTarget::Function {
                    method_id: ids::function_id("add_one"),
                },
                2,
                &enc(&41_u32),
            );
            call_ok(&rt, &payload);
            plain(move || {
                black_box(rt.call_sync(black_box(&payload)));
            })
        }),
        Workload::new("dispatch/call_sync/echo_record1k", || {
            let (rt, _host) = runtime();
            let calc = construct(&rt, "Calculator", &enc(&7_i64));
            let payload = method_call(calc, "Calculator", "echo", 2, &enc(&fixtures::record1k()));
            let reply = call_ok(&rt, &payload);
            assert!(reply.len() > 1024, "the record comes back");
            plain(move || {
                black_box(rt.call_sync(black_box(&payload)));
            })
        }),
        Workload::new("dispatch/call_async/ready_add", || {
            let (rt, host) = runtime();
            let calc = construct(&rt, "Calculator", &enc(&7_i64));
            let args = [enc(&1_i64), enc(&2_i64)].concat();
            let next = AtomicU64::new(10);
            let call = move |rt: &Runtime| {
                let id = (next.fetch_add(1, Ordering::Relaxed) % 4_000_000_000) as u32 + 10;
                let payload = method_call(calc, "Calculator", "ready_add", id, &args);
                let status = rt.call(&payload);
                rt.run_pending();
                status
            };
            let before = host.replies();
            assert_eq!(call(&rt), 0, "the call is accepted");
            assert_eq!(host.replies(), before + 1, "and answered by the executor");
            plain(move || {
                black_box(call(&rt));
            })
        }),
    ]
}

// ---------------------------------------------------------------------------------------------
// signals
// ---------------------------------------------------------------------------------------------

/// A sink that counts what it is given.
#[derive(Default)]
struct CountingSink {
    change_sets: AtomicU64,
    bytes: AtomicU64,
}

impl ChangeSink for CountingSink {
    fn deliver(&self, change_set: &[u8]) {
        self.change_sets.fetch_add(1, Ordering::Relaxed);
        self.bytes
            .fetch_add(change_set.len() as u64, Ordering::Relaxed);
    }
}

/// Change-sets, keyed patches, computeds and first observation.
pub fn signals() -> Vec<Workload> {
    vec![
        Workload::new("signals/changeset_100/cell", changeset_100_cell),
        Workload::new("signals/changeset_100/runtime", changeset_100_runtime),
        Workload::new("signals/changeset_100/decode", changeset_100_decode),
        Workload::new("signals/observe_100_initial", observe_100_initial),
        Workload::new("signals/keyed_10k/insert", || {
            keyed(10_000, KeyedOp::Insert)
        }),
        Workload::new("signals/keyed_10k/update", || {
            keyed(10_000, KeyedOp::Update)
        }),
        Workload::new("signals/keyed_10k/move", || keyed(10_000, KeyedOp::Move)),
        // The fallback: the same one-row edit written with the raw `update`, which the commit
        // finds by diffing the list against what the host has (O(list)).
        Workload::new("signals/keyed_10k/raw_update_diff", || {
            keyed(10_000, KeyedOp::RawUpdate)
        }),
        // The same insert on shorter lists: how the cost scales with the list, not the change.
        Workload::new("signals/keyed_1k/insert", || keyed(1_000, KeyedOp::Insert)),
        Workload::new("signals/keyed_100/insert", || keyed(100, KeyedOp::Insert)),
        // One write and its commit, nobody observing: pins the cost of the write check that
        // every build runs since ADR-035.
        Workload::new("signals/set_attached", set_attached),
        Workload::new("signals/computed/recompute_1", computed_recompute_1),
        Workload::new(
            "signals/computed/recompute_chain_10",
            computed_recompute_chain_10,
        ),
    ]
}

/// One write to a signal of a published store (owner recorded, handle set) and its implicit
/// commit, with nothing observed: the write check of every build (ADR-035: the checker the
/// runtime installed, three thread-local reads on a driver thread), the claim and the
/// nothing-to-send path.
fn set_attached() -> Box<dyn Bench> {
    // A runtime installs the process's write checker; this thread plays its core's driver.
    let (rt, _host) = runtime();
    drive_from_this_thread();
    let cell = StoreCell::new(0xBE_C0_03);
    let value = Signal::new(0_u32);
    cell.attach(&value, 0).expect("attach");
    cell.set_owner(rt.id());
    cell.set_handle(Handle::new(1, 1).0);
    assert!(value.can_write(), "a driver thread may write");
    value.set(1);
    assert_eq!(value.get(), 1);
    plain(move || {
        let _keep = &rt;
        value.update(|n| *n = n.wrapping_add(1));
        black_box(&value);
    })
}

/// 100 signals of one store written in one transaction: build the change-set and hand it to the
/// sink. No runtime, no dispatch: the signals crate on its own.
fn changeset_100_cell() -> Box<dyn Bench> {
    drive_from_this_thread();
    let cell = StoreCell::new(0xBE_C0_01);
    let signals: Vec<Signal<u32>> = (0..100).map(|_| Signal::new(0)).collect();
    for (id, signal) in signals.iter().enumerate() {
        cell.attach(signal, id as u32).expect("attach");
    }
    cell.set_handle(Handle::new(1, 1).0);
    cell.observe(ALL_SIGNALS, true, &mut Writer::new());
    let sink = Arc::new(CountingSink::default());
    let bump = move || {
        with_sink(sink.clone(), || {
            txn(|| {
                for signal in &signals {
                    signal.update(|n| *n = n.wrapping_add(1));
                }
            });
        });
        sink.change_sets.load(Ordering::Relaxed)
    };
    assert_eq!(bump(), 1, "one transaction, one change-set");
    plain(move || {
        black_box(bump());
    })
}

/// The same 100 writes as one method call on a store, through the runtime: dispatch, the writes,
/// the change-set, and the host callback.
fn changeset_100_runtime() -> Box<dyn Bench> {
    let (rt, host) = runtime();
    let store = construct(&rt, "Wide100", &[]);
    rt.observe(store.0, ALL_SIGNALS, true);
    let payload = method_call(store, "Wide100", "bump_all", 2, &[]);
    let (sets, bytes) = (host.change_sets(), host.change_set_bytes());
    call_ok(&rt, &payload);
    assert_eq!(
        host.change_sets(),
        sets + 1,
        "one transaction, one change-set"
    );
    assert!(
        host.change_set_bytes() - bytes >= 100 * 4,
        "the change-set carries 100 values"
    );
    plain(move || {
        black_box(rt.call_sync(black_box(&payload)));
    })
}

/// What a platform runtime does with that change-set before it touches its own state: validate
/// it and walk its entries (borrowed, no allocation).
fn changeset_100_decode() -> Box<dyn Bench> {
    let (rt, host) = runtime();
    let store = construct(&rt, "Wide100", &[]);
    rt.observe(store.0, ALL_SIGNALS, true);
    let before = host.change_set_bytes();
    call_ok(&rt, &method_call(store, "Wide100", "bump_all", 2, &[]));
    let len = (host.change_set_bytes() - before) as usize;
    // Rebuild the same payload shape with the cell (the runtime hands out no copy): 100 entries.
    let bytes = {
        let cell_sink = Arc::new(CaptureOne::default());
        drive_from_this_thread();
        let cell = StoreCell::new(0xBE_C0_02);
        let signals: Vec<Signal<u32>> = (0..100).map(|_| Signal::new(0)).collect();
        for (id, signal) in signals.iter().enumerate() {
            cell.attach(signal, id as u32).expect("attach");
        }
        cell.set_handle(Handle::new(1, 1).0);
        cell.observe(ALL_SIGNALS, true, &mut Writer::new());
        with_sink(cell_sink.clone(), || {
            txn(|| {
                for signal in &signals {
                    signal.set(9);
                }
            });
        });
        cell_sink.take()
    };
    assert_eq!(bytes.len(), len, "same shape as the runtime's change-set");
    plain(move || {
        let mut r = Reader::new(black_box(&bytes));
        let set = ChangeSetRef::decode(&mut r).expect("a change-set");
        let mut total = 0_usize;
        for entry in &set {
            total += entry.value.len() + entry.signal_id as usize;
        }
        black_box(total);
    })
}

/// Keeps the last payload it was given.
#[derive(Default)]
struct CaptureOne(std::sync::Mutex<Vec<u8>>);

impl CaptureOne {
    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.0.lock().expect("not poisoned"))
    }
}

impl ChangeSink for CaptureOne {
    fn deliver(&self, change_set: &[u8]) {
        *self.0.lock().expect("not poisoned") = change_set.to_vec();
    }
}

/// A host starting to observe a 100-signal store: every current value comes back at once.
fn observe_100_initial() -> Box<dyn Bench> {
    let (rt, host) = runtime();
    let store = construct(&rt, "Wide100", &[]);
    let rt2 = rt.clone();
    let before = host.change_sets();
    rt.observe(store.0, ALL_SIGNALS, true);
    assert_eq!(
        host.change_sets(),
        before + 1,
        "the initial emission is one change-set"
    );
    rt.observe(store.0, ALL_SIGNALS, false);
    with_reset(
        move || rt.observe(store.0, ALL_SIGNALS, true),
        move || rt2.observe(store.0, ALL_SIGNALS, false),
    )
}

#[derive(Clone, Copy)]
enum KeyedOp {
    Insert,
    Update,
    Move,
    /// An update through the raw `Signal::update`: the diff path.
    RawUpdate,
}

/// A keyed list of `rows` rows, observed, changed by one insert, one update or one move written
/// with the recorded list operations (what generated store code does, ADR-027), or by one update
/// through the raw `Signal::update`, which takes the diff path. Through the runtime: dispatch,
/// argument decode, the write, the patch, the host callback.
fn keyed(rows: u32, op: KeyedOp) -> Box<dyn Bench> {
    let (rt, host) = runtime();
    let (middle, low, high) = (rows / 2, rows / 10, rows / 10 * 9);
    let feed = construct(&rt, "Feed", &[]);
    call_ok(
        &rt,
        &method_call(
            feed,
            "Feed",
            "seed",
            2,
            &[enc(&rows), enc(&24_u32)].concat(),
        ),
    );
    rt.observe(feed.0, ALL_SIGNALS, true);
    let at = |method: &str, id: u32, args: Vec<u8>| method_call(feed, "Feed", method, id, &args);

    // The two halves of every scenario, prebuilt: the timed call, and what undoes or alternates it.
    let (first, second, resets) = match op {
        KeyedOp::Insert => {
            let row = Item {
                id: 10_000_000, // even: the seeded ids are odd
                title: fixtures::title_of(rows, 24),
                done: false,
            };
            (
                at("insert_at", 3, [enc(&middle), enc(&row)].concat()),
                at("remove_at", 4, enc(&middle)),
                true,
            )
        }
        KeyedOp::Update => (
            at(
                "rename",
                3,
                [enc(&middle), enc(&"first title".to_owned())].concat(),
            ),
            at(
                "rename",
                4,
                [enc(&middle), enc(&"second title".to_owned())].concat(),
            ),
            false,
        ),
        KeyedOp::Move => (
            at("move_item", 3, [enc(&low), enc(&high)].concat()),
            at("move_item", 4, [enc(&high), enc(&low)].concat()),
            false,
        ),
        KeyedOp::RawUpdate => (
            at(
                "rename_raw",
                3,
                [enc(&middle), enc(&"first title".to_owned())].concat(),
            ),
            at(
                "rename_raw",
                4,
                [enc(&middle), enc(&"second title".to_owned())].concat(),
            ),
            false,
        ),
    };

    // Prove that what ships is a patch of a few dozen bytes, not the list.
    let before = host.change_set_bytes();
    call_ok(&rt, &first);
    let shipped = host.change_set_bytes() - before;
    assert!(
        (1..2_000).contains(&shipped),
        "a keyed change must ship a patch; {shipped} bytes shipped"
    );
    call_ok(&rt, &second);

    if resets {
        let rt2 = rt.clone();
        let (first, second) = (first.clone(), second.clone());
        with_reset(
            move || {
                black_box(rt.call_sync(black_box(&first)));
            },
            move || {
                black_box(rt2.call_sync(&second));
            },
        )
    } else {
        // Alternate between the two calls: every iteration is one real change of the same size.
        let mut flip = false;
        plain(move || {
            flip = !flip;
            let payload = if flip { &first } else { &second };
            black_box(rt.call_sync(black_box(payload)));
        })
    }
}

/// One signal write and the read that recomputes a computed derived from it.
fn computed_recompute_1() -> Box<dyn Bench> {
    drive_from_this_thread();
    let source = Signal::new(0_u32);
    let derived = Computed::new(&source, |n| n.wrapping_mul(3));
    let mut next = 0_u32;
    assert_eq!(derived.get(), 0);
    plain(move || {
        next = next.wrapping_add(1);
        source.set(next);
        black_box(derived.get());
    })
}

/// The same through a chain of ten computeds: one write dirties ten nodes, one read recomputes
/// all ten.
fn computed_recompute_chain_10() -> Box<dyn Bench> {
    drive_from_this_thread();
    let source = Signal::new(0_u32);
    let mut chain = vec![Computed::new(&source, |n| n.wrapping_add(1))];
    for _ in 1..10 {
        let previous = chain.last().expect("non-empty").clone();
        chain.push(Computed::new(&previous, |n| n.wrapping_add(1)));
    }
    let last = chain.last().expect("non-empty").clone();
    assert_eq!(last.get(), 10);
    let mut next = 0_u32;
    plain(move || {
        next = next.wrapping_add(1);
        source.set(next);
        black_box(last.get());
        black_box(&chain);
    })
}

// ---------------------------------------------------------------------------------------------
// snapshot
// ---------------------------------------------------------------------------------------------

/// `stores` stores holding 250 rows of 100 bytes each: four of them are the blueprint's 100 KB
/// cold-start scenario, forty its 1 MB crash-recovery one.
fn snapshot_fixture(stores: u32) -> (Arc<Core>, Arc<CountingHost>, Vec<u8>) {
    let (rt, host) = runtime();
    for _ in 0..stores {
        let feed = construct(&rt, "Feed", &[]);
        call_ok(
            &rt,
            &method_call(
                feed,
                "Feed",
                "seed",
                2,
                &[enc(&250_u32), enc(&87_u32)].concat(),
            ),
        );
    }
    let snapshot = rt.snapshot();
    let expected = stores as usize * 25_000;
    assert!(
        (expected..=expected + expected / 100 + 64).contains(&snapshot.len()),
        "{stores} stores should snapshot to about {expected} bytes, not {}",
        snapshot.len()
    );
    (rt, host, snapshot)
}

/// Snapshot, restore, and a whole cold start.
pub fn snapshot() -> Vec<Workload> {
    vec![
        Workload::new("snapshot/encode_100kb", || {
            let (rt, _host, _snapshot) = snapshot_fixture(4);
            plain(move || {
                black_box(rt.snapshot());
            })
        }),
        Workload::new("snapshot/restore_100kb", || {
            let (rt, _host, snapshot) = snapshot_fixture(4);
            rt.restore(&snapshot).expect("the snapshot restores");
            plain(move || {
                rt.restore(black_box(&snapshot)).expect("restore");
            })
        }),
        // The blueprint's web crash-recovery row: a 1 MB state restored into a live runtime.
        Workload::new("snapshot/restore_1mb", || {
            let (rt, _host, snapshot) = snapshot_fixture(40);
            rt.restore(&snapshot).expect("the snapshot restores");
            plain(move || {
                rt.restore(black_box(&snapshot)).expect("restore");
            })
        }),
        Workload::new("snapshot/cold_start_restore_100kb", || cold_start(0)),
        Workload::new("snapshot/cold_start_restore_100kb_core_thread", || {
            cold_start(1)
        }),
    ]
}

/// A new runtime that restores the 100 KB snapshot: what launching the app pays in the core
/// before the first screen. `core_threads == 1` also starts (and, in the untimed reset, joins)
/// the `undra-core` thread, as a platform does; 0 is the wasm shape.
fn cold_start(core_threads: u8) -> Box<dyn Bench> {
    let (_rt, _host, snapshot) = snapshot_fixture(4);
    let slot: std::rc::Rc<std::cell::RefCell<Option<Core>>> = Default::default();
    let held = slot.clone();
    let run = move || {
        let host = Arc::new(CountingHost::default());
        let config = RuntimeConfig {
            platform: "bench".to_owned(),
            core_threads,
            ..RuntimeConfig::default()
        };
        let rt = Core::new(config, host);
        rt.restore(black_box(&snapshot))
            .expect("the snapshot restores");
        *slot.borrow_mut() = Some(rt);
    };
    // The previous runtime is shut down and dropped outside the timed region: an app does not
    // time its own exit.
    with_reset(run, move || drop(held.borrow_mut().take()))
}
