//! The query cache as one JSON document, for `undra dev`'s devtools page (ADR-054).
//!
//! Registered with the runtime as the inspector `"queries"` the first time the client starts.
//! The document is a view of the cache at one moment; it never changes it, and the core never
//! reads it. Values are hex of their wire encoding: the page decodes them with the schema (the
//! query's `returns`), exactly as it decodes a signal.

use std::fmt::Write as _;

use crate::erased::Erased;
use crate::shared::{Entry, Shared};

/// Entries whose encoded value is larger than this are listed without it (`data` is `null`,
/// `data_len` says how big it is): a 10,000-row result must not make every sample megabytes.
const MAX_VALUE_BYTES: usize = 256 * 1024;

fn hex(bytes: &[u8], out: &mut String) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    out.reserve(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
}

fn json_str(text: &str, out: &mut String) {
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

fn value(label: &str, bytes: Option<&[u8]>, out: &mut String) {
    let _ = write!(out, ",\"{label}\":");
    match bytes {
        Some(b) if b.len() <= MAX_VALUE_BYTES => {
            out.push('"');
            hex(b, out);
            out.push('"');
        }
        _ => out.push_str("null"),
    }
    let _ = write!(out, ",\"{label}_len\":{}", bytes.map_or(0, <[u8]>::len));
}

/// What one entry shows, copied out of the cache so the document is built after its lock is
/// released: the strings are small, and the values are shared (`Arc`), not copied.
struct Row {
    id: u32,
    rendered: String,
    template: &'static str,
    status: &'static str,
    fetching: bool,
    observers: u32,
    invalidated: bool,
    failed: bool,
    layers: usize,
    stamp: u64,
    updated_at: Option<i64>,
    data: Option<Erased>,
    error: Option<Erased>,
}

impl Row {
    fn of(e: &Entry) -> Row {
        Row {
            id: e.vt.id,
            rendered: e.rendered.clone(),
            template: e.vt.key,
            status: match e.status() {
                crate::QueryStatus::Idle => "idle",
                crate::QueryStatus::Fetching => "fetching",
                crate::QueryStatus::Success => "success",
                crate::QueryStatus::Error => "error",
            },
            fetching: e.inflight.is_some(),
            observers: e.observers,
            invalidated: e.invalidated,
            failed: e.failed,
            layers: e.layers.len(),
            stamp: e.stamp,
            updated_at: e.updated_at,
            data: e.data.clone(),
            error: e.error.clone(),
        }
    }

    fn write(&self, out: &mut String) {
        let _ = write!(out, "{{\"query_id\":{},\"key\":", self.id);
        json_str(&self.rendered, out);
        out.push_str(",\"template\":");
        json_str(self.template, out);
        let _ = write!(
            out,
            ",\"status\":\"{}\",\"fetching\":{},\"observers\":{},\"invalidated\":{},\"failed\":{},\"layers\":{},\"stamp\":{}",
            self.status,
            self.fetching,
            self.observers,
            self.invalidated,
            self.failed,
            self.layers,
            self.stamp,
        );
        match self.updated_at {
            Some(at) => {
                let _ = write!(out, ",\"updated_at\":{at}");
            }
            None => out.push_str(",\"updated_at\":null"),
        }
        value("data", self.data.as_ref().map(|d| &*d.bytes), out);
        value("error", self.error.as_ref().map(|d| &*d.bytes), out);
        out.push('}');
    }
}

/// The cache of `shared` as JSON, entries ordered by query id then key. The cache's lock is held
/// only while the rows are copied out (a `String` and a few numbers each); on ten thousand entries
/// formatting takes several times longer than that, and no query operation waits for it.
pub(crate) fn describe(shared: &Shared) -> String {
    let pending = shared.queue_len();
    let mut rows: Vec<Row> = {
        let state = shared.state.lock();
        state.entries.values().map(Row::of).collect()
    };
    rows.sort_by(|a, b| (a.id, &a.rendered).cmp(&(b.id, &b.rendered)));
    let mut out = String::with_capacity(512 + rows.len() * 160);
    let _ = write!(
        out,
        "{{\"online\":{},\"gc_ms\":{},\"pending_mutations\":{pending},\"entries\":[",
        shared.online.load(std::sync::atomic::Ordering::Relaxed),
        shared.gc_ms.load(std::sync::atomic::Ordering::Relaxed),
    );
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        row.write(&mut out);
    }
    out.push_str("]}");
    out
}
