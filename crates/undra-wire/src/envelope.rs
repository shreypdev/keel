//! The transport envelope (SPEC 3.2): a 23 byte header in front of every message on the
//! WebSocket and Worker transports.

use crate::macros::wire_u8_enum;
use crate::writer::len_u32;
use crate::{Reader, WireError, Writer};

/// The four magic bytes every envelope starts with: `4B 45 45 4C`. They are fixed by the wire
/// format (SPEC 3.2) and were not renamed with the product.
pub const MAGIC: [u8; 4] = [0x4B, 0x45, 0x45, 0x4C];

/// The envelope protocol version this crate reads and writes.
pub const VERSION: u16 = 1;

/// Size of the envelope header in bytes: magic 4 + version 2 + schema 8 + kind 1 + seq 4 +
/// len 4.
pub const HEADER_LEN: usize = 23;

/// Offset of the `kind` byte inside the header.
const KIND_AT: usize = 14;
/// Offset of the `len` field inside the header.
const LEN_AT: usize = 19;

wire_u8_enum! {
    /// The kind of message an [`Envelope`] carries. The payload layout for each kind is in
    /// the [`payload`](crate::payload) module.
    pub enum Kind {
        /// Host to core: invoke a function, method, constructor or lazy page.
        Call = 1,
        /// Core to host: the result of a call.
        Reply = 2,
        /// Core to host: signal updates from one transaction.
        ChangeSet = 3,
        /// Core to host: invoke a port method implemented by the platform.
        PortCall = 4,
        /// Host to core: the result of a port call.
        PortReply = 5,
        /// Host to core: cancel an in-flight call or stream.
        Cancel = 6,
        /// Host to core: grant a stream more credit.
        StreamCredit = 7,
        /// Core to host: an item, end or error of an open stream.
        StreamItem = 8,
        /// Host to core: start or stop observing a signal.
        Observe = 9,
        /// Host to core: release an object handle.
        Release = 10,
        /// Host to core: deliver an event to an event port.
        Event = 11,
        /// Both directions: handshake announcing version, schema hash and platform.
        Hello = 12,
        /// Core to host: a log record.
        Log = 13,
        /// Host to core: a timer set through the Timer port has fired.
        TimerFired = 14,
        /// Core to host: a snapshot of every store.
        Snapshot = 15,
        /// Host to core: restore stores from a snapshot (same payload as `Snapshot`).
        Restore = 16,
    }
}

/// A parsed envelope: header fields plus a borrowed view of the payload.
///
/// # Example
///
/// ```
/// use undra_wire::{Envelope, Kind, Writer};
///
/// let mut w = Writer::new();
/// Envelope::write(&mut w, Kind::Cancel, 7, 0xfeed, &[9, 0, 0, 0]);
/// let frame = w.into_vec();
///
/// let env = Envelope::parse(&frame).unwrap();
/// assert_eq!(env.kind, Kind::Cancel);
/// assert_eq!(env.seq, 7);
/// assert_eq!(env.schema, 0xfeed);
/// assert_eq!(env.payload, &[9, 0, 0, 0]);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Envelope<'a> {
    /// What the payload is.
    pub kind: Kind,
    /// Per-direction sequence number, for ordering and debugging.
    pub seq: u32,
    /// Schema hash of the core that produced or expects this message.
    pub schema: u64,
    /// The payload bytes, borrowed from the frame.
    pub payload: &'a [u8],
}

impl<'a> Envelope<'a> {
    /// Size of the header in bytes (23).
    pub const HEADER_LEN: usize = HEADER_LEN;

    /// Appends a complete envelope (header and payload) to `w`.
    ///
    /// Reserves the whole frame up front, so it allocates at most once.
    pub fn write(w: &mut Writer, kind: Kind, seq: u32, schema: u64, payload: &[u8]) {
        w.reserve(HEADER_LEN + payload.len());
        write_header(w, kind, seq, schema, len_u32(payload.len()));
        w.write_raw(payload);
    }

    /// Appends an envelope whose payload is produced by `payload` writing directly into `w`.
    ///
    /// This avoids building the payload in a separate buffer and copying it: the length field
    /// is back-patched after the closure returns.
    ///
    /// ```
    /// use undra_wire::{Encode, Envelope, Kind, Writer};
    ///
    /// let mut w = Writer::new();
    /// Envelope::write_with(&mut w, Kind::Cancel, 1, 0, |w| 9_u32.encode(w));
    /// assert_eq!(Envelope::parse(w.as_slice()).unwrap().payload, &[9, 0, 0, 0]);
    /// ```
    pub fn write_with(
        w: &mut Writer,
        kind: Kind,
        seq: u32,
        schema: u64,
        payload: impl FnOnce(&mut Writer),
    ) {
        let start = w.len();
        write_header(w, kind, seq, schema, 0);
        payload(w);
        let payload_len = w.len().saturating_sub(start + HEADER_LEN);
        w.patch_u32(start + LEN_AT, len_u32(payload_len));
    }

    /// Appends this envelope to `w`. Equivalent to [`Envelope::write`] with its fields.
    pub fn encode(&self, w: &mut Writer) {
        Envelope::write(w, self.kind, self.seq, self.schema, self.payload);
    }

    /// Parses one complete frame.
    ///
    /// Checks, in order: the magic, the version, the kind tag, and that the `len` field
    /// matches the bytes that follow the header exactly. A frame that is too short for its
    /// header is [`WireError::UnexpectedEof`], a `len` larger than the data present is
    /// [`WireError::LengthTooLarge`] and bytes after the payload are
    /// [`WireError::TrailingBytes`]. The schema hash is *not* compared here; use
    /// [`Envelope::check_schema`].
    pub fn parse(frame: &'a [u8]) -> Result<Envelope<'a>, WireError> {
        // A prefix that already contradicts the magic is BadMagic rather than "too short".
        let seen = frame.len().min(MAGIC.len());
        if frame.get(..seen) != MAGIC.get(..seen) {
            return Err(WireError::BadMagic);
        }
        let mut r = Reader::new(frame);
        r.read_raw(MAGIC.len())?;
        let version = r.read_u16()?;
        if version != VERSION {
            return Err(WireError::UnsupportedVersion(version));
        }
        let schema = r.read_u64()?;
        debug_assert_eq!(r.position(), KIND_AT);
        let kind = Kind::read(&mut r)?;
        let seq = r.read_u32()?;
        debug_assert_eq!(r.position(), LEN_AT);
        let len = r.read_len()?;
        let payload = r.read_raw(len)?;
        r.finish()?;
        Ok(Envelope {
            kind,
            seq,
            schema,
            payload,
        })
    }

    /// Returns [`WireError::SchemaMismatch`] unless this envelope's schema hash equals
    /// `expected`.
    pub fn check_schema(&self, expected: u64) -> Result<(), WireError> {
        if self.schema == expected {
            Ok(())
        } else {
            Err(WireError::SchemaMismatch {
                expected,
                got: self.schema,
            })
        }
    }
}

fn write_header(w: &mut Writer, kind: Kind, seq: u32, schema: u64, len: u32) {
    w.write_raw(&MAGIC);
    w.write_u16(VERSION);
    w.write_u64(schema);
    w.write_u8(kind.as_u8());
    w.write_u32(seq);
    w.write_u32(len);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(kind: Kind, seq: u32, schema: u64, payload: &[u8]) -> Vec<u8> {
        let mut w = Writer::new();
        Envelope::write(&mut w, kind, seq, schema, payload);
        w.into_vec()
    }

    #[test]
    fn header_is_23_bytes() {
        assert_eq!(frame(Kind::Call, 0, 0, &[]).len(), HEADER_LEN);
        assert_eq!(HEADER_LEN, 23);
        assert_eq!(Envelope::HEADER_LEN, 23);
    }

    #[test]
    fn all_sixteen_kinds_round_trip() {
        assert_eq!(Kind::ALL.len(), 16);
        for (i, &kind) in Kind::ALL.iter().enumerate() {
            assert_eq!(usize::from(kind.as_u8()), i + 1);
            assert_eq!(Kind::try_from(kind.as_u8()), Ok(kind));
            let f = frame(kind, 5, 6, &[1]);
            assert_eq!(Envelope::parse(&f).unwrap().kind, kind);
        }
    }

    #[test]
    fn unknown_kinds_are_rejected() {
        for tag in [0_u8, 17, 255] {
            assert_eq!(
                Kind::try_from(tag),
                Err(WireError::InvalidTag {
                    tag: u32::from(tag),
                    at: 0,
                    ty: "Kind"
                })
            );
            let mut f = frame(Kind::Call, 0, 0, &[]);
            f[KIND_AT] = tag;
            assert_eq!(
                Envelope::parse(&f),
                Err(WireError::InvalidTag {
                    tag: u32::from(tag),
                    at: KIND_AT,
                    ty: "Kind"
                })
            );
        }
    }

    #[test]
    fn parse_matches_the_spec_layout() {
        let f = frame(Kind::Call, 7, 0x0102_0304_0506_0708, &[0xaa, 0xbb, 0xcc]);
        assert_eq!(
            f,
            [
                b'K', b'E', b'E', b'L', 1, 0, 8, 7, 6, 5, 4, 3, 2, 1, 1, 7, 0, 0, 0, 3, 0, 0, 0,
                0xaa, 0xbb, 0xcc
            ]
        );
        let env = Envelope::parse(&f).unwrap();
        assert_eq!(env.kind, Kind::Call);
        assert_eq!(env.seq, 7);
        assert_eq!(env.schema, 0x0102_0304_0506_0708);
        assert_eq!(env.payload, &[0xaa, 0xbb, 0xcc]);
    }

    #[test]
    fn payload_is_borrowed_from_the_frame() {
        let f = frame(Kind::Log, 1, 2, &[1, 2, 3]);
        let env = Envelope::parse(&f).unwrap();
        assert_eq!(env.payload.as_ptr(), f[HEADER_LEN..].as_ptr());
    }

    #[test]
    fn write_with_matches_write() {
        let payload = [1_u8, 2, 3, 4, 5];
        let mut w = Writer::new();
        w.write_u8(0xee); // pre-existing content must not disturb the back-patch
        Envelope::write_with(&mut w, Kind::Event, 9, 10, |w| w.write_raw(&payload));
        assert_eq!(&w.as_slice()[1..], &frame(Kind::Event, 9, 10, &payload)[..]);
    }

    #[test]
    fn bad_magic() {
        let mut f = frame(Kind::Call, 0, 0, &[]);
        f[0] = b'X';
        assert_eq!(Envelope::parse(&f), Err(WireError::BadMagic));
        // A prefix that contradicts the magic is BadMagic even when the frame is tiny.
        assert_eq!(Envelope::parse(b"X"), Err(WireError::BadMagic));
        assert_eq!(Envelope::parse(b"KEEX"), Err(WireError::BadMagic));
        // A prefix that is consistent with the magic is merely short.
        assert_eq!(
            Envelope::parse(b"KE"),
            Err(WireError::UnexpectedEof { needed: 4, at: 0 })
        );
        assert_eq!(
            Envelope::parse(b""),
            Err(WireError::UnexpectedEof { needed: 4, at: 0 })
        );
    }

    #[test]
    fn unsupported_version() {
        let mut f = frame(Kind::Call, 0, 0, &[]);
        f[4] = 2;
        assert_eq!(Envelope::parse(&f), Err(WireError::UnsupportedVersion(2)));
    }

    #[test]
    fn truncated_header() {
        let f = frame(Kind::Call, 0, 0, &[]);
        for cut in 0..HEADER_LEN {
            assert!(
                matches!(
                    Envelope::parse(&f[..cut]),
                    Err(WireError::UnexpectedEof { .. })
                ),
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn payload_length_must_match_exactly() {
        let f = frame(Kind::Call, 0, 0, &[1, 2, 3]);
        assert_eq!(
            Envelope::parse(&f[..f.len() - 1]),
            Err(WireError::LengthTooLarge { len: 3, at: LEN_AT })
        );
        let mut long = f.clone();
        long.push(0);
        assert_eq!(
            Envelope::parse(&long),
            Err(WireError::TrailingBytes { count: 1 })
        );
    }

    #[test]
    fn hostile_length_is_rejected() {
        let mut f = frame(Kind::Call, 0, 0, &[]);
        f[LEN_AT..].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            Envelope::parse(&f),
            Err(WireError::LengthTooLarge {
                len: u32::MAX,
                at: LEN_AT
            })
        );
    }

    #[test]
    fn check_schema() {
        let f = frame(Kind::Call, 0, 42, &[]);
        let env = Envelope::parse(&f).unwrap();
        assert_eq!(env.check_schema(42), Ok(()));
        assert_eq!(
            env.check_schema(43),
            Err(WireError::SchemaMismatch {
                expected: 43,
                got: 42
            })
        );
    }

    #[test]
    fn encode_round_trips() {
        let env = Envelope {
            kind: Kind::Hello,
            seq: 3,
            schema: 4,
            payload: &[5, 6],
        };
        let mut w = Writer::new();
        env.encode(&mut w);
        assert_eq!(Envelope::parse(w.as_slice()), Ok(env));
    }
}
