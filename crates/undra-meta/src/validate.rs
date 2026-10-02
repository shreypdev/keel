//! Semantic validation of a [`Schema`] (SPEC §2.1, §12).
//!
//! [`Schema::validate`] re-checks, on the assembled schema, the rules the
//! macros enforce item by item and the ones only a whole-schema view can
//! enforce (unique type names, resolvable references). `undra-bindgen` and the
//! runtime call it before trusting a schema that came from outside (a
//! `schema.json`, a dlopen'd core).
//!
//! Every failure is reported, not just the first, in schema order: records,
//! enums, objects, functions, ports, queries.

use core::fmt;
use std::collections::{HashMap, HashSet};

use crate::{Schema, TypeRef};

/// The category of a named type, used in [`SchemaError::DuplicateTypeName`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeKind {
    /// A record.
    Record,
    /// A plain enum.
    Enum,
    /// An enum with `is_error = true`.
    Error,
    /// An object or store.
    Object,
}

impl fmt::Display for TypeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TypeKind::Record => "record",
            TypeKind::Enum => "enum",
            TypeKind::Error => "error",
            TypeKind::Object => "object",
        })
    }
}

/// A violation found by [`Schema::validate`].
///
/// Each variant maps to a diagnostic code from SPEC §12, available through
/// [`SchemaError::code`]. `at` is a human-readable location such as
/// `record Todo, field owner` or `object Calculator, method add, return type`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaError {
    /// Two records, enums, errors or objects share a name (E0050).
    DuplicateTypeName {
        /// The clashing name.
        name: String,
        /// What the first declaration is.
        first: TypeKind,
        /// What the later declaration is.
        duplicate: TypeKind,
    },
    /// A `Named` reference does not resolve to any record, enum or object
    /// (E0001).
    UnresolvedType {
        /// The unknown type name.
        name: String,
        /// Where the reference appears.
        at: String,
    },
    /// A `Result` outside the outermost position of a return type (E0005).
    MisplacedResult {
        /// The offending type.
        ty: TypeRef,
        /// Where it appears.
        at: String,
    },
    /// A `Stream` outside a return type, or nested inside anything but the
    /// `Ok` side of a returned `Result` (E0005).
    MisplacedStream {
        /// The offending type.
        ty: TypeRef,
        /// Where it appears.
        at: String,
    },
    /// A `Lazy` anywhere other than as the type of a store signal (E0001).
    MisplacedLazy {
        /// The offending type.
        ty: TypeRef,
        /// Where it appears.
        at: String,
    },
    /// A `Unit` where it is not allowed: as a record or variant field, a
    /// parameter, a signal type, or the inner type of `Option`, `Vec`, `Map`
    /// values or `Lazy` (E0001). `Unit` is legal only as a return type or as
    /// a variant with no fields; zero-width items defeat length validation.
    MisplacedUnit {
        /// Where it appears.
        at: String,
    },
    /// A map whose key type is not `String`, an integer, `Bool` or `Uuid`
    /// (E0006).
    InvalidMapKey {
        /// The rejected key type.
        key: TypeRef,
        /// Where the map appears.
        at: String,
    },
    /// A store object with no constructor (E0011).
    StoreWithoutConstructor {
        /// The object's name.
        object: String,
    },
    /// A `Named` reference that resolves to an object (E0064): objects are named by
    /// [`TypeRef::Object`], and only a constructor's return type names its own object with
    /// `Named` (ADR-040).
    ObjectNamedAsValue {
        /// The object's name.
        name: String,
        /// Where the reference appears.
        at: String,
    },
    /// An `Object` reference that does not resolve to an object (E0064).
    NotAnObject {
        /// The name that is a record, an enum or an error.
        name: String,
        /// What it is.
        found: TypeKind,
        /// Where the reference appears.
        at: String,
    },
    /// An `Object` anywhere but a method, constructor-argument or function parameter or a method
    /// or function return, as `T`, `Option<T>` or `Vec<T>` (and the `Ok` side of a returned
    /// `Result`) (E0064, ADR-040).
    MisplacedObject {
        /// The offending type.
        ty: TypeRef,
        /// Where it appears.
        at: String,
    },
    /// A `Callback` anywhere but a parameter of a method, constructor or function, as `T` or
    /// `Option<T>` (E0004, ADR-041).
    MisplacedCallback {
        /// The offending type.
        ty: TypeRef,
        /// Where it appears.
        at: String,
    },
    /// A `Callback` that does not name a port of kind `Callback` (E0004).
    NotACallback {
        /// The name.
        name: String,
        /// Where the reference appears.
        at: String,
    },
    /// A `transparent` record (a newtype, ADR-042) that does not have exactly one field named
    /// `value` without `#[undra(default)]` (E0007).
    BadTransparentRecord {
        /// The record.
        name: String,
        /// What is wrong, in a few words.
        problem: &'static str,
    },
    /// An `infinite` query that does not meet ADR-043's shape: it returns `Vec<T>` of a record
    /// `T` with a field called `item_key`, its cursor is a value type, and it is a query (E0073).
    BadInfiniteQuery {
        /// The query.
        query: String,
        /// What is wrong.
        problem: String,
    },
    /// A method of a callback port that is neither fire-and-forget (`()`, not `async`) nor
    /// `async` with a `Result`, or whose name starts with `__` (reserved for `__release` and
    /// `__cancel`) (E0071, ADR-041).
    BadCallbackMethod {
        /// The callback port.
        port: String,
        /// The method.
        method: String,
        /// What is wrong, in a few words.
        problem: &'static str,
    },
}

impl SchemaError {
    /// The stable diagnostic code from SPEC §12, for example `"E0050"`.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            SchemaError::DuplicateTypeName { .. } => "E0050",
            SchemaError::UnresolvedType { .. }
            | SchemaError::MisplacedLazy { .. }
            | SchemaError::MisplacedUnit { .. } => "E0001",
            SchemaError::MisplacedResult { .. } | SchemaError::MisplacedStream { .. } => "E0005",
            SchemaError::InvalidMapKey { .. } => "E0006",
            SchemaError::StoreWithoutConstructor { .. } => "E0011",
            SchemaError::ObjectNamedAsValue { .. }
            | SchemaError::NotAnObject { .. }
            | SchemaError::MisplacedObject { .. } => "E0064",
            SchemaError::MisplacedCallback { .. } | SchemaError::NotACallback { .. } => "E0004",
            SchemaError::BadCallbackMethod { .. } => "E0071",
            SchemaError::BadTransparentRecord { .. } => "E0007",
            SchemaError::BadInfiniteQuery { .. } => "E0073",
        }
    }
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = self.code();
        let (what, why, fix) = match self {
            SchemaError::DuplicateTypeName {
                name,
                first,
                duplicate,
            } => (
                format!(
                    "duplicate type name `{name}` (declared as {first} and again as {duplicate})"
                ),
                "type names must be unique within a core: a type is identified to the platforms by its name (its id is a hash of it), so two declarations cannot share one".to_owned(),
                format!("rename one of the two `{name}` declarations, or remove the duplicate"),
            ),
            SchemaError::UnresolvedType { name, at } => (
                format!("unknown type `{name}` referenced at {at}"),
                "the schema describes a type by name, so the name must be declared in the same core: a record or enum with `#[undra::api]`, an error with `#[undra::error]`".to_owned(),
                format!("declare `{name}` with `#[undra::api]`, or fix the name where it is used"),
            ),
            SchemaError::MisplacedResult { ty, at } => (
                format!("`{ty}` at {at}: Result is only allowed as the outermost type of a return"),
                "`Result<T, E>` is how a method reports a typed error, so it has no meaning inside a field, a parameter or another type".to_owned(),
                "return the `Result` from the method and keep plain values in fields, parameters and nested types".to_owned(),
            ),
            SchemaError::MisplacedStream { ty, at } => (
                format!("`{ty}` at {at}: Stream is only allowed as a return type, alone or as the Ok side of a Result"),
                "a stream is how a method returns many values over time, so it has no meaning anywhere but a method's return".to_owned(),
                "return the stream from the method".to_owned(),
            ),
            SchemaError::MisplacedLazy { ty, at } => (
                format!("`{ty}` at {at}: Lazy is only allowed as the type of a store signal"),
                "a lazy list is a list the platform pages through on demand, which only a store signal can offer".to_owned(),
                "use a `Vec<T>` here, or move the list to a signal of a store".to_owned(),
            ),
            SchemaError::MisplacedUnit { at } => (
                format!("`unit` at {at}: Unit is only allowed as a return type or as a variant with no fields, not as a field, parameter or signal type, nor inside option, vec, map or lazy"),
                "`()` occupies zero bytes on the wire, and zero-width items defeat length validation: a `Vec` of them would accept any count from a four-byte message".to_owned(),
                "remove the value, or use `bool` if you need a marker".to_owned(),
            ),
            SchemaError::InvalidMapKey { key, at } => (
                format!("`{key}` at {at} is not a valid map key"),
                "map keys must compare and hash the same on every platform: `String`, an integer type, `bool`, `Uuid` and a newtype of one of those do, floats, decimals and composite keys do not".to_owned(),
                "use one of those key types, or a `Vec` of records with an explicit key field".to_owned(),
            ),
            SchemaError::StoreWithoutConstructor { object } => (
                format!("store `{object}` has no constructor"),
                "the platforms create a store by calling one of its constructors; without one it can never be instantiated".to_owned(),
                "add `pub fn new(ctx: Ctx) -> Self` to its `#[undra::api(store)]` impl block".to_owned(),
            ),
            SchemaError::ObjectNamedAsValue { name, at } => (
                format!("`{name}` at {at} is an object, named as a value"),
                "an object crosses the boundary as a handle and is named `object` in the schema; `named` is for records, enums and errors, and the one place it names an object is the return type of the object's own constructors".to_owned(),
                format!("take or return `Arc<{name}>` (or `&{name}` as a parameter), which the schema records as an `object` reference"),
            ),
            SchemaError::NotAnObject { name, found, at } => (
                format!("`{name}` at {at} is referenced as an object, but it is a {found}"),
                "an `object` reference names a type with an `#[undra::api] impl` block; records, enums and errors cross by value".to_owned(),
                format!("write `{name}` without `Arc` or `&`, or give it an `#[undra::api] impl` block if it is meant to be an object"),
            ),
            SchemaError::MisplacedObject { ty, at } => (
                format!("`{}` at {at}: an object is only allowed as a method or function parameter or return, alone or as `Option` or `Vec` of one", rust_spelling(ty)),
                "a record field, a signal, a map, a stream item, a port, a query or a mutation holds values: they are copied, compared, hashed and persisted, and none of that can carry a reference to an object".to_owned(),
                "return a record with the data you need, or put the child behind a method of the parent".to_owned(),
            ),
            SchemaError::MisplacedCallback { ty, at } => (
                format!("`{}` at {at}: a callback interface is only allowed as a parameter of a method, constructor or function, alone or as `Option`", rust_spelling(ty)),
                "a callback is an instance the host passes in for the core to call back; it has no value to copy, compare or store, and the core never hands one out".to_owned(),
                "take the callback as a parameter, or pass a record or an id where a value is needed".to_owned(),
            ),
            SchemaError::NotACallback { name, at } => (
                format!("`{name}` at {at} is referenced as a callback interface, but it is not one"),
                "a `callback` reference names a trait declared with `#[undra::callback]`".to_owned(),
                format!("declare `{name}` with `#[undra::callback]`, or fix the name"),
            ),
            SchemaError::BadCallbackMethod { port, method, problem } => (
                format!("method `{method}` of the callback interface `{port}` {problem}"),
                "the host runs a callback outside the core's thread and lock, so the core can never wait for it synchronously, and a host implementation can always fail or be gone: a method either reports (fire-and-forget, returning nothing) or is `async` and returns a `Result`".to_owned(),
                "make a reporting method return `()`, or make a method that answers `async` and return `Result<T, E>`; do not start a name with `__`".to_owned(),
            ),
            SchemaError::BadTransparentRecord { name, problem } => (
                format!("the newtype `{name}` {problem}"),
                "a newtype crosses the boundary as its one inner value, so the schema describes it as a transparent record with a single field called `value`".to_owned(),
                "give the record exactly one field named `value`, without `#[undra(default)]`, or make it an ordinary record".to_owned(),
            ),
            SchemaError::BadInfiniteQuery { query, problem } => (
                format!("the infinite query `{query}` {problem}"),
                "an infinite query's handle shows its pages as one keyed list of `T`, so the schema needs `T` to be a record with the `item_key` field, and a cursor the core can store".to_owned(),
                "return `Page<T, C>` where `T` is an `#[undra::api]` record, and set `item_key` to one of its fields".to_owned(),
            ),
        };
        f.write_str(&crate::diag::message(code, what, why, fix))
    }
}

impl std::error::Error for SchemaError {}

/// Which return-only or signal-only forms are legal at a position.
#[derive(Clone, Copy)]
struct Allow {
    result: bool,
    stream: bool,
    lazy: bool,
    unit: bool,
    /// An `Object`: `NO`, `HERE_OR_WRAPPED` (the node, or `Option`/`Vec` of it) or `HERE`.
    object: Slot,
    /// A `Callback`: `NO`, `HERE_OR_WRAPPED` (the node, or `Option` of it) or `HERE`.
    callback: Slot,
}

/// Where an object or a callback reference may stand.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// Nowhere here.
    No,
    /// At this node, or directly inside an `Option` (or, for an object, a `Vec`) at this node.
    Wrapped,
    /// At this node only.
    Here,
}

/// Fields and parameters: no wrapper and no `Unit`.
const PLAIN: Allow = Allow {
    result: false,
    stream: false,
    lazy: false,
    unit: false,
    object: Slot::No,
    callback: Slot::No,
};
/// A parameter of a method, constructor or function: values, plus an object (alone, `Option` or
/// `Vec` of one) or a callback (alone or `Option`).
const PARAM: Allow = Allow {
    object: Slot::Wrapped,
    callback: Slot::Wrapped,
    ..PLAIN
};
/// Return types: `T`, `Result<T,E>`, `Stream<T>`, `Result<Stream<T>,E>`, where
/// `Unit` may stand for the whole return type.
const RETURN: Allow = Allow {
    result: true,
    stream: true,
    lazy: false,
    unit: true,
    object: Slot::No,
    callback: Slot::No,
};
/// The return of a method or function: as [`RETURN`], and an object (alone, `Option` or `Vec` of
/// one, on the `Ok` side of a `Result` too).
const RETURN_OBJECTS: Allow = Allow {
    object: Slot::Wrapped,
    ..RETURN
};
/// A store signal's type: `Lazy<T>` is legal at the top.
const SIGNAL: Allow = Allow {
    result: false,
    stream: false,
    lazy: true,
    unit: false,
    object: Slot::No,
    callback: Slot::No,
};
/// The components of a `Result` or `Stream`: `Unit` is fine there (`Result<(),
/// E>`, a stream of ticks). Also used for a map key so that a `Unit` key is
/// reported once, as an invalid key, instead of twice.
const COMPONENT: Allow = Allow {
    result: false,
    stream: false,
    lazy: false,
    unit: true,
    object: Slot::No,
    callback: Slot::No,
};

struct Checker<'a> {
    known: HashSet<&'a str>,
    /// What each declared type name is.
    kinds: HashMap<&'a str, TypeKind>,
    /// The ports of kind `Callback`.
    callbacks: HashSet<&'a str>,
    /// While a constructor's return type is checked: the object it constructs, which `Named` may
    /// still spell there.
    constructs: Option<&'a str>,
    /// The transparent records (newtypes, ADR-042) and the type each wraps, for map keys.
    transparent: HashMap<&'a str, &'a TypeRef>,
    errors: Vec<SchemaError>,
}

impl<'a> Checker<'a> {
    fn check(&mut self, ty: &TypeRef, allow: Allow, at: &dyn Fn() -> String) {
        match ty {
            TypeRef::Named(name) => {
                if !self.known.contains(name.as_str()) {
                    self.errors.push(SchemaError::UnresolvedType {
                        name: name.clone(),
                        at: at(),
                    });
                } else if self.kinds.get(name.as_str()) == Some(&TypeKind::Object)
                    && self.constructs != Some(name.as_str())
                {
                    self.errors.push(SchemaError::ObjectNamedAsValue {
                        name: name.clone(),
                        at: at(),
                    });
                }
            }
            TypeRef::Object(name) => {
                if allow.object == Slot::No {
                    self.errors.push(SchemaError::MisplacedObject {
                        ty: ty.clone(),
                        at: at(),
                    });
                }
                match self.kinds.get(name.as_str()) {
                    None => self.errors.push(SchemaError::UnresolvedType {
                        name: name.clone(),
                        at: at(),
                    }),
                    Some(TypeKind::Object) => {}
                    Some(&found) => self.errors.push(SchemaError::NotAnObject {
                        name: name.clone(),
                        found,
                        at: at(),
                    }),
                }
            }
            TypeRef::Callback(name) => {
                if allow.callback == Slot::No {
                    self.errors.push(SchemaError::MisplacedCallback {
                        ty: ty.clone(),
                        at: at(),
                    });
                }
                if !self.callbacks.contains(name.as_str()) {
                    self.errors.push(SchemaError::NotACallback {
                        name: name.clone(),
                        at: at(),
                    });
                }
            }
            TypeRef::Option(inner) => {
                let inner_allow = Allow {
                    object: if allow.object == Slot::Wrapped {
                        Slot::Here
                    } else {
                        Slot::No
                    },
                    callback: if allow.callback == Slot::Wrapped {
                        Slot::Here
                    } else {
                        Slot::No
                    },
                    ..PLAIN
                };
                self.check(inner, inner_allow, at);
            }
            TypeRef::Vec(inner) => {
                let inner_allow = Allow {
                    object: if allow.object == Slot::Wrapped {
                        Slot::Here
                    } else {
                        Slot::No
                    },
                    ..PLAIN
                };
                self.check(inner, inner_allow, at);
            }
            TypeRef::Map(key, value) => {
                if !self.is_valid_key(key, 0) {
                    self.errors.push(SchemaError::InvalidMapKey {
                        key: (**key).clone(),
                        at: at(),
                    });
                }
                self.check(key, COMPONENT, at);
                self.check(value, PLAIN, at);
            }
            TypeRef::Lazy(item) => {
                if !allow.lazy {
                    self.errors.push(SchemaError::MisplacedLazy {
                        ty: ty.clone(),
                        at: at(),
                    });
                }
                self.check(item, PLAIN, at);
            }
            TypeRef::Result(ok, err) => {
                if !allow.result {
                    self.errors.push(SchemaError::MisplacedResult {
                        ty: ty.clone(),
                        at: at(),
                    });
                }
                // `Stream` is only legal directly inside a legal `Result`.
                // `Unit` is legal in both components; when the `Result`
                // itself is misplaced that is already reported, so its
                // components do not add a second error.
                let ok_allow = if allow.result {
                    Allow {
                        stream: allow.stream,
                        object: allow.object,
                        ..COMPONENT
                    }
                } else {
                    COMPONENT
                };
                self.check(ok, ok_allow, at);
                self.check(err, COMPONENT, at);
            }
            TypeRef::Stream(item) => {
                if !allow.stream {
                    self.errors.push(SchemaError::MisplacedStream {
                        ty: ty.clone(),
                        at: at(),
                    });
                }
                self.check(item, COMPONENT, at);
            }
            TypeRef::Unit => {
                if !allow.unit {
                    self.errors.push(SchemaError::MisplacedUnit { at: at() });
                }
            }
            TypeRef::Bool
            | TypeRef::I8
            | TypeRef::I16
            | TypeRef::I32
            | TypeRef::I64
            | TypeRef::U8
            | TypeRef::U16
            | TypeRef::U32
            | TypeRef::U64
            | TypeRef::F32
            | TypeRef::F64
            | TypeRef::String
            | TypeRef::Bytes
            | TypeRef::Duration
            | TypeRef::Timestamp
            | TypeRef::Uuid
            | TypeRef::Decimal => {}
        }
    }

    /// [`Schema::is_valid_map_key`] with the transparent records resolved.
    fn is_valid_key(&self, ty: &TypeRef, depth: u32) -> bool {
        match ty {
            TypeRef::Named(name) if depth < 16 => self
                .transparent
                .get(name.as_str())
                .is_some_and(|inner| self.is_valid_key(inner, depth + 1)),
            other => other.is_valid_map_key(),
        }
    }

    fn check_params(&mut self, params: &[crate::ParamDef], owner: &str, allow: Allow) {
        for param in params {
            self.check(&param.ty, allow, &|| {
                format!("{owner}, param {}", param.name)
            });
        }
    }

    fn check_return(&mut self, returns: &TypeRef, owner: &str, allow: Allow) {
        self.check(returns, allow, &|| format!("{owner}, return type"));
    }

    /// A method of an object: parameters may be objects or callbacks, a return may be an object.
    fn check_method(&mut self, method: &crate::MethodDef, owner: &str, what: &str) {
        let owner = format!("{owner}, {what} {}", method.name);
        self.check_params(&method.params, &owner, PARAM);
        self.check_return(&method.returns, &owner, RETURN_OBJECTS);
    }

    /// A constructor: as a method, except that it returns its own object as `Named`.
    fn check_constructor(&mut self, method: &crate::MethodDef, object: &'a str, owner: &str) {
        let owner = format!("{owner}, constructor {}", method.name);
        self.check_params(&method.params, &owner, PARAM);
        self.constructs = Some(object);
        self.check_return(&method.returns, &owner, RETURN);
        self.constructs = None;
    }

    /// A method of a port: values only. A callback interface's methods are held to its shape.
    fn check_port_method(&mut self, method: &crate::MethodDef, owner: &str) {
        let owner = format!("{owner}, method {}", method.name);
        self.check_params(&method.params, &owner, PLAIN);
        self.check_return(&method.returns, &owner, RETURN);
    }
}

impl Schema {
    /// Checks the schema against the rules of SPEC §2.1 and §12.
    ///
    /// * type names are unique across records, enums, errors and objects
    ///   (E0050);
    /// * every `Named` reference resolves to a record, enum or object
    ///   (E0001);
    /// * `Result` and `Stream` appear only in return position, in the shapes
    ///   `T`, `Result<T,E>`, `Stream<T>`, `Result<Stream<T>,E>` (E0005);
    /// * map keys are `String`, integers, `Bool` or `Uuid` (E0006);
    /// * `Lazy` appears only as the type of a store signal (E0001);
    /// * `Unit` appears only as a return type (alone, or as a component of a
    ///   returned `Result` or `Stream`), never as a record, variant or
    ///   parameter type, a signal type, or inside `Option`, `Vec`, map values
    ///   or `Lazy` (E0001);
    /// * every store has at least one constructor (E0011);
    /// * an `Object` names an object and a `Named` does not, except in the return type of its own
    ///   constructors; objects stand only as method and function parameters and returns, alone or
    ///   as `Option` or `Vec` (E0064, ADR-040);
    /// * a `Callback` names a port of kind `Callback`, stands only as a parameter of a method,
    ///   constructor or function, alone or as `Option`, and a callback port's methods are
    ///   fire-and-forget or `async` with a `Result` (E0004, E0071, ADR-041).
    ///
    /// It does not check that ids match their names or that they are free of
    /// hash collisions; `undra-bindgen` owns collision detection.
    ///
    /// # Errors
    ///
    /// Returns every violation found, in schema order.
    ///
    /// ```
    /// use undra_meta::{FieldDef, RecordDef, Schema, SchemaError, TypeRef};
    ///
    /// let mut schema = Schema::new("demo");
    /// schema.records.push(RecordDef {
    ///     name: "Todo".into(),
    ///     type_id: 1,
    ///     fields: vec![FieldDef {
    ///         name: "owner".into(),
    ///         ty: TypeRef::named("User"),
    ///         default: false,
    ///         docs: String::new(),
    ///     }],
    ///     transparent: false, docs: String::new(),
    /// });
    /// let errors = schema.validate().unwrap_err();
    /// assert!(matches!(&errors[0], SchemaError::UnresolvedType { name, .. } if name == "User"));
    /// ```
    pub fn validate(&self) -> Result<(), Vec<SchemaError>> {
        let mut errors = Vec::new();

        // Unique type names across records, enums (incl. errors) and objects.
        let mut seen: HashMap<&str, TypeKind> = HashMap::new();
        for record in &self.records {
            note_type(&mut seen, &mut errors, &record.name, TypeKind::Record);
        }
        for en in &self.enums {
            let kind = if en.is_error {
                TypeKind::Error
            } else {
                TypeKind::Enum
            };
            note_type(&mut seen, &mut errors, &en.name, kind);
        }
        for object in &self.objects {
            note_type(&mut seen, &mut errors, &object.name, TypeKind::Object);
        }

        let mut checker = Checker {
            known: seen.keys().copied().collect(),
            callbacks: self
                .ports
                .iter()
                .filter(|port| port.kind == crate::PortKind::Callback)
                .map(|port| port.name.as_str())
                .collect(),
            kinds: seen,
            constructs: None,
            transparent: self
                .records
                .iter()
                .filter(|r| r.transparent)
                .filter_map(|r| r.fields.first().map(|f| (r.name.as_str(), &f.ty)))
                .collect(),
            errors,
        };

        for record in &self.records {
            if record.transparent {
                let problem = match record.fields.as_slice() {
                    [only] if only.name != "value" => {
                        Some("has a field that is not called `value`")
                    }
                    [only] if only.default => Some("has `#[undra(default)]` on its field"),
                    [_] => None,
                    _ => Some("does not have exactly one field"),
                };
                if let Some(problem) = problem {
                    checker.errors.push(SchemaError::BadTransparentRecord {
                        name: record.name.clone(),
                        problem,
                    });
                }
            }
            for field in &record.fields {
                checker.check(&field.ty, PLAIN, &|| {
                    format!("record {}, field {}", record.name, field.name)
                });
            }
        }

        for en in &self.enums {
            for variant in &en.variants {
                for field in &variant.fields {
                    checker.check(&field.ty, PLAIN, &|| {
                        format!(
                            "enum {}, variant {}, field {}",
                            en.name, variant.name, field.name
                        )
                    });
                }
            }
        }

        for object in &self.objects {
            let owner = format!("object {}", object.name);
            for ctor in &object.constructors {
                checker.check_constructor(ctor, &object.name, &owner);
            }
            for method in &object.methods {
                checker.check_method(method, &owner, "method");
            }
            if let Some(store) = &object.store {
                for signal in &store.signals {
                    checker.check(&signal.ty, SIGNAL, &|| {
                        format!("store {}, signal {}", object.name, signal.name)
                    });
                }
                if object.constructors.is_empty() {
                    checker.errors.push(SchemaError::StoreWithoutConstructor {
                        object: object.name.clone(),
                    });
                }
            }
        }

        for function in &self.functions {
            let owner = format!("function {}", function.name);
            checker.check_params(&function.params, &owner, PARAM);
            checker.check_return(&function.returns, &owner, RETURN_OBJECTS);
        }

        for port in &self.ports {
            let owner = format!("port {}", port.name);
            for method in &port.methods {
                checker.check_port_method(method, &owner);
                if port.kind == crate::PortKind::Callback {
                    if let Some(problem) = callback_method_problem(method) {
                        checker.errors.push(SchemaError::BadCallbackMethod {
                            port: port.name.clone(),
                            method: method.name.clone(),
                            problem,
                        });
                    }
                }
            }
        }

        for query in &self.queries {
            let owner = match query.kind {
                crate::QueryKind::Query => format!("query {}", query.name),
                crate::QueryKind::Mutation => format!("mutation {}", query.name),
            };
            checker.check_params(&query.params, &owner, PLAIN);
            checker.check_return(&query.returns, &owner, RETURN);
            if let Some(infinite) = &query.infinite {
                checker.check(&infinite.cursor, PLAIN, &|| format!("{owner}, cursor"));
                if let Some(problem) = self.infinite_problem(query, infinite) {
                    checker.errors.push(SchemaError::BadInfiniteQuery {
                        query: query.name.clone(),
                        problem,
                    });
                }
            }
        }

        if checker.errors.is_empty() {
            Ok(())
        } else {
            Err(checker.errors)
        }
    }
}

impl Schema {
    /// Whether `ty` may be a map key in this schema: [`TypeRef::is_valid_map_key`], and a
    /// newtype (a transparent record, ADR-042) of a valid key, however deeply nested.
    ///
    /// ```
    /// use undra_meta::{FieldDef, RecordDef, Schema, TypeRef};
    ///
    /// let mut schema = Schema::new("demo");
    /// schema.records.push(RecordDef {
    ///     name: "UserId".into(),
    ///     type_id: 1,
    ///     fields: vec![FieldDef { name: "value".into(), ty: TypeRef::Uuid, default: false, docs: String::new() }],
    ///     transparent: true,
    ///     docs: String::new(),
    /// });
    /// assert!(schema.is_valid_map_key(&TypeRef::named("UserId")));
    /// assert!(!TypeRef::named("UserId").is_valid_map_key());
    /// assert!(!schema.is_valid_map_key(&TypeRef::Decimal));
    /// ```
    #[must_use]
    pub fn is_valid_map_key(&self, ty: &TypeRef) -> bool {
        let mut ty = ty;
        for _ in 0..16 {
            match ty {
                TypeRef::Named(name) => {
                    match self
                        .records
                        .iter()
                        .find(|r| r.transparent && &r.name == name)
                        .and_then(|r| r.fields.first())
                    {
                        Some(field) => ty = &field.ty,
                        None => return false,
                    }
                }
                other => return other.is_valid_map_key(),
            }
        }
        false
    }

    /// What is wrong with the shape of an `infinite` query (ADR-043), if anything.
    fn infinite_problem(
        &self,
        query: &crate::QueryDef,
        infinite: &crate::InfiniteDef,
    ) -> Option<String> {
        if query.kind != crate::QueryKind::Query {
            return Some("is a mutation: only a query can be infinite".to_owned());
        }
        // `Vec<T>` or, like every query, `Result<Vec<T>, E>`.
        let returned = match &query.returns {
            TypeRef::Result(ok, _) => &**ok,
            other => other,
        };
        let TypeRef::Vec(item) = returned else {
            return Some("does not return `Vec<T>`, the list its handle shows".to_owned());
        };
        let TypeRef::Named(item) = &**item else {
            return Some("returns a list of something that is not a record".to_owned());
        };
        let Some(record) = self.records.iter().find(|r| &r.name == item) else {
            return Some(format!("returns a list of `{item}`, which is not a record"));
        };
        if !record.fields.iter().any(|f| f.name == infinite.item_key) {
            return Some(format!(
                "has `item_key = \"{}\"`, but `{item}` has no such field",
                infinite.item_key
            ));
        }
        None
    }
}

/// A type as the Rust that declared it spells it, for the messages about objects and callbacks:
/// `object:Child` is `Arc<Child>`, `vec<object:Child>` is `Vec<Arc<Child>>`.
fn rust_spelling(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Object(name) => format!("Arc<{name}>"),
        TypeRef::Callback(name) => format!("Arc<dyn {name}>"),
        TypeRef::Option(inner) => format!("Option<{}>", rust_spelling(inner)),
        TypeRef::Vec(inner) => format!("Vec<{}>", rust_spelling(inner)),
        TypeRef::Lazy(inner) => format!("Lazy<{}>", rust_spelling(inner)),
        TypeRef::Stream(inner) => format!("impl Stream<Item = {}>", rust_spelling(inner)),
        TypeRef::Map(k, v) => format!("Map<{}, {}>", rust_spelling(k), rust_spelling(v)),
        TypeRef::Result(ok, err) => {
            format!("Result<{}, {}>", rust_spelling(ok), rust_spelling(err))
        }
        other => other.to_string(),
    }
}

/// What is wrong with a method of a callback port (ADR-041), if anything: a callback method
/// reports (returns `()` and is not `async`) or is `async` and returns a `Result`.
fn callback_method_problem(method: &crate::MethodDef) -> Option<&'static str> {
    if method.name.starts_with("__") {
        return Some(
            "has a name that starts with `__`, which is reserved for `__release` and `__cancel`",
        );
    }
    match (&method.returns, method.is_async) {
        (TypeRef::Unit, false) => None,
        (TypeRef::Result(..), true) => None,
        (TypeRef::Unit, true) => {
            Some("is `async` and returns nothing: an async method must return a `Result`")
        }
        (_, true) => Some("is `async` but does not return a `Result<T, E>`"),
        (_, false) => {
            Some("returns a value synchronously: make it `async` and return a `Result<T, E>`")
        }
    }
}

/// Records `name` as declared, reporting a duplicate if it already was.
fn note_type<'a>(
    seen: &mut HashMap<&'a str, TypeKind>,
    errors: &mut Vec<SchemaError>,
    name: &'a str,
    kind: TypeKind,
) {
    match seen.get(name) {
        Some(&first) => errors.push(SchemaError::DuplicateTypeName {
            name: name.to_owned(),
            first,
            duplicate: kind,
        }),
        None => {
            seen.insert(name, kind);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{field, method, object, param, record, representative_schema};
    use crate::{
        EnumDef, FunctionDef, PortDef, PortKind, QueryDef, QueryKind, SignalDef, StoreDef,
        VariantDef, ids,
    };

    fn known() -> TypeRef {
        TypeRef::named("Known")
    }

    /// A schema with one valid record `Known` plus whatever `f` adds.
    fn base(f: impl FnOnce(&mut Schema)) -> Schema {
        let mut s = Schema::new("t");
        s.records
            .push(record("Known", vec![field("x", TypeRef::U8)]));
        f(&mut s);
        s
    }

    fn errors(s: &Schema) -> Vec<SchemaError> {
        s.validate().err().unwrap_or_default()
    }

    fn field_errors(ty: TypeRef) -> Vec<SchemaError> {
        errors(&base(|s| {
            s.records.push(record("Holder", vec![field("f", ty)]));
        }))
    }

    fn param_errors(ty: TypeRef) -> Vec<SchemaError> {
        errors(&base(|s| {
            s.functions
                .push(function(vec![param("p", ty)], TypeRef::Unit));
        }))
    }

    fn return_errors(ty: TypeRef) -> Vec<SchemaError> {
        errors(&base(|s| s.functions.push(function(vec![], ty))))
    }

    fn signal_errors(ty: TypeRef) -> Vec<SchemaError> {
        errors(&base(|s| {
            let mut store = object(
                "Store",
                vec![method(
                    "Store",
                    "new",
                    vec![],
                    TypeRef::named("Store"),
                    false,
                )],
                vec![],
            );
            store.store = Some(StoreDef {
                signals: vec![SignalDef {
                    name: "sig".into(),
                    signal_id: 0,
                    ty,
                    computed: false,
                    key: None,
                    no_coalesce: false,
                    default: false,
                }],
            });
            s.objects.push(store);
        }))
    }

    fn function(params: Vec<crate::ParamDef>, returns: TypeRef) -> FunctionDef {
        FunctionDef {
            name: "f".into(),
            method_id: ids::function_id("f"),
            params,
            returns,
            is_async: false,
            takes_ctx: false,
            docs: String::new(),
        }
    }

    #[test]
    fn valid_schemas_pass() {
        assert_eq!(Schema::new("empty").validate(), Ok(()));
        assert_eq!(representative_schema().validate(), Ok(()));
    }

    #[test]
    fn duplicate_type_names_are_e0050() {
        let s = base(|s| {
            s.records.push(record("Known", vec![]));
        });
        assert_eq!(
            errors(&s),
            [SchemaError::DuplicateTypeName {
                name: "Known".into(),
                first: TypeKind::Record,
                duplicate: TypeKind::Record,
            }]
        );
        assert_eq!(errors(&s)[0].code(), "E0050");
    }

    #[test]
    fn duplicates_are_detected_across_records_enums_errors_and_objects() {
        let s = base(|s| {
            s.enums.push(EnumDef {
                name: "Known".into(),
                type_id: 1,
                is_error: false,
                variants: vec![],
                docs: String::new(),
            });
            s.enums.push(EnumDef {
                name: "Known".into(),
                type_id: 1,
                is_error: true,
                variants: vec![],
                docs: String::new(),
            });
            s.objects.push(object("Known", vec![], vec![]));
        });
        let errs = errors(&s);
        assert_eq!(
            errs,
            [
                SchemaError::DuplicateTypeName {
                    name: "Known".into(),
                    first: TypeKind::Record,
                    duplicate: TypeKind::Enum
                },
                SchemaError::DuplicateTypeName {
                    name: "Known".into(),
                    first: TypeKind::Record,
                    duplicate: TypeKind::Error
                },
                SchemaError::DuplicateTypeName {
                    name: "Known".into(),
                    first: TypeKind::Record,
                    duplicate: TypeKind::Object
                },
            ]
        );
    }

    #[test]
    fn same_name_in_different_namespaces_is_fine() {
        // Functions, ports and queries are not types.
        let s = base(|s| {
            s.functions.push(FunctionDef {
                name: "Known".into(),
                ..function(vec![], TypeRef::Unit)
            });
            s.ports.push(PortDef {
                name: "Known".into(),
                port_id: 1,
                kind: PortKind::Sync,
                background: false,
                methods: vec![],
                docs: String::new(),
            });
        });
        assert_eq!(s.validate(), Ok(()));
    }

    #[test]
    fn named_references_resolve_to_records_enums_and_errors_but_not_objects() {
        for name in ["Todo", "Priority", "TodoError"] {
            let mut s = representative_schema();
            s.records
                .push(record("Holder", vec![field("f", TypeRef::named(name))]));
            assert_eq!(s.validate(), Ok(()), "{name}");
        }
        // An object is `Object`, not `Named` (ADR-040), and not a field either way.
        let mut s = representative_schema();
        s.records.push(record(
            "Holder",
            vec![field("f", TypeRef::named("Calculator"))],
        ));
        assert!(matches!(
            &s.validate().unwrap_err()[..],
            [SchemaError::ObjectNamedAsValue { name, .. }] if name == "Calculator"
        ));
    }

    #[test]
    fn unresolved_named_types_are_reported_with_location() {
        let errs = field_errors(TypeRef::option(TypeRef::vec(TypeRef::named("Ghost"))));
        assert_eq!(
            errs,
            [SchemaError::UnresolvedType {
                name: "Ghost".into(),
                at: "record Holder, field f".into()
            }]
        );
        assert!(errs[0].to_string().contains("Ghost"));
        assert!(errs[0].to_string().contains("record Holder, field f"));
    }

    #[test]
    fn unresolved_types_are_found_in_every_position() {
        let ghost = || TypeRef::named("Ghost");
        let s = base(|s| {
            s.enums.push(EnumDef {
                name: "E".into(),
                type_id: 1,
                is_error: false,
                variants: vec![VariantDef {
                    fields: vec![field("v", ghost())],
                    ..crate::fixtures::unit_variant("V", 0)
                }],
                docs: String::new(),
            });
            let mut obj = object(
                "Obj",
                vec![method(
                    "Obj",
                    "new",
                    vec![param("c", ghost())],
                    ghost(),
                    false,
                )],
                vec![method(
                    "Obj",
                    "m",
                    vec![param("p", ghost())],
                    ghost(),
                    false,
                )],
            );
            obj.store = Some(StoreDef {
                signals: vec![SignalDef {
                    name: "sig".into(),
                    signal_id: 0,
                    ty: ghost(),
                    computed: false,
                    key: None,
                    no_coalesce: false,
                    default: false,
                }],
            });
            s.objects.push(obj);
            s.functions
                .push(function(vec![param("p", ghost())], ghost()));
            s.ports.push(PortDef {
                name: "P".into(),
                port_id: 1,
                kind: PortKind::Sync,
                background: false,
                methods: vec![method("P", "m", vec![param("p", ghost())], ghost(), false)],
                docs: String::new(),
            });
            s.queries.push(QueryDef {
                name: "q".into(),
                query_id: 1,
                kind: QueryKind::Mutation,
                key: "k".into(),
                params: vec![param("p", ghost())],
                returns: ghost(),
                stale_ms: None,
                persist: false,
                idempotent: false,
                interval_ms: None,
                poll_in_background: false,
                infinite: None,
            });
        });
        let places: Vec<String> = errors(&s)
            .into_iter()
            .map(|e| match e {
                SchemaError::UnresolvedType { at, .. } => at,
                other => panic!("unexpected {other}"),
            })
            .collect();
        assert_eq!(
            places,
            [
                "enum E, variant V, field v",
                "object Obj, constructor new, param c",
                "object Obj, constructor new, return type",
                "object Obj, method m, param p",
                "object Obj, method m, return type",
                "store Obj, signal sig",
                "function f, param p",
                "function f, return type",
                "port P, method m, param p",
                "port P, method m, return type",
                "mutation q, param p",
                "mutation q, return type",
            ]
        );
    }

    #[test]
    fn legal_return_shapes_pass() {
        for ty in [
            TypeRef::Unit,
            TypeRef::String,
            known(),
            TypeRef::result(TypeRef::Unit, known()),
            TypeRef::stream(known()),
            TypeRef::result(TypeRef::stream(known()), known()),
            TypeRef::result(TypeRef::vec(known()), TypeRef::String),
        ] {
            assert_eq!(return_errors(ty.clone()), [], "{ty}");
        }
    }

    #[test]
    fn result_and_stream_are_rejected_outside_return_position() {
        let result = TypeRef::result(TypeRef::Unit, TypeRef::String);
        let stream = TypeRef::stream(TypeRef::U8);
        for (ty, is_result) in [(result, true), (stream, false)] {
            for errs in [
                field_errors(ty.clone()),
                param_errors(ty.clone()),
                signal_errors(ty.clone()),
                field_errors(TypeRef::vec(ty.clone())),
                return_errors(TypeRef::option(ty.clone())),
                return_errors(TypeRef::vec(ty.clone())),
                return_errors(TypeRef::map(TypeRef::String, ty.clone())),
            ] {
                assert_eq!(errs.len(), 1, "{ty}: {errs:?}");
                assert_eq!(errs[0].code(), "E0005");
                match (&errs[0], is_result) {
                    (SchemaError::MisplacedResult { ty: t, .. }, true)
                    | (SchemaError::MisplacedStream { ty: t, .. }, false) => assert_eq!(*t, ty),
                    (other, _) => panic!("unexpected {other}"),
                }
            }
        }
    }

    #[test]
    fn nested_result_and_stream_in_returns_are_rejected() {
        let unit = || TypeRef::Unit;
        // Result<Result<..>, E>
        let nested_ok = TypeRef::result(TypeRef::result(unit(), TypeRef::String), TypeRef::String);
        assert!(matches!(
            return_errors(nested_ok)[..],
            [SchemaError::MisplacedResult { .. }]
        ));
        // Result<T, Result<..>> and Result<T, Stream<..>>
        let nested_err = TypeRef::result(unit(), TypeRef::result(unit(), unit()));
        assert!(matches!(
            return_errors(nested_err)[..],
            [SchemaError::MisplacedResult { .. }]
        ));
        let stream_err = TypeRef::result(unit(), TypeRef::stream(unit()));
        assert!(matches!(
            return_errors(stream_err)[..],
            [SchemaError::MisplacedStream { .. }]
        ));
        // Stream<Stream<..>> and Stream<Result<..>>
        let stream_stream = TypeRef::stream(TypeRef::stream(unit()));
        assert!(matches!(
            return_errors(stream_stream)[..],
            [SchemaError::MisplacedStream { .. }]
        ));
        let stream_result = TypeRef::stream(TypeRef::result(unit(), unit()));
        assert!(matches!(
            return_errors(stream_result)[..],
            [SchemaError::MisplacedResult { .. }]
        ));
        // Result<Vec<Stream<..>>, E>
        let vec_stream = TypeRef::result(TypeRef::vec(TypeRef::stream(unit())), unit());
        assert!(matches!(
            return_errors(vec_stream)[..],
            [SchemaError::MisplacedStream { .. }]
        ));
    }

    #[test]
    fn map_keys_are_checked() {
        for key in [
            TypeRef::String,
            TypeRef::Bool,
            TypeRef::Uuid,
            TypeRef::I8,
            TypeRef::I16,
            TypeRef::I32,
            TypeRef::I64,
            TypeRef::U8,
            TypeRef::U16,
            TypeRef::U32,
            TypeRef::U64,
        ] {
            assert_eq!(
                field_errors(TypeRef::map(key.clone(), known())),
                [],
                "{key}"
            );
        }
        for key in [
            TypeRef::F32,
            TypeRef::F64,
            TypeRef::Bytes,
            TypeRef::Unit,
            TypeRef::Duration,
            TypeRef::Timestamp,
            TypeRef::option(TypeRef::String),
            TypeRef::vec(TypeRef::U8),
            TypeRef::map(TypeRef::String, TypeRef::String),
            known(),
        ] {
            let errs = field_errors(TypeRef::map(key.clone(), TypeRef::U8));
            assert_eq!(
                errs,
                [SchemaError::InvalidMapKey {
                    key: key.clone(),
                    at: "record Holder, field f".into()
                }],
                "{key}"
            );
            assert_eq!(errs[0].code(), "E0006");
        }
    }

    #[test]
    fn map_keys_are_checked_when_nested_and_in_every_position() {
        let bad = TypeRef::map(TypeRef::F64, TypeRef::U8);
        for errs in [
            field_errors(TypeRef::option(TypeRef::vec(bad.clone()))),
            param_errors(bad.clone()),
            return_errors(TypeRef::result(bad.clone(), TypeRef::Unit)),
            signal_errors(bad.clone()),
            field_errors(TypeRef::map(TypeRef::String, bad.clone())),
        ] {
            assert_eq!(errs.len(), 1, "{errs:?}");
            assert!(matches!(errs[0], SchemaError::InvalidMapKey { .. }));
        }
    }

    #[test]
    fn lazy_is_only_allowed_as_a_store_signal() {
        assert_eq!(signal_errors(TypeRef::lazy(known())), []);
        let lazy = TypeRef::lazy(known());
        for errs in [
            field_errors(lazy.clone()),
            param_errors(lazy.clone()),
            return_errors(lazy.clone()),
            return_errors(TypeRef::result(lazy.clone(), TypeRef::Unit)),
            signal_errors(TypeRef::option(lazy.clone())),
            signal_errors(TypeRef::vec(lazy.clone())),
            signal_errors(TypeRef::lazy(lazy.clone())),
        ] {
            assert_eq!(errs.len(), 1, "{errs:?}");
            assert!(matches!(errs[0], SchemaError::MisplacedLazy { .. }));
            assert_eq!(errs[0].code(), "E0001");
        }
    }

    #[test]
    fn lazy_signals_must_not_hide_return_only_types() {
        let errs = signal_errors(TypeRef::lazy(TypeRef::stream(TypeRef::U8)));
        assert!(matches!(errs[..], [SchemaError::MisplacedStream { .. }]));
    }

    #[test]
    fn stores_need_a_constructor() {
        let s = base(|s| {
            let mut store = object("Store", vec![], vec![]);
            store.store = Some(StoreDef { signals: vec![] });
            s.objects.push(store);
        });
        let errs = errors(&s);
        assert_eq!(
            errs,
            [SchemaError::StoreWithoutConstructor {
                object: "Store".into()
            }]
        );
        assert_eq!(errs[0].code(), "E0011");
        assert!(errs[0].to_string().contains("Store"));

        // A plain object without constructors is fine.
        let ok = base(|s| s.objects.push(object("Plain", vec![], vec![])));
        assert_eq!(ok.validate(), Ok(()));
    }

    // ----- objects and callbacks as parameters and returns (ADR-040, ADR-041) ---------------

    fn with_child(f: impl FnOnce(&mut Schema)) -> Schema {
        base(|s| {
            s.objects.push(object("Child", vec![], vec![]));
            f(s);
        })
    }

    fn parent(methods: Vec<crate::MethodDef>) -> crate::ObjectDef {
        object("Parent", vec![], methods)
    }

    #[test]
    fn objects_stand_as_method_parameters_and_returns_alone_or_in_option_and_vec() {
        let child = || TypeRef::object("Child");
        let schema = with_child(|s| {
            s.objects.push(parent(vec![
                method("Parent", "get", vec![], child(), false),
                method("Parent", "maybe", vec![], TypeRef::option(child()), false),
                method("Parent", "all", vec![], TypeRef::vec(child()), false),
                method(
                    "Parent",
                    "open",
                    vec![param("id", TypeRef::U32)],
                    TypeRef::result(child(), TypeRef::named("Known")),
                    true,
                ),
                method(
                    "Parent",
                    "open_all",
                    vec![],
                    TypeRef::result(TypeRef::vec(child()), TypeRef::named("Known")),
                    true,
                ),
                method(
                    "Parent",
                    "take",
                    vec![
                        param("a", child()),
                        param("b", TypeRef::option(child())),
                        param("c", TypeRef::vec(child())),
                    ],
                    TypeRef::Unit,
                    false,
                ),
            ]));
            s.functions.push(crate::FunctionDef {
                name: "child_of".into(),
                method_id: 1,
                params: vec![param("c", child())],
                returns: child(),
                is_async: false,
                takes_ctx: false,
                docs: String::new(),
            });
        });
        assert_eq!(schema.validate(), Ok(()));
    }

    #[test]
    fn objects_are_refused_where_a_value_is_copied() {
        let child = || TypeRef::object("Child");
        for ty in [
            TypeRef::option(TypeRef::option(child())),
            TypeRef::vec(TypeRef::vec(child())),
            TypeRef::vec(TypeRef::option(child())),
            TypeRef::map(TypeRef::String, child()),
            TypeRef::lazy(child()),
        ] {
            let errs = errors(&with_child(|s| {
                s.objects.push(parent(vec![method(
                    "Parent",
                    "m",
                    vec![param("p", ty.clone())],
                    TypeRef::Unit,
                    false,
                )]));
            }));
            assert!(
                errs.iter()
                    .any(|e| matches!(e, SchemaError::MisplacedObject { .. })),
                "{ty}: {errs:?}"
            );
        }
        // A record field, a variant field, a signal, a stream item, an error, a port and a query.
        let in_record = errors(&with_child(|s| {
            s.records.push(record("Holder", vec![field("c", child())]));
        }));
        assert!(
            matches!(&in_record[..], [SchemaError::MisplacedObject { at, .. }] if at == "record Holder, field c")
        );
        let in_signal = errors(&with_child(|s| {
            let mut store = object(
                "Store",
                vec![method(
                    "Store",
                    "new",
                    vec![],
                    TypeRef::named("Store"),
                    false,
                )],
                vec![],
            );
            store.store = Some(StoreDef {
                signals: vec![SignalDef {
                    name: "s".into(),
                    signal_id: 0,
                    ty: TypeRef::vec(child()),
                    computed: false,
                    key: None,
                    no_coalesce: false,
                    default: false,
                }],
            });
            s.objects.push(store);
        }));
        assert!(
            matches!(&in_signal[..], [SchemaError::MisplacedObject { .. }]),
            "{in_signal:?}"
        );
        let in_stream = errors(&with_child(|s| {
            s.objects.push(parent(vec![method(
                "Parent",
                "m",
                vec![],
                TypeRef::stream(child()),
                false,
            )]));
        }));
        assert!(
            matches!(&in_stream[..], [SchemaError::MisplacedObject { .. }]),
            "{in_stream:?}"
        );
        let in_error_side = errors(&with_child(|s| {
            s.objects.push(parent(vec![method(
                "Parent",
                "m",
                vec![],
                TypeRef::result(TypeRef::Unit, child()),
                false,
            )]));
        }));
        assert!(
            matches!(&in_error_side[..], [SchemaError::MisplacedObject { .. }]),
            "{in_error_side:?}"
        );
        let in_query = errors(&with_child(|s| {
            s.queries.push(QueryDef {
                name: "q".into(),
                query_id: 1,
                kind: QueryKind::Query,
                key: "q".into(),
                params: vec![param("c", child())],
                returns: child(),
                stale_ms: None,
                persist: false,
                idempotent: false,
                interval_ms: None,
                poll_in_background: false,
                infinite: None,
            });
        }));
        assert_eq!(in_query.len(), 2, "{in_query:?}");
        assert!(
            in_query
                .iter()
                .all(|e| matches!(e, SchemaError::MisplacedObject { .. }))
        );
        let in_port = errors(&with_child(|s| {
            s.ports.push(PortDef {
                name: "P".into(),
                port_id: 1,
                kind: PortKind::Async,
                background: false,
                methods: vec![method(
                    "P",
                    "m",
                    vec![param("c", child())],
                    TypeRef::Unit,
                    false,
                )],
                docs: String::new(),
            });
        }));
        assert!(
            matches!(&in_port[..], [SchemaError::MisplacedObject { .. }]),
            "{in_port:?}"
        );
        assert_eq!(in_port[0].code(), "E0064");
    }

    #[test]
    fn object_references_must_name_objects_and_named_must_not() {
        let unknown = errors(&base(|s| {
            s.objects.push(parent(vec![method(
                "Parent",
                "m",
                vec![],
                TypeRef::object("Ghost"),
                false,
            )]));
        }));
        assert!(
            matches!(&unknown[..], [SchemaError::UnresolvedType { name, .. }] if name == "Ghost")
        );
        let record = errors(&base(|s| {
            s.objects.push(parent(vec![method(
                "Parent",
                "m",
                vec![],
                TypeRef::object("Known"),
                false,
            )]));
        }));
        assert!(
            matches!(&record[..], [SchemaError::NotAnObject { name, found: TypeKind::Record, .. }] if name == "Known"),
            "{record:?}"
        );
        assert!(record[0].to_string().contains("E0064"));
        // `Named` of an object is refused as a value, but a constructor returns its own object so.
        let named = errors(&with_child(|s| {
            s.objects.push(parent(vec![method(
                "Parent",
                "m",
                vec![],
                TypeRef::named("Child"),
                false,
            )]));
        }));
        assert!(
            matches!(&named[..], [SchemaError::ObjectNamedAsValue { name, .. }] if name == "Child")
        );
        let ctor_ok = with_child(|s| {
            s.objects.push(object(
                "Parent",
                vec![method(
                    "Parent",
                    "new",
                    vec![],
                    TypeRef::named("Parent"),
                    false,
                )],
                vec![],
            ));
        });
        assert_eq!(ctor_ok.validate(), Ok(()));
        // ... but not another object's.
        let ctor_other = errors(&with_child(|s| {
            s.objects.push(object(
                "Parent",
                vec![method(
                    "Parent",
                    "new",
                    vec![],
                    TypeRef::named("Child"),
                    false,
                )],
                vec![],
            ));
        }));
        assert!(matches!(
            &ctor_other[..],
            [SchemaError::ObjectNamedAsValue { .. }]
        ));
    }

    fn callback_port(methods: Vec<crate::MethodDef>) -> PortDef {
        PortDef {
            name: "Listener".into(),
            port_id: ids::port_id("Listener"),
            kind: PortKind::Callback,
            background: false,
            methods,
            docs: String::new(),
        }
    }

    #[test]
    fn callbacks_stand_as_parameters_alone_or_optional() {
        let cb = || TypeRef::callback("Listener");
        let schema = base(|s| {
            s.ports.push(callback_port(vec![
                method(
                    "Listener",
                    "progress",
                    vec![param("n", TypeRef::U64)],
                    TypeRef::Unit,
                    false,
                ),
                method(
                    "Listener",
                    "confirm",
                    vec![param("name", TypeRef::String)],
                    TypeRef::result(TypeRef::Bool, TypeRef::named("Known")),
                    true,
                ),
            ]));
            s.objects.push(parent(vec![method(
                "Parent",
                "watch",
                vec![param("a", cb()), param("b", TypeRef::option(cb()))],
                TypeRef::Unit,
                false,
            )]));
            s.objects.push(object(
                "Other",
                vec![method(
                    "Other",
                    "new",
                    vec![param("l", cb())],
                    TypeRef::named("Other"),
                    false,
                )],
                vec![],
            ));
        });
        assert_eq!(schema.validate(), Ok(()));
    }

    #[test]
    fn callbacks_are_refused_elsewhere_and_must_name_a_callback_port() {
        let cb = || TypeRef::callback("Listener");
        for ty in [
            TypeRef::vec(cb()),
            TypeRef::map(TypeRef::String, cb()),
            TypeRef::option(TypeRef::option(cb())),
        ] {
            let errs = errors(&base(|s| {
                s.ports.push(callback_port(vec![]));
                s.objects.push(parent(vec![method(
                    "Parent",
                    "m",
                    vec![param("p", ty.clone())],
                    TypeRef::Unit,
                    false,
                )]));
            }));
            assert!(
                errs.iter()
                    .any(|e| matches!(e, SchemaError::MisplacedCallback { .. })),
                "{ty}: {errs:?}"
            );
        }
        let as_return = errors(&base(|s| {
            s.ports.push(callback_port(vec![]));
            s.objects
                .push(parent(vec![method("Parent", "m", vec![], cb(), false)]));
        }));
        assert!(
            matches!(&as_return[..], [SchemaError::MisplacedCallback { .. }]),
            "{as_return:?}"
        );
        assert_eq!(as_return[0].code(), "E0004");
        let as_field = errors(&base(|s| {
            s.ports.push(callback_port(vec![]));
            s.records.push(record("Holder", vec![field("l", cb())]));
        }));
        assert!(matches!(
            &as_field[..],
            [SchemaError::MisplacedCallback { .. }]
        ));
        // A plain port is not a callback.
        let not_callback = errors(&base(|s| {
            s.ports.push(PortDef {
                kind: PortKind::Async,
                ..callback_port(vec![])
            });
            s.objects.push(parent(vec![method(
                "Parent",
                "m",
                vec![param("p", cb())],
                TypeRef::Unit,
                false,
            )]));
        }));
        assert!(
            matches!(&not_callback[..], [SchemaError::NotACallback { name, .. }] if name == "Listener")
        );
        // A callback method cannot take a callback or an object.
        let nested = errors(&with_child(|s| {
            s.ports.push(callback_port(vec![method(
                "Listener",
                "m",
                vec![param("c", TypeRef::object("Child"))],
                TypeRef::Unit,
                false,
            )]));
        }));
        assert!(
            matches!(&nested[..], [SchemaError::MisplacedObject { .. }]),
            "{nested:?}"
        );
    }

    #[test]
    fn callback_methods_report_or_are_async_with_a_result() {
        let bad = |m: crate::MethodDef| errors(&base(|s| s.ports.push(callback_port(vec![m]))));
        for (m, needle) in [
            (
                method("Listener", "get", vec![], TypeRef::U32, false),
                "returns a value synchronously",
            ),
            (
                method(
                    "Listener",
                    "get",
                    vec![],
                    TypeRef::Result(Box::new(TypeRef::U32), Box::new(TypeRef::named("Known"))),
                    false,
                ),
                "returns a value synchronously",
            ),
            (
                method("Listener", "go", vec![], TypeRef::Unit, true),
                "returns nothing",
            ),
            (
                method("Listener", "go", vec![], TypeRef::U32, true),
                "does not return a `Result",
            ),
            (
                method("Listener", "__release", vec![], TypeRef::Unit, false),
                "reserved",
            ),
        ] {
            let errs = bad(m);
            assert!(
                matches!(&errs[..], [SchemaError::BadCallbackMethod { .. }]),
                "{needle}: {errs:?}"
            );
            assert_eq!(errs[0].code(), "E0071");
            assert!(
                errs[0].to_string().contains(needle),
                "{needle}: {}",
                errs[0]
            );
        }
    }

    fn is_misplaced_unit(errs: &[SchemaError]) -> bool {
        matches!(errs, [SchemaError::MisplacedUnit { .. }])
    }

    fn variant_field_errors(ty: TypeRef) -> Vec<SchemaError> {
        errors(&base(|s| {
            s.enums.push(EnumDef {
                name: "E".into(),
                type_id: 1,
                is_error: false,
                variants: vec![VariantDef {
                    fields: vec![field("v", ty)],
                    ..crate::fixtures::unit_variant("V", 0)
                }],
                docs: String::new(),
            });
        }))
    }

    fn method_param_errors(ty: TypeRef) -> Vec<SchemaError> {
        errors(&base(|s| {
            s.objects.push(object(
                "Obj",
                vec![],
                vec![method(
                    "Obj",
                    "m",
                    vec![param("p", ty)],
                    TypeRef::Unit,
                    false,
                )],
            ));
        }))
    }

    #[test]
    fn unit_is_legal_as_a_return_type_and_as_a_fieldless_variant() {
        // Whole return type, Ok/Err components, stream items.
        for ty in [
            TypeRef::Unit,
            TypeRef::result(TypeRef::Unit, known()),
            TypeRef::result(TypeRef::Unit, TypeRef::Unit),
            TypeRef::result(known(), TypeRef::Unit),
            TypeRef::stream(TypeRef::Unit),
            TypeRef::result(TypeRef::stream(TypeRef::Unit), known()),
        ] {
            assert_eq!(return_errors(ty.clone()), [], "return {ty}");
        }
        // Methods, constructors and port methods may return Unit too, and a
        // variant with no fields is the way to spell a unit variant.
        let s = base(|s| {
            s.objects.push(object(
                "Obj",
                vec![],
                vec![method("Obj", "m", vec![], TypeRef::Unit, false)],
            ));
            s.enums.push(EnumDef {
                name: "E".into(),
                type_id: 1,
                is_error: false,
                variants: vec![crate::fixtures::unit_variant("V", 0)],
                docs: String::new(),
            });
        });
        assert_eq!(s.validate(), Ok(()));
    }

    #[test]
    fn unit_is_rejected_as_a_record_field() {
        let errs = field_errors(TypeRef::Unit);
        assert_eq!(
            errs,
            [SchemaError::MisplacedUnit {
                at: "record Holder, field f".into()
            }]
        );
        assert_eq!(errs[0].code(), "E0001");
    }

    #[test]
    fn unit_is_rejected_as_a_variant_field() {
        let errs = variant_field_errors(TypeRef::Unit);
        assert_eq!(
            errs,
            [SchemaError::MisplacedUnit {
                at: "enum E, variant V, field v".into()
            }]
        );
    }

    #[test]
    fn unit_is_rejected_as_a_parameter() {
        assert_eq!(
            param_errors(TypeRef::Unit),
            [SchemaError::MisplacedUnit {
                at: "function f, param p".into()
            }]
        );
        assert_eq!(
            method_param_errors(TypeRef::Unit),
            [SchemaError::MisplacedUnit {
                at: "object Obj, method m, param p".into()
            }]
        );
    }

    #[test]
    fn unit_is_rejected_as_a_signal_type() {
        assert_eq!(
            signal_errors(TypeRef::Unit),
            [SchemaError::MisplacedUnit {
                at: "store Store, signal sig".into()
            }]
        );
    }

    #[test]
    fn unit_is_rejected_inside_option_vec_map_and_lazy() {
        let unit = || TypeRef::Unit;
        for ty in [
            TypeRef::option(unit()),
            TypeRef::vec(unit()),
            TypeRef::map(TypeRef::String, unit()),
            TypeRef::option(TypeRef::vec(unit())),
            TypeRef::vec(TypeRef::option(unit())),
            TypeRef::map(TypeRef::String, TypeRef::vec(unit())),
        ] {
            assert!(is_misplaced_unit(&field_errors(ty.clone())), "field {ty}");
            assert!(is_misplaced_unit(&param_errors(ty.clone())), "param {ty}");
            assert!(is_misplaced_unit(&signal_errors(ty.clone())), "signal {ty}");
            assert!(
                is_misplaced_unit(&variant_field_errors(ty.clone())),
                "variant {ty}"
            );
            // Being in return position does not legalise them either.
            assert!(is_misplaced_unit(&return_errors(ty.clone())), "return {ty}");
            assert!(
                is_misplaced_unit(&return_errors(TypeRef::result(ty.clone(), known()))),
                "result ok {ty}"
            );
        }
        // Lazy<Unit> is only reachable as a signal; the item Unit is rejected.
        assert!(is_misplaced_unit(&signal_errors(TypeRef::lazy(unit()))));
    }

    #[test]
    fn unit_inside_a_wrapper_in_a_return_is_rejected_but_the_wrapper_itself_is_fine() {
        // Stream<Vec<Unit>>, Result<Vec<Unit>, E>: legal shapes, illegal item.
        assert!(is_misplaced_unit(&return_errors(TypeRef::stream(
            TypeRef::vec(TypeRef::Unit)
        ))));
        assert!(is_misplaced_unit(&return_errors(TypeRef::result(
            TypeRef::stream(TypeRef::option(TypeRef::Unit)),
            known()
        ))));
    }

    #[test]
    fn a_unit_map_key_is_reported_once_as_an_invalid_key() {
        let errs = field_errors(TypeRef::map(TypeRef::Unit, TypeRef::U8));
        assert_eq!(
            errs,
            [SchemaError::InvalidMapKey {
                key: TypeRef::Unit,
                at: "record Holder, field f".into()
            }]
        );
    }

    #[test]
    fn a_misplaced_wrapper_around_unit_is_reported_once() {
        // The wrapper is the problem; its Unit component is not a second one.
        for errs in [
            field_errors(TypeRef::result(TypeRef::Unit, TypeRef::Unit)),
            field_errors(TypeRef::stream(TypeRef::Unit)),
            param_errors(TypeRef::stream(TypeRef::Unit)),
        ] {
            assert_eq!(errs.len(), 1, "{errs:?}");
            assert_eq!(errs[0].code(), "E0005");
        }
    }

    #[test]
    fn misplaced_unit_message_names_the_rule() {
        let text = field_errors(TypeRef::vec(TypeRef::Unit))[0].to_string();
        assert!(text.starts_with("error[undra::E0001]:"), "{text}");
        assert!(text.contains("record Holder, field f"), "{text}");
        assert!(
            text.contains("only allowed as a return type or as a variant with no fields"),
            "{text}"
        );
        assert!(text.contains("option, vec, map or lazy"), "{text}");
    }

    #[test]
    fn all_errors_are_reported_in_schema_order() {
        let s = base(|s| {
            s.records.push(record("Known", vec![]));
            s.records.push(record(
                "Holder",
                vec![
                    field("a", TypeRef::named("Ghost")),
                    field("b", TypeRef::map(TypeRef::F32, TypeRef::U8)),
                    field("c", TypeRef::stream(TypeRef::Unit)),
                ],
            ));
            s.functions
                .push(function(vec![], TypeRef::lazy(TypeRef::U8)));
        });
        let codes: Vec<&str> = errors(&s).iter().map(SchemaError::code).collect();
        assert_eq!(codes, ["E0050", "E0001", "E0006", "E0005", "E0001"]);
    }

    #[test]
    fn display_carries_code_and_details() {
        let cases = [
            (
                SchemaError::DuplicateTypeName {
                    name: "Todo".into(),
                    first: TypeKind::Record,
                    duplicate: TypeKind::Error,
                },
                ["E0050", "`Todo`", "record", "error"],
            ),
            (
                SchemaError::UnresolvedType {
                    name: "Ghost".into(),
                    at: "here".into(),
                },
                ["E0001", "`Ghost`", "here", "#[undra::api]"],
            ),
            (
                SchemaError::MisplacedResult {
                    ty: TypeRef::result(TypeRef::Unit, TypeRef::Unit),
                    at: "here".into(),
                },
                ["E0005", "result<unit,unit>", "here", "Result"],
            ),
            (
                SchemaError::MisplacedStream {
                    ty: TypeRef::stream(TypeRef::U8),
                    at: "here".into(),
                },
                ["E0005", "stream<u8>", "here", "Stream"],
            ),
            (
                SchemaError::MisplacedLazy {
                    ty: TypeRef::lazy(TypeRef::U8),
                    at: "here".into(),
                },
                ["E0001", "lazy<u8>", "here", "Lazy"],
            ),
            (
                SchemaError::MisplacedUnit { at: "here".into() },
                ["E0001", "`unit`", "here", "return type"],
            ),
            (
                SchemaError::InvalidMapKey {
                    key: TypeRef::F64,
                    at: "here".into(),
                },
                ["E0006", "`f64`", "here", "map key"],
            ),
            (
                SchemaError::StoreWithoutConstructor { object: "S".into() },
                ["E0011", "`S`", "constructor", "store"],
            ),
        ];
        for (err, needles) in cases {
            let text = err.to_string();
            assert!(text.starts_with("error[undra::E00"), "{text}");
            for needle in needles {
                assert!(text.contains(needle), "{text:?} should contain {needle:?}");
            }
            // The shape of every diagnostic: what, note, help and the docs link of the code.
            let lines: Vec<&str> = text.lines().collect();
            assert_eq!(lines.len(), 4, "{text}");
            assert!(lines[1].starts_with("  = note: "), "{text}");
            assert!(lines[2].starts_with("  = help: "), "{text}");
            assert_eq!(
                lines[3],
                format!(
                    "  = docs: https://shreypdev.github.io/undra/docs/errors.html#{}",
                    err.code()
                ),
                "{text}"
            );
        }
    }

    #[test]
    fn schema_error_is_a_std_error() {
        fn takes_error<E: std::error::Error + Send + Sync + 'static>(_: E) {}
        takes_error(SchemaError::StoreWithoutConstructor { object: "S".into() });
    }

    #[test]
    fn type_kind_display() {
        assert_eq!(TypeKind::Record.to_string(), "record");
        assert_eq!(TypeKind::Enum.to_string(), "enum");
        assert_eq!(TypeKind::Error.to_string(), "error");
        assert_eq!(TypeKind::Object.to_string(), "object");
    }

    #[test]
    fn unicode_names_are_reported_verbatim() {
        let mut s = Schema::new("unicode");
        s.records.push(record(
            "T\u{00e9}l\u{00e9}",
            vec![field("f", TypeRef::named("\u{4e2d}"))],
        ));
        let errs = errors(&s);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].to_string().contains('\u{4e2d}'));
    }

    fn newtype(name: &str, inner: TypeRef) -> crate::RecordDef {
        let mut r = record(name, vec![field("value", inner)]);
        r.transparent = true;
        r
    }

    #[test]
    fn a_newtype_of_a_valid_key_is_a_valid_map_key_and_a_decimal_is_not() {
        let mut s = Schema::new("t");
        s.records.push(newtype("UserId", TypeRef::Uuid));
        s.records.push(newtype("OrderNo", TypeRef::named("UserId")));
        s.records.push(newtype("Price", TypeRef::Decimal));
        s.records.push(newtype("Ratio", TypeRef::F64));
        s.records
            .push(record("Plain", vec![field("value", TypeRef::U8)]));
        for ok in ["UserId", "OrderNo"] {
            assert!(s.is_valid_map_key(&TypeRef::named(ok)), "{ok}");
        }
        for bad in ["Price", "Ratio", "Plain", "Missing"] {
            assert!(!s.is_valid_map_key(&TypeRef::named(bad)), "{bad}");
        }
        assert!(s.is_valid_map_key(&TypeRef::String));
        assert!(!s.is_valid_map_key(&TypeRef::Decimal));
        s.records.push(record(
            "Holder",
            vec![
                field(
                    "by_user",
                    TypeRef::map(TypeRef::named("UserId"), TypeRef::Bool),
                ),
                field(
                    "by_order",
                    TypeRef::map(TypeRef::named("OrderNo"), TypeRef::Bool),
                ),
            ],
        ));
        assert_eq!(s.validate(), Ok(()));
        s.records.push(record(
            "Bad",
            vec![
                field(
                    "by_price",
                    TypeRef::map(TypeRef::named("Price"), TypeRef::Bool),
                ),
                field("by_decimal", TypeRef::map(TypeRef::Decimal, TypeRef::Bool)),
            ],
        ));
        let errs = s.validate().unwrap_err();
        assert_eq!(errs.len(), 2, "{errs:?}");
        assert!(errs.iter().all(|e| e.code() == "E0006"));
    }

    #[test]
    fn a_self_referential_newtype_is_not_a_key_and_does_not_loop() {
        let mut s = Schema::new("t");
        s.records
            .push(newtype("Loop", TypeRef::option(TypeRef::named("Loop"))));
        assert!(!s.is_valid_map_key(&TypeRef::named("Loop")));
        let mut cycle = Schema::new("t");
        cycle.records.push(newtype("A", TypeRef::named("B")));
        cycle.records.push(newtype("B", TypeRef::named("A")));
        assert!(!cycle.is_valid_map_key(&TypeRef::named("A")));
        cycle.records.push(record(
            "H",
            vec![field("m", TypeRef::map(TypeRef::named("A"), TypeRef::Bool))],
        ));
        assert!(cycle.validate().is_err());
    }

    #[test]
    fn a_transparent_record_has_one_field_called_value_without_default() {
        let mut ok = Schema::new("t");
        ok.records.push(newtype("UserId", TypeRef::Uuid));
        assert_eq!(ok.validate(), Ok(()));

        for (name, fields, expect) in [
            (
                "Two",
                vec![field("value", TypeRef::U8), field("more", TypeRef::U8)],
                "exactly one field",
            ),
            ("None", vec![], "exactly one field"),
            (
                "Renamed",
                vec![field("inner", TypeRef::U8)],
                "not called `value`",
            ),
        ] {
            let mut s = Schema::new("t");
            let mut r = record(name, fields);
            r.transparent = true;
            s.records.push(r);
            let errs = s.validate().unwrap_err();
            assert_eq!(errs.len(), 1, "{name}: {errs:?}");
            assert_eq!(errs[0].code(), "E0007");
            assert!(errs[0].to_string().contains(expect), "{}", errs[0]);
        }
        let mut s = Schema::new("t");
        let mut r = record("Defaulted", vec![field("value", TypeRef::U8)]);
        r.fields[0].default = true;
        r.transparent = true;
        s.records.push(r);
        assert_eq!(s.validate().unwrap_err()[0].code(), "E0007");
    }

    #[test]
    fn a_newtype_cannot_wrap_a_unit_an_object_or_a_lazy_list() {
        for inner in [
            TypeRef::Unit,
            TypeRef::lazy(TypeRef::U8),
            TypeRef::object("Thing"),
        ] {
            let mut s = Schema::new("t");
            s.objects.push(object("Thing", vec![], vec![]));
            s.records.push(newtype("Bad", inner.clone()));
            let errs = s.validate().unwrap_err();
            assert!(!errs.is_empty(), "{inner}");
        }
        let mut s = Schema::new("t");
        s.records.push(newtype(
            "Many",
            TypeRef::vec(TypeRef::option(TypeRef::Decimal)),
        ));
        assert_eq!(s.validate(), Ok(()));
    }

    fn infinite_query(returns: TypeRef, item_key: &str) -> Schema {
        let mut s = Schema::new("t");
        s.records
            .push(record("Post", vec![field("id", TypeRef::U64)]));
        s.queries.push(QueryDef {
            name: "feed".into(),
            query_id: ids::query_id("feed"),
            kind: QueryKind::Query,
            key: "feed".into(),
            params: vec![],
            returns,
            stale_ms: None,
            persist: false,
            idempotent: false,
            interval_ms: None,
            poll_in_background: false,
            infinite: Some(crate::InfiniteDef {
                cursor: TypeRef::String,
                item_key: item_key.into(),
            }),
        });
        s
    }

    #[test]
    fn an_infinite_query_returns_a_list_of_records_with_the_item_key() {
        let good = infinite_query(TypeRef::vec(TypeRef::named("Post")), "id");
        assert_eq!(good.validate(), Ok(()));
        // A query reports a typed error like every other: `Result<Vec<T>, E>` is the usual shape.
        let mut with_error = infinite_query(
            TypeRef::result(TypeRef::vec(TypeRef::named("Post")), TypeRef::String),
            "id",
        );
        assert_eq!(with_error.validate(), Ok(()));
        with_error.queries[0].infinite.as_mut().unwrap().item_key = "slug".into();
        assert_eq!(with_error.validate().unwrap_err()[0].code(), "E0073");
        for (returns, key, expect) in [
            (TypeRef::named("Post"), "id", "does not return `Vec<T>`"),
            (TypeRef::vec(TypeRef::U8), "id", "not a record"),
            (
                TypeRef::vec(TypeRef::named("Post")),
                "slug",
                "no such field",
            ),
        ] {
            let errs = infinite_query(returns, key).validate().unwrap_err();
            assert_eq!(errs.len(), 1, "{errs:?}");
            assert_eq!(errs[0].code(), "E0073");
            assert!(errs[0].to_string().contains(expect), "{}", errs[0]);
        }
        let mut mutation = infinite_query(TypeRef::vec(TypeRef::named("Post")), "id");
        mutation.queries[0].kind = QueryKind::Mutation;
        assert_eq!(mutation.validate().unwrap_err()[0].code(), "E0073");
    }
}
