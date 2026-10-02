//! Criterion benches for the `boundary` group: calls from the core into the host (ADR-041). See
//! `common/workloads.rs` for what each one measures and `bench/RESULTS.md` for the numbers.

use criterion::{Criterion, criterion_group, criterion_main};

#[path = "../common/mod.rs"]
mod common;

fn boundary(c: &mut Criterion) {
    common::run(c, common::workloads::group("boundary"));
}

criterion_group! {
    name = benches;
    config = common::criterion();
    targets = boundary
}
criterion_main!(benches);
