//! Every diagnostic of `undra_bindgen::validate`, with a schema that triggers
//! exactly it.

mod common;

use common::*;
use undra_bindgen::{BindgenError, validate};
use undra_meta::{PortKind, QueryKind, Schema, TypeRef};

fn codes(schema: &Schema) -> Vec<&'static str> {
    validate(schema)
        .err()
        .unwrap_or_default()
        .iter()
        .map(BindgenError::code)
        .collect()
}

fn messages(schema: &Schema) -> String {
    validate(schema)
        .err()
        .unwrap_or_default()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

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

#[test]
fn a_valid_schema_passes() {
    for case in CASES {
        assert_eq!(codes(&case_schema(case)), Vec::<&str>::new(), "{case}");
    }
    assert!(validate(&Schema::new("empty")).is_ok());
}

fn case_schema(name: &str) -> Schema {
    common::case(name)
}

// ----- errors that come from undra-meta -------------------------------------------

#[test]
fn schema_errors_are_reported_alone() {
    let mut s = Schema::new("t");
    s.records
        .push(record("A", "", vec![field("x", named("Missing"))]));
    // Would also be a collision, but resolution errors come first and alone.
    s.records.push(record(
        "B",
        "",
        vec![field("a_b", TypeRef::U8), field("aB", TypeRef::U8)],
    ));
    assert_eq!(codes(&s), ["E0001"]);
}

#[test]
fn a_store_without_a_constructor_is_e0011() {
    let mut s = Schema::new("t");
    s.objects.push(store(
        object("S", "", vec![], vec![]),
        vec![("n", TypeRef::U32, false, None)],
    ));
    assert_eq!(codes(&s), ["E0011"]);
}

#[test]
fn duplicate_type_names_are_e0050() {
    let mut s = Schema::new("t");
    s.records.push(record("Same", "", vec![]));
    s.enums
        .push(enum_def("Same", "", vec![unit_variant("A", 0)]));
    assert_eq!(codes(&s), ["E0050"]);
}

// ----- E0050 --------------------------------------------------------------------

#[test]
fn a_port_named_like_a_type_is_e0050() {
    let mut s = Schema::new("t");
    s.records.push(record("Clock", "", vec![]));
    s.ports.push(port("Clock", "", PortKind::Sync, vec![]));
    assert_eq!(codes(&s), ["E0050"]);
    assert!(messages(&s).contains("port"));
}

#[test]
fn a_type_named_like_a_generated_or_standard_name_is_e0050() {
    for reserved in [
        "Map",
        "String",
        "UndraCore",
        "UndraIds",
        "Codec",
        "Signal",
        "Date",
    ] {
        let mut s = Schema::new("t");
        s.records.push(record(reserved, "", vec![]));
        assert_eq!(codes(&s), ["E0050"], "{reserved}");
    }
}

#[test]
fn query_names_that_collide_with_types_are_e0050() {
    let mut s = Schema::new("t");
    s.records.push(record("TodosQueryHandle", "", vec![]));
    s.queries.push(query(
        "todos",
        QueryKind::Query,
        "todos",
        vec![],
        TypeRef::U32,
        None,
    ));
    assert_eq!(codes(&s), ["E0050"]);

    let mut s = Schema::new("t");
    s.records.push(record("QueryStatus", "", vec![]));
    s.queries.push(query(
        "todos",
        QueryKind::Query,
        "todos",
        vec![],
        TypeRef::U32,
        None,
    ));
    assert_eq!(codes(&s), ["E0050"]);
}

#[test]
fn hash_collisions_are_e0050() {
    let mut s = Schema::new("t");
    let mut a = record("A", "", vec![]);
    let mut b = record("B", "", vec![]);
    a.type_id = 7;
    b.type_id = 7;
    s.records.push(a);
    s.records.push(b);
    assert_eq!(codes(&s), ["E0050"]);
    assert!(messages(&s).contains("0x00000007"));

    let mut m = one_object(vec![
        method("Thing", "a", "", vec![], TypeRef::Unit, false),
        method("Thing", "b", "", vec![], TypeRef::Unit, false),
    ]);
    m.objects[0].methods[1].method_id = m.objects[0].methods[0].method_id;
    assert_eq!(codes(&m), ["E0050"]);

    let mut d = Schema::new("t");
    d.enums.push(enum_def(
        "E",
        "",
        vec![unit_variant("A", 1), unit_variant("B", 1)],
    ));
    assert_eq!(codes(&d), ["E0050"]);
}

// ----- E0051 --------------------------------------------------------------------

#[test]
fn names_that_collide_after_case_conversion_are_e0051() {
    let mut s = Schema::new("t");
    s.records.push(record(
        "R",
        "",
        vec![field("foo_bar", TypeRef::U8), field("fooBar", TypeRef::U8)],
    ));
    assert_eq!(codes(&s), ["E0051"]);
    let text = messages(&s);
    assert!(
        text.contains("`foo_bar`") && text.contains("`fooBar`"),
        "{text}"
    );

    // Unit enum variants collide in Swift/TS camelCase and in Kotlin UPPER_SNAKE.
    let mut s = Schema::new("t");
    s.enums.push(enum_def(
        "E",
        "",
        vec![unit_variant("FooBar", 0), unit_variant("Foo_Bar", 1)],
    ));
    assert!(codes(&s).iter().all(|c| *c == "E0051"));
    assert!(!codes(&s).is_empty());

    // Parameters.
    let s = one_object(vec![method(
        "Thing",
        "m",
        "",
        vec![param("a_b", TypeRef::U8), param("aB", TypeRef::U8)],
        TypeRef::Unit,
        false,
    )]);
    assert_eq!(codes(&s), ["E0051"]);
}

#[test]
fn members_share_one_namespace_with_the_signals() {
    let mut s = Schema::new("t");
    s.objects.push(store(
        object(
            "S",
            "",
            vec![ctor("S", "new", vec![], false)],
            vec![method("S", "count", "", vec![], TypeRef::U32, false)],
        ),
        vec![("count", TypeRef::U32, false, None)],
    ));
    assert_eq!(codes(&s), ["E0051"]);

    // `new` becomes `create`, so a method called `create` collides.
    let s = one_object(vec![method(
        "Thing",
        "create",
        "",
        vec![],
        TypeRef::Unit,
        false,
    )]);
    assert_eq!(codes(&s), ["E0051"]);
}

#[test]
fn members_may_not_shadow_the_runtime_base_classes() {
    for name in ["close", "core", "handle", "apply"] {
        let s = one_object(vec![method(
            "Thing",
            name,
            "",
            vec![],
            TypeRef::Unit,
            false,
        )]);
        assert_eq!(codes(&s), ["E0051"], "{name}");
    }
}

#[test]
fn names_that_are_not_identifiers_are_e0051() {
    let mut s = Schema::new("t");
    s.records.push(record("a::B", "", vec![]));
    assert_eq!(codes(&s), ["E0051"]);
    let mut s = Schema::new("t");
    s.records
        .push(record("R", "", vec![field("1st", TypeRef::U8)]));
    assert_eq!(codes(&s), ["E0051"]);
}

// ----- E0001 --------------------------------------------------------------------

#[test]
fn lazy_signals_are_an_explicit_limitation() {
    let mut s = Schema::new("t");
    s.records.push(record("Item", "", vec![]));
    s.objects.push(store(
        object("S", "", vec![ctor("S", "new", vec![], false)], vec![]),
        vec![("archive", TypeRef::lazy(named("Item")), false, None)],
    ));
    assert_eq!(codes(&s), ["E0001"]);
    assert!(messages(&s).contains("Lazy"));
}

#[test]
fn objects_cannot_cross_as_values() {
    let mut s = Schema::new("t");
    s.objects.push(object(
        "A",
        "",
        vec![ctor("A", "new", vec![], false)],
        vec![],
    ));
    s.records
        .push(record("R", "", vec![field("a", named("A"))]));
    assert_eq!(codes(&s), ["E0064"]);
    s.records.clear();
    s.functions
        .push(function("f", "", vec![], named("A"), false));
    assert_eq!(codes(&s), ["E0064"]);
    // As an `object` reference it is fine in a function's parameter and return ...
    s.functions.clear();
    s.functions.push(function(
        "f",
        "",
        vec![param("a", TypeRef::object("A"))],
        TypeRef::object("A"),
        false,
    ));
    assert!(undra_bindgen::validate(&s).is_ok());
    // ... and not in a field.
    s.records
        .push(record("R", "", vec![field("a", TypeRef::object("A"))]));
    assert_eq!(codes(&s), ["E0064"]);
}

#[test]
fn an_object_nothing_returns_needs_a_constructor() {
    let mut s = Schema::new("t");
    s.objects.push(object("Orphan", "", vec![], vec![]));
    assert_eq!(codes(&s), ["E0001"]);
    // Returned by a method of another object, it is created by the core and needs none.
    s.objects.push(object(
        "Parent",
        "",
        vec![ctor("Parent", "new", vec![], false)],
        vec![method(
            "Parent",
            "orphan",
            "",
            vec![],
            TypeRef::option(TypeRef::object("Orphan")),
            false,
        )],
    ));
    assert!(undra_bindgen::validate(&s).is_ok(), "{:?}", codes(&s));
}

#[test]
fn callback_interfaces_have_their_own_names_and_reserved_methods() {
    let mut s = Schema::new("t");
    s.enums.push(error_def(
        "E",
        "",
        vec![with_message(unit_variant("X", 0), "x")],
    ));
    s.ports.push(port(
        "Listener",
        "",
        PortKind::Callback,
        vec![
            port_method(
                "Listener",
                "release_instance",
                "",
                vec![],
                TypeRef::Unit,
                false,
            ),
            port_method(
                "Listener",
                "ask",
                "",
                vec![],
                TypeRef::result(TypeRef::Bool, named("E")),
                true,
            ),
        ],
    ));
    assert_eq!(
        codes(&s),
        ["E0051"],
        "`releaseInstance` is reserved for `__release`"
    );
    s.ports[0].methods.remove(0);
    assert!(undra_bindgen::validate(&s).is_ok(), "{:?}", codes(&s));
    // A callback that is not a callback port, and a synchronous value method, are meta's.
    s.ports[0].methods.push(port_method(
        "Listener",
        "get",
        "",
        vec![],
        TypeRef::U32,
        false,
    ));
    assert_eq!(codes(&s), ["E0071"]);
}

#[test]
fn nested_options_cannot_be_represented() {
    let mut s = Schema::new("t");
    s.records.push(record(
        "R",
        "",
        vec![field("x", TypeRef::option(TypeRef::option(TypeRef::U8)))],
    ));
    assert_eq!(codes(&s), ["E0001"]);
}

#[test]
fn unit_is_only_a_return_type() {
    let mut s = Schema::new("t");
    s.records
        .push(record("R", "", vec![field("x", TypeRef::Unit)]));
    assert_eq!(codes(&s), ["E0001"]);
    let s = one_object(vec![method(
        "Thing",
        "m",
        "",
        vec![param("p", TypeRef::vec(TypeRef::Unit))],
        TypeRef::Unit,
        false,
    )]);
    assert_eq!(codes(&s), ["E0001"]);
    let s = one_object(vec![method(
        "Thing",
        "m",
        "",
        vec![],
        TypeRef::stream(TypeRef::Unit),
        true,
    )]);
    assert_eq!(codes(&s), ["E0001"]);
}

#[test]
fn a_result_needs_an_error_enum() {
    let mut s = Schema::new("t");
    s.records.push(record("R", "", vec![]));
    s.functions.push(function(
        "f",
        "",
        vec![],
        TypeRef::result(TypeRef::U8, named("R")),
        false,
    ));
    assert_eq!(codes(&s), ["E0001"]);

    let mut s = Schema::new("t");
    s.functions.push(function(
        "f",
        "",
        vec![],
        TypeRef::result(TypeRef::U8, TypeRef::String),
        false,
    ));
    assert_eq!(codes(&s), ["E0001"]);
}

#[test]
fn enums_need_variants_and_constructors_need_the_right_return() {
    let mut s = Schema::new("t");
    s.enums.push(enum_def("NoVariants", "", vec![]));
    assert_eq!(codes(&s), ["E0001"]);

    let mut s = Schema::new("t");
    let mut bad = ctor("A", "new", vec![], false);
    bad.returns = TypeRef::U32;
    s.objects.push(object("A", "", vec![bad], vec![]));
    assert_eq!(codes(&s), ["E0001"]);

    let mut s = Schema::new("t");
    s.objects.push(object("A", "", vec![], vec![]));
    assert_eq!(codes(&s), ["E0001"]);
}

#[test]
fn queries_return_values_that_are_not_options() {
    let mut s = Schema::new("t");
    s.queries.push(query(
        "q",
        QueryKind::Query,
        "q",
        vec![],
        TypeRef::option(TypeRef::U8),
        None,
    ));
    assert_eq!(codes(&s), ["E0001"]);
    let mut s = Schema::new("t");
    s.queries.push(query(
        "q",
        QueryKind::Query,
        "q",
        vec![],
        TypeRef::Unit,
        None,
    ));
    assert_eq!(codes(&s), ["E0001"]);
    // A mutation may return anything a function can.
    let mut s = Schema::new("t");
    s.queries.push(query(
        "m",
        QueryKind::Mutation,
        "m",
        vec![],
        TypeRef::Unit,
        None,
    ));
    assert!(validate(&s).is_ok());
}

#[test]
fn keyed_signals_must_be_lists() {
    let mut s = Schema::new("t");
    s.objects.push(store(
        object("S", "", vec![ctor("S", "new", vec![], false)], vec![]),
        vec![("n", TypeRef::U32, false, Some("id"))],
    ));
    assert_eq!(codes(&s), ["E0001"]);
}

// ----- E0010 and E0031 -------------------------------------------------------------

#[test]
fn error_variants_need_a_usable_message() {
    let mut s = Schema::new("t");
    s.enums.push(error_def(
        "E",
        "",
        vec![tuple_variant("Two", 0, vec![TypeRef::U8, TypeRef::U8])],
    ));
    assert_eq!(codes(&s), ["E0010"]);

    let mut s = Schema::new("t");
    s.enums.push(error_def(
        "E",
        "",
        vec![with_message(unit_variant("A", 0), "uses {missing}")],
    ));
    assert_eq!(codes(&s), ["E0010"]);

    // A single field without a message is `#[error(transparent)]`.
    let mut s = Schema::new("t");
    s.enums.push(error_def(
        "E",
        "",
        vec![tuple_variant("Wrapped", 0, vec![TypeRef::String])],
    ));
    assert!(validate(&s).is_ok());
}

#[test]
fn event_port_methods_do_not_return() {
    let mut s = Schema::new("t");
    s.ports.push(port(
        "P",
        "",
        PortKind::Event,
        vec![port_method("P", "m", "", vec![], TypeRef::U8, false)],
    ));
    assert_eq!(codes(&s), ["E0031"]);
}

#[test]
fn ports_and_queries_cannot_stream() {
    let mut s = Schema::new("t");
    s.ports.push(port(
        "P",
        "",
        PortKind::Async,
        vec![port_method(
            "P",
            "m",
            "",
            vec![],
            TypeRef::stream(TypeRef::U8),
            true,
        )],
    ));
    assert_eq!(codes(&s), ["E0001"]);
}

#[test]
fn every_problem_is_reported() {
    let mut s = Schema::new("t");
    s.records.push(record("Map", "", vec![]));
    s.records.push(record(
        "R",
        "",
        vec![field("a_b", TypeRef::U8), field("aB", TypeRef::U8)],
    ));
    // (A `()` field would be rejected by `Schema::validate`, which stops the other checks;
    // a keyed signal that is not a list is only bindgen's business.)
    s.objects.push(store(
        object("S", "", vec![ctor("S", "new", vec![], false)], vec![]),
        vec![("n", TypeRef::U32, false, Some("id"))],
    ));
    let found = codes(&s);
    assert!(
        found.contains(&"E0050") && found.contains(&"E0051") && found.contains(&"E0001"),
        "{found:?}"
    );
}

#[test]
fn diagnostics_have_the_spec_shape() {
    let mut s = Schema::new("t");
    s.records.push(record("Map", "", vec![]));
    let text = messages(&s);
    assert!(text.starts_with("error[undra::E0050]: "), "{text}");
}
