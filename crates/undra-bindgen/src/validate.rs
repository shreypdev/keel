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
//! | E0052 | an item named like a standard library item, with another id (new in bindgen) |

use core::fmt;
use std::collections::{BTreeMap, HashMap, HashSet};

use undra_meta::{
    EnumDef, FieldDef, MethodDef, ObjectDef, ParamDef, PortKind, QueryKind, Schema, SchemaError,
    TypeRef,
};

use crate::model::{Callee, Ret, names, parse_message, query_handle_name};
use crate::naming;
use crate::stdlib;

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
    "Decimal",
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
    "BigDecimal",
    "Boolean",
    "Byte",
    "ByteArray",
    "Flow",
    "InfiniteQuery",
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
    // The Undra runtime and the generated `UndraIds` namespace.
    "ChangeOp",
    "CallTarget",
    "Codec",
    "Codecs",
    "Handle",
    "LazyList",
    "UndraBytes",
    "UndraCodec",
    "UndraCore",
    "UndraEnum",
    "UndraError",
    "UndraException",
    "UndraHandle",
    "UndraIds",
    "UndraLazyList",
    "UndraLazyListObject",
    "UndraObject",
    "UndraPort",
    "UndraPortError",
    "UndraPortException",
    "UndraReader",
    "UndraRecord",
    "UndraReplyError",
    "UndraReplyException",
    "UndraResult",
    "UndraStore",
    "UndraWriter",
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
    /// A type has a name the generated code itself uses in at least one language, so it would
    /// shadow it (E0050).
    ReservedName {
        /// What the item is: `record`, `enum`, `error`, `object`, `port`, ...
        what: &'static str,
        /// The reserved name.
        name: String,
    },
    /// A name that cannot be an identifier in a target language (E0051).
    NotAnIdentifier {
        /// The scope the name lives in, such as `record Todo`.
        at: String,
        /// The name.
        name: String,
    },
    /// An item has the name of a standard library item but not its id (E0052).
    ShadowsStandard {
        /// What the item is: `record`, `enum`, `error`, `object` or `port`.
        what: &'static str,
        /// The shared name.
        name: String,
        /// The id the standard item has.
        standard: u32,
        /// The id the schema's item has.
        found: u32,
    },
    /// A type or shape the generated code cannot express (E0001).
    Unsupported {
        /// Where it appears.
        at: String,
        /// What is wrong.
        what: String,
        /// Why the generated code cannot express it.
        why: String,
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
            BindgenError::Duplicate { .. } | BindgenError::ReservedName { .. } => "E0050",
            BindgenError::NameCollision { .. } | BindgenError::NotAnIdentifier { .. } => "E0051",
            BindgenError::ShadowsStandard { .. } => "E0052",
            BindgenError::Unsupported { .. } => "E0001",
            BindgenError::ErrorMessage { .. } => "E0010",
            BindgenError::EventReturn { .. } => "E0031",
        }
    }
}

impl fmt::Display for BindgenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = self.code();
        let (what, why, fix) = match self {
            BindgenError::Schema(e) => return write!(f, "{e}"),
            BindgenError::Duplicate { what, value, at } => (
                format!("duplicate {what} `{value}` at {at}"),
                "names and ids identify a type, a method or a variant to the platforms and in the generated code, so each must be unique within a core".to_owned(),
                match *what {
                    "variant index" => "give every variant of the enum an index of its own".to_owned(),
                    w if w.ends_with(" id") => "rename one of the two items: ids are hashes of the names, so a different name gives a different id".to_owned(),
                    _ => "rename one of the two, or remove the duplicate".to_owned(),
                },
            ),
            BindgenError::ReservedName { what, name } => (
                format!("{what} `{name}` has a name the generated code depends on"),
                format!(
                    "the generated bindings use a type called `{name}` (a standard type of Swift, Kotlin or TypeScript, or one of the Undra runtime's), so a type of the schema with the same name would shadow it"
                ),
                format!("rename it, for example `My{name}`"),
            ),
            BindgenError::NotAnIdentifier { at, name } => (
                format!("at {at}, `{name}` is not a name the generated code can use"),
                "every name of the schema becomes an identifier in Swift, Kotlin and TypeScript: ASCII letters, digits and underscores, not starting with a digit".to_owned(),
                "rename it to letters, digits and underscores, starting with a letter".to_owned(),
            ),
            BindgenError::NameCollision {
                at,
                names,
                converted,
                why,
            } => match names.as_slice() {
                [name] => (
                    if name == converted {
                        format!("at {at}, `{name}` is a name the generated code already uses")
                    } else {
                        format!("at {at}, `{name}` becomes `{converted}`, a name the generated code already uses")
                    },
                    why.clone(),
                    "rename it so that it does not meet that name".to_owned(),
                ),
                _ => (
                    format!(
                        "at {at}, {} all become `{converted}`",
                        names
                            .iter()
                            .map(|n| format!("`{n}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    format!("{why}, so names that differ only in case or underscores end up equal"),
                    "rename one of them so that they stay distinct after conversion".to_owned(),
                ),
            },
            BindgenError::ShadowsStandard {
                what,
                name,
                standard,
                found,
            } => (
                format!(
                    "{what} `{name}` has the name of an Undra standard library item but the id 0x{found:08x} instead of 0x{standard:08x}"
                ),
                "every platform runtime implements the standard ports and types under those names and ids, and generated code refers to them instead of declaring them, so a different item cannot share the name".to_owned(),
                format!("rename it, for example `My{name}`, or use the standard one from `undra-ports`"),
            ),
            BindgenError::Unsupported { at, what, why, help } => {
                (format!("{what} at {at}"), why.clone(), help.clone())
            }
            BindgenError::ErrorMessage { at, what } => (
                format!("{what} at {at}"),
                "the platforms show the message of an error to the user, so every variant needs one they can format".to_owned(),
                "give the variant an `#[error(\"..\")]` message".to_owned(),
            ),
            BindgenError::EventReturn { port, method } => (
                format!("event port `{port}` method `{method}` returns a value or is `async`"),
                "events are fire-and-forget notifications from the host; nothing waits for them, so there is no reply to carry a value".to_owned(),
                "make the method a plain `fn` returning `()`, or make the port a request/reply port".to_owned(),
            ),
        };
        f.write_str(&undra_meta::diag::message(code, what, why, fix))
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
/// use undra_bindgen::validate;
/// use undra_meta::Schema;
///
/// assert!(validate(&Schema::new("demo")).is_ok());
/// ```
pub fn validate(schema: &Schema) -> Result<(), Vec<BindgenError>> {
    validate_for(schema, false)
}

/// [`validate`] for a generator that does (`emit_standard`) or does not declare the standard
/// library: declared, the standard ports claim their names like any other port.
pub(crate) fn validate_for(schema: &Schema, emit_standard: bool) -> Result<(), Vec<BindgenError>> {
    if let Err(errors) = schema.validate() {
        return Err(errors.into_iter().map(BindgenError::Schema).collect());
    }
    let mut checker = Checker::new(schema, emit_standard);
    checker.run();
    if checker.errors.is_empty() {
        Ok(())
    } else {
        Err(checker.errors)
    }
}

/// The names an `ObservableObject` store cannot use for a member, which only matter in the iOS 15 / 16 Swift
/// mode (ADR-045): the protocol declares `objectWillChange`.
const OBSERVABLE_OBJECT_MEMBERS: &[&str] = &["objectWillChange"];

/// The E0051 problems of `schema` that exist only in the given Swift observation `mode` (none for
/// `Observation`): a store member named like a member of `ObservableObject`.
pub(crate) fn swift_floor(schema: &Schema, mode: crate::SwiftObservation) -> Vec<BindgenError> {
    let mut errors = Vec::new();
    if mode != crate::SwiftObservation::ObservableObject {
        return errors;
    }
    for object in schema.objects.iter().filter(|o| o.store.is_some()) {
        let members = object
            .constructors
            .iter()
            .chain(&object.methods)
            .map(|m| m.name.as_str())
            .chain(
                object
                    .store
                    .iter()
                    .flat_map(|s| s.signals.iter().map(|g| g.name.as_str())),
            );
        for source in members {
            let converted = naming::camel(source);
            if OBSERVABLE_OBJECT_MEMBERS.contains(&converted.as_str()) {
                errors.push(BindgenError::NameCollision {
                    at: format!("object {}", object.name),
                    names: vec![source.to_owned()],
                    converted,
                    why: "with the iOS 15 / 16 store shape (`swift_observation = \"observable-object\"`, the default for a deployment target below iOS 17) a store is a Combine `ObservableObject`, which declares that member itself".to_owned(),
                });
            }
        }
    }
    errors
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
    emit_standard: bool,
}

impl<'a> Checker<'a> {
    fn new(schema: &'a Schema, emit_standard: bool) -> Self {
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
            emit_standard,
        }
    }

    fn run(&mut self) {
        self.check_standard_names();
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

    // ----- the standard library -----------------------------------------------

    /// Items named like a standard library item with another id (E0052).
    fn check_standard_names(&mut self) {
        for clash in stdlib::id_clashes(self.schema) {
            self.errors.push(BindgenError::ShadowsStandard {
                what: clash.what,
                name: clash.name,
                standard: clash.standard,
                found: clash.found,
            });
        }
    }

    // ----- identifiers ------------------------------------------------------

    fn bad_ident(&mut self, at: &str, name: &str) {
        if !naming::is_valid_ident(name) {
            self.errors.push(BindgenError::NotAnIdentifier {
                at: at.to_owned(),
                name: name.to_owned(),
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
                // An instantiation of a generic method is named `pinned<Todo>`: the native name
                // is the generic method's own (ADR-058).
                self.bad_ident(&format!("object {}", o.name), m.names().native);
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
            self.bad_ident("a function", f.names().native);
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
                errors.push(BindgenError::ReservedName {
                    what,
                    name: name.clone(),
                });
            }
            if let Some(first) = seen.get(&name) {
                // undra-meta reports duplicates among records, enums and
                // objects itself; only cross-namespace clashes are new.
                let both_schema_types = matches!(*first, "record" | "enum" | "error" | "object")
                    && matches!(what, "record" | "enum" | "error" | "object");
                if !both_schema_types {
                    errors.push(BindgenError::Duplicate {
                        what: "type name",
                        at: format!("the {what} `{name}` (the name is already used by a {first})"),
                        value: name,
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
        // The standard ports are not declared in generated code (ADR-024), so they do not
        // claim a name there: an app may have its own `Timer` or `Log`.
        let standard = if self.emit_standard {
            stdlib::Covered::default()
        } else {
            stdlib::covered(s)
        };
        for p in s
            .ports
            .iter()
            .filter(|p| !standard.ports.contains(p.name.as_str()))
        {
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
                        at: format!("{at} (`{first}` and `{name}` hash to the same id)"),
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
            // A callback interface answers two more methods, reserved by the protocol (ADR-041):
            // a collision of a user method's id with either would route a call wrongly.
            let reserved = (p.kind == PortKind::Callback).then(|| {
                [
                    (
                        undra_meta::ids::callback_release_id(&p.name),
                        undra_meta::ids::CALLBACK_RELEASE.to_owned(),
                    ),
                    (
                        undra_meta::ids::callback_cancel_id(&p.name),
                        undra_meta::ids::CALLBACK_CANCEL.to_owned(),
                    ),
                ]
            });
            self.dup_ids(
                "method id",
                &format!("port {}", p.name),
                p.methods
                    .iter()
                    .map(|m| (m.method_id, m.name.clone()))
                    .chain(reserved.into_iter().flatten()),
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

    /// The id constants of the instantiations of generic functions are named after the function
    /// and the type (`newest_Todo`, ADR-058), in camelCase for Swift and TypeScript and in
    /// UPPER_SNAKE_CASE for Kotlin: a collision with another id constant is E0051. Collisions among
    /// definitions that are not instantiations are the member checks' business and are not
    /// repeated here.
    fn unique_ids_of_instantiations<'n>(
        &mut self,
        at: &str,
        items: impl Iterator<Item = (&'n str, bool, String)>,
    ) {
        let items: Vec<(&str, bool, String)> = items.collect();
        for (why, convert) in [
            (
                "id constants are named in camelCase in Swift and TypeScript, and the id of an instantiation of a generic function names the function and the type",
                naming::camel as fn(&str) -> String,
            ),
            (
                "id constants are named UPPER_SNAKE_CASE in Kotlin, and the id of an instantiation of a generic function names the function and the type",
                naming::upper_snake as fn(&str) -> String,
            ),
        ] {
            let mut by_converted: BTreeMap<String, Vec<(&str, bool)>> = BTreeMap::new();
            for (name, generic, id) in &items {
                by_converted
                    .entry(convert(id))
                    .or_default()
                    .push((name, *generic));
            }
            for (converted, group) in by_converted {
                if group.len() > 1 && group.iter().any(|(_, generic)| *generic) {
                    self.errors.push(BindgenError::NameCollision {
                        at: at.to_owned(),
                        names: group.iter().map(|(name, _)| (*name).to_owned()).collect(),
                        converted,
                        why: why.to_owned(),
                    });
                }
            }
        }
    }

    /// TypeScript presents a generic function as one exported overload set and keeps a private
    /// function or method per instantiation, named like its id constant (`pinnedTodo`, ADR-058),
    /// in the same scope as the store's signals and the other families' overload sets: a name
    /// there that meets one of them would be declared twice in the generated class or module
    /// (E0051). `others` are `(source name, camelCase name)`; what the id checks already compare
    /// (other functions, methods and constructors) is not repeated.
    fn private_names_of_instantiations<'n>(
        &mut self,
        at: &str,
        instantiations: impl Iterator<Item = (&'n str, String)>,
        others: &[(&'n str, String)],
    ) {
        for (name, id) in instantiations {
            let private = naming::camel(&id);
            for (other, converted) in others {
                if *converted == private {
                    self.errors.push(BindgenError::NameCollision {
                        at: at.to_owned(),
                        names: vec![name.to_owned(), (*other).to_owned()],
                        converted: private.clone(),
                        why: "TypeScript keeps a private function or method for each instantiation of a generic function, named after the function and the type, beside the signals and the other functions".to_owned(),
                    });
                }
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
                why: "an enum without variants has no value that could be sent".to_owned(),
                help: "give it at least one variant".to_owned(),
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

    fn check_error_message(&mut self, en: &EnumDef, v: &undra_meta::VariantDef, at: &str) {
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
                at: at.to_owned(),
                what: format!(
                    "the variant of the error `{}` has no `#[error(\"..\")]` message",
                    en.name
                ),
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
        // The instantiations of one generic method share the native name (ADR-058): it is one
        // member, presented as an overload set.
        let mut families: HashSet<&str> = HashSet::new();
        for m in &object.methods {
            let n = names(&m.name, m.generic.as_ref());
            if m.generic.is_none() || families.insert(n.native) {
                members.push((m.name.as_str(), naming::camel(n.native)));
            }
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
                .map(|m| (m.name.as_str(), naming::upper_snake(&m.names().id))),
        );
        self.unique_ids_of_instantiations(
            &at,
            object
                .constructors
                .iter()
                .chain(&object.methods)
                .map(|m| (m.name.as_str(), m.generic.is_some(), m.names().id)),
        );
        let mut beside: Vec<(&str, String)> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for m in object.methods.iter().filter(|m| m.generic.is_some()) {
            let native = m.names().native;
            if seen.insert(native) {
                beside.push((m.name.as_str(), naming::camel(native)));
            }
        }
        if let Some(store) = &object.store {
            beside.extend(
                store
                    .signals
                    .iter()
                    .map(|g| (g.name.as_str(), naming::camel(&g.name))),
            );
        }
        self.private_names_of_instantiations(
            &at,
            object
                .methods
                .iter()
                .filter(|m| m.generic.is_some())
                .map(|m| (m.name.as_str(), m.names().id)),
            &beside,
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
            // A plain object without a constructor is created by the core and handed out by a
            // method or function that returns it (ADR-040); one nothing returns can never be
            // reached. Stores are checked by `Schema::validate`.
            if object.store.is_none() && !self.is_returned(&object.name) {
                self.errors.push(BindgenError::Unsupported {
                    at: at.clone(),
                    what: "an object without a constructor that nothing returns".to_owned(),
                    why: "the platforms create an object by calling one of its constructors, or receive it from a method or function that returns it, and without either it can never be created".to_owned(),
                    help: format!("add `pub fn new(..) -> Self` to its `#[undra::api]` impl block so the platform can create it, or return `Arc<{}>` from a method that hands it out", object.name),
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
                // A `Lazy<T>` signal is a runtime list the platform pages through (ADR-043); what
                // is checked is its item, like the item of a `Vec<T>`.
                self.check_value_type(&g.ty, &gat);
                if matches!(g.ty, TypeRef::Unit) {
                    self.errors.push(BindgenError::Unsupported {
                        at: gat.clone(),
                        what: "a signal of type ()".to_owned(),
                        why: "a signal holds a value the platform shows, and `()` has none"
                            .to_owned(),
                        help: "give the signal a value type, or remove it".to_owned(),
                    });
                }
                if g.key.is_some() && !matches!(g.ty, TypeRef::Vec(_) | TypeRef::Lazy(_)) {
                    self.errors.push(BindgenError::Unsupported {
                        at: gat,
                        what: "a keyed signal that is not a Vec<T> or a Lazy<T>".to_owned(),
                        why: "a key identifies an item of a list across updates, so only a list can have one".to_owned(),
                        help: "put `#[undra(key = ..)]` on a `Signal<Vec<T>>` or a `Lazy<T>` only, or remove it".to_owned(),
                    });
                }
            }
        }
    }

    /// Whether some method or function returns the object `name` (alone, optional, in a list, or
    /// on the `Ok` side of a `Result`): the core can hand an instance to the platform.
    fn is_returned(&self, name: &str) -> bool {
        fn mentions(ty: &TypeRef, name: &str) -> bool {
            match ty {
                TypeRef::Object(n) => n == name,
                TypeRef::Option(inner) | TypeRef::Vec(inner) => mentions(inner, name),
                TypeRef::Result(ok, _) => mentions(ok, name),
                _ => false,
            }
        }
        self.schema
            .objects
            .iter()
            .flat_map(|o| &o.methods)
            .map(|m| &m.returns)
            .chain(self.schema.functions.iter().map(|f| &f.returns))
            .any(|ty| mentions(ty, name))
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
                why: "a constructor creates the object, so it returns the object (or fails with a typed error)".to_owned(),
                help: format!(
                    "make it return `{0}` or `Result<{0}, E>` where `E` is a `#[undra::error]` enum",
                    object.name
                ),
            });
        }
    }

    fn check_port(&mut self, port: &undra_meta::PortDef) {
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
            // A callback interface's id namespace also names the two reserved methods of the
            // protocol (ADR-041): `releaseInstance` and `cancelCall`.
            if port.kind == PortKind::Callback
                && matches!(
                    naming::camel(&m.name).as_str(),
                    "releaseInstance" | "cancelCall"
                )
            {
                self.errors.push(BindgenError::NameCollision {
                    at: at.clone(),
                    names: vec![m.name.clone()],
                    converted: naming::camel(&m.name),
                    why: "the id namespace of a callback interface already names the reserved methods `__release` (as `releaseInstance`) and `__cancel` (as `cancelCall`)".to_owned(),
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
                PortKind::Sync | PortKind::Async | PortKind::Callback => {
                    self.check_return(&m.returns, &mat, false);
                }
            }
        }
    }

    fn check_query(&mut self, query: &undra_meta::QueryDef) {
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
                    why: "a query caches the value it returns, and `()` has none".to_owned(),
                    help: "use a mutation for a call that only has effects, or return the data the platform needs".to_owned(),
                });
            }
            if matches!(ok, TypeRef::Option(_)) || self.is_newtype_of_option(ok) {
                self.errors.push(BindgenError::Unsupported {
                    at,
                    what: "a query that returns an Option".to_owned(),
                    why: "the handle's `data` signal is already optional (no data yet), so an optional result would be ambiguous".to_owned(),
                    help: "return a record or a list (an empty `Vec` says \"nothing\"), or an enum naming the cases".to_owned(),
                });
            }
        }
    }

    /// Duplicate top-level value names: free functions and mutations become
    /// functions, ports become adapter functions.
    fn check_top_level_values(&mut self) {
        let s = self.schema;
        let mut native_names: Vec<(&str, String)> = Vec::new();
        // The instantiations of one generic function share the native name (ADR-058): it is one
        // function, presented as an overload set.
        let mut families: HashSet<&str> = HashSet::new();
        for f in &s.functions {
            let n = names(&f.name, f.generic.as_ref());
            if f.generic.is_none() || families.insert(n.native) {
                native_names.push((f.name.as_str(), naming::camel(n.native)));
            }
        }
        self.unique_ids_of_instantiations(
            "the schema",
            s.functions
                .iter()
                .map(|f| (f.name.as_str(), f.generic.is_some(), f.names().id))
                .chain(
                    s.queries
                        .iter()
                        .filter(|q| q.kind == QueryKind::Mutation)
                        .map(|q| (q.name.as_str(), false, q.name.clone())),
                ),
        );
        let families: Vec<(&str, String)> = s
            .functions
            .iter()
            .filter(|f| f.generic.is_some())
            .map(|f| (f.name.as_str(), naming::camel(f.names().native)))
            .collect();
        self.private_names_of_instantiations(
            "the schema",
            s.functions
                .iter()
                .filter(|f| f.generic.is_some())
                .map(|f| (f.name.as_str(), f.names().id)),
            &families,
        );
        for q in s.queries.iter().filter(|q| q.kind == QueryKind::Mutation) {
            native_names.push((q.name.as_str(), naming::camel(&q.name)));
        }
        for p in &s.ports {
            if p.kind != PortKind::Event {
                native_names.push((
                    p.name.as_str(),
                    format!("{}PortImpl", naming::camel(&p.name)),
                ));
            }
        }
        self.unique(
            "the schema",
            "functions, mutations and port adapters are top-level functions named in camelCase",
            native_names.into_iter(),
        );
    }

    // ----- types ------------------------------------------------------------

    fn check_error_type(&mut self, err: &str, at: &str) {
        if self.kinds.get(err) != Some(&Kind::Error) {
            self.errors.push(BindgenError::Unsupported {
                at: at.to_owned(),
                what: format!("`{err}` as the error type of a Result"),
                why: "the platforms throw the error by name, and only a `#[undra::error]` enum carries the messages they show".to_owned(),
                help: format!("declare `{err}` with `#[undra::error]`, or use an error enum as the error type"),
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
                why: "a method answers with a value, a typed error, a stream, or a stream that can fail to open; nothing else crosses the boundary".to_owned(),
                help: "return `T`, `Result<T, E>`, `Stream<T>` or `Result<Stream<T>, E>` where `E` is a `#[undra::error]` enum".to_owned(),
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
                why:
                    "port calls and queries are request/reply, and a stream has no place in either"
                        .to_owned(),
                help:
                    "return a `Vec<T>` page, or have the platform push events through an event port"
                        .to_owned(),
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
                        why: "a stream yields values the platform handles one by one, and `()` has none".to_owned(),
                        help: "give the stream an item type, for example `impl Stream<Item = u64>`".to_owned(),
                    });
                } else {
                    self.check_value_type(item, &at);
                }
            }
        }
    }

    /// Whether `ty` is a newtype (ADR-042), however deeply nested, whose innermost type is an
    /// `Option`.
    fn is_newtype_of_option(&self, ty: &TypeRef) -> bool {
        let mut ty = ty;
        for _ in 0..16 {
            let TypeRef::Named(name) = ty else {
                return false;
            };
            match self
                .schema
                .records
                .iter()
                .find(|r| r.transparent && &r.name == name)
                .and_then(|r| r.fields.first())
            {
                Some(field) if matches!(field.ty, TypeRef::Option(_)) => return true,
                Some(field) => ty = &field.ty,
                None => return false,
            }
        }
        false
    }

    /// A type in a value position (field, parameter, item): no `()` anywhere,
    /// no nested `Option`, no object handles.
    fn check_value_type(&mut self, ty: &TypeRef, at: &str) {
        match ty {
            TypeRef::Unit => self.errors.push(BindgenError::Unsupported {
                at: at.to_owned(),
                what: "the type ()".to_owned(),
                why: "`()` occupies zero bytes on the wire, and zero-width values defeat length validation (SPEC section 3.1)".to_owned(),
                help: "remove the value, or use `bool` if you need a marker".to_owned(),
            }),
            TypeRef::Named(name) if self.kinds.get(name.as_str()) == Some(&Kind::Object) => {
                self.errors.push(BindgenError::Unsupported {
                    at: at.to_owned(),
                    what: format!("the object `{name}` used as a value"),
                    why: "an object lives in the core and crosses the boundary as a handle; its contents have no wire representation".to_owned(),
                    help: format!("return it as `Arc<{name}>`, take it as `&{name}` or `Arc<{name}>`, or use a record with the data the platform needs"),
                });
            }
            // Where an object or a callback may stand was decided by `Schema::validate` (E0064,
            // E0004); what stands here is one the generators write.
            TypeRef::Object(_) | TypeRef::Callback(_) => {}
            TypeRef::Option(inner) => {
                if matches!(**inner, TypeRef::Option(_)) {
                    self.errors.push(BindgenError::Unsupported {
                        at: at.to_owned(),
                        what: format!("the nested option `{ty}`"),
                        why: "Kotlin and TypeScript cannot tell `Some(None)` from `None`".to_owned(),
                        help: "wrap the inner option in a record or an enum that names the two cases".to_owned(),
                    });
                } else if let (true, TypeRef::Named(newtype)) =
                    (self.is_newtype_of_option(inner), &**inner)
                {
                    self.errors.push(BindgenError::Unsupported {
                        at: at.to_owned(),
                        what: format!("`Option<{newtype}>`, an option of a newtype that wraps an option"),
                        why: "that is an option of an option on the wire, and Kotlin and TypeScript cannot tell `Some(None)` from `None`".to_owned(),
                        help: format!("make `{newtype}` wrap the value instead of an `Option`, or wrap the option in a record or an enum that names the two cases"),
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
