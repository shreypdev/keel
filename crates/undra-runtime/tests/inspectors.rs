//! Inspectors: the one seam dev tooling reads a layered crate's state through (ADR-054).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Weak};

use undra_runtime::testing::TestRuntime;

#[test]
fn an_inspector_is_asked_by_name_and_answers_a_document() {
    let tr = TestRuntime::new();
    let rt = tr.runtime();
    assert!(
        rt.inspect("queries").is_none(),
        "nothing is registered by default"
    );
    assert!(rt.inspectors().is_empty());

    let calls = Arc::new(AtomicU32::new(0));
    let counted = calls.clone();
    rt.register_inspector(
        "queries",
        Arc::new(move || format!("{{\"n\":{}}}", counted.fetch_add(1, Ordering::SeqCst) + 1)),
    );
    assert_eq!(rt.inspectors(), ["queries"]);
    assert_eq!(rt.inspect("queries").as_deref(), Some("{\"n\":1}"));
    assert_eq!(rt.inspect("queries").as_deref(), Some("{\"n\":2}"));
    assert!(rt.inspect("other").is_none());
}

#[test]
fn registering_again_under_a_name_replaces_it() {
    let tr = TestRuntime::new();
    let rt = tr.runtime();
    rt.register_inspector("a", Arc::new(|| "1".to_owned()));
    rt.register_inspector("b", Arc::new(|| "b".to_owned()));
    rt.register_inspector("a", Arc::new(|| "2".to_owned()));
    assert_eq!(rt.inspectors(), ["a", "b"]);
    assert_eq!(rt.inspect("a").as_deref(), Some("2"));
}

#[test]
fn a_panicking_inspector_is_contained_reported_once_and_not_asked_again() {
    let tr = TestRuntime::new();
    let rt = tr.runtime();
    let asked = Arc::new(AtomicU32::new(0));
    let counted = asked.clone();
    rt.register_inspector(
        "boom",
        Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            panic!("an inspector that breaks")
        }),
    );
    for _ in 0..5 {
        assert!(
            rt.inspect("boom").is_none(),
            "a panic is not an answer, and not a crash"
        );
    }
    assert_eq!(
        asked.load(Ordering::SeqCst),
        1,
        "a broken inspector is not asked again"
    );
    let reports: Vec<_> = tr
        .host()
        .take_logs()
        .into_iter()
        .filter(|l| l.message.contains("inspector `boom` panicked"))
        .collect();
    assert_eq!(reports.len(), 1, "reported once: {reports:?}");
    assert_eq!(reports[0].level, 5);
    assert!(reports[0].message.contains("an inspector that breaks"));
    let stats: serde_json::Value = serde_json::from_str(&rt.stats_json()).unwrap();
    assert_eq!(stats["panics"], 1);
    // Another inspector is untouched, and registering a new `boom` makes it ask-able again.
    rt.register_inspector("fine", Arc::new(|| "{}".to_owned()));
    assert_eq!(rt.inspect("fine").as_deref(), Some("{}"));
    rt.register_inspector("boom", Arc::new(|| "{\"back\":true}".to_owned()));
    assert_eq!(rt.inspect("boom").as_deref(), Some("{\"back\":true}"));
}

#[test]
fn an_inspector_that_asks_the_runtime_for_a_snapshot_or_another_inspector_does_not_deadlock() {
    let tr = TestRuntime::new();
    let rt = tr.runtime();
    let weak: Weak<_> = Arc::downgrade(rt);
    rt.register_inspector("inner", Arc::new(|| "i".to_owned()));
    rt.register_inspector(
        "outer",
        Arc::new(move || {
            let rt = weak.upgrade().expect("the runtime is alive");
            format!(
                "{}:{}",
                rt.inspect("inner").unwrap_or_default(),
                rt.snapshot().len()
            )
        }),
    );
    // From the thread of a test, from a thread that has the core, and from another thread.
    assert!(rt.inspect("outer").unwrap().starts_with("i:"));
    let rt2 = rt.clone();
    let from_core = {
        let (tx, rx) = std::sync::mpsc::channel();
        rt.ctx().spawn(async move {
            let _ = tx.send(rt2.inspect("outer"));
        });
        tr.run_pending();
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("no deadlock on the core")
    };
    assert!(
        from_core
            .expect("an answer from the core")
            .starts_with("i:")
    );
    let rt3 = rt.clone();
    let from_thread = std::thread::spawn(move || rt3.inspect("outer"))
        .join()
        .unwrap();
    assert!(from_thread.unwrap().starts_with("i:"));
}

#[test]
fn an_inspector_may_register_another_without_deadlocking() {
    let tr = TestRuntime::new();
    let rt = tr.runtime();
    let weak: Weak<_> = Arc::downgrade(rt);
    rt.register_inspector(
        "outer",
        Arc::new(move || {
            if let Some(rt) = weak.upgrade() {
                rt.register_inspector("inner", Arc::new(|| "i".to_owned()));
            }
            "o".to_owned()
        }),
    );
    assert_eq!(rt.inspect("outer").as_deref(), Some("o"));
    assert_eq!(rt.inspect("inner").as_deref(), Some("i"));
}
