//! The Undra standard library: the ten standard ports of SPEC section 8 and the eight records,
//! enums and errors they exchange.
//!
//! Every app core links `undra-ports`, so its schema truthfully contains them (R1, and the schema
//! hash covers them). The three platform runtimes implement exactly these ports and ship exactly
//! these types, so generating them again into every app would put a second `FsError` in the
//! app's namespace and break the native-review bar (R3). The generators therefore leave the
//! standard surface out of their output and let references to the standard types resolve to the
//! runtime's own (ADR-024); this module is the table that says what "standard" means.
//!
//! An item is standard only when it is **exactly** the standard one: the same name, the same
//! id and the same shape (field names and types, variant names, indices and payloads, method
//! ids and signatures; documentation is ignored). Ids are derived from names (SPEC 1.1), so the
//! id alone cannot tell a user's own `HttpRequest` from the standard one; the shape can. A type
//! that only shares a name keeps being generated as the user's own type.
//!
//! The ids below are pinned as hex so that a change is a visible diff. The tests recompute every
//! one with FNV-1a and compare the whole table with the registrations of `undra-ports`.

use std::collections::BTreeSet;

use undra_meta::{EnumDef, MethodDef, PortDef, PortKind, RecordDef, Schema, TypeRef};

use crate::model::Lang;

/// What kind of item a [`StandardType`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandardKind {
    /// A record.
    Record,
    /// An enum without the error flag.
    Enum,
    /// A `#[undra::error]` enum.
    Error,
}

/// One standard record, enum or error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StandardType {
    /// The type's name, as declared in `undra-ports`.
    pub name: &'static str,
    /// `fnv1a32(name)` (SPEC 1.1), pinned.
    pub type_id: u32,
    /// Record, enum or error.
    pub kind: StandardKind,
    /// The fields of a record (`name: Type`, in order) or the variants of an enum or error
    /// (`Name`, `Name(T, U)` or `Name { a: T }`, each followed by ` = index`).
    pub shape: &'static str,
}

/// One method of a standard port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StandardMethod {
    /// `fnv1a32("<Port>.<method>")`, pinned.
    pub id: u32,
    /// The declaration: an optional `async `, the method name, the parameters and an optional
    /// `-> Return` (nothing for `()`).
    pub decl: &'static str,
}

impl StandardMethod {
    /// The method's name, the part of [`decl`](Self::decl) before the parameter list.
    #[must_use]
    pub fn name(&self) -> &'static str {
        let decl = self.decl.strip_prefix("async ").unwrap_or(self.decl);
        decl.split('(').next().unwrap_or(decl)
    }

    /// Whether the method is `async`.
    #[must_use]
    pub fn is_async(&self) -> bool {
        self.decl.starts_with("async ")
    }
}

/// One standard port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StandardPort {
    /// The port trait's name.
    pub name: &'static str,
    /// `fnv1a32("port.<name>")`, pinned.
    pub port_id: u32,
    /// Sync, async or event.
    pub kind: PortKind,
    /// The methods, in declaration order.
    pub methods: &'static [StandardMethod],
}

const fn m(id: u32, decl: &'static str) -> StandardMethod {
    StandardMethod { id, decl }
}

/// The eight standard types (SPEC section 8).
pub const TYPES: &[StandardType] = &[
    StandardType {
        name: "HttpMethod",
        type_id: 0x77bf_0650,
        kind: StandardKind::Enum,
        shape: "Get = 0, Post = 1, Put = 2, Delete = 3, Patch = 4, Head = 5, Options = 6",
    },
    StandardType {
        name: "Header",
        type_id: 0x114f_9980,
        kind: StandardKind::Record,
        shape: "name: String, value: String",
    },
    StandardType {
        name: "HttpRequest",
        type_id: 0xbbbc_6e52,
        kind: StandardKind::Record,
        shape: "method: HttpMethod, url: String, headers: Vec<Header>, body: Option<Bytes>, timeout_ms: Option<u32>",
    },
    StandardType {
        name: "HttpResponse",
        type_id: 0xcc45_59fe,
        kind: StandardKind::Record,
        shape: "status: u16, headers: Vec<Header>, body: Bytes",
    },
    StandardType {
        name: "HttpError",
        type_id: 0xee63_c1f1,
        kind: StandardKind::Error,
        shape: "Network(String) = 0, Timeout = 1, Cancelled = 2, InvalidUrl(String) = 3",
    },
    StandardType {
        name: "FsError",
        type_id: 0xd15e_c208,
        kind: StandardKind::Error,
        shape: "NotFound = 0, Denied = 1, Io(String) = 2",
    },
    StandardType {
        name: "NetKind",
        type_id: 0x0371_71aa,
        kind: StandardKind::Enum,
        shape: "Wifi = 0, Cellular = 1, Wired = 2, Unknown = 3, None = 4",
    },
    StandardType {
        name: "AppState",
        type_id: 0xcfb6_6091,
        kind: StandardKind::Enum,
        shape: "Active = 0, Inactive = 1, Background = 2",
    },
];

const KV_METHODS: &[StandardMethod] = &[
    m(0xf050_bb1a, "async get(key: String) -> Option<Bytes>"),
    m(0x6242_7856, "async set(key: String, value: Bytes)"),
    m(0x60a3_86b9, "async delete(key: String)"),
    m(0x32f1_d03a, "async list(prefix: String) -> Vec<String>"),
];

const SECURE_STORE_METHODS: &[StandardMethod] = &[
    m(0x5703_6f6f, "async get(key: String) -> Option<Bytes>"),
    m(0xe91e_017b, "async set(key: String, value: Bytes)"),
    m(0xd57d_b4e2, "async delete(key: String)"),
    m(0xf5ba_b8c9, "async list(prefix: String) -> Vec<String>"),
];

const FS_METHODS: &[StandardMethod] = &[
    m(
        0x01fd_be44,
        "async read(path: String) -> Result<Bytes, FsError>",
    ),
    m(
        0x6b70_d47f,
        "async write(path: String, data: Bytes) -> Result<(), FsError>",
    ),
    m(
        0xa90a_826b,
        "async delete(path: String) -> Result<(), FsError>",
    ),
    m(
        0x4fba_8678,
        "async list(dir: String) -> Result<Vec<String>, FsError>",
    ),
];

/// The ten standard ports (SPEC section 8).
pub const PORTS: &[StandardPort] = &[
    StandardPort {
        name: "Clock",
        port_id: 0xcd99_c48e,
        kind: PortKind::Sync,
        methods: &[
            m(0xccc9_4d90, "now_ms() -> i64"),
            m(0x2cb2_b4bf, "monotonic_ns() -> u64"),
        ],
    },
    StandardPort {
        name: "Rng",
        port_id: 0x2513_5bf5,
        kind: PortKind::Sync,
        methods: &[m(0x2832_b8ed, "fill(len: u32) -> Bytes")],
    },
    StandardPort {
        name: "Log",
        port_id: 0x575f_f24a,
        kind: PortKind::Sync,
        methods: &[m(
            0xd49d_5649,
            "log(level: u8, target: String, message: String)",
        )],
    },
    StandardPort {
        name: "Http",
        port_id: 0x1ebe_b908,
        kind: PortKind::Async,
        methods: &[m(
            0x6b14_df26,
            "async request(req: HttpRequest) -> Result<HttpResponse, HttpError>",
        )],
    },
    StandardPort {
        name: "Kv",
        port_id: 0x5389_110d,
        kind: PortKind::Async,
        methods: KV_METHODS,
    },
    StandardPort {
        name: "SecureStore",
        port_id: 0xc01f_5bea,
        kind: PortKind::Async,
        methods: SECURE_STORE_METHODS,
    },
    StandardPort {
        name: "Fs",
        port_id: 0x4ea3_4cab,
        kind: PortKind::Async,
        methods: FS_METHODS,
    },
    StandardPort {
        name: "Timer",
        port_id: 0x00c2_cdd9,
        kind: PortKind::Sync,
        methods: &[m(0x923a_766c, "set(timer_id: u32, delay_ms: u64)")],
    },
    StandardPort {
        name: "Connectivity",
        port_id: 0x1fef_f6ff,
        kind: PortKind::Event,
        methods: &[m(0xb4f2_a010, "changed(online: bool, kind: NetKind)")],
    },
    StandardPort {
        name: "Lifecycle",
        port_id: 0x81c0_afd4,
        kind: PortKind::Event,
        methods: &[m(0x0bc8_2569, "changed(state: AppState)")],
    },
];

/// Whether `name` is the name of a standard type.
#[must_use]
pub fn is_standard_type_name(name: &str) -> bool {
    TYPES.iter().any(|t| t.name == name)
}

/// Whether `name` is the name of a standard port.
#[must_use]
pub fn is_standard_port_name(name: &str) -> bool {
    PORTS.iter().any(|p| p.name == name)
}

// ----- shapes --------------------------------------------------------------------

/// A type as Rust spells it: `Vec<Header>`, `Option<Bytes>`, `()`.
fn type_text(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Bool => "bool".to_owned(),
        TypeRef::I8 => "i8".to_owned(),
        TypeRef::I16 => "i16".to_owned(),
        TypeRef::I32 => "i32".to_owned(),
        TypeRef::I64 => "i64".to_owned(),
        TypeRef::U8 => "u8".to_owned(),
        TypeRef::U16 => "u16".to_owned(),
        TypeRef::U32 => "u32".to_owned(),
        TypeRef::U64 => "u64".to_owned(),
        TypeRef::F32 => "f32".to_owned(),
        TypeRef::F64 => "f64".to_owned(),
        TypeRef::String => "String".to_owned(),
        TypeRef::Bytes => "Bytes".to_owned(),
        TypeRef::Unit => "()".to_owned(),
        TypeRef::Duration => "Duration".to_owned(),
        TypeRef::Timestamp => "Timestamp".to_owned(),
        TypeRef::Uuid => "Uuid".to_owned(),
        TypeRef::Option(t) => format!("Option<{}>", type_text(t)),
        TypeRef::Vec(t) => format!("Vec<{}>", type_text(t)),
        TypeRef::Map(k, v) => format!("Map<{}, {}>", type_text(k), type_text(v)),
        TypeRef::Lazy(t) => format!("Lazy<{}>", type_text(t)),
        TypeRef::Named(n) => n.clone(),
        TypeRef::Result(t, e) => format!("Result<{}, {}>", type_text(t), type_text(e)),
        TypeRef::Stream(t) => format!("Stream<{}>", type_text(t)),
    }
}

fn record_shape(record: &RecordDef) -> String {
    record
        .fields
        .iter()
        .map(|f| format!("{}: {}", f.name, type_text(&f.ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn enum_shape(en: &EnumDef) -> String {
    let mut variants: Vec<_> = en.variants.iter().collect();
    variants.sort_by_key(|v| v.index);
    variants
        .iter()
        .map(|v| {
            let payload = if v.fields.is_empty() {
                String::new()
            } else if v.tuple {
                let types: Vec<String> = v.fields.iter().map(|f| type_text(&f.ty)).collect();
                format!("({})", types.join(", "))
            } else {
                let fields: Vec<String> = v
                    .fields
                    .iter()
                    .map(|f| format!("{}: {}", f.name, type_text(&f.ty)))
                    .collect();
                format!(" {{ {} }}", fields.join(", "))
            };
            format!("{}{payload} = {}", v.name, v.index)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn method_decl(method: &MethodDef) -> String {
    let params: Vec<String> = method
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, type_text(&p.ty)))
        .collect();
    let returns = match &method.returns {
        TypeRef::Unit => String::new(),
        other => format!(" -> {}", type_text(other)),
    };
    format!(
        "{}{}({}){returns}",
        if method.is_async { "async " } else { "" },
        method.name,
        params.join(", ")
    )
}

// ----- matching -------------------------------------------------------------------

/// The schema's definition of the standard type `t`, when it is exactly the standard one.
fn type_matches(schema: &Schema, t: &StandardType) -> bool {
    match t.kind {
        StandardKind::Record => schema
            .records
            .iter()
            .any(|r| r.name == t.name && r.type_id == t.type_id && record_shape(r) == t.shape),
        StandardKind::Enum | StandardKind::Error => schema.enums.iter().any(|e| {
            e.name == t.name
                && e.type_id == t.type_id
                && e.is_error == (t.kind == StandardKind::Error)
                && enum_shape(e) == t.shape
        }),
    }
}

fn port_matches(def: &PortDef, p: &StandardPort) -> bool {
    def.name == p.name
        && def.port_id == p.port_id
        && def.kind == p.kind
        && def.methods.len() == p.methods.len()
        && p.methods.iter().all(|sm| {
            def.methods
                .iter()
                .any(|dm| dm.method_id == sm.id && method_decl(dm) == sm.decl)
        })
}

/// Collects the names `ty` refers to.
fn mentioned(ty: &TypeRef, out: &mut BTreeSet<String>) {
    match ty {
        TypeRef::Named(n) => {
            out.insert(n.clone());
        }
        TypeRef::Option(t) | TypeRef::Vec(t) | TypeRef::Lazy(t) | TypeRef::Stream(t) => {
            mentioned(t, out);
        }
        TypeRef::Map(a, b) | TypeRef::Result(a, b) => {
            mentioned(a, out);
            mentioned(b, out);
        }
        _ => {}
    }
}

/// The names the schema's definition of the standard type `name` refers to.
fn type_dependencies(schema: &Schema, name: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Some(record) = schema.records.iter().find(|r| r.name == name) {
        for f in &record.fields {
            mentioned(&f.ty, &mut out);
        }
    }
    if let Some(en) = schema.enums.iter().find(|e| e.name == name) {
        for v in &en.variants {
            for f in &v.fields {
                mentioned(&f.ty, &mut out);
            }
        }
    }
    out
}

/// The standard items of a schema that the platform runtimes already provide, and that the
/// generators therefore leave out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Covered {
    /// Names of the standard types the schema declares exactly as the standard library does.
    pub types: BTreeSet<&'static str>,
    /// Names of the standard ports the schema declares exactly as the standard library does.
    pub ports: BTreeSet<&'static str>,
}

/// Which standard items `schema` contains exactly.
///
/// A standard type that refers to another standard type (`HttpRequest` to `Header`) is covered
/// only while everything it refers to is: the runtime's `HttpRequest` holds the runtime's
/// `Header`, so a schema with a different `Header` does not get the runtime's `HttpRequest`. The
/// same goes for a port and the types of its methods.
///
/// ```
/// use undra_bindgen::stdlib::covered;
/// use undra_meta::Schema;
///
/// assert!(covered(&Schema::new("demo")).types.is_empty());
/// ```
#[must_use]
pub fn covered(schema: &Schema) -> Covered {
    let mut types: BTreeSet<&'static str> = TYPES
        .iter()
        .filter(|t| type_matches(schema, t))
        .map(|t| t.name)
        .collect();
    loop {
        let before = types.len();
        let snapshot = types.clone();
        types.retain(|name| {
            type_dependencies(schema, name)
                .iter()
                .all(|dep| !is_standard_type_name(dep) || snapshot.contains(dep.as_str()))
        });
        if types.len() == before {
            break;
        }
    }
    let ports = PORTS
        .iter()
        .filter(|p| {
            schema.ports.iter().any(|def| {
                port_matches(def, p) && {
                    let mut names = BTreeSet::new();
                    for method in &def.methods {
                        for param in &method.params {
                            mentioned(&param.ty, &mut names);
                        }
                        mentioned(&method.returns, &mut names);
                    }
                    names
                        .iter()
                        .all(|n| !is_standard_type_name(n) || types.contains(n.as_str()))
                }
            })
        })
        .map(|p| p.name)
        .collect();
    Covered { types, ports }
}

/// How the platform runtime of `lang` spells the standard type `name` (one of [`TYPES`]).
///
/// Every runtime exports every standard type, so generated code refers to all eight and
/// declares none (ADR-024):
///
/// * TypeScript: `@undra/runtime` exports all eight types with their codecs (`HttpRequestCodec`,
///   ...) from `adapters/types.ts` and `adapters/codecs.ts`, under the standard names.
/// * Kotlin: `dev.undra.runtime.adapters` declares all eight as public classes whose companion
///   object is the `UndraCodec` (`StandardRecords.kt`), under the standard names.
/// * Swift: `UndraRuntime` exports all eight as public types (`Core/StandardRecords.swift`),
///   under the standard names except `AppState`, which is `UndraAppState`: an app's own
///   `AppState` is the commonest type name in Swift, and the runtime has exported it under that
///   name since v1 (ADR-024, amended).
pub(crate) fn runtime_spelling(lang: Lang, name: &'static str) -> &'static str {
    match lang {
        Lang::Swift if name == "AppState" => "UndraAppState",
        Lang::Swift | Lang::TypeScript | Lang::Kotlin => name,
    }
}

/// A schema item that has the name of a standard one but not its id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IdClash {
    /// `record`, `enum`, `error`, `object` or `port`.
    pub(crate) what: &'static str,
    /// The shared name.
    pub(crate) name: String,
    /// The id the standard item has.
    pub(crate) standard: u32,
    /// The id the schema's item has.
    pub(crate) found: u32,
}

/// Items named like a standard type or port whose id is not the standard one. Ids come from
/// names (SPEC 1.1), so the macros never produce such an item; a hand-written or foreign schema
/// can, and the runtimes would route its calls to the standard adapters.
pub(crate) fn id_clashes(schema: &Schema) -> Vec<IdClash> {
    let mut out = Vec::new();
    let mut type_item = |what: &'static str, name: &str, id: u32| {
        if let Some(t) = TYPES.iter().find(|t| t.name == name) {
            if t.type_id != id {
                out.push(IdClash {
                    what,
                    name: name.to_owned(),
                    standard: t.type_id,
                    found: id,
                });
            }
        }
    };
    for r in &schema.records {
        type_item("record", &r.name, r.type_id);
    }
    for e in &schema.enums {
        type_item(
            if e.is_error { "error" } else { "enum" },
            &e.name,
            e.type_id,
        );
    }
    for o in &schema.objects {
        type_item("object", &o.name, o.type_id);
    }
    for port in &schema.ports {
        if let Some(p) = PORTS.iter().find(|p| p.name == port.name) {
            if p.port_id != port.port_id {
                out.push(IdClash {
                    what: "port",
                    name: port.name.clone(),
                    standard: p.port_id,
                    found: port.port_id,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use undra_meta::ids;

    use super::*;

    #[test]
    fn every_id_is_the_fnv_derivation() {
        for t in TYPES {
            assert_eq!(t.type_id, ids::type_id(t.name), "type {}", t.name);
        }
        for p in PORTS {
            assert_eq!(p.port_id, ids::port_id(p.name), "port {}", p.name);
            for method in p.methods {
                assert_eq!(
                    method.id,
                    ids::port_method_id(p.name, method.name()),
                    "{}.{}",
                    p.name,
                    method.name()
                );
            }
        }
    }

    #[test]
    fn the_table_has_the_ten_ports_and_eight_types() {
        assert_eq!(PORTS.len(), 10);
        assert_eq!(TYPES.len(), 8);
        let names: BTreeSet<_> = PORTS.iter().map(|p| p.name).collect();
        assert_eq!(names.len(), 10, "port names are unique");
        let names: BTreeSet<_> = TYPES.iter().map(|t| t.name).collect();
        assert_eq!(names.len(), 8, "type names are unique");
    }

    #[test]
    fn method_declarations_parse() {
        let m = PORTS[3].methods[0];
        assert_eq!(m.name(), "request");
        assert!(m.is_async());
        let m = PORTS[0].methods[0];
        assert_eq!(m.name(), "now_ms");
        assert!(!m.is_async());
    }

    #[test]
    fn every_runtime_exports_every_standard_type() {
        for t in TYPES {
            assert_eq!(runtime_spelling(Lang::TypeScript, t.name), t.name);
            assert_eq!(runtime_spelling(Lang::Kotlin, t.name), t.name);
            // Swift spells `AppState` as the runtime's `UndraAppState` and the rest as they are.
            let expected = if t.name == "AppState" {
                "UndraAppState"
            } else {
                t.name
            };
            assert_eq!(runtime_spelling(Lang::Swift, t.name), expected);
        }
    }
}
