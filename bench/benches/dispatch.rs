//! Criterion benches for the `dispatch` group. See `common/workloads.rs` for what each one measures
//! and `bench/RESULTS.md` for the numbers.

use criterion::{Criterion, criterion_group, criterion_main};

#[path = "../common/mod.rs"]
mod common;

fn dispatch(c: &mut Criterion) {
    common::run(c, common::workloads::group("dispatch"));
}

criterion_group! {
    name = benches;
    config = common::criterion();
    targets = dispatch
}
criterion_main!(benches);
