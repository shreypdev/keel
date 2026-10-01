//! Runtime statistics: crossing counters and the JSON document behind `undra_stats_json`.

use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic counters, updated without any lock.
#[derive(Default)]
pub(crate) struct Stats {
    /// Calls received from the host (`call` and `call_sync`).
    pub calls: AtomicU64,
    /// Replies sent to the host.
    pub replies: AtomicU64,
    /// Change-sets delivered to the host (one per committed transaction that had observers).
    pub change_sets: AtomicU64,
    /// Bytes of those change-sets.
    pub change_set_bytes: AtomicU64,
    /// Port calls issued to the host.
    pub port_calls: AtomicU64,
    /// Port replies received from the host.
    pub port_replies: AtomicU64,
    /// Stream items (including end and error markers) sent to the host.
    pub stream_items: AtomicU64,
    /// Events received from the host.
    pub events: AtomicU64,
    /// Panics caught by the guard.
    pub panics: AtomicU64,
    /// Requests answered with status 5 (or rejected before a reply was possible).
    pub bad_requests: AtomicU64,
    /// Calls cancelled by the host.
    pub cancelled: AtomicU64,
    /// Task polls executed.
    pub polls: AtomicU64,
    /// Executor turns executed.
    pub turns: AtomicU64,
}

impl Stats {
    pub(crate) fn inc(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::Relaxed);
    }

    pub(crate) fn get(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
    }
}

/// Appends `s` to `out` as a JSON string literal.
pub(crate) fn push_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quoted(s: &str) -> String {
        let mut out = String::new();
        push_json_string(&mut out, s);
        out
    }

    #[test]
    fn strings_are_escaped() {
        assert_eq!(quoted("plain"), "\"plain\"");
        assert_eq!(quoted("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(quoted("line\nbreak\ttab\r"), "\"line\\nbreak\\ttab\\r\"");
        assert_eq!(quoted("\u{1}"), "\"\\u0001\"");
        assert_eq!(quoted("caf\u{e9} \u{1F30A}"), "\"caf\u{e9} \u{1F30A}\"");
    }

    #[test]
    fn counters_add_up() {
        let s = Stats::default();
        Stats::inc(&s.calls);
        Stats::add(&s.calls, 4);
        assert_eq!(Stats::get(&s.calls), 5);
    }
}
