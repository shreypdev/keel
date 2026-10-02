//! The opt-in leaf types (ADR-042): each foreign type encodes exactly as the wire type it
//! stands for and round-trips. Run with `--features uuid,chrono,time,rust_decimal,bytes`.

#![allow(unused_imports)] // each feature uses its own

mod common;

use proptest::prelude::*;
use undra_wire::{Decode, Encode, Timestamp, WireError};

#[cfg(feature = "uuid")]
proptest! {
    #[test]
    fn a_uuid_crate_uuid_is_the_wire_uuid(bytes in any::<[u8; 16]>()) {
        let theirs = uuid::Uuid::from_bytes(bytes);
        let ours = undra_wire::Uuid(bytes);
        prop_assert_eq!(theirs.encode_to_vec(), ours.encode_to_vec());
        prop_assert_eq!(uuid::Uuid::decode_exact(&ours.encode_to_vec()), Ok(theirs));
    }
}

#[cfg(feature = "chrono")]
mod with_chrono {
    use super::*;
    use chrono::{DateTime, TimeDelta, Utc};

    proptest! {
        #[test]
        fn a_datetime_is_a_timestamp_in_milliseconds(ms in -62_135_596_800_000_i64..253_402_300_799_999) {
            let at = DateTime::<Utc>::from_timestamp_millis(ms).unwrap();
            prop_assert_eq!(at.encode_to_vec(), Timestamp(ms).encode_to_vec());
            prop_assert_eq!(DateTime::<Utc>::decode_exact(&Timestamp(ms).encode_to_vec()), Ok(at));
        }

        /// A sub-millisecond value truncates toward negative infinity.
        #[test]
        fn sub_millisecond_precision_floors(ms in -1_000_000_i64..1_000_000, sub in 0_i64..1_000_000) {
            let at = DateTime::<Utc>::from_timestamp_nanos(ms * 1_000_000 + sub);
            prop_assert_eq!(at.encode_to_vec(), Timestamp(ms).encode_to_vec());
        }

        #[test]
        fn a_time_delta_is_a_duration_in_nanoseconds(n in 0_i64..i64::MAX) {
            let d = TimeDelta::nanoseconds(n);
            prop_assert_eq!(d.encode_to_vec(), n.to_le_bytes().to_vec());
            prop_assert_eq!(TimeDelta::decode_exact(&n.to_le_bytes()), Ok(d));
        }
    }

    #[test]
    fn what_a_duration_cannot_hold_is_clamped_and_what_a_datetime_cannot_hold_is_an_error() {
        assert_eq!(
            TimeDelta::nanoseconds(-5).encode_to_vec(),
            0_i64.to_le_bytes()
        );
        assert_eq!(TimeDelta::MAX.encode_to_vec(), i64::MAX.to_le_bytes());
        assert!(matches!(
            DateTime::<Utc>::decode_exact(&Timestamp(i64::MAX).encode_to_vec()),
            Err(WireError::InvalidTag { .. })
        ));
        assert!(matches!(
            TimeDelta::decode_exact(&(-1_i64).to_le_bytes()),
            Err(WireError::NegativeDuration { .. })
        ));
    }
}

#[cfg(feature = "time")]
mod with_time {
    use super::*;
    use time::{OffsetDateTime, UtcDateTime};

    proptest! {
        #[test]
        fn offset_and_utc_datetimes_are_timestamps(ms in -62_135_596_800_000_i64..253_402_300_799_999) {
            let nanos = i128::from(ms) * 1_000_000;
            let offset = OffsetDateTime::from_unix_timestamp_nanos(nanos).unwrap();
            let utc = UtcDateTime::from_unix_timestamp_nanos(nanos).unwrap();
            let wire = Timestamp(ms).encode_to_vec();
            prop_assert_eq!(offset.encode_to_vec(), wire.clone());
            prop_assert_eq!(utc.encode_to_vec(), wire.clone());
            prop_assert_eq!(OffsetDateTime::decode_exact(&wire), Ok(offset));
            prop_assert_eq!(UtcDateTime::decode_exact(&wire), Ok(utc));
        }

        #[test]
        fn a_time_duration_is_a_duration_in_nanoseconds(n in 0_i64..i64::MAX) {
            let d = time::Duration::nanoseconds(n);
            prop_assert_eq!(d.encode_to_vec(), n.to_le_bytes().to_vec());
            prop_assert_eq!(time::Duration::decode_exact(&n.to_le_bytes()), Ok(d));
        }
    }

    #[test]
    fn a_datetime_in_another_offset_is_normalised_to_utc_and_floors() {
        let at = OffsetDateTime::from_unix_timestamp_nanos(-1)
            .unwrap()
            .to_offset(time::UtcOffset::from_hms(5, 30, 0).unwrap());
        assert_eq!(at.encode_to_vec(), Timestamp(-1).encode_to_vec());
        assert_eq!(
            time::Duration::seconds(-1).encode_to_vec(),
            0_i64.to_le_bytes()
        );
    }
}

#[cfg(feature = "rust_decimal")]
mod with_rust_decimal {
    use super::*;

    proptest! {
        #[test]
        fn a_rust_decimal_is_a_wire_decimal(lo in any::<i64>(), hi in 0_i32..i32::MAX, scale in 0_u32..=28, negative in any::<bool>()) {
            let mantissa = (i128::from(hi) << 64 | i128::from(lo as u64)) * if negative { -1 } else { 1 };
            let theirs = rust_decimal::Decimal::from_i128_with_scale(mantissa, scale);
            let wire = undra_wire::Decimal::new(theirs.mantissa(), u8::try_from(theirs.scale()).unwrap());
            prop_assert_eq!(theirs.encode_to_vec(), wire.encode_to_vec());
            prop_assert_eq!(rust_decimal::Decimal::decode_exact(&wire.encode_to_vec()), Ok(theirs));
        }
    }

    #[test]
    fn a_wire_decimal_outside_its_range_is_an_error_not_a_panic() {
        for wire in [
            undra_wire::Decimal::new(i128::MAX, 0),
            undra_wire::Decimal::new(1, 29),
            undra_wire::Decimal::new(i128::MIN, 38),
        ] {
            assert!(matches!(
                rust_decimal::Decimal::decode_exact(&wire.encode_to_vec()),
                Err(WireError::InvalidTag { .. })
            ));
        }
    }
}

#[cfg(feature = "bytes")]
proptest! {
    #[test]
    fn a_bytes_crate_bytes_is_the_wire_bytes(data in proptest::collection::vec(any::<u8>(), 0..64)) {
        let theirs = bytes::Bytes::from(data.clone());
        let ours = undra_wire::Bytes(data);
        prop_assert_eq!(theirs.encode_to_vec(), ours.encode_to_vec());
        prop_assert_eq!(bytes::Bytes::decode_exact(&ours.encode_to_vec()), Ok(theirs));
    }
}
