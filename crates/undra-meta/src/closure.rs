//! Type closures and fingerprints (ADR-037): the identity of persisted data.
//!
//! A value an older build wrote to disk (a snapshot, a cached query result, a queued mutation's
//! input) can only be read back if the reader knows the structure it was written with. The whole
//! schema hash is too coarse for that (adding an unrelated method changes it) and says nothing
//! about the old structure. A [`TypeClosure`] is the part of the schema one persisted item
//! depends on: its root (a type, a parameter list or a store's signals) plus every record and enum
//! it reaches, transitively, in a canonical form. Its [`fingerprint`](TypeClosure::fingerprint)
//! moves exactly when the encoded structure of the item can have changed:
//!
//! * field and variant **names**, **order** and **types**, variant **indices**, tuple-ness,
//!   `#[undra(default)]` flags and, for a store, signal names, ids and types are covered;
//! * doc comments, error messages, type ids (derived from names), the store's computed signals,
//!   its methods, and everything in the schema the item does not reach are **not**.
//!
//! The closure's canonical JSON is the description persisted next to the data (snapshot layout 2,
//! the `undra.types.<fingerprint>` keys of `undra-query`), so a later build can decode the old
//! bytes by name and migrate them. [`StoresClosure`] is the multi-store form a snapshot carries.
//!
//! Canonical form: the JSON `serde_json` writes for these types (no whitespace, struct fields in
//! declaration order), records and enums sorted by name, variants by index; a field's or signal's
//! `default` flag is written only when it is `true`. It is written and read by hand
//! (`closure_json.rs`), because every core does both and `serde`'s code for them is large.

use serde::{Deserialize, Serialize};

use crate::ids::fnv1a64;
use crate::{FieldDef, ParamDef, QueryKind, Schema, TypeRef};

/// What a [`TypeClosure`] describes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClosureRoot {
    /// One type: a query's cached value, the target of a `ty = ".."` migration hook.
    Type {
        /// The type.
        ty: TypeRef,
    },
    /// Named parameters, in order: a mutation's input.
    Params {
        /// The parameters.
        params: Vec<ClosureField>,
    },
    /// The non-computed signals of a store, in `signal_id` order.
    Signals {
        /// The signals.
        signals: Vec<ClosureSignal>,
    },
}

/// A field of a record or variant, or a parameter, as a closure records it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosureField {
    /// The name (a tuple variant's fields are `"0"`, `"1"`, ..).
    pub name: String,
    /// The type.
    pub ty: TypeRef,
    /// `#[undra(default)]`: a structural migration may fill the field when the old value lacks it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub default: bool,
}

/// One plain (non-computed) signal of a store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosureSignal {
    /// The field name.
    pub name: String,
    /// The signal's id (its index among the store's signal fields).
    pub signal_id: u32,
    /// The value type.
    pub ty: TypeRef,
    /// `#[undra(default)]` on the `Signal<T>` field: a restore that finds the signal missing
    /// fills it with `T::default()`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub default: bool,
}

/// A record reached by a closure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosureRecord {
    /// The record's name.
    pub name: String,
    /// Its fields, in wire order.
    pub fields: Vec<ClosureField>,
    /// A newtype (ADR-042): it crosses as its one field, byte for byte, so a migration may
    /// wrap a value of the field's type in it or unwrap one. Written only when `true`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub transparent: bool,
}

/// An enum (or error) reached by a closure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosureEnum {
    /// The enum's name.
    pub name: String,
    /// Its variants, sorted by index.
    pub variants: Vec<ClosureVariant>,
}

/// A variant of a [`ClosureEnum`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosureVariant {
    /// The variant's name.
    pub name: String,
    /// Its wire index.
    pub index: u16,
    /// Its payload, in order (empty for a unit variant).
    pub fields: Vec<ClosureField>,
    /// Whether the payload is positional.
    pub tuple: bool,
}

/// The part of a schema one persisted item depends on (ADR-037): a root and every record and
/// enum it reaches.
///
/// ```
/// use undra_meta::{RecordDef, FieldDef, Schema, TypeRef};
///
/// let mut schema = Schema::new("demo");
/// schema.records.push(RecordDef {
///     name: "Todo".into(),
///     type_id: undra_meta::ids::type_id("Todo"),
///     fields: vec![FieldDef { name: "title".into(), ty: TypeRef::String, default: false, docs: "".into() }],
///     transparent: false, docs: "A thing to do.".into(),
/// });
/// let before = schema.closure(&TypeRef::vec(TypeRef::named("Todo"))).fingerprint();
///
/// // Docs and unrelated items do not move it ...
/// schema.records[0].docs.clear();
/// schema.records.push(RecordDef { name: "Other".into(), type_id: 1, fields: vec![], transparent: false, docs: "".into() });
/// assert_eq!(schema.closure(&TypeRef::vec(TypeRef::named("Todo"))).fingerprint(), before);
///
/// // ... a structural change does.
/// schema.records[0].fields[0].ty = TypeRef::Bytes;
/// assert_ne!(schema.closure(&TypeRef::vec(TypeRef::named("Todo"))).fingerprint(), before);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeClosure {
    /// What the closure describes.
    pub root: ClosureRoot,
    /// Every record the root reaches, sorted by name.
    pub records: Vec<ClosureRecord>,
    /// Every enum the root reaches, sorted by name.
    pub enums: Vec<ClosureEnum>,
}

impl TypeClosure {
    /// The canonical JSON the fingerprint is computed over, and what is persisted next to data.
    #[must_use]
    pub fn canonical_json(&self) -> String {
        crate::closure_json::write_type_closure(self)
    }

    /// `fnv1a64` of [`canonical_json`](Self::canonical_json).
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        fnv1a64(self.canonical_json().as_bytes())
    }

    /// Parses a persisted closure (any JSON of its shape: whitespace and key order are free,
    /// unknown keys are ignored).
    ///
    /// # Errors
    ///
    /// [`ClosureJsonError`](crate::ClosureJsonError) for anything that is not a closure's JSON.
    pub fn from_json(json: &str) -> Result<TypeClosure, crate::ClosureJsonError> {
        crate::closure_json::read_type_closure(json)
    }

    /// The record called `name`, if the closure reaches one.
    #[must_use]
    pub fn record(&self, name: &str) -> Option<&ClosureRecord> {
        self.records
            .binary_search_by(|r| r.name.as_str().cmp(name))
            .ok()
            .map(|at| &self.records[at])
    }

    /// The enum called `name`, if the closure reaches one.
    #[must_use]
    pub fn enum_def(&self, name: &str) -> Option<&ClosureEnum> {
        self.enums
            .binary_search_by(|e| e.name.as_str().cmp(name))
            .ok()
            .map(|at| &self.enums[at])
    }

    /// The closure of one type inside this one (the same records and enums, narrowed to what
    /// `ty` reaches): what a `ty = ".."` hook's `from` fingerprint is computed over.
    #[must_use]
    pub fn narrowed(&self, ty: &TypeRef) -> TypeClosure {
        let mut collector = Collector::default();
        collector.reach_type(ty, &|name| self.lookup(name));
        collector.finish(ClosureRoot::Type { ty: ty.clone() })
    }

    fn lookup(&self, name: &str) -> Option<Reached> {
        if let Some(record) = self.record(name) {
            return Some(Reached::Record(record.clone()));
        }
        self.enum_def(name).map(|e| Reached::Enum(e.clone()))
    }
}

/// The description a snapshot carries (ADR-037): every snapshotted store type's signals and the
/// records and enums they reach, once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoresClosure {
    /// The store types, sorted by name.
    pub stores: Vec<DescribedStore>,
    /// Every record the stores reach, sorted by name.
    pub records: Vec<ClosureRecord>,
    /// Every enum the stores reach, sorted by name.
    pub enums: Vec<ClosureEnum>,
}

/// One store type of a [`StoresClosure`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DescribedStore {
    /// `fnv1a32(name)`.
    pub type_id: u32,
    /// The store type's name.
    pub name: String,
    /// Its plain signals, in `signal_id` order.
    pub signals: Vec<ClosureSignal>,
}

impl StoresClosure {
    /// The canonical JSON: the description bytes of a snapshot.
    #[must_use]
    pub fn canonical_json(&self) -> String {
        crate::closure_json::write_stores_closure(self)
    }

    /// Parses a snapshot's description.
    ///
    /// # Errors
    ///
    /// [`ClosureJsonError`](crate::ClosureJsonError) for anything that is not a description.
    pub fn from_json(json: &str) -> Result<StoresClosure, crate::ClosureJsonError> {
        crate::closure_json::read_stores_closure(json)
    }

    /// The described store with `type_id`.
    #[must_use]
    pub fn store(&self, type_id: u32) -> Option<&DescribedStore> {
        self.stores.iter().find(|s| s.type_id == type_id)
    }

    /// The closure of one described store: its signals and what they reach. Its fingerprint is
    /// the one the writing build computed for the store (so a description can be checked against
    /// a snapshot's type table).
    #[must_use]
    pub fn closure_of(&self, type_id: u32) -> Option<TypeClosure> {
        let store = self.store(type_id)?;
        let lookup = |name: &str| {
            if let Ok(at) = self.records.binary_search_by(|r| r.name.as_str().cmp(name)) {
                return Some(Reached::Record(self.records[at].clone()));
            }
            self.enums
                .binary_search_by(|e| e.name.as_str().cmp(name))
                .ok()
                .map(|at| Reached::Enum(self.enums[at].clone()))
        };
        let mut collector = Collector::default();
        for signal in &store.signals {
            collector.reach_type(&signal.ty, &lookup);
        }
        Some(collector.finish(ClosureRoot::Signals {
            signals: store.signals.clone(),
        }))
    }
}

impl Schema {
    /// The closure of `ty`: the type and every record and enum it reaches (ADR-037). A `Named`
    /// type the schema does not define as a record or an enum (an object, a typo) reaches nothing.
    #[must_use]
    pub fn closure(&self, ty: &TypeRef) -> TypeClosure {
        let mut collector = Collector::default();
        collector.reach_type(ty, &|name| self.reached(name));
        collector.finish(ClosureRoot::Type { ty: ty.clone() })
    }

    /// The closure of a parameter list (a mutation's input), by name and in order.
    #[must_use]
    pub fn closure_of_params(&self, params: &[ParamDef]) -> TypeClosure {
        let mut collector = Collector::default();
        for param in params {
            collector.reach_type(&param.ty, &|name| self.reached(name));
        }
        collector.finish(ClosureRoot::Params {
            params: params
                .iter()
                .map(|p| ClosureField {
                    name: p.name.clone(),
                    ty: p.ty.clone(),
                    default: false,
                })
                .collect(),
        })
    }

    /// The closure of the store type `type_id`: its non-computed signals by name, type and id,
    /// and what they reach. `None` if the schema has no store with that id.
    #[must_use]
    pub fn store_closure(&self, type_id: u32) -> Option<TypeClosure> {
        let signals = self.plain_signals(type_id)?;
        let mut collector = Collector::default();
        for signal in &signals {
            collector.reach_type(&signal.ty, &|name| self.reached(name));
        }
        Some(collector.finish(ClosureRoot::Signals { signals }))
    }

    /// The fingerprint of the store type `type_id` ([`Schema::store_closure`]).
    #[must_use]
    pub fn store_fingerprint(&self, type_id: u32) -> Option<u64> {
        self.store_closure(type_id).map(|c| c.fingerprint())
    }

    /// The closure of what a query caches: its success type (the `T` of `T` or `Result<T, E>`).
    /// `None` if `query_id` is not a query.
    #[must_use]
    pub fn query_closure(&self, query_id: u32) -> Option<TypeClosure> {
        let query = self
            .queries
            .iter()
            .find(|q| q.query_id == query_id && q.kind == QueryKind::Query)?;
        let ty = match &query.returns {
            TypeRef::Result(ok, _) => ok.as_ref(),
            other => other,
        };
        Some(self.closure(ty))
    }

    /// The closure of a mutation's input (its parameters). `None` if `mutation_id` is not a
    /// mutation.
    #[must_use]
    pub fn mutation_closure(&self, mutation_id: u32) -> Option<TypeClosure> {
        let mutation = self
            .queries
            .iter()
            .find(|q| q.query_id == mutation_id && q.kind == QueryKind::Mutation)?;
        Some(self.closure_of_params(&mutation.params))
    }

    /// The description of the store types `type_ids` (unknown ids are skipped): what a snapshot
    /// carries next to its type table.
    #[must_use]
    pub fn stores_closure(&self, type_ids: &[u32]) -> StoresClosure {
        let mut collector = Collector::default();
        let mut stores = Vec::new();
        for (at, &type_id) in type_ids.iter().enumerate() {
            if type_ids[..at].contains(&type_id) {
                continue;
            }
            let Some(object) = self
                .objects
                .iter()
                .find(|o| o.type_id == type_id && o.store.is_some())
            else {
                continue;
            };
            let signals = self.plain_signals(type_id).unwrap_or_default();
            for signal in &signals {
                collector.reach_type(&signal.ty, &|name| self.reached(name));
            }
            stores.push(DescribedStore {
                type_id,
                name: object.name.clone(),
                signals,
            });
        }
        crate::sort::insertion_by_name(&mut stores, |s| &s.name);
        let (records, enums) = collector.into_parts();
        StoresClosure {
            stores,
            records,
            enums,
        }
    }

    fn plain_signals(&self, type_id: u32) -> Option<Vec<ClosureSignal>> {
        let store = self
            .objects
            .iter()
            .find(|o| o.type_id == type_id)?
            .store
            .as_ref()?;
        let mut signals: Vec<ClosureSignal> = store
            .signals
            .iter()
            .filter(|s| !s.computed)
            .map(|s| ClosureSignal {
                name: s.name.clone(),
                signal_id: s.signal_id,
                ty: s.ty.clone(),
                default: s.default,
            })
            .collect();
        crate::sort::insertion_by_key(&mut signals, |s| s.signal_id);
        Some(signals)
    }

    fn reached(&self, name: &str) -> Option<Reached> {
        if let Some(record) = self.records.iter().find(|r| r.name == name) {
            return Some(Reached::Record(ClosureRecord {
                name: record.name.clone(),
                fields: fields(&record.fields),
                transparent: record.transparent,
            }));
        }
        let en = self.enums.iter().find(|e| e.name == name)?;
        let mut variants: Vec<ClosureVariant> = en
            .variants
            .iter()
            .map(|v| ClosureVariant {
                name: v.name.clone(),
                index: v.index,
                fields: fields(&v.fields),
                tuple: v.tuple,
            })
            .collect();
        crate::sort::insertion_by_key(&mut variants, |v| v.index);
        Some(Reached::Enum(ClosureEnum {
            name: en.name.clone(),
            variants,
        }))
    }
}

fn fields(defs: &[FieldDef]) -> Vec<ClosureField> {
    defs.iter()
        .map(|f| ClosureField {
            name: f.name.clone(),
            ty: f.ty.clone(),
            default: f.default,
        })
        .collect()
}

/// A named type a closure reached.
enum Reached {
    Record(ClosureRecord),
    Enum(ClosureEnum),
}

/// Walks types and keeps every record and enum it meets once, each list sorted by name (kept
/// sorted as it grows: a closure reaches a handful of types, and this needs no map or sort code).
#[derive(Default)]
struct Collector {
    records: Vec<ClosureRecord>,
    enums: Vec<ClosureEnum>,
}

impl Collector {
    fn reach_type(&mut self, ty: &TypeRef, lookup: &dyn Fn(&str) -> Option<Reached>) {
        // An explicit stack, not recursion: a recursive record must not overflow, and each name
        // is expanded once.
        let mut owned: Vec<TypeRef> = vec![ty.clone()];
        while let Some(next) = owned.pop() {
            match &next {
                TypeRef::Option(inner) | TypeRef::Vec(inner) | TypeRef::Lazy(inner) => {
                    owned.push((**inner).clone());
                }
                TypeRef::Stream(inner) => owned.push((**inner).clone()),
                TypeRef::Map(k, v) | TypeRef::Result(k, v) => {
                    owned.push((**k).clone());
                    owned.push((**v).clone());
                }
                TypeRef::Named(name) => {
                    let record_at = self.records.binary_search_by(|r| r.name.cmp(name));
                    let enum_at = self.enums.binary_search_by(|e| e.name.cmp(name));
                    let (Err(record_at), Err(enum_at)) = (record_at, enum_at) else {
                        continue;
                    };
                    match lookup(name) {
                        Some(Reached::Record(record)) => {
                            for field in &record.fields {
                                owned.push(field.ty.clone());
                            }
                            self.records.insert(record_at, record);
                        }
                        Some(Reached::Enum(en)) => {
                            for variant in &en.variants {
                                for field in &variant.fields {
                                    owned.push(field.ty.clone());
                                }
                            }
                            self.enums.insert(enum_at, en);
                        }
                        None => {}
                    }
                }
                _ => {}
            }
        }
    }

    fn into_parts(self) -> (Vec<ClosureRecord>, Vec<ClosureEnum>) {
        (self.records, self.enums)
    }

    fn finish(self, root: ClosureRoot) -> TypeClosure {
        let (records, enums) = self.into_parts();
        TypeClosure {
            root,
            records,
            enums,
        }
    }
}

/// `skip_serializing_if` for flags written only when set.
#[allow(clippy::trivially_copy_pass_by_ref)] // serde passes a reference
fn is_false(flag: &bool) -> bool {
    !*flag
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnumDef, ObjectDef, QueryDef, RecordDef, SignalDef, StoreDef, VariantDef, ids};

    fn field(name: &str, ty: TypeRef) -> FieldDef {
        FieldDef {
            name: name.into(),
            ty,
            default: false,
            docs: String::new(),
        }
    }

    fn record(name: &str, fields: Vec<FieldDef>) -> RecordDef {
        RecordDef {
            name: name.into(),
            type_id: ids::type_id(name),
            fields,
            transparent: false,
            docs: String::new(),
        }
    }

    fn signal(name: &str, id: u32, ty: TypeRef, computed: bool) -> SignalDef {
        SignalDef {
            name: name.into(),
            signal_id: id,
            ty,
            computed,
            key: None,
            no_coalesce: false,
            default: false,
        }
    }

    fn variant(name: &str, index: u16, fields: Vec<FieldDef>) -> VariantDef {
        VariantDef {
            name: name.into(),
            index,
            fields,
            tuple: false,
            message: None,
            docs: String::new(),
        }
    }

    /// `Profile { age: Signal<i32>, tier: Signal<Tier>, todos: Signal<Vec<Todo>>, n: Computed<u32> }`.
    fn schema() -> Schema {
        let mut s = Schema::new("t");
        s.records.push(record(
            "Todo",
            vec![
                field("id", TypeRef::Uuid),
                field("title", TypeRef::String),
                field("tags", TypeRef::vec(TypeRef::named("Tag"))),
            ],
        ));
        s.records
            .push(record("Tag", vec![field("name", TypeRef::String)]));
        s.records
            .push(record("Unrelated", vec![field("x", TypeRef::I8)]));
        s.enums.push(EnumDef {
            name: "Tier".into(),
            type_id: ids::type_id("Tier"),
            is_error: false,
            variants: vec![variant("Free", 0, vec![]), variant("Pro", 1, vec![])],
            docs: String::new(),
        });
        s.objects.push(ObjectDef {
            name: "Profile".into(),
            type_id: ids::type_id("Profile"),
            constructors: vec![],
            methods: vec![],
            store: Some(StoreDef {
                signals: vec![
                    signal("age", 0, TypeRef::I32, false),
                    signal("tier", 1, TypeRef::named("Tier"), false),
                    signal("todos", 2, TypeRef::vec(TypeRef::named("Todo")), false),
                    signal("n", 3, TypeRef::U32, true),
                ],
            }),
            docs: String::new(),
        });
        s
    }

    fn profile(s: &Schema) -> u64 {
        s.store_fingerprint(ids::type_id("Profile")).unwrap()
    }

    #[test]
    fn a_closure_reaches_records_and_enums_transitively_and_nothing_else() {
        let s = schema();
        let c = s.store_closure(ids::type_id("Profile")).unwrap();
        let names: Vec<&str> = c.records.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Tag", "Todo"], "sorted, Unrelated left out");
        assert_eq!(c.enums.len(), 1);
        let ClosureRoot::Signals { signals } = &c.root else {
            panic!("{:?}", c.root)
        };
        assert_eq!(
            signals.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["age", "tier", "todos"],
            "computed signals are not persisted"
        );
        assert!(c.record("Todo").is_some() && c.record("Unrelated").is_none());
        assert!(c.enum_def("Tier").is_some());
    }

    #[test]
    fn docs_messages_computed_signals_and_unrelated_items_do_not_move_a_fingerprint() {
        let base = profile(&schema());
        let mut s = schema();
        s.records[0].docs = "changed".into();
        s.records[0].fields[0].docs = "changed".into();
        s.records[2].fields.push(field("y", TypeRef::I8));
        s.enums[0].variants[0].message = Some("free".into());
        s.objects[0].docs = "changed".into();
        s.objects[0].store.as_mut().unwrap().signals[3].ty = TypeRef::U64;
        s.queries.push(QueryDef {
            name: "q".into(),
            query_id: 1,
            kind: QueryKind::Query,
            key: "q".into(),
            params: vec![],
            returns: TypeRef::String,
            stale_ms: None,
            persist: false,
            idempotent: false,
            interval_ms: None,
            poll_in_background: false,
            infinite: None,
        });
        assert_eq!(profile(&s), base);
    }

    #[test]
    fn every_structural_change_moves_a_fingerprint() {
        let base = profile(&schema());
        type Change = Box<dyn Fn(&mut Schema)>;
        let changes: Vec<(&str, Change)> = vec![
            (
                "a field type",
                Box::new(|s| s.records[0].fields[1].ty = TypeRef::Bytes),
            ),
            (
                "a field name",
                Box::new(|s| s.records[0].fields[1].name = "name".into()),
            ),
            ("field order", Box::new(|s| s.records[0].fields.swap(0, 1))),
            (
                "an added field",
                Box::new(|s| s.records[0].fields.push(field("done", TypeRef::Bool))),
            ),
            (
                "a default flag",
                Box::new(|s| s.records[0].fields[1].default = true),
            ),
            (
                "a nested record",
                Box::new(|s| s.records[1].fields[0].ty = TypeRef::Bytes),
            ),
            (
                "a variant index",
                Box::new(|s| s.enums[0].variants[1].index = 5),
            ),
            (
                "a variant name",
                Box::new(|s| s.enums[0].variants[1].name = "Plus".into()),
            ),
            (
                "an added variant",
                Box::new(|s| s.enums[0].variants.push(variant("Team", 2, vec![]))),
            ),
            (
                "a signal type",
                Box::new(|s| s.objects[0].store.as_mut().unwrap().signals[0].ty = TypeRef::F32),
            ),
            (
                "a signal name",
                Box::new(|s| s.objects[0].store.as_mut().unwrap().signals[0].name = "years".into()),
            ),
            (
                "a signal id",
                Box::new(|s| s.objects[0].store.as_mut().unwrap().signals[0].signal_id = 9),
            ),
            (
                "a signal default",
                Box::new(|s| s.objects[0].store.as_mut().unwrap().signals[0].default = true),
            ),
            (
                "a computed made plain",
                Box::new(|s| s.objects[0].store.as_mut().unwrap().signals[3].computed = false),
            ),
        ];
        for (what, change) in changes {
            let mut s = schema();
            change(&mut s);
            assert_ne!(profile(&s), base, "{what}");
        }
    }

    #[test]
    fn the_canonical_form_is_independent_of_declaration_order_of_unordered_things() {
        let mut s = schema();
        s.records.reverse();
        s.enums[0].variants.reverse();
        assert_eq!(profile(&s), profile(&schema()));
    }

    #[test]
    fn a_closure_round_trips_through_its_json() {
        let c = schema().store_closure(ids::type_id("Profile")).unwrap();
        let back = TypeClosure::from_json(&c.canonical_json()).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.fingerprint(), c.fingerprint());
        assert!(
            !c.canonical_json().contains("default"),
            "false flags are not written"
        );
    }

    #[test]
    fn a_stores_description_gives_back_each_store_closure_with_its_fingerprint() {
        let s = schema();
        let id = ids::type_id("Profile");
        let description = s.stores_closure(&[id, id, 12345]);
        assert_eq!(
            description.stores.len(),
            1,
            "unknown ids skipped, duplicates once"
        );
        let back = StoresClosure::from_json(&description.canonical_json()).unwrap();
        assert_eq!(back, description);
        let closure = back.closure_of(id).unwrap();
        assert_eq!(closure.fingerprint(), profile(&s));
        assert!(back.closure_of(1).is_none());
    }

    #[test]
    fn query_and_mutation_closures() {
        let mut s = schema();
        let q = |kind, id, returns, params| QueryDef {
            name: "x".into(),
            query_id: id,
            kind,
            key: "x".into(),
            params,
            returns,
            stale_ms: None,
            persist: true,
            idempotent: false,
            interval_ms: None,
            poll_in_background: false,
            infinite: None,
        };
        s.queries.push(q(
            QueryKind::Query,
            1,
            TypeRef::result(TypeRef::vec(TypeRef::named("Todo")), TypeRef::named("E")),
            vec![],
        ));
        s.queries.push(q(
            QueryKind::Mutation,
            2,
            TypeRef::Unit,
            vec![ParamDef {
                name: "todo".into(),
                ty: TypeRef::named("Todo"),
            }],
        ));
        let query = s.query_closure(1).unwrap();
        assert_eq!(
            query.root,
            ClosureRoot::Type {
                ty: TypeRef::vec(TypeRef::named("Todo"))
            },
            "the cached value is the success type"
        );
        assert_eq!(query, s.closure(&TypeRef::vec(TypeRef::named("Todo"))));
        let mutation = s.mutation_closure(2).unwrap();
        assert!(
            matches!(&mutation.root, ClosureRoot::Params { params } if params[0].name == "todo")
        );
        assert!(s.query_closure(2).is_none() && s.mutation_closure(1).is_none());
        // A narrowed closure is the closure of that type alone.
        assert_eq!(
            mutation.narrowed(&TypeRef::named("Todo")).fingerprint(),
            s.closure(&TypeRef::named("Todo")).fingerprint()
        );
    }

    #[test]
    fn a_recursive_record_terminates() {
        let mut s = Schema::new("t");
        s.records.push(record(
            "Node",
            vec![field("next", TypeRef::option(TypeRef::named("Node")))],
        ));
        let c = s.closure(&TypeRef::named("Node"));
        assert_eq!(c.records.len(), 1);
    }

    #[test]
    fn a_newtype_is_described_as_transparent_and_moves_the_fingerprint() {
        let mut s = Schema::new("t");
        s.records.push(record(
            "Todo",
            vec![field("owner", TypeRef::named("UserId"))],
        ));
        s.records
            .push(record("UserId", vec![field("value", TypeRef::Uuid)]));
        let flat = s.closure(&TypeRef::named("Todo"));
        assert!(flat.records.iter().all(|r| !r.transparent));
        s.records[1].transparent = true;
        let wrapped = s.closure(&TypeRef::named("Todo"));
        assert!(wrapped.record("UserId").unwrap().transparent);
        assert_ne!(flat.fingerprint(), wrapped.fingerprint());
        let text = wrapped.canonical_json();
        assert!(text.contains("\"transparent\":true"), "{text}");
        assert_eq!(TypeClosure::from_json(&text).unwrap(), wrapped);
        assert!(!flat.canonical_json().contains("transparent"));
    }
}
