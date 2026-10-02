//! The [`Encode`] and [`Decode`] traits and their implementations for the value types of
//! SPEC 3.1.

use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasher, Hash};
use std::sync::Arc;
use std::time::Duration;

use crate::types::{Bytes, Decimal, Handle, Timestamp, Uuid};
use crate::writer::len_u32;
use crate::{Reader, WireError, Writer};

/// A type that can be written to the wire.
///
/// Implementations are total: encoding never fails. See [`Writer`] for the one exception (a
/// length above `u32::MAX`).
///
/// # Example
///
/// ```
/// use undra_wire::{Encode, Writer};
///
/// struct Point { x: i32, y: i32 }
///
/// impl Encode for Point {
///     fn encode(&self, w: &mut Writer) {
///         self.x.encode(w);
///         self.y.encode(w);
///     }
/// }
///
/// assert_eq!(Point { x: 1, y: -1 }.encode_to_vec(), [1, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
/// ```
#[diagnostic::on_unimplemented(
    message = "error[undra::E0001]: `{Self}` cannot cross the boundary by value\n  = note: a value crosses as a scalar, `String`, `Bytes`, `Vec`, `Option`, a map, `Duration`, `Timestamp`, `Uuid`, `Decimal`, or a type declared with `#[undra::api]` (records, enums) or `#[undra::error]`; an object (`#[undra::api] impl`) crosses by handle, never as a value\n  = help: declare `{Self}` with `#[undra::api]`, or return a record with the data the platform needs; a type of the `uuid`, `chrono`, `time`, `rust_decimal` or `bytes` crate crosses once you enable that feature of `undra`\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0001",
    label = "not a type that can cross the boundary"
)]
pub trait Encode {
    /// Appends the wire encoding of `self` to `w`.
    fn encode(&self, w: &mut Writer);

    /// Encodes `self` into a freshly allocated buffer.
    fn encode_to_vec(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.encode(&mut w);
        w.into_vec()
    }
}

/// A type that can be read from the wire.
///
/// Decoders must be total: any input yields `Ok` or `Err`, never a panic, an unbounded
/// allocation or unbounded recursion. Use [`Reader::read_count`] to size collections,
/// [`Reader::nested`] around recursive calls, and [`Vec::with_capacity`] with a small cap.
///
/// # Example
///
/// ```
/// use undra_wire::{Decode, Reader, WireError};
///
/// #[derive(Debug, PartialEq)]
/// struct Point { x: i32, y: i32 }
///
/// impl Decode for Point {
///     const MIN_ENCODED_LEN: usize = 8;
///     fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
///         Ok(Point { x: i32::decode(r)?, y: i32::decode(r)? })
///     }
/// }
///
/// assert_eq!(Point::decode_exact(&[1, 0, 0, 0, 2, 0, 0, 0]), Ok(Point { x: 1, y: 2 }));
/// assert!(Point::decode_exact(&[1, 0, 0, 0]).is_err());
/// ```
#[diagnostic::on_unimplemented(
    message = "error[undra::E0001]: `{Self}` cannot cross the boundary by value\n  = note: a value crosses as a scalar, `String`, `Bytes`, `Vec`, `Option`, a map, `Duration`, `Timestamp`, `Uuid`, `Decimal`, or a type declared with `#[undra::api]` (records, enums) or `#[undra::error]`; an object (`#[undra::api] impl`) crosses by handle, never as a value\n  = help: declare `{Self}` with `#[undra::api]`, or return a record with the data the platform needs; a type of the `uuid`, `chrono`, `time`, `rust_decimal` or `bytes` crate crosses once you enable that feature of `undra`\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0001",
    label = "not a type that can cross the boundary"
)]
pub trait Decode: Sized {
    /// A lower bound on the number of bytes any encoding of this type occupies.
    ///
    /// Collection decoders use it to reject a count prefix that cannot possibly fit in the
    /// remaining input *before* allocating for it. The default of `1` is right for every type
    /// that always writes at least one byte; override it (with a value that is never larger
    /// than the true minimum) for types that are larger, or to `0` for types that can encode
    /// to nothing, such as `()` and empty records.
    const MIN_ENCODED_LEN: usize = 1;

    /// Reads one value from `r`.
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError>;

    /// Decodes a value from `bytes`, which must contain exactly one encoding and nothing else.
    fn decode_exact(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes);
        let value = Self::decode(&mut r)?;
        r.finish()?;
        Ok(value)
    }
}

// ---------------------------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------------------------

macro_rules! impl_prim {
    ($($t:ty => $write:ident, $read:ident, $size:expr;)*) => {
        $(
            impl Encode for $t {
                #[inline]
                fn encode(&self, w: &mut Writer) {
                    w.$write(*self);
                }
            }

            impl Decode for $t {
                const MIN_ENCODED_LEN: usize = $size;

                #[inline]
                fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
                    r.$read()
                }
            }
        )*
    };
}

impl_prim! {
    bool => write_bool, read_bool, 1;
    u8 => write_u8, read_u8, 1;
    u16 => write_u16, read_u16, 2;
    u32 => write_u32, read_u32, 4;
    u64 => write_u64, read_u64, 8;
    i8 => write_i8, read_i8, 1;
    i16 => write_i16, read_i16, 2;
    i32 => write_i32, read_i32, 4;
    i64 => write_i64, read_i64, 8;
    f32 => write_f32, read_f32, 4;
    f64 => write_f64, read_f64, 8;
}

impl Encode for () {
    #[inline]
    fn encode(&self, _w: &mut Writer) {}
}

impl Decode for () {
    const MIN_ENCODED_LEN: usize = 0;

    #[inline]
    fn decode(_r: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Strings and bytes
// ---------------------------------------------------------------------------------------------

impl Encode for str {
    #[inline]
    fn encode(&self, w: &mut Writer) {
        w.write_str(self);
    }
}

impl Encode for String {
    #[inline]
    fn encode(&self, w: &mut Writer) {
        w.write_str(self);
    }
}

impl Decode for String {
    const MIN_ENCODED_LEN: usize = 4;

    #[inline]
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        r.read_str().map(str::to_owned)
    }
}

impl Encode for Bytes {
    #[inline]
    fn encode(&self, w: &mut Writer) {
        w.write_bytes(&self.0);
    }
}

impl Decode for Bytes {
    const MIN_ENCODED_LEN: usize = 4;

    #[inline]
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        r.read_bytes().map(|b| Bytes(b.to_vec()))
    }
}

// ---------------------------------------------------------------------------------------------
// References and smart pointers
// ---------------------------------------------------------------------------------------------

impl<T: Encode + ?Sized> Encode for &T {
    #[inline]
    fn encode(&self, w: &mut Writer) {
        (**self).encode(w);
    }
}

impl<T: Encode + ?Sized> Encode for Box<T> {
    #[inline]
    fn encode(&self, w: &mut Writer) {
        (**self).encode(w);
    }
}

impl<T: Decode> Decode for Box<T> {
    const MIN_ENCODED_LEN: usize = T::MIN_ENCODED_LEN;

    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        r.nested(T::decode).map(Box::new)
    }
}

/// `Arc<T>` can be encoded (a shared value is written like the value itself) but not decoded:
/// decode to `T` and wrap it.
impl<T: Encode + ?Sized> Encode for Arc<T> {
    #[inline]
    fn encode(&self, w: &mut Writer) {
        (**self).encode(w);
    }
}

// ---------------------------------------------------------------------------------------------
// Option, Result, tuples
// ---------------------------------------------------------------------------------------------

impl<T: Encode> Encode for Option<T> {
    fn encode(&self, w: &mut Writer) {
        match self {
            None => w.write_u8(0),
            Some(value) => {
                w.write_u8(1);
                value.encode(w);
            }
        }
    }
}

impl<T: Decode> Decode for Option<T> {
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let at = r.position();
        match r.read_u8()? {
            0 => Ok(None),
            1 => T::decode(r).map(Some),
            tag => Err(WireError::InvalidTag {
                tag: u32::from(tag),
                at,
                ty: "Option",
            }),
        }
    }
}

impl<T: Encode, E: Encode> Encode for Result<T, E> {
    fn encode(&self, w: &mut Writer) {
        match self {
            Ok(value) => {
                w.write_u8(0);
                value.encode(w);
            }
            Err(error) => {
                w.write_u8(1);
                error.encode(w);
            }
        }
    }
}

impl<T: Decode, E: Decode> Decode for Result<T, E> {
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let at = r.position();
        match r.read_u8()? {
            0 => T::decode(r).map(Ok),
            1 => E::decode(r).map(Err),
            tag => Err(WireError::InvalidTag {
                tag: u32::from(tag),
                at,
                ty: "Result",
            }),
        }
    }
}

macro_rules! impl_tuple {
    ($( ($($name:ident $idx:tt),+) )+) => {
        $(
            impl<$($name: Encode),+> Encode for ($($name,)+) {
                fn encode(&self, w: &mut Writer) {
                    $( self.$idx.encode(w); )+
                }
            }

            impl<$($name: Decode),+> Decode for ($($name,)+) {
                const MIN_ENCODED_LEN: usize = 0 $( + $name::MIN_ENCODED_LEN )+;

                fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
                    Ok(( $( $name::decode(r)?, )+ ))
                }
            }
        )+
    };
}

impl_tuple! {
    (A 0)
    (A 0, B 1)
    (A 0, B 1, C 2)
    (A 0, B 1, C 2, D 3)
}

// ---------------------------------------------------------------------------------------------
// Sequences and maps
// ---------------------------------------------------------------------------------------------

impl<T: Encode> Encode for [T] {
    fn encode(&self, w: &mut Writer) {
        w.write_len(len_u32(self.len()));
        for item in self {
            item.encode(w);
        }
    }
}

impl<T: Encode> Encode for Vec<T> {
    #[inline]
    fn encode(&self, w: &mut Writer) {
        self.as_slice().encode(w);
    }
}

impl<T: Decode> Decode for Vec<T> {
    const MIN_ENCODED_LEN: usize = 4;

    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let count = r.read_count(T::MIN_ENCODED_LEN)?;
        r.nested(|r| {
            // Never trust the count for the allocation: reserve a small amount and let the
            // vector grow as items actually decode.
            let mut items = Vec::with_capacity(count.min(1024));
            for _ in 0..count {
                items.push(T::decode(r)?);
            }
            Ok(items)
        })
    }
}

/// Writes `len` entries so that the output is independent of the iteration order of the map:
/// entries are ordered by their *encoded key bytes* (SPEC 3.1), then by encoded value bytes
/// as a tie-break for key types whose encoding is not injective.
fn encode_map<'a, K, V>(
    len: usize,
    mut entries: impl Iterator<Item = (&'a K, &'a V)>,
    w: &mut Writer,
) where
    K: Encode + 'a,
    V: Encode + 'a,
{
    w.write_len(len_u32(len));
    if len <= 1 {
        if let Some((k, v)) = entries.next() {
            k.encode(w);
            v.encode(w);
        }
        return;
    }

    // Encode every entry once into scratch space, remember where each key and entry lies,
    // sort the spans and copy them out in order.
    let mut scratch = Writer::new();
    let mut spans: Vec<(usize, usize, usize)> = Vec::with_capacity(len);
    for (k, v) in entries {
        let start = scratch.len();
        k.encode(&mut scratch);
        let key_end = scratch.len();
        v.encode(&mut scratch);
        spans.push((start, key_end, scratch.len()));
    }
    let bytes = scratch.as_slice();
    let slice = |from: usize, to: usize| bytes.get(from..to).unwrap_or(&[]);
    spans.sort_unstable_by(|a, b| {
        slice(a.0, a.1)
            .cmp(slice(b.0, b.1))
            .then_with(|| slice(a.1, a.2).cmp(slice(b.1, b.2)))
    });
    w.reserve(bytes.len());
    for (start, _, end) in spans {
        w.write_raw(slice(start, end));
    }
}

impl<K: Encode, V: Encode, S> Encode for HashMap<K, V, S> {
    fn encode(&self, w: &mut Writer) {
        encode_map(self.len(), self.iter(), w);
    }
}

impl<K, V, S> Decode for HashMap<K, V, S>
where
    K: Decode + Eq + Hash,
    V: Decode,
    S: BuildHasher + Default,
{
    const MIN_ENCODED_LEN: usize = 4;

    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let count = r.read_count(K::MIN_ENCODED_LEN.saturating_add(V::MIN_ENCODED_LEN))?;
        r.nested(|r| {
            let mut map = HashMap::with_capacity_and_hasher(count.min(1024), S::default());
            for _ in 0..count {
                let at = r.position();
                let key = K::decode(r)?;
                let value = V::decode(r)?;
                if map.insert(key, value).is_some() {
                    return Err(WireError::DuplicateKey { at });
                }
            }
            Ok(map)
        })
    }
}

impl<K: Encode, V: Encode> Encode for BTreeMap<K, V> {
    fn encode(&self, w: &mut Writer) {
        encode_map(self.len(), self.iter(), w);
    }
}

impl<K, V> Decode for BTreeMap<K, V>
where
    K: Decode + Ord,
    V: Decode,
{
    const MIN_ENCODED_LEN: usize = 4;

    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let count = r.read_count(K::MIN_ENCODED_LEN.saturating_add(V::MIN_ENCODED_LEN))?;
        r.nested(|r| {
            let mut map = BTreeMap::new();
            for _ in 0..count {
                let at = r.position();
                let key = K::decode(r)?;
                let value = V::decode(r)?;
                if map.insert(key, value).is_some() {
                    return Err(WireError::DuplicateKey { at });
                }
            }
            Ok(map)
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Time, ids
// ---------------------------------------------------------------------------------------------

/// Encoded as `i64` nanoseconds. Durations longer than `i64::MAX` nanoseconds (about 292
/// years) saturate to `i64::MAX`; the wire format cannot represent them.
impl Encode for Duration {
    fn encode(&self, w: &mut Writer) {
        w.write_i64(i64::try_from(self.as_nanos()).unwrap_or(i64::MAX));
    }
}

/// Rejects negative nanosecond counts with [`WireError::NegativeDuration`].
impl Decode for Duration {
    const MIN_ENCODED_LEN: usize = 8;

    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let at = r.position();
        let nanos = r.read_i64()?;
        match u64::try_from(nanos) {
            Ok(n) => Ok(Duration::from_nanos(n)),
            Err(_) => Err(WireError::NegativeDuration { at }),
        }
    }
}

impl Encode for Timestamp {
    #[inline]
    fn encode(&self, w: &mut Writer) {
        w.write_i64(self.0);
    }
}

impl Decode for Timestamp {
    const MIN_ENCODED_LEN: usize = 8;

    #[inline]
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        r.read_i64().map(Timestamp)
    }
}

impl Encode for Uuid {
    #[inline]
    fn encode(&self, w: &mut Writer) {
        w.write_raw(&self.0);
    }
}

impl Decode for Uuid {
    const MIN_ENCODED_LEN: usize = 16;

    #[inline]
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        r.read_array::<16>().map(Uuid)
    }
}

impl Encode for Decimal {
    #[inline]
    fn encode(&self, w: &mut Writer) {
        w.write_raw(&self.mantissa.to_le_bytes());
        w.write_u8(self.scale);
    }
}

impl Decode for Decimal {
    const MIN_ENCODED_LEN: usize = 17;

    #[inline]
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let mantissa = i128::from_le_bytes(r.read_array::<16>()?);
        let at = r.position();
        let scale = r.read_u8()?;
        if scale > Decimal::MAX_SCALE {
            return Err(WireError::InvalidTag {
                tag: u32::from(scale),
                at,
                ty: "decimal scale",
            });
        }
        Ok(Decimal { mantissa, scale })
    }
}

impl Encode for Handle {
    #[inline]
    fn encode(&self, w: &mut Writer) {
        w.write_u64(self.0);
    }
}

impl Decode for Handle {
    const MIN_ENCODED_LEN: usize = 8;

    #[inline]
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        r.read_u64().map(Handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T: Encode + Decode + PartialEq + core::fmt::Debug>(value: T) -> Vec<u8> {
        let bytes = value.encode_to_vec();
        assert_eq!(T::decode_exact(&bytes).as_ref(), Ok(&value));
        bytes
    }

    #[test]
    fn primitive_encodings() {
        assert_eq!(round_trip(true), [1]);
        assert_eq!(round_trip(false), [0]);
        assert_eq!(round_trip(-2_i32), [0xfe, 0xff, 0xff, 0xff]);
        assert_eq!(round_trip(u16::MAX), [0xff, 0xff]);
        assert_eq!(round_trip(()), [] as [u8; 0]);
    }

    #[test]
    fn option_and_result_tags() {
        assert_eq!(round_trip(None::<u8>), [0]);
        assert_eq!(round_trip(Some(7_u8)), [1, 7]);
        assert_eq!(round_trip(Ok::<u8, u8>(7)), [0, 7]);
        assert_eq!(round_trip(Err::<u8, u8>(9)), [1, 9]);
        assert_eq!(round_trip(Some(None::<u8>)), [1, 0]);
    }

    #[test]
    fn tuples() {
        assert_eq!(round_trip((1_u8,)), [1]);
        assert_eq!(round_trip((1_u8, 2_u16)), [1, 2, 0]);
        assert_eq!(round_trip((1_u8, 2_u8, 3_u8, 4_u8)), [1, 2, 3, 4]);
    }

    #[test]
    fn map_encoding_is_sorted_by_encoded_key_bytes() {
        // Encoded: "b" = 01 00 00 00 62, "aa" = 02 00 00 00 61 61. Byte order puts "b" first
        // (its length prefix 01 sorts before 02) even though "aa" < "b" as strings.
        let mut map = BTreeMap::new();
        map.insert("aa".to_owned(), 1_u8);
        map.insert("b".to_owned(), 2_u8);
        let expected = [
            2, 0, 0, 0, // count
            1, 0, 0, 0, b'b', 2, // "b" => 2
            2, 0, 0, 0, b'a', b'a', 1, // "aa" => 1
        ];
        assert_eq!(map.encode_to_vec(), expected);
        let hash: HashMap<String, u8> = map.clone().into_iter().collect();
        assert_eq!(hash.encode_to_vec(), expected);
        assert_eq!(BTreeMap::decode_exact(&expected), Ok(map));
    }

    #[test]
    fn map_encoding_is_independent_of_insertion_order() {
        let entries: Vec<(u32, u32)> = (0..50).map(|i| (i * 7919 % 101, i)).collect();
        let a: HashMap<u32, u32> = entries.iter().copied().collect();
        let b: HashMap<u32, u32> = entries.iter().rev().copied().collect();
        assert_eq!(a.encode_to_vec(), b.encode_to_vec());
    }

    #[test]
    fn duplicate_map_keys_are_rejected() {
        let bytes = [2, 0, 0, 0, 1, 10, 1, 20];
        assert_eq!(
            HashMap::<u8, u8>::decode_exact(&bytes),
            Err(WireError::DuplicateKey { at: 6 })
        );
        assert_eq!(
            BTreeMap::<u8, u8>::decode_exact(&bytes),
            Err(WireError::DuplicateKey { at: 6 })
        );
    }

    #[test]
    fn duration_rejects_negative_and_saturates() {
        let neg = (-1_i64).to_le_bytes();
        assert_eq!(
            Duration::decode_exact(&neg),
            Err(WireError::NegativeDuration { at: 0 })
        );
        assert_eq!(
            round_trip(Duration::from_millis(1500)),
            1_500_000_000_i64.to_le_bytes()
        );
        assert_eq!(
            Duration::MAX.encode_to_vec(),
            i64::MAX.to_le_bytes(),
            "unrepresentable durations saturate"
        );
    }

    #[test]
    fn hostile_vec_count_does_not_allocate() {
        // Claims 4 billion items with 3 bytes of data behind it.
        let bytes = [0xff, 0xff, 0xff, 0xff, 1, 2, 3];
        assert_eq!(
            Vec::<u8>::decode_exact(&bytes),
            Err(WireError::LengthTooLarge {
                len: u32::MAX,
                at: 0
            })
        );
        assert!(Vec::<String>::decode_exact(&bytes).is_err());
        assert!(HashMap::<u32, u32>::decode_exact(&bytes).is_err());
    }

    #[test]
    fn vec_of_unit_round_trips() {
        // Zero-sized items occupy no bytes, so the count is not bounded by the input.
        let bytes = round_trip(vec![(); 100]);
        assert_eq!(bytes, 100_u32.to_le_bytes());
    }

    #[test]
    fn vec_count_is_bounded_by_item_size() {
        // Three u32 items need 12 bytes; only 11 are present.
        let mut bytes = vec![3, 0, 0, 0];
        bytes.extend_from_slice(&[0; 11]);
        assert!(Vec::<u32>::decode_exact(&bytes).is_err());
    }

    #[test]
    fn deep_recursion_is_an_error_not_a_stack_overflow() {
        // Vec<Vec<Vec<...>>> one level deeper than the limit, each level a count of 1.
        fn nest(levels: usize) -> Vec<u8> {
            let mut bytes = Vec::new();
            for _ in 0..levels {
                bytes.extend_from_slice(&1_u32.to_le_bytes());
            }
            bytes.extend_from_slice(&0_u32.to_le_bytes());
            bytes
        }
        #[derive(Debug)]
        struct Tree(#[allow(dead_code)] Vec<Tree>);
        impl Decode for Tree {
            fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
                Vec::<Tree>::decode(r).map(Tree)
            }
        }
        assert!(Tree::decode_exact(&nest(100)).is_ok());
        assert!(matches!(
            Tree::decode_exact(&nest(100_000)),
            Err(WireError::NestingTooDeep { .. })
        ));
    }

    #[test]
    fn box_and_arc_are_transparent() {
        assert_eq!(round_trip(Box::new(5_u16)), [5, 0]);
        assert_eq!(Arc::new(5_u16).encode_to_vec(), [5, 0]);
        assert_eq!(Arc::<str>::from("x").encode_to_vec(), [1, 0, 0, 0, b'x']);
    }

    #[test]
    fn str_and_string_encode_alike() {
        assert_eq!("héllo".encode_to_vec(), "héllo".to_owned().encode_to_vec());
        let quoted: &&str = &"x";
        assert_eq!(quoted.encode_to_vec(), [1, 0, 0, 0, b'x']);
    }

    #[test]
    fn bytes_and_vec_u8_share_a_layout() {
        let v = vec![1_u8, 2, 3];
        assert_eq!(Bytes(v.clone()).encode_to_vec(), v.encode_to_vec());
    }

    #[test]
    fn decode_exact_rejects_trailing_bytes() {
        assert_eq!(
            u8::decode_exact(&[1, 2]),
            Err(WireError::TrailingBytes { count: 1 })
        );
    }
}
