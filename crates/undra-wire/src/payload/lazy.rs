//! The three encodings of a lazily paged list (`Lazy<T>`, ADR-043 decision 3.2).
//!
//! * [`LazyValue`]: the value of a `Lazy<T>` signal (change-set op 0 and a snapshot's
//!   per-signal restore): the page server's object handle, how many items the list has and the
//!   version those counts belong to.
//! * [`LazyInvalidated`]: the value of change-set op 2: the new length and version, so the host
//!   knows both without a round trip and re-pages its visible window.
//! * [`LazyPage`]: the reply to a page call (call target 3): the version the page was read at,
//!   the list's total length, how many items follow, then the items back to back, each encoded
//!   as the item type.
//!
//! ```
//! use undra_wire::payload::{LazyPage, LazyValue};
//! use undra_wire::{Handle, Reader, Writer};
//!
//! let mut w = Writer::new();
//! LazyValue { handle: Handle::new(3, 1), len: 50_000, version: 7 }.encode(&mut w);
//! assert_eq!(w.as_slice().len(), 8 + 4 + 8);
//!
//! let mut w = Writer::new();
//! LazyPage { version: 7, total: 50_000, count: 2 }.encode(&mut w);
//! w.write_u32(10);
//! w.write_u32(11);
//! let mut r = Reader::new(w.as_slice());
//! let page = LazyPage::decode(&mut r).unwrap();
//! assert_eq!((page.version, page.total, page.count), (7, 50_000, 2));
//! assert_eq!(r.remaining(), 8); // the two items follow
//! ```

use crate::{Handle, Reader, WireError, Writer};

/// The value of a `Lazy<T>` signal. Layout: `handle u64, len u32, version u64`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LazyValue {
    /// The page server: the object a `LazyPage` call is addressed to.
    pub handle: Handle,
    /// The number of items.
    pub len: u32,
    /// The version of the list `len` was read at; it increases with every change.
    pub version: u64,
}

impl LazyValue {
    /// Appends the value to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.write_u64(self.handle.0);
        w.write_u32(self.len);
        w.write_u64(self.version);
    }

    /// Reads a value; does not require the reader to be exhausted.
    ///
    /// # Errors
    ///
    /// [`WireError::UnexpectedEof`] if fewer than 20 bytes remain.
    pub fn decode(r: &mut Reader<'_>) -> Result<LazyValue, WireError> {
        Ok(LazyValue {
            handle: Handle(r.read_u64()?),
            len: r.read_u32()?,
            version: r.read_u64()?,
        })
    }
}

/// The value of change-set op 2 (`LazyInvalidated`). Layout: `len u32, version u64`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LazyInvalidated {
    /// The new number of items.
    pub len: u32,
    /// The new version.
    pub version: u64,
}

impl LazyInvalidated {
    /// Appends the value to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.write_u32(self.len);
        w.write_u64(self.version);
    }

    /// Reads a value; does not require the reader to be exhausted.
    ///
    /// # Errors
    ///
    /// [`WireError::UnexpectedEof`] if fewer than 12 bytes remain.
    pub fn decode(r: &mut Reader<'_>) -> Result<LazyInvalidated, WireError> {
        Ok(LazyInvalidated {
            len: r.read_u32()?,
            version: r.read_u64()?,
        })
    }
}

/// The header of a page reply: the items follow it, `count` of them, each encoded as the item
/// type. Layout: `version u64, total u32, count u32`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LazyPage {
    /// The version of the list the page was read at.
    pub version: u64,
    /// The number of items the list had.
    pub total: u32,
    /// How many items follow.
    pub count: u32,
}

impl LazyPage {
    /// Appends the header to `w`; the caller writes the items.
    pub fn encode(&self, w: &mut Writer) {
        w.write_u64(self.version);
        w.write_u32(self.total);
        w.write_u32(self.count);
    }

    /// Reads a header; does not require the reader to be exhausted.
    ///
    /// # Errors
    ///
    /// [`WireError::UnexpectedEof`] if fewer than 16 bytes remain.
    pub fn decode(r: &mut Reader<'_>) -> Result<LazyPage, WireError> {
        Ok(LazyPage {
            version: r.read_u64()?,
            total: r.read_u32()?,
            count: r.read_u32()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layouts_are_the_documented_ones() {
        let mut w = Writer::new();
        LazyValue {
            handle: Handle(0x0102_0304_0506_0708),
            len: 9,
            version: 0x1112_1314_1516_1718,
        }
        .encode(&mut w);
        assert_eq!(
            w.as_slice(),
            [
                8, 7, 6, 5, 4, 3, 2, 1, 9, 0, 0, 0, 0x18, 0x17, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11
            ]
        );
        let mut w = Writer::new();
        LazyInvalidated { len: 3, version: 5 }.encode(&mut w);
        assert_eq!(w.as_slice(), [3, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0]);
        let mut w = Writer::new();
        LazyPage {
            version: 5,
            total: 3,
            count: 2,
        }
        .encode(&mut w);
        assert_eq!(
            w.as_slice(),
            [5, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 2, 0, 0, 0]
        );
    }

    #[test]
    fn every_truncation_is_a_typed_error_and_the_values_round_trip() {
        let value = LazyValue {
            handle: Handle::new(7, 9),
            len: u32::MAX,
            version: u64::MAX,
        };
        let mut w = Writer::new();
        value.encode(&mut w);
        let bytes = w.into_vec();
        for cut in 0..bytes.len() {
            assert!(LazyValue::decode(&mut Reader::new(&bytes[..cut])).is_err());
        }
        assert_eq!(LazyValue::decode(&mut Reader::new(&bytes)).unwrap(), value);

        let inv = LazyInvalidated { len: 0, version: 0 };
        let mut w = Writer::new();
        inv.encode(&mut w);
        let bytes = w.into_vec();
        for cut in 0..bytes.len() {
            assert!(LazyInvalidated::decode(&mut Reader::new(&bytes[..cut])).is_err());
        }
        assert_eq!(
            LazyInvalidated::decode(&mut Reader::new(&bytes)).unwrap(),
            inv
        );

        let page = LazyPage {
            version: 1,
            total: 2,
            count: 0,
        };
        let mut w = Writer::new();
        page.encode(&mut w);
        let bytes = w.into_vec();
        for cut in 0..bytes.len() {
            assert!(LazyPage::decode(&mut Reader::new(&bytes[..cut])).is_err());
        }
        assert_eq!(LazyPage::decode(&mut Reader::new(&bytes)).unwrap(), page);
    }
}
