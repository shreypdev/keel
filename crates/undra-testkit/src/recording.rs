//! The recording format: one versioned JSON document for what crossed the boundary in a session
//! (`undra.recording`, version 1; `docs/TESTING.md`, ADR-055).
//!
//! A recording is the session's events in order, each stamped with `t`, the whole milliseconds
//! since the session started. Payloads are lower-case hex of the bytes SPEC 3 defines, handles are
//! `"0x.."` strings (a u64 does not fit a JavaScript number) and ids are numbers.
//!
//! ```json
//! {
//!   "format": "undra.recording",
//!   "version": 1,
//!   "schema_hash": "0x00000000000000ff",
//!   "source": "dev-server",
//!   "events": [
//!   {"t":0,"kind":"port_call","port":1,"method":2,"call":1,"args":"","name":"Http.request"},
//!   {"t":4,"kind":"port_reply","call":1,"status":"ok","body":"00"}
//!   ]
//! }
//! ```
//!
//! The writer ([`Recording::to_json`]) is canonical: fixed key order, one event per line, nothing
//! that depends on the machine or the clock, so equal recordings are equal bytes. Every platform's
//! writer is held to the same bytes by the fixtures under `testkit/fixtures/`.

use core::fmt;
use std::fmt::Write as _;

use serde_json::Value;
use undra_wire::payload::{ChangeOp, PortStatus, ReplyStatus, StreamFlag};

use crate::hex;
use crate::names::standard_name;

/// The `format` of a recording.
pub const FORMAT: &str = "undra.recording";

/// The `version` this crate reads and writes.
pub const VERSION: u64 = 1;

/// Which call a [`EventKind::Call`] is (SPEC 3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A free function.
    Function {
        /// `fnv1a32("fn.<name>")`.
        method: u32,
    },
    /// A method of the object `handle`.
    Method {
        /// The receiver.
        handle: u64,
        /// `fnv1a32("<Type>.<method>")`.
        method: u32,
    },
    /// A constructor.
    Constructor {
        /// The object type.
        type_id: u32,
        /// Which constructor.
        method: u32,
    },
    /// A page of a lazy list.
    LazyPage {
        /// The lazy-list object.
        handle: u64,
        /// The first item.
        offset: u32,
        /// How many items.
        limit: u32,
    },
}

/// One signal update of a [`EventKind::ChangeSet`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The store.
    pub handle: u64,
    /// The signal within it.
    pub signal: u32,
    /// How `value` reads.
    pub op: ChangeOp,
    /// The encoded value.
    pub value: Vec<u8>,
}

/// What happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// Host to core: a call.
    Call {
        /// What is called.
        target: Target,
        /// The host's call id.
        call: u32,
        /// The encoded parameters (empty for a lazy page).
        args: Vec<u8>,
    },
    /// Core to host: a call's reply.
    Reply {
        /// The call.
        call: u32,
        /// How it ended.
        status: ReplyStatus,
        /// The reply body.
        body: Vec<u8>,
    },
    /// Core to host: one transaction's updates.
    ChangeSet {
        /// The transaction id.
        txn: u64,
        /// The updates.
        entries: Vec<Entry>,
    },
    /// Core to host: an item, end or failure of a stream.
    StreamItem {
        /// The stream's call.
        call: u32,
        /// What the item is.
        flag: StreamFlag,
        /// The body.
        body: Vec<u8>,
    },
    /// Core to host: the core calls a port.
    PortCall {
        /// The port id.
        port: u32,
        /// The method id.
        method: u32,
        /// The core's id for the call.
        call: u32,
        /// The encoded parameters.
        args: Vec<u8>,
    },
    /// Host to core: the answer to a port call.
    PortReply {
        /// The port call.
        call: u32,
        /// How it ended (`unavailable` is a recorded outcome like any other).
        status: PortStatus,
        /// The reply body.
        body: Vec<u8>,
    },
    /// Host to core: an event of an event port (`Connectivity.changed`, ...).
    PortEvent {
        /// The port id.
        port: u32,
        /// The method id.
        method: u32,
        /// The encoded parameters.
        payload: Vec<u8>,
    },
    /// Host to core: a platform timer fired.
    TimerFired {
        /// The timer.
        timer: u32,
    },
    /// Host to core: start or stop observing a signal (`u32::MAX` is every signal).
    Observe {
        /// The store.
        handle: u64,
        /// The signal.
        signal: u32,
        /// On or off.
        on: bool,
    },
    /// Host to core: an object was released.
    Release {
        /// The object.
        handle: u64,
    },
    /// Host to core: a call or stream was cancelled.
    Cancel {
        /// The call.
        call: u32,
    },
}

/// One event and when it happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// Whole milliseconds since the session started.
    pub t: u64,
    /// What happened.
    pub kind: EventKind,
}

/// A recorded session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recording {
    /// The schema hash of the core the bytes belong to: a replay on another schema is refused.
    pub schema_hash: u64,
    /// Where it came from, informational: `"dev-server"`, `"adapters"`, `"test"`, `"hand"`.
    pub source: String,
    /// The platform of the host, informational.
    pub platform: Option<String>,
    /// The events, in the order they happened.
    pub events: Vec<Event>,
}

/// Why a recording could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordingError {
    /// The text is not JSON.
    Json(String),
    /// `format` is not [`FORMAT`].
    WrongFormat(String),
    /// `version` is not one this crate reads.
    UnsupportedVersion(u64),
    /// A field is missing or has the wrong type or value: `event` is the index of the event
    /// (`None` for the document), `field` its name.
    Field {
        /// Index of the event, or `None` for the document header.
        event: Option<usize>,
        /// The field.
        field: &'static str,
        /// What is wrong with it.
        problem: String,
    },
    /// A payload handed to the recorder does not decode (SPEC 3).
    Payload(String),
}

impl fmt::Display for RecordingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecordingError::Json(e) => write!(f, "the recording is not valid JSON: {e}"),
            RecordingError::WrongFormat(got) => {
                write!(f, "not a recording: format is {got:?}, expected {FORMAT:?}")
            }
            RecordingError::UnsupportedVersion(v) => write!(
                f,
                "recording version {v} is not supported (this reader knows version {VERSION})"
            ),
            RecordingError::Field {
                event: Some(i),
                field,
                problem,
            } => write!(f, "event {i}: \"{field}\" {problem}"),
            RecordingError::Field {
                event: None,
                field,
                problem,
            } => write!(f, "\"{field}\" {problem}"),
            RecordingError::Payload(e) => write!(f, "a recorded payload does not decode: {e}"),
        }
    }
}

impl std::error::Error for RecordingError {}

fn reply_status_name(s: ReplyStatus) -> &'static str {
    match s {
        ReplyStatus::Ok => "ok",
        ReplyStatus::Error => "error",
        ReplyStatus::Panic => "panic",
        ReplyStatus::Cancelled => "cancelled",
        ReplyStatus::StreamOpened => "stream_opened",
        ReplyStatus::BadRequest => "bad_request",
    }
}

fn reply_status_from(name: &str) -> Option<ReplyStatus> {
    [
        ReplyStatus::Ok,
        ReplyStatus::Error,
        ReplyStatus::Panic,
        ReplyStatus::Cancelled,
        ReplyStatus::StreamOpened,
        ReplyStatus::BadRequest,
    ]
    .into_iter()
    .find(|s| reply_status_name(*s) == name)
}

fn port_status_name(s: PortStatus) -> &'static str {
    match s {
        PortStatus::Ok => "ok",
        PortStatus::Error => "error",
        PortStatus::Unavailable => "unavailable",
    }
}

fn port_status_from(name: &str) -> Option<PortStatus> {
    [PortStatus::Ok, PortStatus::Error, PortStatus::Unavailable]
        .into_iter()
        .find(|s| port_status_name(*s) == name)
}

fn flag_name(f: StreamFlag) -> &'static str {
    match f {
        StreamFlag::Item => "item",
        StreamFlag::End => "end",
        StreamFlag::Error => "error",
        StreamFlag::Failed => "failed",
    }
}

fn flag_from(name: &str) -> Option<StreamFlag> {
    [
        StreamFlag::Item,
        StreamFlag::End,
        StreamFlag::Error,
        StreamFlag::Failed,
    ]
    .into_iter()
    .find(|f| flag_name(*f) == name)
}

fn op_name(op: ChangeOp) -> &'static str {
    match op {
        ChangeOp::Full => "full",
        ChangeOp::KeyedPatch => "patch",
        ChangeOp::LazyInvalidated => "lazy_invalidated",
    }
}

fn op_from(name: &str) -> Option<ChangeOp> {
    [
        ChangeOp::Full,
        ChangeOp::KeyedPatch,
        ChangeOp::LazyInvalidated,
    ]
    .into_iter()
    .find(|o| op_name(*o) == name)
}

/// Appends `text` as a JSON string. Only `"`, `\` and control characters are escaped, so the
/// output is the same in every language's writer.
fn push_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

impl Event {
    /// The event as one line of canonical JSON.
    pub fn to_json_line(&self) -> String {
        let mut o = String::new();
        let _ = write!(o, "{{\"t\":{}", self.t);
        match &self.kind {
            EventKind::Call { target, call, args } => {
                o.push_str(",\"kind\":\"call\"");
                match target {
                    Target::Function { method } => {
                        let _ = write!(o, ",\"target\":\"function\",\"method\":{method}");
                    }
                    Target::Method { handle, method } => {
                        let _ = write!(
                            o,
                            ",\"target\":\"method\",\"handle\":\"{handle:#018x}\",\"method\":{method}"
                        );
                    }
                    Target::Constructor { type_id, method } => {
                        let _ = write!(
                            o,
                            ",\"target\":\"constructor\",\"type\":{type_id},\"method\":{method}"
                        );
                    }
                    Target::LazyPage {
                        handle,
                        offset,
                        limit,
                    } => {
                        let _ = write!(
                            o,
                            ",\"target\":\"page\",\"handle\":\"{handle:#018x}\",\"offset\":{offset},\"limit\":{limit}"
                        );
                    }
                }
                let _ = write!(o, ",\"call\":{call},\"args\":\"{}\"", hex::encode(args));
            }
            EventKind::Reply { call, status, body } => {
                let _ = write!(
                    o,
                    ",\"kind\":\"reply\",\"call\":{call},\"status\":\"{}\",\"body\":\"{}\"",
                    reply_status_name(*status),
                    hex::encode(body)
                );
            }
            EventKind::ChangeSet { txn, entries } => {
                let _ = write!(o, ",\"kind\":\"change_set\",\"txn\":{txn},\"entries\":[");
                for (i, e) in entries.iter().enumerate() {
                    if i > 0 {
                        o.push(',');
                    }
                    let _ = write!(
                        o,
                        "{{\"handle\":\"{:#018x}\",\"signal\":{},\"op\":\"{}\",\"value\":\"{}\"}}",
                        e.handle,
                        e.signal,
                        op_name(e.op),
                        hex::encode(&e.value)
                    );
                }
                o.push(']');
            }
            EventKind::StreamItem { call, flag, body } => {
                let _ = write!(
                    o,
                    ",\"kind\":\"stream_item\",\"call\":{call},\"flag\":\"{}\",\"body\":\"{}\"",
                    flag_name(*flag),
                    hex::encode(body)
                );
            }
            EventKind::PortCall {
                port,
                method,
                call,
                args,
            } => {
                let _ = write!(
                    o,
                    ",\"kind\":\"port_call\",\"port\":{port},\"method\":{method},\"call\":{call},\"args\":\"{}\"",
                    hex::encode(args)
                );
                push_name(&mut o, *port, *method);
            }
            EventKind::PortReply { call, status, body } => {
                let _ = write!(
                    o,
                    ",\"kind\":\"port_reply\",\"call\":{call},\"status\":\"{}\",\"body\":\"{}\"",
                    port_status_name(*status),
                    hex::encode(body)
                );
            }
            EventKind::PortEvent {
                port,
                method,
                payload,
            } => {
                let _ = write!(
                    o,
                    ",\"kind\":\"event\",\"port\":{port},\"method\":{method},\"payload\":\"{}\"",
                    hex::encode(payload)
                );
                push_name(&mut o, *port, *method);
            }
            EventKind::TimerFired { timer } => {
                let _ = write!(o, ",\"kind\":\"timer_fired\",\"timer\":{timer}");
            }
            EventKind::Observe { handle, signal, on } => {
                let _ = write!(
                    o,
                    ",\"kind\":\"observe\",\"handle\":\"{handle:#018x}\",\"signal\":{signal},\"on\":{on}"
                );
            }
            EventKind::Release { handle } => {
                let _ = write!(o, ",\"kind\":\"release\",\"handle\":\"{handle:#018x}\"");
            }
            EventKind::Cancel { call } => {
                let _ = write!(o, ",\"kind\":\"cancel\",\"call\":{call}");
            }
        }
        o.push('}');
        o
    }
}

fn push_name(o: &mut String, port: u32, method: u32) {
    if let Some(name) = standard_name(port, method) {
        o.push_str(",\"name\":");
        push_string(o, &name);
    }
}

impl Recording {
    /// An empty recording for a core with `schema_hash`.
    pub fn new(schema_hash: u64, source: impl Into<String>) -> Recording {
        Recording {
            schema_hash,
            source: source.into(),
            platform: None,
            events: Vec::new(),
        }
    }

    /// The canonical JSON text of the recording (ends with a newline).
    pub fn to_json(&self) -> String {
        let mut o = String::new();
        o.push_str("{\n");
        let _ = writeln!(o, "  \"format\": \"{FORMAT}\",");
        let _ = writeln!(o, "  \"version\": {VERSION},");
        let _ = writeln!(o, "  \"schema_hash\": \"{:#018x}\",", self.schema_hash);
        o.push_str("  \"source\": ");
        push_string(&mut o, &self.source);
        o.push_str(",\n");
        if let Some(platform) = &self.platform {
            o.push_str("  \"platform\": ");
            push_string(&mut o, platform);
            o.push_str(",\n");
        }
        if self.events.is_empty() {
            o.push_str("  \"events\": []\n}\n");
            return o;
        }
        o.push_str("  \"events\": [\n");
        for (i, event) in self.events.iter().enumerate() {
            o.push_str("    ");
            o.push_str(&event.to_json_line());
            o.push_str(if i + 1 < self.events.len() {
                ",\n"
            } else {
                "\n"
            });
        }
        o.push_str("  ]\n}\n");
        o
    }

    /// Reads a recording.
    ///
    /// # Errors
    ///
    /// [`RecordingError`] for text that is not JSON, another `format`, a `version` newer than
    /// this reader, or a field that is missing or malformed (named, with the event's index).
    pub fn from_json(text: &str) -> Result<Recording, RecordingError> {
        let doc: Value =
            serde_json::from_str(text).map_err(|e| RecordingError::Json(e.to_string()))?;
        let format = doc.get("format").and_then(Value::as_str).unwrap_or("");
        if format != FORMAT {
            return Err(RecordingError::WrongFormat(format.to_owned()));
        }
        let version = doc.get("version").and_then(Value::as_u64).unwrap_or(0);
        if version != VERSION {
            return Err(RecordingError::UnsupportedVersion(version));
        }
        let head = |field: &'static str, problem: &str| RecordingError::Field {
            event: None,
            field,
            problem: problem.to_owned(),
        };
        let schema_hash = doc
            .get("schema_hash")
            .and_then(Value::as_str)
            .and_then(parse_hex_u64)
            .ok_or_else(|| head("schema_hash", "must be a \"0x..\" string"))?;
        let source = doc
            .get("source")
            .and_then(Value::as_str)
            .ok_or_else(|| head("source", "must be a string"))?
            .to_owned();
        let platform = match doc.get("platform") {
            None | Some(Value::Null) => None,
            Some(Value::String(p)) => Some(p.clone()),
            Some(_) => return Err(head("platform", "must be a string")),
        };
        let events = doc
            .get("events")
            .and_then(Value::as_array)
            .ok_or_else(|| head("events", "must be an array"))?
            .iter()
            .enumerate()
            .map(|(i, e)| parse_event(i, e))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Recording {
            schema_hash,
            source,
            platform,
            events,
        })
    }
}

fn parse_hex_u64(text: &str) -> Option<u64> {
    u64::from_str_radix(text.strip_prefix("0x")?, 16).ok()
}

/// A typed view of one event object, reporting the first bad field.
struct Fields<'a> {
    index: usize,
    obj: &'a Value,
}

impl Fields<'_> {
    fn bad(&self, field: &'static str, problem: &str) -> RecordingError {
        RecordingError::Field {
            event: Some(self.index),
            field,
            problem: problem.to_owned(),
        }
    }

    fn u64(&self, field: &'static str) -> Result<u64, RecordingError> {
        self.obj
            .get(field)
            .and_then(Value::as_u64)
            .ok_or_else(|| self.bad(field, "must be a non-negative integer"))
    }

    fn u32(&self, field: &'static str) -> Result<u32, RecordingError> {
        u32::try_from(self.u64(field)?).map_err(|_| self.bad(field, "does not fit a u32"))
    }

    fn str(&self, field: &'static str) -> Result<&str, RecordingError> {
        self.obj
            .get(field)
            .and_then(Value::as_str)
            .ok_or_else(|| self.bad(field, "must be a string"))
    }

    fn handle(&self, field: &'static str) -> Result<u64, RecordingError> {
        parse_hex_u64(self.str(field)?).ok_or_else(|| self.bad(field, "must be a \"0x..\" string"))
    }

    fn bytes(&self, field: &'static str) -> Result<Vec<u8>, RecordingError> {
        hex::decode(self.str(field)?).ok_or_else(|| self.bad(field, "must be hex"))
    }

    fn bool(&self, field: &'static str) -> Result<bool, RecordingError> {
        self.obj
            .get(field)
            .and_then(Value::as_bool)
            .ok_or_else(|| self.bad(field, "must be true or false"))
    }
}

fn parse_event(index: usize, obj: &Value) -> Result<Event, RecordingError> {
    let f = Fields { index, obj };
    let t = f.u64("t")?;
    let kind = match f.str("kind")? {
        "call" => {
            let target = match f.str("target")? {
                "function" => Target::Function {
                    method: f.u32("method")?,
                },
                "method" => Target::Method {
                    handle: f.handle("handle")?,
                    method: f.u32("method")?,
                },
                "constructor" => Target::Constructor {
                    type_id: f.u32("type")?,
                    method: f.u32("method")?,
                },
                "page" => Target::LazyPage {
                    handle: f.handle("handle")?,
                    offset: f.u32("offset")?,
                    limit: f.u32("limit")?,
                },
                _ => return Err(f.bad("target", "is not function, method, constructor or page")),
            };
            EventKind::Call {
                target,
                call: f.u32("call")?,
                args: f.bytes("args")?,
            }
        }
        "reply" => EventKind::Reply {
            call: f.u32("call")?,
            status: reply_status_from(f.str("status")?)
                .ok_or_else(|| f.bad("status", "is not a reply status"))?,
            body: f.bytes("body")?,
        },
        "change_set" => {
            let entries = obj
                .get("entries")
                .and_then(Value::as_array)
                .ok_or_else(|| f.bad("entries", "must be an array"))?
                .iter()
                .map(|e| {
                    let ef = Fields { index, obj: e };
                    Ok(Entry {
                        handle: ef.handle("handle")?,
                        signal: ef.u32("signal")?,
                        op: op_from(ef.str("op")?).ok_or_else(|| {
                            ef.bad("op", "is not full, patch or lazy_invalidated")
                        })?,
                        value: ef.bytes("value")?,
                    })
                })
                .collect::<Result<Vec<_>, RecordingError>>()?;
            EventKind::ChangeSet {
                txn: f.u64("txn")?,
                entries,
            }
        }
        "stream_item" => EventKind::StreamItem {
            call: f.u32("call")?,
            flag: flag_from(f.str("flag")?)
                .ok_or_else(|| f.bad("flag", "is not item, end, error or failed"))?,
            body: f.bytes("body")?,
        },
        "port_call" => EventKind::PortCall {
            port: f.u32("port")?,
            method: f.u32("method")?,
            call: f.u32("call")?,
            args: f.bytes("args")?,
        },
        "port_reply" => EventKind::PortReply {
            call: f.u32("call")?,
            status: port_status_from(f.str("status")?)
                .ok_or_else(|| f.bad("status", "is not ok, error or unavailable"))?,
            body: f.bytes("body")?,
        },
        "event" => EventKind::PortEvent {
            port: f.u32("port")?,
            method: f.u32("method")?,
            payload: f.bytes("payload")?,
        },
        "timer_fired" => EventKind::TimerFired {
            timer: f.u32("timer")?,
        },
        "observe" => EventKind::Observe {
            handle: f.handle("handle")?,
            signal: f.u32("signal")?,
            on: f.bool("on")?,
        },
        "release" => EventKind::Release {
            handle: f.handle("handle")?,
        },
        "cancel" => EventKind::Cancel {
            call: f.u32("call")?,
        },
        _ => return Err(f.bad("kind", "is not a known event kind")),
    };
    Ok(Event { t, kind })
}

/// One event of every kind, for the fixtures that pin the canonical writer (`testkit/fixtures/recording-all-kinds.json`).
#[doc(hidden)]
pub fn every_kind() -> Recording {
    let mut r = Recording::new(0xdead_beef, "test");
    r.platform = Some("ios".to_owned());
    let kinds = vec![
        EventKind::Call {
            target: Target::Constructor {
                type_id: 7,
                method: 8,
            },
            call: 1,
            args: vec![],
        },
        EventKind::Reply {
            call: 1,
            status: ReplyStatus::Ok,
            body: 0x0000_0001_0000_0001_u64.to_le_bytes().to_vec(),
        },
        EventKind::Call {
            target: Target::Method {
                handle: 0x1_0000_0001,
                method: 9,
            },
            call: 2,
            args: vec![1, 2, 3],
        },
        EventKind::Call {
            target: Target::Function { method: 10 },
            call: 3,
            args: vec![],
        },
        EventKind::Call {
            target: Target::LazyPage {
                handle: 0x2_0000_0003,
                offset: 0,
                limit: 50,
            },
            call: 4,
            args: vec![],
        },
        EventKind::Observe {
            handle: 0x1_0000_0001,
            signal: u32::MAX,
            on: true,
        },
        EventKind::ChangeSet {
            txn: 5,
            entries: vec![
                Entry {
                    handle: 0x1_0000_0001,
                    signal: 0,
                    op: ChangeOp::Full,
                    value: vec![0xab],
                },
                Entry {
                    handle: 0x1_0000_0001,
                    signal: 1,
                    op: ChangeOp::KeyedPatch,
                    value: vec![],
                },
                Entry {
                    handle: 0x2_0000_0003,
                    signal: 2,
                    op: ChangeOp::LazyInvalidated,
                    value: vec![],
                },
            ],
        },
        EventKind::StreamItem {
            call: 3,
            flag: StreamFlag::Item,
            body: vec![9],
        },
        EventKind::StreamItem {
            call: 3,
            flag: StreamFlag::End,
            body: vec![],
        },
        EventKind::PortCall {
            port: undra_meta::ids::port_id("Http"),
            method: undra_meta::ids::port_method_id("Http", "request"),
            call: 1,
            args: vec![4, 5],
        },
        EventKind::PortReply {
            call: 1,
            status: PortStatus::Unavailable,
            body: vec![],
        },
        EventKind::PortCall {
            port: 99,
            method: 100,
            call: 2,
            args: vec![],
        },
        EventKind::PortEvent {
            port: undra_meta::ids::port_id("Connectivity"),
            method: undra_meta::ids::port_method_id("Connectivity", "changed"),
            payload: vec![1, 0, 0],
        },
        EventKind::TimerFired { timer: 6 },
        EventKind::Release {
            handle: 0x1_0000_0001,
        },
        EventKind::Cancel { call: 2 },
        EventKind::Reply {
            call: 2,
            status: ReplyStatus::Cancelled,
            body: vec![],
        },
    ];
    for (i, kind) in kinds.into_iter().enumerate() {
        r.events.push(Event {
            t: (i as u64) * 3,
            kind,
        });
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_round_trips_through_canonical_json() {
        let original = every_kind();
        let text = original.to_json();
        let back = Recording::from_json(&text).unwrap();
        assert_eq!(back, original);
        assert_eq!(back.to_json(), text, "the writer is canonical");
    }

    #[test]
    fn the_layout_is_the_documented_one() {
        let mut r = Recording::new(255, "hand");
        r.events.push(Event {
            t: 0,
            kind: EventKind::Cancel { call: 1 },
        });
        assert_eq!(
            r.to_json(),
            "{\n  \"format\": \"undra.recording\",\n  \"version\": 1,\n  \"schema_hash\": \"0x00000000000000ff\",\n  \"source\": \"hand\",\n  \"events\": [\n    {\"t\":0,\"kind\":\"cancel\",\"call\":1}\n  ]\n}\n"
        );
        assert!(
            Recording::new(1, "x")
                .to_json()
                .ends_with("\"events\": []\n}\n")
        );
    }

    #[test]
    fn standard_ports_carry_a_name_and_others_do_not() {
        let text = every_kind().to_json();
        assert!(text.contains("\"name\":\"Http.request\""));
        assert!(text.contains("\"name\":\"Connectivity.changed\""));
        assert_eq!(text.matches("\"name\"").count(), 2);
    }

    #[test]
    fn bad_documents_are_typed_errors_naming_the_field() {
        let e = |text: &str| Recording::from_json(text).unwrap_err();
        assert!(matches!(e("nope"), RecordingError::Json(_)));
        assert!(matches!(
            e(r#"{"format":"other","version":1}"#),
            RecordingError::WrongFormat(_)
        ));
        assert_eq!(
            e(r#"{"format":"undra.recording","version":2}"#),
            RecordingError::UnsupportedVersion(2)
        );
        let head =
            r#"{"format":"undra.recording","version":1,"schema_hash":"0x1","source":"t","events":"#;
        let bad = |events: &str| e(&format!("{head}{events}}}"));
        let RecordingError::Field { event, field, .. } =
            bad(r#"[{"t":0,"kind":"cancel","call":"x"}]"#)
        else {
            panic!("expected a field error");
        };
        assert_eq!((event, field), (Some(0), "call"));
        assert!(matches!(
            bad(r#"[{"t":0,"kind":"warp"}]"#),
            RecordingError::Field { field: "kind", .. }
        ));
        assert!(matches!(
            bad(r#"[{"t":0,"kind":"reply","call":1,"status":"ok","body":"zz"}]"#),
            RecordingError::Field { field: "body", .. }
        ));
        assert!(
            !bad(r#"[{"t":0,"kind":"cancel","call":1}"#)
                .to_string()
                .is_empty(),
            "an unterminated document is a JSON error"
        );
    }

    #[test]
    fn strings_are_escaped_only_where_json_requires() {
        let mut r = Recording::new(1, "a\"b\\c\nd\u{1}é");
        r.platform = None;
        let text = r.to_json();
        assert!(text.contains(r#""source": "a\"b\\c\nd\u0001é""#));
        assert_eq!(Recording::from_json(&text).unwrap(), r);
    }
}
