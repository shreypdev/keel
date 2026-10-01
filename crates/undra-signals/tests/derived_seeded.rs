//! 60,000 seeded operations over three derived views (ADR-039): after every operation each view's
//! `get()` equals `filter + stable sort` of the source, and after every commit so does a host
//! mirror that applies the delivered change-sets. The workload is `support/seeded_views.rs`; the
//! same recording replays through each platform runtime's patch applier in contract scenario S19.
//!
//! `UNDRA_DERIVED_SEEDED_OPS` overrides the number of operations (default 60,000).

#[path = "support/seeded_views.rs"]
mod seeded_views;

fn ops() -> usize {
    std::env::var("UNDRA_DERIVED_SEEDED_OPS")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(60_000)
}

#[test]
fn sixty_thousand_seeded_operations_over_three_views_match_filter_and_stable_sort() {
    let summary = seeded_views::run(0x00D3_51ED_1157_5EED, ops(), false);
    println!("{summary:?}");
    assert!(summary.ops >= ops());
    // The workload is mostly recorded operations: almost every change-set is patches.
    assert!(summary.patches > 20 * summary.full_values, "{summary:?}");
    assert!(
        summary.rebuilds > 3,
        "raw writes rebuild the views: {summary:?}"
    );
}

#[test]
fn the_recording_is_deterministic() {
    let a = seeded_views::run(7, 2_000, true);
    let b = seeded_views::run(7, 2_000, true);
    assert_eq!(a.records, b.records);
    assert_eq!(&a.records[..4], b"UDV1");
}
