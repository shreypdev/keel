//! The generator's view of a schema: sorted, indexed and augmented with the
//! items bindgen synthesizes (the query status enum, one handle object per
//! query, one function per mutation).
//!
//! Every language generator reads the [`Model`], never the raw [`Schema`], so
//! ordering, kind lookups and the shape of returns are decided in one place.

use std::collections::{BTreeSet, HashMap};

use undra_meta::{
    EnumDef, FunctionDef, MethodDef, ObjectDef, PortDef, QueryDef, QueryKind, RecordDef, Schema,
    SignalDef, StoreDef, TypeRef, VariantDef, ids,
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
    /// Ports, sorted by name.
    pub ports: Vec<PortDef>,
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
    /// of definitions: the standard ports entirely, and the standard types when the platform
    /// runtime of `lang` provides them. A provided type stays known to [`Model::kind`] and its
    /// lookups and is reported by [`Model::external`]; a standard type the runtime does not
    /// provide is declared like a user's type, but only when something refers to it.
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
            if covered.types.contains(record.name.as_str()) {
                standard_records.push(record.clone());
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
            if covered.types.contains(en.name.as_str()) {
                standard_enums.push(en);
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

        let mut functions = schema.functions.clone();
        functions.sort_by(|a, b| a.name.cmp(&b.name));
        let mut ports: Vec<PortDef> = schema
            .ports
            .iter()
            .filter(|p| !covered.ports.contains(p.name.as_str()))
            .cloned()
            .collect();
        ports.sort_by(|a, b| a.name.cmp(&b.name));
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

        // The standard types: provided by the runtime (external), declared because something
        // refers to them, or dropped.
        let mut externals = HashMap::new();
        let mut external_records = Vec::new();
        let mut external_enums = Vec::new();
        let mut external_errors = Vec::new();
        let mut declared: Vec<Pending> = Vec::new();
        for record in standard_records {
            match stdlib::runtime_spelling(lang, &record.name) {
                Some(spelling) => {
                    externals.insert(
                        record.name.clone(),
                        External {
                            spelling,
                            kind: NamedKind::Record,
                        },
                    );
                    external_records.push(record);
                }
                None => declared.push(Pending::new(PendingDef::Record(record))),
            }
        }
        for en in standard_enums {
            match stdlib::runtime_spelling(lang, &en.name) {
                Some(spelling) => {
                    let kind = if en.is_error {
                        NamedKind::Error
                    } else {
                        NamedKind::UnitEnum
                    };
                    externals.insert(en.name.clone(), External { spelling, kind });
                    if en.is_error {
                        external_errors.push(en);
                    } else {
                        external_enums.push(en);
                    }
                }
                None => declared.push(Pending::new(PendingDef::Enum(en))),
            }
        }
        if !declared.is_empty() {
            let mut referenced = BTreeSet::new();
            for r in &records {
                for f in &r.fields {
                    mentions(&f.ty, &mut referenced);
                }
            }
            for e in enums.iter().chain(&errors) {
                enum_mentions(e, &mut referenced);
            }
            for o in objects.iter().chain(&stores).chain(&query_handles) {
                object_mentions(o, &mut referenced);
            }
            for f in functions.iter().chain(&mutations) {
                method_mentions(&f.params, &f.returns, &mut referenced);
            }
            for p in &ports {
                for m in &p.methods {
                    method_mentions(&m.params, &m.returns, &mut referenced);
                }
            }
            // A declared type pulls in the types it refers to.
            loop {
                let mut progressed = false;
                for d in &mut declared {
                    if !d.emitted && referenced.contains(d.name()) {
                        d.emitted = true;
                        d.mentions(&mut referenced);
                        progressed = true;
                    }
                }
                if !progressed {
                    break;
                }
            }
            for d in declared.into_iter().filter(|d| d.emitted) {
                match d.def {
                    PendingDef::Record(r) => records.push(r),
                    PendingDef::Enum(e) if e.is_error => errors.push(e),
                    PendingDef::Enum(e) => enums.push(e),
                }
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

    /// All objects that own a native class: plain objects, stores, then query
    /// handles.
    pub fn all_objects(&self) -> impl Iterator<Item = &ObjectDef> {
        self.objects
            .iter()
            .chain(&self.stores)
            .chain(&self.query_handles)
    }

    /// The doc comment of a store signal, when there is one to write.
    #[must_use]
    pub fn signal_doc(&self, object: &ObjectDef, signal: &SignalDef) -> Option<&'static str> {
        if self.query_handles.iter().any(|h| h.name == object.name) {
            return Some(match signal.name.as_str() {
                "data" => "The latest successful result, if any.",
                "status" => "Where the query is in its fetch lifecycle.",
                "error" => "The error of the latest failed fetch, cleared by the next success.",
                "fetching" => {
                    "Whether a fetch is in flight (also true while refetching stale data)."
                }
                _ => "When `data` was last updated.",
            });
        }
        signal
            .computed
            .then_some("Computed by the core; read-only.")
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

/// A standard type the runtime does not provide, waiting to see whether something refers to it.
struct Pending {
    def: PendingDef,
    emitted: bool,
}

enum PendingDef {
    Record(RecordDef),
    Enum(EnumDef),
}

impl Pending {
    fn new(def: PendingDef) -> Pending {
        Pending {
            def,
            emitted: false,
        }
    }

    fn name(&self) -> &str {
        match &self.def {
            PendingDef::Record(r) => &r.name,
            PendingDef::Enum(e) => &e.name,
        }
    }

    fn mentions(&self, out: &mut BTreeSet<String>) {
        match &self.def {
            PendingDef::Record(r) => {
                for f in &r.fields {
                    mentions(&f.ty, out);
                }
            }
            PendingDef::Enum(e) => enum_mentions(e, out),
        }
    }
}

/// Collects the names `ty` refers to.
fn mentions(ty: &TypeRef, out: &mut BTreeSet<String>) {
    match ty {
        TypeRef::Named(n) => {
            out.insert(n.clone());
        }
        TypeRef::Option(t) | TypeRef::Vec(t) | TypeRef::Lazy(t) | TypeRef::Stream(t) => {
            mentions(t, out);
        }
        TypeRef::Map(a, b) | TypeRef::Result(a, b) => {
            mentions(a, out);
            mentions(b, out);
        }
        _ => {}
    }
}

fn enum_mentions(en: &EnumDef, out: &mut BTreeSet<String>) {
    for v in &en.variants {
        for f in &v.fields {
            mentions(&f.ty, out);
        }
    }
}

fn method_mentions(params: &[undra_meta::ParamDef], returns: &TypeRef, out: &mut BTreeSet<String>) {
    for p in params {
        mentions(&p.ty, out);
    }
    mentions(returns, out);
}

fn object_mentions(object: &ObjectDef, out: &mut BTreeSet<String>) {
    for m in object.constructors.iter().chain(&object.methods) {
        method_mentions(&m.params, &m.returns, out);
    }
    if let Some(store) = &object.store {
        for signal in &store.signals {
            mentions(&signal.ty, out);
        }
    }
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
    };
    let method = |name: &str, id: u32, docs: &str| MethodDef {
        name: name.to_owned(),
        method_id: id,
        params: Vec::new(),
        returns: TypeRef::Unit,
        is_async: false,
        takes_ctx: false,
        docs: docs.to_owned(),
    };
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
            docs: String::new(),
        }],
        methods: vec![
            method(
                "refetch",
                QUERY_REFETCH_ID,
                "Fetches again now, even if the data is fresh.",
            ),
            method(
                "invalidate",
                QUERY_INVALIDATE_ID,
                "Marks the cached entry stale; it refetches while observed.",
            ),
        ],
        store: Some(StoreDef {
            signals: vec![
                signal(0, "data", TypeRef::option(ok)),
                signal(1, "status", TypeRef::named(QUERY_STATUS)),
                signal(2, "error", error_ty),
                signal(3, "fetching", TypeRef::Bool),
                signal(4, "updated_at", TypeRef::option(TypeRef::Timestamp)),
            ],
        }),
        docs: format!(
            "Observes the `{}` query (cache key `{}`).\nConstructing it registers an observer and fetches when the data is stale or missing.",
            query.name, query.key
        ),
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
    use undra_meta::FieldDef;

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

    #[test]
    fn query_status_ids_are_the_documented_hashes() {
        assert_eq!(QUERY_REFETCH_ID, ids::fnv1a32("query.refetch"));
        assert_eq!(QUERY_INVALIDATE_ID, ids::fnv1a32("query.invalidate"));
        assert_ne!(QUERY_REFETCH_ID, QUERY_INVALIDATE_ID);
    }
}
