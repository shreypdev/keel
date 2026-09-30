//! Criterion benches for the `wire` group: every wire type's encode, decode and round trip. See
//! `common/workloads.rs` for what each one measures and `bench/RESULTS.md` for the numbers.

use criterion::{Criterion, criterion_group, criterion_main};

#[path = "../common/mod.rs"]
mod common;

fn wire(c: &mut Criterion) {
    // The budgets test gates the round trips; the two halves are for humans.
    common::run(c, common::workloads::wire_halves());
    common::run(c, common::workloads::wire());
}

criterion_group! {
    name = benches;
    config = common::criterion();
    targets = wire
}
criterion_main!(benches);
