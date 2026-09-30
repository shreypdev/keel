//! Property tests: every `Encode` / `Decode` implementation round-trips, and every encoding
//! has the structural properties the format promises (self-delimiting, deterministic).

mod common;

use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

use proptest::collection::{btree_map, hash_map, vec};
use proptest::prelude::*;
use undra_wire::{Bytes, Decode, Encode, Handle, Reader, Timestamp, Uuid, WireError, Writer};

/// Checks everything that must hold for one value of a type with a canonical encoding:
///
/// * `decode(encode(v))` equals `v` (as judged by `same`), consuming every byte;
/// * re-encoding the decoded value gives identical bytes;
/// * every strict prefix of the encoding is a decode error (the format is self-delimiting);
/// * one trailing byte is reported as `TrailingBytes`;
/// * two values written back to back decode back to back.
fn check_with<T: Encode + Decode + Debug>(
    value: &T,
    same: impl Fn(&T, &T) -> bool,
) -> Result<(), TestCaseError> {
    let bytes = value.encode_to_vec();
    let back = T::decode_exact(&bytes)
        .map_err(|e| TestCaseError::fail(format!("decoding {value:?} failed: {e}")))?;
    prop_assert!(same(value, &back), "{:?} decoded as {:?}", value, back);
    prop_assert_eq!(back.encode_to_vec(), bytes.clone());

    for cut in 0..bytes.len() {
        prop_assert!(
            T::decode_exact(&bytes[..cut]).is_err(),
            "prefix of {} bytes out of {} decoded",
            cut,
            bytes.len()
        );
    }

    let mut longer = bytes.clone();
    longer.push(0);
    prop_assert_eq!(
        T::decode_exact(&longer).err(),
        Some(WireError::TrailingBytes { count: 1 })
    );

    let mut w = Writer::new();
    value.encode(&mut w);
    value.encode(&mut w);
    let mut r = Reader::new(w.as_slice());
    let first = T::decode(&mut r).map_err(|e| TestCaseError::fail(e.to_string()))?;
    let second = T::decode(&mut r).map_err(|e| TestCaseError::fail(e.to_string()))?;
    prop_assert!(same(&first, value) && same(&second, value));
    prop_assert!(r.finish().is_ok());
    Ok(())
}

fn check<T: Encode + Decode + Debug + PartialEq>(value: &T) -> Result<(), TestCaseError> {
    check_with(value, |a, b| a == b)
}

fn uuid() -> impl Strategy<Value = Uuid> {
    any::<[u8; 16]>().prop_map(Uuid)
}

fn timestamp() -> impl Strategy<Value = Timestamp> {
    any::<i64>().prop_map(Timestamp)
}

fn duration() -> impl Strategy<Value = Duration> {
    (0..=i64::MAX as u64).prop_map(Duration::from_nanos)
}

macro_rules! int_round_trips {
    ($($name:ident: $t:ty),* $(,)?) => {
        proptest! {
            $(
                #[test]
                fn $name(v in any::<$t>()) {
                    check(&v)?;
                    prop_assert_eq!(v.encode_to_vec(), v.to_le_bytes().to_vec());
                }
            )*
        }
    };
}

int_round_trips! {
    round_trip_u8: u8, round_trip_u16: u16, round_trip_u32: u32, round_trip_u64: u64,
    round_trip_i8: i8, round_trip_i16: i16, round_trip_i32: i32, round_trip_i64: i64,
}

proptest! {
    #[test]
    fn round_trip_bool(v in any::<bool>()) {
        check(&v)?;
    }

    #[test]
    fn round_trip_unit(_v in Just(())) {
        check(&())?;
        prop_assert!(().encode_to_vec().is_empty());
    }

    // Floats are compared bit for bit so that NaN payloads and signed zeros are covered.
    #[test]
    fn round_trip_f32(bits in any::<u32>()) {
        let v = f32::from_bits(bits);
        check_with(&v, |a, b| a.to_bits() == b.to_bits())?;
        prop_assert_eq!(v.encode_to_vec(), bits.to_le_bytes().to_vec());
    }

    #[test]
    fn round_trip_f64(bits in any::<u64>()) {
        let v = f64::from_bits(bits);
        check_with(&v, |a, b| a.to_bits() == b.to_bits())?;
        prop_assert_eq!(v.encode_to_vec(), bits.to_le_bytes().to_vec());
    }

    #[test]
    fn round_trip_string(s in any::<String>()) {
        check(&s)?;
        // &str and str encode like String.
        prop_assert_eq!(s.as_str().encode_to_vec(), s.encode_to_vec());
        prop_assert_eq!(<&String as Encode>::encode_to_vec(&&s), s.encode_to_vec());
        prop_assert_eq!(Arc::<str>::from(s.as_str()).encode_to_vec(), s.encode_to_vec());
    }

    #[test]
    fn round_trip_bytes(b in vec(any::<u8>(), 0..300)) {
        check(&Bytes(b.clone()))?;
        // Bytes and Vec<u8> share one layout.
        prop_assert_eq!(Bytes(b.clone()).encode_to_vec(), b.encode_to_vec());
        check(&b)?;
    }

    #[test]
    fn round_trip_option(v in any::<Option<String>>(), nested in any::<Option<Option<u8>>>()) {
        check(&v)?;
        check(&nested)?;
    }

    #[test]
    fn round_trip_vec(
        ints in vec(any::<i32>(), 0..50),
        strings in vec(any::<String>(), 0..20),
        deep in vec(vec(vec(any::<u8>(), 0..4), 0..4), 0..4),
        opts in vec(any::<Option<(u8, String)>>(), 0..10),
    ) {
        check(&ints)?;
        check(&strings)?;
        check(&deep)?;
        check(&opts)?;
    }

    #[test]
    fn round_trip_vec_of_zero_sized_items(n in 0_usize..1000) {
        check(&vec![(); n])?;
    }

    #[test]
    fn round_trip_hash_map(m in hash_map(any::<String>(), any::<i32>(), 0..30)) {
        check(&m)?;
        // A BTreeMap with the same content must produce identical bytes.
        let b: BTreeMap<String, i32> = m.clone().into_iter().collect();
        prop_assert_eq!(m.encode_to_vec(), b.encode_to_vec());
    }

    #[test]
    fn round_trip_btree_map(
        m in btree_map(any::<u32>(), any::<String>(), 0..30),
        nested in btree_map(any::<u8>(), vec(any::<Option<i64>>(), 0..4), 0..6),
        by_uuid in hash_map(uuid(), any::<bool>(), 0..10),
    ) {
        check(&m)?;
        check(&nested)?;
        check(&by_uuid)?;
        let h: HashMap<u32, String> = m.clone().into_iter().collect();
        prop_assert_eq!(h.encode_to_vec(), m.encode_to_vec());
    }

    // The map layout is `count` followed by (key, value) pairs, exactly the layout of a
    // Vec of pairs, so decoding a map's bytes as that Vec exposes the entry order.
    #[test]
    fn map_entries_are_sorted_by_encoded_key_bytes(
        m in hash_map(any::<String>(), any::<u8>(), 0..30),
        n in hash_map(any::<i16>(), any::<u8>(), 0..30),
    ) {
        let pairs = Vec::<(String, u8)>::decode_exact(&m.encode_to_vec()).unwrap();
        let keys: Vec<Vec<u8>> = pairs.iter().map(|(k, _)| k.encode_to_vec()).collect();
        prop_assert!(keys.windows(2).all(|w| w[0] < w[1]), "keys not strictly increasing");

        let pairs = Vec::<(i16, u8)>::decode_exact(&n.encode_to_vec()).unwrap();
        let keys: Vec<Vec<u8>> = pairs.iter().map(|(k, _)| k.encode_to_vec()).collect();
        prop_assert!(keys.windows(2).all(|w| w[0] < w[1]), "keys not strictly increasing");
    }

    #[test]
    fn round_trip_duration(d in duration()) {
        check(&d)?;
        prop_assert_eq!(
            d.encode_to_vec(),
            i64::try_from(d.as_nanos()).unwrap().to_le_bytes().to_vec()
        );
    }

    #[test]
    fn duration_decode_accepts_exactly_the_non_negative_range(n in any::<i64>()) {
        let decoded = Duration::decode_exact(&n.to_le_bytes());
        if n >= 0 {
            prop_assert_eq!(decoded, Ok(Duration::from_nanos(n as u64)));
        } else {
            prop_assert_eq!(decoded, Err(WireError::NegativeDuration { at: 0 }));
        }
    }

    #[test]
    fn round_trip_timestamp(ms in any::<i64>()) {
        check(&Timestamp(ms))?;
        prop_assert_eq!(Timestamp(ms).encode_to_vec(), ms.to_le_bytes().to_vec());
    }

    #[test]
    fn round_trip_uuid(u in uuid()) {
        check(&u)?;
        prop_assert_eq!(u.encode_to_vec(), u.as_bytes().to_vec());
        // Display is canonical and FromStr inverts it.
        let text = u.to_string();
        prop_assert_eq!(text.len(), 36);
        prop_assert_eq!(text.clone(), text.to_lowercase());
        prop_assert_eq!(text.parse::<Uuid>(), Ok(u));
        prop_assert_eq!(text.to_uppercase().parse::<Uuid>(), Ok(u));
    }

    #[test]
    fn uuid_parse_never_panics(s in any::<String>()) {
        let _ = s.parse::<Uuid>();
    }

    #[test]
    fn round_trip_result(
        r in any::<Result<i32, String>>(),
        nested in any::<Result<Vec<u8>, Option<String>>>(),
    ) {
        check(&r)?;
        check(&nested)?;
    }

    #[test]
    fn round_trip_tuples(
        t1 in any::<(u8,)>(),
        t2 in any::<(u8, String)>(),
        t3 in any::<(bool, i64, Option<u16>)>(),
        t4 in any::<(String, Vec<u8>, i8, Result<u8, u8>)>(),
    ) {
        check(&t1)?;
        check(&t2)?;
        check(&t3)?;
        check(&t4)?;
        // Tuples are their fields concatenated.
        let mut w = Writer::new();
        t2.0.encode(&mut w);
        t2.1.encode(&mut w);
        prop_assert_eq!(t2.encode_to_vec(), w.into_vec());
    }

    #[test]
    fn round_trip_box_and_arc(v in any::<u16>(), s in any::<String>()) {
        check(&Box::new(v))?;
        check(&Box::new(Box::new(s.clone())))?;
        prop_assert_eq!(Arc::new(v).encode_to_vec(), v.encode_to_vec());
        prop_assert_eq!(Arc::new(s.clone()).encode_to_vec(), s.encode_to_vec());
        prop_assert_eq!(Box::new(s.clone()).encode_to_vec(), s.encode_to_vec());
    }

    #[test]
    fn round_trip_handle(raw in any::<u64>(), index in any::<u32>(), generation in any::<u32>()) {
        check(&Handle(raw))?;
        prop_assert_eq!(Handle(raw).encode_to_vec(), raw.to_le_bytes().to_vec());
        let h = Handle::new(index, generation);
        prop_assert_eq!((h.index(), h.generation()), (index, generation));
        prop_assert_eq!(h.is_null(), index == 0 && generation == 0);
        prop_assert_eq!(Handle::new(Handle(raw).index(), Handle(raw).generation()), Handle(raw));
    }

    #[test]
    fn round_trip_mixed_document(
        doc in hash_map(
            any::<String>(),
            (
                vec(proptest::option::of((timestamp(), uuid())), 0..4),
                any::<Result<Option<u8>, String>>(),
            ),
            0..6,
        )
    ) {
        check(&doc)?;
    }
}
