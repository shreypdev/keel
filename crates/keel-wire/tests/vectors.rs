//! Table-driven test over `contract-tests/wire-vectors.json`, the byte-exact contract shared
//! with the Swift, Kotlin and TypeScript codecs. Every vector is checked in both directions:
//! encoding the described value must give exactly `hex`, and decoding `hex` must give the
//! value back with no trailing bytes.

mod common;

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::time::Duration;

use common::{hex, unhex};
use keel_wire::payload::{
    Call, CallOwned, CallTarget, ChangeEntry, ChangeOp, ChangeSet, ChangeSetRef, Reply, ReplyStatus,
};
use keel_wire::{
    Bytes, Decode, Encode, Envelope, Handle, KeyedPatch, Kind, PatchOp, Reader, Timestamp, Uuid,
    Writer,
};
use serde_json::Value;

/// Vectors this crate does not cover, with the reason. Everything else must be covered, so a
/// new vector cannot be silently ignored.
const SKIPPED: &[(&str, &str)] = &[
    (
        "fnv1a32_calculator_add",
        "FNV hashing lives in keel-meta, not the wire crate",
    ),
    (
        "fnv1a64_keel",
        "FNV hashing lives in keel-meta, not the wire crate",
    ),
];

fn load() -> Vec<Value> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../contract-tests/wire-vectors.json"
    );
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
    let doc: Value = serde_json::from_str(&text).expect("vectors file is valid JSON");
    doc["vectors"]
        .as_array()
        .expect("`vectors` is an array")
        .clone()
}

/// Integers beyond the JS safe range are strings in the file; small ones are numbers.
fn int(v: &Value) -> i128 {
    match v {
        Value::Number(n) => n
            .as_i64()
            .map(i128::from)
            .or_else(|| n.as_u64().map(i128::from))
            .unwrap_or_else(|| panic!("not an integer: {n}")),
        Value::String(s) => s
            .parse()
            .unwrap_or_else(|_| panic!("not an integer: {s:?}")),
        other => panic!("not an integer: {other:?}"),
    }
}

fn u32_of(v: &Value) -> u32 {
    u32::try_from(int(v)).expect("fits u32")
}

/// Encoding `value` must give `expected`; decoding `expected` must give `value`.
fn both<T: Encode + Decode + PartialEq + Debug>(name: &str, value: T, expected: &[u8]) {
    assert_eq!(
        hex(&value.encode_to_vec()),
        hex(expected),
        "{name}: encoding differs"
    );
    assert_eq!(
        T::decode_exact(expected).as_ref(),
        Ok(&value),
        "{name}: decoding differs"
    );
}

/// The vectors describe `args` and `body` as logical `i32` lists; on the wire they are
/// those integers encoded back to back.
fn i32_list_bytes(v: &Value) -> Vec<u8> {
    let mut w = Writer::new();
    for item in v.as_array().expect("array of ints") {
        i32::try_from(int(item)).expect("fits i32").encode(&mut w);
    }
    w.into_vec()
}

fn check_call(name: &str, value: &Value, expected: &[u8]) {
    let args = i32_list_bytes(&value["args"]);
    let call_id = u32_of(&value["call_id"]);
    let target = match int(&value["target"]) {
        0 => CallTarget::Function {
            method_id: u32_of(&value["method_id"]),
        },
        1 => CallTarget::Method {
            handle: Handle(u64::try_from(int(&value["handle"])).unwrap()),
            method_id: u32_of(&value["method_id"]),
        },
        other => panic!("{name}: vector target {other} not handled by the test"),
    };
    let call = Call {
        target,
        call_id,
        args: &args,
    };
    let mut w = Writer::new();
    call.encode(&mut w);
    assert_eq!(hex(w.as_slice()), hex(expected), "{name}: encoding differs");

    let mut r = Reader::new(expected);
    assert_eq!(Call::decode(&mut r), Ok(call), "{name}: decoding differs");
    r.finish().unwrap();

    // The owned variant is the same format.
    let mut r = Reader::new(expected);
    let owned = CallOwned::decode(&mut r).unwrap();
    assert_eq!(owned.as_call(), call, "{name}: owned decoding differs");
}

fn check_reply(name: &str, value: &Value, expected: &[u8]) {
    let body = i32_list_bytes(&Value::Array(vec![value["body"].clone()]));
    let reply = Reply {
        call_id: u32_of(&value["call_id"]),
        status: ReplyStatus::try_from(u8::try_from(int(&value["status"])).unwrap()).unwrap(),
        body: &body,
    };
    let mut w = Writer::new();
    reply.encode(&mut w);
    assert_eq!(hex(w.as_slice()), hex(expected), "{name}: encoding differs");
    let mut r = Reader::new(expected);
    assert_eq!(Reply::decode(&mut r), Ok(reply), "{name}: decoding differs");
    r.finish().unwrap();
}

fn check_changeset(name: &str, value: &Value, expected: &[u8]) {
    let entries: Vec<ChangeEntry> = value["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| ChangeEntry {
            handle: Handle(u64::try_from(int(&e["handle"])).unwrap()),
            signal_id: u32_of(&e["signal_id"]),
            op: ChangeOp::try_from(u8::try_from(int(&e["op"])).unwrap()).unwrap(),
            // The vector's `value` is the signal's `Vec<i32>`, encoded.
            value: e["value"]
                .as_array()
                .map(|items| {
                    let ints: Vec<i32> = items
                        .iter()
                        .map(|i| i32::try_from(int(i)).unwrap())
                        .collect();
                    ints.encode_to_vec()
                })
                .unwrap(),
        })
        .collect();
    let changeset = ChangeSet {
        txn_id: u64::try_from(int(&value["txn_id"])).unwrap(),
        entries,
    };
    let mut w = Writer::new();
    changeset.encode(&mut w);
    assert_eq!(hex(w.as_slice()), hex(expected), "{name}: encoding differs");
    let mut r = Reader::new(expected);
    assert_eq!(
        ChangeSet::decode(&mut r).as_ref(),
        Ok(&changeset),
        "{name}: decoding differs"
    );
    r.finish().unwrap();

    // The zero-copy variant sees the same entries.
    let mut r = Reader::new(expected);
    let view = ChangeSetRef::decode(&mut r).unwrap();
    r.finish().unwrap();
    assert_eq!(view.txn_id, changeset.txn_id);
    let seen: Vec<ChangeEntry> = view.iter().map(|e| ChangeEntry::from(&e)).collect();
    assert_eq!(seen, changeset.entries, "{name}: ChangeSetRef differs");
}

fn check_keyed_patch(name: &str, value: &Value, expected: &[u8]) {
    let ops = value["ops"]
        .as_array()
        .unwrap()
        .iter()
        .map(|op| match op["op"].as_str().unwrap() {
            "insert" => PatchOp::Insert {
                index: u32_of(&op["index"]),
                item: i32::try_from(int(&op["item"])).unwrap(),
            },
            "remove" => PatchOp::Remove {
                index: u32_of(&op["index"]),
            },
            "update" => PatchOp::Update {
                index: u32_of(&op["index"]),
                item: i32::try_from(int(&op["item"])).unwrap(),
            },
            "move" => PatchOp::Move {
                from: u32_of(&op["from"]),
                to: u32_of(&op["to"]),
            },
            "clear" => PatchOp::Clear,
            other => panic!("{name}: unknown op {other:?}"),
        })
        .collect();
    let patch = KeyedPatch { ops };
    let mut w = Writer::new();
    patch.encode(&mut w);
    assert_eq!(hex(w.as_slice()), hex(expected), "{name}: encoding differs");
    let mut r = Reader::new(expected);
    assert_eq!(
        KeyedPatch::<i32>::decode(&mut r).as_ref(),
        Ok(&patch),
        "{name}: decoding differs"
    );
    r.finish().unwrap();
}

fn check_envelope(name: &str, value: &Value, expected: &[u8]) {
    let kind = Kind::try_from(u8::try_from(int(&value["kind"])).unwrap()).unwrap();
    let seq = u32_of(&value["seq"]);
    let schema = u64::try_from(int(&value["schema"])).unwrap();
    let payload = unhex(value["payload_hex"].as_str().unwrap());

    let mut w = Writer::new();
    Envelope::write(&mut w, kind, seq, schema, &payload);
    assert_eq!(hex(w.as_slice()), hex(expected), "{name}: write differs");

    let mut w = Writer::new();
    Envelope::write_with(&mut w, kind, seq, schema, |w| w.write_raw(&payload));
    assert_eq!(
        hex(w.as_slice()),
        hex(expected),
        "{name}: write_with differs"
    );

    let env = Envelope::parse(expected).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(
        (env.kind, env.seq, env.schema, env.payload),
        (kind, seq, schema, &payload[..]),
        "{name}: parse differs"
    );
    assert_eq!(expected.len(), Envelope::HEADER_LEN + payload.len());
}

/// Covers the vectors that describe records and enums. Generated code does not exist yet, so
/// these are encoded by hand from the primitives, exactly as the macros will emit them.
fn check_record_or_enum(name: &str, ty: &str, value: &Value, expected: &[u8]) {
    match name {
        "record_todo" => {
            assert!(ty.starts_with("record Todo{id:uuid,title:string,done:bool}"));
            let todo = (
                value["id"].as_str().unwrap().parse::<Uuid>().unwrap(),
                value["title"].as_str().unwrap().to_owned(),
                value["done"].as_bool().unwrap(),
            );
            both(name, todo, expected);
        }
        "enum_unit" => {
            // enum Filter { All, Active, Done }: u16 variant index.
            let index: u16 = match value.as_str().unwrap() {
                "all" => 0,
                "active" => 1,
                "done" => 2,
                other => panic!("unknown variant {other}"),
            };
            both(name, index, expected);
        }
        "enum_data" => {
            // enum Shape { Circle { radius: f64 }, Rect { w: f64, h: f64 } }.
            assert_eq!(value["kind"], "rect");
            let rect = (
                1_u16,
                value["w"].as_f64().unwrap(),
                value["h"].as_f64().unwrap(),
            );
            both(name, rect, expected);
        }
        other => panic!("unhandled record/enum vector {other}"),
    }
}

fn check(v: &Value) {
    let name = v["name"].as_str().expect("vector has a name");
    let ty = v["type"].as_str().expect("vector has a type");
    let value = &v["value"];
    let expected = unhex(v["hex"].as_str().expect("vector has hex"));

    match ty {
        "bool" => both(name, value.as_bool().unwrap(), &expected),
        "u8" => both(name, u8::try_from(int(value)).unwrap(), &expected),
        "u16" => both(name, u16::try_from(int(value)).unwrap(), &expected),
        "u32" => both(name, u32::try_from(int(value)).unwrap(), &expected),
        "u64" => both(name, u64::try_from(int(value)).unwrap(), &expected),
        "i8" => both(name, i8::try_from(int(value)).unwrap(), &expected),
        "i16" => both(name, i16::try_from(int(value)).unwrap(), &expected),
        "i32" => both(name, i32::try_from(int(value)).unwrap(), &expected),
        "i64" => both(name, i64::try_from(int(value)).unwrap(), &expected),
        "f32" => {
            let f = value.as_f64().unwrap() as f32;
            both(name, f, &expected);
            // Bit-exact, not merely equal.
            assert_eq!(
                f.to_bits(),
                f32::from_le_bytes(expected[..].try_into().unwrap()).to_bits()
            );
        }
        "f64" => both(name, value.as_f64().unwrap(), &expected),
        "string" => both(name, value.as_str().unwrap().to_owned(), &expected),
        "bytes" => {
            let bytes: Vec<u8> = value
                .as_array()
                .unwrap()
                .iter()
                .map(|b| u8::try_from(int(b)).unwrap())
                .collect();
            both(name, Bytes(bytes), &expected);
        }
        "option<string>" => both(name, value.as_str().map(str::to_owned), &expected),
        "vec<i32>" => {
            let items: Vec<i32> = value
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i32::try_from(int(i)).unwrap())
                .collect();
            both(name, items, &expected);
        }
        "vec<string>" => {
            let items: Vec<String> = value
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_owned())
                .collect();
            both(name, items, &expected);
        }
        "map<string,i32>" => {
            let map: BTreeMap<String, i32> = value
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), i32::try_from(int(v)).unwrap()))
                .collect();
            both(name, map.clone(), &expected);
            // A HashMap must produce the same bytes: the encoder sorts by encoded key.
            let hash: std::collections::HashMap<String, i32> = map.into_iter().collect();
            both(name, hash, &expected);
        }
        "duration" => both(
            name,
            Duration::from_nanos(u64::try_from(int(value)).unwrap()),
            &expected,
        ),
        "timestamp" => both(
            name,
            Timestamp(i64::try_from(int(value)).unwrap()),
            &expected,
        ),
        "uuid" => {
            let id: Uuid = value.as_str().unwrap().parse().unwrap();
            assert_eq!(id.to_string(), value.as_str().unwrap(), "canonical Display");
            both(name, id, &expected);
        }
        "result<i32,string>" => {
            let result: Result<i32, String> = if let Some(ok) = value.get("ok") {
                Ok(i32::try_from(int(ok)).unwrap())
            } else {
                Err(value["err"].as_str().unwrap().to_owned())
            };
            both(name, result, &expected);
        }
        "handle" => {
            let h = Handle(u64::try_from(int(value)).unwrap());
            assert_eq!(
                (h.index(), h.generation()),
                (1, 1),
                "vector note: index 1, generation 1"
            );
            assert_eq!(Handle::new(1, 1), h);
            both(name, h, &expected);
        }
        "envelope" => check_envelope(name, value, &expected),
        "call payload" => check_call(name, value, &expected),
        "reply payload" => check_reply(name, value, &expected),
        "changeset payload" => check_changeset(name, value, &expected),
        "keyed patch (item i32)" => check_keyed_patch(name, value, &expected),
        _ if ty.starts_with("record ") || ty.starts_with("enum ") => {
            check_record_or_enum(name, ty, value, &expected)
        }
        other => panic!(
            "vector {name:?} has type {other:?} which this test does not know; add it or list it in SKIPPED"
        ),
    }
}

#[test]
fn every_vector_round_trips_both_ways() {
    let vectors = load();
    assert!(vectors.len() >= 30, "vectors file looks truncated");

    let mut covered = 0;
    for v in &vectors {
        let name = v["name"].as_str().unwrap();
        if SKIPPED.iter().any(|(skipped, _)| *skipped == name) {
            continue;
        }
        check(v);
        covered += 1;
    }
    assert_eq!(covered + SKIPPED.len(), vectors.len());
}

#[test]
fn skipped_vectors_still_exist() {
    // A stale SKIPPED entry would hide a renamed vector.
    let vectors = load();
    for (name, reason) in SKIPPED {
        assert!(
            vectors.iter().any(|v| v["name"] == *name),
            "SKIPPED lists {name:?} ({reason}) but the file has no such vector"
        );
    }
}

#[test]
fn vector_names_are_unique() {
    let vectors = load();
    let mut names: Vec<&str> = vectors
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(before, names.len());
}
