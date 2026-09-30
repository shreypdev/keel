#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! Keel's benchmark harness (constitution R9: budgets are tests).
//!
//! The crate has two faces over one set of operations:
//!
//! * `cargo bench -p keel-bench` runs the **criterion** benches (`benches/*.rs`) for humans:
//!   medians, outlier analysis, before/after comparisons. The numbers in `bench/RESULTS.md` come
//!   from there.
//! * `cargo test -p keel-bench --test budgets --release` is what **CI** runs. It executes the
//!   very same operations with plain `Instant` timing (no criterion, no statistics machinery),
//!   and fails when the p50 of any of them is over its budget in `bench/budgets.toml`.
//!
//! The operations live in one place (`bench/common/`, compiled into every bench and into the
//! budgets test) so the two can never drift apart. This library holds the parts that need none of
//! Keel's generated fixtures: the [`workload`] description, the [`measure`] routine and the
//! [`budget`] file parser.
//!
//! # Wall-clock time is fine here
//!
//! The deterministic-core rule (R12) bans `Instant::now` from the core. This crate is a
//! host-side test and measurement crate: nothing in it runs inside a Keel core, and measuring
//! elapsed time is its whole job.
//!
//! # Host budgets are not device budgets
//!
//! The blueprint's budget table (section 14) is per device: an A15 iPhone, a 2022 mid-range
//! Android phone, Chromium. The budgets in `budgets.toml` are **host regression guards**: about
//! five times what an Apple-silicon laptop measures, so a slower shared CI runner passes while a
//! change that makes an operation several times slower does not. They prove an operation did not
//! regress; the device numbers that prove the blueprint's targets are produced in the playground
//! phase (see `bench/RESULTS.md`).

pub mod budget;
pub mod measure;
pub mod workload;
