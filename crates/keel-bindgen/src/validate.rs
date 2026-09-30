//! Schema validation for code generation.
//!
//! [`validate`] runs [`Schema::validate`] and then the checks only a
//! generator can make: names that collide once they are converted to each
//! language's conventions, ids that hash to the same value, names the
//! generated code itself depends on, and the few schema shapes the emitted
//! code cannot express yet. Every problem is reported, not just the first, so a
//! schema author can fix them in one pass.
//!
//! The diagnostic codes follow SPEC section 12:
//!
//! | Code | Meaning here |
//! |---|---|
//! | E0001 | a type the generated code cannot express |
//! | E0010 | an error variant without a usable `#[error("...")]` message |
//! | E0011 | a store without a constructor (from [`Schema::validate`]) |
//! | E0031 | an event port method with a return type |
//! | E0050 | duplicate type name, id or variant index |
//! | E0051 | a name that collides after case conversion or is not an identifier (new in bindgen) |

use core::fmt;
use std::collections::{BTreeMap, HashMap, HashSet};

use keel_meta::{
    EnumDef, FieldDef, MethodDef, ObjectDef, ParamDef, PortKind, QueryKind, Schema, SchemaError,
    TypeRef,
};

use crate::model::{Ret, parse_message, query_handle_name};
use crate::naming;

/// Names the generated code refers to in at least one language. A schema type
/// with one of these names would shadow it, so it is rejected up front.
const RESERVED_TYPE_NAMES: &[&str] = &[
    // Swift standard library and Foundation.
    "Array",
    "AsyncThrowingStream",
    "Bool",
    "Codable",
    "Data",
    "Date",
    "Dictionary",
    "Double",
    "Duration",
    "Error",
    "Float",
    "Hashable",
    "Int16",
    "Int32",
    "Int64",
    "Int8",
    "LocalizedError",
    "MainActor",
    "Never",
    "Observable",
    "Optional",
    "Sendable",
    "String",
    "Task",
    "UInt16",
    "UInt32",
    "UInt64",
    "UInt8",
    "UUID",
    "Void",
    // Kotlin standard library and coroutines.
    "Any",
    "Boolean",
    "Byte",
    "ByteArray",
    "Flow",
    "Int",
    "List",
    "Long",
    "Map",
    "MutableStateFlow",
    "Nothing",
    "Short",
    "StateFlow",
    "Throwable",
    "UByte",
    "UInt",
    "ULong",
    "UShort",
    "Unit",
    // TypeScript / JavaScript globals.
    "AbortSignal",
    "AsyncIterable",
    "BigInt",
    "Number",
    "Object",
    "Promise",
    "Record",
    "Set",
    "Symbol",
    "Uint8Array",
    // The Keel runtime and the generated `KeelIds` namespace.
    "ChangeOp",
    "CallTarget",
    "Codec",
    "Codecs",
    "Handle",
    "KeelBytes",
    "KeelCodec",
    "KeelCore",
    "KeelEnum",
    "KeelError",
    "KeelException",
    "KeelHandle",
    "KeelIds",
    "KeelObject",
    "KeelPort",
    "KeelPortError",
    "KeelPortException",
    "KeelReader",
    "KeelRecord",
    "KeelReplyError",
    "KeelReplyException",
    "KeelResult",
    "KeelStore",
    "KeelWriter",
    "KeyedPatch",
    "Observe",
    "PatchError",
    "PatchOp",
    "PortImpl",
    "ReplyStatus",
    "Signal",
    "Timestamp",
    "WireError",
    "WireException",
];

/// Member names a generated object or store cannot use because a runtime base
/// class declares them or the generated code adds them.
const RESERVED_MEMBERS: &[&str] = &[
    "apply", "close", "core", "handle", "deinit", "init", "resync", "self", "signal", "typeId",
];

/// A problem that makes a schema unfit for code generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindgenError {
    /// A violation found by [`Schema::validate`].
    Schema(SchemaError),
    /// Two things share a type name, a type or method id, or a variant index
    /// (E0050).
    Duplicate {
        /// What is duplicated: `type name`, `type id`, `method id`, ...
        what: &'static str,
        /// The duplicated name or number.
        value: String,
        /// Where the clash is.
        at: String,
    },
    /// Distinct source names that convert to the same identifier, or a name
    /// that cannot be an identifier (E0051).
    NameCollision {
        /// The scope the names live in, such as `record Todo` or `object Store`.
        at: String,
        /// The source names involved.
        names: Vec<String>,
        /// The identifier they all become.
        converted: String,
        /// Why that is a problem.
        why: String,
    },
    /// A type or shape the generated code cannot express (E0001).
    Unsupported {
        /// Where it appears.
        at: String,
        /// What is wrong.
        what: String,
        /// How to fix the schema.
        help: String,
    },
    /// An error variant without a usable message (E0010).
    ErrorMessage {
        /// Where it appears.
        at: String,
        /// What is wrong.
        what: String,
    },
    /// An event port method that returns something (E0031).
    EventReturn {
        /// The port.
        port: String,
        /// The method.
        method: String,
    },
}

impl BindgenError {
    /// The stable diagnostic code from SPEC section 12.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            BindgenError::Schema(e) => e.code(),
            BindgenError::Duplicate { .. } => "E0050",
            BindgenError::NameCollision { .. } => "E0051",
            BindgenError::Unsupported { .. } => "E0001",
            BindgenError::ErrorMessage { .. } => "E0010",
            BindgenError::EventReturn { .. } => "E0031",
        }
    }
}

impl fmt::Display for BindgenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = self.code();
        match self {
            BindgenError::Schema(e) => write!(f, "{e}"),
            BindgenError::Duplicate { what, value, at } => write!(
                f,
                "error[keel::{code}]: duplicate {what} `{value}` at {at}; names and ids must be unique within a core"
            ),
            BindgenError::NameCollision {
                at,
                names,
                converted,
                why,
            } => write!(
                f,
                "error[keel::{code}]: at {at}, {} all become `{converted}`: {why}; rename one of them",
                names
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            BindgenError::Unsupported { at, what, help } => {
                write!(f, "error[keel::{code}]: {what} at {at}; {help}")
            }
            BindgenError::ErrorMessage { at, what } => write!(
                f,
                "error[keel::{code}]: {what} at {at}; give the variant an #[error(\"...\")] message"
            ),
            BindgenError::EventReturn { port, method } => write!(
                f,
                "error[keel::{code}]: event port `{port}` method `{method}` returns a value; event methods are fire-and-forget and return ()"
            ),
        }
    }
}

impl std::error::Error for BindgenError {}

impl From<SchemaError> for BindgenError {
    fn from(e: SchemaError) -> Self {
        BindgenError::Schema(e)
    }
}

/// Checks that `schema` can be turned into bindings.
///
/// Runs [`Schema::validate`] first; when that fails its errors are returned
/// alone because the remaining checks assume every name resolves.
///
/// # Errors
///
/// Returns every problem found, in schema order.
///
/// ```
/// use keel_bindgen::validate;
/// use keel_meta::Schema;
///
/// assert!(validate(&Schema::new("demo")).is_ok());
/// ```
pub fn validate(schema: &Schema) -> Result<(), Vec<BindgenError>> {
    if let Err(errors) = schema.validate() {
        return Err(errors.into_iter().map(BindgenError::Schema).collect());
    }
    let mut checker = Checker::new(schema);
    checker.run();
    if checker.errors.is_empty() {
        Ok(())
    } else {
        Err(checker.errors)
    }
}

/// What a type name refers to, for the checks below.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Record,
    Enum,
    Error,
    Object,
}

struct Checker<'a> {
    schema: &'a Schema,
    kinds: HashMap<&'a str, Kind>,
    errors: Vec<BindgenError>,
}

impl<'a> Checker<'a> {
    fn new(schema: &'a Schema) -> Self {
        let mut kinds = HashMap::new();
        for r in &schema.records {
            kinds.insert(r.name.as_str(), Kind::Record);
        }
        for e in &schema.enums {
            kinds.insert(
                e.name.as_str(),
                if e.is_error { Kind::Error } else { Kind::Enum },
            );
        }
        for o in &schema.objects {
            kinds.insert(o.name.as_str(), Kind::Object);
        }
        Checker {
            schema,
            kinds,
            errors: Vec::new(),
        }
    }

    fn run(&mut self) {
        self.check_identifiers();
        self.check_type_names();
        self.check_ids();
        for record in &self.schema.records {
            let at = format!("record {}", record.name);
            self.check_fields(&at, &record.fields);
            for field in &record.fields {
                self.check_value_type(&field.ty, &format!("{at}, field {}", field.name));
            }
        }
        for en in &self.schema.enums {
            self.check_enum(en);
        }
        for object in &self.schema.objects {
            self.check_object(object);
        }
        for function in &self.schema.functions {
            let at = format!("function {}", function.name);
            self.check_params(&at, &function.params);
            self.check_return(&function.returns, &at, true);
        }
        for port in &self.schema.ports {
            self.check_port(port);
        }
        for query in &self.schema.queries {
            self.check_query(query);
        }
        self.check_top_level_values();
    }

    // ----- identifiers ------------------------------------------------------

    fn bad_ident(&mut self, at: &str, name: &str) {
        if !naming::is_valid_ident(name) {
            self.errors.push(BindgenError::NameCollision {
                at: at.to_owned(),
                names: vec![name.to_owned()],
                converted: name.to_owned(),
                why: "it is not an identifier (ASCII letters, digits and underscores, not starting with a digit)"
                    .to_owned(),
            });
        }
    }

    fn check_identifiers(&mut self) {
        let s = self.schema;
        for r in &s.records {
            self.bad_ident("a record", &r.name);
            for f in &r.fields {
                self.bad_ident(&format!("record {}", r.name), &f.name);
            }
        }
        for e in &s.enums {
            self.bad_ident("an enum", &e.name);
            for v in &e.variants {
                self.bad_ident(&format!("enum {}", e.name), &v.name);
                if !v.tuple {
                    for f in &v.fields {
                        self.bad_ident(&format!("enum {}, variant {}", e.name, v.name), &f.name);
                    }
                }
            }
        }
        for o in &s.objects {
            self.bad_ident("an object", &o.name);
            for m in o.constructors.iter().chain(&o.methods) {
                self.bad_ident(&format!("object {}", o.name), &m.name);
                for p in &m.params {
                    self.bad_ident(&format!("object {}, method {}", o.name, m.name), &p.name);
                }
            }
            if let Some(store) = &o.store {
                for g in &store.signals {
                    self.bad_ident(&format!("store {}", o.name), &g.name);
                }
            }
        }
        for f in &s.functions {
            self.bad_ident("a function", &f.name);
            for p in &f.params {
                self.bad_ident(&format!("function {}", f.name), &p.name);
            }
        }
        for p in &s.ports {
            self.bad_ident("a port", &p.name);
            for m in &p.methods {
                self.bad_ident(&format!("port {}", p.name), &m.name);
                for a in &m.params {
                    self.bad_ident(&format!("port {}, method {}", p.name, m.name), &a.name);
                }
            }
        }
        for q in &s.queries {
            self.bad_ident("a query", &q.name);
            for p in &q.params {
                self.bad_ident(&format!("query {}", q.name), &p.name);
            }
        }
    }

    // ----- names and ids ----------------------------------------------------

    /// Duplicate type names across the whole generated namespace, and clashes
    /// with names the generated code depends on.
    fn check_type_names(&mut self) {
        let s = self.schema;
        let mut seen: HashMap<String, &'static str> = HashMap::new();
        let mut declare = |errors: &mut Vec<BindgenError>, name: String, what: &'static str| {
            if RESERVED_TYPE_NAMES.contains(&name.as_str()) {
                errors.push(BindgenError::Duplicate {
                    what: "type name",
                    value: name.clone(),
                    at: format!(
                        "{what} `{name}` (it collides with a name the generated code depends on)"
                    ),
                });
            }
            if let Some(first) = seen.get(&name) {
                // keel-meta reports duplicates among records, enums and
                // objects itself; only cross-namespace clashes are new.
                let both_schema_types = matches!(*first, "record" | "enum" | "error" | "object")
                    && matches!(what, "record" | "enum" | "error" | "object");
                if !both_schema_types {
                    errors.push(BindgenError::Duplicate {
                        what: "type name",
                        value: name,
                        at: format!("{what} (already used by a {first})"),
                    });
                }
            } else {
                seen.insert(name, what);
            }
        };
        for r in &s.records {
            declare(&mut self.errors, r.name.clone(), "record");
        }
        for e in &s.enums {
            declare(
                &mut self.errors,
                e.name.clone(),
                if e.is_error { "error" } else { "enum" },
            );
        }
        for o in &s.objects {
            declare(&mut self.errors, o.name.clone(), "object");
        }
        for p in &s.ports {
            declare(&mut self.errors, p.name.clone(), "port");
            if p.kind == PortKind::Event {
                declare(
                    &mut self.errors,
                    format!("{}Events", p.name),
                    "event port emitter",
                );
            }
        }
        let mut has_query = false;
        for q in &s.queries {
            if q.kind == QueryKind::Query {
                has_query = true;
                declare(&mut self.errors, query_handle_name(&q.name), "query handle");
            }
        }
        if has_query {
            declare(
                &mut self.errors,
                crate::model::QUERY_STATUS.to_owned(),
                "query status enum",
            );
        }
    }

    fn dup_ids(&mut self, what: &'static str, at: &str, ids: impl Iterator<Item = (u32, String)>) {
        let mut seen: HashMap<u32, String> = HashMap::new();
        for (id, name) in ids {
            match seen.get(&id) {
                Some(first) if *first != name => {
                    self.errors.push(BindgenError::Duplicate {
                        what,
                        value: format!("0x{id:08x}"),
                        at: format!("{at}: `{first}` and `{name}` hash to the same id"),
                    });
                }
                Some(_) => {}
                None => {
                    seen.insert(id, name);
                }
            }
        }
    }

    fn check_ids(&mut self) {
        let s = self.schema;
        self.dup_ids(
            "type id",
            "the schema",
            s.records
                .iter()
                .map(|r| (r.type_id, r.name.clone()))
                .chain(s.enums.iter().map(|e| (e.type_id, e.name.clone())))
                .chain(s.objects.iter().map(|o| (o.type_id, o.name.clone()))),
        );
        for o in &s.objects {
            self.dup_ids(
                "method id",
                &format!("object {}", o.name),
                o.constructors
                    .iter()
                    .chain(&o.methods)
                    .map(|m| (m.method_id, m.name.clone())),
            );
        }
        self.dup_ids(
            "port id",
            "the schema",
            s.ports.iter().map(|p| (p.port_id, p.name.clone())),
        );
        for p in &s.ports {
            self.dup_ids(
                "method id",
                &format!("port {}", p.name),
                p.methods.iter().map(|m| (m.method_id, m.name.clone())),
            );
        }
        self.dup_ids(
            "function id",
            "the schema",
            s.functions
                .iter()
                .map(|f| (f.method_id, f.name.clone()))
                .chain(
                    s.queries
                        .iter()
                        .filter(|q| q.kind == QueryKind::Mutation)
                        .map(|q| (q.query_id, q.name.clone())),
                ),
        );
        self.dup_ids(
            "query id",
            "the schema",
            s.queries.iter().map(|q| (q.query_id, q.name.clone())),
        );
        for en in &s.enums {
            let mut seen = HashSet::new();
            for v in &en.variants {
                if !seen.insert(v.index) {
                    self.errors.push(BindgenError::Duplicate {
                        what: "variant index",
                        value: v.index.to_string(),
                        at: format!("enum {}", en.name),
                    });
                }
            }
        }
    }

    // ----- name collisions after case conversion ----------------------------

    /// Reports source names in `items` that convert to the same identifier.
    fn unique<'n>(&mut self, at: &str, why: &str, items: impl Iterator<Item = (&'n str, String)>) {
        let mut by_converted: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (source, converted) in items {
            by_converted
                .entry(converted)
                .or_default()
                .push(source.to_owned());
        }
        for (converted, names) in by_converted {
            if names.len() > 1 {
                self.errors.push(BindgenError::NameCollision {
                    at: at.to_owned(),
                    names,
                    converted,
                    why: why.to_owned(),
                });
            }
        }
    }

    fn check_fields(&mut self, at: &str, fields: &[FieldDef]) {
        self.unique(
            at,
            "field names are converted to camelCase",
            fields
                .iter()
                .map(|f| (f.name.as_str(), naming::camel(&f.name))),
        );
    }

    fn check_params(&mut self, at: &str, params: &[ParamDef]) {
        self.unique(
            at,
            "parameter names are converted to camelCase",
            params
                .iter()
                .map(|p| (p.name.as_str(), naming::camel(&p.name))),
        );
    }

    fn check_enum(&mut self, en: &EnumDef) {
        let at = format!("enum {}", en.name);
        if en.variants.is_empty() {
            self.errors.push(BindgenError::Unsupported {
                at: at.clone(),
                what: "an enum with no variants".to_owned(),
                help: "it has no value to send; give it at least one variant".to_owned(),
            });
        }
        let unit = crate::model::is_unit_enum(en);
        self.unique(
            &at,
            "variant names become camelCase in Swift and TypeScript",
            en.variants
                .iter()
                .map(|v| (v.name.as_str(), naming::camel(&v.name))),
        );
        if unit {
            self.unique(
                &at,
                "variant names become UPPER_SNAKE_CASE in Kotlin",
                en.variants
                    .iter()
                    .map(|v| (v.name.as_str(), naming::upper_snake(&v.name))),
            );
        }
        for v in &en.variants {
            let vat = format!("{at}, variant {}", v.name);
            if !v.tuple {
                self.check_fields(&vat, &v.fields);
            }
            for f in &v.fields {
                self.check_value_type(&f.ty, &format!("{vat}, field {}", f.name));
            }
            if en.is_error {
                self.check_error_message(en, v, &vat);
            }
        }
    }

    fn check_error_message(&mut self, en: &EnumDef, v: &keel_meta::VariantDef, at: &str) {
        match &v.message {
            Some(message) => {
                if let Err(problem) = parse_message(v, message) {
                    self.errors.push(BindgenError::ErrorMessage {
                        at: at.to_owned(),
                        what: problem,
                    });
                }
            }
            // `#[error(transparent)]` on a single-field variant: the message
            // is the field's own.
            None if v.fields.len() == 1 => {}
            None => self.errors.push(BindgenError::ErrorMessage {
                at: format!("{at} of error {}", en.name),
                what: "the variant has no message".to_owned(),
            }),
        }
    }

    fn check_object(&mut self, object: &ObjectDef) {
        let at = format!("object {}", object.name);
        // Constructors and methods live in one namespace with the signals.
        let mut members: Vec<(&str, String)> = Vec::new();
        for c in &object.constructors {
            let name = if c.name == "new" {
                "create".to_owned()
            } else {
                naming::camel(&c.name)
            };
            members.push((c.name.as_str(), name));
        }
        for m in &object.methods {
            members.push((m.name.as_str(), naming::camel(&m.name)));
        }
        if let Some(store) = &object.store {
            for g in &store.signals {
                members.push((g.name.as_str(), naming::camel(&g.name)));
            }
        }
        self.unique(
            &at,
            "constructors, methods and signals share one member namespace and are converted to camelCase (`new` becomes `create`)",
            members.iter().map(|(s, c)| (*s, c.clone())),
        );
        self.unique(
            &at,
            "member ids are named UPPER_SNAKE_CASE in Kotlin",
            object
                .constructors
                .iter()
                .chain(&object.methods)
                .map(|m| (m.name.as_str(), naming::upper_snake(&m.name))),
        );
        for (source, converted) in &members {
            if RESERVED_MEMBERS.contains(&converted.as_str()) {
                self.errors.push(BindgenError::NameCollision {
                    at: at.clone(),
                    names: vec![(*source).to_owned()],
                    converted: converted.clone(),
                    why: "the runtime base class or the generated code already declares a member with that name".to_owned(),
                });
            }
        }

        if object.constructors.is_empty() {
            // Plain objects without a constructor can only be reached through
            // other calls, which are not supported; stores are checked by
            // `Schema::validate`.
            if object.store.is_none() {
                self.errors.push(BindgenError::Unsupported {
                    at: at.clone(),
                    what: "an object without a constructor".to_owned(),
                    help: "add a `pub fn new(..) -> Self` to its #[keel::api] impl so the platform can create it".to_owned(),
                });
            }
        }
        for c in &object.constructors {
            let cat = format!("{at}, constructor {}", c.name);
            self.check_params(&cat, &c.params);
            self.check_constructor_return(object, c, &cat);
            self.check_param_types(&c.params, &cat);
        }
        for m in &object.methods {
            let mat = format!("{at}, method {}", m.name);
            self.check_params(&mat, &m.params);
            self.check_param_types(&m.params, &mat);
            self.check_return(&m.returns, &mat, true);
        }
        if let Some(store) = &object.store {
            self.unique(
                &at,
                "signal ids are named in Kotlin as UPPER_SNAKE_CASE members of the store's id object",
                store
                    .signals
                    .iter()
                    .map(|g| (g.name.as_str(), naming::upper_snake(&g.name))),
            );
            for g in &store.signals {
                let gat = format!("{at}, signal {}", g.name);
                if matches!(g.ty, TypeRef::Lazy(_)) {
                    self.errors.push(BindgenError::Unsupported {
                        at: gat.clone(),
                        what: "a Lazy<T> signal".to_owned(),
                        help: "lazy lists need a runtime API that SPEC section 17 does not define yet; expose the items as a Vec<T> signal or a paged method".to_owned(),
                    });
                } else {
                    self.check_value_type(&g.ty, &gat);
                    if matches!(g.ty, TypeRef::Unit) {
                        self.errors.push(BindgenError::Unsupported {
                            at: gat.clone(),
                            what: "a signal of type ()".to_owned(),
                            help: "a signal needs a value".to_owned(),
                        });
                    }
                }
                if g.key.is_some() && !matches!(g.ty, TypeRef::Vec(_)) {
                    self.errors.push(BindgenError::Unsupported {
                        at: gat,
                        what: "a keyed signal that is not a Vec<T>".to_owned(),
                        help: "#[keel(key = ..)] applies to Signal<Vec<T>> only".to_owned(),
                    });
                }
            }
        }
    }

    fn check_constructor_return(&mut self, object: &ObjectDef, c: &MethodDef, at: &str) {
        let ok = match Ret::classify(&c.returns) {
            Some(Ret::Plain(TypeRef::Named(n))) if *n == object.name => true,
            Some(Ret::Result {
                ok: TypeRef::Named(n),
                err,
            }) if *n == object.name => {
                self.check_error_type(err, at);
                true
            }
            _ => false,
        };
        if !ok {
            self.errors.push(BindgenError::Unsupported {
                at: at.to_owned(),
                what: format!("a constructor returning `{}`", c.returns),
                help: format!(
                    "a constructor returns `{0}` or `Result<{0}, E>`",
                    object.name
                ),
            });
        }
    }

    fn check_port(&mut self, port: &keel_meta::PortDef) {
        let at = format!("port {}", port.name);
        self.unique(
            &at,
            "port methods are converted to camelCase",
            port.methods
                .iter()
                .map(|m| (m.name.as_str(), naming::camel(&m.name))),
        );
        self.unique(
            &at,
            "member ids are named UPPER_SNAKE_CASE in Kotlin",
            port.methods
                .iter()
                .map(|m| (m.name.as_str(), naming::upper_snake(&m.name))),
        );
        for m in &port.methods {
            if naming::camel(&m.name) == "portId" {
                self.errors.push(BindgenError::NameCollision {
                    at: at.clone(),
                    names: vec![m.name.clone()],
                    converted: "portId".to_owned(),
                    why: "the id namespace of a port already has `portId`".to_owned(),
                });
            }
            let mat = format!("{at}, method {}", m.name);
            self.check_params(&mat, &m.params);
            self.check_param_types(&m.params, &mat);
            match port.kind {
                PortKind::Event => {
                    if !matches!(m.returns, TypeRef::Unit) || m.is_async {
                        self.errors.push(BindgenError::EventReturn {
                            port: port.name.clone(),
                            method: m.name.clone(),
                        });
                    }
                }
                PortKind::Sync | PortKind::Async => {
                    self.check_return(&m.returns, &mat, false);
                }
            }
        }
    }

    fn check_query(&mut self, query: &keel_meta::QueryDef) {
        let at = format!("query {}", query.name);
        self.check_params(&at, &query.params);
        self.check_param_types(&query.params, &at);
        self.check_return(&query.returns, &at, false);
        if query.kind == QueryKind::Query {
            let ok: &TypeRef = match &query.returns {
                TypeRef::Result(ok, _) => ok,
                other => other,
            };
            if matches!(ok, TypeRef::Unit) {
                self.errors.push(BindgenError::Unsupported {
                    at: at.clone(),
                    what: "a query that returns ()".to_owned(),
                    help: "a query caches a value; use a mutation for effects".to_owned(),
                });
            }
            if matches!(ok, TypeRef::Option(_)) {
                self.errors.push(BindgenError::Unsupported {
                    at,
                    what: "a query that returns an Option".to_owned(),
                    help: "the handle's `data` signal is already optional (no data yet), so a nested option would be ambiguous; return a record or a list".to_owned(),
                });
            }
        }
    }

    /// Duplicate top-level value names: free functions and mutations become
    /// functions, ports become adapter functions.
    fn check_top_level_values(&mut self) {
        let s = self.schema;
        let mut names: Vec<(&str, String)> = Vec::new();
        for f in &s.functions {
            names.push((f.name.as_str(), naming::camel(&f.name)));
        }
        for q in s.queries.iter().filter(|q| q.kind == QueryKind::Mutation) {
            names.push((q.name.as_str(), naming::camel(&q.name)));
        }
        for p in &s.ports {
            if p.kind != PortKind::Event {
                names.push((
                    p.name.as_str(),
                    format!("{}PortImpl", naming::camel(&p.name)),
                ));
            }
        }
        self.unique(
            "the schema",
            "functions, mutations and port adapters are top-level functions named in camelCase",
            names.into_iter(),
        );
    }

    // ----- types ------------------------------------------------------------

    fn check_error_type(&mut self, err: &str, at: &str) {
        if self.kinds.get(err) != Some(&Kind::Error) {
            self.errors.push(BindgenError::Unsupported {
                at: at.to_owned(),
                what: format!("`{err}` as the error type of a Result"),
                help: "the error type of a Result must be a #[keel::error] enum".to_owned(),
            });
        }
    }

    fn check_param_types(&mut self, params: &[ParamDef], at: &str) {
        for p in params {
            self.check_value_type(&p.ty, &format!("{at}, param {}", p.name));
        }
    }

    /// A return: `T`, `Result<T, E>`, `Stream<T>` or `Result<Stream<T>, E>`
    /// with `E` an error enum. `allow_stream` is false for ports and queries.
    fn check_return(&mut self, ty: &TypeRef, at: &str, allow_stream: bool) {
        let at = format!("{at}, return type");
        let Some(ret) = Ret::classify(ty) else {
            self.errors.push(BindgenError::Unsupported {
                at,
                what: format!("the return type `{ty}`"),
                help: "returns are `T`, `Result<T, E>`, `Stream<T>` or `Result<Stream<T>, E>` where E is a #[keel::error] enum".to_owned(),
            });
            return;
        };
        if let Some(err) = ret.error() {
            self.check_error_type(err, &at);
        }
        if ret.is_stream() && !allow_stream {
            self.errors.push(BindgenError::Unsupported {
                at: at.clone(),
                what: "a stream".to_owned(),
                help: "ports and queries cannot return streams".to_owned(),
            });
        }
        match ret {
            Ret::Plain(t) | Ret::Result { ok: t, .. } => {
                if !matches!(t, TypeRef::Unit) {
                    self.check_value_type(t, &at);
                }
            }
            Ret::Stream(item) | Ret::ResultStream { item, .. } => {
                if matches!(item, TypeRef::Unit) {
                    self.errors.push(BindgenError::Unsupported {
                        at,
                        what: "a stream of ()".to_owned(),
                        help: "stream items need a value".to_owned(),
                    });
                } else {
                    self.check_value_type(item, &at);
                }
            }
        }
    }

    /// A type in a value position (field, parameter, item): no `()` anywhere,
    /// no nested `Option`, no object handles.
    fn check_value_type(&mut self, ty: &TypeRef, at: &str) {
        match ty {
            TypeRef::Unit => self.errors.push(BindgenError::Unsupported {
                at: at.to_owned(),
                what: "the type ()".to_owned(),
                help: "() is only legal as a return type; zero-width values defeat length validation (SPEC section 3.1)".to_owned(),
            }),
            TypeRef::Named(name) if self.kinds.get(name.as_str()) == Some(&Kind::Object) => {
                self.errors.push(BindgenError::Unsupported {
                    at: at.to_owned(),
                    what: format!("the object `{name}` used as a value"),
                    help: "object handles cannot cross as values yet; return a record with the data, or construct the object from the platform".to_owned(),
                });
            }
            TypeRef::Option(inner) => {
                if matches!(**inner, TypeRef::Option(_)) {
                    self.errors.push(BindgenError::Unsupported {
                        at: at.to_owned(),
                        what: format!("the nested option `{ty}`"),
                        help: "Kotlin and TypeScript cannot tell Some(None) from None; wrap the inner option in a record or an enum".to_owned(),
                    });
                }
                self.check_value_type(inner, at);
            }
            TypeRef::Vec(inner) | TypeRef::Lazy(inner) | TypeRef::Stream(inner) => {
                self.check_value_type(inner, at);
            }
            TypeRef::Map(k, v) => {
                self.check_value_type(k, at);
                self.check_value_type(v, at);
            }
            TypeRef::Result(ok, err) => {
                self.check_value_type(ok, at);
                self.check_value_type(err, at);
            }
            _ => {}
        }
    }
}
