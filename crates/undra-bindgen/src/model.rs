//! The generator's view of a schema: sorted, indexed and augmented with the
//! items bindgen synthesizes (the query status enum, one handle object per
//! query, one function per mutation).
//!
//! Every language generator reads the [`Model`], never the raw [`Schema`], so
//! ordering, kind lookups and the shape of returns are decided in one place.

use std::collections::HashMap;

use undra_meta::{
    EnumDef, FunctionDef, MethodDef, ObjectDef, ParamDef, PortDef, QueryDef, QueryKind, RecordDef,
    Schema, SignalDef, StoreDef, TypeRef, VariantDef, ids,
};

use crate::naming;
use crate::stdlib::{self, Covered};

/// Name of the synthesized unit enum describing a query's fetch state.
pub const QUERY_STATUS: &str = "QueryStatus";

/// Method id of `refetch()` on every generated query handle:
/// `fnv1a32("query.refetch")`. `undra-query` must dispatch it on handle
/// objects.
pub const QUERY_REFETCH_ID: u32 = ids::fnv1a32("query.refetch");

/// Method id of `invalidate()` on every generated query handle:
/// `fnv1a32("query.invalidate")`. `undra-query` must dispatch it on handle
/// objects.
pub const QUERY_INVALIDATE_ID: u32 = ids::fnv1a32("query.invalidate");

/// Method id of `setPollInterval(_:)` on every generated query handle
/// ([`ids::SET_POLL_INTERVAL_METHOD_ID`], ADR-043): its one argument is the wire
/// `Option<Duration>`.
pub const QUERY_SET_POLL_INTERVAL_ID: u32 = ids::SET_POLL_INTERVAL_METHOD_ID;

/// Method id of `fetchNextPage()` on the handle of an infinite query
/// ([`ids::FETCH_NEXT_PAGE_METHOD_ID`], ADR-043).
pub const QUERY_FETCH_NEXT_PAGE_ID: u32 = ids::FETCH_NEXT_PAGE_METHOD_ID;

/// What a `Named` type reference points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedKind {
    /// A record.
    Record,
    /// An enum whose variants all have no fields.
    UnitEnum,
    /// An enum with at least one variant that has fields.
    DataEnum,
    /// A `#[undra::error]` enum.
    Error,
    /// An object or store (crosses by handle).
    Object,
}

/// The language a [`Model`] is built for. Most of the model is the same in all three; what the
/// platform runtime already provides (the standard library, ADR-024) is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    /// Swift (`UndraRuntime`).
    Swift,
    /// Kotlin (`dev.undra.runtime`).
    Kotlin,
    /// TypeScript (`@undra/runtime`).
    TypeScript,
}

/// A standard type the platform runtime provides: generated code refers to it and does not
/// declare it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct External {
    /// What the runtime calls it (`UndraAppState` for the `AppState` of Swift, the standard name
    /// everywhere else).
    pub spelling: &'static str,
    /// What kind of type it is.
    pub kind: NamedKind,
}

/// The shape of a method's return type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ret<'a> {
    /// `T` (possibly `Unit`).
    Plain(&'a TypeRef),
    /// `Result<T, E>`.
    Result {
        /// The success type (possibly `Unit`).
        ok: &'a TypeRef,
        /// The name of the error enum.
        err: &'a str,
    },
    /// `Stream<T>`.
    Stream(&'a TypeRef),
    /// `Result<Stream<T>, E>`.
    ResultStream {
        /// The stream's item type.
        item: &'a TypeRef,
        /// The name of the error enum.
        err: &'a str,
    },
}

impl<'a> Ret<'a> {
    /// Classifies `ty`, or `None` when it is not one of the four legal return
    /// shapes (a `Result` whose error type is not a plain name, for example).
    #[must_use]
    pub fn classify(ty: &'a TypeRef) -> Option<Ret<'a>> {
        match ty {
            TypeRef::Result(ok, err) => {
                let TypeRef::Named(err) = &**err else {
                    return None;
                };
                match &**ok {
                    TypeRef::Stream(item) => Some(Ret::ResultStream { item, err }),
                    TypeRef::Result(..) => None,
                    ok => Some(Ret::Result { ok, err }),
                }
            }
            TypeRef::Stream(item) => Some(Ret::Stream(item)),
            other => Some(Ret::Plain(other)),
        }
    }

    /// The error enum's name, when the return can fail with a typed error.
    #[must_use]
    pub fn error(&self) -> Option<&'a str> {
        match self {
            Ret::Result { err, .. } | Ret::ResultStream { err, .. } => Some(err),
            Ret::Plain(_) | Ret::Stream(_) => None,
        }
    }

    /// Whether the call opens a stream.
    #[must_use]
    pub fn is_stream(&self) -> bool {
        matches!(self, Ret::Stream(_) | Ret::ResultStream { .. })
    }
}

/// The sorted, indexed, augmented schema.
#[derive(Clone, Debug)]
pub struct Model {
    /// `crate_name` of the schema.
    pub crate_name: String,
    /// `Schema::hash()` of the schema as given (before augmentation).
    pub schema_hash: u64,
    /// Records, sorted by name.
    pub records: Vec<RecordDef>,
    /// Non-error enums, sorted by name, variants sorted by index; the
    /// synthesized `QueryStatus` comes last when the schema has queries.
    pub enums: Vec<EnumDef>,
    /// Error enums, sorted by name, variants sorted by index.
    pub errors: Vec<EnumDef>,
    /// Plain objects (no store), sorted by name.
    pub objects: Vec<ObjectDef>,
    /// Stores, sorted by name.
    pub stores: Vec<ObjectDef>,
    /// One synthesized handle object per query (not mutation), sorted by
    /// query name.
    pub query_handles: Vec<ObjectDef>,
    /// Free functions, sorted by name.
    pub functions: Vec<FunctionDef>,
    /// One synthesized function per mutation, sorted by name.
    pub mutations: Vec<FunctionDef>,
    /// Ports (not callback interfaces), sorted by name.
    pub ports: Vec<PortDef>,
    /// Host callback interfaces (`PortKind::Callback`, ADR-041), sorted by name. They are ports
    /// with many instances: the host implements the generated protocol per instance, so they
    /// have their own generated shape and are not in [`Model::ports`].
    pub callbacks: Vec<PortDef>,
    /// Queries and mutations, sorted by name.
    pub queries: Vec<QueryDef>,
    kinds: HashMap<String, NamedKind>,
    externals: HashMap<String, External>,
    // The definitions behind `externals`, for the lookups that build placeholder values.
    external_records: Vec<RecordDef>,
    external_enums: Vec<EnumDef>,
    external_errors: Vec<EnumDef>,
}

impl Model {
    /// Builds the model for `lang`. The schema is assumed to have passed
    /// [`crate::validate`].
    ///
    /// Unless `emit_standard` is set, the standard library (ADR-024) is left out of the lists
    /// of definitions: the standard ports entirely, and the standard types, which every platform
    /// runtime provides. A provided type stays known to [`Model::kind`] and its lookups and is
    /// reported by [`Model::external`].
    #[must_use]
    pub fn new(schema: &Schema, lang: Lang, emit_standard: bool) -> Model {
        let covered = if emit_standard {
            Covered::default()
        } else {
            stdlib::covered(schema)
        };

        let mut records = Vec::new();
        let mut standard_records = Vec::new();
        for record in &schema.records {
            if let Some(&standard) = covered.types.get(record.name.as_str()) {
                standard_records.push((standard, record.clone()));
            } else {
                records.push(record.clone());
            }
        }

        let mut enums = Vec::new();
        let mut errors = Vec::new();
        let mut standard_enums = Vec::new();
        for en in &schema.enums {
            let mut en = en.clone();
            en.variants.sort_by_key(|v| v.index);
            if let Some(&standard) = covered.types.get(en.name.as_str()) {
                standard_enums.push((standard, en));
            } else if en.is_error {
                errors.push(en);
            } else {
                enums.push(en);
            }
        }

        let mut objects = Vec::new();
        let mut stores = Vec::new();
        for object in &schema.objects {
            if object.store.is_some() {
                stores.push(object.clone());
            } else {
                objects.push(object.clone());
            }
        }
        objects.sort_by(|a, b| a.name.cmp(&b.name));
        stores.sort_by(|a, b| a.name.cmp(&b.name));

        // The standard function (ADR-046) is the runtimes' own API, not generated.
        let mut functions: Vec<_> = schema
            .functions
            .iter()
            .filter(|f| !covered.functions.contains(f.name.as_str()))
            .cloned()
            .collect();
        functions.sort_by(|a, b| a.name.cmp(&b.name));
        let mut ports: Vec<PortDef> = schema
            .ports
            .iter()
            .filter(|p| !covered.ports.contains(p.name.as_str()))
            .filter(|p| p.kind != undra_meta::PortKind::Callback)
            .cloned()
            .collect();
        ports.sort_by(|a, b| a.name.cmp(&b.name));
        let mut callbacks: Vec<PortDef> = schema
            .ports
            .iter()
            .filter(|p| p.kind == undra_meta::PortKind::Callback)
            .cloned()
            .collect();
        callbacks.sort_by(|a, b| a.name.cmp(&b.name));
        let mut queries = schema.queries.clone();
        queries.sort_by(|a, b| a.name.cmp(&b.name));

        let mut query_handles = Vec::new();
        let mut mutations = Vec::new();
        for query in &queries {
            match query.kind {
                QueryKind::Query => query_handles.push(query_handle(query)),
                QueryKind::Mutation => mutations.push(mutation_function(query)),
            }
        }

        // The standard types are provided by the platform runtime: generated code refers to them
        // and declares none (ADR-024).
        let mut externals = HashMap::new();
        let mut external_records = Vec::new();
        let mut external_enums = Vec::new();
        let mut external_errors = Vec::new();
        for (standard, record) in standard_records {
            externals.insert(
                record.name.clone(),
                External {
                    spelling: stdlib::runtime_spelling(lang, standard),
                    kind: NamedKind::Record,
                },
            );
            external_records.push(record);
        }
        for (standard, en) in standard_enums {
            // The opt-in standard library has enums with data (`WsMessage`, `DbValue`): a
            // standard enum is classified by its shape like any other.
            let kind = if en.is_error {
                NamedKind::Error
            } else if is_unit_enum(&en) {
                NamedKind::UnitEnum
            } else {
                NamedKind::DataEnum
            };
            externals.insert(
                en.name.clone(),
                External {
                    spelling: stdlib::runtime_spelling(lang, standard),
                    kind,
                },
            );
            if en.is_error {
                external_errors.push(en);
            } else {
                external_enums.push(en);
            }
        }

        records.sort_by(|a, b| a.name.cmp(&b.name));
        enums.sort_by(|a, b| a.name.cmp(&b.name));
        errors.sort_by(|a, b| a.name.cmp(&b.name));
        // The synthesized `QueryStatus` comes last.
        if !query_handles.is_empty() {
            enums.push(query_status());
        }

        let mut kinds = HashMap::new();
        for r in &records {
            kinds.insert(r.name.clone(), NamedKind::Record);
        }
        for e in &enums {
            kinds.insert(
                e.name.clone(),
                if is_unit_enum(e) {
                    NamedKind::UnitEnum
                } else {
                    NamedKind::DataEnum
                },
            );
        }
        for e in &errors {
            kinds.insert(e.name.clone(), NamedKind::Error);
        }
        for o in objects.iter().chain(&stores).chain(&query_handles) {
            kinds.insert(o.name.clone(), NamedKind::Object);
        }
        for (name, external) in &externals {
            kinds.insert(name.clone(), external.kind);
        }

        Model {
            crate_name: schema.crate_name.clone(),
            schema_hash: schema.hash(),
            records,
            enums,
            errors,
            objects,
            stores,
            query_handles,
            functions,
            mutations,
            ports,
            callbacks,
            queries,
            kinds,
            externals,
            external_records,
            external_enums,
            external_errors,
        }
    }

    /// What `name` refers to, if it is a declared type.
    #[must_use]
    pub fn kind(&self, name: &str) -> Option<NamedKind> {
        self.kinds.get(name).copied()
    }

    /// The standard type `name` when the platform runtime provides it, so that generated code
    /// refers to it instead of declaring it.
    #[must_use]
    pub fn external(&self, name: &str) -> Option<&External> {
        self.externals.get(name)
    }

    /// The record called `name`, declared or provided by the runtime.
    #[must_use]
    pub fn record(&self, name: &str) -> Option<&RecordDef> {
        self.records
            .iter()
            .chain(&self.external_records)
            .find(|r| r.name == name)
    }

    /// The enum (not error) called `name`, declared or provided by the runtime.
    #[must_use]
    pub fn enum_def(&self, name: &str) -> Option<&EnumDef> {
        self.enums
            .iter()
            .chain(&self.external_enums)
            .find(|e| e.name == name)
    }

    /// The error enum called `name`, declared or provided by the runtime.
    #[must_use]
    pub fn error_def(&self, name: &str) -> Option<&EnumDef> {
        self.errors
            .iter()
            .chain(&self.external_errors)
            .find(|e| e.name == name)
    }

    /// The type `name` as generated code writes it: the runtime's own spelling for a standard
    /// type the runtime provides, `name` for everything else.
    #[must_use]
    pub fn spelled<'a>(&'a self, name: &'a str) -> &'a str {
        self.externals.get(name).map_or(name, |e| e.spelling)
    }

    /// The newtype called `name`: a record the schema marks `transparent` (ADR-042), which crosses
    /// as its one field and is a wrapper type on the platforms.
    #[must_use]
    pub fn newtype(&self, name: &str) -> Option<&RecordDef> {
        self.records
            .iter()
            .find(|r| r.transparent && r.name == name)
    }

    /// The type a newtype wraps, when `name` is one.
    #[must_use]
    pub fn newtype_inner(&self, name: &str) -> Option<&TypeRef> {
        self.newtype(name)
            .and_then(|r| r.fields.first())
            .map(|f| &f.ty)
    }

    /// `ty` with the newtypes at its top peeled off: `UserId` of `Uuid` is `Uuid`, a newtype of a
    /// newtype is the innermost type. A newtype and its inner type have the same bytes on the wire,
    /// so this is also the type whose encoding `ty` has. (A reference cycle between newtypes, which
    /// no Rust type can have, stops after a few steps.)
    #[must_use]
    pub fn resolve_newtypes<'a>(&'a self, ty: &'a TypeRef) -> &'a TypeRef {
        let mut ty = ty;
        for _ in 0..16 {
            match ty {
                TypeRef::Named(name) => match self.newtype_inner(name) {
                    Some(inner) => ty = inner,
                    None => return ty,
                },
                other => return other,
            }
        }
        ty
    }

    /// Whether a newtype of `ty` is generated `Comparable` (ADR-042): the platform type has an
    /// order that means something: integers, floats, `String`, `Timestamp`, `Duration`, `Decimal`,
    /// and a newtype of one of those. Not `Uuid` (Foundation's `UUID` is `Comparable` only from
    /// iOS 17), `bool`, bytes, records, enums or collections.
    #[must_use]
    pub fn is_ordered(&self, ty: &TypeRef) -> bool {
        let ty = self.resolve_newtypes(ty);
        ty.is_integer()
            || matches!(
                ty,
                TypeRef::F32
                    | TypeRef::F64
                    | TypeRef::String
                    | TypeRef::Timestamp
                    | TypeRef::Duration
                    | TypeRef::Decimal
            )
    }

    /// Whether `name` is the generated handle of a query (not a mutation).
    #[must_use]
    pub fn is_query_handle(&self, name: &str) -> bool {
        self.query_handles.iter().any(|h| h.name == name)
    }

    /// The `infinite` query whose handle is called `handle`, with what the generators need of it
    /// (ADR-043).
    #[must_use]
    pub fn infinite(&self, handle: &str) -> Option<Infinite<'_>> {
        let query = self.queries.iter().find(|q| {
            q.kind == QueryKind::Query
                && q.infinite.is_some()
                && query_handle_name(&q.name) == handle
        })?;
        let infinite = query.infinite.as_ref()?;
        let (TypeRef::Vec(item), _) = query_types(query) else {
            return None;
        };
        let TypeRef::Named(item) = *item else {
            return None;
        };
        let item_record = self.record(&item)?;
        Some(Infinite {
            item: item_record,
            key: &item_record
                .fields
                .iter()
                .find(|f| f.name == infinite.item_key)?
                .name,
        })
    }

    /// The names of the item records of every infinite query whose `item_key` field is called
    /// `id`: Swift makes them `Identifiable` (ADR-043).
    #[must_use]
    pub fn identifiable_items(&self) -> Vec<&str> {
        self.queries
            .iter()
            .filter(|q| q.kind == QueryKind::Query)
            .filter_map(|q| {
                let handle = query_handle_name(&q.name);
                self.infinite(&handle)
            })
            .filter(|i| i.key == "id")
            .map(|i| i.item.name.as_str())
            .collect()
    }

    /// All objects that own a native class: plain objects, stores, then query
    /// handles.
    pub fn all_objects(&self) -> impl Iterator<Item = &ObjectDef> {
        self.objects
            .iter()
            .chain(&self.stores)
            .chain(&self.query_handles)
    }

    /// Whether any method or function of the schema returns a stream (`Stream<T>` or `Result<Stream<T>, E>`). The TypeScript entry
    /// of such a schema passes `features: [streams]` to the runtime, so the stream support is up front instead of fetched at the
    /// first stream (ADR-057).
    #[must_use]
    pub fn has_streams(&self) -> bool {
        let is_stream =
            |returns: &TypeRef| Ret::classify(returns).is_some_and(|ret| ret.is_stream());
        self.all_objects()
            .flat_map(|object| &object.methods)
            .any(|method| is_stream(&method.returns))
            || self
                .functions
                .iter()
                .chain(&self.mutations)
                .any(|function| is_stream(&function.returns))
    }

    /// The doc comment of a store signal, when there is one to write.
    #[must_use]
    pub fn signal_doc(&self, object: &ObjectDef, signal: &SignalDef) -> Option<&'static str> {
        if self.query_handles.iter().any(|h| h.name == object.name) {
            return Some(match signal.name.as_str() {
                "data" if self.infinite(&object.name).is_some() => {
                    "Every row loaded so far, in order: the first page, then each page `fetchNextPage()` loaded. Empty until the first page arrives."
                }
                "data" => "The latest successful result, if any.",
                "status" => "Where the query is in its fetch lifecycle.",
                "error" => "The error of the latest failed fetch, cleared by the next success.",
                "fetching" => {
                    "Whether a fetch is in flight (also true while refetching stale data)."
                }
                "has_next_page" => {
                    "Whether another page can be fetched: the last page loaded named a next cursor."
                }
                "fetching_next_page" => "Whether the fetch of the next page is in flight.",
                _ => "When `data` was last updated.",
            });
        }
        if matches!(signal.ty, TypeRef::Lazy(_)) {
            return Some(if signal.computed {
                "A list the core derives from another list and this mirror pages through: a row is missing until its page has arrived, and reading it asks for the page. Read-only."
            } else {
                "A list the core holds and this mirror pages through: a row is missing until its page has arrived, and reading it asks for the page."
            });
        }
        signal.computed.then_some(if signal.key.is_some() {
            // `computed` with a key is a derived keyed list (ADR-039): the same declaration as a
            // computed list, maintained from its source as keyed patches.
            "Derived by the core from another list; read-only. Changes arrive as keyed patches."
        } else {
            "Computed by the core; read-only."
        })
    }
}

/// What an `infinite` query adds to its handle (ADR-043): the record of its rows and the field that
/// identifies a row.
#[derive(Clone, Copy, Debug)]
pub struct Infinite<'a> {
    /// The record of one row (`T` of `Page<T, C>`).
    pub item: &'a RecordDef,
    /// The row's key field, as the schema names it (`item_key`).
    pub key: &'a str,
}

/// How a method of a generated class hands objects over: what a return or a parameter that
/// involves an object looks like once classified (ADR-040).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObjectUse<'a> {
    /// `T`: one object.
    One(&'a str),
    /// `Option<T>`.
    Optional(&'a str),
    /// `Vec<T>`.
    Many(&'a str),
}

impl<'a> ObjectUse<'a> {
    /// Classifies `ty` as an object, an optional object or a list of objects; `None` for any
    /// other type.
    #[must_use]
    pub fn of(ty: &'a TypeRef) -> Option<ObjectUse<'a>> {
        match ty {
            TypeRef::Object(name) => Some(ObjectUse::One(name)),
            TypeRef::Option(inner) => match &**inner {
                TypeRef::Object(name) => Some(ObjectUse::Optional(name)),
                _ => None,
            },
            TypeRef::Vec(inner) => match &**inner {
                TypeRef::Object(name) => Some(ObjectUse::Many(name)),
                _ => None,
            },
            _ => None,
        }
    }

    /// The object's type name.
    #[must_use]
    pub fn name(&self) -> &'a str {
        match self {
            ObjectUse::One(name) | ObjectUse::Optional(name) | ObjectUse::Many(name) => name,
        }
    }
}

/// A host callback interface in a parameter: a callback, or an optional one (ADR-041).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallbackUse<'a> {
    /// `Arc<dyn Trait>`.
    One(&'a str),
    /// `Option<Arc<dyn Trait>>`.
    Optional(&'a str),
}

impl<'a> CallbackUse<'a> {
    /// Classifies `ty` as a callback parameter; `None` for any other type.
    #[must_use]
    pub fn of(ty: &'a TypeRef) -> Option<CallbackUse<'a>> {
        match ty {
            TypeRef::Callback(name) => Some(CallbackUse::One(name)),
            TypeRef::Option(inner) => match &**inner {
                TypeRef::Callback(name) => Some(CallbackUse::Optional(name)),
                _ => None,
            },
            _ => None,
        }
    }

    /// The callback interface's name.
    #[must_use]
    pub fn name(&self) -> &'a str {
        match self {
            CallbackUse::One(name) | CallbackUse::Optional(name) => name,
        }
    }
}

/// The ids of a store's `#[undra(no_coalesce)]` signals, in signal order: what the generated store
/// passes to its mirror registration so the platform applies every entry of them (ADR-031).
/// Empty for an object that is not a store or has none.
#[must_use]
pub fn no_coalesce_ids(object: &ObjectDef) -> Vec<u32> {
    object
        .store
        .iter()
        .flat_map(|store| store.signals.iter())
        .filter(|signal| signal.no_coalesce)
        .map(|signal| signal.signal_id)
        .collect()
}

/// Whether an enum's variants all carry no fields.
#[must_use]
pub fn is_unit_enum(en: &EnumDef) -> bool {
    !en.is_error && en.variants.iter().all(|v| v.fields.is_empty())
}

fn query_status() -> EnumDef {
    let variants = [
        ("Idle", "No fetch has started."),
        ("Fetching", "A fetch is in flight."),
        ("Success", "The latest fetch succeeded."),
        ("Error", "The latest fetch failed."),
    ];
    EnumDef {
        name: QUERY_STATUS.to_owned(),
        type_id: ids::type_id(QUERY_STATUS),
        is_error: false,
        variants: variants
            .iter()
            .enumerate()
            .map(|(index, (name, docs))| VariantDef {
                name: (*name).to_owned(),
                index: u16::try_from(index).unwrap_or(u16::MAX),
                fields: Vec::new(),
                tuple: false,
                message: None,
                docs: (*docs).to_owned(),
            })
            .collect(),
        docs: "The fetch state of a query (SPEC section 9).".to_owned(),
    }
}

/// The name of the handle class generated for query `name`.
#[must_use]
pub fn query_handle_name(query_name: &str) -> String {
    format!("{}QueryHandle", naming::pascal(query_name))
}

/// The type a query hands to its `data` signal and the error it reports.
fn query_types(query: &QueryDef) -> (TypeRef, Option<String>) {
    match &query.returns {
        TypeRef::Result(ok, err) => {
            let err = match &**err {
                TypeRef::Named(name) => Some(name.clone()),
                _ => None,
            };
            ((**ok).clone(), err)
        }
        other => (other.clone(), None),
    }
}

fn query_handle(query: &QueryDef) -> ObjectDef {
    let name = query_handle_name(&query.name);
    let (ok, err) = query_types(query);
    let error_ty = match err {
        Some(err) => TypeRef::option(TypeRef::named(err)),
        None => TypeRef::option(TypeRef::String),
    };
    let signal = |id: u32, name: &str, ty: TypeRef| SignalDef {
        name: name.to_owned(),
        signal_id: id,
        ty,
        computed: false,
        key: None,
        no_coalesce: false,
        default: false,
    };
    let method = |name: &str, id: u32, params: Vec<ParamDef>, docs: &str| MethodDef {
        name: name.to_owned(),
        method_id: id,
        params,
        returns: TypeRef::Unit,
        is_async: false,
        takes_ctx: false,
        coalesce: false,
        docs: docs.to_owned(),
    };
    // An infinite query's `data` is the list of every row loaded, keyed by the item's key so that
    // the next page arrives as a patch of appended rows (ADR-043); any other query's is the
    // optional result.
    let data = match &query.infinite {
        Some(infinite) => SignalDef {
            key: Some(infinite.item_key.clone()),
            ..signal(0, "data", ok)
        },
        None => signal(0, "data", TypeRef::option(ok)),
    };
    let mut signals = vec![
        data,
        signal(1, "status", TypeRef::named(QUERY_STATUS)),
        signal(2, "error", error_ty),
        signal(3, "fetching", TypeRef::Bool),
        signal(4, "updated_at", TypeRef::option(TypeRef::Timestamp)),
    ];
    let mut methods = Vec::new();
    if query.infinite.is_some() {
        signals.push(signal(5, "has_next_page", TypeRef::Bool));
        signals.push(signal(6, "fetching_next_page", TypeRef::Bool));
        methods.push(method(
            "fetch_next_page",
            QUERY_FETCH_NEXT_PAGE_ID,
            Vec::new(),
            "Fetches the next page and appends its rows to `data`; nothing happens while a page is loading or when there is no next page.",
        ));
    }
    methods.push(method(
        "refetch",
        QUERY_REFETCH_ID,
        Vec::new(),
        "Fetches again now, even if the data is fresh.",
    ));
    methods.push(method(
        "invalidate",
        QUERY_INVALIDATE_ID,
        Vec::new(),
        "Marks the cached entry stale; it refetches while observed.",
    ));
    methods.push(method(
        "set_poll_interval",
        QUERY_SET_POLL_INTERVAL_ID,
        vec![ParamDef {
            name: "interval".to_owned(),
            ty: TypeRef::option(TypeRef::Duration),
        }],
        "Overrides how often the query polls while this handle observes it, counted from the end of a fetch.\nThe entry polls at the smallest interval among its observers; no interval clears this handle's override.\nAn interval below 1 second or above 7 days is clamped to that range.",
    ));
    let mut docs = format!(
        "Observes the `{}` query (cache key `{}`).\nConstructing it registers an observer and fetches when the data is stale or missing.",
        query.name, query.key
    );
    if query.infinite.is_some() {
        docs.push_str(
            "\nThe query is paged: `data` grows by one page each time `fetchNextPage()` completes.",
        );
    }
    ObjectDef {
        name: name.clone(),
        type_id: query.query_id,
        constructors: vec![MethodDef {
            name: "new".to_owned(),
            method_id: query.query_id,
            params: query.params.clone(),
            returns: TypeRef::named(name),
            is_async: false,
            takes_ctx: false,
            coalesce: false,
            docs: String::new(),
        }],
        methods,
        store: Some(StoreDef { signals }),
        docs,
    }
}

fn mutation_function(query: &QueryDef) -> FunctionDef {
    FunctionDef {
        name: query.name.clone(),
        method_id: query.query_id,
        params: query.params.clone(),
        returns: query.returns.clone(),
        is_async: true,
        takes_ctx: false,
        docs: format!("Runs the `{}` mutation.", query.name),
    }
}

/// One piece of a rendered `#[error("...")]` message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MsgPart {
    /// Literal text.
    Text(String),
    /// The value of the variant field at this index.
    Field(usize),
}

/// Splits an `#[error("...")]` template into text and field references.
///
/// `{{` and `}}` are literal braces; `{0}` and `{name}` interpolate a field;
/// a format spec after a colon (`{0:?}`) is accepted and ignored, the value
/// is rendered with the target language's default string conversion.
///
/// # Errors
///
/// Returns a description of the problem for an unclosed brace or a
/// placeholder that names no field of the variant.
pub fn parse_message(variant: &VariantDef, template: &str) -> Result<Vec<MsgPart>, String> {
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                text.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                text.push('}');
            }
            '{' => {
                let mut inner = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(c) => inner.push(c),
                        None => return Err("unclosed `{` in the message".to_owned()),
                    }
                }
                let name = inner.split(':').next().unwrap_or("").trim();
                let index = field_index(variant, name).ok_or_else(|| {
                    format!(
                        "the message refers to `{{{name}}}`, which is not a field of the variant"
                    )
                })?;
                if !text.is_empty() {
                    parts.push(MsgPart::Text(std::mem::take(&mut text)));
                }
                parts.push(MsgPart::Field(index));
            }
            '}' => return Err("unmatched `}` in the message".to_owned()),
            c => text.push(c),
        }
    }
    if !text.is_empty() {
        parts.push(MsgPart::Text(text));
    }
    Ok(parts)
}

fn field_index(variant: &VariantDef, name: &str) -> Option<usize> {
    if variant.tuple {
        name.parse::<usize>()
            .ok()
            .filter(|&i| i < variant.fields.len())
    } else {
        variant.fields.iter().position(|f| f.name == name)
    }
}

/// The lines of a doc comment with the indentation common to all of them and
/// trailing whitespace removed. Empty for an empty comment.
#[must_use]
pub fn doc_lines(docs: &str) -> Vec<String> {
    let lines: Vec<&str> = docs.lines().map(str::trim_end).collect();
    let common = lines
        .iter()
        .filter(|l| !l.is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out: Vec<String> = lines
        .iter()
        .map(|l| l.get(common..).unwrap_or("").to_owned())
        .collect();
    while out.first().is_some_and(String::is_empty) {
        out.remove(0);
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use undra_meta::{FieldDef, InfiniteDef};

    use super::*;

    fn variant(tuple: bool, names: &[&str]) -> VariantDef {
        VariantDef {
            name: "V".into(),
            index: 0,
            fields: names
                .iter()
                .map(|n| FieldDef {
                    name: (*n).into(),
                    ty: TypeRef::String,
                    default: false,
                    docs: String::new(),
                })
                .collect(),
            tuple,
            message: None,
            docs: String::new(),
        }
    }

    #[test]
    fn classify_returns() {
        let t = TypeRef::I32;
        assert!(matches!(Ret::classify(&t), Some(Ret::Plain(_))));
        let r = TypeRef::result(TypeRef::Unit, TypeRef::named("E"));
        assert!(matches!(
            Ret::classify(&r),
            Some(Ret::Result { err: "E", .. })
        ));
        let s = TypeRef::stream(TypeRef::U32);
        assert!(matches!(Ret::classify(&s), Some(Ret::Stream(_))));
        let rs = TypeRef::result(TypeRef::stream(TypeRef::U32), TypeRef::named("E"));
        let c = Ret::classify(&rs).unwrap();
        assert!(matches!(c, Ret::ResultStream { err: "E", .. }));
        assert!(c.is_stream());
        assert_eq!(c.error(), Some("E"));
        // An error type that is not a plain name is not a legal return shape.
        let bad = TypeRef::result(TypeRef::Unit, TypeRef::String);
        assert!(Ret::classify(&bad).is_none());
        let nested = TypeRef::result(
            TypeRef::result(TypeRef::Unit, TypeRef::named("E")),
            TypeRef::named("E"),
        );
        assert!(Ret::classify(&nested).is_none());
    }

    #[test]
    fn messages_parse_placeholders_and_escapes() {
        let v = variant(true, &["0", "1"]);
        assert_eq!(
            parse_message(&v, "a {0} b {1:?} {{x}}").unwrap(),
            vec![
                MsgPart::Text("a ".into()),
                MsgPart::Field(0),
                MsgPart::Text(" b ".into()),
                MsgPart::Field(1),
                MsgPart::Text(" {x}".into()),
            ]
        );
        let named = variant(false, &["reason"]);
        assert_eq!(
            parse_message(&named, "storage ({reason})").unwrap(),
            vec![
                MsgPart::Text("storage (".into()),
                MsgPart::Field(0),
                MsgPart::Text(")".into())
            ]
        );
        assert!(parse_message(&named, "{nope}").is_err());
        assert!(parse_message(&named, "{reason").is_err());
        assert!(parse_message(&named, "oops }").is_err());
        assert!(parse_message(&variant(true, &["0"]), "{1}").is_err());
        assert_eq!(parse_message(&named, "").unwrap(), vec![]);
    }

    fn newtype(name: &str, inner: TypeRef) -> RecordDef {
        RecordDef {
            name: name.into(),
            type_id: ids::type_id(name),
            fields: vec![FieldDef {
                name: "value".into(),
                ty: inner,
                default: false,
                docs: String::new(),
            }],
            transparent: true,
            docs: String::new(),
        }
    }

    fn model_of(records: Vec<RecordDef>) -> Model {
        let mut schema = Schema::new("t");
        schema.records = records;
        Model::new(&schema, Lang::Swift, false)
    }

    #[test]
    fn a_newtype_resolves_to_the_innermost_type_and_is_ordered_when_that_is() {
        let model = model_of(vec![
            newtype("UserId", TypeRef::Uuid),
            newtype("Owner", TypeRef::named("UserId")),
            newtype("Meters", TypeRef::F64),
            newtype("Span", TypeRef::named("Meters")),
            newtype("Tags", TypeRef::vec(TypeRef::String)),
            RecordDef {
                transparent: false,
                ..newtype("Plain", TypeRef::U8)
            },
        ]);
        assert_eq!(
            model.resolve_newtypes(&TypeRef::named("Owner")),
            &TypeRef::Uuid
        );
        assert_eq!(model.resolve_newtypes(&TypeRef::U8), &TypeRef::U8);
        // A record that is not a newtype is not peeled, and neither is what a newtype holds in a list.
        assert_eq!(
            model.resolve_newtypes(&TypeRef::named("Plain")),
            &TypeRef::named("Plain")
        );
        assert_eq!(
            model.resolve_newtypes(&TypeRef::named("Tags")),
            &TypeRef::vec(TypeRef::String)
        );
        for ordered in ["Meters", "Span"] {
            assert!(model.is_ordered(&TypeRef::named(ordered)), "{ordered}");
        }
        for unordered in ["UserId", "Owner", "Tags", "Plain"] {
            assert!(!model.is_ordered(&TypeRef::named(unordered)), "{unordered}");
        }
        // The scalars of ADR-042's list, and only those.
        for ty in [
            TypeRef::I8,
            TypeRef::U64,
            TypeRef::F32,
            TypeRef::String,
            TypeRef::Timestamp,
            TypeRef::Duration,
            TypeRef::Decimal,
        ] {
            assert!(model.is_ordered(&ty), "{ty}");
        }
        for ty in [TypeRef::Bool, TypeRef::Uuid, TypeRef::Bytes] {
            assert!(!model.is_ordered(&ty), "{ty}");
        }
    }

    #[test]
    fn a_cycle_of_newtypes_resolves_in_bounded_time() {
        let model = model_of(vec![
            newtype("A", TypeRef::named("B")),
            newtype("B", TypeRef::named("A")),
        ]);
        let _ = model.resolve_newtypes(&TypeRef::named("A"));
        assert!(!model.is_ordered(&TypeRef::named("A")));
    }

    fn infinite_schema() -> Schema {
        let mut schema = Schema::new("t");
        schema.records.push(RecordDef {
            name: "Post".into(),
            type_id: ids::type_id("Post"),
            fields: vec![FieldDef {
                name: "id".into(),
                ty: TypeRef::U64,
                default: false,
                docs: String::new(),
            }],
            transparent: false,
            docs: String::new(),
        });
        schema.queries.push(QueryDef {
            name: "feed".into(),
            query_id: ids::query_id("feed"),
            kind: QueryKind::Query,
            key: "feed".into(),
            params: Vec::new(),
            returns: TypeRef::vec(TypeRef::named("Post")),
            stale_ms: None,
            persist: false,
            idempotent: false,
            interval_ms: None,
            poll_in_background: false,
            infinite: Some(InfiniteDef {
                cursor: TypeRef::String,
                item_key: "id".into(),
            }),
        });
        schema
    }

    #[test]
    fn every_handle_gets_the_poll_method_and_an_infinite_one_its_paging() {
        let mut schema = infinite_schema();
        let mut plain = schema.queries[0].clone();
        plain.name = "latest".into();
        plain.query_id = ids::query_id("latest");
        plain.infinite = None;
        plain.returns = TypeRef::named("Post");
        schema.queries.push(plain);
        let model = Model::new(&schema, Lang::Kotlin, false);
        let handle = |name: &str| {
            model
                .query_handles
                .iter()
                .find(|h| h.name == name)
                .unwrap_or_else(|| panic!("no handle {name}"))
        };
        let methods = |h: &ObjectDef| -> Vec<(String, u32)> {
            h.methods
                .iter()
                .map(|m| (m.name.clone(), m.method_id))
                .collect()
        };
        let signals = |h: &ObjectDef| -> Vec<(u32, String, Option<String>)> {
            h.store
                .iter()
                .flat_map(|s| s.signals.iter())
                .map(|g| (g.signal_id, g.name.clone(), g.key.clone()))
                .collect()
        };
        let poll = (
            "set_poll_interval".to_owned(),
            ids::SET_POLL_INTERVAL_METHOD_ID,
        );
        let latest = handle("LatestQueryHandle");
        assert_eq!(
            methods(latest),
            [
                ("refetch".to_owned(), QUERY_REFETCH_ID),
                ("invalidate".to_owned(), QUERY_INVALIDATE_ID),
                poll.clone()
            ]
        );
        assert_eq!(
            latest.methods[2].params,
            [ParamDef {
                name: "interval".into(),
                ty: TypeRef::option(TypeRef::Duration)
            }]
        );
        assert_eq!(signals(latest).len(), 5);
        assert!(signals(latest).iter().all(|(_, _, key)| key.is_none()));
        assert!(model.infinite("LatestQueryHandle").is_none());

        let feed = handle("FeedQueryHandle");
        assert_eq!(
            methods(feed),
            [
                ("fetch_next_page".to_owned(), ids::FETCH_NEXT_PAGE_METHOD_ID),
                ("refetch".to_owned(), QUERY_REFETCH_ID),
                ("invalidate".to_owned(), QUERY_INVALIDATE_ID),
                poll
            ]
        );
        assert_eq!(
            signals(feed),
            [
                (0, "data".to_owned(), Some("id".to_owned())),
                (1, "status".to_owned(), None),
                (2, "error".to_owned(), None),
                (3, "fetching".to_owned(), None),
                (4, "updated_at".to_owned(), None),
                (5, "has_next_page".to_owned(), None),
                (6, "fetching_next_page".to_owned(), None),
            ]
        );
        // `data` is the list itself, not an option of one.
        let data = &feed.store.as_ref().unwrap().signals[0];
        assert_eq!(data.ty, TypeRef::vec(TypeRef::named("Post")));
        let infinite = model.infinite("FeedQueryHandle").unwrap();
        assert_eq!((infinite.item.name.as_str(), infinite.key), ("Post", "id"));
        assert_eq!(model.identifiable_items(), ["Post"]);
    }

    #[test]
    fn an_infinite_query_that_can_fail_keeps_its_typed_error() {
        // `Result<Vec<T>, E>`: the error type is the handle's, as for any query.
        let mut schema = infinite_schema();
        schema.queries[0].returns = TypeRef::result(
            TypeRef::vec(TypeRef::named("Post")),
            TypeRef::named("FeedError"),
        );
        let model = Model::new(&schema, Lang::Swift, false);
        let handle = &model.query_handles[0];
        let error = &handle.store.as_ref().unwrap().signals[2];
        assert_eq!(error.ty, TypeRef::option(TypeRef::named("FeedError")));
        assert!(model.infinite(&handle.name).is_some());
    }

    #[test]
    fn only_a_key_called_id_makes_the_rows_identifiable() {
        let mut schema = infinite_schema();
        schema.records[0].fields[0].name = "slug".into();
        schema.queries[0].infinite.as_mut().unwrap().item_key = "slug".into();
        let model = Model::new(&schema, Lang::Swift, false);
        assert!(model.infinite("FeedQueryHandle").is_some());
        assert!(model.identifiable_items().is_empty());
    }

    #[test]
    fn query_status_ids_are_the_documented_hashes() {
        assert_eq!(QUERY_REFETCH_ID, ids::fnv1a32("query.refetch"));
        assert_eq!(QUERY_INVALIDATE_ID, ids::fnv1a32("query.invalidate"));
        assert_ne!(QUERY_REFETCH_ID, QUERY_INVALIDATE_ID);
        assert_eq!(
            QUERY_SET_POLL_INTERVAL_ID,
            ids::fnv1a32("QueryHandle.set_poll_interval")
        );
        assert_eq!(
            QUERY_FETCH_NEXT_PAGE_ID,
            ids::fnv1a32("QueryHandle.fetch_next_page")
        );
    }
}
