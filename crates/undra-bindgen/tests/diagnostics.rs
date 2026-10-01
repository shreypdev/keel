//! The diagnostics a schema raises when it is validated (`undra build`, `undra bindgen`, a runtime
//! that loads a core): the codes the macros cannot raise because they only exist for the whole
//! schema (E0050 to E0052), and the schema-level form of the ones they can.
//!
//! Each case is a schema that triggers it; what a user reads is locked in
//! `tests/golden/diagnostics/<code>.txt` (messages of one code, separated by a blank line, in
//! the order of the table below). The error-codes page of the site shows these files as the real
//! messages of codes that no macro test can show; regenerate them with
//! `UPDATE_GOLDEN=1 cargo test -p undra-bindgen --test diagnostics` and review the diff.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use common::*;
use undra_bindgen::{BindgenError, validate};
use undra_meta::{PortKind, Schema, TypeRef};

type Case = (&'static str, fn() -> Schema);

fn with_error(s: &mut Schema) {
    s.enums.push(error_def(
        "Oops",
        "",
        vec![with_message(unit_variant("Bad", 0), "bad")],
    ));
}

fn one_object(methods: Vec<undra_meta::MethodDef>) -> Schema {
    let mut s = Schema::new("t");
    with_error(&mut s);
    s.objects.push(object(
        "Thing",
        "",
        vec![ctor("Thing", "new", vec![], false)],
        methods,
    ));
    s
}

fn duplicate_type_name() -> Schema {
    let mut s = Schema::new("t");
    s.records.push(record("Todo", "", vec![]));
    s.enums
        .push(enum_def("Todo", "", vec![unit_variant("A", 0)]));
    s
}

fn port_named_like_a_type() -> Schema {
    let mut s = Schema::new("t");
    s.records.push(record("Storage", "", vec![]));
    s.ports.push(port("Storage", "", PortKind::Async, vec![]));
    s
}

fn type_named_like_a_generated_name() -> Schema {
    let mut s = Schema::new("t");
    s.records.push(record("Signal", "", vec![]));
    s
}

fn type_id_collision() -> Schema {
    let mut s = Schema::new("t");
    let mut a = record("Alpha", "", vec![]);
    let mut b = record("Beta", "", vec![]);
    a.type_id = 7;
    b.type_id = 7;
    s.records.push(a);
    s.records.push(b);
    s
}

fn duplicate_variant_index() -> Schema {
    let mut s = Schema::new("t");
    s.enums.push(enum_def(
        "Status",
        "",
        vec![unit_variant("Open", 1), unit_variant("Closed", 1)],
    ));
    s
}

fn fields_that_collide() -> Schema {
    let mut s = Schema::new("t");
    s.records.push(record(
        "Todo",
        "",
        vec![
            field("due_date", TypeRef::U64),
            field("dueDate", TypeRef::U64),
        ],
    ));
    s
}

fn member_that_shadows_the_runtime() -> Schema {
    one_object(vec![method(
        "Thing",
        "close",
        "",
        vec![],
        TypeRef::Unit,
        false,
    )])
}

fn name_that_is_not_an_identifier() -> Schema {
    let mut s = Schema::new("t");
    s.records
        .push(record("Todo", "", vec![field("1st", TypeRef::U8)]));
    s
}

fn standard_name_with_another_id() -> Schema {
    // Ids come from names, so the macros never do this; a hand-written schema can.
    let mut s = Schema::new("app-core");
    let mut request = record("HttpRequest", "", vec![field("url", TypeRef::String)]);
    request.type_id = 7;
    s.records.push(request);
    s
}

fn unknown_type() -> Schema {
    let mut s = Schema::new("t");
    s.records
        .push(record("Todo", "", vec![field("owner", named("User"))]));
    s
}

fn lazy_signal() -> Schema {
    let mut s = Schema::new("t");
    s.records.push(record("Item", "", vec![]));
    s.objects.push(store(
        object("Feed", "", vec![ctor("Feed", "new", vec![], false)], vec![]),
        vec![("archive", TypeRef::lazy(named("Item")), false, None)],
    ));
    s
}

fn object_as_a_value() -> Schema {
    let mut s = Schema::new("t");
    s.objects.push(object(
        "Child",
        "",
        vec![ctor("Child", "new", vec![], false)],
        vec![],
    ));
    s.records
        .push(record("Parent", "", vec![field("child", named("Child"))]));
    s
}

fn unit_as_a_field() -> Schema {
    let mut s = Schema::new("t");
    s.records
        .push(record("Todo", "", vec![field("marker", TypeRef::Unit)]));
    s
}

fn result_in_a_field() -> Schema {
    let mut s = Schema::new("t");
    with_error(&mut s);
    s.records.push(record(
        "Todo",
        "",
        vec![field(
            "outcome",
            TypeRef::result(TypeRef::U8, named("Oops")),
        )],
    ));
    s
}

fn float_map_key() -> Schema {
    let mut s = Schema::new("t");
    s.records.push(record(
        "Scores",
        "",
        vec![field("by_weight", TypeRef::map(TypeRef::F32, TypeRef::U8))],
    ));
    s
}

fn error_variant_without_a_message() -> Schema {
    let mut s = Schema::new("t");
    s.enums
        .push(error_def("Oops", "", vec![unit_variant("Bad", 0)]));
    s
}

fn store_without_a_constructor() -> Schema {
    let mut s = Schema::new("t");
    s.objects.push(store(
        object("Counter", "", vec![], vec![]),
        vec![("count", TypeRef::U32, false, None)],
    ));
    s
}

fn event_method_that_returns() -> Schema {
    let mut s = Schema::new("t");
    s.ports.push(port(
        "Connectivity",
        "",
        PortKind::Event,
        vec![port_method(
            "Connectivity",
            "changed",
            "",
            vec![],
            TypeRef::Bool,
            false,
        )],
    ));
    s
}

/// `(code, case)`: the order of the cases of a code is the order of its golden.
const CASES: &[(&str, &[Case])] = &[
    (
        "E0001",
        &[
            ("an unknown type", unknown_type),
            ("a lazy signal", lazy_signal),
            ("an object used as a value", object_as_a_value),
            ("a unit field", unit_as_a_field),
        ],
    ),
    ("E0005", &[("a Result in a field", result_in_a_field)]),
    ("E0006", &[("a float map key", float_map_key)]),
    (
        "E0010",
        &[(
            "a variant without a message",
            error_variant_without_a_message,
        )],
    ),
    (
        "E0011",
        &[("a store without a constructor", store_without_a_constructor)],
    ),
    (
        "E0031",
        &[("an event method that returns", event_method_that_returns)],
    ),
    (
        "E0050",
        &[
            ("a duplicate type name", duplicate_type_name),
            ("a port named like a type", port_named_like_a_type),
            (
                "a name the generated code depends on",
                type_named_like_a_generated_name,
            ),
            ("two types with one id", type_id_collision),
            ("two variants with one index", duplicate_variant_index),
        ],
    ),
    (
        "E0051",
        &[
            ("fields that collide after conversion", fields_that_collide),
            (
                "a member that shadows the runtime",
                member_that_shadows_the_runtime,
            ),
            (
                "a name that is not an identifier",
                name_that_is_not_an_identifier,
            ),
        ],
    ),
    (
        "E0052",
        &[(
            "a standard name with another id",
            standard_name_with_another_id,
        )],
    ),
];

fn golden_dir() -> PathBuf {
    manifest_dir().join("tests/golden/diagnostics")
}

fn update() -> bool {
    std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1")
}

/// The messages `case` raises with `code`, in order (a case may also raise others; they belong to
/// another code's golden).
fn raised(code: &str, case: fn() -> Schema) -> Vec<String> {
    let errors: Vec<BindgenError> = validate(&case()).err().unwrap_or_default();
    errors
        .iter()
        .filter(|e| e.code() == code)
        .map(ToString::to_string)
        .collect()
}

#[test]
fn every_message_has_what_why_fix_and_the_docs_link_of_its_code() {
    for (code, cases) in CASES {
        for (name, case) in *cases {
            let messages = raised(code, *case);
            assert!(!messages.is_empty(), "{code}: `{name}` raises no {code}");
            for message in messages {
                let lines: Vec<&str> = message.lines().collect();
                assert_eq!(lines.len(), 4, "{code} `{name}`: {message}");
                assert!(
                    lines[0].starts_with(&format!("error[undra::{code}]: ")),
                    "{message}"
                );
                for (line, prefix) in [(1, "  = note: "), (2, "  = help: ")] {
                    let text = lines[line].strip_prefix(prefix).unwrap_or_else(|| {
                        panic!("{code} `{name}`: line {line} is not `{prefix}..`: {message}")
                    });
                    assert!(text.len() > 12, "{code} `{name}`: too short: {message}");
                }
                assert_eq!(
                    lines[3],
                    format!("  = docs: https://shreypdev.github.io/undra/docs/errors.html#{code}"),
                    "{message}"
                );
            }
        }
    }
}

#[test]
fn the_messages_are_the_goldens() {
    let mut files: BTreeMap<&str, String> = BTreeMap::new();
    for (code, cases) in CASES {
        let mut parts = Vec::new();
        for (_, case) in *cases {
            parts.extend(raised(code, *case));
        }
        files.insert(code, parts.join("\n\n") + "\n");
    }
    let dir = golden_dir();
    if update() {
        std::fs::create_dir_all(&dir).unwrap();
        for (code, text) in &files {
            std::fs::write(dir.join(format!("{code}.txt")), text).unwrap();
        }
        return;
    }
    for (code, text) in &files {
        let path = dir.join(format!("{code}.txt"));
        let golden = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e}; run with UPDATE_GOLDEN=1", path.display()));
        assert_eq!(
            &golden,
            text,
            "{code}: the messages differ from {}; run UPDATE_GOLDEN=1 cargo test -p undra-bindgen --test diagnostics and review the diff",
            path.display()
        );
    }
}
