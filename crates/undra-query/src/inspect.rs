//! The query cache as one JSON document, for `undra dev`'s devtools page (ADR-054).
//!
//! Registered with the runtime as the inspector `"queries"` the first time the client starts.
//! The document is a view of the cache at one moment; it never changes it, and the core never
//! reads it. Values are hex of their wire encoding: the page decodes them with the schema (the
//! query's `returns`), exactly as it decodes a signal.

use std::fmt::Write as _;

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

fn entry(e: &Entry, out: &mut String) {
    let _ = write!(out, "{{\"query_id\":{},\"key\":", e.vt.id);
    json_str(&e.rendered, out);
    out.push_str(",\"template\":");
    json_str(e.vt.key, out);
    let _ = write!(
        out,
        ",\"status\":\"{}\",\"fetching\":{},\"observers\":{},\"invalidated\":{},\"failed\":{},\"layers\":{},\"stamp\":{}",
        match e.status() {
            crate::QueryStatus::Idle => "idle",
            crate::QueryStatus::Fetching => "fetching",
            crate::QueryStatus::Success => "success",
            crate::QueryStatus::Error => "error",
        },
        e.inflight.is_some(),
        e.observers,
        e.invalidated,
        e.failed,
        e.layers.len(),
        e.stamp,
    );
    match e.updated_at {
        Some(at) => {
            let _ = write!(out, ",\"updated_at\":{at}");
        }
        None => out.push_str(",\"updated_at\":null"),
    }
    value("data", e.data.as_ref().map(|d| &*d.bytes), out);
    value("error", e.error.as_ref().map(|d| &*d.bytes), out);
    out.push('}');
}

/// The cache of `shared` as JSON, entries ordered by query id then key.
pub(crate) fn describe(shared: &Shared) -> String {
    let (mut out, pending) = (String::with_capacity(512), shared.queue_len());
    let mut rows: Vec<(u32, String, String)> = Vec::new();
    {
        let state = shared.state.lock();
        for e in state.entries.values() {
            let mut row = String::new();
            entry(e, &mut row);
            rows.push((e.vt.id, e.rendered.clone(), row));
        }
    }
    rows.sort();
    let _ = write!(
        out,
        "{{\"online\":{},\"gc_ms\":{},\"pending_mutations\":{pending},\"entries\":[",
        shared.online.load(std::sync::atomic::Ordering::Relaxed),
        shared.gc_ms.load(std::sync::atomic::Ordering::Relaxed),
    );
    for (i, (_, _, row)) in rows.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(row);
    }
    out.push_str("]}");
    out
}
