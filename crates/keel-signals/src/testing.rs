//! Helpers for testing code built on signals. Not needed in production.

use std::sync::Arc;

use keel_wire::payload::ChangeSet;
use keel_wire::{Reader, WireError};
use parking_lot::Mutex;

use crate::ChangeSink;

/// A [`ChangeSink`] that records every payload it receives.
///
/// # Example
///
/// ```
/// use keel_signals::testing::CaptureSink;
/// use keel_signals::{with_sink, StoreCell, Signal};
/// use keel_wire::Writer;
///
/// let cell = StoreCell::new(1);
/// let count = Signal::new(0_u32);
/// cell.attach(&count, 0).unwrap();
/// cell.set_handle(0x1_0000_0001);
/// cell.observe(0, true, &mut Writer::new());
///
/// let capture = CaptureSink::new();
/// with_sink(capture.clone(), || count.set(5));
///
/// let sets = capture.take_decoded();
/// assert_eq!(sets.len(), 1);
/// assert_eq!(sets[0].entries[0].value, 5_u32.to_le_bytes());
/// ```
#[derive(Debug, Default)]
pub struct CaptureSink {
    received: Mutex<Vec<Vec<u8>>>,
}

impl CaptureSink {
    /// Creates an empty sink, ready to be passed to [`set_sink`](crate::set_sink) or
    /// [`with_sink`](crate::with_sink).
    pub fn new() -> Arc<CaptureSink> {
        Arc::new(CaptureSink::default())
    }

    /// Number of payloads received and not yet taken.
    pub fn len(&self) -> usize {
        self.received.lock().len()
    }

    /// Returns `true` if nothing has been received (or everything was taken).
    pub fn is_empty(&self) -> bool {
        self.received.lock().is_empty()
    }

    /// Takes the raw payloads received so far, oldest first, leaving the sink empty.
    pub fn take(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut *self.received.lock())
    }

    /// Takes the payloads received so far and decodes each as a [`ChangeSet`].
    ///
    /// # Panics
    ///
    /// If a payload is not a valid change-set; the signals crate never produces one, so that is
    /// a test failure.
    pub fn take_decoded(&self) -> Vec<ChangeSet> {
        self.take()
            .iter()
            .map(|bytes| {
                decode(bytes).unwrap_or_else(|e| {
                    panic!("the sink received an invalid change-set ({e}): {bytes:?}")
                })
            })
            .collect()
    }
}

impl ChangeSink for CaptureSink {
    fn deliver(&self, change_set: &[u8]) {
        self.received.lock().push(change_set.to_vec());
    }
}

/// Decodes a complete change-set payload, requiring that no bytes are left over.
pub fn decode(bytes: &[u8]) -> Result<ChangeSet, WireError> {
    let mut r = Reader::new(bytes);
    let cs = ChangeSet::decode(&mut r)?;
    r.finish()?;
    Ok(cs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_in_order_and_take_empties() {
        let sink = CaptureSink::new();
        assert!(sink.is_empty());
        sink.deliver(&[1, 2]);
        sink.deliver(&[3]);
        assert_eq!(sink.len(), 2);
        assert_eq!(sink.take(), vec![vec![1, 2], vec![3]]);
        assert!(sink.is_empty());
    }

    #[test]
    fn decode_rejects_garbage_and_trailing_bytes() {
        assert!(decode(&[1, 2, 3]).is_err());
        let mut ok = vec![0_u8; 12];
        ok[0] = 9; // txn_id 9, zero entries
        assert_eq!(decode(&ok).expect("valid").txn_id, 9);
        ok.push(0);
        assert_eq!(decode(&ok), Err(WireError::TrailingBytes { count: 1 }));
    }

    #[test]
    #[should_panic(expected = "invalid change-set")]
    fn take_decoded_panics_on_invalid_payloads() {
        let sink = CaptureSink::new();
        sink.deliver(&[1]);
        let _ = sink.take_decoded();
    }
}
