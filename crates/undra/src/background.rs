//! Background tasks of your own (ADR-046).
//!
//! The OS grants an app a window to work in while it is not on screen (iOS `BGTaskScheduler`,
//! Android WorkManager, a page about to hide); the platform runtime calls the core's standard
//! function `run_background(deadline)` and the core runs its **background tasks** inside it. The
//! query runtime registers three (replay the offline queue, refetch stale queries, flush what
//! waits to be persisted); [`register`] adds one of yours, from an `InitHook` or any place with a
//! `Ctx`:
//!
//! ```ignore
//! use undra::background::{BackgroundOutcome, Deadline, register};
//! use undra::prelude::*;
//!
//! fn start(ctx: &Ctx) {
//!     register(
//!         ctx,
//!         "app.upload-drafts",
//!         |_ctx| pending_drafts(),                       // how much work waits (0: none)
//!         |ctx, deadline| {
//!             let weak = ctx.downgrade();                // never keep a `Ctx` across an await
//!             Box::pin(async move {
//!                 while !deadline.expired() && send_next_draft(&weak).await {
//!                     deadline.note_replayed(1);         // progress is reported as it happens
//!                 }
//!                 if pending_drafts() == 0 { BackgroundOutcome::Done } else { BackgroundOutcome::Incomplete }
//!             })
//!         },
//!     );
//! }
//! ```
//!
//! A task that panics is contained and reported like any panic (`onPanic`); a task the window
//! ends on is dropped, so keep progress durable as you go.

use undra_runtime::Ctx;
pub use undra_runtime::background::{
    BackgroundFuture, BackgroundOutcome, BackgroundTotals, Deadline, MARGIN,
};

/// Registers the background task `name` on `ctx`'s runtime (a second task of one name is ignored).
///
/// `pending` says cheaply how much work is waiting (`0` for none): the platform reads the sum
/// through the runtime's statistics (`background.pending`) to decide whether a window is worth
/// asking the OS for. `run` starts one run of the task; it holds the runtime weakly across awaits
/// (`ctx.downgrade()`, ADR-034) and stops when its [`Deadline`] is expired or its future is
/// dropped.
pub fn register(
    ctx: &Ctx,
    name: &'static str,
    pending: impl Fn(&Ctx) -> u32 + Send + Sync + 'static,
    run: impl Fn(&Ctx, Deadline) -> BackgroundFuture + Send + Sync + 'static,
) {
    ctx.runtime().add_background_task(name, pending, run);
}
