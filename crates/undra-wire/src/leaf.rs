//! Which types are the Rust side of a wire leaf (ADR-042).
//!
//! The schema describes a position as `Uuid`, `Timestamp`, `Duration`, `Bytes` or `Decimal`;
//! the macros check, in the user's crate, that the type spelled there really is one: it must
//! implement [`WireLeaf`] for the leaf's [`kinds`] marker. The canonical types implement it
//! here, and each optional feature (`uuid`, `chrono`, `time`, `rust_decimal`, `bytes`) adds the
//! foreign type that maps onto the same wire encoding, together with its `Encode` and
//! `Decode`. A type that implements the trait without the wire encoding of the leaf would make
//! the schema lie, which is why the trait is hidden and the impls live only in this crate.

use crate::{Bytes, Decimal, Timestamp, Uuid};

/// The leaf kinds [`WireLeaf`] is parameterised by.
#[doc(hidden)]
pub mod kinds {
    /// `TypeRef::Uuid`.
    pub struct Uuid;
    /// `TypeRef::Timestamp`.
    pub struct Timestamp;
    /// `TypeRef::Duration`.
    pub struct Duration;
    /// `TypeRef::Bytes`.
    pub struct Bytes;
    /// `TypeRef::Decimal`.
    pub struct Decimal;
}

/// Marks a type as the Rust spelling of the wire leaf `K` (a type of [`kinds`]). Not for
/// implementation outside this crate.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "error[undra::E0060]: `{Self}` is spelled like a built-in Undra type, but it is not one\n  = note: the schema records this position as a wire leaf (`Uuid`, `Timestamp`, `Duration`, `Bytes` or `Decimal`), so the platforms would read the bytes of that leaf where the generated code writes `{Self}`\n  = help: rename your type, import the built-in one, or, for a type of the `uuid`, `chrono`, `time`, `rust_decimal` or `bytes` crate, enable that feature of `undra`\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0060",
    label = "not a type that crosses as this leaf"
)]
pub trait WireLeaf<K> {}

impl WireLeaf<kinds::Uuid> for Uuid {}
impl WireLeaf<kinds::Timestamp> for Timestamp {}
impl WireLeaf<kinds::Duration> for core::time::Duration {}
impl WireLeaf<kinds::Bytes> for Bytes {}
impl WireLeaf<kinds::Decimal> for Decimal {}

/// What a `#[undra::api]` newtype (`struct UserId(pub Uuid);`) says about itself: the type it
/// wraps. The macro implements it; [`MapKey`] reads it to decide whether the newtype may be a
/// map key, so a type that is not a newtype fails *this* bound when it is asked to be a key (or
/// the newtype of one): the message is the one of [`MapKey`].
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "error[undra::E0006]: `{Self}` cannot be a map key\n  = note: map keys must be `String`, an integer, `bool`, `Uuid` or a newtype of one of those: they compare and hash identically on every platform (floats, decimals, records and collections do not)\n  = help: use one of those key types, a newtype of one (`struct UserId(pub Uuid);`), or a `Vec` of records with an explicit key field\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0006",
    label = "not a type that can be a map key"
)]
pub trait Newtype {
    /// The wrapped type.
    type Inner: ?Sized;
}

/// Marks a type that may be a map key: `String`, `bool`, the integers, `Uuid` (and, behind the
/// `uuid` feature, `uuid::Uuid`), and a [`Newtype`] of one of those. They compare and hash alike
/// on every platform; floats, decimals, records and collections do not (E0006). The macros assert
/// it at every `HashMap` and `BTreeMap` key position, in the user's crate.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "error[undra::E0006]: `{Self}` cannot be a map key\n  = note: map keys must be `String`, an integer, `bool`, `Uuid` or a newtype of one of those: they compare and hash identically on every platform (floats, decimals, records and collections do not)\n  = help: use one of those key types, a newtype of one (`struct UserId(pub Uuid);`), or a `Vec` of records with an explicit key field\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0006",
    label = "not a type that can be a map key"
)]
pub trait MapKey {}

macro_rules! map_keys {
    ($($ty:ty),* $(,)?) => { $(impl MapKey for $ty {})* };
}

map_keys!(String, bool, i8, i16, i32, i64, u8, u16, u32, u64, Uuid);

/// A newtype is a key exactly when the type it wraps is.
impl<T: Newtype + ?Sized> MapKey for T where T::Inner: MapKey {}

#[cfg(feature = "uuid")]
mod with_uuid {
    use super::{WireLeaf, kinds};
    use crate::{Decode, Encode, Reader, WireError, Writer};

    impl WireLeaf<kinds::Uuid> for uuid::Uuid {}
    impl super::MapKey for uuid::Uuid {}

    impl Encode for uuid::Uuid {
        fn encode(&self, w: &mut Writer) {
            w.write_raw(self.as_bytes());
        }
    }

    impl Decode for uuid::Uuid {
        const MIN_ENCODED_LEN: usize = 16;

        fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
            r.read_array::<16>().map(uuid::Uuid::from_bytes)
        }
    }
}

#[cfg(feature = "chrono")]
mod with_chrono {
    use super::{WireLeaf, kinds};
    use crate::{Decode, Encode, Reader, Timestamp, WireError, Writer};
    use chrono::{DateTime, TimeDelta, Utc};

    impl WireLeaf<kinds::Timestamp> for DateTime<Utc> {}
    impl WireLeaf<kinds::Duration> for TimeDelta {}

    /// Milliseconds since the epoch, truncated toward negative infinity (`Timestamp` has no
    /// finer unit).
    impl Encode for DateTime<Utc> {
        fn encode(&self, w: &mut Writer) {
            Timestamp(self.timestamp_millis()).encode(w);
        }
    }

    impl Decode for DateTime<Utc> {
        const MIN_ENCODED_LEN: usize = 8;

        fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
            let at = r.position();
            let Timestamp(ms) = Timestamp::decode(r)?;
            DateTime::from_timestamp_millis(ms).ok_or(WireError::InvalidTag {
                tag: 0,
                at,
                ty: "chrono::DateTime<Utc> range",
            })
        }
    }

    /// Nanoseconds; a negative `TimeDelta` (a `Duration` is never negative) encodes as zero and
    /// one beyond `i64::MAX` nanoseconds (about 292 years) as the largest.
    impl Encode for TimeDelta {
        fn encode(&self, w: &mut Writer) {
            let nanos = self.num_nanoseconds().unwrap_or(i64::MAX).max(0);
            w.write_i64(nanos);
        }
    }

    impl Decode for TimeDelta {
        const MIN_ENCODED_LEN: usize = 8;

        fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
            core::time::Duration::decode(r).map(|d| {
                // At most `i64::MAX` nanoseconds, which `TimeDelta` holds.
                TimeDelta::nanoseconds(i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
            })
        }
    }
}

#[cfg(feature = "time")]
mod with_time {
    use super::{WireLeaf, kinds};
    use crate::{Decode, Encode, Reader, Timestamp, WireError, Writer};
    use time::{OffsetDateTime, UtcDateTime};

    impl WireLeaf<kinds::Timestamp> for OffsetDateTime {}
    impl WireLeaf<kinds::Timestamp> for UtcDateTime {}
    impl WireLeaf<kinds::Duration> for time::Duration {}

    fn millis(nanos: i128) -> i64 {
        i64::try_from(nanos.div_euclid(1_000_000)).unwrap_or(if nanos < 0 {
            i64::MIN
        } else {
            i64::MAX
        })
    }

    /// Milliseconds since the epoch, in UTC, truncated toward negative infinity.
    impl Encode for OffsetDateTime {
        fn encode(&self, w: &mut Writer) {
            Timestamp(millis(self.unix_timestamp_nanos())).encode(w);
        }
    }

    impl Decode for OffsetDateTime {
        const MIN_ENCODED_LEN: usize = 8;

        fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
            let at = r.position();
            let Timestamp(ms) = Timestamp::decode(r)?;
            OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).map_err(|_| {
                WireError::InvalidTag {
                    tag: 0,
                    at,
                    ty: "time::OffsetDateTime range",
                }
            })
        }
    }

    impl Encode for UtcDateTime {
        fn encode(&self, w: &mut Writer) {
            Timestamp(millis(self.unix_timestamp_nanos())).encode(w);
        }
    }

    impl Decode for UtcDateTime {
        const MIN_ENCODED_LEN: usize = 8;

        fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
            let at = r.position();
            let Timestamp(ms) = Timestamp::decode(r)?;
            UtcDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).map_err(|_| {
                WireError::InvalidTag {
                    tag: 0,
                    at,
                    ty: "time::UtcDateTime range",
                }
            })
        }
    }

    /// Nanoseconds; a negative `time::Duration` encodes as zero, one beyond `i64::MAX`
    /// nanoseconds as the largest.
    impl Encode for time::Duration {
        fn encode(&self, w: &mut Writer) {
            let nanos = i64::try_from(self.whole_nanoseconds())
                .unwrap_or(i64::MAX)
                .max(0);
            w.write_i64(nanos);
        }
    }

    impl Decode for time::Duration {
        const MIN_ENCODED_LEN: usize = 8;

        fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
            core::time::Duration::decode(r).map(|d| {
                time::Duration::nanoseconds(i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
            })
        }
    }
}

#[cfg(feature = "rust_decimal")]
mod with_rust_decimal {
    use super::{WireLeaf, kinds};
    use crate::{Decimal, Decode, Encode, Reader, WireError, Writer};

    impl WireLeaf<kinds::Decimal> for rust_decimal::Decimal {}

    /// A `rust_decimal::Decimal` always encodes: its mantissa has 96 bits and its scale is at
    /// most 28.
    impl Encode for rust_decimal::Decimal {
        fn encode(&self, w: &mut Writer) {
            // `scale()` is at most 28.
            let scale = u8::try_from(self.scale()).unwrap_or(Decimal::MAX_SCALE);
            Decimal {
                mantissa: self.mantissa(),
                scale,
            }
            .encode(w);
        }
    }

    /// A wire decimal outside `rust_decimal`'s range (a mantissa of more than 96 bits, a scale
    /// above 28) is an error, never a panic.
    impl Decode for rust_decimal::Decimal {
        const MIN_ENCODED_LEN: usize = 17;

        fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
            let at = r.position();
            let wire = Decimal::decode(r)?;
            rust_decimal::Decimal::try_from_i128_with_scale(wire.mantissa, u32::from(wire.scale))
                .map_err(|_| WireError::InvalidTag {
                    tag: u32::from(wire.scale),
                    at,
                    ty: "rust_decimal::Decimal range",
                })
        }
    }
}

#[cfg(feature = "bytes")]
mod with_bytes {
    use super::{WireLeaf, kinds};
    use crate::{Decode, Encode, Reader, WireError, Writer};

    impl WireLeaf<kinds::Bytes> for bytes::Bytes {}

    impl Encode for bytes::Bytes {
        fn encode(&self, w: &mut Writer) {
            w.write_bytes(self);
        }
    }

    impl Decode for bytes::Bytes {
        const MIN_ENCODED_LEN: usize = 4;

        fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
            r.read_bytes().map(bytes::Bytes::copy_from_slice)
        }
    }
}
