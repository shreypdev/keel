//! End-to-end registration through `inventory`.
//!
//! This is its own test binary so the process-wide registry holds exactly the
//! items submitted below.

use std::any::Any;

use undra_meta::{
    DispatchCall, DispatchOutcome, EnumMeta, FieldMeta, FunctionMeta, MethodMeta, ObjectMeta,
    ParamMeta, PortKind, PortMeta, QueryKind, QueryMeta, RecordMeta, Registration, SignalMeta,
    StoreMeta, TypeKind, TypeRefMeta, VariantMeta, collect_schema, ids, registrations,
};

fn dispatch_calculator(rt: &dyn Any, call: DispatchCall<'_>) -> DispatchOutcome {
    // A stand-in for a generated dispatcher: downcast the runtime, answer.
    let base = rt.downcast_ref::<u32>().copied().unwrap_or(0);
    DispatchOutcome::new(base + call.method_id)
}

fn dispatch_greet(_rt: &dyn Any, call: DispatchCall<'_>) -> DispatchOutcome {
    DispatchOutcome::new(call.args.to_vec())
}

static TODO: RecordMeta = RecordMeta {
    name: "Todo",
    type_id: ids::type_id("Todo"),
    fields: &[
        FieldMeta {
            name: "id",
            ty: TypeRefMeta::Uuid,
            default: false,
            docs: "",
        },
        FieldMeta {
            name: "tags",
            ty: TypeRefMeta::Map(&TypeRefMeta::String, &TypeRefMeta::Vec(&TypeRefMeta::U8)),
            default: true,
            docs: "Tags by name.",
        },
    ],
    transparent: false,
    docs: "A todo item.",
};

static TODO_ERROR: EnumMeta = EnumMeta {
    name: "TodoError",
    type_id: ids::type_id("TodoError"),
    is_error: true,
    variants: &[VariantMeta {
        name: "NotFound",
        index: 0,
        fields: &[FieldMeta {
            name: "0",
            ty: TypeRefMeta::Uuid,
            default: false,
            docs: "",
        }],
        tuple: true,
        message: Some("todo {0} not found"),
        docs: "",
    }],
    docs: "",
};

static CALCULATOR: ObjectMeta = ObjectMeta {
    name: "Calculator",
    type_id: ids::type_id("Calculator"),
    constructors: &[MethodMeta {
        name: "new",
        method_id: ids::method_id("Calculator", "new"),
        params: &[],
        returns: TypeRefMeta::Named("Calculator"),
        is_async: false,
        takes_ctx: false,
        coalesce: false,
        docs: "",
        generic: None,
    }],
    methods: &[MethodMeta {
        name: "add",
        method_id: ids::method_id("Calculator", "add"),
        params: &[
            ParamMeta {
                name: "a",
                ty: TypeRefMeta::I32,
            },
            ParamMeta {
                name: "b",
                ty: TypeRefMeta::I32,
            },
        ],
        returns: TypeRefMeta::I32,
        is_async: false,
        takes_ctx: false,
        coalesce: false,
        docs: "",
        generic: None,
    }],
    store: None,
    docs: "",
    dispatch: dispatch_calculator,
};

static COUNTER: ObjectMeta = ObjectMeta {
    name: "Counter",
    type_id: ids::type_id("Counter"),
    constructors: &[MethodMeta {
        name: "new",
        method_id: ids::method_id("Counter", "new"),
        params: &[],
        returns: TypeRefMeta::Named("Counter"),
        is_async: false,
        takes_ctx: true,
        coalesce: false,
        docs: "",
        generic: None,
    }],
    methods: &[],
    store: Some(StoreMeta {
        signals: &[SignalMeta {
            name: "todos",
            signal_id: 0,
            ty: TypeRefMeta::Vec(&TypeRefMeta::Named("Todo")),
            computed: false,
            key: Some("id"),
            no_coalesce: false,
            default: false,
        }],
    }),
    docs: "",
    dispatch: dispatch_calculator,
};

static GREET: FunctionMeta = FunctionMeta {
    name: "greet",
    method_id: ids::function_id("greet"),
    params: &[ParamMeta {
        name: "name",
        ty: TypeRefMeta::String,
    }],
    returns: TypeRefMeta::String,
    is_async: false,
    takes_ctx: false,
    docs: "Greets.",
    dispatch: dispatch_greet,
    generic: None,
};

static CLOCK: PortMeta = PortMeta {
    name: "Clock",
    port_id: ids::port_id("Clock"),
    kind: PortKind::Sync,
    background: false,
    methods: &[MethodMeta {
        name: "now_ms",
        method_id: ids::port_method_id("Clock", "now_ms"),
        params: &[],
        returns: TypeRefMeta::I64,
        is_async: false,
        takes_ctx: false,
        coalesce: false,
        docs: "",
        generic: None,
    }],
    docs: "",
};

static TODOS: QueryMeta = QueryMeta {
    name: "todos",
    query_id: ids::query_id("todos"),
    kind: QueryKind::Query,
    key: "todos",
    params: &[],
    returns: TypeRefMeta::Result(
        &TypeRefMeta::Vec(&TypeRefMeta::Named("Todo")),
        &TypeRefMeta::Named("TodoError"),
    ),
    stale_ms: Some(30_000),
    persist: true,
    idempotent: true,
    interval_ms: None,
    poll_in_background: false,
    infinite: None,
};

static ADD_TODO: QueryMeta = QueryMeta {
    name: "add_todo",
    query_id: ids::mutation_id("add_todo"),
    kind: QueryKind::Mutation,
    key: "todos",
    params: &[ParamMeta {
        name: "title",
        ty: TypeRefMeta::String,
    }],
    returns: TypeRefMeta::Result(
        &TypeRefMeta::Named("Todo"),
        &TypeRefMeta::Named("TodoError"),
    ),
    stale_ms: None,
    persist: false,
    idempotent: false,
    interval_ms: None,
    poll_in_background: false,
    infinite: None,
};

undra_meta::inventory::submit! { Registration::Record(&TODO) }
undra_meta::inventory::submit! { Registration::Enum(&TODO_ERROR) }
undra_meta::inventory::submit! { Registration::Object(&CALCULATOR) }
undra_meta::inventory::submit! { Registration::Object(&COUNTER) }
undra_meta::inventory::submit! { Registration::Function(&GREET) }
undra_meta::inventory::submit! { Registration::Port(&CLOCK) }
undra_meta::inventory::submit! { Registration::Query(&TODOS) }
undra_meta::inventory::submit! { Registration::Query(&ADD_TODO) }

#[test]
fn a_submitted_record_is_visible_in_collect_schema() {
    let schema = collect_schema("registration-test");
    assert_eq!(schema.crate_name, "registration-test");
    assert_eq!(schema.undra_version, undra_meta::UNDRA_VERSION);
    let todo = schema
        .records
        .iter()
        .find(|r| r.name == "Todo")
        .expect("Todo registered via inventory::submit!");
    assert_eq!(todo.type_id, ids::type_id("Todo"));
    assert_eq!(todo.docs, "A todo item.");
    assert_eq!(todo.fields.len(), 2);
    assert!(todo.fields[1].default);
    assert_eq!(
        todo.fields[1].ty,
        undra_meta::TypeRef::map(
            undra_meta::TypeRef::String,
            undra_meta::TypeRef::vec(undra_meta::TypeRef::U8)
        )
    );
}

#[test]
fn every_registration_kind_is_collected_exactly_once() {
    let schema = collect_schema("registration-test");
    assert_eq!(schema.records.len(), 1);
    assert_eq!(schema.enums.len(), 1);
    assert!(schema.enums[0].is_error);
    assert_eq!(schema.objects.len(), 2);
    assert_eq!(schema.functions.len(), 1);
    assert_eq!(schema.ports.len(), 1);
    assert_eq!(schema.queries.len(), 2);
    assert_eq!(registrations().count(), 8);
}

#[test]
fn collected_schema_is_sorted_and_deterministic() {
    let a = collect_schema("registration-test");
    let b = collect_schema("registration-test");
    assert_eq!(a, b);
    let objects: Vec<_> = a.objects.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(objects, ["Calculator", "Counter"]);
    let queries: Vec<_> = a.queries.iter().map(|q| q.name.as_str()).collect();
    assert_eq!(queries, ["add_todo", "todos"]);
}

#[test]
fn collected_schema_validates_and_hashes() {
    let schema = collect_schema("registration-test");
    schema.validate().expect("registered schema is valid");
    assert_eq!(schema.hash(), collect_schema("registration-test").hash());
    let counter = schema.objects.iter().find(|o| o.name == "Counter").unwrap();
    assert_eq!(
        counter.store.as_ref().unwrap().signals[0].key.as_deref(),
        Some("id")
    );
}

#[test]
fn the_collected_hash_is_independent_of_docs() {
    // Docs are on the registered Todo record; the canonical form drops them.
    let schema = collect_schema("registration-test");
    let mut undocumented = schema.clone();
    undocumented.records[0].docs.clear();
    undocumented.records[0].fields[1].docs.clear();
    assert_ne!(schema, undocumented);
    assert_eq!(schema.hash(), undocumented.hash());
}

#[test]
fn dispatchers_are_reachable_through_registrations() {
    let call = DispatchCall {
        method_id: 7,
        call_id: 1,
        handle: 0,
        args: b"hi",
    };
    let mut seen_objects = 0;
    let mut seen_functions = 0;
    for registration in registrations() {
        match registration {
            Registration::Object(meta) if meta.name == "Calculator" => {
                seen_objects += 1;
                let out = (meta.dispatch)(&100_u32, call);
                assert_eq!(out.downcast::<u32>().unwrap(), 107);
            }
            Registration::Function(meta) => {
                seen_functions += 1;
                let out = (meta.dispatch)(&(), call);
                assert_eq!(out.downcast::<Vec<u8>>().unwrap(), b"hi");
            }
            _ => {}
        }
    }
    assert_eq!((seen_objects, seen_functions), (1, 1));
}

#[test]
fn a_schema_built_from_defs_matches_the_collected_one() {
    use undra_meta::{
        EnumDef, FieldDef, FunctionDef, MethodDef, ObjectDef, ParamDef, PortDef, QueryDef,
        RecordDef, Schema, SignalDef, StoreDef, TypeRef, VariantDef,
    };

    let collected = collect_schema("registration-test");
    let mut by_hand = Schema::new("registration-test");
    by_hand.records.push(RecordDef {
        name: "Todo".into(),
        type_id: ids::type_id("Todo"),
        fields: vec![
            FieldDef {
                name: "id".into(),
                ty: TypeRef::Uuid,
                default: false,
                docs: String::new(),
            },
            FieldDef {
                name: "tags".into(),
                ty: TypeRef::map(TypeRef::String, TypeRef::vec(TypeRef::U8)),
                default: true,
                docs: "Tags by name.".into(),
            },
        ],
        transparent: false,
        docs: "A todo item.".into(),
    });
    by_hand.enums.push(EnumDef {
        name: "TodoError".into(),
        type_id: ids::type_id("TodoError"),
        is_error: true,
        variants: vec![VariantDef {
            name: "NotFound".into(),
            index: 0,
            fields: vec![FieldDef {
                name: "0".into(),
                ty: TypeRef::Uuid,
                default: false,
                docs: String::new(),
            }],
            tuple: true,
            message: Some("todo {0} not found".into()),
            docs: String::new(),
        }],
        docs: String::new(),
    });
    let ctor = |ty: &str, takes_ctx: bool| MethodDef {
        coalesce: false,
        name: "new".into(),
        method_id: ids::method_id(ty, "new"),
        params: vec![],
        returns: TypeRef::named(ty),
        is_async: false,
        takes_ctx,
        docs: String::new(),
        generic: None,
    };
    by_hand.objects.push(ObjectDef {
        name: "Calculator".into(),
        type_id: ids::type_id("Calculator"),
        constructors: vec![ctor("Calculator", false)],
        methods: vec![MethodDef {
            name: "add".into(),
            method_id: ids::method_id("Calculator", "add"),
            params: vec![
                ParamDef {
                    name: "a".into(),
                    ty: TypeRef::I32,
                },
                ParamDef {
                    name: "b".into(),
                    ty: TypeRef::I32,
                },
            ],
            returns: TypeRef::I32,
            is_async: false,
            takes_ctx: false,
            coalesce: false,
            docs: String::new(),
            generic: None,
        }],
        store: None,
        docs: String::new(),
    });
    by_hand.objects.push(ObjectDef {
        name: "Counter".into(),
        type_id: ids::type_id("Counter"),
        constructors: vec![ctor("Counter", true)],
        methods: vec![],
        store: Some(StoreDef {
            signals: vec![SignalDef {
                name: "todos".into(),
                signal_id: 0,
                ty: TypeRef::vec(TypeRef::named("Todo")),
                computed: false,
                key: Some("id".into()),
                no_coalesce: false,
                default: false,
            }],
        }),
        docs: String::new(),
    });
    by_hand.functions.push(FunctionDef {
        name: "greet".into(),
        method_id: ids::function_id("greet"),
        params: vec![ParamDef {
            name: "name".into(),
            ty: TypeRef::String,
        }],
        returns: TypeRef::String,
        is_async: false,
        takes_ctx: false,
        docs: "Greets.".into(),
        generic: None,
    });
    by_hand.ports.push(PortDef {
        name: "Clock".into(),
        port_id: ids::port_id("Clock"),
        kind: PortKind::Sync,
        background: false,
        methods: vec![MethodDef {
            name: "now_ms".into(),
            method_id: ids::port_method_id("Clock", "now_ms"),
            params: vec![],
            returns: TypeRef::I64,
            is_async: false,
            takes_ctx: false,
            coalesce: false,
            docs: String::new(),
            generic: None,
        }],
        docs: String::new(),
    });
    by_hand.queries.push(QueryDef {
        name: "add_todo".into(),
        query_id: ids::mutation_id("add_todo"),
        kind: QueryKind::Mutation,
        key: "todos".into(),
        params: vec![ParamDef {
            name: "title".into(),
            ty: TypeRef::String,
        }],
        returns: TypeRef::result(TypeRef::named("Todo"), TypeRef::named("TodoError")),
        stale_ms: None,
        persist: false,
        idempotent: false,
        interval_ms: None,
        poll_in_background: false,
        infinite: None,
    });
    by_hand.queries.push(QueryDef {
        name: "todos".into(),
        query_id: ids::query_id("todos"),
        kind: QueryKind::Query,
        key: "todos".into(),
        params: vec![],
        returns: TypeRef::result(
            TypeRef::vec(TypeRef::named("Todo")),
            TypeRef::named("TodoError"),
        ),
        stale_ms: Some(30_000),
        persist: true,
        idempotent: true,
        interval_ms: None,
        poll_in_background: false,
        infinite: None,
    });

    assert_eq!(collected, by_hand);
    assert_eq!(collected.canonical_json(), by_hand.canonical_json());
    assert_eq!(collected.hash(), by_hand.hash());
    // TypeKind is re-exported for consumers of SchemaError.
    let _ = TypeKind::Record;
}
