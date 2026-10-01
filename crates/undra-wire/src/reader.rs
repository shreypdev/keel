//! [`Reader`]: the decoding half of the codec.

use crate::WireError;

/// Maximum nesting depth of values that can contain themselves (`Box`, `Vec`, maps).
///
/// Recursive schema types (a tree whose nodes hold `Vec<Node>`) would otherwise let a few
/// kilobytes of hostile input overflow the stack, which aborts the process instead of
/// returning an error. Exceeding the limit yields [`WireError::NestingTooDeep`].
pub const MAX_DEPTH: u32 = 128;

/// Largest element count accepted for a collection whose elements may occupy zero bytes
/// (for example `Vec<()>`). Such a count cannot be bounded by the remaining input, so it is
/// bounded by this constant instead.
pub const MAX_ZERO_SIZED_COUNT: u32 = 65_536;

macro_rules! read_prim {
    ($($name:ident -> $t:ty;)*) => {
        $(
            #[doc = concat!("Reads a `", stringify!($t), "` in little-endian byte order.")]
            #[inline]
            pub fn $name(&mut self) -> Result<$t, WireError> {
                self.read_array().map(<$t>::from_le_bytes)
            }
        )*
    };
}

/// A cursor over a borrowed byte slice.
///
/// Every read is bounds checked and returns a [`WireError`] instead of panicking. Strings
/// and byte strings are returned as slices of the input (`&'a str`, `&'a [u8]`), so reading
/// them does not allocate.
///
/// After a read returns an error the position is unspecified; stop decoding.
///
/// # Example
///
/// ```
/// use undra_wire::{Reader, WireError};
///
/// let bytes = [1, 0, 0, 0, b'x', 7];
/// let mut r = Reader::new(&bytes);
/// assert_eq!(r.read_str().unwrap(), "x");
/// assert_eq!(r.read_u8().unwrap(), 7);
/// assert_eq!(r.read_u8(), Err(WireError::UnexpectedEof { needed: 1, at: 6 }));
/// ```
#[derive(Clone, Debug)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    depth: u32,
}

impl<'a> Reader<'a> {
    /// Creates a reader positioned at the start of `buf`.
    #[inline]
    pub fn new(buf: &'a [u8]) -> Self {
        Reader {
            buf,
            pos: 0,
            depth: 0,
        }
    }

    /// Number of bytes not yet read.
    #[inline]
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    /// Offset of the next byte to read, counted from the start of the buffer.
    #[inline]
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Succeeds if every byte has been consumed, otherwise returns
    /// [`WireError::TrailingBytes`].
    #[inline]
    pub fn finish(&self) -> Result<(), WireError> {
        match self.remaining() {
            0 => Ok(()),
            count => Err(WireError::TrailingBytes { count }),
        }
    }

    /// The unread tail of the buffer.
    #[inline]
    fn rest(&self) -> &'a [u8] {
        self.buf.get(self.pos..).unwrap_or(&[])
    }

    /// Consumes and returns the next `n` bytes.
    #[inline]
    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        match self.rest().get(..n) {
            Some(head) => {
                self.pos += n;
                Ok(head)
            }
            None => Err(WireError::UnexpectedEof {
                needed: n,
                at: self.pos,
            }),
        }
    }

    /// Reads exactly `N` bytes as an array, with no length prefix.
    #[inline]
    pub fn read_array<const N: usize>(&mut self) -> Result<[u8; N], WireError> {
        match self.rest().first_chunk::<N>() {
            Some(chunk) => {
                self.pos += N;
                Ok(*chunk)
            }
            None => Err(WireError::UnexpectedEof {
                needed: N,
                at: self.pos,
            }),
        }
    }

    /// The bytes consumed since offset `start` (crate-internal, for zero-copy payloads).
    #[inline]
    pub(crate) fn consumed_since(&self, start: usize) -> &'a [u8] {
        self.buf.get(start..self.pos).unwrap_or(&[])
    }

    read_prim! {
        read_u8 -> u8;
        read_u16 -> u16;
        read_u32 -> u32;
        read_u64 -> u64;
        read_i8 -> i8;
        read_i16 -> i16;
        read_i32 -> i32;
        read_i64 -> i64;
        read_f32 -> f32;
        read_f64 -> f64;
    }

    /// Reads a `bool`. Only the bytes `0` and `1` are accepted.
    #[inline]
    pub fn read_bool(&mut self) -> Result<bool, WireError> {
        let at = self.pos;
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            tag => Err(WireError::InvalidTag {
                tag: u32::from(tag),
                at,
                ty: "bool",
            }),
        }
    }

    /// Reads a `u32` length or element count and checks it against the remaining input.
    ///
    /// The count is rejected with [`WireError::LengthTooLarge`] if it exceeds
    /// [`remaining`](Reader::remaining): every element occupies at least one byte, so a larger
    /// count cannot be satisfied and must not be used to size an allocation. Use
    /// [`read_count`](Reader::read_count) when elements can be larger or smaller than one byte.
    #[inline]
    pub fn read_len(&mut self) -> Result<usize, WireError> {
        self.read_count(1)
    }

    /// Reads a `u32` element count for elements that each occupy at least `min_item_len`
    /// bytes, and checks that `count * min_item_len` fits in the remaining input.
    ///
    /// With `min_item_len == 0` the input cannot bound the count, so it is bounded by
    /// [`MAX_ZERO_SIZED_COUNT`] instead.
    pub fn read_count(&mut self, min_item_len: usize) -> Result<usize, WireError> {
        let at = self.pos;
        let len = self.read_u32()?;
        let too_large = WireError::LengthTooLarge { len, at };
        let count = usize::try_from(len).map_err(|_| too_large)?;
        let fits = if min_item_len == 0 {
            len <= MAX_ZERO_SIZED_COUNT
        } else {
            count
                .checked_mul(min_item_len)
                .is_some_and(|bytes| bytes <= self.remaining())
        };
        if fits { Ok(count) } else { Err(too_large) }
    }

    /// Reads a string: `u32` byte length, then that many UTF-8 bytes. Borrows from the input.
    ///
    /// A length larger than the remaining input is [`WireError::LengthTooLarge`]; malformed
    /// UTF-8 is [`WireError::InvalidUtf8`].
    pub fn read_str(&mut self) -> Result<&'a str, WireError> {
        let len = self.read_len()?;
        let start = self.pos;
        let bytes = self.take(len)?;
        core::str::from_utf8(bytes).map_err(|e| WireError::InvalidUtf8 {
            at: start + e.valid_up_to(),
        })
    }

    /// Reads a byte string: `u32` length, then that many bytes. Borrows from the input.
    pub fn read_bytes(&mut self) -> Result<&'a [u8], WireError> {
        let len = self.read_len()?;
        self.take(len)
    }

    /// Reads exactly `n` bytes with no length prefix. Borrows from the input.
    #[inline]
    pub fn read_raw(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        self.take(n)
    }

    /// Consumes and returns everything that has not been read yet. Never fails.
    #[inline]
    pub fn read_rest(&mut self) -> &'a [u8] {
        let rest = self.rest();
        self.pos = self.buf.len();
        rest
    }

    /// Runs `f` one nesting level deeper, failing with [`WireError::NestingTooDeep`] beyond
    /// [`MAX_DEPTH`].
    ///
    /// Hand-written `Decode` impls for recursive types should wrap the recursive call in this;
    /// the impls for `Box`, `Vec` and the maps already do.
    pub fn nested<T>(
        &mut self,
        f: impl FnOnce(&mut Reader<'a>) -> Result<T, WireError>,
    ) -> Result<T, WireError> {
        if self.depth >= MAX_DEPTH {
            return Err(WireError::NestingTooDeep { at: self.pos });
        }
        self.depth += 1;
        let result = f(self);
        self.depth -= 1;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian_integers() {
        let bytes = [
            1, 0x03, 0x02, 0x07, 0x06, 0x05, 0x04, 0xfe, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        ];
        let mut r = Reader::new(&bytes);
        assert_eq!(r.read_u8().unwrap(), 1);
        assert_eq!(r.read_u16().unwrap(), 0x0203);
        assert_eq!(r.read_u32().unwrap(), 0x0405_0607);
        assert_eq!(r.read_i64().unwrap(), -2);
        assert_eq!(r.remaining(), 0);
        assert!(r.finish().is_ok());
    }

    #[test]
    fn eof_reports_needed_and_position() {
        let mut r = Reader::new(&[1, 2, 3]);
        r.read_u8().unwrap();
        assert_eq!(
            r.read_u32(),
            Err(WireError::UnexpectedEof { needed: 4, at: 1 })
        );
    }

    #[test]
    fn bool_rejects_other_values() {
        let mut r = Reader::new(&[0, 1, 2]);
        assert!(!r.read_bool().unwrap());
        assert!(r.read_bool().unwrap());
        assert_eq!(
            r.read_bool(),
            Err(WireError::InvalidTag {
                tag: 2,
                at: 2,
                ty: "bool"
            })
        );
    }

    #[test]
    fn read_str_borrows_from_input() {
        let bytes = [2, 0, 0, 0, b'o', b'k'];
        let mut r = Reader::new(&bytes);
        let s = r.read_str().unwrap();
        assert_eq!(s, "ok");
        assert_eq!(s.as_ptr(), bytes[4..].as_ptr());
    }

    #[test]
    fn read_str_length_beyond_input_is_rejected_before_reading() {
        let bytes = [0xff, 0xff, 0xff, 0xff, b'x'];
        assert_eq!(
            Reader::new(&bytes).read_str(),
            Err(WireError::LengthTooLarge {
                len: u32::MAX,
                at: 0
            })
        );
    }

    #[test]
    fn read_str_reports_offset_of_invalid_byte() {
        let bytes = [3, 0, 0, 0, b'a', 0xff, b'b'];
        assert_eq!(
            Reader::new(&bytes).read_str(),
            Err(WireError::InvalidUtf8 { at: 5 })
        );
    }

    #[test]
    fn read_len_rejects_counts_that_cannot_fit() {
        let bytes = [3, 0, 0, 0, 1, 2];
        assert_eq!(
            Reader::new(&bytes).read_len(),
            Err(WireError::LengthTooLarge { len: 3, at: 0 })
        );
        let bytes = [2, 0, 0, 0, 1, 2];
        assert_eq!(Reader::new(&bytes).read_len(), Ok(2));
    }

    #[test]
    fn read_count_scales_with_item_size() {
        // 3 items of at least 4 bytes need 12 bytes.
        let mut bytes = vec![3, 0, 0, 0];
        bytes.extend_from_slice(&[0; 11]);
        assert!(Reader::new(&bytes).read_count(4).is_err());
        bytes.push(0);
        assert_eq!(Reader::new(&bytes).read_count(4), Ok(3));
    }

    #[test]
    fn read_count_multiplication_cannot_overflow() {
        let bytes = [0xff, 0xff, 0xff, 0xff];
        assert!(Reader::new(&bytes).read_count(usize::MAX).is_err());
    }

    #[test]
    fn zero_sized_elements_are_bounded_by_constant() {
        let ok = MAX_ZERO_SIZED_COUNT.to_le_bytes();
        assert_eq!(
            Reader::new(&ok).read_count(0),
            Ok(MAX_ZERO_SIZED_COUNT as usize)
        );
        let too_many = (MAX_ZERO_SIZED_COUNT + 1).to_le_bytes();
        assert!(Reader::new(&too_many).read_count(0).is_err());
    }

    #[test]
    fn finish_reports_trailing_bytes() {
        let mut r = Reader::new(&[1, 2, 3]);
        r.read_u8().unwrap();
        assert_eq!(r.finish(), Err(WireError::TrailingBytes { count: 2 }));
    }

    #[test]
    fn read_rest_consumes_everything() {
        let mut r = Reader::new(&[1, 2, 3]);
        r.read_u8().unwrap();
        assert_eq!(r.read_rest(), &[2, 3]);
        assert_eq!(r.read_rest(), &[] as &[u8]);
        assert_eq!(r.position(), 3);
    }

    #[test]
    fn nesting_is_limited() {
        fn recurse(r: &mut Reader<'_>, n: u32) -> Result<(), WireError> {
            if n == 0 {
                return Ok(());
            }
            r.nested(|r| recurse(r, n - 1))
        }
        let mut r = Reader::new(&[]);
        assert_eq!(recurse(&mut r, MAX_DEPTH), Ok(()));
        assert_eq!(
            recurse(&mut r, MAX_DEPTH + 1),
            Err(WireError::NestingTooDeep { at: 0 })
        );
        // The depth counter is restored after an error, so the reader stays usable.
        assert_eq!(recurse(&mut r, MAX_DEPTH), Ok(()));
    }
}
