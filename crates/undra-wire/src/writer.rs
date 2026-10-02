//! [`Writer`]: the encoding half of the codec.

/// Converts an in-memory length to the `u32` the wire format uses.
///
/// The wire format cannot represent a single string, byte string or collection of more than
/// `u32::MAX` elements. Silently truncating would produce a stream that decodes to something
/// else, so this panics instead. The runtime's dispatch boundary turns the panic into a typed
/// error (status 2); on a 32-bit target such as wasm32 the conversion cannot fail.
#[inline]
#[track_caller]
pub(crate) fn len_u32(len: usize) -> u32 {
    match u32::try_from(len) {
        Ok(v) => v,
        Err(_) => panic!("undra-wire: length {len} does not fit the u32 wire limit"),
    }
}

macro_rules! write_prim {
    ($($name:ident($t:ty);)*) => {
        $(
            #[doc = concat!("Appends a `", stringify!($t), "` in little-endian byte order.")]
            #[inline]
            pub fn $name(&mut self, v: $t) {
                self.buf.extend_from_slice(&v.to_le_bytes());
            }
        )*
    };
}

/// An append-only byte buffer that encoders write into.
///
/// Every integer is little-endian, floats are their IEEE 754 bit patterns, and lengths are
/// `u32`. `Writer` never fails: it grows a `Vec<u8>`.
///
/// # Panics
///
/// [`write_len`](Writer::write_len) cannot panic, but everything that derives a length from a
/// Rust value ([`write_str`](Writer::write_str), [`write_bytes`](Writer::write_bytes) and the
/// `Encode` impls of collections) panics if that length exceeds `u32::MAX`, because the wire
/// format has no way to express it.
///
/// # Example
///
/// ```
/// use undra_wire::Writer;
///
/// let mut w = Writer::new();
/// w.write_u16(0x0102);
/// w.write_str("hi");
/// assert_eq!(w.as_slice(), &[0x02, 0x01, 2, 0, 0, 0, b'h', b'i']);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    /// Creates an empty writer.
    #[inline]
    pub fn new() -> Self {
        Writer { buf: Vec::new() }
    }

    /// Creates an empty writer with room for `capacity` bytes.
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Writer {
            buf: Vec::with_capacity(capacity),
        }
    }

    /// Wraps an existing buffer. New data is appended after its current contents.
    ///
    /// Together with [`into_vec`](Writer::into_vec) and [`clear`](Writer::clear) this lets a
    /// hot path reuse one allocation for many messages.
    #[inline]
    pub fn from_vec(buf: Vec<u8>) -> Self {
        Writer { buf }
    }

    /// Reserves capacity for at least `additional` more bytes.
    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        self.buf.reserve(additional);
    }

    /// Number of bytes written so far.
    #[inline]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Returns `true` if nothing has been written.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Discards the written bytes but keeps the allocation.
    #[inline]
    pub fn clear(&mut self) {
        self.buf.clear();
    }

    /// Discards everything written after the first `len` bytes (nothing if fewer were written):
    /// undoes a partial write.
    ///
    /// ```
    /// let mut w = undra_wire::Writer::new();
    /// w.write_u8(1);
    /// let mark = w.len();
    /// w.write_u32(7);
    /// w.truncate(mark);
    /// assert_eq!(w.as_slice(), [1]);
    /// ```
    #[inline]
    pub fn truncate(&mut self, len: usize) {
        self.buf.truncate(len);
    }

    /// The bytes written so far.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    /// Consumes the writer and returns the buffer.
    #[inline]
    pub fn into_vec(self) -> Vec<u8> {
        self.buf
    }

    write_prim! {
        write_u8(u8);
        write_u16(u16);
        write_u32(u32);
        write_u64(u64);
        write_i8(i8);
        write_i16(i16);
        write_i32(i32);
        write_i64(i64);
        write_f32(f32);
        write_f64(f64);
    }

    /// Appends a `bool` as one byte, `0` or `1`.
    #[inline]
    pub fn write_bool(&mut self, v: bool) {
        self.buf.push(u8::from(v));
    }

    /// Appends a `u32` length or element count.
    #[inline]
    pub fn write_len(&mut self, len: u32) {
        self.write_u32(len);
    }

    /// Appends raw bytes with no length prefix.
    #[inline]
    pub fn write_raw(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Appends a string: `u32` byte length followed by the UTF-8 bytes.
    ///
    /// Reserves the full size once, so it allocates at most once.
    #[inline]
    pub fn write_str(&mut self, s: &str) {
        self.write_bytes(s.as_bytes());
    }

    /// Appends a byte string: `u32` length followed by the bytes.
    ///
    /// Reserves the full size once, so it allocates at most once.
    #[inline]
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        let len = len_u32(bytes.len());
        self.buf.reserve(4 + bytes.len());
        self.buf.extend_from_slice(&len.to_le_bytes());
        self.buf.extend_from_slice(bytes);
    }

    /// Overwrites four bytes at `at` with `value`. Used to back-patch a length once the
    /// data it describes has been written. Does nothing if `at..at + 4` is out of range,
    /// which cannot happen for the offsets this crate computes.
    #[inline]
    pub(crate) fn patch_u32(&mut self, at: usize, value: u32) {
        if let Some(dst) = self.buf.get_mut(at..at.saturating_add(4)) {
            dst.copy_from_slice(&value.to_le_bytes());
        }
    }
}

impl AsRef<[u8]> for Writer {
    fn as_ref(&self) -> &[u8] {
        &self.buf
    }
}

impl From<Writer> for Vec<u8> {
    fn from(w: Writer) -> Vec<u8> {
        w.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_are_little_endian() {
        let mut w = Writer::new();
        w.write_u8(1);
        w.write_u16(0x0203);
        w.write_u32(0x0405_0607);
        w.write_u64(0x0809_0a0b_0c0d_0e0f);
        assert_eq!(
            w.as_slice(),
            &[
                1, 0x03, 0x02, 0x07, 0x06, 0x05, 0x04, 0x0f, 0x0e, 0x0d, 0x0c, 0x0b, 0x0a, 0x09,
                0x08
            ]
        );
    }

    #[test]
    fn negative_integers_are_twos_complement() {
        let mut w = Writer::new();
        w.write_i8(-1);
        w.write_i16(-2);
        w.write_i32(-2);
        w.write_i64(-2);
        assert_eq!(
            w.into_vec(),
            [
                0xff, 0xfe, 0xff, 0xfe, 0xff, 0xff, 0xff, 0xfe, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff
            ]
        );
    }

    #[test]
    fn floats_are_ieee_bits() {
        let mut w = Writer::new();
        w.write_f32(f32::from_bits(0x4048_f5c3)); // 3.14
        w.write_f64(f64::from_bits(0x4005_bf0a_8b14_5769)); // 2.718281828459045
        assert_eq!(
            w.as_slice(),
            &[
                0xc3, 0xf5, 0x48, 0x40, 0x69, 0x57, 0x14, 0x8b, 0x0a, 0xbf, 0x05, 0x40
            ]
        );
    }

    #[test]
    fn strings_and_bytes_are_length_prefixed() {
        let mut w = Writer::new();
        w.write_str("");
        w.write_str("héllo");
        w.write_bytes(&[9, 8]);
        assert_eq!(
            w.as_slice(),
            &[
                0, 0, 0, 0, 6, 0, 0, 0, b'h', 0xc3, 0xa9, b'l', b'l', b'o', 2, 0, 0, 0, 9, 8
            ]
        );
    }

    #[test]
    fn write_str_reserves_once() {
        let mut w = Writer::new();
        w.write_str("0123456789");
        assert!(w.buf.capacity() >= 14);
        assert_eq!(w.len(), 14);
    }

    #[test]
    fn bool_is_one_byte() {
        let mut w = Writer::new();
        w.write_bool(true);
        w.write_bool(false);
        assert_eq!(w.as_slice(), &[1, 0]);
    }

    #[test]
    fn from_vec_appends_and_clear_keeps_capacity() {
        let mut w = Writer::from_vec(vec![7]);
        w.write_u8(8);
        assert_eq!(w.as_slice(), &[7, 8]);
        let cap = w.buf.capacity();
        w.clear();
        assert!(w.is_empty());
        assert_eq!(w.buf.capacity(), cap);
    }

    #[test]
    fn patch_u32_is_bounds_safe() {
        let mut w = Writer::new();
        w.write_u32(0);
        w.patch_u32(0, 0xdead_beef);
        assert_eq!(w.as_slice(), &0xdead_beef_u32.to_le_bytes());
        w.patch_u32(1, 1);
        w.patch_u32(usize::MAX, 1);
        assert_eq!(w.as_slice(), &0xdead_beef_u32.to_le_bytes());
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    #[should_panic(expected = "u32 wire limit")]
    fn oversize_length_panics_instead_of_truncating() {
        let _ = len_u32(u32::MAX as usize + 1);
    }
}
