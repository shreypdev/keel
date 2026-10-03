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
use undra_bindgen::{BindgenError, Generator, SwiftObservation, validate};
use undra_meta::{InfiniteDef, PortKind, QueryKind, RecordDef, Schema, TypeRef};

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

/// `Option<Nickname>` where `struct Nickname(Option<String>)`: an option of an option on the wire.
fn option_of_a_newtype_of_an_option() -> Schema {
    let mut s = Schema::new("t");
    s.records.push(newtype_record(
        "Nickname",
        "",
        TypeRef::option(TypeRef::String),
    ));
    s.records.push(record(
        "Account",
        "",
        vec![field("nickname", TypeRef::option(named("Nickname")))],
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

fn object_in_a_field() -> Schema {
    let mut s = Schema::new("t");
    s.objects.push(object(
        "Child",
        "",
        vec![ctor("Child", "new", vec![], false)],
        vec![],
    ));
    s.records.push(record(
        "Parent",
        "",
        vec![field("child", TypeRef::object("Child"))],
    ));
    s
}

fn record_as_an_object() -> Schema {
    let mut s = Schema::new("t");
    s.records
        .push(record("Plain", "", vec![field("id", TypeRef::U32)]));
    s.objects.push(object(
        "Holder",
        "",
        vec![ctor("Holder", "new", vec![], false)],
        vec![method(
            "Holder",
            "take",
            "",
            vec![param("plain", TypeRef::object("Plain"))],
            TypeRef::Unit,
            false,
        )],
    ));
    s
}

fn object_in_a_signal() -> Schema {
    let mut s = Schema::new("t");
    s.objects.push(object(
        "Child",
        "",
        vec![ctor("Child", "new", vec![], false)],
        vec![],
    ));
    s.objects.push(store(
        object("Feed", "", vec![ctor("Feed", "new", vec![], false)], vec![]),
        vec![(
            "children",
            TypeRef::vec(TypeRef::object("Child")),
            false,
            None,
        )],
    ));
    s
}

fn callback_in_a_field() -> Schema {
    let mut s = Schema::new("t");
    s.ports.push(port(
        "Listener",
        "",
        PortKind::Callback,
        vec![port_method(
            "Listener",
            "changed",
            "",
            vec![],
            TypeRef::Unit,
            false,
        )],
    ));
    s.records.push(record(
        "Holder",
        "",
        vec![field("listener", TypeRef::callback("Listener"))],
    ));
    s
}

fn callback_without_a_port() -> Schema {
    let mut s = Schema::new("t");
    s.ports.push(port(
        "Platform",
        "",
        PortKind::Async,
        vec![port_method(
            "Platform",
            "ping",
            "",
            vec![],
            TypeRef::Unit,
            false,
        )],
    ));
    s.objects.push(object(
        "Service",
        "",
        vec![ctor("Service", "new", vec![], false)],
        vec![method(
            "Service",
            "use_it",
            "",
            vec![param("p", TypeRef::callback("Platform"))],
            TypeRef::Unit,
            false,
        )],
    ));
    s
}

fn callback_method_shapes() -> Schema {
    let mut s = Schema::new("t");
    s.ports.push(port(
        "Provider",
        "",
        PortKind::Callback,
        vec![
            port_method("Provider", "token", "", vec![], TypeRef::String, false),
            port_method("Provider", "ask", "", vec![], TypeRef::Unit, true),
            port_method("Provider", "__release", "", vec![], TypeRef::Unit, false),
        ],
    ));
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

fn decimal_map_key() -> Schema {
    let mut s = Schema::new("t");
    s.records.push(record(
        "Prices",
        "",
        vec![field(
            "by_amount",
            TypeRef::map(TypeRef::Decimal, TypeRef::U8),
        )],
    ));
    s
}

fn newtype_of_a_float_as_a_map_key() -> Schema {
    let mut s = Schema::new("t");
    s.records.push(newtype_record("Meters", "", TypeRef::F64));
    s.records.push(record(
        "Routes",
        "",
        vec![field(
            "by_length",
            TypeRef::map(named("Meters"), TypeRef::U8),
        )],
    ));
    s
}

fn newtype_with_two_fields() -> Schema {
    let mut s = Schema::new("t");
    s.records.push(RecordDef {
        transparent: true,
        ..record(
            "UserId",
            "",
            vec![field("value", TypeRef::Uuid), field("tenant", TypeRef::U32)],
        )
    });
    s
}

fn newtype_whose_field_is_not_called_value() -> Schema {
    let mut s = Schema::new("t");
    s.records.push(RecordDef {
        transparent: true,
        ..record("UserId", "", vec![field("inner", TypeRef::Uuid)])
    });
    s
}

fn infinite_query(returns: TypeRef, item_key: &str) -> Schema {
    let mut s = Schema::new("t");
    s.records
        .push(record("Post", "", vec![field("id", TypeRef::U64)]));
    let mut feed = query("feed", QueryKind::Query, "feed", vec![], returns, None);
    feed.infinite = Some(InfiniteDef {
        cursor: TypeRef::String,
        item_key: item_key.into(),
    });
    s.queries.push(feed);
    s
}

fn infinite_query_that_returns_one_record() -> Schema {
    infinite_query(named("Post"), "id")
}

fn infinite_query_with_a_key_that_is_not_a_field() -> Schema {
    infinite_query(TypeRef::vec(named("Post")), "slug")
}

fn infinite_mutation() -> Schema {
    let mut s = infinite_query(TypeRef::vec(named("Post")), "id");
    s.queries[0].kind = QueryKind::Mutation;
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

/// Fine as the default `@Observable` store, a collision in the iOS 15 / 16 shape (ADR-045).
fn store_member_named_like_observable_object() -> Schema {
    let mut s = Schema::new("t");
    s.objects.push(store(
        object(
            "Counter",
            "",
            vec![ctor("Counter", "new", vec![], false)],
            vec![],
        ),
        vec![("object_will_change", TypeRef::U32, false, None)],
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

/// `newest<Todo>` and `newest<Note>` over two records, the instantiations of one generic function.
fn two_instantiations() -> Schema {
    let mut s = Schema::new("t");
    s.records
        .push(record("Todo", "", vec![field("id", TypeRef::U32)]));
    s.records
        .push(record("Note", "", vec![field("id", TypeRef::U32)]));
    s.records
        .push(record("Tag", "", vec![field("id", TypeRef::U32)]));
    for arg in ["Todo", "Note"] {
        s.functions.push(instance_fn(
            "newest",
            arg,
            true,
            "",
            vec![param("rows", TypeRef::vec(named(arg)))],
            TypeRef::option(named(arg)),
            false,
        ));
    }
    s
}

/// A plain function whose id constant is the one of an instantiation (`newest_todo` and
/// `newest<Todo>` are both `newestTodo`).
fn instantiation_id_that_collides() -> Schema {
    let mut s = two_instantiations();
    s.functions
        .push(function("newest_todo", "", vec![], TypeRef::Unit, false));
    s
}

/// A plain function with the native name of a generic family.
fn plain_function_named_like_a_family() -> Schema {
    let mut s = two_instantiations();
    s.functions
        .push(function("newest", "", vec![], TypeRef::Unit, false));
    s
}

/// The name says `Todo`, the label says `Tag`.
fn label_that_does_not_describe_its_definition() -> Schema {
    let mut s = two_instantiations();
    s.functions[0].generic = Some(label("newest", "Tag", true));
    s
}

/// `newest<Note>` takes two parameters and `newest<Todo>` one.
fn instantiations_that_disagree() -> Schema {
    let mut s = two_instantiations();
    s.functions[0].params.push(param("limit", TypeRef::U32));
    s
}

/// `(code, case)`: the order of the cases of a code is the order of its golden.
const CASES: &[(&str, &[Case])] = &[
    (
        "E0001",
        &[
            ("an unknown type", unknown_type),
            (
                "an option of a newtype of an option",
                option_of_a_newtype_of_an_option,
            ),
            ("a unit field", unit_as_a_field),
        ],
    ),
    (
        "E0004",
        &[
            ("a callback in a record field", callback_in_a_field),
            (
                "a callback that names no callback port",
                callback_without_a_port,
            ),
        ],
    ),
    ("E0005", &[("a Result in a field", result_in_a_field)]),
    (
        "E0006",
        &[
            ("a float map key", float_map_key),
            ("a decimal map key", decimal_map_key),
            (
                "a newtype of a float as a map key",
                newtype_of_a_float_as_a_map_key,
            ),
        ],
    ),
    (
        "E0007",
        &[
            ("a newtype with two fields", newtype_with_two_fields),
            (
                "a newtype whose field is not called value",
                newtype_whose_field_is_not_called_value,
            ),
        ],
    ),
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
        "E0064",
        &[
            ("an object used as a value", object_as_a_value),
            ("an object in a record field", object_in_a_field),
            ("a record named as an object", record_as_an_object),
            ("an object in a signal", object_in_a_signal),
        ],
    ),
    (
        "E0071",
        &[(
            "callback methods that break the shape",
            callback_method_shapes,
        )],
    ),
    (
        "E0072",
        &[
            (
                "a label that does not describe its definition",
                label_that_does_not_describe_its_definition,
            ),
            (
                "instantiations that are not one function",
                instantiations_that_disagree,
            ),
        ],
    ),
    (
        "E0073",
        &[
            (
                "an infinite query that returns one record",
                infinite_query_that_returns_one_record,
            ),
            (
                "an infinite query whose item_key is not a field",
                infinite_query_with_a_key_that_is_not_a_field,
            ),
            ("an infinite mutation", infinite_mutation),
        ],
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
            (
                "a store member that an iOS 15 / 16 store already has",
                store_member_named_like_observable_object,
            ),
            (
                "an id constant that is the one of an instantiation",
                instantiation_id_that_collides,
            ),
            (
                "a function named like a generic one",
                plain_function_named_like_a_family,
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
    let schema = case();
    let mut errors: Vec<BindgenError> = validate(&schema).err().unwrap_or_default();
    // What only the iOS 15 / 16 Swift mode rejects (ADR-045).
    let mut floor = Generator::for_crate("t");
    floor.swift_observation = SwiftObservation::ObservableObject;
    errors.extend(floor.swift_floor_errors(&schema));
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
