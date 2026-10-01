#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! Undra's benchmark harness (constitution R9: budgets are tests).
//!
//! The crate has two faces over one set of operations:
//!
//! * `cargo bench -p undra-bench` runs the **criterion** benches (`benches/*.rs`) for humans:
//!   medians, outlier analysis, before/after comparisons. The numbers in `bench/RESULTS.md` come
//!   from there.
//! * `cargo test -p undra-bench --test budgets --release` is what **CI** runs. It executes the
//!   very same operations with plain `Instant` timing (no criterion, no statistics machinery),
//!   and fails when the p50 of any of them is over its budget in `bench/budgets.toml`.
//!
//! The operations live in one place (`bench/common/`, compiled into every bench and into the
//! budgets test) so the two can never drift apart. This library holds the parts that need none of
//! Undra's generated fixtures: the [`workload`] description, the [`measure`] routine and the
//! [`budget`] file parser.
//!
//! # Harsh conditions: a second layer
//!
//! The budgets test times one operation at a time. The **sustained** scenarios (`bench/common/
//! stress.rs`, run by `cargo test -p undra-bench --test stress --release` and, for minutes at a
//! time, by the `soak` binary) answer a different question: does it stay fast for seconds, at
//! rates far above any UI, with producers and consumers on several threads? They record every
//! operation in a [`stats::Histogram`] (no allocation per sample), sample resident memory with
//! [`rss`] (standard library only), check invariants that a fast-but-wrong core would fail
//! (nothing lost, nothing reordered, the host's copy of a list equals the core's), and are gated
//! by the `[stress."name"]` tables of `budgets.toml` ([`budget::StressBudget`]): throughput
//! floors, tail-latency ceilings, change-set bytes and RSS growth. Design:
//! `.10x/specs/2026-09-30-stress-bench-design.md`.
//!
//! # Wall-clock time is fine here
//!
//! The deterministic-core rule (R12) bans `Instant::now` from the core. This crate is a
//! host-side test and measurement crate: nothing in it runs inside an Undra core, and measuring
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
pub mod rss;
pub mod stats;
pub mod workload;
