#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! The Keel binary wire codec (`docs/SPEC.md` section 3).
//!
//! Everything that crosses the boundary between a Keel core and a platform (Swift, Kotlin,
//! TypeScript) is encoded with this crate: call arguments, return values, change-sets,
//! snapshots and the envelope that frames them on transports. The format is little-endian,
//! unaligned and unpadded; all lengths are `u32`.
//!
//! The crate has no dependencies, contains no `unsafe`, and its decoders **never panic**: any
//! byte string yields `Ok` or a typed [`WireError`]. Length prefixes are checked against the
//! bytes that are actually present before anything is allocated, and recursion is bounded by
//! [`MAX_DEPTH`], so hostile input cannot exhaust memory or the stack.
//!
//! # Values
//!
//! [`Writer`] and [`Reader`] carry the primitive reads and writes; the [`Encode`] and
//! [`Decode`] traits are implemented for `bool`, the integers, `f32`, `f64`, `()`, `String`,
//! [`Bytes`], `Option`, `Vec`, `HashMap`, `BTreeMap`, [`Duration`](core::time::Duration),
//! [`Timestamp`], [`Uuid`], `Result`, tuples up to four, `Box`, [`Handle`] and (encode only)
//! `&T`, `str` and `Arc<T>`.
//!
//! ```
//! use keel_wire::{Decode, Encode, Reader, Writer};
//! use std::collections::BTreeMap;
//!
//! let mut w = Writer::new();
//! "hello".encode(&mut w);
//! Some(42_i32).encode(&mut w);
//! BTreeMap::from([("b".to_string(), 2_i32), ("a".to_string(), 1)]).encode(&mut w);
//! let bytes = w.into_vec();
//!
//! let mut r = Reader::new(&bytes);
//! assert_eq!(String::decode(&mut r).unwrap(), "hello");
//! assert_eq!(Option::<i32>::decode(&mut r).unwrap(), Some(42));
//! let map = BTreeMap::<String, i32>::decode(&mut r).unwrap();
//! assert_eq!(map["a"], 1);
//! r.finish().unwrap(); // no trailing bytes
//! ```
//!
//! Maps encode their entries sorted by the *encoded key bytes*, so equal maps always produce
//! identical bytes regardless of hash order. Decoding a map with a repeated key is an error
//! ([`WireError::DuplicateKey`]).
//!
//! # Messages
//!
//! Transports frame every message in an [`Envelope`] (23 byte header plus payload). The
//! [`payload`] module has a typed struct for the payload of each [`Kind`], and [`KeyedPatch`]
//! is the compact update format for list signals.
//!
//! ```
//! use keel_wire::payload::{Call, CallTarget};
//! use keel_wire::{Envelope, Handle, Kind, Reader, Writer};
//!
//! // Host side: frame a method call.
//! let call = Call {
//!     target: CallTarget::Method { handle: Handle::new(1, 1), method_id: 0x8c45_40e0 },
//!     call_id: 9,
//!     args: &[2, 0, 0, 0, 3, 0, 0, 0],
//! };
//! let mut frame = Writer::new();
//! Envelope::write_with(&mut frame, Kind::Call, 1, 0xfeed, |w| call.encode(w));
//!
//! // Core side: parse the frame, check the schema, dispatch on the kind.
//! let env = Envelope::parse(frame.as_slice()).unwrap();
//! env.check_schema(0xfeed).unwrap();
//! assert_eq!(env.kind, Kind::Call);
//! let mut r = Reader::new(env.payload);
//! let decoded = Call::decode(&mut r).unwrap();
//! r.finish().unwrap();
//! assert_eq!(decoded, call);
//! ```

mod codec;
mod envelope;
mod error;
mod macros;
mod patch;
pub mod payload;
mod reader;
mod types;
mod writer;

pub use codec::{Decode, Encode};
pub use envelope::{Envelope, HEADER_LEN, Kind, MAGIC, VERSION};
pub use error::WireError;
pub use patch::{KeyedPatch, PatchError, PatchOp};
pub use reader::{MAX_DEPTH, MAX_ZERO_SIZED_COUNT, Reader};
pub use types::{Bytes, Handle, ParseUuidError, Timestamp, Uuid};
pub use writer::Writer;
