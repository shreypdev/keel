//! The standard function `run_background` (ADR-046 decision 3).

use std::time::Duration;

use undra_runtime::Ctx;
use undra_runtime::background::BackgroundTotals;

use crate::BackgroundReport;

/// Runs the background tasks for at most `deadline_ms` (the window the OS granted).
// What it does, in full (ADR-046): every background task of the runtime starts at once (the
// offline queue's replay, the refetch of stale queries and the flush of what waits to be persisted,
// `undra-query`'s; the app's own). The call returns when they are all done, or half a second before
// the deadline so the host can still tell the OS it finished, with what got done and what is left.
// A host about to lose its window cancels the call; work already done is kept. Kept out of the doc
// comment: a core embeds its schema's docs (ADR-050), and every core has this function.
#[undra_macros::api]
#[undra(crate = "crate::root")]
pub async fn run_background(ctx: &Ctx, deadline_ms: u64) -> BackgroundReport {
    // A core that registered no background task has no runner linked (ADR-052): nothing to do.
    let totals = match ctx.runtime().background_runner() {
        Some(run) => run(ctx, Duration::from_millis(deadline_ms)).await,
        None => BackgroundTotals::IDLE,
    };
    BackgroundReport {
        finished: totals.finished,
        replayed: totals.replayed,
        refetched: totals.refetched,
        still_pending: totals.still_pending,
    }
}
