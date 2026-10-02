//! `'static`, const-constructible mirrors of the schema definitions
//! (SPEC §2.4, §4.6).
//!
//! The `*Meta` types have the same shape as their `*Def` counterparts in
//! [`crate::def`], but use `&'static str` and `&'static [T]` instead of
//! `String` and `Vec<T>`, so macro output can live in `static` items:
//!
//! ```
//! use undra_meta::{FieldMeta, RecordDef, RecordMeta, TypeRefMeta, ids};
//!
//! static TODO: RecordMeta = RecordMeta {
//!     name: "Todo",
//!     type_id: ids::type_id("Todo"),
//!     fields: &[
//!         FieldMeta { name: "id", ty: TypeRefMeta::Uuid, default: false, docs: "" },
//!         FieldMeta {
//!             name: "note",
//!             ty: TypeRefMeta::Option(&TypeRefMeta::String),
//!             default: true,
//!             docs: "Free text.",
//!         },
//!     ],
//!     docs: "A todo item.",
//! };
//!
//! let def = RecordDef::from(&TODO);
//! assert_eq!(def.fields[1].docs, "Free text.");
//! ```
//!
//! Every `*Meta` has a `From<&XMeta> for XDef` conversion. Only
//! [`ObjectMeta`] and [`FunctionMeta`] carry extra data, the `dispatch`
//! function pointer, which has no `*Def` counterpart (see
//! [`crate::dispatch`]). Because a function pointer cannot be meaningfully
//! compared, those two types do not implement `PartialEq`.

use crate::dispatch::DispatchFn;
use crate::{
    EnumDef, FieldDef, FunctionDef, MethodDef, ObjectDef, ParamDef, PortDef, PortKind, QueryDef,
    QueryKind, RecordDef, SignalDef, StoreDef, TypeRef, VariantDef,
};

/// Converts a slice of `*Meta` into a `Vec` of owned `*Def`.
fn convert_all<'a, M: 'a, D: From<&'a M>>(items: &'a [M]) -> Vec<D> {
    items.iter().map(D::from).collect()
}

/// A `'static`, const-constructible mirror of [`TypeRef`].
///
/// Nesting uses `&'static TypeRefMeta`, so a type can be written as a constant
/// expression, for example
/// `TypeRefMeta::Option(&TypeRefMeta::Vec(&TypeRefMeta::Named("Todo")))`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeRefMeta {
    /// See [`TypeRef::Bool`].
    Bool,
    /// See [`TypeRef::I8`].
    I8,
    /// See [`TypeRef::I16`].
    I16,
    /// See [`TypeRef::I32`].
    I32,
    /// See [`TypeRef::I64`].
    I64,
    /// See [`TypeRef::U8`].
    U8,
    /// See [`TypeRef::U16`].
    U16,
    /// See [`TypeRef::U32`].
    U32,
    /// See [`TypeRef::U64`].
    U64,
    /// See [`TypeRef::F32`].
    F32,
    /// See [`TypeRef::F64`].
    F64,
    /// See [`TypeRef::String`].
    String,
    /// See [`TypeRef::Bytes`].
    Bytes,
    /// See [`TypeRef::Unit`].
    Unit,
    /// See [`TypeRef::Duration`].
    Duration,
    /// See [`TypeRef::Timestamp`].
    Timestamp,
    /// See [`TypeRef::Uuid`].
    Uuid,
    /// See [`TypeRef::Option`].
    Option(&'static TypeRefMeta),
    /// See [`TypeRef::Vec`].
    Vec(&'static TypeRefMeta),
    /// See [`TypeRef::Map`]: key, then value.
    Map(&'static TypeRefMeta, &'static TypeRefMeta),
    /// See [`TypeRef::Lazy`].
    Lazy(&'static TypeRefMeta),
    /// See [`TypeRef::Named`].
    Named(&'static str),
    /// See [`TypeRef::Result`]: ok, then error.
    Result(&'static TypeRefMeta, &'static TypeRefMeta),
    /// See [`TypeRef::Stream`].
    Stream(&'static TypeRefMeta),
    /// See [`TypeRef::Object`].
    Object(&'static str),
    /// See [`TypeRef::Callback`].
    Callback(&'static str),
}

impl From<&TypeRefMeta> for TypeRef {
    fn from(m: &TypeRefMeta) -> TypeRef {
        match *m {
            TypeRefMeta::Bool => TypeRef::Bool,
            TypeRefMeta::I8 => TypeRef::I8,
            TypeRefMeta::I16 => TypeRef::I16,
            TypeRefMeta::I32 => TypeRef::I32,
            TypeRefMeta::I64 => TypeRef::I64,
            TypeRefMeta::U8 => TypeRef::U8,
            TypeRefMeta::U16 => TypeRef::U16,
            TypeRefMeta::U32 => TypeRef::U32,
            TypeRefMeta::U64 => TypeRef::U64,
            TypeRefMeta::F32 => TypeRef::F32,
            TypeRefMeta::F64 => TypeRef::F64,
            TypeRefMeta::String => TypeRef::String,
            TypeRefMeta::Bytes => TypeRef::Bytes,
            TypeRefMeta::Unit => TypeRef::Unit,
            TypeRefMeta::Duration => TypeRef::Duration,
            TypeRefMeta::Timestamp => TypeRef::Timestamp,
            TypeRefMeta::Uuid => TypeRef::Uuid,
            TypeRefMeta::Option(t) => TypeRef::option(t.into()),
            TypeRefMeta::Vec(t) => TypeRef::vec(t.into()),
            TypeRefMeta::Map(k, v) => TypeRef::map(k.into(), v.into()),
            TypeRefMeta::Lazy(t) => TypeRef::lazy(t.into()),
            TypeRefMeta::Named(n) => TypeRef::named(n),
            TypeRefMeta::Result(t, e) => TypeRef::result(t.into(), e.into()),
            TypeRefMeta::Stream(t) => TypeRef::stream(t.into()),
            TypeRefMeta::Object(n) => TypeRef::object(n),
            TypeRefMeta::Callback(n) => TypeRef::callback(n),
        }
    }
}

/// Mirror of [`RecordDef`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecordMeta {
    /// Type name.
    pub name: &'static str,
    /// `fnv1a32(name)`.
    pub type_id: u32,
    /// Fields in declaration order.
    pub fields: &'static [FieldMeta],
    /// Doc comment (empty if none).
    pub docs: &'static str,
}

impl From<&RecordMeta> for RecordDef {
    fn from(m: &RecordMeta) -> RecordDef {
        RecordDef {
            name: m.name.to_owned(),
            type_id: m.type_id,
            fields: convert_all(m.fields),
            docs: m.docs.to_owned(),
        }
    }
}

/// Mirror of [`FieldDef`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldMeta {
    /// Field name.
    pub name: &'static str,
    /// Field type.
    pub ty: TypeRefMeta,
    /// Whether the field carries `#[undra(default)]`.
    pub default: bool,
    /// Doc comment (empty if none).
    pub docs: &'static str,
}

impl From<&FieldMeta> for FieldDef {
    fn from(m: &FieldMeta) -> FieldDef {
        FieldDef {
            name: m.name.to_owned(),
            ty: (&m.ty).into(),
            default: m.default,
            docs: m.docs.to_owned(),
        }
    }
}

/// Mirror of [`EnumDef`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnumMeta {
    /// Type name.
    pub name: &'static str,
    /// `fnv1a32(name)`.
    pub type_id: u32,
    /// `true` for `#[undra::error]` enums.
    pub is_error: bool,
    /// Variants.
    pub variants: &'static [VariantMeta],
    /// Doc comment (empty if none).
    pub docs: &'static str,
}

impl From<&EnumMeta> for EnumDef {
    fn from(m: &EnumMeta) -> EnumDef {
        EnumDef {
            name: m.name.to_owned(),
            type_id: m.type_id,
            is_error: m.is_error,
            variants: convert_all(m.variants),
            docs: m.docs.to_owned(),
        }
    }
}

/// Mirror of [`VariantDef`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VariantMeta {
    /// Variant name.
    pub name: &'static str,
    /// Wire index.
    pub index: u16,
    /// Payload fields; empty means a unit variant.
    pub fields: &'static [FieldMeta],
    /// `true` for tuple variants.
    pub tuple: bool,
    /// The `#[error("...")]` message template, if any.
    pub message: Option<&'static str>,
    /// Doc comment (empty if none).
    pub docs: &'static str,
}

impl From<&VariantMeta> for VariantDef {
    fn from(m: &VariantMeta) -> VariantDef {
        VariantDef {
            name: m.name.to_owned(),
            index: m.index,
            fields: convert_all(m.fields),
            tuple: m.tuple,
            message: m.message.map(str::to_owned),
            docs: m.docs.to_owned(),
        }
    }
}

/// Mirror of [`ObjectDef`], plus the object's dispatcher.
///
/// `dispatch` is the macro-generated function the runtime calls to invoke
/// methods and constructors of this object; see [`crate::dispatch`] for why it
/// is expressed with erased types. It has no `*Def` counterpart and is dropped
/// by the `From` conversion.
#[derive(Clone, Copy, Debug)]
pub struct ObjectMeta {
    /// Type name.
    pub name: &'static str,
    /// `fnv1a32(name)`.
    pub type_id: u32,
    /// Constructors.
    pub constructors: &'static [MethodMeta],
    /// Methods.
    pub methods: &'static [MethodMeta],
    /// Signal table when the object is a store.
    pub store: Option<StoreMeta>,
    /// Doc comment (empty if none).
    pub docs: &'static str,
    /// The generated dispatcher for this object's methods and constructors.
    pub dispatch: DispatchFn,
}

impl From<&ObjectMeta> for ObjectDef {
    fn from(m: &ObjectMeta) -> ObjectDef {
        ObjectDef {
            name: m.name.to_owned(),
            type_id: m.type_id,
            constructors: convert_all(m.constructors),
            methods: convert_all(m.methods),
            store: m.store.as_ref().map(StoreDef::from),
            docs: m.docs.to_owned(),
        }
    }
}

/// Mirror of [`MethodDef`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MethodMeta {
    /// Method name.
    pub name: &'static str,
    /// `fnv1a32("<Type>.<name>")`.
    pub method_id: u32,
    /// Parameters, excluding `self` and `Ctx`.
    pub params: &'static [ParamMeta],
    /// Return type.
    pub returns: TypeRefMeta,
    /// Whether the method is `async`.
    pub is_async: bool,
    /// Whether the first Rust parameter is a `Ctx`.
    pub takes_ctx: bool,
    /// `#[undra(coalesce)]`, see [`MethodDef::coalesce`].
    pub coalesce: bool,
    /// Doc comment (empty if none).
    pub docs: &'static str,
}

impl From<&MethodMeta> for MethodDef {
    fn from(m: &MethodMeta) -> MethodDef {
        MethodDef {
            name: m.name.to_owned(),
            method_id: m.method_id,
            params: convert_all(m.params),
            returns: (&m.returns).into(),
            is_async: m.is_async,
            takes_ctx: m.takes_ctx,
            coalesce: m.coalesce,
            docs: m.docs.to_owned(),
        }
    }
}

/// Mirror of [`ParamDef`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamMeta {
    /// Parameter name.
    pub name: &'static str,
    /// Parameter type.
    pub ty: TypeRefMeta,
}

impl From<&ParamMeta> for ParamDef {
    fn from(m: &ParamMeta) -> ParamDef {
        ParamDef {
            name: m.name.to_owned(),
            ty: (&m.ty).into(),
        }
    }
}

/// Mirror of [`StoreDef`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StoreMeta {
    /// Signals in declaration order.
    pub signals: &'static [SignalMeta],
}

impl From<&StoreMeta> for StoreDef {
    fn from(m: &StoreMeta) -> StoreDef {
        StoreDef {
            signals: convert_all(m.signals),
        }
    }
}

/// Mirror of [`SignalDef`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SignalMeta {
    /// Field name.
    pub name: &'static str,
    /// Zero-based index among the store's signal fields.
    pub signal_id: u32,
    /// The signal's value type.
    pub ty: TypeRefMeta,
    /// `true` for `Computed<T>`.
    pub computed: bool,
    /// The `#[undra(key = "..")]` field, if any.
    pub key: Option<&'static str>,
    /// `#[undra(no_coalesce)]`.
    pub no_coalesce: bool,
    /// `#[undra(default)]` on a plain signal (ADR-037).
    pub default: bool,
}

impl From<&SignalMeta> for SignalDef {
    fn from(m: &SignalMeta) -> SignalDef {
        SignalDef {
            name: m.name.to_owned(),
            signal_id: m.signal_id,
            ty: (&m.ty).into(),
            computed: m.computed,
            key: m.key.map(str::to_owned),
            no_coalesce: m.no_coalesce,
            default: m.default,
        }
    }
}

/// Mirror of [`FunctionDef`], plus the function's dispatcher.
///
/// Like [`ObjectMeta`], it carries a `dispatch` pointer that is dropped by the
/// `From` conversion.
#[derive(Clone, Copy, Debug)]
pub struct FunctionMeta {
    /// Function name.
    pub name: &'static str,
    /// `fnv1a32("fn.<name>")`.
    pub method_id: u32,
    /// Parameters, excluding `Ctx`.
    pub params: &'static [ParamMeta],
    /// Return type.
    pub returns: TypeRefMeta,
    /// Whether the function is `async`.
    pub is_async: bool,
    /// Whether the first Rust parameter is a `Ctx`.
    pub takes_ctx: bool,
    /// Doc comment (empty if none).
    pub docs: &'static str,
    /// The generated dispatcher for this function.
    pub dispatch: DispatchFn,
}

impl From<&FunctionMeta> for FunctionDef {
    fn from(m: &FunctionMeta) -> FunctionDef {
        FunctionDef {
            name: m.name.to_owned(),
            method_id: m.method_id,
            params: convert_all(m.params),
            returns: (&m.returns).into(),
            is_async: m.is_async,
            takes_ctx: m.takes_ctx,
            docs: m.docs.to_owned(),
        }
    }
}

/// Mirror of [`PortDef`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PortMeta {
    /// Trait name.
    pub name: &'static str,
    /// `fnv1a32("port.<name>")`.
    pub port_id: u32,
    /// Sync, async, event or callback.
    pub kind: PortKind,
    /// `#[undra::callback(background)]`, see [`PortDef::background`].
    pub background: bool,
    /// Port methods.
    pub methods: &'static [MethodMeta],
    /// Doc comment (empty if none).
    pub docs: &'static str,
}

impl From<&PortMeta> for PortDef {
    fn from(m: &PortMeta) -> PortDef {
        PortDef {
            name: m.name.to_owned(),
            port_id: m.port_id,
            kind: m.kind,
            background: m.background,
            methods: convert_all(m.methods),
            docs: m.docs.to_owned(),
        }
    }
}

/// Mirror of [`QueryDef`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QueryMeta {
    /// Function name.
    pub name: &'static str,
    /// `fnv1a32("query.<name>")` or `fnv1a32("mutation.<name>")`.
    pub query_id: u32,
    /// Query or mutation.
    pub kind: QueryKind,
    /// Cache key template.
    pub key: &'static str,
    /// Parameters, excluding `Ctx`.
    pub params: &'static [ParamMeta],
    /// Return type.
    pub returns: TypeRefMeta,
    /// Staleness window in milliseconds.
    pub stale_ms: Option<u64>,
    /// Whether results are persisted.
    pub persist: bool,
    /// Whether the call is safe to replay.
    pub idempotent: bool,
}

impl From<&QueryMeta> for QueryDef {
    fn from(m: &QueryMeta) -> QueryDef {
        QueryDef {
            name: m.name.to_owned(),
            query_id: m.query_id,
            kind: m.kind,
            key: m.key.to_owned(),
            params: convert_all(m.params),
            returns: (&m.returns).into(),
            stale_ms: m.stale_ms,
            persist: m.persist,
            idempotent: m.idempotent,
        }
    }
}

#[cfg(test)]
mod tests {
    use core::any::Any;

    use super::*;
    use crate::dispatch::{DispatchCall, DispatchOutcome};
    use crate::ids;

    fn no_dispatch(_: &dyn Any, _: DispatchCall<'_>) -> DispatchOutcome {
        DispatchOutcome::new(())
    }

    // Everything below is a constant expression: this compiling is the test
    // that the mirrors are const-constructible with nested references.
    static NESTED: TypeRefMeta = TypeRefMeta::Result(
        &TypeRefMeta::Map(
            &TypeRefMeta::Uuid,
            &TypeRefMeta::Vec(&TypeRefMeta::Named("Todo")),
        ),
        &TypeRefMeta::Named("TodoError"),
    );

    static COUNTER: ObjectMeta = ObjectMeta {
        name: "Counter",
        type_id: ids::type_id("Counter"),
        constructors: &[MethodMeta {
            name: "new",
            method_id: ids::method_id("Counter", "new"),
            params: &[ParamMeta {
                name: "start",
                ty: TypeRefMeta::I64,
            }],
            returns: TypeRefMeta::Named("Counter"),
            is_async: false,
            takes_ctx: true,
            coalesce: false,
            docs: "Creates a counter.",
        }],
        methods: &[MethodMeta {
            name: "watch",
            method_id: ids::method_id("Counter", "watch"),
            params: &[],
            returns: TypeRefMeta::Result(
                &TypeRefMeta::Stream(&TypeRefMeta::I64),
                &TypeRefMeta::Named("CounterError"),
            ),
            is_async: true,
            takes_ctx: false,
            coalesce: false,
            docs: "",
        }],
        store: Some(StoreMeta {
            signals: &[
                SignalMeta {
                    name: "value",
                    signal_id: 0,
                    ty: TypeRefMeta::I64,
                    computed: false,
                    key: None,
                    no_coalesce: false,
                    default: false,
                },
                SignalMeta {
                    name: "rows",
                    signal_id: 1,
                    ty: TypeRefMeta::Vec(&TypeRefMeta::Named("Row")),
                    computed: false,
                    key: Some("id"),
                    no_coalesce: false,
                    default: false,
                },
            ],
        }),
        docs: "A counter store.",
        dispatch: no_dispatch,
    };

    #[test]
    fn type_ref_meta_converts_recursively() {
        let ty = TypeRef::from(&NESTED);
        assert_eq!(
            ty,
            TypeRef::result(
                TypeRef::map(TypeRef::Uuid, TypeRef::vec(TypeRef::named("Todo"))),
                TypeRef::named("TodoError"),
            )
        );
    }

    #[test]
    fn every_type_ref_meta_leaf_maps_to_its_twin() {
        let pairs: [(TypeRefMeta, TypeRef); 17] = [
            (TypeRefMeta::Bool, TypeRef::Bool),
            (TypeRefMeta::I8, TypeRef::I8),
            (TypeRefMeta::I16, TypeRef::I16),
            (TypeRefMeta::I32, TypeRef::I32),
            (TypeRefMeta::I64, TypeRef::I64),
            (TypeRefMeta::U8, TypeRef::U8),
            (TypeRefMeta::U16, TypeRef::U16),
            (TypeRefMeta::U32, TypeRef::U32),
            (TypeRefMeta::U64, TypeRef::U64),
            (TypeRefMeta::F32, TypeRef::F32),
            (TypeRefMeta::F64, TypeRef::F64),
            (TypeRefMeta::String, TypeRef::String),
            (TypeRefMeta::Bytes, TypeRef::Bytes),
            (TypeRefMeta::Unit, TypeRef::Unit),
            (TypeRefMeta::Duration, TypeRef::Duration),
            (TypeRefMeta::Timestamp, TypeRef::Timestamp),
            (TypeRefMeta::Uuid, TypeRef::Uuid),
        ];
        for (meta, def) in pairs {
            assert_eq!(TypeRef::from(&meta), def);
        }
        assert_eq!(
            TypeRef::from(&TypeRefMeta::Option(&TypeRefMeta::Lazy(&TypeRefMeta::Bool))),
            TypeRef::option(TypeRef::lazy(TypeRef::Bool))
        );
    }

    #[test]
    fn object_meta_converts_with_store_and_docs() {
        let def = ObjectDef::from(&COUNTER);
        assert_eq!(def.name, "Counter");
        assert_eq!(def.type_id, ids::type_id("Counter"));
        assert_eq!(def.docs, "A counter store.");
        assert_eq!(def.constructors.len(), 1);
        assert!(def.constructors[0].takes_ctx);
        assert_eq!(def.constructors[0].docs, "Creates a counter.");
        assert_eq!(
            def.methods[0].returns,
            TypeRef::result(
                TypeRef::stream(TypeRef::I64),
                TypeRef::named("CounterError")
            )
        );
        let store = def.store.expect("store");
        assert_eq!(store.signals.len(), 2);
        assert_eq!(store.signals[1].key.as_deref(), Some("id"));
        assert_eq!(store.signals[0].key, None);
    }

    #[test]
    fn dispatch_pointer_survives_copy_and_is_callable() {
        let copy = COUNTER;
        let out = (copy.dispatch)(
            &(),
            DispatchCall {
                method_id: 1,
                call_id: 1,
                handle: 0,
                args: &[],
            },
        );
        assert!(out.is::<()>());
    }

    #[test]
    fn variant_and_enum_meta_convert() {
        static E: EnumMeta = EnumMeta {
            name: "TodoError",
            type_id: ids::type_id("TodoError"),
            is_error: true,
            variants: &[
                VariantMeta {
                    name: "NotFound",
                    index: 0,
                    fields: &[FieldMeta {
                        name: "0",
                        ty: TypeRefMeta::String,
                        default: false,
                        docs: "",
                    }],
                    tuple: true,
                    message: Some("not found: {0}"),
                    docs: "Missing.",
                },
                VariantMeta {
                    name: "Offline",
                    index: 1,
                    fields: &[],
                    tuple: false,
                    message: None,
                    docs: "",
                },
            ],
            docs: "",
        };
        let def = EnumDef::from(&E);
        assert!(def.is_error);
        assert_eq!(def.variants[0].message.as_deref(), Some("not found: {0}"));
        assert!(def.variants[0].tuple);
        assert_eq!(def.variants[1].message, None);
        assert!(def.variants[1].fields.is_empty());
    }

    #[test]
    fn function_port_query_meta_convert() {
        static F: FunctionMeta = FunctionMeta {
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
            dispatch: no_dispatch,
        };
        static P: PortMeta = PortMeta {
            name: "Lifecycle",
            port_id: ids::port_id("Lifecycle"),
            kind: PortKind::Event,
            background: false,
            methods: &[MethodMeta {
                name: "changed",
                method_id: ids::port_method_id("Lifecycle", "changed"),
                params: &[ParamMeta {
                    name: "state",
                    ty: TypeRefMeta::Named("AppState"),
                }],
                returns: TypeRefMeta::Unit,
                is_async: false,
                takes_ctx: false,
                coalesce: false,
                docs: "",
            }],
            docs: "",
        };
        static Q: QueryMeta = QueryMeta {
            name: "todos",
            query_id: ids::query_id("todos"),
            kind: QueryKind::Query,
            key: "todos:{page}",
            params: &[ParamMeta {
                name: "page",
                ty: TypeRefMeta::U32,
            }],
            returns: TypeRefMeta::Vec(&TypeRefMeta::Named("Todo")),
            stale_ms: Some(30_000),
            persist: true,
            idempotent: true,
        };
        let f = FunctionDef::from(&F);
        assert_eq!(f.method_id, ids::function_id("greet"));
        assert_eq!(f.docs, "Greets.");
        let p = PortDef::from(&P);
        assert_eq!(p.kind, PortKind::Event);
        assert_eq!(p.methods[0].params[0].ty, TypeRef::named("AppState"));
        let q = QueryDef::from(&Q);
        assert_eq!(q.kind, QueryKind::Query);
        assert_eq!(q.stale_ms, Some(30_000));
        assert!(q.persist && q.idempotent);
        assert_eq!(q.key, "todos:{page}");
    }
}
