//! Criterion benches for the `stress` group (the per-operation rows of the harsh-conditions
//! scenarios). See `common/stress.rs` for what each one measures and `bench/RESULTS.md` for the
//! numbers; the sustained runs are `cargo test -p keel-bench --test stress --release`.

use criterion::{Criterion, criterion_group, criterion_main};

#[path = "../common/mod.rs"]
mod common;

fn stress(c: &mut Criterion) {
    common::run(c, common::workloads::group("stress"));
}

criterion_group! {
    name = benches;
    config = common::criterion();
    targets = stress
}
criterion_main!(benches);
