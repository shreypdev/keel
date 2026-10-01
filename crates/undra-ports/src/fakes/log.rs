//! [`CaptureLog`]: a [`Log`] port that keeps what it is told.

use parking_lot::Mutex;

use crate::Log;

/// One record captured by [`CaptureLog`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    /// 0 trace, 1 debug, 2 info, 3 warn, 4 error, 5 fatal.
    pub level: u8,
    /// The emitting module.
    pub target: String,
    /// The text.
    pub message: String,
}

/// A [`Log`] that records every line for assertions.
///
/// ```
/// use undra_ports::Log;
/// use undra_ports::fakes::CaptureLog;
///
/// let log = CaptureLog::new();
/// log.log(3, "sync".into(), "retrying in 2s".into());
/// assert!(log.contains("retrying"));
/// assert_eq!(log.entries_at(3).len(), 1);
/// ```
#[derive(Debug, Default)]
pub struct CaptureLog {
    entries: Mutex<Vec<LogEntry>>,
}

impl CaptureLog {
    /// An empty log.
    pub fn new() -> CaptureLog {
        CaptureLog::default()
    }

    /// Every record so far, oldest first.
    pub fn entries(&self) -> Vec<LogEntry> {
        self.entries.lock().clone()
    }

    /// Removes and returns every record so far.
    pub fn take(&self) -> Vec<LogEntry> {
        std::mem::take(&mut *self.entries.lock())
    }

    /// The records of exactly `level`.
    pub fn entries_at(&self, level: u8) -> Vec<LogEntry> {
        self.entries
            .lock()
            .iter()
            .filter(|entry| entry.level == level)
            .cloned()
            .collect()
    }

    /// The text of every record, oldest first.
    pub fn messages(&self) -> Vec<String> {
        self.entries
            .lock()
            .iter()
            .map(|entry| entry.message.clone())
            .collect()
    }

    /// Whether any record's text contains `needle`.
    pub fn contains(&self, needle: &str) -> bool {
        self.entries
            .lock()
            .iter()
            .any(|entry| entry.message.contains(needle))
    }

    /// How many records there are.
    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    /// Whether there are no records.
    pub fn is_empty(&self) -> bool {
        self.entries.lock().is_empty()
    }

    /// Forgets every record.
    pub fn clear(&self) {
        self.entries.lock().clear();
    }
}

impl Log for CaptureLog {
    fn log(&self, level: u8, target: String, message: String) {
        self.entries.lock().push(LogEntry {
            level,
            target,
            message,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_in_order_and_filters() {
        let log = CaptureLog::new();
        assert!(log.is_empty());
        log.log(2, "a".into(), "one".into());
        log.log(4, "b".into(), "two".into());
        log.log(2, "c".into(), "three".into());
        assert_eq!(log.len(), 3);
        assert_eq!(log.messages(), ["one", "two", "three"]);
        assert_eq!(
            log.entries_at(2),
            [
                LogEntry {
                    level: 2,
                    target: "a".into(),
                    message: "one".into()
                },
                LogEntry {
                    level: 2,
                    target: "c".into(),
                    message: "three".into()
                },
            ]
        );
        assert!(log.contains("wo"));
        assert!(!log.contains("four"));
    }

    #[test]
    fn take_empties_and_clear_forgets() {
        let log = CaptureLog::new();
        log.log(0, "t".into(), "m".into());
        assert_eq!(log.take().len(), 1);
        assert!(log.is_empty());
        log.log(0, "t".into(), "m".into());
        log.clear();
        assert!(log.entries().is_empty());
    }
}
