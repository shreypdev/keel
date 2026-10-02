//! Newtypes for wire types that have no natural Rust primitive: [`Bytes`], [`Timestamp`],
//! [`Uuid`] and [`Handle`].

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
}
