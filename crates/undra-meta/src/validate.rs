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
        }
    }
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = self.code();
        match self {
            SchemaError::DuplicateTypeName {
                name,
                first,
                duplicate,
            } => write!(
                f,
                "error[undra::{code}]: duplicate type name `{name}` (declared as {first} and again as {duplicate}); type names must be unique within a core"
            ),
            SchemaError::UnresolvedType { name, at } => write!(
                f,
                "error[undra::{code}]: unknown type `{name}` referenced at {at}; declare it with #[undra::api] or fix the name"
            ),
            SchemaError::MisplacedResult { ty, at } => write!(
                f,
                "error[undra::{code}]: `{ty}` at {at}: Result is only allowed as the outermost type of a return"
            ),
            SchemaError::MisplacedStream { ty, at } => write!(
                f,
                "error[undra::{code}]: `{ty}` at {at}: Stream is only allowed as a return type, alone or as the Ok side of a Result"
            ),
            SchemaError::MisplacedLazy { ty, at } => write!(
                f,
                "error[undra::{code}]: `{ty}` at {at}: Lazy is only allowed as the type of a store signal"
            ),
            SchemaError::MisplacedUnit { at } => write!(
                f,
                "error[undra::{code}]: `unit` at {at}: Unit is only allowed as a return type or as a variant with no fields, not as a field, parameter or signal type, nor inside option, vec, map or lazy (zero-width items defeat length validation)"
            ),
            SchemaError::InvalidMapKey { key, at } => write!(
                f,
                "error[undra::{code}]: `{key}` at {at} is not a valid map key; use String, an integer type, Bool or Uuid"
            ),
            SchemaError::StoreWithoutConstructor { object } => write!(
                f,
                "error[undra::{code}]: store `{object}` has no constructor; add a `pub fn new(..) -> Self` to its #[undra::api] impl"
            ),
        }
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
}

/// Fields and parameters: no wrapper and no `Unit`.
const PLAIN: Allow = Allow {
    result: false,
    stream: false,
    lazy: false,
    unit: false,
};
/// Return types: `T`, `Result<T,E>`, `Stream<T>`, `Result<Stream<T>,E>`, where
/// `Unit` may stand for the whole return type.
const RETURN: Allow = Allow {
    result: true,
    stream: true,
    lazy: false,
    unit: true,
};
/// A store signal's type: `Lazy<T>` is legal at the top.
const SIGNAL: Allow = Allow {
    result: false,
    stream: false,
    lazy: true,
    unit: false,
};
/// The components of a `Result` or `Stream`: `Unit` is fine there (`Result<(),
/// E>`, a stream of ticks). Also used for a map key so that a `Unit` key is
/// reported once, as an invalid key, instead of twice.
const COMPONENT: Allow = Allow {
    result: false,
    stream: false,
    lazy: false,
    unit: true,
};

struct Checker<'a> {
    known: HashSet<&'a str>,
    errors: Vec<SchemaError>,
}

impl Checker<'_> {
    fn check(&mut self, ty: &TypeRef, allow: Allow, at: &dyn Fn() -> String) {
        match ty {
            TypeRef::Named(name) => {
                if !self.known.contains(name.as_str()) {
                    self.errors.push(SchemaError::UnresolvedType {
                        name: name.clone(),
                        at: at(),
                    });
                }
            }
            TypeRef::Option(inner) | TypeRef::Vec(inner) => self.check(inner, PLAIN, at),
            TypeRef::Map(key, value) => {
                if !key.is_valid_map_key() {
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
            | TypeRef::Uuid => {}
        }
    }

    fn check_params(&mut self, params: &[crate::ParamDef], owner: &str) {
        for param in params {
            self.check(&param.ty, PLAIN, &|| {
                format!("{owner}, param {}", param.name)
            });
        }
    }

    fn check_return(&mut self, returns: &TypeRef, owner: &str) {
        self.check(returns, RETURN, &|| format!("{owner}, return type"));
    }

    fn check_method(&mut self, method: &crate::MethodDef, owner: &str, what: &str) {
        let owner = format!("{owner}, {what} {}", method.name);
        self.check_params(&method.params, &owner);
        self.check_return(&method.returns, &owner);
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
    /// * every store has at least one constructor (E0011).
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
    ///     docs: String::new(),
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
            errors,
        };

        for record in &self.records {
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
                checker.check_method(ctor, &owner, "constructor");
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
            checker.check_params(&function.params, &owner);
            checker.check_return(&function.returns, &owner);
        }

        for port in &self.ports {
            let owner = format!("port {}", port.name);
            for method in &port.methods {
                checker.check_method(method, &owner, "method");
            }
        }

        for query in &self.queries {
            let owner = match query.kind {
                crate::QueryKind::Query => format!("query {}", query.name),
                crate::QueryKind::Mutation => format!("mutation {}", query.name),
            };
            checker.check_params(&query.params, &owner);
            checker.check_return(&query.returns, &owner);
        }

        if checker.errors.is_empty() {
            Ok(())
        } else {
            Err(checker.errors)
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
                methods: vec![],
                docs: String::new(),
            });
        });
        assert_eq!(s.validate(), Ok(()));
    }

    #[test]
    fn named_references_resolve_to_records_enums_errors_and_objects() {
        for name in ["Todo", "Priority", "TodoError", "Calculator"] {
            let mut s = representative_schema();
            s.records
                .push(record("Holder", vec![field("f", TypeRef::named(name))]));
            assert_eq!(s.validate(), Ok(()), "{name}");
        }
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
                }],
            });
            s.objects.push(obj);
            s.functions
                .push(function(vec![param("p", ghost())], ghost()));
            s.ports.push(PortDef {
                name: "P".into(),
                port_id: 1,
                kind: PortKind::Sync,
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
}
