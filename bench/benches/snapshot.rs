//! Criterion benches for the `snapshot` group. See `common/workloads.rs` for what each one measures
//! and `bench/RESULTS.md` for the numbers.

use criterion::{Criterion, criterion_group, criterion_main};

#[path = "../common/mod.rs"]
mod common;

fn snapshot(c: &mut Criterion) {
    common::run(c, common::workloads::group("snapshot"));
}

criterion_group! {
    name = benches;
    config = common::criterion();
    targets = snapshot
}
criterion_main!(benches);
