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
fn a_panicking_inspector_is_contained() {
    let tr = TestRuntime::new();
    let rt = tr.runtime();
    rt.register_inspector("boom", Arc::new(|| panic!("an inspector that breaks")));
    assert!(
        rt.inspect("boom").is_none(),
        "a panic is not an answer, and not a crash"
    );
    rt.register_inspector("fine", Arc::new(|| "{}".to_owned()));
    assert_eq!(rt.inspect("fine").as_deref(), Some("{}"));
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
