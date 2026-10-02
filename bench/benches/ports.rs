//! Criterion benches for the opt-in ports (`ports/*` and `db/*`, ADR-047 and ADR-048). See
//! `common/ports.rs` for what each one measures and `bench/RESULTS.md` for the numbers.

use criterion::{Criterion, criterion_group, criterion_main};

#[path = "../common/mod.rs"]
mod common;

fn ports(c: &mut Criterion) {
    common::run(c, common::workloads::group("ports"));
    common::run(c, common::workloads::group("db"));
}

criterion_group! {
    name = benches;
    config = common::criterion();
    targets = ports
}
criterion_main!(benches);
