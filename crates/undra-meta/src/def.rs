//! Owned schema definitions (SPEC §2.2).
//!
//! These are the types that are serialized to JSON, hashed, validated and
//! consumed by `undra-bindgen`. The macros do not build them directly; they emit
//! the `'static` mirrors in [`crate::meta`], which convert into these with
//! `From`.
//!
//! Field declaration order is significant: it is the key order of the JSON
//! output, and therefore of the canonical JSON the schema hash is computed
//! over. Which *lists* are order-sensitive is described in the crate's
//! canonical-form rules (SPEC §2.3): record and variant fields, parameters and
//! signals keep their declared order; the top-level lists, object methods and
//! constructors, and port methods are sorted by name in the canonical form.
//!
//! Every `docs` field defaults to empty when absent in JSON and is omitted from
//! serialized output when empty. The canonical form drops docs entirely.

use serde::{Deserialize, Serialize};

use crate::TypeRef;

/// The Undra specification version stamped into every schema built by
/// [`Schema::new`] and [`crate::collect_schema`].
pub const UNDRA_VERSION: &str = "1.0.0";

/// The complete public surface of one Undra core (SPEC §2.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schema {
    /// Undra specification version, `"1.0.0"` for v1. A label: excluded from
    /// the canonical JSON and the schema hash.
    pub undra_version: String,
    /// Cargo package name of the core. A label: excluded from the canonical
    /// JSON and the schema hash.
    pub crate_name: String,
    /// Records (`#[undra::api] struct`).
    pub records: Vec<RecordDef>,
    /// Enums, including errors (`is_error = true`).
    pub enums: Vec<EnumDef>,
    /// Objects, including stores (`store = Some(..)`).
    pub objects: Vec<ObjectDef>,
    /// Free functions.
    pub functions: Vec<FunctionDef>,
    /// Ports (traits implemented by the platform or by a Rust fake).
    pub ports: Vec<PortDef>,
    /// Queries and mutations.
    pub queries: Vec<QueryDef>,
}

impl Schema {
    /// An empty schema for `crate_name`, stamped with [`UNDRA_VERSION`].
    #[must_use]
    pub fn new(crate_name: impl Into<String>) -> Schema {
        Schema {
            undra_version: UNDRA_VERSION.to_owned(),
            crate_name: crate_name.into(),
            records: Vec::new(),
            enums: Vec::new(),
            objects: Vec::new(),
            functions: Vec::new(),
            ports: Vec::new(),
            queries: Vec::new(),
        }
    }
}

/// A record: a struct that crosses the boundary by value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordDef {
    /// Type name (Rust identifier, no module path).
    pub name: String,
    /// `fnv1a32(name)`, see [`crate::ids::type_id`].
    pub type_id: u32,
    /// Fields in declaration order, which is also their wire order.
    pub fields: Vec<FieldDef>,
    /// Doc comment; excluded from the schema hash.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs: String,
}

/// A field of a record or of an enum variant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDef {
    /// Field name; for tuple variants, the positional index (`"0"`, `"1"`, ..).
    pub name: String,
    /// Field type.
    pub ty: TypeRef,
    /// Whether the field carries `#[undra(default)]`.
    pub default: bool,
    /// Doc comment; excluded from the schema hash.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs: String,
}

/// An enum or error type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumDef {
    /// Type name.
    pub name: String,
    /// `fnv1a32(name)`.
    pub type_id: u32,
    /// `true` for `#[undra::error]` enums.
    pub is_error: bool,
    /// Variants; the canonical form sorts them by `index`.
    pub variants: Vec<VariantDef>,
    /// Doc comment; excluded from the schema hash.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs: String,
}

/// A variant of an [`EnumDef`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantDef {
    /// Variant name.
    pub name: String,
    /// Wire index (`u16` on the wire).
    pub index: u16,
    /// Payload fields; empty means a unit variant.
    pub fields: Vec<FieldDef>,
    /// `true` for tuple variants (positional fields), `false` for named ones.
    pub tuple: bool,
    /// The `#[error("...")]` message template, for error enums.
    pub message: Option<String>,
    /// Doc comment; excluded from the schema hash.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs: String,
}

/// An object (crosses by handle); a store when [`ObjectDef::store`] is set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectDef {
    /// Type name.
    pub name: String,
    /// `fnv1a32(name)`.
    pub type_id: u32,
    /// Functions returning `Self` / `Result<Self, E>` (sorted by name in the
    /// canonical form).
    pub constructors: Vec<MethodDef>,
    /// Methods, in declaration order (sorted by name in the canonical form).
    pub methods: Vec<MethodDef>,
    /// Signal table when the object is a `#[undra::store]`.
    pub store: Option<StoreDef>,
    /// Doc comment; excluded from the schema hash.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs: String,
}

/// A method, constructor or port method.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MethodDef {
    /// Method name.
    pub name: String,
    /// `fnv1a32("<Type>.<name>")`, see [`crate::ids::method_id`].
    pub method_id: u32,
    /// Parameters, excluding `self` and `Ctx`.
    pub params: Vec<ParamDef>,
    /// `Unit`, `T`, `Result<T,E>`, `Stream<T>` or `Result<Stream<T>,E>`.
    pub returns: TypeRef,
    /// Whether the method is `async`.
    pub is_async: bool,
    /// Whether the first Rust parameter is a `Ctx` (constructors and free
    /// functions only).
    pub takes_ctx: bool,
    /// Doc comment; excluded from the schema hash.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs: String,
}

/// A method or function parameter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamDef {
    /// Parameter name.
    pub name: String,
    /// Parameter type.
    pub ty: TypeRef,
}

/// The signal table of a store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreDef {
    /// Signals in declaration order; `signal_id` equals the index.
    pub signals: Vec<SignalDef>,
}

/// One reactive field of a store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalDef {
    /// Field name.
    pub name: String,
    /// Zero-based index among the store's signal fields.
    pub signal_id: u32,
    /// The value type `T` of the `Signal<T>` / `Computed<T>` / `Lazy<T>`.
    pub ty: TypeRef,
    /// `true` for `Computed<T>` signals.
    pub computed: bool,
    /// The `#[undra(key = "id")]` field enabling keyed patches on `Vec<T>`.
    pub key: Option<String>,
}

/// A free function.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionDef {
    /// Function name.
    pub name: String,
    /// `fnv1a32("fn.<name>")`, see [`crate::ids::function_id`].
    pub method_id: u32,
    /// Parameters, excluding `Ctx`.
    pub params: Vec<ParamDef>,
    /// Return type, same shapes as [`MethodDef::returns`].
    pub returns: TypeRef,
    /// Whether the function is `async`.
    pub is_async: bool,
    /// Whether the first Rust parameter is a `Ctx`.
    pub takes_ctx: bool,
    /// Doc comment; excluded from the schema hash.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs: String,
}

/// How a port's methods behave.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortKind {
    /// Every method is synchronous.
    Sync,
    /// At least one method is `async`.
    Async,
    /// Fire-and-forget host to core events; methods return `()`.
    Event,
}

/// A port: a trait implemented by the platform (foreign) or a Rust fake.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortDef {
    /// Trait name.
    pub name: String,
    /// `fnv1a32("port.<name>")`, see [`crate::ids::port_id`].
    pub port_id: u32,
    /// Sync, async or event.
    pub kind: PortKind,
    /// Port methods; `method_id` is `fnv1a32("<Trait>.<method>")`. Sorted by
    /// name in the canonical form.
    pub methods: Vec<MethodDef>,
    /// Doc comment; excluded from the schema hash.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs: String,
}

/// Whether a [`QueryDef`] reads (query) or writes (mutation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryKind {
    /// A cached, keyed read (`#[undra::query]`).
    Query,
    /// A write with optimistic patches (`#[undra::mutation]`).
    Mutation,
}

/// A query or mutation managed by `undra-query`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryDef {
    /// Function name.
    pub name: String,
    /// `fnv1a32("query.<name>")` or `fnv1a32("mutation.<name>")`.
    pub query_id: u32,
    /// Query or mutation.
    pub kind: QueryKind,
    /// Cache key template; may contain `{param}` placeholders.
    pub key: String,
    /// Parameters, excluding `Ctx`.
    pub params: Vec<ParamDef>,
    /// Return type (`T` or `Result<T,E>`).
    pub returns: TypeRef,
    /// Staleness window in milliseconds (`stale = "30s"`).
    pub stale_ms: Option<u64>,
    /// Whether results are persisted (`persist`).
    pub persist: bool,
    /// Whether the call is safe to replay (`idempotent`).
    pub idempotent: bool,
}
