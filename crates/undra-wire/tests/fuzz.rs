//! Byte-fuzz tests: no input, however malformed, may panic, hang, overflow the stack or
//! allocate without bound in any decoder. Every decoder must return `Ok` or `Err`.
//!
//! `cargo-fuzz` needs a nightly toolchain, so this is an in-tree harness: a seeded PRNG
//! (failures reproduce from the printed seed and input) feeding every decoder in the crate,
//! both with uniformly random bytes and with mutations of valid encodings, which get much
//! deeper into the decoders than random bytes ever do.

mod common;

use std::collections::{BTreeMap, HashMap};
use std::hint::black_box;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

use common::{Rng, hex};
use undra_wire::payload::{
    Call, CallOwned, CallTarget, Cancel, ChangeEntry, ChangeOp, ChangeSet, ChangeSetBuilder,
    ChangeSetRef, Event, Hello, Log, Observe, PortCall, PortReply, Release, Reply, ReplyStatus,
    Restore, Snapshot, StoreSnapshot, StreamCredit, StreamFailure, StreamItem, TimerFired,
};
use undra_wire::{
    Bytes, Decode, Encode, Envelope, Handle, KeyedPatch, Kind, PatchOp, Reader, Timestamp, Uuid,
    WireError, Writer,
};

const ITERATIONS: usize = 20_000;

/// A recursive type: `Vec<Tree>` inside `Tree`. Decoding hostile input must hit the depth
/// limit rather than the stack limit.
#[derive(Debug)]
struct Tree(#[allow(dead_code)] Vec<Tree>);

impl Decode for Tree {
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        Vec::<Tree>::decode(r).map(Tree)
    }
}

/// A recursive type through `Option<Box<_>>`.
#[derive(Debug)]
struct Node(#[allow(dead_code)] Option<Box<Node>>);

impl Decode for Node {
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        Option::<Box<Node>>::decode(r).map(Node)
    }
}

/// A named decoder; it returns whether the input was accepted.
type Decoder = (&'static str, fn(&[u8]) -> bool);

/// Decodes `bytes` as `T`. When decoding succeeds and `canonical` is set, also checks that
/// re-encoding reproduces exactly the bytes that were consumed.
fn decode_as<T: Decode + Encode>(bytes: &[u8], canonical: bool) -> bool {
    let mut r = Reader::new(bytes);
    let result = T::decode(&mut r);
    if let Ok(value) = &result {
        if canonical {
            assert_eq!(
                value.encode_to_vec(),
                bytes[..r.position()],
                "re-encoding an accepted value must reproduce the input"
            );
        }
    }
    let ok = result.is_ok();
    let _ = black_box(T::decode_exact(bytes).map(|_| ()));
    black_box(ok)
}

fn decode_only<T: Decode>(bytes: &[u8]) -> bool {
    let mut r = Reader::new(bytes);
    black_box(T::decode(&mut r)).is_ok()
}

fn accepted<T, E>(result: Result<T, E>) -> bool {
    black_box(result).is_ok()
}

macro_rules! decoders {
    ($($name:literal => $body:expr),* $(,)?) => {
        &[ $( ($name, $body) ),* ]
    };
}

/// Every decoder in the crate, by name.
const DECODERS: &[Decoder] = decoders! {
    "bool" => |b| decode_as::<bool>(b, true),
    "u8" => |b| decode_as::<u8>(b, true),
    "u16" => |b| decode_as::<u16>(b, true),
    "u32" => |b| decode_as::<u32>(b, true),
    "u64" => |b| decode_as::<u64>(b, true),
    "i8" => |b| decode_as::<i8>(b, true),
    "i16" => |b| decode_as::<i16>(b, true),
    "i32" => |b| decode_as::<i32>(b, true),
    "i64" => |b| decode_as::<i64>(b, true),
    "f32" => |b| decode_only::<f32>(b),
    "f64" => |b| decode_only::<f64>(b),
    "()" => |b| decode_as::<()>(b, true),
    "String" => |b| decode_as::<String>(b, true),
    "Bytes" => |b| decode_as::<Bytes>(b, true),
    "Option<String>" => |b| decode_as::<Option<String>>(b, true),
    "Option<Option<u8>>" => |b| decode_as::<Option<Option<u8>>>(b, true),
    "Vec<u8>" => |b| decode_as::<Vec<u8>>(b, true),
    "Vec<i32>" => |b| decode_as::<Vec<i32>>(b, true),
    "Vec<String>" => |b| decode_as::<Vec<String>>(b, true),
    "Vec<Vec<Vec<u8>>>" => |b| decode_as::<Vec<Vec<Vec<u8>>>>(b, true),
    "Vec<()>" => |b| decode_as::<Vec<()>>(b, true),
    "Vec<(u32, String)>" => |b| decode_as::<Vec<(u32, String)>>(b, true),
    "HashMap<String, i32>" => |b| decode_only::<HashMap<String, i32>>(b),
    "HashMap<u8, ()>" => |b| decode_only::<HashMap<u8, ()>>(b),
    "HashMap<(), ()>" => |b| decode_only::<HashMap<(), ()>>(b),
    "BTreeMap<u32, String>" => |b| decode_only::<BTreeMap<u32, String>>(b),
    "BTreeMap<Uuid, Vec<u8>>" => |b| decode_only::<BTreeMap<Uuid, Vec<u8>>>(b),
    "Duration" => |b| decode_as::<Duration>(b, true),
    "Timestamp" => |b| decode_as::<Timestamp>(b, true),
    "Uuid" => |b| decode_as::<Uuid>(b, true),
    "Handle" => |b| decode_as::<Handle>(b, true),
    "Result<i32, String>" => |b| decode_as::<Result<i32, String>>(b, true),
    "(u8,)" => |b| decode_as::<(u8,)>(b, true),
    "(u8, u16)" => |b| decode_as::<(u8, u16)>(b, true),
    "(bool, String, Option<u8>)" => |b| decode_as::<(bool, String, Option<u8>)>(b, true),
    "(u8, u8, u8, Vec<u8>)" => |b| decode_as::<(u8, u8, u8, Vec<u8>)>(b, true),
    "Box<String>" => |b| decode_as::<Box<String>>(b, true),
    "Tree" => |b| decode_only::<Tree>(b),
    "Node" => |b| decode_only::<Node>(b),
    "Envelope::parse" => |b| accepted(Envelope::parse(b)),
    "Call" => |b| accepted(Call::decode(&mut Reader::new(b))),
    "CallOwned" => |b| accepted(CallOwned::decode(&mut Reader::new(b))),
    "Reply" => |b| accepted(Reply::decode(&mut Reader::new(b))),
    "PortCall" => |b| accepted(PortCall::decode(&mut Reader::new(b))),
    "PortReply" => |b| accepted(PortReply::decode(&mut Reader::new(b))),
    "Cancel" => |b| accepted(Cancel::decode(&mut Reader::new(b))),
    "StreamCredit" => |b| accepted(StreamCredit::decode(&mut Reader::new(b))),
    "StreamItem" => |b| accepted(StreamItem::decode(&mut Reader::new(b))),
    "StreamFailure" => |b| accepted(StreamFailure::decode(&mut Reader::new(b))),
    "Observe" => |b| accepted(Observe::decode(&mut Reader::new(b))),
    "Release" => |b| accepted(Release::decode(&mut Reader::new(b))),
    "Event" => |b| accepted(Event::decode(&mut Reader::new(b))),
    "Hello" => |b| accepted(Hello::decode(&mut Reader::new(b))),
    "Log" => |b| accepted(Log::decode(&mut Reader::new(b))),
    "TimerFired" => |b| accepted(TimerFired::decode(&mut Reader::new(b))),
    "ChangeSet" => |b| accepted(ChangeSet::decode(&mut Reader::new(b))),
    "ChangeSetRef" => |b| {
        let Ok(set) = ChangeSetRef::decode(&mut Reader::new(b)) else {
            return false;
        };
        // Iterating a validated view must yield exactly `len` entries and never fail.
        let mut n = 0;
        let mut value_bytes = 0;
        for entry in set.iter() {
            n += 1;
            value_bytes += entry.value.len();
        }
        assert_eq!(n, set.len());
        assert!(value_bytes <= b.len());
        true
    },
    "Snapshot" => |b| accepted(Snapshot::decode(&mut Reader::new(b))),
    "Restore" => |b| accepted(Restore::decode(&mut Reader::new(b))),
    "KeyedPatch<i32>" => |b| {
        let Ok(patch) = KeyedPatch::<i32>::decode(&mut Reader::new(b)) else {
            return false;
        };
        // Applying arbitrary decoded ops to a list must be total.
        let mut list = vec![1, 2, 3];
        let _ = black_box(patch.apply(&mut list));
        true
    },
    "KeyedPatch<String>" => |b| accepted(KeyedPatch::<String>::decode(&mut Reader::new(b))),
    "KeyedPatch<(u32, Vec<u8>)>" => |b| accepted(KeyedPatch::<(u32, Vec<u8>)>::decode(&mut Reader::new(b))),
    "Kind" => |b| accepted(b.first().map_or(Err(()), |&t| Kind::try_from(t).map_err(|_| ()))),
    "Reader primitives" => |b| reader_program(b),
};

/// Drives a `Reader` with a "program" derived from the input itself: each byte picks the next
/// primitive to call. Exercises every `Reader` method on every kind of position.
fn reader_program(bytes: &[u8]) -> bool {
    let mut r = Reader::new(bytes);
    let mut steps = 0;
    while steps < 256 {
        steps += 1;
        let op = match bytes.get(r.position()) {
            Some(&b) => b % 18,
            None => 17,
        };
        let outcome: Result<(), WireError> = match op {
            0 => r.read_u8().map(drop),
            1 => r.read_u16().map(drop),
            2 => r.read_u32().map(drop),
            3 => r.read_u64().map(drop),
            4 => r.read_i8().map(drop),
            5 => r.read_i16().map(drop),
            6 => r.read_i32().map(drop),
            7 => r.read_i64().map(drop),
            8 => r.read_f32().map(drop),
            9 => r.read_f64().map(drop),
            10 => r.read_bool().map(drop),
            11 => r.read_str().map(drop),
            12 => r.read_bytes().map(drop),
            13 => r.read_len().map(drop),
            14 => r.read_count(3).map(drop),
            15 => r.read_count(0).map(drop),
            16 => r.read_array::<7>().map(drop),
            _ => {
                let rest = r.read_rest();
                assert!(rest.len() <= bytes.len());
                assert_eq!(r.remaining(), 0);
                assert!(r.finish().is_ok());
                return true;
            }
        };
        if outcome.is_err() {
            // After an error the position is unspecified but still in range.
            assert!(r.position() <= bytes.len());
            return false;
        }
        assert!(r.position() <= bytes.len());
        assert_eq!(r.position() + r.remaining(), bytes.len());
    }
    true
}

/// How many inputs each decoder accepted.
type Accepted = HashMap<&'static str, usize>;

/// Runs every decoder on `bytes`; on a panic, reports which decoder and which input.
fn run_all(seed: u64, iteration: usize, bytes: &[u8], accepted: &mut Accepted) {
    for (name, decode) in DECODERS {
        match catch_unwind(AssertUnwindSafe(|| decode(bytes))) {
            Ok(true) => *accepted.entry(name).or_insert(0) += 1,
            Ok(false) => {}
            Err(_) => panic!(
                "decoder {name} panicked (seed {seed:#x}, iteration {iteration}) on {} bytes: {}",
                bytes.len(),
                hex(bytes)
            ),
        }
    }
}

/// Bytes that tend to be structurally meaningful: tags, small lengths, all-ones.
const INTERESTING: &[u8] = &[0, 0, 0, 1, 1, 2, 3, 4, 5, 8, 16, 0x7f, 0x80, 0xff, 0xff];

fn random_input(rng: &mut Rng) -> Vec<u8> {
    let len = match rng.below(4) {
        0 => rng.below(9),
        1 => rng.below(65),
        2 => rng.below(257),
        _ => rng.below(2049),
    };
    if rng.below(2) == 0 {
        rng.bytes(len)
    } else {
        // Biased toward values that pass tag and length checks, so decoders get past their
        // first few bytes far more often.
        (0..len)
            .map(|_| {
                if rng.below(3) == 0 {
                    rng.byte()
                } else {
                    INTERESTING[rng.below(INTERESTING.len())]
                }
            })
            .collect()
    }
}

#[test]
fn random_bytes_never_panic_any_decoder() {
    let seed = 0x554e_4452_0001;
    let mut rng = Rng::new(seed);
    let mut accepted = Accepted::new();
    for i in 0..ITERATIONS {
        let input = random_input(&mut rng);
        run_all(seed, i, &input, &mut accepted);
    }
    // The harness is not vacuous: even uniformly random bytes get accepted by the simple
    // decoders (which is what lets the canonical-encoding check run on them).
    // (Random bytes rarely form a valid string, so that bound is small.)
    for (name, at_least) in [
        ("bool", 100),
        ("u32", 100),
        ("Cancel", 100),
        ("Reply", 100),
        ("Option<String>", 100),
        ("Vec<i32>", 5),
        ("String", 3),
    ] {
        assert!(
            accepted.get(name).copied().unwrap_or(0) >= at_least,
            "{name} accepted only {:?} of {ITERATIONS} random inputs",
            accepted.get(name)
        );
    }
}

/// Valid encodings of composite values, to be mutated.
fn seeds() -> Vec<Vec<u8>> {
    let mut seeds: Vec<Vec<u8>> = Vec::new();

    // Envelope frames around real payloads.
    let cs = ChangeSet {
        txn_id: 42,
        entries: vec![
            ChangeEntry {
                handle: Handle::new(1, 1),
                signal_id: 0,
                op: ChangeOp::Full,
                value: vec![2, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0],
            },
            ChangeEntry {
                handle: Handle::new(2, 1),
                signal_id: 1,
                op: ChangeOp::KeyedPatch,
                value: vec![1, 0, 0, 0, 4],
            },
        ],
    };
    let mut payload = Writer::new();
    cs.encode(&mut payload);
    seeds.push(payload.as_slice().to_vec());
    let mut frame = Writer::new();
    Envelope::write(&mut frame, Kind::ChangeSet, 9, 0xfeed, payload.as_slice());
    seeds.push(frame.into_vec());

    let mut w = Writer::new();
    let mut b = ChangeSetBuilder::new(&mut w, 1);
    b.push(Handle::new(3, 2), 7, ChangeOp::LazyInvalidated, &[]);
    b.finish();
    seeds.push(w.into_vec());

    let snapshot = Snapshot {
        generation_floor: 4,
        stores: vec![
            StoreSnapshot {
                handle: Handle::new(1, 1),
                type_id: 0xabcd,
                signals: vec![(0, vec![1, 2, 3]), (1, vec![])],
            },
            StoreSnapshot {
                handle: Handle::new(2, 4),
                type_id: 0xdcba,
                signals: vec![],
            },
        ],
    };
    seeds.push(snapshot.encode_to_vec_payload());

    let targets = [
        CallTarget::Function { method_id: 5 },
        CallTarget::Method {
            handle: Handle::new(1, 1),
            method_id: 6,
        },
        CallTarget::Constructor {
            type_id: 7,
            method_id: 8,
        },
        CallTarget::LazyPage {
            handle: Handle::new(2, 2),
            offset: 10,
            limit: 20,
        },
    ];
    for target in targets {
        let mut w = Writer::new();
        Call {
            target,
            call_id: 9,
            args: &[1, 0, 0, 0, 2, 0, 0, 0],
        }
        .encode(&mut w);
        seeds.push(w.into_vec());
    }

    let mut w = Writer::new();
    Reply {
        call_id: 3,
        status: ReplyStatus::Panic,
        body: &"boom".to_owned().encode_to_vec(),
    }
    .encode(&mut w);
    seeds.push(w.into_vec());

    let mut w = Writer::new();
    Hello {
        undra_version: "1.0.0",
        schema_hash: 0x0123_4567_89ab_cdef,
        platform: "ios",
        mode: "dev",
    }
    .encode(&mut w);
    seeds.push(w.into_vec());

    let mut w = Writer::new();
    Log {
        level: 2,
        target: "undra::runtime",
        message: "héllo 🌊",
    }
    .encode(&mut w);
    seeds.push(w.into_vec());

    seeds.push(
        KeyedPatch {
            ops: vec![
                PatchOp::Insert {
                    index: 0,
                    item: 5_i32,
                },
                PatchOp::Update { index: 0, item: 6 },
                PatchOp::Move { from: 0, to: 1 },
                PatchOp::Remove { index: 1 },
                PatchOp::Clear,
            ],
        }
        .encode_to_vec_payload(),
    );
    seeds.push(
        KeyedPatch {
            ops: vec![PatchOp::Insert {
                index: 1,
                item: "row".to_owned(),
            }],
        }
        .encode_to_vec_payload(),
    );

    // Plain values.
    seeds.push(vec!["a".to_owned(), String::new(), "héllo".to_owned()].encode_to_vec());
    seeds.push(HashMap::from([("x".to_owned(), 1_i32), ("yy".to_owned(), -2)]).encode_to_vec());
    seeds.push(BTreeMap::from([(1_u32, "one".to_owned()), (2, "two".to_owned())]).encode_to_vec());
    seeds.push(Some(Some(7_u8)).encode_to_vec());
    seeds.push(Ok::<i32, String>(5).encode_to_vec());
    seeds.push((true, "s".to_owned(), Some(3_u8)).encode_to_vec());
    seeds.push(vec![vec![vec![1_u8, 2], vec![]], vec![]].encode_to_vec());
    seeds.push(Duration::from_millis(1500).encode_to_vec());
    seeds.push(Uuid([0x12; 16]).encode_to_vec());
    seeds.push(vec![(); 3].encode_to_vec());
    seeds.push(vec![1_u8, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0]); // nested singletons

    seeds
}

/// Small convenience: encode via the inherent `encode` of payload-style types.
trait EncodePayload {
    fn encode_to_vec_payload(&self) -> Vec<u8>;
}

impl EncodePayload for Snapshot {
    fn encode_to_vec_payload(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.encode(&mut w);
        w.into_vec()
    }
}

impl<T: Encode> EncodePayload for KeyedPatch<T> {
    fn encode_to_vec_payload(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.encode(&mut w);
        w.into_vec()
    }
}

fn mutate(rng: &mut Rng, seeds: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = seeds[rng.below(seeds.len())].clone();
    for _ in 0..1 + rng.below(4) {
        match rng.below(8) {
            // Flip one bit.
            0 if !bytes.is_empty() => {
                let i = rng.below(bytes.len());
                bytes[i] ^= 1 << rng.below(8);
            }
            // Overwrite a byte with an interesting value.
            1 if !bytes.is_empty() => {
                let i = rng.below(bytes.len());
                bytes[i] = INTERESTING[rng.below(INTERESTING.len())];
            }
            // Overwrite four bytes (a length or count) with an extreme.
            2 if bytes.len() >= 4 => {
                let i = rng.below(bytes.len() - 3);
                let v: u32 =
                    [0, 1, 2, 0x7fff_ffff, 0x8000_0000, u32::MAX, 0x0100_0000][rng.below(7)];
                bytes[i..i + 4].copy_from_slice(&v.to_le_bytes());
            }
            // Truncate.
            3 if !bytes.is_empty() => {
                let keep = rng.below(bytes.len());
                bytes.truncate(keep);
            }
            // Insert random bytes.
            4 => {
                let at = rng.below(bytes.len() + 1);
                let n = 1 + rng.below(8);
                let extra = rng.bytes(n);
                bytes.splice(at..at, extra);
            }
            // Delete a span.
            5 if bytes.len() > 1 => {
                let from = rng.below(bytes.len());
                let to = (from + 1 + rng.below(8)).min(bytes.len());
                bytes.drain(from..to);
            }
            // Duplicate a span.
            6 if !bytes.is_empty() => {
                let from = rng.below(bytes.len());
                let to = (from + 1 + rng.below(16)).min(bytes.len());
                let span = bytes[from..to].to_vec();
                bytes.splice(to..to, span);
            }
            // Append garbage.
            _ => {
                let n = rng.below(6);
                bytes.extend(rng.bytes(n));
            }
        }
    }
    bytes
}

#[test]
fn mutated_valid_encodings_never_panic_any_decoder() {
    let seed = 0x554e_4452_0002;
    let mut rng = Rng::new(seed);
    let seeds = seeds();
    // Unmutated seeds first: the composite decoders must accept their own kind of seed.
    let mut accepted = Accepted::new();
    for (i, s) in seeds.iter().enumerate() {
        run_all(seed, i, s, &mut accepted);
    }
    for name in [
        "ChangeSet",
        "ChangeSetRef",
        "Snapshot",
        "Call",
        "Reply",
        "Hello",
        "Log",
        "KeyedPatch<i32>",
        "KeyedPatch<String>",
        "Envelope::parse",
        "HashMap<String, i32>",
        "BTreeMap<u32, String>",
    ] {
        assert!(
            accepted.get(name).copied().unwrap_or(0) >= 1,
            "no seed is accepted by {name}"
        );
    }

    let mut accepted = Accepted::new();
    for i in 0..ITERATIONS {
        let input = mutate(&mut rng, &seeds);
        run_all(seed, i, &input, &mut accepted);
    }
    // Mutations of valid data must stay decodable often enough to reach the deep paths
    // (entry loops, nested collections, map insertion), or the fuzzing proves little.
    for name in [
        "ChangeSet",
        "ChangeSetRef",
        "Snapshot",
        "Call",
        "Hello",
        "Log",
        "KeyedPatch<i32>",
        "Vec<String>",
        "HashMap<String, i32>",
        "BTreeMap<u32, String>",
    ] {
        assert!(
            accepted.get(name).copied().unwrap_or(0) >= 20,
            "{name} accepted only {:?} of {ITERATIONS} mutated inputs",
            accepted.get(name)
        );
    }
}

#[test]
fn recursion_bombs_are_errors_not_stack_overflows() {
    // `Tree`: every byte group is "one child follows"; a megabyte of input would nest a
    // quarter of a million levels if the decoder did not stop.
    let mut bomb = Vec::new();
    for _ in 0..250_000 {
        bomb.extend_from_slice(&1_u32.to_le_bytes());
    }
    bomb.extend_from_slice(&0_u32.to_le_bytes());
    assert!(matches!(
        Tree::decode_exact(&bomb),
        Err(WireError::NestingTooDeep { .. })
    ));

    // `Node`: Some(Some(Some(...))) through Box.
    let bomb = vec![1_u8; 500_000];
    assert!(matches!(
        Node::decode_exact(&bomb),
        Err(WireError::NestingTooDeep { .. })
    ));

    // Nested statically-typed vectors are shallow and fine.
    assert!(Vec::<Vec<Vec<u8>>>::decode_exact(&[0, 0, 0, 0]).is_ok());
}

#[test]
fn hostile_counts_do_not_allocate_proportionally() {
    // A count of u32::MAX in front of a few bytes must be rejected up front for every
    // collection type. If any of them reserved capacity from the count this would abort the
    // process with an allocation failure long before the assertion.
    let mut bytes = u32::MAX.to_le_bytes().to_vec();
    bytes.extend_from_slice(&[0; 32]);
    assert!(Vec::<u8>::decode_exact(&bytes).is_err());
    assert!(Vec::<u64>::decode_exact(&bytes).is_err());
    assert!(Vec::<Vec<u64>>::decode_exact(&bytes).is_err());
    assert!(Vec::<Option<u8>>::decode_exact(&bytes).is_err());
    assert!(HashMap::<u64, u64>::decode_exact(&bytes).is_err());
    assert!(BTreeMap::<u8, u8>::decode_exact(&bytes).is_err());
    assert!(String::decode_exact(&bytes).is_err());
    assert!(Bytes::decode_exact(&bytes).is_err());
    assert!(Vec::<()>::decode_exact(&bytes).is_err());
    assert!(KeyedPatch::<u8>::decode(&mut Reader::new(&bytes)).is_err());
    assert!(Snapshot::decode(&mut Reader::new(&bytes)).is_err());
    let mut changeset = 1_u64.to_le_bytes().to_vec();
    changeset.extend_from_slice(&bytes);
    assert!(ChangeSetRef::decode(&mut Reader::new(&changeset)).is_err());
    assert!(ChangeSet::decode(&mut Reader::new(&changeset)).is_err());
}
