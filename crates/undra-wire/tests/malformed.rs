//! Explicit malformed-input cases: each one names the exact error the decoder must return,
//! including the offset it reports.

mod common;

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use common::unhex;
use undra_wire::payload::{Call, ChangeSet, ChangeSetRef, Reply, Snapshot};
use undra_wire::{
    Bytes, Decode, Encode, Envelope, KeyedPatch, Kind, Reader, Timestamp, Uuid, WireError,
};

// --- strings ---------------------------------------------------------------------------------

#[test]
fn truncated_string_body() {
    // Claims 5 bytes, has 3: the length prefix is rejected before any byte is read.
    let bytes = unhex("05000000616263");
    assert_eq!(
        String::decode_exact(&bytes),
        Err(WireError::LengthTooLarge { len: 5, at: 0 })
    );
    assert_eq!(
        Reader::new(&bytes).read_str(),
        Err(WireError::LengthTooLarge { len: 5, at: 0 })
    );
}

#[test]
fn truncated_string_length_prefix() {
    for cut in 0..4 {
        assert_eq!(
            String::decode_exact(&unhex("05000000")[..cut]),
            Err(WireError::UnexpectedEof { needed: 4, at: 0 }),
            "cut at {cut}"
        );
    }
}

#[test]
fn invalid_utf8() {
    // 0xff can never appear in UTF-8; it is the second byte here.
    assert_eq!(
        String::decode_exact(&unhex("0300000061ff62")),
        Err(WireError::InvalidUtf8 { at: 5 })
    );
    // A multi-byte sequence cut short by the string's own length (first two bytes of the
    // three-byte U+20AC): the length prefix is honest, the content is not UTF-8.
    assert_eq!(
        String::decode_exact(&unhex("02000000e282")),
        Err(WireError::InvalidUtf8 { at: 4 })
    );
    // Overlong encoding of '/' and a lone surrogate (both invalid).
    assert!(String::decode_exact(&unhex("02000000c0af")).is_err());
    assert!(String::decode_exact(&unhex("03000000eda080")).is_err());
    // The same bytes are fine as Bytes.
    assert_eq!(
        Bytes::decode_exact(&unhex("0300000061ff62")),
        Ok(Bytes(vec![0x61, 0xff, 0x62]))
    );
}

#[test]
fn unicode_strings_round_trip() {
    for s in ["", "é", "日本語", "🌊", "a\0b", "\u{10ffff}"] {
        assert_eq!(
            String::decode_exact(&s.to_owned().encode_to_vec()).as_deref(),
            Ok(s)
        );
    }
}

// --- tags -------------------------------------------------------------------------------------

#[test]
fn bad_bool_tag() {
    for tag in [2_u8, 3, 0x80, 0xff] {
        assert_eq!(
            bool::decode_exact(&[tag]),
            Err(WireError::InvalidTag {
                tag: u32::from(tag),
                at: 0,
                ty: "bool"
            })
        );
    }
    // Inside a composite the offset points at the tag byte.
    assert_eq!(
        <(u8, bool)>::decode_exact(&[7, 9]),
        Err(WireError::InvalidTag {
            tag: 9,
            at: 1,
            ty: "bool"
        })
    );
}

#[test]
fn bad_option_tag() {
    assert_eq!(
        Option::<u8>::decode_exact(&[2, 0]),
        Err(WireError::InvalidTag {
            tag: 2,
            at: 0,
            ty: "Option"
        })
    );
    // Some(<bad bool>) reports the inner tag, at the inner offset.
    assert_eq!(
        Option::<bool>::decode_exact(&[1, 5]),
        Err(WireError::InvalidTag {
            tag: 5,
            at: 1,
            ty: "bool"
        })
    );
    // Some with no value.
    assert_eq!(
        Option::<u8>::decode_exact(&[1]),
        Err(WireError::UnexpectedEof { needed: 1, at: 1 })
    );
}

#[test]
fn bad_result_tag() {
    assert_eq!(
        Result::<u8, u8>::decode_exact(&[2, 0]),
        Err(WireError::InvalidTag {
            tag: 2,
            at: 0,
            ty: "Result"
        })
    );
}

#[test]
fn bad_payload_tags() {
    assert_eq!(
        Call::decode(&mut Reader::new(&[9])),
        Err(WireError::InvalidTag {
            tag: 9,
            at: 0,
            ty: "CallTarget"
        })
    );
    assert_eq!(
        Reply::decode(&mut Reader::new(&[1, 0, 0, 0, 6])),
        Err(WireError::InvalidTag {
            tag: 6,
            at: 4,
            ty: "ReplyStatus"
        })
    );
}

// --- lengths and counts -------------------------------------------------------------------------

#[test]
fn oversize_length() {
    let bytes = unhex("ffffffff0102");
    assert_eq!(
        String::decode_exact(&bytes),
        Err(WireError::LengthTooLarge {
            len: u32::MAX,
            at: 0
        })
    );
    assert_eq!(
        Bytes::decode_exact(&bytes),
        Err(WireError::LengthTooLarge {
            len: u32::MAX,
            at: 0
        })
    );
    assert_eq!(
        Vec::<u8>::decode_exact(&bytes),
        Err(WireError::LengthTooLarge {
            len: u32::MAX,
            at: 0
        })
    );
    // One more than the input can hold is enough to be rejected.
    assert_eq!(
        Vec::<u8>::decode_exact(&unhex("030000000102")),
        Err(WireError::LengthTooLarge { len: 3, at: 0 })
    );
    // Exactly enough is fine.
    assert_eq!(
        Vec::<u8>::decode_exact(&unhex("020000000102")),
        Ok(vec![1, 2])
    );
}

#[test]
fn oversize_count_for_wider_items() {
    // 3 items of 4 bytes each need 12 bytes; 8 are present. The count is rejected even
    // though 3 <= 8 remaining bytes.
    let bytes = unhex("030000000100000002000000");
    assert_eq!(
        Vec::<i32>::decode_exact(&bytes),
        Err(WireError::LengthTooLarge { len: 3, at: 0 })
    );
    assert_eq!(
        HashMap::<u16, u16>::decode_exact(&unhex("0300000001000100")),
        Err(WireError::LengthTooLarge { len: 3, at: 0 })
    );
}

#[test]
fn oversize_count_in_nested_collections() {
    // Vec<Vec<u8>>: the outer count is fine, the inner one is not.
    let bytes = unhex("01000000ffffffff");
    assert_eq!(
        Vec::<Vec<u8>>::decode_exact(&bytes),
        Err(WireError::LengthTooLarge {
            len: u32::MAX,
            at: 4
        })
    );
}

#[test]
fn truncated_primitives() {
    assert_eq!(
        u32::decode_exact(&[1, 2, 3]),
        Err(WireError::UnexpectedEof { needed: 4, at: 0 })
    );
    assert_eq!(
        u64::decode_exact(&[]),
        Err(WireError::UnexpectedEof { needed: 8, at: 0 })
    );
    assert_eq!(
        <(u8, u32)>::decode_exact(&[1, 2, 3]),
        Err(WireError::UnexpectedEof { needed: 4, at: 1 })
    );
    assert_eq!(
        Uuid::decode_exact(&[0; 15]),
        Err(WireError::UnexpectedEof { needed: 16, at: 0 })
    );
    assert_eq!(
        f64::decode_exact(&[0; 7]),
        Err(WireError::UnexpectedEof { needed: 8, at: 0 })
    );
}

// --- trailing bytes ---------------------------------------------------------------------------

#[test]
fn trailing_bytes() {
    assert_eq!(
        u8::decode_exact(&[1, 2]),
        Err(WireError::TrailingBytes { count: 1 })
    );
    assert_eq!(
        String::decode_exact(&unhex("0100000061ffff")),
        Err(WireError::TrailingBytes { count: 2 })
    );
    assert_eq!(
        <()>::decode_exact(&[0]),
        Err(WireError::TrailingBytes { count: 1 })
    );
    // `Reader::finish` is the same check for hand-driven decoding.
    let mut r = Reader::new(&[1, 2, 3]);
    r.read_u8().unwrap();
    assert_eq!(r.finish(), Err(WireError::TrailingBytes { count: 2 }));
}

// --- envelope ---------------------------------------------------------------------------------

fn valid_frame() -> Vec<u8> {
    unhex("4b45454c01000807060504030201010700000003000000aabbcc")
}

#[test]
fn envelope_bad_magic() {
    let mut frame = valid_frame();
    frame[..4].copy_from_slice(b"KEEK");
    assert_eq!(Envelope::parse(&frame), Err(WireError::BadMagic));
    assert_eq!(
        Envelope::parse(&[0x6B, 0x65, 0x65, 0x6C, 0x01, 0x00]),
        Err(WireError::BadMagic)
    );
    assert_eq!(Envelope::parse(&[0; 40]), Err(WireError::BadMagic));
}

#[test]
fn envelope_unsupported_version() {
    for version in [0_u16, 2, 0x0100, u16::MAX] {
        let mut frame = valid_frame();
        frame[4..6].copy_from_slice(&version.to_le_bytes());
        assert_eq!(
            Envelope::parse(&frame),
            Err(WireError::UnsupportedVersion(version))
        );
    }
}

#[test]
fn envelope_unknown_kind() {
    let mut frame = valid_frame();
    frame[14] = 17;
    assert_eq!(
        Envelope::parse(&frame),
        Err(WireError::InvalidTag {
            tag: 17,
            at: 14,
            ty: "Kind"
        })
    );
    assert!(Kind::try_from(0).is_err());
}

#[test]
fn envelope_length_mismatch() {
    let frame = valid_frame();
    // Payload shorter than the header claims.
    assert_eq!(
        Envelope::parse(&frame[..frame.len() - 1]),
        Err(WireError::LengthTooLarge { len: 3, at: 19 })
    );
    // Payload longer than the header claims.
    let mut long = frame.clone();
    long.extend_from_slice(&[1, 2]);
    assert_eq!(
        Envelope::parse(&long),
        Err(WireError::TrailingBytes { count: 2 })
    );
    // Header claims 4 GiB.
    let mut huge = frame;
    huge[19..23].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        Envelope::parse(&huge),
        Err(WireError::LengthTooLarge {
            len: u32::MAX,
            at: 19
        })
    );
}

#[test]
fn envelope_truncated_header() {
    let frame = valid_frame();
    for cut in 0..Envelope::HEADER_LEN {
        assert!(
            matches!(
                Envelope::parse(&frame[..cut]),
                Err(WireError::UnexpectedEof { .. })
            ),
            "cut at {cut}"
        );
    }
}

#[test]
fn envelope_schema_mismatch_is_reported_by_check_schema() {
    let frame = valid_frame();
    let env = Envelope::parse(&frame).unwrap();
    assert_eq!(env.schema, 0x0102_0304_0506_0708);
    assert_eq!(
        env.check_schema(1),
        Err(WireError::SchemaMismatch {
            expected: 1,
            got: 0x0102_0304_0506_0708
        })
    );
}

// --- durations, maps ------------------------------------------------------------------------------

#[test]
fn negative_duration() {
    for nanos in [-1_i64, i64::MIN, -1_500_000_000] {
        assert_eq!(
            Duration::decode_exact(&nanos.to_le_bytes()),
            Err(WireError::NegativeDuration { at: 0 })
        );
    }
    // Inside a composite the offset is that of the i64.
    assert_eq!(
        <(u8, Duration)>::decode_exact(&{
            let mut b = vec![7];
            b.extend_from_slice(&(-1_i64).to_le_bytes());
            b
        }),
        Err(WireError::NegativeDuration { at: 1 })
    );
    // Zero and the maximum are fine; negative timestamps are fine too (they precede 1970).
    assert_eq!(
        Duration::decode_exact(&0_i64.to_le_bytes()),
        Ok(Duration::ZERO)
    );
    assert!(Duration::decode_exact(&i64::MAX.to_le_bytes()).is_ok());
    assert_eq!(
        Timestamp::decode_exact(&(-1_i64).to_le_bytes()),
        Ok(Timestamp(-1))
    );
}

#[test]
fn duplicate_map_key() {
    // {"a": 1, "a": 2}: count, then two 9-byte entries; the second key starts at byte 13.
    let mut w = undra_wire::Writer::new();
    w.write_len(2);
    for value in [1_i32, 2] {
        "a".encode(&mut w);
        value.encode(&mut w);
    }
    let bytes = w.as_slice();
    assert_eq!(bytes.len(), 4 + 9 + 9);
    assert_eq!(
        HashMap::<String, i32>::decode_exact(bytes),
        Err(WireError::DuplicateKey { at: 13 })
    );
    assert_eq!(
        BTreeMap::<String, i32>::decode_exact(bytes),
        Err(WireError::DuplicateKey { at: 13 })
    );
    // The duplicate is detected even when the copies are not adjacent.
    let mut w = undra_wire::Writer::new();
    w.write_len(3);
    for (k, v) in [(1_u8, 1_u8), (2, 2), (1, 3)] {
        k.encode(&mut w);
        v.encode(&mut w);
    }
    assert_eq!(
        HashMap::<u8, u8>::decode_exact(w.as_slice()),
        Err(WireError::DuplicateKey { at: 8 })
    );
}

#[test]
fn map_order_on_the_wire_is_not_enforced_but_duplicates_are() {
    // Keys out of order decode fine (other implementations may not sort); the encoder
    // always sorts.
    let mut w = undra_wire::Writer::new();
    w.write_len(2);
    for k in [2_u32, 1] {
        k.encode(&mut w);
        0_u32.encode(&mut w);
    }
    let decoded = BTreeMap::<u32, u32>::decode_exact(w.as_slice()).unwrap();
    assert_eq!(decoded, BTreeMap::from([(1, 0), (2, 0)]));
    assert_ne!(decoded.encode_to_vec(), w.as_slice(), "re-encoding sorts");
}

// --- other payloads ---------------------------------------------------------------------------------

#[test]
fn changeset_and_snapshot_reject_malformed_input() {
    // Entry count claims 1 but there are no entries.
    let mut bytes = 9_u64.to_le_bytes().to_vec();
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    assert_eq!(
        ChangeSet::decode(&mut Reader::new(&bytes)),
        Err(WireError::LengthTooLarge { len: 1, at: 8 })
    );
    assert_eq!(
        ChangeSetRef::decode(&mut Reader::new(&bytes)).unwrap_err(),
        WireError::LengthTooLarge { len: 1, at: 8 }
    );
    // Snapshot with one store and nothing else.
    assert_eq!(
        Snapshot::decode(&mut Reader::new(&[1, 0, 0, 0, 0, 0, 0, 0])),
        Err(WireError::LengthTooLarge { len: 1, at: 0 })
    );
    // A snapshot that ends where its generation floor should be (the pre-ADR-022 layout).
    assert!(matches!(
        Snapshot::decode(&mut Reader::new(&[0, 0, 0, 0])),
        Err(WireError::UnexpectedEof { .. })
    ));
}

#[test]
fn keyed_patch_rejects_malformed_input() {
    assert_eq!(
        KeyedPatch::<u8>::decode(&mut Reader::new(&[1, 0, 0, 0, 9])),
        Err(WireError::InvalidTag {
            tag: 9,
            at: 4,
            ty: "PatchOp"
        })
    );
    // Insert needs an index and an item.
    assert_eq!(
        KeyedPatch::<u8>::decode(&mut Reader::new(&[1, 0, 0, 0, 0, 1, 0, 0, 0])),
        Err(WireError::UnexpectedEof { needed: 1, at: 9 })
    );
    // 100 ops claimed, 1 byte present.
    assert_eq!(
        KeyedPatch::<u8>::decode(&mut Reader::new(&[100, 0, 0, 0, 4])),
        Err(WireError::LengthTooLarge { len: 100, at: 0 })
    );
}

#[test]
fn errors_display_usefully() {
    let messages = [
        WireError::UnexpectedEof { needed: 4, at: 10 }.to_string(),
        WireError::InvalidUtf8 { at: 3 }.to_string(),
        WireError::InvalidTag {
            tag: 7,
            at: 2,
            ty: "bool",
        }
        .to_string(),
        WireError::LengthTooLarge { len: 99, at: 0 }.to_string(),
        WireError::TrailingBytes { count: 2 }.to_string(),
        WireError::BadMagic.to_string(),
        WireError::UnsupportedVersion(9).to_string(),
        WireError::SchemaMismatch {
            expected: 1,
            got: 2,
        }
        .to_string(),
        WireError::DuplicateKey { at: 5 }.to_string(),
        WireError::NegativeDuration { at: 6 }.to_string(),
        WireError::NestingTooDeep { at: 7 }.to_string(),
    ];
    for m in &messages {
        assert!(!m.is_empty());
        assert!(!m.chars().next().unwrap().is_uppercase(), "{m}");
        assert!(!m.ends_with('.'), "{m}");
    }
    assert!(messages[2].contains("bool") && messages[2].contains('7'));
    // `WireError` is a real error type.
    let boxed: Box<dyn std::error::Error> = Box::new(WireError::BadMagic);
    assert!(boxed.to_string().contains("4b45454c"));
}
