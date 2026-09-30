//! [`KeelBuf`]: the byte buffer the core hands to its host (SPEC 6).

use core::ptr;

/// A byte buffer owned by the core (SPEC 6).
///
/// Every function that returns a `KeelBuf` (`keel_schema_json`, `keel_call_sync`,
/// `keel_snapshot`, `keel_stats_json`) hands ownership to the caller, who reads `len` bytes at
/// `ptr` and then releases it with `keel_buf_free`. The layout is `#[repr(C)]` and, on
/// `wasm32`, is exactly the `{ ptr i32, len i32, cap i32 }` struct of SPEC 7.
///
/// An empty buffer is `{ NULL, 0, 0 }`; freeing it is a no-op. `cap` is the capacity of the Rust
/// allocation behind `ptr` and only the core may interpret it: a host must never change any
/// field of a buffer it got from the core.
///
/// The one place a *host* fills a `KeelBuf` is the `out_reply` argument of a synchronous port
/// callback; see `KeelPortCb` in the `native` module for that memory rule: a `malloc`ed block
/// with `len` set, which the core releases with `free`. `cap` is reserved there (set it to `0`)
/// and ignored, so a host cannot turn it into an allocator mix-up.
#[repr(C)]
#[derive(Debug, PartialEq, Eq)]
pub struct KeelBuf {
    /// First byte, or null for an empty buffer.
    pub ptr: *mut u8,
    /// Number of valid bytes.
    pub len: u32,
    /// Capacity of the allocation behind `ptr` (`0` when there is none).
    pub cap: u32,
}

impl KeelBuf {
    /// The empty buffer: no allocation, nothing to free.
    pub const EMPTY: KeelBuf = KeelBuf {
        ptr: ptr::null_mut(),
        len: 0,
        cap: 0,
    };

    /// Moves `bytes` into a `KeelBuf`; the allocation now belongs to the buffer until
    /// [`KeelBuf::free`] (or `keel_buf_free`) reclaims it.
    ///
    /// A vector whose length or capacity does not fit in `u32` cannot be described by the C
    /// struct (the wire format has `u32` lengths, so no valid payload is that large); it is
    /// dropped and the result is [`KeelBuf::EMPTY`].
    pub fn from_vec(mut bytes: Vec<u8>) -> KeelBuf {
        if bytes.capacity() == 0 {
            return KeelBuf::EMPTY;
        }
        if u32::try_from(bytes.capacity()).is_err() {
            bytes.shrink_to_fit();
        }
        let (Ok(len), Ok(cap)) = (u32::try_from(bytes.len()), u32::try_from(bytes.capacity()))
        else {
            return KeelBuf::EMPTY;
        };
        let mut bytes = core::mem::ManuallyDrop::new(bytes);
        KeelBuf {
            ptr: bytes.as_mut_ptr(),
            len,
            cap,
        }
    }

    /// The valid bytes, or `&[]` for an empty buffer.
    ///
    /// # Safety
    ///
    /// `self` must be an empty buffer or a live one: `ptr` valid for reads of `len` bytes, as
    /// every buffer the core returns is until it is freed.
    pub unsafe fn as_slice(&self) -> &[u8] {
        if self.ptr.is_null() || self.len == 0 {
            &[]
        } else {
            // SAFETY: the caller guarantees `ptr` is valid for `len` bytes; both are non-zero
            // here, so the pointer is non-null.
            unsafe { core::slice::from_raw_parts(self.ptr, self.len as usize) }
        }
    }

    /// Releases a buffer previously returned by the core. Empty buffers (`cap == 0`) are
    /// ignored, so freeing twice an *empty* buffer is harmless; freeing a non-empty one twice
    /// is a double free, as with any allocator.
    ///
    /// # Safety
    ///
    /// `self` must be a buffer produced by this crate (`KeelBuf::from_vec` or a `keel_*`
    /// function that returns one), unmodified and not yet freed.
    pub unsafe fn free(self) {
        if self.ptr.is_null() || self.cap == 0 {
            return;
        }
        // SAFETY: by the caller's contract `ptr`, `len` and `cap` are the parts of the `Vec`
        // that `from_vec` leaked, so rebuilding it is sound and dropping it deallocates it.
        drop(unsafe { Vec::from_raw_parts(self.ptr, self.len as usize, self.cap as usize) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_vec_is_the_empty_buffer() {
        assert_eq!(KeelBuf::from_vec(Vec::new()), KeelBuf::EMPTY);
        // SAFETY: the empty buffer owns nothing.
        unsafe { KeelBuf::EMPTY.free() };
    }

    #[test]
    fn round_trip_keeps_the_bytes_and_frees() {
        let buf = KeelBuf::from_vec(vec![1, 2, 3]);
        assert_eq!(buf.len, 3);
        assert!(buf.cap >= 3);
        // SAFETY: `buf` came from `from_vec` and is freed exactly once, after the read.
        unsafe {
            assert_eq!(buf.as_slice(), [1, 2, 3]);
            buf.free();
        }
    }

    #[test]
    fn length_zero_with_capacity_is_still_freed() {
        let buf = KeelBuf::from_vec(Vec::with_capacity(16));
        assert_eq!(buf.len, 0);
        assert!(buf.cap >= 16);
        // SAFETY: from `from_vec`, freed once.
        unsafe {
            assert_eq!(buf.as_slice(), &[] as &[u8]);
            buf.free();
        }
    }
}
