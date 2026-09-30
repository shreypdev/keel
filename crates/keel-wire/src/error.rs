//! The error type returned by every decoder in this crate.

use core::fmt;

/// Everything that can go wrong while decoding (or validating) wire data.
///
/// Positions (`at`) are byte offsets into the buffer the [`Reader`](crate::Reader) was created
/// over, so they are directly usable when debugging a captured frame.
///
/// The type is `Copy` and allocation free: a decoder that rejects hostile input never
/// allocates to describe the rejection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    /// The input ended before a read completed.
    UnexpectedEof {
        /// How many bytes the failing read asked for.
        needed: usize,
        /// Offset at which the read started.
        at: usize,
    },
    /// A string was not valid UTF-8.
    InvalidUtf8 {
        /// Offset of the first invalid byte.
        at: usize,
    },
    /// A tag byte (`bool`, `Option`, `Result`, enum discriminant, ...) had an unknown value.
    InvalidTag {
        /// The offending tag value.
        tag: u32,
        /// Offset of the tag. Zero when the error comes from a bare `TryFrom<u8>` conversion,
        /// which has no position.
        at: usize,
        /// The type that was being decoded.
        ty: &'static str,
    },
    /// A length or count prefix claims more data than the input can possibly hold.
    LengthTooLarge {
        /// The claimed length or count.
        len: u32,
        /// Offset of the length prefix.
        at: usize,
    },
    /// The value decoded cleanly but bytes were left over.
    TrailingBytes {
        /// Number of unread bytes.
        count: usize,
    },
    /// An envelope did not start with the `KEEL` magic.
    BadMagic,
    /// An envelope carried a protocol version this crate does not speak.
    UnsupportedVersion(u16),
    /// The peer was built from a different schema.
    SchemaMismatch {
        /// The schema hash this side expected.
        expected: u64,
        /// The schema hash the peer announced.
        got: u64,
    },
    /// A map contained the same key twice.
    DuplicateKey {
        /// Offset of the second occurrence of the key.
        at: usize,
    },
    /// A `Duration` was encoded with a negative nanosecond count.
    NegativeDuration {
        /// Offset of the offending `i64`.
        at: usize,
    },
    /// Values were nested deeper than [`MAX_DEPTH`](crate::MAX_DEPTH).
    NestingTooDeep {
        /// Offset at which the limit was hit.
        at: usize,
    },
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            WireError::UnexpectedEof { needed, at } => {
                write!(
                    f,
                    "unexpected end of input at byte {at}: the read needs {needed} byte(s)"
                )
            }
            WireError::InvalidUtf8 { at } => {
                write!(f, "invalid UTF-8 in string at byte {at}")
            }
            WireError::InvalidTag { tag, at, ty } => {
                write!(f, "invalid tag {tag} for {ty} at byte {at}")
            }
            WireError::LengthTooLarge { len, at } => {
                write!(f, "length {len} at byte {at} exceeds the remaining input")
            }
            WireError::TrailingBytes { count } => {
                write!(f, "{count} trailing byte(s) after the decoded value")
            }
            WireError::BadMagic => f.write_str("envelope does not start with the KEEL magic"),
            WireError::UnsupportedVersion(v) => {
                write!(f, "unsupported wire protocol version {v}")
            }
            WireError::SchemaMismatch { expected, got } => {
                write!(
                    f,
                    "schema mismatch: expected hash {expected:#018x}, peer sent {got:#018x}"
                )
            }
            WireError::DuplicateKey { at } => {
                write!(f, "duplicate map key at byte {at}")
            }
            WireError::NegativeDuration { at } => {
                write!(f, "negative duration at byte {at}")
            }
            WireError::NestingTooDeep { at } => {
                write!(
                    f,
                    "values nested deeper than {} levels at byte {at}",
                    crate::MAX_DEPTH
                )
            }
        }
    }
}

impl std::error::Error for WireError {}
