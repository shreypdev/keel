//! Tests of [`crate::schema_diff`]: every kind of change, whether it is breaking or additive, and
//! the order of the output.

use undra_meta::{
    EnumDef, FieldDef, FunctionDef, GenericArg, GenericOf, InfiniteDef, MethodDef, ObjectDef,
    ParamDef, PortDef, PortKind, QueryDef, QueryKind, RecordDef, Schema, SignalDef, StoreDef,
    TypeRef, VariantDef, ids,
};

use super::*;

// ----- builders -------------------------------------------------------------------------------

fn field(name: &str, ty: TypeRef, default: bool) -> FieldDef {
    FieldDef {
        name: name.into(),
        ty,
        default,
        docs: String::new(),
    }
}

fn record(name: &str, fields: Vec<FieldDef>) -> RecordDef {
    RecordDef {
        name: name.into(),
        type_id: ids::type_id(name),
        fields,
        transparent: false,
        docs: String::new(),
    }
}

fn variant(name: &str, index: u16, fields: Vec<FieldDef>, tuple: bool) -> VariantDef {
    VariantDef {
        name: name.into(),
        index,
        fields,
        tuple,
        message: None,
        docs: String::new(),
    }
}

fn enumeration(name: &str, is_error: bool, variants: Vec<VariantDef>) -> EnumDef {
    EnumDef {
        name: name.into(),
        type_id: ids::type_id(name),
        is_error,
        variants,
        docs: String::new(),
    }
}

fn param(name: &str, ty: TypeRef) -> ParamDef {
    ParamDef {
        name: name.into(),
        ty,
    }
}

fn method(owner: &str, name: &str, params: Vec<ParamDef>, returns: TypeRef) -> MethodDef {
    MethodDef {
        name: name.into(),
        method_id: ids::method_id(owner, name),
        params,
        returns,
        is_async: false,
        takes_ctx: false,
        coalesce: false,
        generic: None,
        docs: String::new(),
    }
}

fn function(name: &str, params: Vec<ParamDef>, returns: TypeRef) -> FunctionDef {
    FunctionDef {
        name: name.into(),
        method_id: ids::function_id(name),
        params,
        returns,
        is_async: false,
        takes_ctx: false,
        generic: None,
        docs: String::new(),
    }
}

fn signal(name: &str, id: u32, ty: TypeRef) -> SignalDef {
    SignalDef {
        name: name.into(),
        signal_id: id,
        ty,
        computed: false,
        key: None,
        no_coalesce: false,
        default: false,
    }
}

fn object(name: &str, methods: Vec<MethodDef>) -> ObjectDef {
    ObjectDef {
        name: name.into(),
        type_id: ids::type_id(name),
        constructors: vec![method(name, "new", vec![], TypeRef::named(name))],
        methods,
        store: None,
        docs: String::new(),
    }
}

fn store(name: &str, signals: Vec<SignalDef>, methods: Vec<MethodDef>) -> ObjectDef {
    ObjectDef {
        store: Some(StoreDef { signals }),
        ..object(name, methods)
    }
}

fn port(name: &str, kind: PortKind, methods: Vec<MethodDef>) -> PortDef {
    PortDef {
        name: name.into(),
        port_id: ids::port_id(name),
        kind,
        background: false,
        methods,
        docs: String::new(),
    }
}

fn query(name: &str, kind: QueryKind, params: Vec<ParamDef>, returns: TypeRef) -> QueryDef {
    QueryDef {
        name: name.into(),
        query_id: ids::fnv1a32(&format!("query.{name}")),
        kind,
        key: format!("{name}/{{id}}"),
        params,
        returns,
        stale_ms: None,
        persist: false,
        idempotent: false,
        interval_ms: None,
        poll_in_background: false,
        infinite: None,
    }
}

fn schema() -> Schema {
    Schema::new("demo-core")
}

/// The lines of the diff of `old` and `new`, as printed.
fn lines(old: &Schema, new: &Schema) -> Vec<String> {
    diff(old, new).changes.iter().map(Change::line).collect()
}

fn with_record(fields: Vec<FieldDef>) -> Schema {
    let mut s = schema();
    s.records.push(record("Todo", fields));
    s
}

fn with_enum(is_error: bool, variants: Vec<VariantDef>) -> Schema {
    let mut s = schema();
    s.enums.push(enumeration("Status", is_error, variants));
    s
}

fn with_function(f: FunctionDef) -> Schema {
    let mut s = schema();
    s.functions.push(f);
    s
}

fn with_object(o: ObjectDef) -> Schema {
    let mut s = schema();
    s.objects.push(o);
    s
}

// ----- nothing changed --------------------------------------------------------------------------

#[test]
fn an_unchanged_schema_has_no_changes_and_says_so() {
    let s = with_record(vec![field("title", TypeRef::String, false)]);
    let d = diff(&s, &s.clone());
    assert!(d.changes.is_empty());
    assert!(!d.has_breaking());
    assert_eq!(
        d.summary(),
        format!(
            "No changes: the public API is the same (schema {:#018x}).",
            s.hash()
        )
    );
}

#[test]
fn doc_comments_and_labels_are_not_the_api() {
    let old = with_record(vec![field("title", TypeRef::String, false)]);
    let mut new = old.clone();
    new.crate_name = "renamed-core".into();
    new.undra_version = "9.9.9".into();
    new.records[0].docs = "Now documented.".into();
    new.records[0].fields[0].docs = "The title.".into();
    assert!(diff(&old, &new).changes.is_empty());
    // A schema exported without docs and one with them are the same API.
    assert!(diff(&new, &new.without_docs()).changes.is_empty());
}

#[test]
fn different_hashes_with_nothing_to_report_are_not_called_unchanged() {
    let old = with_record(vec![field("title", TypeRef::String, false)]);
    let mut new = old.clone();
    new.records[0].type_id ^= 1; // an id no rule reads
    let d = diff(&old, &new);
    assert!(d.changes.is_empty());
    assert_ne!(d.old_hash, d.new_hash);
    let summary = d.summary();
    assert!(
        summary.contains("hashes differ") && summary.contains("a wire id differs"),
        "{summary}"
    );
}

// ----- records ----------------------------------------------------------------------------------

#[test]
fn a_record_added_is_additive_and_removed_is_breaking() {
    let some = with_record(vec![field("title", TypeRef::String, false)]);
    assert_eq!(lines(&schema(), &some), ["additive  record Todo: added"]);
    assert_eq!(lines(&some, &schema()), ["breaking  record Todo: removed"]);
}

#[test]
fn a_field_added_without_a_default_breaks_every_initializer() {
    let old = with_record(vec![field("title", TypeRef::String, false)]);
    let required = with_record(vec![
        field("title", TypeRef::String, false),
        field("priority", TypeRef::U8, false),
    ]);
    assert_eq!(
        lines(&old, &required),
        [
            "breaking  field Todo.priority: added without a default (u8): everything that builds a `Todo` must supply it"
        ]
    );
}

#[test]
fn a_field_added_with_a_default_still_breaks_a_typescript_object_literal() {
    // A TypeScript record is an interface whose every member is required: an object literal typed
    // `Todo` that does not name the new field stops compiling, default or not. Swift's memberwise
    // initializer and Kotlin's data class constructor default it, so they are not the reason.
    let old = with_record(vec![field("title", TypeRef::String, false)]);
    let defaulted = with_record(vec![
        field("title", TypeRef::String, false),
        field("due", TypeRef::option(TypeRef::Timestamp), true),
    ]);
    assert_eq!(
        lines(&old, &defaulted),
        [
            "breaking  field Todo.due: added (Option<Timestamp>, with a default): Swift and Kotlin initializers default it, \
             but a TypeScript object literal that builds a `Todo` must name it"
        ]
    );
}

#[test]
fn a_defaulted_field_inserted_before_another_shifts_kotlin_positional_arguments() {
    // `Todo(id, "x", true)` meant `done = true`; with `pinned: Boolean = false` before `done` it
    // means `pinned = true`, and `val (id, title, done) = todo` reads `pinned`.
    let old = with_record(vec![
        field("title", TypeRef::String, false),
        field("done", TypeRef::Bool, true),
    ]);
    let inserted = with_record(vec![
        field("title", TypeRef::String, false),
        field("pinned", TypeRef::Bool, true),
        field("done", TypeRef::Bool, true),
    ]);
    assert_eq!(
        lines(&old, &inserted),
        [
            "breaking  field Todo.pinned: added (bool, with a default) before `done`: Kotlin's positional arguments and \
             destructuring of a `Todo` shift, and a TypeScript object literal that builds one must name it"
        ]
    );
    // Two new fields at the end are both appended: neither moves an old one.
    let appended = with_record(vec![
        field("title", TypeRef::String, false),
        field("done", TypeRef::Bool, true),
        field("pinned", TypeRef::Bool, true),
        field("rank", TypeRef::U8, true),
    ]);
    let found = lines(&old, &appended);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found
            .iter()
            .all(|l| l.contains("Swift and Kotlin initializers default it")),
        "{found:?}"
    );
}

#[test]
fn a_defaulted_field_of_a_record_or_enum_type_has_no_generated_default() {
    // Swift and Kotlin spell the default of a primitive, a string, an optional and a collection,
    // never of a named type: the initializer requires it as if it had no default.
    let old = with_record(vec![field("title", TypeRef::String, false)]);
    let named = with_record(vec![
        field("title", TypeRef::String, false),
        field("priority", TypeRef::named("Priority"), true),
    ]);
    assert_eq!(
        lines(&old, &named),
        [
            "breaking  field Todo.priority: added (Priority, with a default no generated initializer spells): everything \
             that builds a `Todo` must supply it"
        ]
    );
    // For the same reason, losing that default changes no initializer.
    let undefaulted = with_record(vec![
        field("title", TypeRef::String, false),
        field("priority", TypeRef::named("Priority"), false),
    ]);
    assert_eq!(
        lines(&named, &undefaulted),
        ["additive  field Todo.priority: lost its default (no generated initializer spelled it)"]
    );
}

/// The parameter lines of the initializer (Swift) or constructor (Kotlin) of the record `name`.
fn initializer_lines(text: &str, open: &str, close: &str) -> Vec<String> {
    let start = text.find(open).unwrap_or_else(|| panic!("no {open}")) + open.len();
    let end = start + text[start..].find(close).expect("the block closes");
    text[start..end]
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("/**") && !l.starts_with("*"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_rule_for_a_defaulted_field_is_what_the_generators_emit() {
    // One defaulted field of every kind a record can hold; the rules above say which of them a
    // Swift initializer and a Kotlin constructor default. Generate both and compare, so the rule
    // cannot drift from the generators. TypeScript never does: every interface member is required.
    let kinds: Vec<(&str, TypeRef)> = vec![
        ("flag", TypeRef::Bool),
        ("small", TypeRef::U8),
        ("wide", TypeRef::I64),
        ("ratio", TypeRef::F64),
        ("text", TypeRef::String),
        ("blob", TypeRef::Bytes),
        ("span", TypeRef::Duration),
        ("at", TypeRef::Timestamp),
        ("uid", TypeRef::Uuid),
        ("amount", TypeRef::Decimal),
        ("maybe", TypeRef::option(TypeRef::String)),
        ("list", TypeRef::vec(TypeRef::String)),
        ("table", TypeRef::map(TypeRef::String, TypeRef::U8)),
        ("priority", TypeRef::named("Priority")),
        ("point", TypeRef::named("Point")),
    ];
    let mut s = schema();
    s.records.push(record(
        "Defaults",
        kinds
            .iter()
            .map(|(name, ty)| field(name, ty.clone(), true))
            .collect(),
    ));
    s.records.push(record(
        "Point",
        vec![
            field("x", TypeRef::I32, false),
            field("y", TypeRef::I32, false),
        ],
    ));
    s.enums.push(enumeration(
        "Priority",
        false,
        vec![
            variant("Low", 0, vec![], false),
            variant("High", 1, vec![], false),
        ],
    ));
    let generator = undra_bindgen::Generator::for_crate("demo-core");
    let file = |files: Vec<undra_bindgen::GeneratedFile>, suffix: &str| {
        files
            .into_iter()
            .find(|f| f.path.ends_with(suffix))
            .unwrap_or_else(|| panic!("no {suffix}"))
            .contents
    };
    let swift = file(generator.swift(&s).expect("Swift generates"), "Types.swift");
    let kotlin = file(generator.kotlin(&s).expect("Kotlin generates"), "Types.kt");
    let ts = file(
        generator.typescript(&s).expect("TypeScript generates"),
        "types.ts",
    );
    let swift_init = initializer_lines(
        &swift[swift.find("public struct Defaults").unwrap()..],
        "public init(",
        ") {",
    );
    let kotlin_init = initializer_lines(
        &kotlin[kotlin.find("data class Defaults(").unwrap()..],
        "data class Defaults(",
        ") :",
    );
    let ts_members = initializer_lines(
        &ts[ts.find("export interface Defaults").unwrap()..],
        "{",
        "}",
    );
    for (name, ty) in &kinds {
        let expected = has_generated_default(ty);
        let swift_line = swift_init
            .iter()
            .find(|l| l.starts_with(&format!("{name}:")))
            .unwrap_or_else(|| panic!("no Swift parameter {name} in {swift_init:?}"));
        let kotlin_line = kotlin_init
            .iter()
            .find(|l| l.starts_with(&format!("val {name}:")))
            .unwrap_or_else(|| panic!("no Kotlin parameter {name} in {kotlin_init:?}"));
        assert_eq!(swift_line.contains(" = "), expected, "Swift {swift_line}");
        assert_eq!(
            kotlin_line.contains(" = "),
            expected,
            "Kotlin {kotlin_line}"
        );
        let ts_line = ts_members
            .iter()
            .find(|l| l.starts_with(*name))
            .unwrap_or_else(|| panic!("no TypeScript member {name} in {ts_members:?}"));
        assert!(
            ts_line.starts_with(&format!("{name}: ")),
            "a TypeScript member is required, never `{name}?:`: {ts_line}"
        );
    }
}

#[test]
fn a_field_removed_retyped_or_losing_its_default_is_breaking() {
    let old = with_record(vec![
        field("title", TypeRef::String, false),
        field("done", TypeRef::Bool, true),
    ]);
    assert_eq!(
        lines(&old, &with_record(vec![field("done", TypeRef::Bool, true)])),
        ["breaking  field Todo.title: removed (was String)"]
    );
    assert_eq!(
        lines(
            &old,
            &with_record(vec![
                field("title", TypeRef::option(TypeRef::String), false),
                field("done", TypeRef::Bool, true),
            ])
        ),
        ["breaking  field Todo.title: type changed from String to Option<String>"]
    );
    assert_eq!(
        lines(
            &old,
            &with_record(vec![
                field("title", TypeRef::String, false),
                field("done", TypeRef::Bool, false),
            ])
        ),
        ["breaking  field Todo.done: lost its default"]
    );
}

#[test]
fn a_field_gaining_a_default_is_additive() {
    let old = with_record(vec![field("done", TypeRef::Bool, false)]);
    let new = with_record(vec![field("done", TypeRef::Bool, true)]);
    assert_eq!(
        lines(&old, &new),
        ["additive  field Todo.done: gained a default"]
    );
}

#[test]
fn reordering_fields_is_breaking_and_names_both_orders() {
    let old = with_record(vec![
        field("id", TypeRef::U32, false),
        field("title", TypeRef::String, false),
    ]);
    let new = with_record(vec![
        field("title", TypeRef::String, false),
        field("id", TypeRef::U32, false),
    ]);
    let found = lines(&old, &new);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].starts_with("breaking  record Todo: fields reordered (id, title -> title, id)"),
        "{found:?}"
    );
}

#[test]
fn becoming_a_newtype_is_breaking_both_ways() {
    let old = with_record(vec![field("value", TypeRef::U64, false)]);
    let mut new = old.clone();
    new.records[0].transparent = true;
    assert_eq!(
        lines(&old, &new),
        ["breaking  record Todo: is now a newtype"]
    );
    assert_eq!(
        lines(&new, &old),
        ["breaking  record Todo: is no longer a newtype"]
    );
}

// ----- enums and errors --------------------------------------------------------------------------

#[test]
fn enums_and_errors_are_added_and_removed_by_their_own_noun() {
    let en = with_enum(false, vec![variant("Open", 0, vec![], false)]);
    let er = with_enum(true, vec![variant("Open", 0, vec![], false)]);
    assert_eq!(lines(&schema(), &en), ["additive  enum Status: added"]);
    assert_eq!(lines(&schema(), &er), ["additive  error Status: added"]);
    assert_eq!(lines(&er, &schema()), ["breaking  error Status: removed"]);
    assert_eq!(
        lines(&en, &er),
        ["breaking  error Status: is now an error enum"]
    );
    assert_eq!(
        lines(&er, &en),
        ["breaking  enum Status: is no longer an error enum"]
    );
}

#[test]
fn a_case_added_breaks_an_exhaustive_match() {
    let old = with_enum(false, vec![variant("Open", 0, vec![], false)]);
    let new = with_enum(
        false,
        vec![
            variant("Open", 0, vec![], false),
            variant("Archived", 1, vec![], false),
        ],
    );
    let found = lines(&old, &new);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0]
            .starts_with("breaking  case Status.Archived: added (an exhaustive `switch` or `when`"),
        "{found:?}"
    );
    assert_eq!(
        lines(&new, &old),
        ["breaking  case Status.Archived: removed"]
    );
}

#[test]
fn a_case_whose_index_or_payload_changed_is_breaking() {
    let old = with_enum(
        false,
        vec![
            variant("Open", 0, vec![], false),
            variant("Done", 1, vec![field("0", TypeRef::Timestamp, false)], true),
        ],
    );
    let moved = with_enum(
        false,
        vec![
            variant("Open", 1, vec![], false),
            variant("Done", 0, vec![field("0", TypeRef::Timestamp, false)], true),
        ],
    );
    assert_eq!(
        lines(&old, &moved),
        [
            "breaking  case Status.Open: index changed from 0 to 1 (its wire value moved)",
            "breaking  case Status.Done: index changed from 1 to 0 (its wire value moved)",
        ]
    );
    let retyped = with_enum(
        false,
        vec![
            variant("Open", 0, vec![], false),
            variant(
                "Done",
                1,
                vec![
                    field("0", TypeRef::Timestamp, false),
                    field("1", TypeRef::String, false),
                ],
                true,
            ),
        ],
    );
    assert_eq!(
        lines(&old, &retyped),
        ["breaking  case Status.Done: payload changed from (Timestamp) to (Timestamp, String)"]
    );
    let named = with_enum(
        false,
        vec![
            variant("Open", 0, vec![], false),
            variant(
                "Done",
                1,
                vec![field("at", TypeRef::Timestamp, false)],
                false,
            ),
        ],
    );
    assert_eq!(
        lines(&old, &named),
        ["breaking  case Status.Done: payload changed from (Timestamp) to { at: Timestamp }"]
    );
}

#[test]
fn an_error_message_changing_is_additive() {
    let mut old = with_enum(true, vec![variant("Missing", 0, vec![], false)]);
    old.enums[0].variants[0].message = Some("not found".into());
    let mut new = old.clone();
    new.enums[0].variants[0].message = Some("no such item".into());
    assert_eq!(
        lines(&old, &new),
        ["additive  case Status.Missing: message changed"]
    );
}

#[test]
fn a_case_fields_default_is_reported_and_additive() {
    // The default of a case's field moves the hash (SPEC 2.3) and only a migration reads it: no
    // generator spells a default in a case's payload. It is a line, not an unexplained hash change.
    let at = |default: bool| {
        with_enum(
            false,
            vec![variant(
                "Done",
                0,
                vec![field("at", TypeRef::Timestamp, default)],
                false,
            )],
        )
    };
    assert_eq!(
        lines(&at(false), &at(true)),
        [
            "additive  case Status.Done: field `at` gained a default (only a migration reads it; no generated case spells one)"
        ]
    );
    assert_eq!(
        lines(&at(true), &at(false)),
        [
            "additive  case Status.Done: field `at` lost its default (only a migration reads it; no generated case spells one)"
        ]
    );
}

// ----- functions, methods, parameters -------------------------------------------------------------

#[test]
fn a_function_added_is_additive_and_removed_is_breaking_with_its_signature() {
    let f = function(
        "archive",
        vec![param("id", TypeRef::named("TodoId"))],
        TypeRef::result(TypeRef::Unit, TypeRef::named("TodoError")),
    );
    let with = with_function(f);
    assert_eq!(
        lines(&schema(), &with),
        ["additive  function archive: added (fn archive(id: TodoId) -> Result<(), TodoError>)"]
    );
    assert_eq!(
        lines(&with, &schema()),
        [
            "breaking  function archive: removed (was fn archive(id: TodoId) -> Result<(), TodoError>)"
        ]
    );
}

#[test]
fn an_async_function_says_so_in_its_signature() {
    let mut f = function("ping", vec![], TypeRef::Unit);
    f.is_async = true;
    assert_eq!(
        lines(&schema(), &with_function(f)),
        ["additive  function ping: added (async fn ping())"]
    );
}

#[test]
fn every_change_to_a_signature_is_breaking() {
    let base = || {
        function(
            "add",
            vec![
                param("title", TypeRef::String),
                param("tags", TypeRef::vec(TypeRef::String)),
            ],
            TypeRef::named("Todo"),
        )
    };
    let old = with_function(base());
    let change = |edit: &dyn Fn(&mut FunctionDef)| {
        let mut f = base();
        edit(&mut f);
        lines(&old, &with_function(f))
    };
    assert_eq!(
        change(&|f| f.params.push(param("pinned", TypeRef::Bool))),
        ["breaking  function add: parameter `pinned: bool` added"]
    );
    assert_eq!(
        change(&|f| {
            f.params.pop();
        }),
        ["breaking  function add: parameter `tags: Vec<String>` removed"]
    );
    assert_eq!(
        change(&|f| f.params[0].ty = TypeRef::option(TypeRef::String)),
        ["breaking  function add: parameter `title`: type changed from String to Option<String>"]
    );
    assert_eq!(
        change(&|f| f.params[0].name = "name".into()),
        ["breaking  function add: parameter `title` renamed to `name`"]
    );
    assert_eq!(
        change(&|f| f.params.reverse()),
        ["breaking  function add: parameters reordered (title, tags -> tags, title)"]
    );
    assert_eq!(
        change(&|f| f.returns = TypeRef::option(TypeRef::named("Todo"))),
        ["breaking  function add: return type changed from Todo to Option<Todo>"]
    );
    assert_eq!(
        change(&|f| f.is_async = true),
        ["breaking  function add: now `async`"]
    );
    // `Ctx` is not a parameter on the wire and no generator reads `takes_ctx`: every generated
    // function takes the core the same way, so the change is the core's own business.
    assert_eq!(
        change(&|f| f.takes_ctx = true),
        [
            "additive  function add: now takes a `Ctx` (inside the core: no generated signature shows it)"
        ]
    );
    assert_eq!(
        change(&|f| {
            f.generic = Some(GenericOf {
                of: "add".into(),
                args: vec![GenericArg {
                    param: "T".into(),
                    ty: TypeRef::named("Todo"),
                    inferred: false,
                }],
            });
        }),
        ["breaking  function add: generic instantiation changed from none to add<Todo>"]
    );
}

#[test]
fn several_reasons_are_several_lines_in_a_fixed_order() {
    let old = with_function(function(
        "add",
        vec![param("title", TypeRef::String)],
        TypeRef::Unit,
    ));
    let new = with_function(function(
        "add",
        vec![param("title", TypeRef::I32), param("tags", TypeRef::U8)],
        TypeRef::Bool,
    ));
    assert_eq!(
        lines(&old, &new),
        [
            "breaking  function add: parameter `tags: u8` added",
            "breaking  function add: parameter `title`: type changed from String to i32",
            "breaking  function add: return type changed from () to bool",
        ]
    );
}

#[test]
fn methods_and_constructors_of_an_object_follow_the_same_rules() {
    let old = with_object(object(
        "Mailbox",
        vec![method(
            "Mailbox",
            "send",
            vec![param("body", TypeRef::String)],
            TypeRef::Unit,
        )],
    ));
    let mut new_object = object(
        "Mailbox",
        vec![
            method(
                "Mailbox",
                "send",
                vec![param("body", TypeRef::Bytes)],
                TypeRef::Unit,
            ),
            method("Mailbox", "count", vec![], TypeRef::U32),
        ],
    );
    new_object.constructors.push(method(
        "Mailbox",
        "with_capacity",
        vec![param("n", TypeRef::U32)],
        TypeRef::named("Mailbox"),
    ));
    let new = with_object(new_object);
    assert_eq!(
        lines(&old, &new),
        [
            "additive  constructor Mailbox.with_capacity: added (fn with_capacity(n: u32) -> Mailbox)",
            "additive  method Mailbox.count: added (fn count() -> u32)",
            "breaking  method Mailbox.send: parameter `body`: type changed from String to Bytes",
        ]
    );
    assert_eq!(
        lines(&new, &old),
        [
            "breaking  constructor Mailbox.with_capacity: removed (was fn with_capacity(n: u32) -> Mailbox)",
            "breaking  method Mailbox.count: removed (was fn count() -> u32)",
            "breaking  method Mailbox.send: parameter `body`: type changed from Bytes to String",
        ]
    );
}

#[test]
fn objects_and_arcs_are_spelled_as_rust_writes_them() {
    let f = function(
        "open",
        vec![
            param("inbox", TypeRef::option(TypeRef::object("Mailbox"))),
            param("listener", TypeRef::callback("UploadListener")),
        ],
        TypeRef::result(
            TypeRef::stream(TypeRef::map(
                TypeRef::String,
                TypeRef::lazy(TypeRef::Decimal),
            )),
            TypeRef::named("OpenError"),
        ),
    );
    assert_eq!(
        lines(&schema(), &with_function(f)),
        [
            "additive  function open: added (fn open(inbox: Option<Arc<Mailbox>>, listener: Arc<dyn UploadListener>) -> Result<Stream<Map<String, Lazy<Decimal>>>, OpenError>)"
        ]
    );
}

// ----- objects and stores ------------------------------------------------------------------------

#[test]
fn objects_and_stores_are_added_and_removed_by_their_own_noun() {
    let o = with_object(object("Mailbox", vec![]));
    let s = with_object(store(
        "Ledger",
        vec![signal("balance", 0, TypeRef::I64)],
        vec![],
    ));
    assert_eq!(lines(&schema(), &o), ["additive  object Mailbox: added"]);
    assert_eq!(lines(&schema(), &s), ["additive  store Ledger: added"]);
    assert_eq!(lines(&s, &schema()), ["breaking  store Ledger: removed"]);
    assert_eq!(lines(&o, &schema()), ["breaking  object Mailbox: removed"]);
}

#[test]
fn an_object_that_becomes_a_store_or_stops_being_one_is_breaking() {
    let o = with_object(object("Ledger", vec![]));
    let s = with_object(store(
        "Ledger",
        vec![signal("balance", 0, TypeRef::I64)],
        vec![],
    ));
    let up = lines(&o, &s);
    assert_eq!(up.len(), 1, "{up:?}");
    assert!(
        up[0].starts_with("breaking  store Ledger: was an object and is now a store"),
        "{up:?}"
    );
    let down = lines(&s, &o);
    assert_eq!(down.len(), 1, "{down:?}");
    assert!(
        down[0].starts_with("breaking  object Ledger: was a store and is now an object"),
        "{down:?}"
    );
}

#[test]
fn signals_follow_the_rules_of_a_store() {
    let base = || {
        with_object(store(
            "Ledger",
            vec![
                signal("balance", 0, TypeRef::I64),
                signal("owner", 1, TypeRef::String),
            ],
            vec![],
        ))
    };
    let change = |edit: &dyn Fn(&mut Vec<SignalDef>)| {
        let mut new = base();
        edit(&mut new.objects[0].store.as_mut().unwrap().signals);
        lines(&base(), &new)
    };
    assert_eq!(
        change(&|s| s.push(signal("limit", 2, TypeRef::option(TypeRef::I64)))),
        ["additive  signal Ledger.limit: added (Option<i64>)"]
    );
    assert_eq!(
        change(&|s| {
            let mut derived = signal("label", 2, TypeRef::String);
            derived.computed = true;
            s.push(derived);
        }),
        ["additive  signal Ledger.label: added (String, computed)"]
    );
    assert_eq!(
        change(&|s| {
            s.pop();
        }),
        ["breaking  signal Ledger.owner: removed (was String)"]
    );
    assert_eq!(
        change(&|s| s[0].ty = TypeRef::Decimal),
        ["breaking  signal Ledger.balance: type changed from i64 to Decimal"]
    );
    // Every signal is read-only on the platforms and a key only changes how a list's changes travel:
    // the generated property is the same either way.
    assert_eq!(
        change(&|s| s[0].computed = true),
        [
            "additive  signal Ledger.balance: is now computed (the platforms read it as before: every signal is read-only there)"
        ]
    );
    assert_eq!(
        change(&|s| s[0].key = Some("id".into())),
        [
            "additive  signal Ledger.balance: key changed from none to `id` (how its changes travel; the property is the same)"
        ]
    );
    let flags = change(&|s| {
        s[0].no_coalesce = true;
        s[1].default = true;
    });
    assert_eq!(flags.len(), 2, "{flags:?}");
    assert!(
        flags[0].starts_with("additive  signal Ledger.balance: is now `no_coalesce`"),
        "{flags:?}"
    );
    assert_eq!(flags[1], "additive  signal Ledger.owner: gained a default");
    let reordered = change(&|s| s.reverse());
    assert_eq!(reordered.len(), 1, "{reordered:?}");
    assert!(
        reordered[0].starts_with(
            "additive  store Ledger: signals reordered (balance, owner -> owner, balance)"
        ),
        "{reordered:?}"
    );
}

// ----- ports and callbacks ------------------------------------------------------------------------

fn with_port(p: PortDef) -> Schema {
    let mut s = schema();
    s.ports.push(p);
    s
}

#[test]
fn a_port_the_app_must_implement_is_breaking_to_add() {
    let mine = with_port(port("Biometrics", PortKind::Async, vec![]));
    let found = lines(&schema(), &mine);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].starts_with("breaking  port Biometrics: added (the app must supply an adapter"),
        "{found:?}"
    );
    assert_eq!(
        lines(&mine, &schema()),
        ["breaking  port Biometrics: removed"]
    );
    // A port that only shares a standard name is the app's own (SPEC 10.5: standard means the same
    // name, id and shape), so the app implements it like any other.
    let named_like_kv = with_port(port("Kv", PortKind::Async, vec![]));
    let found = lines(&schema(), &named_like_kv);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].starts_with("breaking  port Kv: added (the app must supply an adapter"),
        "{found:?}"
    );
}

/// The schema every core has: the standard library of `undra-ports`, all opt-in ports included
/// (the golden schema of `undra-bindgen`'s stdlib case, which its tests pin to the registrations).
fn standard_library() -> Schema {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../undra-bindgen/tests/golden/stdlib/schema.json");
    Schema::from_json(&std::fs::read_to_string(&path).expect("the stdlib golden schema"))
        .expect("it parses")
}

fn without_port(mut s: Schema, name: &str) -> Schema {
    s.ports.retain(|p| p.name != name);
    s
}

#[test]
fn a_standard_port_is_additive_to_add_but_an_opt_in_one_needs_a_web_adapter() {
    let full = standard_library();
    // A port of SPEC 8 (here as an Undra upgrade would bring one): every runtime registers it.
    let found = lines(&without_port(full.clone(), "Clock"), &full);
    assert_eq!(
        found,
        [
            "additive  port Clock: added (a standard port: the runtimes ship and register its adapter)"
        ]
    );
    // An opt-in port of SPEC 8.1 (a core that enables `websocket`): Swift's and Kotlin's platform
    // defaults register its adapter, a web app has to pass one in `LoadOptions.ports`.
    let found = lines(&without_port(full.clone(), "WebSocket"), &full);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].starts_with("breaking  port WebSocket: added (an opt-in standard port")
            && found[0].contains("LoadOptions.ports"),
        "{found:?}"
    );
}

#[test]
fn the_app_sends_an_event_port_so_adding_one_or_a_method_of_one_is_additive() {
    // An event port is host to core: the bindings give the app `ConnectivityEvents` to call, and
    // there is nothing to implement.
    let base = || {
        with_port(port(
            "Presence",
            PortKind::Event,
            vec![method(
                "Presence",
                "joined",
                vec![param("user", TypeRef::String)],
                TypeRef::Unit,
            )],
        ))
    };
    let found = lines(&schema(), &base());
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0]
            .starts_with("additive  port Presence: added (an event port: the app sends its events"),
        "{found:?}"
    );
    let mut more = base();
    more.ports[0]
        .methods
        .push(method("Presence", "left", vec![], TypeRef::Unit));
    assert_eq!(
        lines(&base(), &more),
        ["additive  method Presence.left: added (fn left()); the app sends it when it has one"]
    );
    // What the app already sends is still a signature it calls.
    assert_eq!(
        lines(&more, &base()),
        ["breaking  method Presence.left: removed (was fn left())"]
    );
    let mut retyped = base();
    retyped.ports[0].methods[0].params[0].ty = TypeRef::Uuid;
    assert_eq!(
        lines(&base(), &retyped),
        ["breaking  method Presence.joined: parameter `user`: type changed from String to Uuid"]
    );
    assert_eq!(
        lines(&base(), &schema()),
        ["breaking  port Presence: removed"]
    );
}

#[test]
fn a_change_to_what_the_app_implements_is_breaking() {
    let base = || {
        with_port(port(
            "Biometrics",
            PortKind::Async,
            vec![method(
                "Biometrics",
                "prompt",
                vec![param("reason", TypeRef::String)],
                TypeRef::Bool,
            )],
        ))
    };
    let mut added = base();
    added.ports[0]
        .methods
        .push(method("Biometrics", "enroll", vec![], TypeRef::Unit));
    let found = lines(&base(), &added);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].starts_with(
            "breaking  method Biometrics.enroll: added (fn enroll()); the app implements it"
        ),
        "{found:?}"
    );
    let mut retyped = base();
    retyped.ports[0].methods[0].returns = TypeRef::Unit;
    assert_eq!(
        lines(&base(), &retyped),
        ["breaking  method Biometrics.prompt: return type changed from bool to ()"]
    );
    let mut kind = base();
    kind.ports[0].kind = PortKind::Sync;
    assert_eq!(
        lines(&base(), &kind),
        ["breaking  port Biometrics: kind changed from async to sync"]
    );
}

#[test]
fn callbacks_are_added_freely_but_changing_one_breaks_its_implementers() {
    let base = || {
        with_port(port(
            "UploadListener",
            PortKind::Callback,
            vec![method(
                "UploadListener",
                "progress",
                vec![param("percent", TypeRef::U8)],
                TypeRef::Unit,
            )],
        ))
    };
    assert_eq!(
        lines(&schema(), &base()),
        ["additive  callback UploadListener: added"]
    );
    assert_eq!(
        lines(&base(), &schema()),
        ["breaking  callback UploadListener: removed"]
    );
    let mut retyped = base();
    retyped.ports[0].methods[0].params[0].ty = TypeRef::F32;
    assert_eq!(
        lines(&base(), &retyped),
        [
            "breaking  method UploadListener.progress: parameter `percent`: type changed from u8 to f32"
        ]
    );
    let mut coalesce = base();
    coalesce.ports[0].methods[0].coalesce = true;
    let found = lines(&base(), &coalesce);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].starts_with("additive  method UploadListener.progress: is now `coalesce`"),
        "{found:?}"
    );
    // `background` moves the calls off or onto the main thread: Swift's protocol stops or starts
    // being `@MainActor` (a Swift 6 implementation no longer conforms as it did), and an
    // implementation that touched the UI from a call now does it from another thread.
    let mut background = base();
    background.ports[0].background = true;
    let found = lines(&base(), &background);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].starts_with(
            "breaking  callback UploadListener: is now `background`: its calls arrive off the main thread"
        ) && found[0].contains("@MainActor"),
        "{found:?}"
    );
    let found = lines(&background, &base());
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].starts_with(
            "breaking  callback UploadListener: is no longer `background`: its calls arrive on the main thread"
        ),
        "{found:?}"
    );
    // A port that became a callback is reported with the callbacks, and as a change of kind.
    let mut moved = base();
    moved.ports[0].kind = PortKind::Event;
    assert_eq!(
        lines(&moved, &base()),
        ["breaking  callback UploadListener: kind changed from event to callback"]
    );
}

// ----- queries and mutations -------------------------------------------------------------------------

fn with_query(q: QueryDef) -> Schema {
    let mut s = schema();
    s.queries.push(q);
    s
}

fn todos_query() -> QueryDef {
    query(
        "todos",
        QueryKind::Query,
        vec![param("page", TypeRef::U32)],
        TypeRef::result(
            TypeRef::vec(TypeRef::named("Todo")),
            TypeRef::named("TodoError"),
        ),
    )
}

#[test]
fn queries_and_mutations_are_added_and_removed_with_their_signature() {
    let q = with_query(todos_query());
    assert_eq!(
        lines(&schema(), &q),
        [
            "additive  query todos: added (async fn todos(page: u32) -> Result<Vec<Todo>, TodoError>)"
        ]
    );
    let m = with_query(query(
        "add_todo",
        QueryKind::Mutation,
        vec![],
        TypeRef::named("Todo"),
    ));
    assert_eq!(
        lines(&m, &schema()),
        ["breaking  mutation add_todo: removed (was async fn add_todo() -> Todo)"]
    );
}

#[test]
fn what_a_query_returns_asks_and_pages_by_is_breaking() {
    let old = with_query(todos_query());
    let change = |edit: &dyn Fn(&mut QueryDef)| {
        let mut q = todos_query();
        edit(&mut q);
        lines(&old, &with_query(q))
    };
    assert_eq!(
        change(&|q| q.params[0].ty = TypeRef::U64),
        ["breaking  query todos: parameter `page`: type changed from u32 to u64"]
    );
    assert_eq!(
        change(&|q| q.returns = TypeRef::named("Todo")),
        ["breaking  query todos: return type changed from Result<Vec<Todo>, TodoError> to Todo"]
    );
    assert_eq!(
        change(&|q| q.kind = QueryKind::Mutation),
        ["breaking  mutation todos: was a query and is now a mutation"]
    );
    assert_eq!(
        change(&|q| {
            q.infinite = Some(InfiniteDef {
                cursor: TypeRef::option(TypeRef::String),
                item_key: "id".into(),
            });
        }),
        ["breaking  query todos: is now an infinite (paged) query"]
    );
}

#[test]
fn a_paged_query_breaks_where_its_handle_or_rows_change_shape() {
    let paged = |cursor: TypeRef, key: &str| {
        let mut q = todos_query();
        q.infinite = Some(InfiniteDef {
            cursor,
            item_key: key.into(),
        });
        with_query(q)
    };
    // The cursor stays in the core (the handle's `fetchNextPage()` takes none), and a key other
    // than `id` only decides how pages merge; a row keyed by `id` is `Identifiable` in Swift, so
    // leaving or taking that key changes the row type an app's `ForEach` reads.
    let old = paged(TypeRef::option(TypeRef::String), "id");
    assert_eq!(
        lines(&old, &paged(TypeRef::option(TypeRef::U64), "uid")),
        [
            "additive  query todos: cursor type changed from Option<String> to Option<u64> (the core pages with it; \
             no generated declaration shows it)",
            "breaking  query todos: item key changed from `id` to `uid` (Swift's row type is `Identifiable` only when \
             the key is `id`)",
        ]
    );
    assert_eq!(
        lines(&paged(TypeRef::U32, "slug"), &paged(TypeRef::U32, "uid")),
        [
            "additive  query todos: item key changed from `slug` to `uid` (how pages merge; the handle is the same)"
        ]
    );
    assert_eq!(
        lines(&old, &with_query(todos_query())),
        ["breaking  query todos: is no longer an infinite (paged) query"]
    );
}

#[test]
fn query_behaviour_that_no_signature_shows_is_additive() {
    let old = with_query(todos_query());
    let mut q = todos_query();
    q.key = "todos/{page}".into();
    q.stale_ms = Some(30_000);
    q.interval_ms = Some(5_000);
    q.persist = true;
    q.idempotent = true;
    q.poll_in_background = true;
    let found = lines(&old, &with_query(q));
    assert_eq!(
        found,
        [
            "additive  query todos: cache key changed from `todos/{id}` to `todos/{page}`",
            "additive  query todos: stale time changed from none to 30000 ms",
            "additive  query todos: poll interval changed from none to 5000 ms",
            "additive  query todos: `persist` changed from false to true",
            "additive  query todos: `idempotent` changed from false to true",
            "additive  query todos: `poll_in_background` changed from false to true",
        ]
    );
}

// ----- order, rendering -------------------------------------------------------------------------------

/// A schema with something in every group, and a second one that changes something in each.
fn busy() -> (Schema, Schema) {
    let mut old = schema();
    old.records
        .push(record("Zeta", vec![field("a", TypeRef::U8, false)]));
    old.records
        .push(record("Alpha", vec![field("a", TypeRef::U8, false)]));
    old.enums.push(enumeration(
        "Mode",
        false,
        vec![variant("On", 0, vec![], false)],
    ));
    old.objects.push(object(
        "Mailbox",
        vec![method("Mailbox", "send", vec![], TypeRef::Unit)],
    ));
    old.objects.push(store(
        "Ledger",
        vec![signal("balance", 0, TypeRef::I64)],
        vec![],
    ));
    old.functions.push(function("zip", vec![], TypeRef::Unit));
    old.functions.push(function("add", vec![], TypeRef::Unit));
    old.ports.push(port("Biometrics", PortKind::Async, vec![]));
    old.ports
        .push(port("UploadListener", PortKind::Callback, vec![]));
    old.queries.push(todos_query());

    let mut new = old.clone();
    new.records[0].fields[0].ty = TypeRef::U16; // Zeta.a
    new.records[1].fields.push(field("b", TypeRef::U8, true)); // Alpha.b
    new.enums[0].variants.push(variant("Off", 1, vec![], false));
    new.objects[0]
        .methods
        .push(method("Mailbox", "count", vec![], TypeRef::U32));
    new.objects[1].store.as_mut().unwrap().signals[0].ty = TypeRef::Decimal;
    new.functions[0].params.push(param("n", TypeRef::U8)); // zip
    new.functions[1].is_async = true; // add
    new.ports[0].kind = PortKind::Sync;
    new.ports[1]
        .methods
        .push(method("UploadListener", "done", vec![], TypeRef::Unit));
    new.queries[0].stale_ms = Some(1);
    (old, new)
}

#[test]
fn the_output_is_grouped_and_ordered_by_name_within_a_group() {
    let (old, new) = busy();
    let nouns: Vec<String> = lines(&old, &new)
        .iter()
        .map(|l| {
            let rest = l.split_once("  ").unwrap().1;
            rest.split(':').next().unwrap().to_owned()
        })
        .collect();
    assert_eq!(
        nouns,
        [
            // types, by name: records and enums together
            "field Alpha.b",
            "case Mode.Off",
            "field Zeta.a",
            // objects and stores, by name
            "signal Ledger.balance",
            "method Mailbox.count",
            // functions
            "function add",
            "function zip",
            // ports, then callbacks
            "port Biometrics",
            "method UploadListener.done",
            // queries
            "query todos",
        ]
    );
}

#[test]
fn the_same_schemas_print_the_same_text_whatever_order_their_lists_were_written_in() {
    let (old, new) = busy();
    let expected = diff(&old, &new).render("old", "new");
    let reversed = |s: &Schema| {
        let mut s = s.clone();
        s.records.reverse();
        s.enums.reverse();
        s.objects.reverse();
        s.functions.reverse();
        s.ports.reverse();
        s.queries.reverse();
        for o in &mut s.objects {
            o.methods.reverse();
            o.constructors.reverse();
        }
        for p in &mut s.ports {
            p.methods.reverse();
        }
        s
    };
    assert_eq!(
        diff(&reversed(&old), &reversed(&new)).render("old", "new"),
        expected
    );
    // And twice in a row.
    assert_eq!(diff(&old, &new).render("old", "new"), expected);
}

#[test]
fn the_report_names_both_sides_and_counts_each_kind() {
    let old = with_function(function(
        "archive",
        vec![param("id", TypeRef::U32)],
        TypeRef::Unit,
    ));
    let mut new = with_function(function(
        "archive",
        vec![param("id", TypeRef::U64)],
        TypeRef::Unit,
    ));
    new.functions
        .push(function("restore", vec![], TypeRef::Unit));
    let d = diff(&old, &new);
    assert_eq!((d.breaking(), d.additive()), (1, 1));
    assert!(d.has_breaking());
    let text = d.render("origin/main:schema.json", "schema.json");
    assert_eq!(
        text,
        format!(
            "Public API: origin/main:schema.json (schema {:#018x}) -> schema.json (schema {:#018x})\n\
             \n\
             breaking  function archive: parameter `id`: type changed from u32 to u64\n\
             additive  function restore: added (fn restore())\n\
             \n\
             1 breaking, 1 additive.\n",
            old.hash(),
            new.hash()
        )
    );
}

#[test]
fn only_additive_changes_do_not_gate_a_ci_run() {
    let old = schema();
    let new = with_function(function("archive", vec![], TypeRef::Unit));
    let d = diff(&old, &new);
    assert_eq!((d.breaking(), d.additive()), (0, 1));
    assert!(!d.has_breaking());
    assert!(d.summary().starts_with("0 breaking, 1 additive."));
}

#[test]
fn the_severity_word_can_be_painted_and_the_text_is_the_same() {
    let old = with_function(function("archive", vec![], TypeRef::Unit));
    let d = diff(&old, &schema());
    let plain = d.render("a", "b");
    let painted = d.render_with("a", "b", |_, label| format!("<{label}>"));
    assert_eq!(painted, plain.replace("breaking  ", "<breaking>  "));
}

#[test]
fn a_rename_that_also_changes_the_type_is_a_removal_and_an_addition() {
    let old = with_function(function("f", vec![param("a", TypeRef::U8)], TypeRef::Unit));
    let new = with_function(function("f", vec![param("b", TypeRef::U16)], TypeRef::Unit));
    assert_eq!(
        lines(&old, &new),
        [
            "breaking  function f: parameter `b: u16` added",
            "breaking  function f: parameter `a: u8` removed",
        ]
    );
}
