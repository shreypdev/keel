//! Shared test fixtures: a representative schema exercising every definition
//! kind. Compiled only for unit tests.

use crate::ids;
use crate::{
    EnumDef, FieldDef, FunctionDef, MethodDef, ObjectDef, ParamDef, PortDef, PortKind, QueryDef,
    QueryKind, RecordDef, Schema, SignalDef, StoreDef, TypeRef, VariantDef,
};

pub(crate) fn field(name: &str, ty: TypeRef) -> FieldDef {
    FieldDef {
        name: name.into(),
        ty,
        default: false,
        docs: String::new(),
    }
}

pub(crate) fn param(name: &str, ty: TypeRef) -> ParamDef {
    ParamDef {
        name: name.into(),
        ty,
    }
}

pub(crate) fn record(name: &str, fields: Vec<FieldDef>) -> RecordDef {
    RecordDef {
        name: name.into(),
        type_id: ids::type_id(name),
        fields,
        transparent: false,
        docs: String::new(),
    }
}

pub(crate) fn method(
    owner: &str,
    name: &str,
    params: Vec<ParamDef>,
    returns: TypeRef,
    is_async: bool,
) -> MethodDef {
    MethodDef {
        name: name.into(),
        method_id: ids::method_id(owner, name),
        params,
        returns,
        is_async,
        takes_ctx: false,
        coalesce: false,
        docs: String::new(),
    }
}

pub(crate) fn unit_variant(name: &str, index: u16) -> VariantDef {
    VariantDef {
        name: name.into(),
        index,
        fields: Vec::new(),
        tuple: false,
        message: None,
        docs: String::new(),
    }
}

pub(crate) fn object(name: &str, ctors: Vec<MethodDef>, methods: Vec<MethodDef>) -> ObjectDef {
    ObjectDef {
        name: name.into(),
        type_id: ids::type_id(name),
        constructors: ctors,
        methods,
        store: None,
        docs: String::new(),
    }
}

fn error_enum(name: &str, variants: Vec<VariantDef>) -> EnumDef {
    EnumDef {
        name: name.into(),
        type_id: ids::type_id(name),
        is_error: true,
        variants,
        docs: String::new(),
    }
}

/// A schema with records, unit / data / error enums, an object with sync,
/// async and stream methods, a store with a keyed `Vec`, a `Computed` and a
/// `Lazy` signal, a port of each kind, a query and a mutation. Every top-level
/// list has at least two entries and is deliberately not in name order.
pub(crate) fn representative_schema() -> Schema {
    let mut s = Schema::new("playground-core");

    let mut todo = record(
        "Todo",
        vec![
            field("id", TypeRef::Uuid),
            field("title", TypeRef::String),
            FieldDef {
                default: true,
                ..field("done", TypeRef::Bool)
            },
            field("tags", TypeRef::vec(TypeRef::String)),
            field("due", TypeRef::option(TypeRef::Timestamp)),
            field("meta", TypeRef::map(TypeRef::String, TypeRef::I32)),
            field("priority", TypeRef::named("Priority")),
        ],
    );
    todo.docs = "A todo item.".into();
    todo.fields[1].docs = "Shown in the list.".into();
    s.records.push(todo);
    s.records.push(record(
        "Page",
        vec![
            field("items", TypeRef::vec(TypeRef::named("Todo"))),
            field("next", TypeRef::option(TypeRef::String)),
            field("total", TypeRef::U64),
        ],
    ));
    s.records.push(record(
        "HttpRequest",
        vec![
            field("method", TypeRef::String),
            field("url", TypeRef::String),
            field("headers", TypeRef::map(TypeRef::String, TypeRef::String)),
            field("body", TypeRef::option(TypeRef::Bytes)),
        ],
    ));
    s.records.push(record(
        "HttpResponse",
        vec![
            field("status", TypeRef::U16),
            field("headers", TypeRef::map(TypeRef::String, TypeRef::String)),
            field("body", TypeRef::Bytes),
            field("elapsed", TypeRef::Duration),
        ],
    ));

    // Unit enum.
    s.enums.push(EnumDef {
        name: "Priority".into(),
        type_id: ids::type_id("Priority"),
        is_error: false,
        variants: vec![
            unit_variant("Low", 0),
            unit_variant("Normal", 1),
            unit_variant("High", 2),
        ],
        docs: String::new(),
    });
    // Data enum with named and tuple variants.
    s.enums.push(EnumDef {
        name: "Shape".into(),
        type_id: ids::type_id("Shape"),
        is_error: false,
        variants: vec![
            VariantDef {
                fields: vec![field("r", TypeRef::F64)],
                ..unit_variant("Circle", 0)
            },
            VariantDef {
                fields: vec![field("0", TypeRef::F64), field("1", TypeRef::F64)],
                tuple: true,
                ..unit_variant("Rect", 1)
            },
            unit_variant("Empty", 2),
        ],
        docs: String::new(),
    });
    // Error enums.
    s.enums.push(error_enum(
        "TodoError",
        vec![
            VariantDef {
                fields: vec![field("0", TypeRef::String)],
                tuple: true,
                message: Some("todo {0} not found".into()),
                ..unit_variant("NotFound", 0)
            },
            VariantDef {
                fields: vec![field("reason", TypeRef::String)],
                message: Some("storage failure ({reason})".into()),
                ..unit_variant("Storage", 1)
            },
            VariantDef {
                message: Some("offline".into()),
                ..unit_variant("Offline", 2)
            },
        ],
    ));
    s.enums.push(error_enum(
        "HttpError",
        vec![
            VariantDef {
                message: Some("timed out".into()),
                ..unit_variant("Timeout", 0)
            },
            VariantDef {
                fields: vec![field("0", TypeRef::U16)],
                tuple: true,
                message: Some("status {0}".into()),
                ..unit_variant("Status", 1)
            },
        ],
    ));
    s.enums.push(EnumDef {
        name: "NetKind".into(),
        type_id: ids::type_id("NetKind"),
        is_error: false,
        variants: vec![
            unit_variant("Wifi", 0),
            unit_variant("Cellular", 1),
            unit_variant("None", 2),
        ],
        docs: String::new(),
    });

    // Plain object: sync, async, stream and result-of-stream methods.
    let mut calc = object(
        "Calculator",
        vec![method(
            "Calculator",
            "new",
            vec![],
            TypeRef::named("Calculator"),
            false,
        )],
        vec![
            method(
                "Calculator",
                "add",
                vec![param("a", TypeRef::I32), param("b", TypeRef::I32)],
                TypeRef::I32,
                false,
            ),
            method(
                "Calculator",
                "fetch",
                vec![param("id", TypeRef::Uuid)],
                TypeRef::result(TypeRef::named("Todo"), TypeRef::named("TodoError")),
                true,
            ),
            method(
                "Calculator",
                "ticks",
                vec![param("n", TypeRef::U32)],
                TypeRef::stream(TypeRef::U32),
                true,
            ),
            method(
                "Calculator",
                "watch",
                vec![param("priority", TypeRef::named("Priority"))],
                TypeRef::result(
                    TypeRef::stream(TypeRef::named("Todo")),
                    TypeRef::named("TodoError"),
                ),
                true,
            ),
            method("Calculator", "reset", vec![], TypeRef::Unit, false),
        ],
    );
    calc.docs = "Adds numbers.".into();
    s.objects.push(calc);

    // Store: keyed Vec, plain, Computed, Lazy and Option signals.
    let mut store = object(
        "TodoStore",
        vec![
            MethodDef {
                takes_ctx: true,
                ..method(
                    "TodoStore",
                    "new",
                    vec![],
                    TypeRef::named("TodoStore"),
                    false,
                )
            },
            method(
                "TodoStore",
                "open",
                vec![param("path", TypeRef::String)],
                TypeRef::result(TypeRef::named("TodoStore"), TypeRef::named("TodoError")),
                true,
            ),
        ],
        vec![
            method(
                "TodoStore",
                "add",
                vec![param("title", TypeRef::String)],
                TypeRef::result(TypeRef::named("Todo"), TypeRef::named("TodoError")),
                false,
            ),
            method(
                "TodoStore",
                "toggle",
                vec![param("id", TypeRef::Uuid)],
                TypeRef::Unit,
                false,
            ),
        ],
    );
    store.store = Some(StoreDef {
        signals: vec![
            SignalDef {
                name: "todos".into(),
                signal_id: 0,
                ty: TypeRef::vec(TypeRef::named("Todo")),
                computed: false,
                key: Some("id".into()),
                no_coalesce: false,
                default: false,
            },
            SignalDef {
                name: "filter".into(),
                signal_id: 1,
                ty: TypeRef::named("Priority"),
                computed: false,
                key: None,
                no_coalesce: false,
                default: false,
            },
            SignalDef {
                name: "remaining".into(),
                signal_id: 2,
                ty: TypeRef::U32,
                computed: true,
                key: None,
                no_coalesce: false,
                default: false,
            },
            SignalDef {
                name: "archive".into(),
                signal_id: 3,
                ty: TypeRef::lazy(TypeRef::named("Todo")),
                computed: false,
                key: None,
                no_coalesce: false,
                default: false,
            },
            SignalDef {
                name: "selected".into(),
                signal_id: 4,
                ty: TypeRef::option(TypeRef::named("Todo")),
                computed: false,
                key: None,
                no_coalesce: false,
                default: false,
            },
        ],
    });
    s.objects.push(store);

    // Free functions.
    s.functions.push(FunctionDef {
        name: "greet".into(),
        method_id: ids::function_id("greet"),
        params: vec![param("name", TypeRef::String)],
        returns: TypeRef::String,
        is_async: false,
        takes_ctx: false,
        docs: String::new(),
    });
    s.functions.push(FunctionDef {
        name: "ping".into(),
        method_id: ids::function_id("ping"),
        params: vec![],
        returns: TypeRef::result(TypeRef::Unit, TypeRef::named("TodoError")),
        is_async: true,
        takes_ctx: true,
        docs: String::new(),
    });

    // One port of each kind.
    s.ports.push(PortDef {
        name: "Clock".into(),
        port_id: ids::port_id("Clock"),
        kind: PortKind::Sync,
        background: false,
        methods: vec![
            MethodDef {
                method_id: ids::port_method_id("Clock", "now_ms"),
                ..method("Clock", "now_ms", vec![], TypeRef::I64, false)
            },
            MethodDef {
                method_id: ids::port_method_id("Clock", "monotonic_ns"),
                ..method("Clock", "monotonic_ns", vec![], TypeRef::U64, false)
            },
        ],
        docs: String::new(),
    });
    s.ports.push(PortDef {
        name: "Http".into(),
        port_id: ids::port_id("Http"),
        kind: PortKind::Async,
        background: false,
        methods: vec![method(
            "Http",
            "request",
            vec![param("req", TypeRef::named("HttpRequest"))],
            TypeRef::result(TypeRef::named("HttpResponse"), TypeRef::named("HttpError")),
            true,
        )],
        docs: String::new(),
    });
    s.ports.push(PortDef {
        name: "Connectivity".into(),
        port_id: ids::port_id("Connectivity"),
        kind: PortKind::Event,
        background: false,
        methods: vec![method(
            "Connectivity",
            "changed",
            vec![
                param("online", TypeRef::Bool),
                param("kind", TypeRef::named("NetKind")),
            ],
            TypeRef::Unit,
            false,
        )],
        docs: String::new(),
    });

    // A query and a mutation.
    s.queries.push(QueryDef {
        name: "todos".into(),
        query_id: ids::query_id("todos"),
        kind: QueryKind::Query,
        key: "todos:{page}".into(),
        params: vec![param("page", TypeRef::U32)],
        returns: TypeRef::result(TypeRef::named("Page"), TypeRef::named("TodoError")),
        stale_ms: Some(30_000),
        persist: true,
        idempotent: true,
        interval_ms: None,
        poll_in_background: false,
        infinite: None,
    });
    s.queries.push(QueryDef {
        name: "add_todo".into(),
        query_id: ids::mutation_id("add_todo"),
        kind: QueryKind::Mutation,
        key: "todos".into(),
        params: vec![param("title", TypeRef::String)],
        returns: TypeRef::result(TypeRef::named("Todo"), TypeRef::named("TodoError")),
        stale_ms: None,
        persist: false,
        idempotent: false,
        interval_ms: None,
        poll_in_background: false,
        infinite: None,
    });

    s
}

/// The same schema with a distinctive doc string on every documentable item.
pub(crate) fn with_docs(mut s: Schema) -> Schema {
    fn doc(name: &str) -> String {
        format!("docs: todo item {name}")
    }
    for r in &mut s.records {
        r.docs = doc(&r.name);
        for f in &mut r.fields {
            f.docs = doc(&f.name);
        }
    }
    for e in &mut s.enums {
        e.docs = doc(&e.name);
        for v in &mut e.variants {
            v.docs = doc(&v.name);
            for f in &mut v.fields {
                f.docs = doc(&f.name);
            }
        }
    }
    for o in &mut s.objects {
        o.docs = doc(&o.name);
        for m in o.constructors.iter_mut().chain(o.methods.iter_mut()) {
            m.docs = doc(&m.name);
        }
    }
    for f in &mut s.functions {
        f.docs = doc(&f.name);
    }
    for p in &mut s.ports {
        p.docs = doc(&p.name);
        for m in &mut p.methods {
            m.docs = doc(&m.name);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixture_is_itself_valid() {
        representative_schema().validate().unwrap();
        with_docs(representative_schema()).validate().unwrap();
    }

    #[test]
    fn the_fixture_covers_every_definition_kind() {
        let s = representative_schema();
        assert!(s.records.len() >= 2 && s.enums.len() >= 2);
        assert!(s.objects.len() >= 2 && s.functions.len() >= 2);
        assert!(s.ports.len() >= 3 && s.queries.len() >= 2);
        assert!(s.enums.iter().any(|e| e.is_error));
        assert!(
            s.enums
                .iter()
                .any(|e| !e.is_error && e.variants.iter().all(|v| v.fields.is_empty()))
        );
        assert!(
            s.enums
                .iter()
                .any(|e| e.variants.iter().any(|v| !v.fields.is_empty()))
        );
        for kind in [PortKind::Sync, PortKind::Async, PortKind::Event] {
            assert!(s.ports.iter().any(|p| p.kind == kind));
        }
        assert!(s.queries.iter().any(|q| q.kind == QueryKind::Query));
        assert!(s.queries.iter().any(|q| q.kind == QueryKind::Mutation));
        let store = s.objects.iter().find_map(|o| o.store.as_ref()).unwrap();
        assert!(store.signals.iter().any(|g| g.key.is_some()));
        assert!(store.signals.iter().any(|g| g.computed));
    }
}
