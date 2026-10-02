//! Criterion benches for the `lazy` group (ADR-043). See `common/workloads.rs` for what each one
//! measures and `bench/RESULTS.md` for the numbers.

use criterion::{Criterion, criterion_group, criterion_main};

#[path = "../common/mod.rs"]
mod common;

fn lazy(c: &mut Criterion) {
    common::run(c, common::workloads::group("lazy"));
}

criterion_group! {
    name = benches;
    config = common::criterion();
    targets = lazy
}
criterion_main!(benches);
