//! Newtypes for wire types that have no natural Rust primitive: [`Bytes`], [`Timestamp`],
//! [`Uuid`], [`Decimal`] and [`Handle`].

use core::cmp::Ordering;
use core::fmt;
use core::ops::Deref;
use core::str::FromStr;

/// An opaque byte string, encoded as `u32` length + raw bytes.
///
/// `Vec<u8>` also encodes to the same bytes, but through the generic `Vec<T>` impl one element
/// at a time. Use `Bytes` for bulk data.
///
/// # Example
///
/// ```
/// use undra_wire::{Bytes, Encode};
///
/// let b = Bytes::from(vec![1, 2, 3]);
/// assert_eq!(b.len(), 3); // via Deref<Target = [u8]>
/// assert_eq!(b.encode_to_vec(), [3, 0, 0, 0, 1, 2, 3]);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Bytes(pub Vec<u8>);

impl Bytes {
    /// Creates an empty byte string.
    #[inline]
    pub fn new() -> Self {
        Bytes(Vec::new())
    }

    /// Unwraps the underlying vector.
    #[inline]
    pub fn into_vec(self) -> Vec<u8> {
        self.0
    }
}

impl Deref for Bytes {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8]> for Bytes {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for Bytes {
    #[inline]
    fn from(v: Vec<u8>) -> Self {
        Bytes(v)
    }
}

impl From<&[u8]> for Bytes {
    #[inline]
    fn from(v: &[u8]) -> Self {
        Bytes(v.to_vec())
    }
}

impl From<Bytes> for Vec<u8> {
    #[inline]
    fn from(b: Bytes) -> Self {
        b.0
    }
}

/// A point in time as milliseconds since the Unix epoch, encoded as `i64`.
///
/// # Example
///
/// ```
/// use undra_wire::{Encode, Timestamp};
///
/// assert_eq!(Timestamp(1).encode_to_vec(), [1, 0, 0, 0, 0, 0, 0, 0]);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Timestamp(pub i64);

/// A 128-bit universally unique identifier: 16 raw bytes in RFC 4122 (big-endian) order.
///
/// `Display` prints the canonical lowercase hyphenated form and [`FromStr`] parses it.
///
/// # Example
///
/// ```
/// use undra_wire::Uuid;
///
/// let id: Uuid = "123e4567-e89b-12d3-a456-426614174000".parse().unwrap();
/// assert_eq!(id.as_bytes()[0], 0x12);
/// assert_eq!(id.to_string(), "123e4567-e89b-12d3-a456-426614174000");
/// assert!(Uuid::nil().is_nil());
/// ```
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Uuid(pub [u8; 16]);

impl Uuid {
    /// The all-zero UUID.
    #[inline]
    pub const fn nil() -> Self {
        Uuid([0; 16])
    }

    /// Wraps 16 raw bytes.
    #[inline]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Uuid(bytes)
    }

    /// The 16 raw bytes.
    #[inline]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Returns `true` for the all-zero UUID.
    #[inline]
    pub fn is_nil(&self) -> bool {
        self.0 == [0; 16]
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = [0_u8; 36];
        let mut at = 0;
        for (i, byte) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                out[at] = b'-';
                at += 1;
            }
            out[at] = HEX[usize::from(byte >> 4)];
            out[at + 1] = HEX[usize::from(byte & 0x0f)];
            at += 2;
        }
        f.write_str(core::str::from_utf8(&out).map_err(|_| fmt::Error)?)
    }
}

impl fmt::Debug for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Uuid({self})")
    }
}

/// Why a string is not a valid UUID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseUuidError {
    /// The string is not exactly 36 bytes long.
    InvalidLength {
        /// The length of the input in bytes.
        len: usize,
    },
    /// A hyphen is missing or misplaced, or a character is not a hexadecimal digit.
    InvalidCharacter {
        /// Byte offset of the offending character.
        at: usize,
    },
}

impl fmt::Display for ParseUuidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            ParseUuidError::InvalidLength { len } => {
                write!(f, "a UUID is 36 bytes (8-4-4-4-12), got {len}")
            }
            ParseUuidError::InvalidCharacter { at } => {
                write!(f, "invalid character in UUID at byte {at}")
            }
        }
    }
}

impl std::error::Error for ParseUuidError {}

impl FromStr for Uuid {
    type Err = ParseUuidError;

    /// Parses the hyphenated 8-4-4-4-12 form. Hex digits may be upper or lower case.
    fn from_str(s: &str) -> Result<Self, ParseUuidError> {
        fn hex(c: u8) -> Option<u8> {
            match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                b'A'..=b'F' => Some(c - b'A' + 10),
                _ => None,
            }
        }

        let text = s.as_bytes();
        if text.len() != 36 {
            return Err(ParseUuidError::InvalidLength { len: text.len() });
        }
        let mut out = [0_u8; 16];
        let mut nibbles = 0_usize;
        for (at, &c) in text.iter().enumerate() {
            if matches!(at, 8 | 13 | 18 | 23) {
                if c != b'-' {
                    return Err(ParseUuidError::InvalidCharacter { at });
                }
                continue;
            }
            let digit = hex(c).ok_or(ParseUuidError::InvalidCharacter { at })?;
            if let Some(byte) = out.get_mut(nibbles / 2) {
                *byte = if nibbles % 2 == 0 {
                    digit << 4
                } else {
                    *byte | digit
                };
            }
            nibbles += 1;
        }
        Ok(Uuid(out))
    }
}

/// A reference to a runtime object: `u64` with the slot index in the low 24 bits and the
/// generation (starting at 1) in the high 40 bits. `0` is the null handle.
///
/// The split is 24 bits of slot (16.7 million live objects) and 40 bits of generation (1.1 x 10^12
/// issues: 3.5 years at 10,000 a second, ADR-040 decision 8). Hosts never interpret a handle (wasm
/// passes it as two `i32`s, JNI as a `long`), so only the runtime and the snapshot's floor word
/// know the layout.
///
/// Handles are only meaningful inside the runtime instance that issued them (SPEC 1.2).
///
/// # Example
///
/// ```
/// use undra_wire::Handle;
///
/// let h = Handle::new(1, 1);
/// assert_eq!(h.0, 16_777_217);
/// assert_eq!((h.index(), h.generation()), (1, 1));
/// assert!(!h.is_null());
/// assert!(Handle::NULL.is_null());
/// ```
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Handle(pub u64);

impl Handle {
    /// The null handle (`0`), which never refers to an object.
    pub const NULL: Handle = Handle(0);

    /// How many low bits of a handle are the slot index.
    pub const INDEX_BITS: u32 = 24;
    /// The largest slot index a handle can hold (2^24 - 1).
    pub const MAX_INDEX: u32 = (1 << Self::INDEX_BITS) - 1;
    /// The largest generation a handle can hold (2^40 - 1).
    pub const MAX_GENERATION: u64 = (1 << (64 - Self::INDEX_BITS)) - 1;

    /// Builds a handle from a slot index and a generation. Both are masked to their field, so an
    /// out-of-range value cannot alias a neighbouring field: the table refuses to issue one
    /// (`index > MAX_INDEX` is "object table is full", a generation past `MAX_GENERATION` is
    /// exhaustion) before it ever calls this.
    #[inline]
    pub const fn new(index: u32, generation: u64) -> Self {
        Handle(
            ((generation & Self::MAX_GENERATION) << Self::INDEX_BITS)
                | (index & Self::MAX_INDEX) as u64,
        )
    }

    /// The slot index (low 24 bits).
    #[inline]
    pub const fn index(self) -> u32 {
        (self.0 & Self::MAX_INDEX as u64) as u32
    }

    /// The generation (high 40 bits).
    #[inline]
    pub const fn generation(self) -> u64 {
        self.0 >> Self::INDEX_BITS
    }

    /// Returns `true` for the null handle.
    #[inline]
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Handle(index={}, gen={})",
            self.index(),
            self.generation()
        )
    }
}

/// A decimal number: `mantissa x 10^-scale`, exactly (ADR-042).
///
/// On the wire it is the mantissa as 16 little-endian two's-complement bytes (an `i128`), then
/// the scale as one byte, at most [`Decimal::MAX_SCALE`] (38); the decoder rejects a larger one.
/// A decimal is **not normalised**: `1.0` and `1.00` are different values to `==` (and to a
/// `HashMap`), the same number to [`Decimal::cmp_numeric`]. There is no arithmetic and no
/// float round trip: convert through [`Display`](fmt::Display) / [`FromStr`] to the decimal
/// library the app uses, or enable the `rust_decimal` feature.
///
/// # Example
///
/// ```
/// use undra_wire::{Decimal, Encode};
///
/// let price: Decimal = "19.99".parse().unwrap();
/// assert_eq!((price.mantissa, price.scale), (1999, 2));
/// assert_eq!(price.to_string(), "19.99");
/// assert_eq!(Decimal::new(150, 2).to_string(), "1.50");
/// assert_ne!(Decimal::new(10, 1), Decimal::new(100, 2)); // structural equality ...
/// assert!(Decimal::new(10, 1).eq_numeric(&Decimal::new(100, 2))); // ... and numeric
/// assert_eq!(price.encode_to_vec().len(), 17);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Decimal {
    /// The unscaled value.
    pub mantissa: i128,
    /// How many of the mantissa's digits are after the point; at most [`Decimal::MAX_SCALE`].
    pub scale: u8,
}

/// Why a text is not a [`Decimal`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseDecimalError {
    /// The text has no digits.
    Empty,
    /// A character that is not a digit, a sign at the start, or the one decimal point.
    InvalidCharacter {
        /// Byte offset of the character.
        at: usize,
    },
    /// More than 38 digits after the point.
    ScaleTooLarge {
        /// How many digits there are after the point.
        digits: usize,
    },
    /// The digits do not fit a 128-bit signed mantissa.
    OutOfRange,
}

impl fmt::Display for ParseDecimalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            ParseDecimalError::Empty => f.write_str("a decimal needs at least one digit"),
            ParseDecimalError::InvalidCharacter { at } => {
                write!(f, "invalid character in a decimal at byte {at}")
            }
            ParseDecimalError::ScaleTooLarge { digits } => write!(
                f,
                "a decimal has at most {} digits after the point, found {digits}",
                Decimal::MAX_SCALE
            ),
            ParseDecimalError::OutOfRange => {
                f.write_str("the digits do not fit a 128-bit signed mantissa")
            }
        }
    }
}

impl std::error::Error for ParseDecimalError {}

impl Decimal {
    /// The largest scale a decimal can have (the number of digits of `i128::MAX`, less one).
    pub const MAX_SCALE: u8 = 38;

    /// Zero, with scale 0.
    pub const ZERO: Decimal = Decimal {
        mantissa: 0,
        scale: 0,
    };

    /// `mantissa x 10^-scale`.
    ///
    /// # Panics
    ///
    /// If `scale` is more than [`Decimal::MAX_SCALE`]; [`Decimal::try_new`] does not.
    #[must_use]
    pub const fn new(mantissa: i128, scale: u8) -> Decimal {
        assert!(
            scale <= Decimal::MAX_SCALE,
            "a decimal's scale is at most 38"
        );
        Decimal { mantissa, scale }
    }

    /// `mantissa x 10^-scale`, or `None` if `scale` is more than [`Decimal::MAX_SCALE`].
    #[must_use]
    pub const fn try_new(mantissa: i128, scale: u8) -> Option<Decimal> {
        if scale > Decimal::MAX_SCALE {
            None
        } else {
            Some(Decimal { mantissa, scale })
        }
    }

    /// Whether the value is zero, whatever its scale.
    #[must_use]
    pub const fn is_zero(&self) -> bool {
        self.mantissa == 0
    }

    /// Whether the value is below zero.
    #[must_use]
    pub const fn is_negative(&self) -> bool {
        self.mantissa < 0
    }

    /// Compares the numbers, ignoring scale: `1.0` and `1.00` are `Equal`. Exact for every
    /// pair of decimals (no overflow, no rounding).
    #[must_use]
    pub fn cmp_numeric(&self, other: &Decimal) -> Ordering {
        match (self.mantissa.signum(), other.mantissa.signum()) {
            (a, b) if a != b => return a.cmp(&b),
            (0, _) => return Ordering::Equal,
            _ => {}
        }
        let negative = self.mantissa < 0;
        let magnitude = Decimal::cmp_magnitude(self, other);
        if negative {
            magnitude.reverse()
        } else {
            magnitude
        }
    }

    /// Whether the two are the same number, whatever their scales.
    #[must_use]
    pub fn eq_numeric(&self, other: &Decimal) -> bool {
        self.cmp_numeric(other) == Ordering::Equal
    }

    /// Compares `|a|` and `|b|`: the whole parts first, then the fractions on a common scale
    /// (a fraction times at most 10^38 fits a `u128`).
    fn cmp_magnitude(a: &Decimal, b: &Decimal) -> Ordering {
        let (ma, mb) = (a.mantissa.unsigned_abs(), b.mantissa.unsigned_abs());
        let (pa, pb) = (pow10(a.scale), pow10(b.scale));
        let (whole_a, whole_b) = (ma / pa, mb / pb);
        if whole_a != whole_b {
            return whole_a.cmp(&whole_b);
        }
        let common = a.scale.max(b.scale);
        let frac_a = (ma % pa) * pow10(common - a.scale);
        let frac_b = (mb % pb) * pow10(common - b.scale);
        frac_a.cmp(&frac_b)
    }
}

/// `10^n` for `n <= 38`.
fn pow10(n: u8) -> u128 {
    let mut p: u128 = 1;
    for _ in 0..n {
        p *= 10;
    }
    p
}

impl Default for Decimal {
    fn default() -> Decimal {
        Decimal::ZERO
    }
}

impl fmt::Display for Decimal {
    /// The exact decimal text: the scale's digits after the point are all printed (`1.50`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let digits = self.mantissa.unsigned_abs().to_string();
        let scale = usize::from(self.scale);
        let mut text = String::with_capacity(digits.len() + scale + 3);
        if self.mantissa < 0 {
            text.push('-');
        }
        if scale == 0 {
            text.push_str(&digits);
        } else if digits.len() > scale {
            let (whole, frac) = digits.split_at(digits.len() - scale);
            text.push_str(whole);
            text.push('.');
            text.push_str(frac);
        } else {
            text.push_str("0.");
            for _ in digits.len()..scale {
                text.push('0');
            }
            text.push_str(&digits);
        }
        f.write_str(&text)
    }
}

impl fmt::Debug for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Decimal({self})")
    }
}

impl FromStr for Decimal {
    type Err = ParseDecimalError;

    /// `[+-]digits[.digits]`: the scale is the number of digits after the point (kept, so
    /// `"1.50"` has scale 2).
    fn from_str(s: &str) -> Result<Decimal, ParseDecimalError> {
        let bytes = s.as_bytes();
        let (negative, start) = match bytes.first() {
            Some(b'-') => (true, 1),
            Some(b'+') => (false, 1),
            _ => (false, 0),
        };
        let mut magnitude: u128 = 0;
        let mut digits = 0_usize;
        let mut frac_digits: Option<usize> = None;
        for (i, &c) in bytes.iter().enumerate().skip(start) {
            match c {
                b'0'..=b'9' => {
                    digits += 1;
                    if let Some(n) = frac_digits.as_mut() {
                        *n += 1;
                    }
                    magnitude = magnitude
                        .checked_mul(10)
                        .and_then(|m| m.checked_add(u128::from(c - b'0')))
                        .ok_or(ParseDecimalError::OutOfRange)?;
                }
                b'.' if frac_digits.is_none() => frac_digits = Some(0),
                _ => return Err(ParseDecimalError::InvalidCharacter { at: i }),
            }
        }
        if digits == 0 {
            return Err(ParseDecimalError::Empty);
        }
        let scale = frac_digits.unwrap_or(0);
        let Ok(scale) = u8::try_from(scale) else {
            return Err(ParseDecimalError::ScaleTooLarge { digits: scale });
        };
        if scale > Decimal::MAX_SCALE {
            return Err(ParseDecimalError::ScaleTooLarge {
                digits: usize::from(scale),
            });
        }
        let mantissa = if negative {
            if magnitude == i128::MIN.unsigned_abs() {
                i128::MIN
            } else {
                i128::try_from(magnitude)
                    .map(|m| -m)
                    .map_err(|_| ParseDecimalError::OutOfRange)?
            }
        } else {
            i128::try_from(magnitude).map_err(|_| ParseDecimalError::OutOfRange)?
        };
        Ok(Decimal { mantissa, scale })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_packs_index_and_generation() {
        let h = Handle::new(0xde_adbe, 0x12_3456_789a);
        assert_eq!(h.0, 0x1234_5678_9ade_adbe);
        assert_eq!(h.index(), 0xde_adbe);
        assert_eq!(h.generation(), 0x12_3456_789a);
        // A field never spills into its neighbour.
        assert_eq!(Handle::new(u32::MAX, 0).generation(), 0);
        assert_eq!(Handle::new(0, u64::MAX).index(), 0);
        assert_eq!(Handle::new(0, 0), Handle::NULL);
        assert!(!Handle::new(0, 1).is_null());
        assert!(!Handle::new(1, 0).is_null());
    }

    #[test]
    fn uuid_display_is_lowercase_hyphenated() {
        let id = Uuid::from_bytes([
            0x12, 0x3e, 0x45, 0x67, 0xe8, 0x9b, 0x12, 0xd3, 0xa4, 0x56, 0x42, 0x66, 0x14, 0x17,
            0x40, 0x00,
        ]);
        assert_eq!(id.to_string(), "123e4567-e89b-12d3-a456-426614174000");
        assert_eq!(
            Uuid::nil().to_string(),
            "00000000-0000-0000-0000-000000000000"
        );
        assert_eq!(
            Uuid([0xab; 16]).to_string(),
            "abababab-abab-abab-abab-abababababab"
        );
    }

    #[test]
    fn uuid_parse_round_trips_and_accepts_uppercase() {
        let s = "123E4567-E89B-12D3-A456-426614174000";
        let id: Uuid = s.parse().unwrap();
        assert_eq!(id.to_string(), s.to_lowercase());
        assert_eq!(id.to_string().parse::<Uuid>().unwrap(), id);
    }

    #[test]
    fn uuid_parse_rejects_malformed_input_without_panicking() {
        assert_eq!(
            "".parse::<Uuid>(),
            Err(ParseUuidError::InvalidLength { len: 0 })
        );
        assert_eq!(
            "123e4567e89b12d3a456426614174000".parse::<Uuid>(),
            Err(ParseUuidError::InvalidLength { len: 32 })
        );
        // Hyphen where a digit belongs, digit where a hyphen belongs, non-hex digit.
        assert_eq!(
            "123e4567-e89b-12d3-a456-42661417400g".parse::<Uuid>(),
            Err(ParseUuidError::InvalidCharacter { at: 35 })
        );
        assert_eq!(
            "123e45678e89b-12d3-a456-426614174000".parse::<Uuid>(),
            Err(ParseUuidError::InvalidCharacter { at: 8 })
        );
        assert_eq!(
            "123e456-7e89b-12d3-a456-426614174000".parse::<Uuid>(),
            Err(ParseUuidError::InvalidCharacter { at: 7 })
        );
        // 36 bytes but multi-byte UTF-8: must be an error, never a slicing panic.
        let s = "é".repeat(18);
        assert_eq!(s.len(), 36);
        assert!(s.parse::<Uuid>().is_err());
    }

    #[test]
    fn nil_uuid() {
        assert!(Uuid::nil().is_nil());
        assert!(Uuid::default().is_nil());
        assert!(!Uuid([1; 16]).is_nil());
    }

    #[test]
    fn bytes_conversions() {
        let b = Bytes::from(vec![1, 2, 3]);
        assert_eq!(&*b, &[1, 2, 3]);
        assert_eq!(b.as_ref(), &[1, 2, 3]);
        assert_eq!(Bytes::from(&[9_u8][..]), Bytes(vec![9]));
        assert_eq!(Vec::from(b), vec![1, 2, 3]);
        assert!(Bytes::new().is_empty());
    }

    #[test]
    fn a_decimal_prints_and_parses_exactly() {
        for (text, mantissa, scale) in [
            ("0", 0, 0),
            ("1.50", 150, 2),
            ("-1.50", -150, 2),
            ("19.99", 1999, 2),
            ("0.05", 5, 2),
            ("-0.001", -1, 3),
            ("100", 100, 0),
            ("+7.0", 70, 1),
        ] {
            let d: Decimal = text.parse().unwrap();
            assert_eq!((d.mantissa, d.scale), (mantissa, scale), "{text}");
            assert_eq!(d.to_string(), text.trim_start_matches('+'), "{text}");
        }
        let max: Decimal = i128::MAX.to_string().parse().unwrap();
        assert_eq!(max.mantissa, i128::MAX);
        let min: Decimal = i128::MIN.to_string().parse().unwrap();
        assert_eq!(min.mantissa, i128::MIN);
        assert_eq!(min.to_string(), i128::MIN.to_string());
        let tiny = Decimal::new(1, 38);
        assert_eq!(tiny.to_string(), format!("0.{}1", "0".repeat(37)));
        assert_eq!(tiny.to_string().parse::<Decimal>().unwrap(), tiny);
    }

    #[test]
    fn a_decimal_rejects_what_it_cannot_hold() {
        assert_eq!("".parse::<Decimal>(), Err(ParseDecimalError::Empty));
        assert_eq!("-".parse::<Decimal>(), Err(ParseDecimalError::Empty));
        assert_eq!(".".parse::<Decimal>(), Err(ParseDecimalError::Empty));
        assert_eq!(
            "1,5".parse::<Decimal>(),
            Err(ParseDecimalError::InvalidCharacter { at: 1 })
        );
        assert_eq!(
            "1.2.3".parse::<Decimal>(),
            Err(ParseDecimalError::InvalidCharacter { at: 3 })
        );
        assert_eq!(
            "1e5".parse::<Decimal>(),
            Err(ParseDecimalError::InvalidCharacter { at: 1 })
        );
        assert_eq!(
            format!("0.{}", "1".repeat(39)).parse::<Decimal>(),
            Err(ParseDecimalError::ScaleTooLarge { digits: 39 })
        );
        assert_eq!(
            "170141183460469231731687303715884105728".parse::<Decimal>(),
            Err(ParseDecimalError::OutOfRange)
        );
        assert_eq!(
            "340282366920938463463374607431768211456".parse::<Decimal>(),
            Err(ParseDecimalError::OutOfRange)
        );
        assert!(Decimal::try_new(1, 39).is_none());
    }

    #[test]
    fn numeric_order_ignores_scale_and_never_overflows() {
        let d = |m, s| Decimal::new(m, s);
        assert!(d(10, 1).eq_numeric(&d(100, 2)));
        assert_ne!(d(10, 1), d(100, 2));
        assert_eq!(d(1, 0).cmp_numeric(&d(2, 0)), Ordering::Less);
        assert_eq!(d(-1, 0).cmp_numeric(&d(1, 0)), Ordering::Less);
        assert_eq!(d(-2, 0).cmp_numeric(&d(-1, 0)), Ordering::Less);
        assert_eq!(d(0, 5).cmp_numeric(&d(0, 0)), Ordering::Equal);
        assert_eq!(d(5, 1).cmp_numeric(&d(49, 2)), Ordering::Greater);
        assert_eq!(d(-5, 1).cmp_numeric(&d(-49, 2)), Ordering::Less);
        // The extremes that a naive scale alignment would overflow.
        assert_eq!(
            d(i128::MAX, 0).cmp_numeric(&d(i128::MAX, 38)),
            Ordering::Greater
        );
        assert_eq!(
            d(i128::MIN, 38).cmp_numeric(&d(i128::MIN, 0)),
            Ordering::Greater
        );
        assert_eq!(
            d(i128::MAX, 38).cmp_numeric(&d(i128::MAX, 38)),
            Ordering::Equal
        );
        assert_eq!(
            d(i128::MAX, 37).cmp_numeric(&d(i128::MAX, 38)),
            Ordering::Greater
        );
    }
}
