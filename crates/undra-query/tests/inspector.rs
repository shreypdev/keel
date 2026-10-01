//! The query cache as a document for the devtools (ADR-054): the `queries` inspector the client
//! registers with the runtime.

mod common;

use common::*;
use undra_ports::Clock;

fn doc(h: &Harness) -> serde_json::Value {
    let text =
        h.t.runtime()
            .inspect("queries")
            .expect("the client registered its inspector");
    serde_json::from_str(&text).expect("the inspector answers JSON")
}

#[test]
fn the_cache_is_described_with_status_observers_and_values() {
    let h = Harness::new();
    h.serve_page(0, vec![todo(1, "milk")]);
    // Using the client registers the inspector; nothing is cached yet.
    let handle = h.query().observe::<TodosQuery>((0,));
    let before = doc(&h);
    assert_eq!(before["entries"].as_array().unwrap().len(), 1);
    assert_eq!(before["entries"][0]["status"], "fetching");
    assert_eq!(before["entries"][0]["observers"], 1);
    assert_eq!(before["entries"][0]["data"], serde_json::Value::Null);
    assert_eq!(before["online"], true);

    h.t.run_pending();
    let after = doc(&h);
    let entry = &after["entries"][0];
    assert_eq!(entry["status"], "success");
    assert_eq!(entry["fetching"], false);
    assert_eq!(entry["invalidated"], false);
    assert!(entry["key"].as_str().unwrap().contains('0'), "{entry}");
    assert_eq!(entry["updated_at"].as_i64(), Some(h.fakes.clock.now_ms()));
    // The value is the wire encoding in hex: the page decodes it with the schema.
    let hex = entry["data"].as_str().expect("the data is there");
    assert_eq!(hex.len() / 2, entry["data_len"].as_u64().unwrap() as usize);
    assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
    drop(handle);
}

#[test]
fn reading_the_cache_changes_nothing_and_sampling_is_stable() {
    let h = Harness::new();
    h.serve_page(0, vec![todo(1, "milk")]);
    let handle = h.query().observe::<TodosQuery>((0,));
    h.t.run_pending();
    assert_eq!(doc(&h), doc(&h), "reading the cache does not change it");
    drop(handle);
}

/// What sampling costs on a big cache (ADR-054 review): ten thousand entries, each with a value.
/// The page samples once a second while it is open; the document is built under the cache's lock,
/// so this is how long a query operation can wait behind a sample.
#[test]
fn sampling_a_cache_of_ten_thousand_entries_stays_within_a_budget() {
    let h = Harness::new();
    let mut handles = Vec::new();
    for n in 0..10_000_u32 {
        h.serve_page(n, vec![todo(1, "a title of some length, as a todo has")]);
        handles.push(h.query().observe::<TodosQuery>((n,)));
    }
    h.t.run_pending();
    let started = std::time::Instant::now();
    let text = h.t.runtime().inspect("queries").expect("the inspector answers");
    let took = started.elapsed();
    let rows = serde_json::from_str::<serde_json::Value>(&text).unwrap()["entries"]
        .as_array()
        .unwrap()
        .len();
    eprintln!("describe: {rows} entries, {} KiB of JSON in {took:?}", text.len() / 1024);
    assert_eq!(rows, 10_000);
    // A debug build on a loaded machine; the release figure is a tenth of this.
    assert!(took < std::time::Duration::from_millis(1500), "{took:?}");
    drop(handles);
}
