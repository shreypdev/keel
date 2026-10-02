//! [`CaptureDiagnostics`]: a [`Diagnostics`] port that keeps the panic reports it is told.

use parking_lot::Mutex;

use crate::{Diagnostics, PanicReport};

/// A [`Diagnostics`] that records every report for assertions.
///
/// ```
/// use undra_ports::fakes::CaptureDiagnostics;
/// use undra_ports::{Diagnostics, PanicReport};
///
/// let diagnostics = CaptureDiagnostics::new();
/// diagnostics.panicked(PanicReport {
///     message: "boom".into(),
///     operation: "Todos.add".into(),
///     ..PanicReport::default()
/// });
/// assert_eq!(diagnostics.len(), 1);
/// assert_eq!(diagnostics.last().unwrap().operation, "Todos.add");
/// ```
#[derive(Debug, Default)]
pub struct CaptureDiagnostics {
    reports: Mutex<Vec<PanicReport>>,
}

impl CaptureDiagnostics {
    /// No reports yet.
    pub fn new() -> CaptureDiagnostics {
        CaptureDiagnostics::default()
    }

    /// Every report so far, oldest first.
    pub fn reports(&self) -> Vec<PanicReport> {
        self.reports.lock().clone()
    }

    /// Removes and returns every report so far.
    pub fn take(&self) -> Vec<PanicReport> {
        std::mem::take(&mut *self.reports.lock())
    }

    /// The most recent report.
    pub fn last(&self) -> Option<PanicReport> {
        self.reports.lock().last().cloned()
    }

    /// How many reports there are.
    pub fn len(&self) -> usize {
        self.reports.lock().len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.reports.lock().is_empty()
    }

    /// Forgets every report.
    pub fn clear(&self) {
        self.reports.lock().clear();
    }
}

impl Diagnostics for CaptureDiagnostics {
    fn panicked(&self, report: PanicReport) {
        self.reports.lock().push(report);
    }
}
