//! Criterion benches for the `signals` group. See `common/workloads.rs` for what each one measures
//! and `bench/RESULTS.md` for the numbers.

use criterion::{Criterion, criterion_group, criterion_main};

#[path = "../common/mod.rs"]
mod common;

fn signals(c: &mut Criterion) {
    common::run(c, common::workloads::group("signals"));
}

criterion_group! {
    name = benches;
    config = common::criterion();
    targets = signals
}
criterion_main!(benches);
