//! ADR-039's before/after, as the ADR's throwaway probe measured it, kept: one `update_at` of a
//! visible row's title in a keyed list of 1,000 / 10,000 / 100,000 rows (every fourth done, titles
//! of 34 characters), with `visible` = the rows not done
//!
//! * **before**: a `Computed<Vec<Item>>` that filters the list (recomputed and sent whole);
//! * **after**: a `DerivedList<Item>` built with `derive().filter(..)` (kept from the recorded op);
//! * **source**: the keyed list alone, the O(change) reference.
//!
//! `undra-signals` directly (a `StoreCell`, both slots observed, a sink that copies each change-set
//! as the host does), so the numbers are the core's commit, the view's recompute or drain, the
//! encoding and the copy, without dispatch. Not a gate: run it to refresh `bench/RESULTS.md`,
//! "Derived lists":
//!
//! ```text
//! cargo test -p undra-bench --release --test derived_before_after -- --ignored --nocapture
//! ```
//!
//! `UNDRA_BENCH_RESULTS_DIR=dir` also writes the numbers as JSON.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use undra::signals::{
    ALL_SIGNALS, ChangeSink, Computed, DerivedList, Signal, StoreCell, with_sink,
};
use undra::wire::Writer;
use undra_bench::hostinfo;

#[path = "../common/mod.rs"]
mod common;

use common::fixtures::{Item, title_of};

/// Copies every change-set, as a platform's FFI callback does, and remembers the last one's size.
#[derive(Default)]
struct CopySink {
    copy: Mutex<Vec<u8>>,
}

impl ChangeSink for CopySink {
    fn deliver(&self, change_set: &[u8]) {
        let mut copy = self.copy.lock().expect("not poisoned");
        copy.clear();
        copy.extend_from_slice(change_set);
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Shape {
    Before,
    After,
    Source,
}

fn rows(n: usize) -> Vec<Item> {
    (0..n)
        .map(|i| Item {
            id: i as u64 + 1,
            title: title_of(i as u32, 34),
            done: i % 4 == 3,
        })
        .collect()
}

/// Median nanoseconds and change-set bytes of one change, for `shape` at `n` rows.
fn measure(shape: Shape, n: usize, iterations: usize) -> (f64, usize) {
    let cell = StoreCell::new(0xBEF0);
    cell.set_handle(0x1_0000_0001);
    let list = Signal::new(rows(n));
    cell.attach_keyed(&list, 0, |item: &Item| item.id).unwrap();
    // Kept alive for the whole run.
    let mut _computed: Option<Computed<Vec<Item>>> = None;
    let mut _derived: Option<DerivedList<Item>> = None;
    match shape {
        Shape::Before => {
            let visible = Computed::new(&list, |rows: &Vec<Item>| {
                rows.iter().filter(|row| !row.done).cloned().collect()
            });
            cell.attach_computed(&visible, 1).unwrap();
            _computed = Some(visible);
        }
        Shape::After => {
            let visible = list.derive().filter(|row: &Item| !row.done).build();
            cell.attach_derived(&visible, 1, |item: &Item| item.id)
                .unwrap();
            _derived = Some(visible);
        }
        Shape::Source => {}
    }
    cell.observe(ALL_SIGNALS, true, &mut Writer::new());
    let sink = Arc::new(CopySink::default());
    let target = n / 2; // `n / 2 % 4 == 0`: a visible row
    let titles = [title_of(1, 34), title_of(2, 34)];
    let mut samples = Vec::with_capacity(iterations);
    with_sink(sink.clone(), || {
        for i in 0..iterations + iterations / 10 {
            let title = titles[i % 2].clone();
            let t0 = Instant::now();
            list.update_at(target, |row| row.title = title);
            let ns = t0.elapsed().as_nanos() as f64;
            if i >= iterations / 10 {
                samples.push(ns);
            }
        }
    });
    let bytes = sink.copy.lock().expect("not poisoned").len();
    samples.sort_by(f64::total_cmp);
    (samples[samples.len() / 2], bytes)
}

fn human_ns(ns: f64) -> String {
    if ns >= 1_000_000.0 {
        format!("{:.2} ms", ns / 1_000_000.0)
    } else if ns >= 1_000.0 {
        format!("{:.1} us", ns / 1_000.0)
    } else {
        format!("{ns:.0} ns")
    }
}

fn human_bytes(bytes: usize) -> String {
    if bytes >= 1_000_000 {
        format!("{:.2} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes} bytes")
    }
}

#[test]
#[ignore = "prints ADR-039's before/after table; run it explicitly with --ignored --nocapture"]
fn before_and_after() {
    let load_before = hostinfo::load_average();
    let mut json_rows = Vec::new();
    println!(
        "| Rows | Before: computed, per change | Before: change-set | After: derived list, per change | After: change-set | Source alone |"
    );
    println!("|---|---|---|---|---|---|");
    for (n, iterations) in [(1_000_usize, 4_000_usize), (10_000, 1_000), (100_000, 100)] {
        // Best of three medians: the machine is shared.
        let best = |shape| {
            (0..3)
                .map(|_| measure(shape, n, iterations))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .expect("three runs")
        };
        let (before_ns, before_bytes) = best(Shape::Before);
        let (after_ns, after_bytes) = best(Shape::After);
        let (source_ns, source_bytes) = best(Shape::Source);
        println!(
            "| {n} | {} | {} | {} | {} | {}, {} |",
            human_ns(before_ns),
            human_bytes(before_bytes),
            human_ns(after_ns),
            human_bytes(after_bytes),
            human_ns(source_ns),
            human_bytes(source_bytes)
        );
        json_rows.push(format!(
            "    {{\"rows\": {n}, \"before_ns\": {before_ns:.0}, \"before_bytes\": {before_bytes}, \"after_ns\": {after_ns:.0}, \"after_bytes\": {after_bytes}, \"source_ns\": {source_ns:.0}, \"source_bytes\": {source_bytes}}}"
        ));
    }
    if let Some(dir) = std::env::var_os("UNDRA_BENCH_RESULTS_DIR") {
        let date = hostinfo::date();
        let load_after = hostinfo::load_average();
        let text = format!(
            "{{\n  \"kind\": \"derived-before-after\",\n  \"date\": \"{date}\",\n  \"command\": \"cargo test -p undra-bench --release --test derived_before_after -- --ignored --nocapture\",\n  \"what\": \"one update_at of a visible row's title; visible = rows not done (75%); undra-signals directly, both slots observed, a sink that copies each change-set; median per change, best of three\",\n  \"load_average\": {{\"before\": {}, \"after\": {}}},\n  \"rows\": [\n{}\n  ]\n}}\n",
            load_before.map_or_else(|| "null".to_owned(), |l| format!("{l:.2}")),
            load_after.map_or_else(|| "null".to_owned(), |l| format!("{l:.2}")),
            json_rows.join(",\n")
        );
        let path =
            std::path::Path::new(&dir).join(format!("{date}-derived-lists-before-after.json"));
        std::fs::write(&path, text).expect("write the results");
        eprintln!("wrote {}", path.display());
    }
}
