//! Typed payloads for every envelope [`Kind`](crate::Kind) (SPEC 3.3 to 3.7 and 5.9).
//!
//! Each payload type has an inherent `encode(&self, &mut Writer)` and a
//! `decode(&mut Reader) -> Result<Self, WireError>`, so no trait import is needed. `decode`
//! reads exactly the payload's own bytes and does **not** require the reader to be exhausted;
//! call [`Reader::finish`] afterwards to reject trailing garbage:
//!
//! ```
//! use undra_wire::payload::Cancel;
//! use undra_wire::{Reader, Writer};
//!
//! let mut w = Writer::new();
//! Cancel { call_id: 9 }.encode(&mut w);
//!
//! let mut r = Reader::new(w.as_slice());
//! let cancel = Cancel::decode(&mut r).unwrap();
//! r.finish().unwrap();
//! assert_eq!(cancel.call_id, 9);
//! ```
//!
//! Payloads that end in a caller-defined body (`Call::args`, `Reply::body`, ...) borrow it
//! from the input on decode and from the caller on encode, so neither direction copies the
//! body except into the output buffer. Their body is *the rest of the payload*; the envelope
//! length delimits it.

mod call;
mod changeset;
mod snapshot;

use crate::macros::wire_u8_enum;
use crate::{Handle, Reader, WireError, Writer};

pub use call::{Call, CallOwned, CallTarget};
pub use changeset::{
    ChangeEntries, ChangeEntry, ChangeEntryRef, ChangeOp, ChangeSet, ChangeSetBuilder, ChangeSetRef,
};
pub use snapshot::{Restore, Snapshot, StoreSnapshot};

wire_u8_enum! {
    /// Outcome of a call (SPEC 3.4).
    pub enum ReplyStatus {
        /// Success; the body is the return value (empty for unit; the `T` of a `Result<T, E>`).
        Ok = 0,
        /// A typed error; the body is the `E` value.
        Error = 1,
        /// The callee panicked; the body is `String message, String backtrace`.
        Panic = 2,
        /// The call was cancelled; the body is empty.
        Cancelled = 3,
        /// A stream was opened; the body is empty and items follow as `StreamItem`s.
        StreamOpened = 4,
        /// The request was malformed (unknown method, decode failure, schema mismatch, stale
        /// handle); the body is a `String` reason.
        BadRequest = 5,
    }
}

wire_u8_enum! {
    /// Outcome of a port call (SPEC 3.6).
    pub enum PortStatus {
        /// Success; the body is the port method's return value.
        Ok = 0,
        /// The port method returned an error; the body is the `E` value.
        Error = 1,
        /// The port is not available on this platform.
        Unavailable = 2,
    }
}

wire_u8_enum! {
    /// What a `StreamItem` carries (SPEC 3.7).
    pub enum StreamFlag {
        /// An item; the body is the item `T`.
        Item = 0,
        /// The stream ended normally; the body is empty.
        End = 1,
        /// The stream failed; the body is the error `E` (or a `String` if the stream has no
        /// error type).
        Error = 2,
    }
}

/// Reply to a [`Call`] (kind `Reply`, SPEC 3.4).
///
/// Layout: `call_id u32, status u8, body` where the body is the rest of the payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reply<'a> {
    /// The id of the call being answered.
    pub call_id: u32,
    /// How the call ended.
    pub status: ReplyStatus,
    /// The encoded body; its meaning depends on `status`.
    pub body: &'a [u8],
}

impl<'a> Reply<'a> {
    /// Appends the payload to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.reserve(5 + self.body.len());
        w.write_u32(self.call_id);
        w.write_u8(self.status.as_u8());
        w.write_raw(self.body);
    }

    /// Reads a payload; the body is everything after the status byte.
    pub fn decode(r: &mut Reader<'a>) -> Result<Self, WireError> {
        let call_id = r.read_u32()?;
        let status = ReplyStatus::read(r)?;
        let body = r.read_rest();
        Ok(Reply {
            call_id,
            status,
            body,
        })
    }
}

/// A call from the core into a platform-implemented port (kind `PortCall`, SPEC 3.6).
///
/// Layout: `port_id u32, method_id u32, port_call_id u32, args`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortCall<'a> {
    /// The port being called.
    pub port_id: u32,
    /// The port method.
    pub method_id: u32,
    /// Chosen by the core; unique among in-flight port calls.
    pub port_call_id: u32,
    /// The encoded parameters, in declaration order.
    pub args: &'a [u8],
}

impl<'a> PortCall<'a> {
    /// Appends the payload to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.reserve(12 + self.args.len());
        w.write_u32(self.port_id);
        w.write_u32(self.method_id);
        w.write_u32(self.port_call_id);
        w.write_raw(self.args);
    }

    /// Reads a payload; `args` is everything after `port_call_id`.
    pub fn decode(r: &mut Reader<'a>) -> Result<Self, WireError> {
        let port_id = r.read_u32()?;
        let method_id = r.read_u32()?;
        let port_call_id = r.read_u32()?;
        let args = r.read_rest();
        Ok(PortCall {
            port_id,
            method_id,
            port_call_id,
            args,
        })
    }
}

/// The platform's answer to a [`PortCall`] (kind `PortReply`, SPEC 3.6).
///
/// Layout: `port_call_id u32, status u8, body` where the body is the rest of the payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortReply<'a> {
    /// The id of the port call being answered.
    pub port_call_id: u32,
    /// How the port call ended.
    pub status: PortStatus,
    /// The encoded return value (status `Ok`) or error (status `Error`); empty otherwise.
    pub body: &'a [u8],
}

impl<'a> PortReply<'a> {
    /// Appends the payload to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.reserve(5 + self.body.len());
        w.write_u32(self.port_call_id);
        w.write_u8(self.status.as_u8());
        w.write_raw(self.body);
    }

    /// Reads a payload; the body is everything after the status byte.
    pub fn decode(r: &mut Reader<'a>) -> Result<Self, WireError> {
        let port_call_id = r.read_u32()?;
        let status = PortStatus::read(r)?;
        let body = r.read_rest();
        Ok(PortReply {
            port_call_id,
            status,
            body,
        })
    }
}

/// Cancels an in-flight call or closes a stream (kind `Cancel`). Layout: `call_id u32`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancel {
    /// The call to cancel.
    pub call_id: u32,
}

impl Cancel {
    /// Appends the payload to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.write_u32(self.call_id);
    }

    /// Reads a payload.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Cancel {
            call_id: r.read_u32()?,
        })
    }
}

/// Grants a stream more credit (kind `StreamCredit`). Layout: `call_id u32, credit u32`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamCredit {
    /// The stream's call id.
    pub call_id: u32,
    /// How many more items the core may send.
    pub credit: u32,
}

impl StreamCredit {
    /// Appends the payload to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.write_u32(self.call_id);
        w.write_u32(self.credit);
    }

    /// Reads a payload.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(StreamCredit {
            call_id: r.read_u32()?,
            credit: r.read_u32()?,
        })
    }
}

/// One element of an open stream (kind `StreamItem`, SPEC 3.7).
///
/// Layout: `call_id u32, flag u8, body` where the body is the rest of the payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamItem<'a> {
    /// The stream's call id.
    pub call_id: u32,
    /// Item, end or error.
    pub flag: StreamFlag,
    /// The encoded item or error; empty for `End`.
    pub body: &'a [u8],
}

impl<'a> StreamItem<'a> {
    /// Appends the payload to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.reserve(5 + self.body.len());
        w.write_u32(self.call_id);
        w.write_u8(self.flag.as_u8());
        w.write_raw(self.body);
    }

    /// Reads a payload; the body is everything after the flag byte.
    pub fn decode(r: &mut Reader<'a>) -> Result<Self, WireError> {
        let call_id = r.read_u32()?;
        let flag = StreamFlag::read(r)?;
        let body = r.read_rest();
        Ok(StreamItem {
            call_id,
            flag,
            body,
        })
    }
}

/// Starts or stops observing a signal (kind `Observe`).
///
/// Layout: `handle u64, signal_id u32, on u8` (`0` or `1`). A `signal_id` of `u32::MAX` means
/// every signal of the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observe {
    /// The store.
    pub handle: Handle,
    /// The signal, or `u32::MAX` for all of them.
    pub signal_id: u32,
    /// `true` to start observing, `false` to stop.
    pub on: bool,
}

impl Observe {
    /// Appends the payload to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.write_u64(self.handle.0);
        w.write_u32(self.signal_id);
        w.write_bool(self.on);
    }

    /// Reads a payload. `on` must be `0` or `1`.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Observe {
            handle: Handle(r.read_u64()?),
            signal_id: r.read_u32()?,
            on: r.read_bool()?,
        })
    }
}

/// Releases an object handle (kind `Release`). Layout: `handle u64`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Release {
    /// The handle to release.
    pub handle: Handle,
}

impl Release {
    /// Appends the payload to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.write_u64(self.handle.0);
    }

    /// Reads a payload.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Release {
            handle: Handle(r.read_u64()?),
        })
    }
}

/// An event delivered to an event port (kind `Event`).
///
/// Layout: `port_id u32, method_id u32, payload` where the payload is the rest of the message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event<'a> {
    /// The event port.
    pub port_id: u32,
    /// The port method being invoked.
    pub method_id: u32,
    /// The encoded event parameters.
    pub payload: &'a [u8],
}

impl<'a> Event<'a> {
    /// Appends the payload to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.reserve(8 + self.payload.len());
        w.write_u32(self.port_id);
        w.write_u32(self.method_id);
        w.write_raw(self.payload);
    }

    /// Reads a payload; `payload` is everything after `method_id`.
    pub fn decode(r: &mut Reader<'a>) -> Result<Self, WireError> {
        let port_id = r.read_u32()?;
        let method_id = r.read_u32()?;
        let payload = r.read_rest();
        Ok(Event {
            port_id,
            method_id,
            payload,
        })
    }
}

/// The handshake message (kind `Hello`), sent by both sides when a transport connects.
///
/// Layout: `undra_version String, schema_hash u64, platform String, mode String`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hello<'a> {
    /// The Undra version of the sender, for example `"1.0.0"`.
    pub undra_version: &'a str,
    /// The sender's schema hash. Peers with different hashes must not talk to each other.
    pub schema_hash: u64,
    /// The sender's platform, for example `"ios"`, `"android"`, `"web"`, `"rust"`.
    pub platform: &'a str,
    /// The sender's mode, for example `"dev"` or `"release"`.
    pub mode: &'a str,
}

impl<'a> Hello<'a> {
    /// Appends the payload to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.write_str(self.undra_version);
        w.write_u64(self.schema_hash);
        w.write_str(self.platform);
        w.write_str(self.mode);
    }

    /// Reads a payload, borrowing the strings from the input.
    pub fn decode(r: &mut Reader<'a>) -> Result<Self, WireError> {
        Ok(Hello {
            undra_version: r.read_str()?,
            schema_hash: r.read_u64()?,
            platform: r.read_str()?,
            mode: r.read_str()?,
        })
    }
}

/// A log record from the core (kind `Log`). Layout: `level u8, target String, message String`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Log<'a> {
    /// The severity, using the numbering of the Log port.
    pub level: u8,
    /// The emitting module or subsystem.
    pub target: &'a str,
    /// The message text.
    pub message: &'a str,
}

impl<'a> Log<'a> {
    /// Appends the payload to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.write_u8(self.level);
        w.write_str(self.target);
        w.write_str(self.message);
    }

    /// Reads a payload, borrowing the strings from the input.
    pub fn decode(r: &mut Reader<'a>) -> Result<Self, WireError> {
        Ok(Log {
            level: r.read_u8()?,
            target: r.read_str()?,
            message: r.read_str()?,
        })
    }
}

/// A timer set through the Timer port has fired (kind `TimerFired`). Layout: `timer_id u32`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimerFired {
    /// The id passed to `Timer::set`.
    pub timer_id: u32,
}

impl TimerFired {
    /// Appends the payload to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.write_u32(self.timer_id);
    }

    /// Reads a payload.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(TimerFired {
            timer_id: r.read_u32()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(f: impl FnOnce(&mut Writer)) -> Vec<u8> {
        let mut w = Writer::new();
        f(&mut w);
        w.into_vec()
    }

    #[test]
    fn status_enums_convert_from_u8() {
        assert_eq!(ReplyStatus::ALL.len(), 6);
        for (i, &s) in ReplyStatus::ALL.iter().enumerate() {
            assert_eq!(ReplyStatus::try_from(i as u8), Ok(s));
            assert_eq!(u8::from(s), i as u8);
        }
        assert!(ReplyStatus::try_from(6).is_err());
        assert_eq!(PortStatus::ALL.len(), 3);
        assert!(PortStatus::try_from(3).is_err());
        assert_eq!(StreamFlag::ALL.len(), 3);
        assert!(StreamFlag::try_from(3).is_err());
    }

    #[test]
    fn reply_layout() {
        let reply = Reply {
            call_id: 9,
            status: ReplyStatus::Ok,
            body: &[5, 0, 0, 0],
        };
        let b = bytes(|w| reply.encode(w));
        assert_eq!(b, [9, 0, 0, 0, 0, 5, 0, 0, 0]);
        assert_eq!(Reply::decode(&mut Reader::new(&b)), Ok(reply));
    }

    #[test]
    fn reply_rejects_unknown_status_with_offset() {
        let b = [9, 0, 0, 0, 6];
        assert_eq!(
            Reply::decode(&mut Reader::new(&b)),
            Err(WireError::InvalidTag {
                tag: 6,
                at: 4,
                ty: "ReplyStatus"
            })
        );
    }

    #[test]
    fn port_call_and_reply_layout() {
        let call = PortCall {
            port_id: 1,
            method_id: 2,
            port_call_id: 3,
            args: &[7],
        };
        let b = bytes(|w| call.encode(w));
        assert_eq!(b, [1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 7]);
        assert_eq!(PortCall::decode(&mut Reader::new(&b)), Ok(call));

        let reply = PortReply {
            port_call_id: 3,
            status: PortStatus::Unavailable,
            body: &[],
        };
        let b = bytes(|w| reply.encode(w));
        assert_eq!(b, [3, 0, 0, 0, 2]);
        assert_eq!(PortReply::decode(&mut Reader::new(&b)), Ok(reply));
    }

    #[test]
    fn small_fixed_payload_layouts() {
        let b = bytes(|w| Cancel { call_id: 9 }.encode(w));
        assert_eq!(b, [9, 0, 0, 0]);

        let b = bytes(|w| {
            StreamCredit {
                call_id: 1,
                credit: 16,
            }
            .encode(w)
        });
        assert_eq!(b, [1, 0, 0, 0, 16, 0, 0, 0]);

        let observe = Observe {
            handle: Handle::new(1, 1),
            signal_id: u32::MAX,
            on: true,
        };
        let b = bytes(|w| observe.encode(w));
        assert_eq!(
            b,
            [1, 0, 0, 0, 1, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 1],
            "handle u64, signal_id u32, on u8"
        );
        assert_eq!(Observe::decode(&mut Reader::new(&b)), Ok(observe));

        let b = bytes(|w| {
            Release {
                handle: Handle::new(2, 3),
            }
            .encode(w)
        });
        assert_eq!(b, [2, 0, 0, 0, 3, 0, 0, 0]);

        let b = bytes(|w| TimerFired { timer_id: 77 }.encode(w));
        assert_eq!(b, [77, 0, 0, 0]);
    }

    #[test]
    fn observe_rejects_non_boolean_on() {
        let mut b = bytes(|w| {
            Observe {
                handle: Handle(1),
                signal_id: 0,
                on: true,
            }
            .encode(w)
        });
        b[12] = 2;
        assert_eq!(
            Observe::decode(&mut Reader::new(&b)),
            Err(WireError::InvalidTag {
                tag: 2,
                at: 12,
                ty: "bool"
            })
        );
    }

    #[test]
    fn stream_item_layout() {
        let item = StreamItem {
            call_id: 4,
            flag: StreamFlag::Item,
            body: &[1, 2],
        };
        let b = bytes(|w| item.encode(w));
        assert_eq!(b, [4, 0, 0, 0, 0, 1, 2]);
        assert_eq!(StreamItem::decode(&mut Reader::new(&b)), Ok(item));
    }

    #[test]
    fn event_layout() {
        let event = Event {
            port_id: 1,
            method_id: 2,
            payload: &[3, 4],
        };
        let b = bytes(|w| event.encode(w));
        assert_eq!(b, [1, 0, 0, 0, 2, 0, 0, 0, 3, 4]);
        assert_eq!(Event::decode(&mut Reader::new(&b)), Ok(event));
    }

    #[test]
    fn hello_layout_and_borrowing() {
        let hello = Hello {
            undra_version: "1.0.0",
            schema_hash: 0xabcd,
            platform: "web",
            mode: "dev",
        };
        let b = bytes(|w| hello.encode(w));
        let mut r = Reader::new(&b);
        let back = Hello::decode(&mut r).unwrap();
        r.finish().unwrap();
        assert_eq!(back, hello);
        // Strings point into the input, not into fresh allocations.
        let range = b.as_ptr_range();
        assert!(range.contains(&back.platform.as_ptr()));
    }

    #[test]
    fn log_layout() {
        let log = Log {
            level: 3,
            target: "undra::runtime",
            message: "héllo",
        };
        let b = bytes(|w| log.encode(w));
        assert_eq!(b[0], 3);
        assert_eq!(Log::decode(&mut Reader::new(&b)), Ok(log));
    }

    #[test]
    fn truncated_payloads_are_errors() {
        let b = bytes(|w| {
            Hello {
                undra_version: "1",
                schema_hash: 2,
                platform: "p",
                mode: "m",
            }
            .encode(w)
        });
        for cut in 0..b.len() {
            assert!(Hello::decode(&mut Reader::new(&b[..cut])).is_err(), "{cut}");
        }
    }
}
