//! Property tests for the message layer: envelope, typed payloads and keyed patches.

mod common;

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;

use common::Rng;
use proptest::collection::vec;
use proptest::prelude::*;
use undra_wire::payload::{
    Call, CallOwned, CallTarget, Cancel, ChangeEntry, ChangeOp, ChangeSet, ChangeSetBuilder,
    ChangeSetRef, Event, Hello, Log, Observe, PortCall, PortReply, PortStatus, Release, Reply,
    ReplyStatus, Snapshot, SnapshotType, StoreSnapshot, StreamCredit, StreamFailure, StreamFlag,
    StreamItem, TimerFired,
};
use undra_wire::{
    Decode, Encode, Envelope, Handle, KeyedPatch, Kind, PatchOp, Reader, WireError, Writer,
};

fn handle() -> impl Strategy<Value = Handle> {
    any::<u64>().prop_map(Handle)
}

fn body() -> impl Strategy<Value = Vec<u8>> {
    vec(any::<u8>(), 0..64)
}

/// Encodes `$value`, decodes it back with `$ty::decode` and checks: equality, that the reader
/// is left at the end, and that every prefix shorter than `$fixed` bytes (the part of the
/// layout that is not a trailing body) is an error.
///
/// A macro rather than a function because payloads that borrow from the input have a
/// lifetime that a closure-based helper cannot express.
macro_rules! check_payload {
    ($value:expr, $fixed:expr, $ty:ident) => {{
        let value = $value;
        let mut w = Writer::new();
        value.encode(&mut w);
        let bytes = w.into_vec();

        let mut r = Reader::new(&bytes);
        let back =
            $ty::decode(&mut r).map_err(|e| TestCaseError::fail(format!("{:?}: {}", value, e)))?;
        prop_assert_eq!(&back, &value);
        prop_assert!(r.finish().is_ok(), "reader not exhausted");

        for cut in 0..($fixed).min(bytes.len()) {
            prop_assert!(
                $ty::decode(&mut Reader::new(&bytes[..cut])).is_err(),
                "prefix of {} bytes decoded",
                cut
            );
        }
        bytes
    }};
}

// ---------------------------------------------------------------------------------------------
// Simple payloads
// ---------------------------------------------------------------------------------------------

fn call_target() -> impl Strategy<Value = CallTarget> {
    prop_oneof![
        any::<u32>().prop_map(|method_id| CallTarget::Function { method_id }),
        (handle(), any::<u32>())
            .prop_map(|(handle, method_id)| CallTarget::Method { handle, method_id }),
        (any::<u32>(), any::<u32>())
            .prop_map(|(type_id, method_id)| CallTarget::Constructor { type_id, method_id }),
        (handle(), any::<u32>(), any::<u32>()).prop_map(|(handle, offset, limit)| {
            CallTarget::LazyPage {
                handle,
                offset,
                limit,
            }
        }),
    ]
}

proptest! {
    #[test]
    fn call(target in call_target(), call_id in any::<u32>(), mut args in body()) {
        // Lazy-page calls have no argument bytes on the wire.
        if matches!(target, CallTarget::LazyPage { .. }) {
            args.clear();
        }
        let call = Call { target, call_id, args: &args };
        let fixed = match target {
            CallTarget::Function { .. } | CallTarget::Method { .. } => 17,
            CallTarget::Constructor { .. } => 13,
            CallTarget::LazyPage { .. } => 21,
        };
        let bytes = check_payload!(call, fixed, Call);

        // The owned variant produces the same bytes and the same value.
        let owned = CallOwned::from(&call);
        prop_assert_eq!(owned.as_call(), call);
        let mut w = Writer::new();
        owned.encode(&mut w);
        prop_assert_eq!(w.as_slice(), &bytes[..]);
        prop_assert_eq!(CallOwned::decode(&mut Reader::new(&bytes)), Ok(owned));
    }

    #[test]
    fn call_targets_are_distinguished_by_the_first_byte(
        target in call_target(), call_id in any::<u32>()
    ) {
        let mut w = Writer::new();
        Call { target, call_id, args: &[] }.encode(&mut w);
        let expected = match target {
            CallTarget::Function { .. } => 0,
            CallTarget::Method { .. } => 1,
            CallTarget::Constructor { .. } => 2,
            CallTarget::LazyPage { .. } => 3,
        };
        prop_assert_eq!(w.as_slice()[0], expected);
    }

    #[test]
    fn reply(call_id in any::<u32>(), status in 0_u8..6, body in body()) {
        let reply = Reply { call_id, status: ReplyStatus::try_from(status).unwrap(), body: &body };
        check_payload!(reply, 5, Reply);
    }

    #[test]
    fn port_call_and_reply(
        port_id in any::<u32>(), method_id in any::<u32>(), port_call_id in any::<u32>(),
        status in 0_u8..3, body in body(),
    ) {
        let call = PortCall { port_id, method_id, port_call_id, args: &body };
        check_payload!(call, 12, PortCall);
        let reply = PortReply {
            port_call_id,
            status: PortStatus::try_from(status).unwrap(),
            body: &body,
        };
        check_payload!(reply, 5, PortReply);
    }

    #[test]
    fn fixed_size_payloads(
        call_id in any::<u32>(), credit in any::<u32>(), signal_id in any::<u32>(),
        on in any::<bool>(), h in handle(), flag in 0_u8..4, body in body(),
        timer_id in any::<u32>(), port_id in any::<u32>(), method_id in any::<u32>(),
    ) {
        check_payload!(Cancel { call_id }, 4, Cancel);
        check_payload!(StreamCredit { call_id, credit }, 8, StreamCredit);
        check_payload!(Observe { handle: h, signal_id, on }, 13, Observe);
        check_payload!(Release { handle: h }, 8, Release);
        check_payload!(TimerFired { timer_id }, 4, TimerFired);
        check_payload!(
            StreamItem { call_id, flag: StreamFlag::try_from(flag).unwrap(), body: &body },
            5,
            StreamItem
        );
        check_payload!(Event { port_id, method_id, payload: &body }, 8, Event);
    }

    #[test]
    fn stream_failures(
        status in prop_oneof![
            Just(ReplyStatus::Panic),
            Just(ReplyStatus::Cancelled),
            Just(ReplyStatus::BadRequest),
        ],
        message in any::<String>(), detail in any::<String>(),
    ) {
        // Entirely length-delimited after the status byte, so every strict prefix must fail.
        check_payload!(StreamFailure { status, message: &message, detail: &detail }, usize::MAX, StreamFailure);
    }

    #[test]
    fn hello_and_log(
        version in any::<String>(), schema_hash in any::<u64>(), platform in any::<String>(),
        mode in any::<String>(), level in any::<u8>(), target in any::<String>(),
        message in any::<String>(),
    ) {
        // These payloads are entirely length-delimited, so every strict prefix must fail:
        // `usize::MAX` as the fixed length checks all of them.
        let hello = Hello {
            undra_version: &version, schema_hash, platform: &platform, mode: &mode,
        };
        check_payload!(hello, usize::MAX, Hello);
        let log = Log { level, target: &target, message: &message };
        check_payload!(log, usize::MAX, Log);
    }
}

// ---------------------------------------------------------------------------------------------
// ChangeSet and Snapshot
// ---------------------------------------------------------------------------------------------

fn change_entry() -> impl Strategy<Value = ChangeEntry> {
    (handle(), any::<u32>(), 0_u8..3, body()).prop_map(|(handle, signal_id, op, value)| {
        ChangeEntry {
            handle,
            signal_id,
            op: ChangeOp::try_from(op).unwrap(),
            value,
        }
    })
}

proptest! {
    #[test]
    fn change_set(txn_id in any::<u64>(), entries in vec(change_entry(), 0..12)) {
        let cs = ChangeSet { txn_id, entries };
        let bytes = check_payload!(cs.clone(), usize::MAX, ChangeSet);

        // The zero-copy view yields the same entries in the same order, without failing.
        let mut r = Reader::new(&bytes);
        let view = ChangeSetRef::decode(&mut r).unwrap();
        prop_assert!(r.finish().is_ok());
        prop_assert_eq!(view.txn_id, cs.txn_id);
        prop_assert_eq!(view.len(), cs.entries.len());
        prop_assert_eq!(view.iter().len(), cs.entries.len());
        let seen: Vec<ChangeEntry> = view.iter().map(|e| ChangeEntry::from(&e)).collect();
        prop_assert_eq!(&seen, &cs.entries);
        prop_assert_eq!(ChangeSet::from(&view), cs.clone());

        // The streaming builder produces the same bytes as the owned encoder, whichever
        // of its two entry methods is used.
        let mut built = Writer::new();
        let mut b = ChangeSetBuilder::new(&mut built, cs.txn_id);
        for (i, e) in cs.entries.iter().enumerate() {
            if i % 2 == 0 {
                b.push(e.handle, e.signal_id, e.op, &e.value);
            } else {
                b.push_with(e.handle, e.signal_id, e.op, |w| w.write_raw(&e.value));
            }
        }
        prop_assert_eq!(b.finish() as usize, cs.entries.len());
        prop_assert_eq!(built.as_slice(), &bytes[..]);
    }

    #[test]
    fn snapshot(
        generation_floor in any::<u64>(),
        schema_hash in any::<u64>(),
        description in ".{0,40}",
        stores in vec(
            (handle(), 0_u32..4, vec((any::<u32>(), body()), 0..5)),
            0..6,
        )
    ) {
        // Store types are drawn from a small set so that several stores share one; each type in
        // use is listed once, as a runtime writes it.
        let mut types: Vec<SnapshotType> = Vec::new();
        for (_, type_id, _) in &stores {
            if !types.iter().any(|t| t.type_id == *type_id) {
                types.push(SnapshotType { type_id: *type_id, fingerprint: u64::from(*type_id) * 31 });
            }
        }
        let snap = Snapshot {
            generation_floor,
            schema_hash,
            types,
            description,
            stores: stores
                .into_iter()
                .map(|(handle, type_id, signals)| StoreSnapshot { handle, type_id, signals })
                .collect(),
        };
        check_payload!(snap, usize::MAX, Snapshot);
    }
}

// ---------------------------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------------------------

proptest! {
    #[test]
    fn envelope(
        kind in 1_u8..=16, seq in any::<u32>(), schema in any::<u64>(),
        payload in vec(any::<u8>(), 0..200),
    ) {
        let kind = Kind::try_from(kind).unwrap();
        let mut w = Writer::new();
        Envelope::write(&mut w, kind, seq, schema, &payload);
        let frame = w.into_vec();
        prop_assert_eq!(frame.len(), Envelope::HEADER_LEN + payload.len());

        let env = Envelope::parse(&frame).unwrap();
        prop_assert_eq!(env, Envelope { kind, seq, schema, payload: &payload });

        // write_with and encode agree with write.
        let mut w = Writer::new();
        Envelope::write_with(&mut w, kind, seq, schema, |w| w.write_raw(&payload));
        prop_assert_eq!(w.as_slice(), &frame[..]);
        let mut w = Writer::new();
        env.encode(&mut w);
        prop_assert_eq!(w.as_slice(), &frame[..]);

        // Every strict prefix is rejected; so is any extension.
        for cut in 0..frame.len() {
            prop_assert!(Envelope::parse(&frame[..cut]).is_err(), "prefix {} parsed", cut);
        }
        let mut longer = frame.clone();
        longer.push(0);
        prop_assert_eq!(Envelope::parse(&longer), Err(WireError::TrailingBytes { count: 1 }));

        // The schema check is exact.
        prop_assert_eq!(env.check_schema(schema), Ok(()));
        prop_assert_eq!(
            env.check_schema(schema ^ 1),
            Err(WireError::SchemaMismatch { expected: schema ^ 1, got: schema })
        );
    }

    #[test]
    fn envelope_carries_a_typed_payload(call_id in any::<u32>(), reason in any::<String>()) {
        // A realistic frame: a bad-request Reply whose body is a String.
        let mut body = Writer::new();
        reason.encode(&mut body);
        let mut frame = Writer::new();
        Envelope::write_with(&mut frame, Kind::Reply, 1, 7, |w| {
            Reply { call_id, status: ReplyStatus::BadRequest, body: body.as_slice() }.encode(w)
        });
        let env = Envelope::parse(frame.as_slice()).unwrap();
        let reply = Reply::decode(&mut Reader::new(env.payload)).unwrap();
        prop_assert_eq!(reply.call_id, call_id);
        prop_assert_eq!(String::decode_exact(reply.body), Ok(reason));
    }
}

// ---------------------------------------------------------------------------------------------
// Keyed patches
// ---------------------------------------------------------------------------------------------

fn patch_op<T: Debug + Clone + 'static>(
    item: BoxedStrategy<T>,
) -> impl Strategy<Value = PatchOp<T>> {
    prop_oneof![
        (any::<u32>(), item.clone()).prop_map(|(index, item)| PatchOp::Insert { index, item }),
        any::<u32>().prop_map(|index| PatchOp::Remove { index }),
        (any::<u32>(), item).prop_map(|(index, item)| PatchOp::Update { index, item }),
        (any::<u32>(), any::<u32>()).prop_map(|(from, to)| PatchOp::Move { from, to }),
        Just(PatchOp::Clear),
    ]
}

/// Wire round trip for a patch, including that every strict prefix is rejected.
fn check_patch<T: Encode + Decode + PartialEq + Debug>(
    patch: KeyedPatch<T>,
) -> Result<(), TestCaseError> {
    let mut w = Writer::new();
    patch.encode(&mut w);
    let bytes = w.into_vec();
    let mut r = Reader::new(&bytes);
    let decoded = KeyedPatch::<T>::decode(&mut r);
    prop_assert_eq!(decoded.as_ref(), Ok(&patch));
    prop_assert!(r.finish().is_ok());
    for cut in 0..bytes.len() {
        prop_assert!(
            KeyedPatch::<T>::decode(&mut Reader::new(&bytes[..cut])).is_err(),
            "prefix of {} bytes decoded",
            cut
        );
    }
    Ok(())
}

proptest! {
    #[test]
    fn keyed_patch_wire_round_trip(
        ints in vec(patch_op(any::<i32>().boxed()), 0..20),
        strings in vec(patch_op(any::<String>().boxed()), 0..10),
    ) {
        check_patch(KeyedPatch { ops: ints })?;
        check_patch(KeyedPatch { ops: strings })?;
    }

    /// `apply` never panics, whatever the ops and the list, and it is atomic: on error the
    /// list is exactly what it was.
    #[test]
    fn apply_is_total_and_atomic(
        list in vec(any::<i8>(), 0..6),
        ops in vec(patch_op(any::<i8>().boxed()), 0..12),
    ) {
        // Indices from `any::<u32>()` are almost always out of bounds, which tests the error
        // path well; pull every other op into a small range to exercise the success path.
        let small = |i: u32| i % 8;
        let ops: Vec<PatchOp<i8>> = ops
            .into_iter()
            .enumerate()
            .map(|(n, op)| {
                if n % 2 == 1 {
                    return op;
                }
                match op {
                    PatchOp::Insert { index, item } => PatchOp::Insert { index: small(index), item },
                    PatchOp::Remove { index } => PatchOp::Remove { index: small(index) },
                    PatchOp::Update { index, item } => PatchOp::Update { index: small(index), item },
                    PatchOp::Move { from, to } => PatchOp::Move { from: small(from), to: small(to) },
                    PatchOp::Clear => PatchOp::Clear,
                }
            })
            .collect();
        let patch = KeyedPatch { ops };
        let mut applied = list.clone();
        match patch.apply(&mut applied) {
            Ok(()) => {
                // The final length follows from the op sequence alone.
                let mut len = list.len();
                for op in &patch.ops {
                    match op {
                        PatchOp::Insert { .. } => len += 1,
                        PatchOp::Remove { .. } => len -= 1,
                        PatchOp::Clear => len = 0,
                        PatchOp::Update { .. } | PatchOp::Move { .. } => {}
                    }
                }
                prop_assert_eq!(applied.len(), len);
            }
            Err(_) => prop_assert_eq!(applied, list),
        }
    }
}

type Item = (u32, i8);

/// A list of items with distinct keys drawn from `0..40`.
fn unique_list() -> impl Strategy<Value = Vec<Item>> {
    vec((0_u32..40, any::<i8>()), 0..30).prop_map(dedupe)
}

fn dedupe(items: Vec<Item>) -> Vec<Item> {
    let mut seen = HashSet::new();
    items.into_iter().filter(|(k, _)| seen.insert(*k)).collect()
}

/// Derives a `new` list from `old` the way a UI list evolves: some items dropped, some edited,
/// the order sometimes shuffled, fresh items sprinkled in.
fn evolve(old: &[Item], seed: u64) -> Vec<Item> {
    let mut rng = Rng::new(seed);
    let keep_pct = 40 + rng.below(61); // keep 40..=100 percent
    let edit_pct = rng.below(50);
    let shuffle = rng.below(3) == 0;
    let mut new: Vec<Item> = Vec::new();
    for &(k, v) in old {
        if rng.below(100) < keep_pct {
            let edited = rng.below(100) < edit_pct;
            new.push((k, if edited { v.wrapping_add(1) } else { v }));
        }
    }
    if shuffle {
        for i in (1..new.len()).rev() {
            let j = rng.below(i + 1);
            new.swap(i, j);
        }
    } else if new.len() > 1 && rng.below(2) == 0 {
        // A few local swaps: the common "drag one row" shape.
        for _ in 0..rng.below(3) {
            let (a, b) = (rng.below(new.len()), rng.below(new.len()));
            new.swap(a, b);
        }
    }
    for n in 0..rng.below(6) {
        let at = rng.below(new.len() + 1);
        let value = rng.byte() as i8;
        new.insert(at, (100 + n as u32, value));
    }
    new
}

/// Guards against the diff properties below passing vacuously: the generated list pairs must
/// mostly produce patches, and the patches must contain every kind of op.
#[test]
fn evolved_lists_exercise_every_op_kind() {
    let base: Vec<Item> = (0..20).map(|i| (i, i as i8)).collect();
    let (mut patches, mut inserts, mut removes, mut updates, mut moves) = (0, 0, 0, 0, 0);
    for seed in 0..3000 {
        let new = evolve(&base, seed);
        let Some(patch) = diff(&base, &new) else {
            continue;
        };
        patches += 1;
        for op in &patch.ops {
            match op {
                PatchOp::Insert { .. } => inserts += 1,
                PatchOp::Remove { .. } => removes += 1,
                PatchOp::Update { .. } => updates += 1,
                PatchOp::Move { .. } => moves += 1,
                PatchOp::Clear => unreachable!("diff never emits Clear"),
            }
        }
    }
    assert!(patches > 1500, "only {patches} of 3000 pairs gave a patch");
    for (kind, n) in [
        ("insert", inserts),
        ("remove", removes),
        ("update", updates),
        ("move", moves),
    ] {
        assert!(n > 500, "only {n} {kind} ops across all patches");
    }
}

/// Length of the longest strictly increasing subsequence, computed the slow obvious way.
fn lis_len(seq: &[usize]) -> usize {
    let mut best = vec![1_usize; seq.len()];
    for i in 0..seq.len() {
        for j in 0..i {
            if seq[j] < seq[i] {
                best[i] = best[i].max(best[j] + 1);
            }
        }
    }
    best.into_iter().max().unwrap_or(0)
}

fn diff(old: &[Item], new: &[Item]) -> Option<KeyedPatch<Item>> {
    KeyedPatch::diff(old, new, |t| t.0, |a, b| a == b)
}

/// Everything `diff` promises, checked against an independent computation.
fn check_diff(old: &[Item], new: &[Item]) -> Result<(), TestCaseError> {
    let new_keys: HashSet<u32> = new.iter().map(|t| t.0).collect();
    let old_by_key: HashMap<u32, Item> = old.iter().map(|&t| (t.0, t)).collect();
    let removed = old.iter().filter(|t| !new_keys.contains(&t.0)).count();
    let kept = old.len() - removed;
    let expect_patch = removed * 2 <= old.len() && !(kept == 0 && old.len() + new.len() > 0);

    let Some(patch) = diff(old, new) else {
        prop_assert!(!expect_patch, "diff gave up on {:?} -> {:?}", old, new);
        return Ok(());
    };
    prop_assert!(expect_patch, "diff should have chosen the full value");

    // Correctness: applying the patch to `old` gives `new`.
    let mut list = old.to_vec();
    patch.apply(&mut list).unwrap();
    prop_assert_eq!(&list, new, "patch {:?}", patch);

    // It survives the wire.
    let mut w = Writer::new();
    patch.encode(&mut w);
    let mut r = Reader::new(w.as_slice());
    let decoded = KeyedPatch::<Item>::decode(&mut r);
    prop_assert_eq!(decoded.as_ref(), Ok(&patch));
    prop_assert!(r.finish().is_ok());

    // Shape: removals first and descending, then the rest; no Clear.
    let removal_indices: Vec<u32> = patch
        .ops
        .iter()
        .map_while(|op| match op {
            PatchOp::Remove { index } => Some(*index),
            _ => None,
        })
        .collect();
    prop_assert_eq!(removal_indices.len(), removed);
    prop_assert!(
        removal_indices.windows(2).all(|w| w[0] > w[1]),
        "removals not descending"
    );
    prop_assert!(
        patch.ops[removed..]
            .iter()
            .all(|op| !matches!(op, PatchOp::Remove { .. } | PatchOp::Clear)),
        "a Remove or Clear after the removal phase"
    );

    // Exact op counts.
    let count = |f: fn(&PatchOp<Item>) -> bool| patch.ops.iter().filter(|op| f(op)).count();
    let inserted = new
        .iter()
        .filter(|t| !old_by_key.contains_key(&t.0))
        .count();
    let changed = new
        .iter()
        .filter(|t| old_by_key.get(&t.0).is_some_and(|o| o != *t))
        .count();
    prop_assert_eq!(count(|op| matches!(op, PatchOp::Insert { .. })), inserted);
    prop_assert_eq!(count(|op| matches!(op, PatchOp::Update { .. })), changed);

    // Minimality: exactly as many moves as survivors outside a longest in-order run.
    let position_in_new: HashMap<u32, usize> =
        new.iter().enumerate().map(|(i, t)| (t.0, i)).collect();
    let survivor_targets: Vec<usize> = old
        .iter()
        .filter_map(|t| position_in_new.get(&t.0).copied())
        .collect();
    prop_assert_eq!(
        count(|op| matches!(op, PatchOp::Move { .. })),
        survivor_targets.len() - lis_len(&survivor_targets),
        "moves are not minimal for {:?} -> {:?}: {:?}",
        old,
        new,
        patch
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    /// The headline property: whenever `diff` produces a patch, applying it to `old` gives
    /// exactly `new`. Independent random lists.
    #[test]
    fn diff_then_apply_reproduces_new_for_unrelated_lists(
        old in unique_list(), new in unique_list()
    ) {
        check_diff(&old, &new)?;
    }

    /// Same, for lists related the way real updates are, which is where patches (rather
    /// than full values) get chosen.
    #[test]
    fn diff_then_apply_reproduces_new_for_evolved_lists(old in unique_list(), seed in any::<u64>()) {
        let new = evolve(&old, seed);
        check_diff(&old, &new)?;
    }

    #[test]
    fn diff_of_a_list_with_itself_is_empty(list in unique_list()) {
        match diff(&list, &list) {
            Some(patch) => prop_assert!(patch.is_empty()),
            None => prop_assert!(false, "identical lists must diff to an empty patch"),
        }
    }

    #[test]
    fn diff_with_duplicate_keys_never_yields_a_wrong_patch(
        old in vec((0_u32..6, any::<i8>()), 0..10),
        new in vec((0_u32..6, any::<i8>()), 0..10),
    ) {
        // Duplicate keys are ambiguous, so diff must decline (or, if it does answer because
        // the keys happen to be unique after all, answer correctly).
        if let Some(patch) = diff(&old, &new) {
            let mut list = old.clone();
            patch.apply(&mut list).unwrap();
            prop_assert_eq!(list, new);
        }
    }
}
