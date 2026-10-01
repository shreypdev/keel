//! The devtools connection's messages (ADR-054).
//!
//! A devtools connection is **not** an envelope stream: every WebSocket binary message is one
//! message `[tag u8][body]`, in the little-endian primitives of SPEC 3. Documents (the schema,
//! the counters, the query cache) are JSON text inside a `String`; everything that is a record
//! of something that happened is binary, and a change-set is the very payload of SPEC 3.5 so a
//! page decodes it with the runtime's own wire code. Nothing here is reachable from an app
//! client, and nothing here changes the envelope, a payload or an ABI.
//!
//! The page and the server ship together (the page is compiled into the runner `undra dev`
//! generates), and [`PROTOCOL`] is checked in [`Welcome`] anyway.

use undra_wire::{Reader, WireError, Writer};

/// The protocol version a [`Welcome`] announces. A page that does not speak it says so.
pub const PROTOCOL: u16 = 1;

/// Message tags, server to page.
pub mod server {
    /// [`Welcome`](super::Welcome).
    pub const WELCOME: u8 = 1;
    /// [`ServerMsg::Stores`].
    pub const STORES: u8 = 2;
    /// [`ServerMsg::ChangeSet`].
    pub const CHANGE_SET: u8 = 3;
    /// [`StepInfo`].
    pub const STEP: u8 = 4;
    /// [`ServerMsg::Evicted`].
    pub const EVICTED: u8 = 5;
    /// [`PortRecord`].
    pub const PORT: u8 = 6;
    /// [`ServerMsg::Stats`].
    pub const STATS: u8 = 7;
    /// [`ServerMsg::Queries`].
    pub const QUERIES: u8 = 8;
    /// [`Traveled`].
    pub const TRAVELED: u8 = 9;
    /// [`ServerMsg::App`].
    pub const APP: u8 = 10;
}

/// Message tags, page to server.
pub mod client {
    /// [`ClientMsg::Restore`].
    pub const RESTORE: u8 = 1;
    /// [`ClientMsg::Resync`].
    pub const RESYNC: u8 = 2;
}

/// What caused a change-set, as far as the server can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// A task, a timer, a stream or a port completion: the server cannot name the call.
    Other,
    /// A synchronous call from the app client; the id is the method's (the page names it from the
    /// schema).
    Call(u32),
    /// A time-travel restore of the step.
    Restore(u32),
}

impl Cause {
    fn encode(self, w: &mut Writer) {
        match self {
            Cause::Other => {
                w.write_u8(0);
                w.write_u32(0);
            }
            Cause::Call(method) => {
                w.write_u8(1);
                w.write_u32(method);
            }
            Cause::Restore(step) => {
                w.write_u8(2);
                w.write_u32(step);
            }
        }
    }

    fn decode(r: &mut Reader<'_>) -> Result<Cause, WireError> {
        let at = r.position();
        let tag = r.read_u8()?;
        let arg = r.read_u32()?;
        match tag {
            0 => Ok(Cause::Other),
            1 => Ok(Cause::Call(arg)),
            2 => Ok(Cause::Restore(arg)),
            _ => Err(WireError::InvalidTag {
                tag: u32::from(tag),
                at,
                ty: "Cause",
            }),
        }
    }
}

/// Whether a change-set is news or a state the page asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// A transaction committed: a timeline entry.
    Commit,
    /// The current values of a store, sent because the page attached or the hub began to watch
    /// the store: apply it, but it is not an event.
    Initial,
}

/// The first message of a connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Welcome {
    /// [`PROTOCOL`].
    pub protocol: u16,
    /// The Undra version of the server.
    pub undra_version: String,
    /// The core's schema hash.
    pub schema_hash: u64,
    /// The core's platform label.
    pub platform: String,
    /// The core's mode (`"dev"`).
    pub mode: String,
    /// Identifies this core process: steps of another epoch cannot be restored.
    pub core_epoch: u64,
    /// Milliseconds since the Unix epoch when the hub started (the page shows clock times).
    pub started_unix_ms: u64,
    /// The ring's bounds: steps kept.
    pub ring_steps: u32,
    /// The ring's bounds: bytes kept in all.
    pub ring_bytes: u64,
    /// The ring's bounds: the largest snapshot that is kept.
    pub ring_step_bytes: u64,
    /// The whole schema, in the exchange form (`Schema::to_json`).
    pub schema_json: String,
}

/// One store the core holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreRef {
    /// The store's handle.
    pub handle: u64,
    /// The store's type id (`ObjectDef.type_id` in the schema).
    pub type_id: u32,
}

/// A snapshot the ring took: a point the page can travel back to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepInfo {
    /// The step number, increasing from 1.
    pub step: u32,
    /// The last change-set sequence the step covers.
    pub through_seq: u64,
    /// The last `txn_id` delivered before the snapshot.
    pub txn: u64,
    /// Milliseconds since the hub started.
    pub at_ms: u64,
    /// The snapshot's size.
    pub bytes: u32,
    /// How many stores it holds.
    pub stores: u32,
    /// Whether it can be restored (a snapshot over the per-step bound is listed, not kept).
    pub restorable: bool,
    /// The step this one restored, when it is the result of a time travel; `0` otherwise.
    pub restored_from: u32,
}

/// A call to a platform-implemented port, at its start or its end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortRecord {
    /// The core asked the client to run a port method.
    Start {
        /// The call's id (unique while it is open).
        id: u32,
        /// The port's id.
        port_id: u32,
        /// The method's id.
        method_id: u32,
        /// Milliseconds since the hub started.
        at_ms: u64,
        /// The encoded arguments.
        args: Vec<u8>,
    },
    /// The call ended.
    End {
        /// The call's id.
        id: u32,
        /// The port's id.
        port_id: u32,
        /// The method's id.
        method_id: u32,
        /// Milliseconds since the hub started, at the end.
        at_ms: u64,
        /// `0` ok, `1` typed error, `2` unavailable (SPEC 3.6).
        status: u8,
        /// Microseconds from start to end, on the server's monotonic clock.
        latency_us: u64,
        /// The encoded reply body (empty when unavailable).
        reply: Vec<u8>,
    },
}

/// The answer to a [`ClientMsg::Restore`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Traveled {
    /// The request it answers.
    pub request_id: u32,
    /// Whether the core was restored.
    pub ok: bool,
    /// The step asked for.
    pub step: u32,
    /// How many stores the core had that the step did not (their handles are stale now).
    pub dropped: u32,
    /// A sentence: what happened, or why it did not.
    pub message: String,
}

/// A message from the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerMsg {
    /// The first message.
    Welcome(Welcome),
    /// The stores that exist now, sent whenever the set changes.
    Stores(Vec<StoreRef>),
    /// A change-set (SPEC 3.5), with what the server knows about it.
    ChangeSet {
        /// The hub's sequence number: increasing, one per change-set it forwards.
        seq: u64,
        /// Milliseconds since the hub started.
        at_ms: u64,
        /// News or state.
        delivery: Delivery,
        /// What caused it.
        cause: Cause,
        /// The payload of SPEC 3.5.
        payload: Vec<u8>,
    },
    /// A snapshot was taken.
    Step(StepInfo),
    /// Steps below this number were dropped from the ring.
    Evicted {
        /// The oldest step still kept.
        below_step: u32,
    },
    /// A port call.
    Port(PortRecord),
    /// The counters, as JSON (see `docs/DEV_LOOP.md`).
    Stats(String),
    /// The query cache: the document the `queries` inspector produced (one JSON object with an
    /// `entries` array), sampled when it changes. The page derives the log of what happened to
    /// each entry by comparing samples.
    Queries {
        /// Milliseconds since the hub started.
        at_ms: u64,
        /// The document.
        json: String,
    },
    /// The result of a time travel.
    Traveled(Traveled),
    /// Whether an app client is attached, and what it said about itself.
    App {
        /// A client holds the app slot.
        connected: bool,
        /// Its platform (`web`, `ios`, ...), empty when none.
        platform: String,
    },
}

impl ServerMsg {
    /// Writes the message: its tag, then its body.
    pub fn encode(&self, w: &mut Writer) {
        match self {
            ServerMsg::Welcome(m) => {
                w.write_u8(server::WELCOME);
                w.write_u16(m.protocol);
                w.write_str(&m.undra_version);
                w.write_u64(m.schema_hash);
                w.write_str(&m.platform);
                w.write_str(&m.mode);
                w.write_u64(m.core_epoch);
                w.write_u64(m.started_unix_ms);
                w.write_u32(m.ring_steps);
                w.write_u64(m.ring_bytes);
                w.write_u64(m.ring_step_bytes);
                w.write_str(&m.schema_json);
            }
            ServerMsg::Stores(stores) => {
                w.write_u8(server::STORES);
                w.write_len(u32::try_from(stores.len()).unwrap_or(u32::MAX));
                for s in stores {
                    w.write_u64(s.handle);
                    w.write_u32(s.type_id);
                }
            }
            ServerMsg::ChangeSet {
                seq,
                at_ms,
                delivery,
                cause,
                payload,
            } => encode_change_set(w, *seq, *at_ms, *delivery, *cause, payload),
            ServerMsg::Step(s) => {
                w.write_u8(server::STEP);
                w.write_u32(s.step);
                w.write_u64(s.through_seq);
                w.write_u64(s.txn);
                w.write_u64(s.at_ms);
                w.write_u32(s.bytes);
                w.write_u32(s.stores);
                w.write_bool(s.restorable);
                w.write_u32(s.restored_from);
            }
            ServerMsg::Evicted { below_step } => {
                w.write_u8(server::EVICTED);
                w.write_u32(*below_step);
            }
            ServerMsg::Port(PortRecord::Start {
                id,
                port_id,
                method_id,
                at_ms,
                args,
            }) => encode_port_start(w, *id, *port_id, *method_id, *at_ms, args),
            ServerMsg::Port(PortRecord::End {
                id,
                port_id,
                method_id,
                at_ms,
                status,
                latency_us,
                reply,
            }) => encode_port_end(w, *id, *port_id, *method_id, *at_ms, *status, *latency_us, reply),
            ServerMsg::Stats(json) => {
                w.write_u8(server::STATS);
                w.write_str(json);
            }
            ServerMsg::Queries { at_ms, json } => {
                w.write_u8(server::QUERIES);
                w.write_u64(*at_ms);
                w.write_str(json);
            }
            ServerMsg::Traveled(t) => {
                w.write_u8(server::TRAVELED);
                w.write_u32(t.request_id);
                w.write_bool(t.ok);
                w.write_u32(t.step);
                w.write_u32(t.dropped);
                w.write_str(&t.message);
            }
            ServerMsg::App {
                connected,
                platform,
            } => {
                w.write_u8(server::APP);
                w.write_bool(*connected);
                w.write_str(platform);
            }
        }
    }

    /// Reads one message.
    ///
    /// # Errors
    ///
    /// The [`WireError`] of whatever does not decode, including an unknown tag.
    pub fn decode(bytes: &[u8]) -> Result<ServerMsg, WireError> {
        let mut r = Reader::new(bytes);
        let at = r.position();
        let tag = r.read_u8()?;
        let msg = match tag {
            server::WELCOME => ServerMsg::Welcome(Welcome {
                protocol: r.read_u16()?,
                undra_version: r.read_str()?.to_owned(),
                schema_hash: r.read_u64()?,
                platform: r.read_str()?.to_owned(),
                mode: r.read_str()?.to_owned(),
                core_epoch: r.read_u64()?,
                started_unix_ms: r.read_u64()?,
                ring_steps: r.read_u32()?,
                ring_bytes: r.read_u64()?,
                ring_step_bytes: r.read_u64()?,
                schema_json: r.read_str()?.to_owned(),
            }),
            server::STORES => {
                let count = r.read_count(12)?;
                let mut stores = Vec::with_capacity(count);
                for _ in 0..count {
                    stores.push(StoreRef {
                        handle: r.read_u64()?,
                        type_id: r.read_u32()?,
                    });
                }
                ServerMsg::Stores(stores)
            }
            server::CHANGE_SET => {
                let seq = r.read_u64()?;
                let at_ms = r.read_u64()?;
                let delivery = match r.read_u8()? {
                    0 => Delivery::Commit,
                    1 => Delivery::Initial,
                    tag => {
                        return Err(WireError::InvalidTag {
                            tag: u32::from(tag),
                            at: r.position() - 1,
                            ty: "Delivery",
                        });
                    }
                };
                let cause = Cause::decode(&mut r)?;
                ServerMsg::ChangeSet {
                    seq,
                    at_ms,
                    delivery,
                    cause,
                    payload: r.read_bytes()?.to_vec(),
                }
            }
            server::STEP => ServerMsg::Step(StepInfo {
                step: r.read_u32()?,
                through_seq: r.read_u64()?,
                txn: r.read_u64()?,
                at_ms: r.read_u64()?,
                bytes: r.read_u32()?,
                stores: r.read_u32()?,
                restorable: r.read_bool()?,
                restored_from: r.read_u32()?,
            }),
            server::EVICTED => ServerMsg::Evicted {
                below_step: r.read_u32()?,
            },
            server::PORT => {
                let phase = r.read_u8()?;
                let id = r.read_u32()?;
                let port_id = r.read_u32()?;
                let method_id = r.read_u32()?;
                let at_ms = r.read_u64()?;
                match phase {
                    0 => ServerMsg::Port(PortRecord::Start {
                        id,
                        port_id,
                        method_id,
                        at_ms,
                        args: r.read_bytes()?.to_vec(),
                    }),
                    1 => ServerMsg::Port(PortRecord::End {
                        id,
                        port_id,
                        method_id,
                        at_ms,
                        status: r.read_u8()?,
                        latency_us: r.read_u64()?,
                        reply: r.read_bytes()?.to_vec(),
                    }),
                    tag => {
                        return Err(WireError::InvalidTag {
                            tag: u32::from(tag),
                            at: r.position() - 1,
                            ty: "PortRecord",
                        });
                    }
                }
            }
            server::STATS => ServerMsg::Stats(r.read_str()?.to_owned()),
            server::QUERIES => ServerMsg::Queries {
                at_ms: r.read_u64()?,
                json: r.read_str()?.to_owned(),
            },
            server::TRAVELED => ServerMsg::Traveled(Traveled {
                request_id: r.read_u32()?,
                ok: r.read_bool()?,
                step: r.read_u32()?,
                dropped: r.read_u32()?,
                message: r.read_str()?.to_owned(),
            }),
            server::APP => ServerMsg::App {
                connected: r.read_bool()?,
                platform: r.read_str()?.to_owned(),
            },
            other => {
                return Err(WireError::InvalidTag {
                    tag: u32::from(other),
                    at,
                    ty: "ServerMsg",
                });
            }
        };
        r.finish()?;
        Ok(msg)
    }
}

/// Writes a `ChangeSet` message without building a [`ServerMsg`] (the hot path).
pub(crate) fn encode_change_set(
    w: &mut Writer,
    seq: u64,
    at_ms: u64,
    delivery: Delivery,
    cause: Cause,
    payload: &[u8],
) {
    w.reserve(32 + payload.len());
    w.write_u8(server::CHANGE_SET);
    w.write_u64(seq);
    w.write_u64(at_ms);
    w.write_u8(match delivery {
        Delivery::Commit => 0,
        Delivery::Initial => 1,
    });
    cause.encode(w);
    w.write_bytes(payload);
}

/// Writes the start of a port call.
pub(crate) fn encode_port_start(
    w: &mut Writer,
    id: u32,
    port_id: u32,
    method_id: u32,
    at_ms: u64,
    args: &[u8],
) {
    w.write_u8(server::PORT);
    w.write_u8(0);
    w.write_u32(id);
    w.write_u32(port_id);
    w.write_u32(method_id);
    w.write_u64(at_ms);
    w.write_bytes(args);
}

/// Writes the end of a port call.
#[allow(clippy::too_many_arguments)]
pub(crate) fn encode_port_end(
    w: &mut Writer,
    id: u32,
    port_id: u32,
    method_id: u32,
    at_ms: u64,
    status: u8,
    latency_us: u64,
    reply: &[u8],
) {
    w.write_u8(server::PORT);
    w.write_u8(1);
    w.write_u32(id);
    w.write_u32(port_id);
    w.write_u32(method_id);
    w.write_u64(at_ms);
    w.write_u8(status);
    w.write_u64(latency_us);
    w.write_bytes(reply);
}

/// A message from the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientMsg {
    /// Travel to a step: restore the core to the snapshot of that step.
    Restore {
        /// Echoed in the [`Traveled`] answer.
        request_id: u32,
        /// The step.
        step: u32,
    },
    /// Send the stores and their current values again (a page whose mirror is lost).
    Resync,
}

impl ClientMsg {
    /// Writes the message.
    pub fn encode(&self, w: &mut Writer) {
        match self {
            ClientMsg::Restore { request_id, step } => {
                w.write_u8(client::RESTORE);
                w.write_u32(*request_id);
                w.write_u32(*step);
            }
            ClientMsg::Resync => w.write_u8(client::RESYNC),
        }
    }

    /// Reads one message.
    ///
    /// # Errors
    ///
    /// The [`WireError`] of whatever does not decode, including an unknown tag.
    pub fn decode(bytes: &[u8]) -> Result<ClientMsg, WireError> {
        let mut r = Reader::new(bytes);
        let at = r.position();
        let msg = match r.read_u8()? {
            client::RESTORE => ClientMsg::Restore {
                request_id: r.read_u32()?,
                step: r.read_u32()?,
            },
            client::RESYNC => ClientMsg::Resync,
            other => {
                return Err(WireError::InvalidTag {
                    tag: u32::from(other),
                    at,
                    ty: "ClientMsg",
                });
            }
        };
        r.finish()?;
        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(msg: &ServerMsg) -> ServerMsg {
        let mut w = Writer::new();
        msg.encode(&mut w);
        ServerMsg::decode(w.as_slice()).expect("a message decodes")
    }

    #[test]
    fn every_server_message_round_trips() {
        let msgs = [
            ServerMsg::Welcome(Welcome {
                protocol: PROTOCOL,
                undra_version: "1.2.0".into(),
                schema_hash: 0xdead_beef_0123_4567,
                platform: "rust".into(),
                mode: "dev".into(),
                core_epoch: 9,
                started_unix_ms: 1_700_000_000_000,
                ring_steps: 200,
                ring_bytes: 32 << 20,
                ring_step_bytes: 4 << 20,
                schema_json: "{\"records\":[]}".into(),
            }),
            ServerMsg::Stores(vec![
                StoreRef { handle: 0x0000_0001_0000_0001, type_id: 7 },
                StoreRef { handle: 2, type_id: 8 },
            ]),
            ServerMsg::ChangeSet {
                seq: 5,
                at_ms: 1234,
                delivery: Delivery::Commit,
                cause: Cause::Call(0xabcd),
                payload: vec![1, 2, 3],
            },
            ServerMsg::ChangeSet {
                seq: 6,
                at_ms: 1235,
                delivery: Delivery::Initial,
                cause: Cause::Restore(3),
                payload: vec![],
            },
            ServerMsg::Step(StepInfo {
                step: 4,
                through_seq: 5,
                txn: 77,
                at_ms: 12,
                bytes: 2048,
                stores: 3,
                restorable: true,
                restored_from: 2,
            }),
            ServerMsg::Evicted { below_step: 3 },
            ServerMsg::Port(PortRecord::Start {
                id: 1,
                port_id: 2,
                method_id: 3,
                at_ms: 4,
                args: vec![9, 9],
            }),
            ServerMsg::Port(PortRecord::End {
                id: 1,
                port_id: 2,
                method_id: 3,
                at_ms: 5,
                status: 2,
                latency_us: 1500,
                reply: vec![],
            }),
            ServerMsg::Stats("{\"a\":1}".into()),
            ServerMsg::Queries {
                at_ms: 3,
                json: "{\"entries\":[]}".into(),
            },
            ServerMsg::Traveled(Traveled {
                request_id: 8,
                ok: false,
                step: 99,
                dropped: 0,
                message: "step 99 is gone".into(),
            }),
            ServerMsg::App {
                connected: true,
                platform: "web".into(),
            },
        ];
        for msg in &msgs {
            assert_eq!(&round(msg), msg);
        }
    }

    #[test]
    fn client_messages_round_trip() {
        for msg in [ClientMsg::Restore { request_id: 3, step: 12 }, ClientMsg::Resync] {
            let mut w = Writer::new();
            msg.encode(&mut w);
            assert_eq!(ClientMsg::decode(w.as_slice()), Ok(msg));
        }
    }

    #[test]
    fn unknown_tags_and_trailing_bytes_are_errors_not_panics() {
        assert!(ServerMsg::decode(&[]).is_err());
        assert!(ServerMsg::decode(&[0xee]).is_err());
        assert!(ClientMsg::decode(&[0xee]).is_err());
        assert!(ClientMsg::decode(&[client::RESYNC, 0]).is_err());
        let mut w = Writer::new();
        ServerMsg::Evicted { below_step: 1 }.encode(&mut w);
        let mut bytes = w.into_vec();
        bytes.push(0);
        assert!(ServerMsg::decode(&bytes).is_err());
    }

    #[test]
    fn random_bytes_never_panic_the_decoders() {
        // A cheap deterministic sweep; the byte-fuzz of the wire crate covers the primitives.
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        for len in 0..200_usize {
            let bytes: Vec<u8> = (0..len)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    state.to_le_bytes()[0]
                })
                .collect();
            let _ = ServerMsg::decode(&bytes);
            let _ = ClientMsg::decode(&bytes);
        }
    }

    // ----- vectors shared with the page (runtimes/ts/devtools/test/vectors.json) --------------

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn server_vectors() -> Vec<(&'static str, ServerMsg)> {
        vec![
            (
                "welcome",
                ServerMsg::Welcome(Welcome {
                    protocol: PROTOCOL,
                    undra_version: "1.2.0".into(),
                    schema_hash: 0x0123_4567_89ab_cdef,
                    platform: "rust".into(),
                    mode: "dev".into(),
                    core_epoch: 77,
                    started_unix_ms: 1_790_000_000_000,
                    ring_steps: 200,
                    ring_bytes: 32 << 20,
                    ring_step_bytes: 4 << 20,
                    schema_json: "{\"records\":[]}".into(),
                }),
            ),
            (
                "stores",
                ServerMsg::Stores(vec![
                    StoreRef { handle: 0x0000_0002_0000_0001, type_id: 7 },
                    StoreRef { handle: 5, type_id: 9 },
                ]),
            ),
            (
                "change_set_commit_call",
                ServerMsg::ChangeSet {
                    seq: 12,
                    at_ms: 3456,
                    delivery: Delivery::Commit,
                    cause: Cause::Call(0xdead_beef),
                    payload: vec![1, 2, 3, 4],
                },
            ),
            (
                "change_set_initial_restore",
                ServerMsg::ChangeSet {
                    seq: 13,
                    at_ms: 4000,
                    delivery: Delivery::Initial,
                    cause: Cause::Restore(3),
                    payload: vec![],
                },
            ),
            (
                "step",
                ServerMsg::Step(StepInfo {
                    step: 4,
                    through_seq: 13,
                    txn: 99,
                    at_ms: 4001,
                    bytes: 2048,
                    stores: 3,
                    restorable: true,
                    restored_from: 2,
                }),
            ),
            ("evicted", ServerMsg::Evicted { below_step: 3 }),
            (
                "port_start",
                ServerMsg::Port(PortRecord::Start {
                    id: 8,
                    port_id: 0xaabb_ccdd,
                    method_id: 0x1122_3344,
                    at_ms: 5000,
                    args: vec![9, 8, 7],
                }),
            ),
            (
                "port_end",
                ServerMsg::Port(PortRecord::End {
                    id: 8,
                    port_id: 0xaabb_ccdd,
                    method_id: 0x1122_3344,
                    at_ms: 5042,
                    status: 1,
                    latency_us: 42_000,
                    reply: vec![5],
                }),
            ),
            ("stats", ServerMsg::Stats("{\"at_ms\":1}".into())),
            ("queries", ServerMsg::Queries { at_ms: 6000, json: "{\"entries\":[]}".into() }),
            (
                "traveled",
                ServerMsg::Traveled(Traveled {
                    request_id: 7,
                    ok: true,
                    step: 3,
                    dropped: 1,
                    message: "restored step 3".into(),
                }),
            ),
            ("app", ServerMsg::App { connected: true, platform: "web".into() }),
        ]
    }

    /// The bytes the Rust server writes are the bytes the page's decoder is tested against, and
    /// the bytes the page writes are the ones the Rust server's decoder is tested against.
    #[test]
    fn the_vectors_the_page_is_tested_with_are_the_ones_the_server_writes() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../runtimes/ts/devtools/test/vectors.json");
        let server: Vec<serde_json::Value> = server_vectors()
            .iter()
            .map(|(name, msg)| {
                let mut w = Writer::new();
                msg.encode(&mut w);
                serde_json::json!({ "name": name, "hex": hex(w.as_slice()) })
            })
            .collect();
        let mut client = Vec::new();
        for (name, msg) in [
            ("restore", ClientMsg::Restore { request_id: 7, step: 3 }),
            ("resync", ClientMsg::Resync),
        ] {
            let mut w = Writer::new();
            msg.encode(&mut w);
            client.push(serde_json::json!({ "name": name, "hex": hex(w.as_slice()) }));
        }
        let doc = serde_json::to_string_pretty(&serde_json::json!({ "server": server, "client": client })).unwrap() + "\n";
        if std::env::var_os("UNDRA_UPDATE_VECTORS").is_some() {
            std::fs::write(path, &doc).unwrap();
        }
        let Ok(on_disk) = std::fs::read_to_string(path) else {
            return; // a packaged crate has no page beside it
        };
        assert_eq!(on_disk, doc, "run with UNDRA_UPDATE_VECTORS=1 to rewrite {path}");
        // And the other way: what the page encodes decodes here.
        let parsed: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        for entry in parsed["client"].as_array().unwrap() {
            let bytes: Vec<u8> = (0..entry["hex"].as_str().unwrap().len() / 2)
                .map(|i| u8::from_str_radix(&entry["hex"].as_str().unwrap()[2 * i..2 * i + 2], 16).unwrap())
                .collect();
            let expected = match entry["name"].as_str().unwrap() {
                "restore" => ClientMsg::Restore { request_id: 7, step: 3 },
                _ => ClientMsg::Resync,
            };
            assert_eq!(ClientMsg::decode(&bytes), Ok(expected));
        }
    }
}
