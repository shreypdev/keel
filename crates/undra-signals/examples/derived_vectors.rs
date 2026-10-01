//! Writes the recording of the seeded derived-list workload (`tests/support/seeded_views.rs`,
//! format `UDV1`) that contract scenario S19 replays through each platform runtime's patch decoder
//! and applier. The run checks every view against `filter + stable sort` after every operation, so
//! a recording is only written for a correct run.
//!
//! ```text
//! cargo run -p undra-signals --example derived_vectors -- <out.bin> [ops]
//! ```

#[path = "../tests/support/seeded_views.rs"]
mod seeded_views;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(out) = args.next() else {
        eprintln!("usage: derived_vectors <out.bin> [ops]");
        std::process::exit(2);
    };
    let ops = args.next().and_then(|n| n.parse().ok()).unwrap_or(60_000);
    let summary = seeded_views::run(0x00D3_51ED_1157_5EED, ops, true);
    if let Err(error) = std::fs::write(&out, &summary.records) {
        eprintln!("derived_vectors: cannot write {out}: {error}");
        std::process::exit(1);
    }
    eprintln!(
        "derived_vectors: {} ops, {} transactions, {} change-sets ({} patches, {} full values, {} rebuilds), {} bytes -> {out}",
        summary.ops,
        summary.transactions,
        summary.change_sets,
        summary.patches,
        summary.full_values,
        summary.rebuilds,
        summary.records.len()
    );
}
