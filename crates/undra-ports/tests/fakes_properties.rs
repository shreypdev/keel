//! Property tests of the fakes against simple models: a fake that misbehaves would make every
//! test built on it lie.

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::ready;
use undra_ports::fakes::{FakeClock, FakeHttp, MemFs, MemKv, SeededRng};
use undra_ports::{Clock, Fs, FsError, Http, HttpError, HttpRequest, HttpResponse, Kv, Rng, Timer};
use undra_wire::Bytes;
use proptest::prelude::*;

// ---- MemKv against a BTreeMap ------------------------------------------------------------------

#[derive(Clone, Debug)]
enum KvOp {
    Set(String, Vec<u8>),
    Delete(String),
    Get(String),
    List(String),
}

/// Short keys over a tiny alphabet, so operations collide and prefixes overlap.
fn key() -> impl Strategy<Value = String> {
    "[ab/]{0,3}"
}

fn kv_op() -> impl Strategy<Value = KvOp> {
    prop_oneof![
        (key(), prop::collection::vec(any::<u8>(), 0..4)).prop_map(|(k, v)| KvOp::Set(k, v)),
        key().prop_map(KvOp::Delete),
        key().prop_map(KvOp::Get),
        key().prop_map(KvOp::List),
    ]
}

proptest! {
    #[test]
    fn mem_kv_behaves_like_an_ordered_map(ops in prop::collection::vec(kv_op(), 0..60)) {
        let kv = MemKv::new();
        let mut model: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for op in ops {
            match op {
                KvOp::Set(k, v) => {
                    ready(kv.set(k.clone(), Bytes(v.clone())));
                    model.insert(k, v);
                }
                KvOp::Delete(k) => {
                    ready(kv.delete(k.clone()));
                    model.remove(&k);
                }
                KvOp::Get(k) => {
                    prop_assert_eq!(ready(kv.get(k.clone())).map(|b| b.0), model.get(&k).cloned());
                }
                KvOp::List(prefix) => {
                    let expected: Vec<String> =
                        model.keys().filter(|k| k.starts_with(&prefix)).cloned().collect();
                    prop_assert_eq!(ready(kv.list(prefix)), expected);
                }
            }
            prop_assert_eq!(kv.entries(), model.clone());
        }
    }
}

// ---- MemFs invariants --------------------------------------------------------------------------

/// Paths built from segments that include the awkward ones: empty, `.`, `..`.
fn path() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            4 => Just("a".to_owned()),
            4 => Just("b".to_owned()),
            1 => Just(String::new()),
            1 => Just(".".to_owned()),
            1 => Just("..".to_owned()),
        ],
        0..4,
    )
    .prop_map(|segments| segments.join("/"))
}

#[derive(Clone, Debug)]
enum FsOp {
    Write(String, Vec<u8>),
    Read(String),
    Delete(String),
    List(String),
}

fn fs_op() -> impl Strategy<Value = FsOp> {
    prop_oneof![
        (path(), prop::collection::vec(any::<u8>(), 0..4)).prop_map(|(p, d)| FsOp::Write(p, d)),
        path().prop_map(FsOp::Read),
        path().prop_map(FsOp::Delete),
        path().prop_map(FsOp::List),
    ]
}

proptest! {
    /// Whatever the operations, `MemFs` answers with a typed result (never panics), a path with
    /// `..` is always denied, and a successful write is readable back until it is deleted.
    #[test]
    fn mem_fs_never_panics_and_keeps_its_promises(ops in prop::collection::vec(fs_op(), 0..40)) {
        let fs = MemFs::new();
        let has_dot_dot = |p: &str| p.split('/').any(|s| s == "..");
        for op in ops {
            match op {
                FsOp::Write(p, data) => {
                    let result = ready(fs.write(p.clone(), Bytes(data.clone())));
                    if has_dot_dot(&p) {
                        prop_assert_eq!(result, Err(FsError::Denied));
                    } else if result.is_ok() {
                        prop_assert_eq!(ready(fs.read(p)).map(|b| b.0), Ok(data));
                    }
                }
                FsOp::Read(p) => {
                    let result = ready(fs.read(p.clone()));
                    if has_dot_dot(&p) {
                        prop_assert_eq!(result, Err(FsError::Denied));
                    }
                }
                FsOp::Delete(p) => {
                    let result = ready(fs.delete(p.clone()));
                    if has_dot_dot(&p) {
                        prop_assert_eq!(result, Err(FsError::Denied));
                    } else if result.is_ok() {
                        prop_assert_eq!(ready(fs.read(p)), Err(FsError::NotFound));
                    }
                }
                FsOp::List(p) => {
                    let result = ready(fs.list(p.clone()));
                    if has_dot_dot(&p) {
                        prop_assert_eq!(result, Err(FsError::Denied));
                    } else if let Ok(names) = result {
                        let mut sorted = names.clone();
                        sorted.sort();
                        sorted.dedup();
                        prop_assert_eq!(names, sorted, "names are ascending and unique");
                    }
                }
            }
        }
    }

    /// Every file the fake holds is listed by its parent directory.
    #[test]
    fn mem_fs_listings_contain_every_file(paths in prop::collection::vec("[ab]/[ab]/[ab]{1,2}", 1..10)) {
        let fs = MemFs::new();
        for p in &paths {
            ready(fs.write(p.clone(), Bytes(vec![1]))).unwrap();
        }
        for p in &paths {
            let (dir, name) = p.rsplit_once('/').unwrap();
            let listed = ready(fs.list(dir.to_owned())).unwrap();
            prop_assert!(listed.iter().any(|n| n == name), "{p} missing from {dir}: {listed:?}");
        }
    }
}

// ---- SeededRng ---------------------------------------------------------------------------------

proptest! {
    #[test]
    fn rng_is_deterministic_and_exact_in_length(seed in any::<u64>(), len in 0u32..300) {
        let a = SeededRng::new(seed);
        let b = SeededRng::new(seed);
        let bytes = a.fill(len);
        prop_assert_eq!(bytes.len(), len as usize);
        prop_assert_eq!(&bytes, &b.fill(len));
    }

    /// Word-sized fills concatenate: `fill(8 * a)` then `fill(8 * b)` is `fill(8 * (a + b))`.
    #[test]
    fn rng_word_fills_concatenate(seed in any::<u64>(), a in 0u32..20, b in 0u32..20) {
        let split = SeededRng::new(seed);
        let mut joined = split.fill(8 * a).0;
        joined.extend(split.fill(8 * b).0);
        prop_assert_eq!(joined, SeededRng::new(seed).fill(8 * (a + b)).0);
    }

    #[test]
    fn rng_words_are_not_stuck(seed in any::<u64>()) {
        let rng = SeededRng::new(seed);
        let words: Vec<u64> = (0..16).map(|_| rng.next_u64()).collect();
        let mut unique = words.clone();
        unique.sort_unstable();
        unique.dedup();
        prop_assert!(unique.len() > 1, "a xorshift sequence must move: {words:x?}");
    }
}

// ---- FakeClock against a timer model -----------------------------------------------------------

proptest! {
    /// Timers armed at random times fire exactly once, in `(deadline, arming order)` order, at the
    /// first `advance` that reaches their deadline, with the clock reading the deadline.
    #[test]
    fn fake_clock_fires_timers_like_the_model(
        steps in prop::collection::vec((prop::collection::vec(0u64..200, 0..4), 0u64..150), 1..12),
    ) {
        let clock = Arc::new(FakeClock::with_now_ms(0));
        let readings = Arc::new(Mutex::new(Vec::new()));
        {
            let (c, r) = (clock.clone(), readings.clone());
            clock.on_timer_fired(move |id| r.lock().unwrap().push((id, c.monotonic_ns() / 1_000_000)));
        }
        // (deadline_ms, arming order, id) of timers that have not fired.
        let mut model: Vec<(u64, usize, u32)> = Vec::new();
        let mut now_ms = 0u64;
        let mut next_id = 0u32;
        let mut order = 0usize;
        for (delays, advance_ms) in steps {
            for delay in delays {
                Timer::set(&*clock, next_id, delay);
                model.push((now_ms + delay, order, next_id));
                order += 1;
                next_id += 1;
            }
            now_ms += advance_ms;
            model.sort_unstable();
            let due: Vec<(u64, usize, u32)> =
                model.iter().copied().filter(|(deadline, ..)| *deadline <= now_ms).collect();
            model.retain(|(deadline, ..)| *deadline > now_ms);

            readings.lock().unwrap().clear();
            let fired = clock.advance(Duration::from_millis(advance_ms));
            prop_assert_eq!(fired, due.iter().map(|(.., id)| *id).collect::<Vec<_>>());
            let seen = readings.lock().unwrap().clone();
            prop_assert_eq!(seen, due.iter().map(|(deadline, _, id)| (*id, *deadline)).collect::<Vec<_>>());
            prop_assert_eq!(clock.monotonic_ns(), now_ms * 1_000_000);
            prop_assert_eq!(clock.pending_timers(), model.len());
        }
    }
}

// ---- FakeHttp ----------------------------------------------------------------------------------

proptest! {
    /// A sequence answers its first `n` requests in order, then later rules take over.
    #[test]
    fn fake_http_sequences_fall_through(statuses in prop::collection::vec(200u16..600, 0..6), calls in 0usize..10) {
        let http = FakeHttp::new();
        http.respond_sequence(
            "u",
            statuses.iter().map(|s| Ok(HttpResponse::new(*s, Vec::new()))),
        );
        http.fail("u", HttpError::Timeout);
        for i in 0..calls {
            let got = ready(http.request(HttpRequest::get("u")));
            match statuses.get(i) {
                Some(status) => prop_assert_eq!(got.map(|r| r.status), Ok(*status)),
                None => prop_assert_eq!(got, Err(HttpError::Timeout)),
            }
        }
        prop_assert_eq!(http.call_count(), calls);
    }
}
