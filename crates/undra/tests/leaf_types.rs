//! The opt-in leaf types (ADR-042), spelled in a core the way a user spells them: a type of
//! `uuid`, `chrono`, `time`, `rust_decimal` or `bytes` in a record, an enum, a map key, a newtype
//! and a function signature. The schema names the wire leaf each stands for; the generated codecs
//! encode the same bytes as the built-in type and round-trip.
//!
//! Needs every feature: `cargo test -p undra --features uuid,chrono,time,rust_decimal,bytes`
//! (the file is empty otherwise, so `cargo test --workspace` leaves the features off).
#![cfg(all(
    feature = "uuid",
    feature = "chrono",
    feature = "time",
    feature = "rust_decimal",
    feature = "bytes"
))]
#![forbid(unsafe_code)]

use std::collections::HashMap;

use chrono::{DateTime, TimeDelta, Utc};
use time::{OffsetDateTime, UtcDateTime};
use undra::meta::{TypeRef, collect_schema};
use undra::wire::{Decimal, Decode, Encode, Timestamp, WireError};

/// An id by the `uuid` crate, spelled `Uuid` and also as a newtype.
#[undra::api]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AccountId(pub uuid::Uuid);

#[undra::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub id: uuid::Uuid,
    pub account: AccountId,
    pub opened: DateTime<Utc>,
    pub closed: Option<chrono::DateTime<chrono::Utc>>,
    pub took: TimeDelta,
    pub seen: OffsetDateTime,
    pub synced: UtcDateTime,
    pub timeout: time::Duration,
    pub amount: rust_decimal::Decimal,
    pub payload: bytes::Bytes,
    pub by_id: HashMap<uuid::Uuid, u8>,
    pub by_account: HashMap<AccountId, Vec<rust_decimal::Decimal>>,
}

#[undra::api]
pub fn latest(since: DateTime<Utc>) -> Option<Event> {
    let _ = since;
    None
}

fn sample() -> Event {
    Event {
        id: uuid::Uuid::from_bytes([7; 16]),
        account: AccountId(uuid::Uuid::from_bytes([9; 16])),
        opened: DateTime::from_timestamp_millis(1_700_000_000_123).unwrap(),
        closed: None,
        took: TimeDelta::milliseconds(2_500),
        seen: OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
        synced: UtcDateTime::from_unix_timestamp(1_700_000_001).unwrap(),
        timeout: time::Duration::seconds(30),
        amount: rust_decimal::Decimal::new(-1999, 2),
        payload: bytes::Bytes::from_static(b"abc"),
        by_id: HashMap::from([(uuid::Uuid::from_bytes([1; 16]), 1)]),
        by_account: HashMap::from([(
            AccountId(uuid::Uuid::from_bytes([2; 16])),
            vec![rust_decimal::Decimal::new(5, 1)],
        )]),
    }
}

#[test]
fn the_foreign_types_round_trip_through_the_generated_codecs() {
    let event = sample();
    let bytes = event.encode_to_vec();
    assert_eq!(Event::decode_exact(&bytes).unwrap(), event);
}

#[test]
fn each_foreign_type_has_the_bytes_of_the_wire_type_it_stands_for() {
    let event = sample();
    // The same record spelled with the built-in leaves, field by field.
    let mut expected = undra::wire::Writer::new();
    undra::wire::Uuid([7; 16]).encode(&mut expected);
    undra::wire::Uuid([9; 16]).encode(&mut expected);
    Timestamp(1_700_000_000_123).encode(&mut expected);
    None::<Timestamp>.encode(&mut expected);
    std::time::Duration::from_millis(2_500).encode(&mut expected);
    Timestamp(1_700_000_000_000).encode(&mut expected);
    Timestamp(1_700_000_001_000).encode(&mut expected);
    std::time::Duration::from_secs(30).encode(&mut expected);
    Decimal::new(-1999, 2).encode(&mut expected);
    undra::wire::Bytes(b"abc".to_vec()).encode(&mut expected);
    HashMap::from([(undra::wire::Uuid([1; 16]), 1_u8)]).encode(&mut expected);
    HashMap::from([(undra::wire::Uuid([2; 16]), vec![Decimal::new(5, 1)])]).encode(&mut expected);
    assert_eq!(event.encode_to_vec(), expected.into_vec());
}

#[test]
fn the_schema_names_the_wire_leaves() {
    let schema = collect_schema("leaf-types-test");
    let event = schema.records.iter().find(|r| r.name == "Event").unwrap();
    let field = |name: &str| {
        &event
            .fields
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("no field {name}"))
            .ty
    };
    assert_eq!(*field("id"), TypeRef::Uuid);
    assert_eq!(*field("account"), TypeRef::Named("AccountId".into()));
    assert_eq!(*field("opened"), TypeRef::Timestamp);
    assert_eq!(*field("closed"), TypeRef::Option(Box::new(TypeRef::Timestamp)));
    assert_eq!(*field("took"), TypeRef::Duration);
    assert_eq!(*field("seen"), TypeRef::Timestamp);
    assert_eq!(*field("synced"), TypeRef::Timestamp);
    assert_eq!(*field("timeout"), TypeRef::Duration);
    assert_eq!(*field("amount"), TypeRef::Decimal);
    assert_eq!(*field("payload"), TypeRef::Bytes);
    assert_eq!(
        *field("by_id"),
        TypeRef::Map(Box::new(TypeRef::Uuid), Box::new(TypeRef::U8))
    );
    let account = schema.records.iter().find(|r| r.name == "AccountId").unwrap();
    assert!(account.transparent);
    assert_eq!(account.fields[0].ty, TypeRef::Uuid);
    let function = schema.functions.iter().find(|f| f.name == "latest").unwrap();
    assert_eq!(function.params[0].ty, TypeRef::Timestamp);
    schema.validate().expect("a schema of foreign leaves is valid");
}

#[test]
fn what_a_foreign_type_cannot_hold_is_a_typed_error_never_a_panic() {
    // Precision: milliseconds, truncated toward negative infinity.
    let before = DateTime::<Utc>::from_timestamp_nanos(-1);
    assert_eq!(before.encode_to_vec(), Timestamp(-1).encode_to_vec());
    // A negative duration encodes as zero.
    assert_eq!(TimeDelta::milliseconds(-5).encode_to_vec(), 0_i64.to_le_bytes());
    assert_eq!(time::Duration::seconds(-1).encode_to_vec(), 0_i64.to_le_bytes());
    // A wire value the foreign type cannot hold is an error.
    assert!(matches!(
        DateTime::<Utc>::decode_exact(&Timestamp(i64::MAX).encode_to_vec()),
        Err(WireError::InvalidTag { .. })
    ));
    let too_wide = Decimal::new(i128::MAX, 0).encode_to_vec();
    assert!(rust_decimal::Decimal::decode_exact(&too_wide).is_err());
    // The built-in `Decimal` still holds it.
    assert_eq!(Decimal::decode_exact(&too_wide).unwrap(), Decimal::new(i128::MAX, 0));
}
