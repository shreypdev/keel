//! The standard function `run_background` through the boundary (ADR-046 decision 3; prod-ops
//! review): its dispatcher is hand-written so that a core with no background task answers at once
//! and links no asynchronous run (ADR-052), and a core with tasks runs them asynchronously. Either
//! way the reply is one `BackgroundReport`.

use std::time::Duration;

use undra_meta::ids;
use undra_ports::BackgroundReport;
use undra_runtime::background::{BackgroundFuture, BackgroundOutcome};
use undra_runtime::testing::TestRuntime;
use undra_wire::payload::{CallTarget, ReplyStatus};
use undra_wire::{Decode, Encode};

fn run_background() -> CallTarget {
    CallTarget::Function {
        method_id: ids::function_id("run_background"),
    }
}

#[test]
fn a_core_without_background_tasks_answers_an_idle_report_at_once() {
    let t = TestRuntime::new();
    t.call(run_background(), 7, &5_000_u64.encode_to_vec());
    // No task was spawned: the reply is already there.
    let replies = t.take_replies();
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert_eq!(replies[0].status, ReplyStatus::Ok);
    let report = BackgroundReport::decode_exact(&replies[0].body).expect("a BackgroundReport");
    assert_eq!(
        report,
        BackgroundReport {
            finished: true,
            replayed: 0,
            refetched: 0,
            still_pending: 0
        }
    );
    // A body that is not one u64 is a bad request, as for any function.
    t.call(run_background(), 8, &[1, 2]);
    assert_eq!(t.take_replies()[0].status, ReplyStatus::BadRequest);
    // It is asynchronous by its declaration: `call_sync` refuses it, before running anything.
    let reply = t.call_sync(run_background(), 9, &5_000_u64.encode_to_vec());
    assert_eq!(reply.status, ReplyStatus::BadRequest, "{reply:?}");
}

#[test]
fn a_core_with_a_task_runs_it_asynchronously_and_reports_it() {
    let t = TestRuntime::new();
    t.runtime().add_background_task(
        "review.one",
        |_| 0,
        |_, deadline| -> BackgroundFuture {
            deadline.note_replayed(2);
            Box::pin(async { BackgroundOutcome::Done })
        },
    );
    t.call(run_background(), 7, &5_000_u64.encode_to_vec());
    t.run_pending();
    t.advance(Duration::from_millis(1));
    let replies = t.take_replies();
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert_eq!(replies[0].status, ReplyStatus::Ok);
    let report = BackgroundReport::decode_exact(&replies[0].body).expect("a BackgroundReport");
    assert_eq!(
        report,
        BackgroundReport {
            finished: true,
            replayed: 2,
            refetched: 0,
            still_pending: 0
        }
    );
    assert!(t.runtime().stats_json().contains("\"runs\":1"));
}
