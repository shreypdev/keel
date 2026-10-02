//! The standard function `run_background` (ADR-046 decision 3): the platform's way of telling the
//! core "the OS gave you this long".

use std::sync::Arc;
use std::time::Duration;

use undra_runtime::Ctx;

use crate::BackgroundReport;

/// Runs the core's background work, for at most `deadline_ms` (the window the OS granted).
///
/// Every background task of the runtime (the offline queue's replay, the refetch of stale queries
/// and the flush of what waits to be persisted, `undra-query`'s; the app's own) starts at once. The
/// call returns when they are all done, or half a second before the deadline so the host can still
/// tell the OS it finished, with what got done and what is left. A host that is about to lose its
/// window cancels the call; work already done is kept.
#[undra_macros::api]
#[undra(crate = "crate::root")]
pub async fn run_background(ctx: &Ctx, deadline_ms: u64) -> BackgroundReport {
    // The deadline is read from the `Clock` port (R12): a fake clock decides it in a test.
    let clock = crate::clock(ctx);
    let totals = undra_runtime::background::run(
        ctx,
        Duration::from_millis(deadline_ms),
        Arc::new(move || clock.monotonic_ns()),
    )
    .await;
    BackgroundReport {
        finished: totals.finished,
        replayed: totals.replayed,
        refetched: totals.refetched,
        still_pending: totals.still_pending,
    }
}
