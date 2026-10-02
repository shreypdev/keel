//! JSON forms of a [`Schema`], the canonical form and the schema hash
//! (SPEC §2.3).
//!
//! * [`Schema::to_json`], [`Schema::to_json_pretty`] and [`Schema::from_json`]
//!   are the exchange format: the same document, compact or indented
//!   (`schema.json` files, the C ABI's `undra_schema_json`). They carry
//!   everything, including the `undra_version` and `crate_name` labels and
//!   docs when present, in declaration order.
//! * [`Schema::canonical_json`] is the form the hash is computed over. It
//!   contains only the six top-level lists (`records`, `enums`, `objects`,
//!   `functions`, `ports`, `queries`), with docs stripped, no whitespace and
//!   struct fields in declaration order. `undra_version` and `crate_name` are
//!   labels: they do not change the wire, so they are excluded.
//! * [`Schema::hash`] is `fnv1a64` of the canonical bytes.
//!
//! # What is sorted and what is not
//!
//! *Unordered* things are sorted by name so that source order never changes the
//! hash: the six top-level lists, the `constructors` and `methods` of an
//! object, and the `methods` of a port. Enum variants are sorted by `index`.
//!
//! *Ordered* things are part of the wire layout and keep their declared order:
//! record fields, variant fields, parameters and signals (a `signal_id` is its
//! index).

#[cfg(test)]
use serde::Serialize;

use crate::ids::fnv1a64;
#[cfg(test)]
use crate::{EnumDef, FunctionDef, ObjectDef, PortDef, QueryDef, RecordDef};
use crate::{Schema, VariantDef};

/// The canonical JSON as `serde` writes it from a doc-stripped, sorted clone: the schema
/// without its labels, field order the key order. This is how the canonical form was produced
/// before [`crate::schema_json`] wrote it straight from the schema; the tests keep it as the
/// oracle the writer is compared with.
#[cfg(test)]
#[derive(Serialize)]
struct Canonical<'a> {
    records: &'a [RecordDef],
    enums: &'a [EnumDef],
    objects: &'a [ObjectDef],
    functions: &'a [FunctionDef],
    ports: &'a [PortDef],
    queries: &'a [QueryDef],
}

/// The whole schema, borrowed: what [`Schema::to_json`] serialized through `serde_json` before
/// [`crate::schema_json`] wrote it. The same document as `Schema`'s own derived `Serialize` (the
/// same keys in the same order); the tests keep it as the writer's oracle.
#[cfg(test)]
#[derive(Serialize)]
struct Document<'a> {
    undra_version: &'a str,
    crate_name: &'a str,
    records: &'a [RecordDef],
    enums: &'a [EnumDef],
    objects: &'a [ObjectDef],
    functions: &'a [FunctionDef],
    ports: &'a [PortDef],
    queries: &'a [QueryDef],
}

impl Schema {
    /// The canonical JSON the schema hash is computed over (SPEC §2.3).
    ///
    /// Two schemas with the same public surface produce identical output
    /// regardless of doc comments, of the `crate_name` and `undra_version`
    /// labels (which are omitted), or of the source order of unordered items:
    /// the six top-level lists, object constructors and methods, and port
    /// methods are sorted by name and enum variants by index, while record and
    /// variant fields, parameters and signals keep their declared order.
    ///
    /// The result is not a full [`Schema`] document and cannot be read back
    /// with [`Schema::from_json`]; it exists to be hashed and compared.
    ///
    /// ```
    /// use undra_meta::Schema;
    ///
    /// let schema = Schema::new("demo");
    /// assert_eq!(
    ///     schema.canonical_json(),
    ///     r#"{"records":[],"enums":[],"objects":[],"functions":[],"ports":[],"queries":[]}"#
    /// );
    /// ```
    #[must_use]
    pub fn canonical_json(&self) -> String {
        // Written straight from `self`, in canonical order: no clone, no `serde` (every core
        // computes this when it starts; see `crate::schema_json`).
        crate::schema_json::canonical(self)
    }

    /// [`Schema::canonical_json`] as it was computed before the writer: `serde_json` on a
    /// doc-stripped, sorted clone. The oracle of the writer's tests.
    #[cfg(test)]
    pub(crate) fn canonical_json_by_serde(&self) -> String {
        let s = self.canonicalized();
        let canonical = Canonical {
            records: &s.records,
            enums: &s.enums,
            objects: &s.objects,
            functions: &s.functions,
            ports: &s.ports,
            queries: &s.queries,
        };
        serde_json::to_string(&canonical).expect("schema types always serialize to JSON")
    }

    /// The schema hash: `fnv1a64(canonical_json())` (SPEC §1.1).
    ///
    /// The `crate_name` and `undra_version` labels do not contribute:
    ///
    /// ```
    /// use undra_meta::Schema;
    ///
    /// let a = Schema::new("demo");
    /// let b = Schema::new("renamed");
    /// assert_eq!(a.hash(), b.hash());
    /// ```
    #[must_use]
    pub fn hash(&self) -> u64 {
        fnv1a64(self.canonical_json().as_bytes())
    }

    /// Compact JSON with everything: the labels, the docs and every list in
    /// declaration order. This is what the C ABI's `undra_schema_json` returns,
    /// so a tool that loads a built core (`undra bindgen`) sees the same schema,
    /// documentation included, that the registrations hold.
    ///
    /// The output is *not* canonical (the hash does not cover docs or labels,
    /// and unordered lists keep the order they were declared in): hash through
    /// [`Schema::hash`], which canonicalizes first. [`Schema::from_json`] reads
    /// it back.
    ///
    /// ```
    /// use undra_meta::Schema;
    ///
    /// let schema = Schema::new("demo");
    /// let back = Schema::from_json(&schema.to_json()).unwrap();
    /// assert_eq!(back, schema);
    /// assert_eq!(back.hash(), schema.hash());
    /// ```
    #[must_use]
    pub fn to_json(&self) -> String {
        // The same writer as the canonical form, so a core links one (`crate::schema_json`).
        crate::schema_json::exchange(self)
    }

    /// [`Schema::to_json`] as `serde_json` wrote it before the writer: its oracle in the tests.
    #[cfg(test)]
    pub(crate) fn to_json_by_serde(&self) -> String {
        let document = Document {
            undra_version: &self.undra_version,
            crate_name: &self.crate_name,
            records: &self.records,
            enums: &self.enums,
            objects: &self.objects,
            functions: &self.functions,
            ports: &self.ports,
            queries: &self.queries,
        };
        // Serializing these types cannot fail: they contain only strings,
        // integers, booleans, options and sequences, and the writer is an
        // in-memory `String`.
        serde_json::to_string(&document).expect("schema types always serialize to JSON")
    }

    /// Pretty-printed JSON, with labels and docs, for humans and `schema.json`
    /// files. The same document as [`Schema::to_json`], indented.
    ///
    /// The output is *not* canonical; use [`Schema::canonical_json`] for
    /// hashing and comparisons.
    #[must_use]
    pub fn to_json_pretty(&self) -> String {
        // Serializing these types cannot fail, as above.
        serde_json::to_string_pretty(self).expect("schema types always serialize to JSON")
    }

    /// Parses a schema from JSON as produced by [`Schema::to_json_pretty`]
    /// (or `serde_json::to_string` on a [`Schema`]).
    ///
    /// Missing `docs` become empty strings. Unknown fields are ignored.
    /// This only checks the JSON shape; call [`Schema::validate`] for the
    /// semantic rules. The canonical form omits the labels and is not
    /// accepted.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`serde_json::Error`] (with line and column) for
    /// malformed JSON, missing fields, unknown type kinds, or nesting deeper
    /// than `serde_json`'s recursion limit.
    ///
    /// ```
    /// use undra_meta::Schema;
    ///
    /// let schema = Schema::new("demo");
    /// let back = Schema::from_json(&schema.to_json_pretty()).unwrap();
    /// assert_eq!(back, schema);
    /// assert!(Schema::from_json("{}").is_err());
    /// ```
    pub fn from_json(json: &str) -> Result<Schema, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// A clone with every doc comment removed. The schema hash does not cover
    /// docs, so the clone has the same [`Schema::hash`] as `self`; bindings
    /// generated from it carry no documentation.
    ///
    /// ```
    /// use undra_meta::{RecordDef, Schema};
    ///
    /// let mut schema = Schema::new("demo");
    /// schema.records.push(RecordDef {
    ///     name: "P".into(),
    ///     type_id: 1,
    ///     fields: vec![],
    ///     transparent: false, docs: "A point.".into(),
    /// });
    /// let bare = schema.without_docs();
    /// assert_eq!(bare.records[0].docs, "");
    /// assert_eq!(bare.hash(), schema.hash());
    /// ```
    #[must_use]
    pub fn without_docs(&self) -> Schema {
        let mut s = self.clone();

        for record in &mut s.records {
            record.docs.clear();
            for field in &mut record.fields {
                field.docs.clear();
            }
        }
        for en in &mut s.enums {
            en.docs.clear();
            for variant in &mut en.variants {
                strip_variant_docs(variant);
            }
        }
        for object in &mut s.objects {
            object.docs.clear();
            for method in object
                .constructors
                .iter_mut()
                .chain(object.methods.iter_mut())
            {
                method.docs.clear();
            }
        }
        for function in &mut s.functions {
            function.docs.clear();
        }
        for port in &mut s.ports {
            port.docs.clear();
            for method in &mut port.methods {
                method.docs.clear();
            }
        }
        s
    }

    /// A doc-stripped clone with every unordered list sorted: the value the
    /// canonical JSON describes (and was serialized from, see
    /// `canonical_json_by_serde`).
    #[cfg(test)]
    fn canonicalized(&self) -> Schema {
        let mut s = self.without_docs();

        for en in &mut s.enums {
            crate::sort::by_index(&mut en.variants, |v| v.index);
        }
        for object in &mut s.objects {
            crate::sort::by_name(&mut object.constructors, |m| &m.name);
            crate::sort::by_name(&mut object.methods, |m| &m.name);
        }
        for port in &mut s.ports {
            crate::sort::by_name(&mut port.methods, |m| &m.name);
        }

        crate::sort::by_name(&mut s.records, |d| &d.name);
        crate::sort::by_name(&mut s.enums, |d| &d.name);
        crate::sort::by_name(&mut s.objects, |d| &d.name);
        crate::sort::by_name(&mut s.functions, |d| &d.name);
        crate::sort::by_name(&mut s.ports, |d| &d.name);
        crate::sort::by_name(&mut s.queries, |d| &d.name);
        s
    }
}

fn strip_variant_docs(variant: &mut VariantDef) {
    variant.docs.clear();
    for field in &mut variant.fields {
        field.docs.clear();
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::{representative_schema, with_docs};
    use crate::{FieldDef, RecordDef, Schema, TypeRef};

    /// A named in-place edit of a schema.
    type Edit = (&'static str, Box<dyn Fn(&mut Schema)>);

    #[test]
    fn json_round_trip_of_representative_schema() {
        let schema = representative_schema();
        let pretty = schema.to_json_pretty();
        let back = Schema::from_json(&pretty).unwrap();
        assert_eq!(back, schema);
        assert_eq!(back.canonical_json(), schema.canonical_json());
        assert_eq!(back.hash(), schema.hash());
        // The compact serde form round-trips too.
        let compact = serde_json::to_string(&schema).unwrap();
        assert_eq!(Schema::from_json(&compact).unwrap(), schema);
    }

    #[test]
    fn round_trip_preserves_docs_and_labels_in_pretty_json() {
        let schema = with_docs(representative_schema());
        let back = Schema::from_json(&schema.to_json_pretty()).unwrap();
        assert_eq!(back, schema);
        assert!(back.records[0].docs.contains("todo"));
        assert_eq!(back.crate_name, "playground-core");
        assert_eq!(back.undra_version, crate::UNDRA_VERSION);
    }

    #[test]
    fn to_json_is_the_full_document_compact() {
        let schema = with_docs(representative_schema());
        let json = schema.to_json();
        assert!(
            !json.contains('\n') && json.starts_with(r#"{"undra_version":"#),
            "{json}"
        );
        // Labels and docs travel, in declaration order.
        assert!(json.contains(r#""crate_name":"playground-core""#), "{json}");
        assert!(json.contains("docs: todo item"), "{json}");
        assert_eq!(Schema::from_json(&json).unwrap(), schema);
        // It is the document `to_json_pretty` indents.
        assert_eq!(
            Schema::from_json(&json).unwrap(),
            Schema::from_json(&schema.to_json_pretty()).unwrap()
        );
        // Declaration order is kept (the canonical form would sort this).
        let mut reversed = schema.clone();
        reversed.records.reverse();
        assert_ne!(reversed.to_json(), json);
        assert_eq!(reversed.hash(), schema.hash());
    }

    #[test]
    fn to_json_is_exactly_what_schemas_derived_serialize_writes() {
        // `to_json` goes through a borrowed twin of `Schema` to share code with the canonical
        // form; the two must never drift apart (a field added to `Schema` and not to the twin).
        for schema in [
            Schema::new("empty"),
            representative_schema(),
            with_docs(representative_schema()),
        ] {
            assert_eq!(schema.to_json(), serde_json::to_string(&schema).unwrap());
        }
    }

    #[test]
    fn the_full_json_hashes_like_the_canonical_form() {
        // `undra_schema_json` carries docs and labels; the hash still covers neither
        // (ADR-025, SPEC 2.3), so it is the same through either door.
        let plain = representative_schema();
        let documented = with_docs(representative_schema());
        let read_back = Schema::from_json(&documented.to_json()).unwrap();
        assert_eq!(read_back.hash(), plain.hash());
        assert_eq!(read_back.canonical_json(), plain.canonical_json());
        assert!(documented.to_json().contains("docs"));
        assert!(!plain.canonical_json().contains("docs"));
    }

    #[test]
    fn without_docs_removes_every_doc_and_nothing_else() {
        // The representative schema documents a few items itself.
        let bare = representative_schema().without_docs();
        let documented = with_docs(representative_schema());
        assert_ne!(documented, bare);
        assert_eq!(documented.without_docs(), bare);
        assert!(!documented.without_docs().to_json().contains("docs"));
        assert_eq!(documented.without_docs().hash(), documented.hash());
        // The original is untouched.
        assert!(documented.records[0].docs.contains("todo"));
    }

    #[test]
    fn empty_docs_are_omitted_and_missing_docs_default_to_empty() {
        let mut schema = Schema::new("demo");
        schema.records.push(RecordDef {
            name: "P".into(),
            type_id: 1,
            fields: vec![FieldDef {
                name: "x".into(),
                ty: TypeRef::F64,
                default: false,
                docs: String::new(),
            }],
            transparent: false,
            docs: String::new(),
        });
        let json = schema.to_json_pretty();
        assert!(!json.contains("docs"), "{json}");
        let back = Schema::from_json(&json).unwrap();
        assert_eq!(back.records[0].docs, "");
        assert_eq!(back.records[0].fields[0].docs, "");
    }

    #[test]
    fn canonical_json_is_compact_docs_free_and_label_free() {
        let canonical = with_docs(representative_schema()).canonical_json();
        assert!(!canonical.contains("docs"));
        assert!(!canonical.contains('\n'));
        assert!(!canonical.contains(": "));
        assert!(!canonical.contains(", "));
        assert!(!canonical.contains("todo item"));
        assert!(!canonical.contains("undra_version"));
        assert!(!canonical.contains("crate_name"));
        assert!(!canonical.contains("playground-core"));
        assert!(canonical.starts_with(r#"{"records":["#), "{canonical}");
    }

    #[test]
    fn canonical_json_has_exactly_the_six_lists_in_order() {
        let value: serde_json::Value =
            serde_json::from_str(&representative_schema().canonical_json()).unwrap();
        let keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let mut sorted_keys = keys.clone();
        sorted_keys.sort_unstable();
        assert_eq!(
            sorted_keys,
            [
                "enums",
                "functions",
                "objects",
                "ports",
                "queries",
                "records"
            ]
        );
        // serde_json without `preserve_order` sorts object keys on parse, so
        // check the textual order too.
        let text = representative_schema().canonical_json();
        let positions: Vec<usize> = [
            "records",
            "enums",
            "objects",
            "functions",
            "ports",
            "queries",
        ]
        .iter()
        .map(|k| text.find(&format!(r#""{k}":["#)).unwrap())
        .collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{positions:?}");
    }

    #[test]
    fn canonical_json_sorts_unordered_lists_and_keeps_ordered_ones() {
        let value: serde_json::Value =
            serde_json::from_str(&representative_schema().canonical_json()).unwrap();
        let names = |v: &serde_json::Value| -> Vec<String> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|x| x["name"].as_str().unwrap().to_owned())
                .collect()
        };
        // Top-level lists are name-sorted.
        assert_eq!(
            names(&value["records"]),
            ["HttpRequest", "HttpResponse", "Page", "Todo"]
        );
        assert_eq!(names(&value["queries"]), ["add_todo", "todos"]);
        // Object methods and constructors are name-sorted...
        let calc = &value["objects"][0];
        assert_eq!(calc["name"], "Calculator");
        assert_eq!(
            names(&calc["methods"]),
            ["add", "fetch", "reset", "ticks", "watch"]
        );
        let store = &value["objects"][1];
        assert_eq!(names(&store["constructors"]), ["new", "open"]);
        // ...as are port methods.
        let clock = value["ports"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "Clock")
            .unwrap();
        assert_eq!(names(&clock["methods"]), ["monotonic_ns", "now_ms"]);
        // Ordered lists keep declaration order: params, signals, record fields.
        let add = &calc["methods"][0];
        assert_eq!(names(&add["params"]), ["a", "b"]);
        assert_eq!(
            names(&store["store"]["signals"]),
            ["todos", "filter", "remaining", "archive", "selected"]
        );
        let todo = &value["records"][3];
        assert_eq!(names(&todo["fields"])[..3], ["id", "title", "done"]);
    }

    #[test]
    fn canonical_json_golden() {
        let mut schema = Schema::new("golden");
        schema.records.push(RecordDef {
            name: "Todo".into(),
            type_id: 7,
            fields: vec![
                FieldDef {
                    name: "id".into(),
                    ty: TypeRef::Uuid,
                    default: false,
                    docs: "the id".into(),
                },
                FieldDef {
                    name: "tags".into(),
                    ty: TypeRef::map(TypeRef::String, TypeRef::option(TypeRef::named("Tag"))),
                    default: true,
                    docs: String::new(),
                },
            ],
            transparent: false,
            docs: "A todo.".into(),
        });
        assert_eq!(
            schema.canonical_json(),
            concat!(
                r#"{"records":["#,
                r#"{"name":"Todo","type_id":7,"fields":["#,
                r#"{"name":"id","ty":{"kind":"uuid"},"default":false},"#,
                r#"{"name":"tags","ty":{"kind":"map","of":[{"kind":"string"},"#,
                r#"{"kind":"option","of":{"kind":"named","of":"Tag"}}]},"default":true}"#,
                r#"]}],"enums":[],"objects":[],"functions":[],"ports":[],"queries":[]}"#,
            )
        );
    }

    #[test]
    fn hash_is_fnv1a64_of_canonical_json() {
        let schema = representative_schema();
        assert_eq!(
            schema.hash(),
            crate::ids::fnv1a64(schema.canonical_json().as_bytes())
        );
    }

    #[test]
    fn hash_golden_for_representative_schema() {
        // Locks the canonical form. If this fails the wire-compatibility hash
        // changed: that is a breaking change (CLAUDE.md R7) and needs an ADR.
        // The value was cross-checked with an independent FNV-1a
        // implementation over the canonical JSON bytes.
        assert_eq!(representative_schema().hash(), 0xd5b8_c3a3_afbd_bc33);
    }

    #[test]
    fn derived_signals_add_no_field() {
        // ADR-039 decision 7: a derived list is a store signal with `computed: true` and a key, a
        // combination E0008 refused until then. It needs no new field, so no existing schema
        // changes (the representative schema's hash is the golden above) and a derived signal's
        // canonical JSON ends exactly `"computed":true,"key":"id"}`.
        let mut schema = representative_schema();
        let store = schema
            .objects
            .iter_mut()
            .find_map(|o| o.store.as_mut())
            .unwrap();
        store.signals.push(crate::SignalDef {
            name: "visible".into(),
            signal_id: u32::try_from(store.signals.len()).unwrap(),
            ty: TypeRef::vec(TypeRef::named("Todo")),
            computed: true,
            key: Some("id".into()),
            no_coalesce: false,
            default: false,
        });
        let id = store.signals.len() - 1;
        let canonical = schema.canonical_json();
        let entry = format!(
            r#"{{"name":"visible","signal_id":{id},"ty":{{"kind":"vec","of":{{"kind":"named","of":"Todo"}}}},"computed":true,"key":"id"}}"#
        );
        assert!(canonical.contains(&entry), "{canonical}");
        assert_eq!(
            canonical.matches(r#""computed":true,"key":"id"}"#).count(),
            1
        );
        assert_eq!(Schema::from_json(&schema.to_json_pretty()).unwrap(), schema);
        // Adding it is a schema change of that store only; the representative schema without it
        // is untouched.
        assert_ne!(schema.hash(), representative_schema().hash());
        assert_eq!(representative_schema().hash(), 0xd5b8_c3a3_afbd_bc33);
    }

    #[test]
    fn objects_and_callbacks_add_variants_only_where_they_are_used() {
        // ADR-040 and ADR-041: `object` and `callback` type references and the `callback` port
        // kind are written only by a schema that uses them, so every existing schema (the
        // representative one, golden above) hashes as it did, and the new spellings are fixed.
        assert_eq!(representative_schema().hash(), 0xd5b8_c3a3_afbd_bc33);
        let mut schema = representative_schema();
        let owner = schema.objects[0].name.clone();
        schema.objects[0].methods.push(crate::fixtures::method(
            &owner,
            "child",
            vec![crate::fixtures::param("c", TypeRef::object("Calculator"))],
            TypeRef::option(TypeRef::object("Calculator")),
            false,
        ));
        schema.ports.push(crate::PortDef {
            name: "Listener".into(),
            port_id: crate::ids::port_id("Listener"),
            kind: crate::PortKind::Callback,
            background: false,
            methods: vec![],
            docs: String::new(),
        });
        let canonical = schema.canonical_json();
        assert!(
            canonical.contains(r#""ty":{"kind":"object","of":"Calculator"}"#),
            "{canonical}"
        );
        assert!(
            canonical.contains(
                r#""returns":{"kind":"option","of":{"kind":"object","of":"Calculator"}}"#
            ),
            "{canonical}"
        );
        assert!(
            canonical.contains(r#""kind":"callback","methods":[]"#),
            "{canonical}"
        );
        assert_ne!(schema.hash(), representative_schema().hash());
        assert_eq!(Schema::from_json(&schema.to_json_pretty()).unwrap(), schema);
    }

    #[test]
    fn no_coalesce_is_written_only_when_set() {
        // ADR-031 decision 6: a schema without a `no_coalesce` signal serializes, and so hashes,
        // exactly as it did before the field existed (the representative schema's hash is the
        // golden above); a signal with it carries `"no_coalesce":true` after `key`.
        let base = representative_schema();
        assert!(!base.canonical_json().contains("no_coalesce"));
        assert!(!base.to_json_pretty().contains("no_coalesce"));

        let mut flagged = base.clone();
        let store = flagged
            .objects
            .iter_mut()
            .find_map(|o| o.store.as_mut())
            .unwrap();
        store.signals[1].no_coalesce = true;
        let canonical = flagged.canonical_json();
        assert!(
            canonical.contains(r#""key":null,"no_coalesce":true}"#),
            "{canonical}"
        );
        assert_eq!(canonical.matches("no_coalesce").count(), 1);
        assert_ne!(flagged.hash(), base.hash());

        // Both forms read back; a missing key means `false`.
        assert_eq!(
            Schema::from_json(&flagged.to_json_pretty()).unwrap(),
            flagged
        );
        assert_eq!(Schema::from_json(&base.to_json_pretty()).unwrap(), base);
    }

    #[test]
    fn hash_ignores_docs() {
        assert_eq!(
            representative_schema().hash(),
            with_docs(representative_schema()).hash()
        );
    }

    #[test]
    fn hash_ignores_the_crate_name_and_undra_version_labels() {
        let base = representative_schema();
        let mut renamed = base.clone();
        renamed.crate_name = "some-other-crate".into();
        assert_eq!(base.hash(), renamed.hash());
        assert_eq!(base.canonical_json(), renamed.canonical_json());

        let mut reversioned = base.clone();
        reversioned.undra_version = "1.2.3".into();
        assert_eq!(base.hash(), reversioned.hash());
        assert_eq!(base.canonical_json(), reversioned.canonical_json());

        // The labels still travel in the exchange format.
        assert_ne!(base.to_json_pretty(), renamed.to_json_pretty());
        assert_ne!(base, renamed);
    }

    #[test]
    fn hash_ignores_top_level_list_order() {
        let base = representative_schema();
        let mut shuffled = base.clone();
        shuffled.records.reverse();
        shuffled.enums.reverse();
        shuffled.objects.reverse();
        shuffled.functions.reverse();
        shuffled.ports.reverse();
        shuffled.queries.reverse();
        assert_ne!(base, shuffled, "the fixture must have >1 item per list");
        assert_eq!(base.hash(), shuffled.hash());
        assert_eq!(base.canonical_json(), shuffled.canonical_json());
    }

    #[test]
    fn hash_ignores_method_constructor_and_port_method_order() {
        let base = representative_schema();
        let mut reordered = base.clone();
        let mut touched = 0;
        for object in &mut reordered.objects {
            object.methods.reverse();
            object.constructors.reverse();
            touched += usize::from(object.methods.len() > 1 || object.constructors.len() > 1);
        }
        for port in &mut reordered.ports {
            port.methods.reverse();
            touched += usize::from(port.methods.len() > 1);
        }
        assert!(touched >= 3, "the fixture must have reorderable lists");
        assert_ne!(base, reordered);
        assert_eq!(base.canonical_json(), reordered.canonical_json());
        assert_eq!(base.hash(), reordered.hash());
    }

    #[test]
    fn hash_ignores_variant_declaration_order_but_not_indices() {
        let base = representative_schema();
        let mut reordered = base.clone();
        for en in &mut reordered.enums {
            en.variants.reverse();
        }
        assert_eq!(base.hash(), reordered.hash());

        let mut renumbered = base.clone();
        renumbered.enums[0].variants[0].index += 10;
        assert_ne!(base.hash(), renumbered.hash());
    }

    #[test]
    fn hash_changes_on_field_rename() {
        let base = representative_schema();
        let mut renamed = base.clone();
        renamed.records[0].fields[0].name.push('2');
        assert_ne!(base.hash(), renamed.hash());
    }

    #[test]
    fn hash_changes_on_record_field_order() {
        let base = representative_schema();
        let mut swapped = base.clone();
        swapped.records[0].fields.swap(0, 1);
        assert_ne!(base.hash(), swapped.hash());
    }

    #[test]
    fn hash_changes_on_variant_field_param_and_signal_order() {
        let base = representative_schema();

        let mut variant_fields = base.clone();
        let shape = variant_fields
            .enums
            .iter_mut()
            .find(|e| e.name == "Shape")
            .unwrap();
        let rect = shape
            .variants
            .iter_mut()
            .find(|v| v.name == "Rect")
            .unwrap();
        rect.fields.swap(0, 1);
        assert_ne!(base.hash(), variant_fields.hash(), "variant field order");

        let mut params = base.clone();
        let add = params.objects[0]
            .methods
            .iter_mut()
            .find(|m| m.name == "add")
            .unwrap();
        add.params.swap(0, 1);
        assert_ne!(base.hash(), params.hash(), "param order");

        let mut signals = base.clone();
        let store = signals
            .objects
            .iter_mut()
            .find_map(|o| o.store.as_mut())
            .unwrap();
        store.signals.swap(0, 1);
        assert_ne!(base.hash(), signals.hash(), "signal order");
    }

    #[test]
    fn hash_changes_on_any_wire_relevant_edit() {
        let base = representative_schema();
        let edits: Vec<Edit> = vec![
            (
                "field type",
                Box::new(|s| s.records[0].fields[0].ty = TypeRef::I64),
            ),
            (
                "field default flag",
                Box::new(|s| s.records[0].fields[0].default ^= true),
            ),
            (
                "error message",
                Box::new(|s| {
                    let e = s.enums.iter_mut().find(|e| e.is_error).unwrap();
                    e.variants[0].message = Some("changed".into());
                }),
            ),
            (
                "method async flag",
                Box::new(|s| s.objects[0].methods[0].is_async ^= true),
            ),
            (
                "method rename",
                Box::new(|s| s.objects[0].methods[0].name.push('2')),
            ),
            (
                "method param order",
                Box::new(|s| {
                    let m = s.objects[0].methods.iter_mut().find(|m| m.params.len() > 1);
                    m.unwrap().params.reverse();
                }),
            ),
            (
                "signal key",
                Box::new(|s| {
                    let store = s.objects.iter_mut().find_map(|o| o.store.as_mut()).unwrap();
                    store.signals[0].key = None;
                }),
            ),
            (
                "signal no_coalesce flag",
                Box::new(|s| {
                    let store = s.objects.iter_mut().find_map(|o| o.store.as_mut()).unwrap();
                    store.signals[0].no_coalesce = true;
                }),
            ),
            (
                "port kind",
                Box::new(|s| s.ports[0].kind = crate::PortKind::Event),
            ),
            ("query stale", Box::new(|s| s.queries[0].stale_ms = Some(1))),
        ];
        for (what, edit) in edits {
            let mut edited = base.clone();
            edit(&mut edited);
            assert_ne!(
                base.hash(),
                edited.hash(),
                "editing {what} must change the hash"
            );
        }
    }

    #[test]
    fn from_json_rejects_malformed_input() {
        for bad in [
            "",
            "{",
            "[]",
            "null",
            r#"{"undra_version":"1.0.0"}"#,
            r#"{"undra_version":1,"crate_name":"x","records":[],"enums":[],"objects":[],"functions":[],"ports":[],"queries":[]}"#,
        ] {
            assert!(Schema::from_json(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn from_json_does_not_accept_the_label_free_canonical_form() {
        let canonical = representative_schema().canonical_json();
        assert!(Schema::from_json(&canonical).is_err());
    }

    #[test]
    fn from_json_rejects_unknown_type_kind_and_bad_port_kind() {
        let mut value = serde_json::to_value(representative_schema()).unwrap();
        value["records"][0]["fields"][0]["ty"] = serde_json::json!({"kind": "quaternion"});
        assert!(Schema::from_json(&value.to_string()).is_err());

        let mut value = serde_json::to_value(representative_schema()).unwrap();
        value["ports"][0]["kind"] = serde_json::json!("telepathic");
        assert!(Schema::from_json(&value.to_string()).is_err());
    }

    #[test]
    fn from_json_rejects_out_of_range_numbers() {
        let mut value = serde_json::to_value(representative_schema()).unwrap();
        value["records"][0]["type_id"] = serde_json::json!(4_294_967_296_u64);
        assert!(Schema::from_json(&value.to_string()).is_err());
        let mut value = serde_json::to_value(representative_schema()).unwrap();
        value["enums"][0]["variants"][0]["index"] = serde_json::json!(70_000);
        assert!(Schema::from_json(&value.to_string()).is_err());
        let mut value = serde_json::to_value(representative_schema()).unwrap();
        value["records"][0]["type_id"] = serde_json::json!(-1);
        assert!(Schema::from_json(&value.to_string()).is_err());
    }

    #[test]
    fn from_json_survives_pathological_nesting() {
        // serde_json's recursion limit turns this into an error, not a stack
        // overflow.
        let depth = 400;
        let mut ty = String::from(r#"{"kind":"u8"}"#);
        for _ in 0..depth {
            ty = format!(r#"{{"kind":"option","of":{ty}}}"#);
        }
        let json = format!(
            r#"{{"undra_version":"1.0.0","crate_name":"x","records":[{{"name":"R","type_id":1,"fields":[{{"name":"f","ty":{ty},"default":false}}]}}],"enums":[],"objects":[],"functions":[],"ports":[],"queries":[]}}"#
        );
        assert!(Schema::from_json(&json).is_err());
    }

    #[test]
    fn unicode_names_round_trip_and_hash_stably() {
        let mut schema = Schema::new("crate-\u{00e9}");
        schema.records.push(RecordDef {
            name: "T\u{00e9}l\u{00e9}vision\u{1F30A}".into(),
            type_id: 1,
            fields: vec![FieldDef {
                name: "na\u{00ef}ve \"quoted\"\n".into(),
                ty: TypeRef::named("\u{4e2d}\u{6587}"),
                default: false,
                docs: String::new(),
            }],
            transparent: false,
            docs: String::new(),
        });
        let back = Schema::from_json(&schema.to_json_pretty()).unwrap();
        assert_eq!(back, schema);
        assert_eq!(back.hash(), schema.hash());
        // The canonical form is valid JSON and keeps the escapes intact.
        let value: serde_json::Value = serde_json::from_str(&schema.canonical_json()).unwrap();
        assert_eq!(
            value["records"][0]["fields"][0]["name"],
            "na\u{00ef}ve \"quoted\"\n"
        );
    }

    #[test]
    fn empty_schema_canonical_form_and_hash_are_stable() {
        let schema = Schema::new("");
        assert_eq!(schema.hash(), schema.hash());
        assert_eq!(schema.hash(), Schema::new("anything").hash());
        assert_eq!(
            schema.canonical_json(),
            r#"{"records":[],"enums":[],"objects":[],"functions":[],"ports":[],"queries":[]}"#
        );
    }

    /// A schema whose every sorted list holds equal keys with different contents, short lists
    /// (at most 16 items) and long ones: what the sort decides and a valid playground does not
    /// show (its names are unique). `collect_schema` validates afterwards, but the order of
    /// duplicates still decides the canonical form, and so the hash, of what it hashes.
    fn equal_keyed_schema() -> Schema {
        use crate::fixtures::{field, method, object, param, record};
        use crate::{EnumDef, FunctionDef, PortDef, PortKind, QueryDef, QueryKind, VariantDef};
        const NAMES: [&str; 4] = ["Todo", "A", "b", "A"];
        let name = |i: usize| NAMES[(i * 7 + i / 3) % NAMES.len()];
        let mut s = Schema::new("equal-keyed");
        for i in 0..40 {
            s.records
                .push(record(name(i), vec![field(&format!("f{i}"), TypeRef::U32)]));
        }
        for i in 0..20 {
            s.enums.push(EnumDef {
                name: name(i).into(),
                type_id: crate::ids::type_id(&format!("E{i}")),
                is_error: i % 2 == 0,
                variants: (0..(i + 3))
                    .map(|v| VariantDef {
                        name: format!("V{v}"),
                        index: ((v * 5) % 3) as u16,
                        fields: Vec::new(),
                        tuple: false,
                        message: None,
                        docs: String::new(),
                    })
                    .collect(),
                docs: String::new(),
            });
        }
        for i in 0..18 {
            let ctors = (0..3)
                .map(|c| {
                    method(
                        name(i),
                        name(c),
                        vec![param(&format!("p{c}"), TypeRef::U8)],
                        TypeRef::Unit,
                        false,
                    )
                })
                .collect();
            let methods = (0..(i + 2))
                .map(|m| method(name(i), name(m), vec![], TypeRef::U16, m % 2 == 1))
                .collect();
            s.objects.push(object(name(i), ctors, methods));
        }
        for i in 0..17 {
            s.functions.push(FunctionDef {
                name: name(i).into(),
                method_id: i as u32,
                params: vec![param(&format!("x{i}"), TypeRef::I64)],
                returns: TypeRef::Bool,
                is_async: false,
                takes_ctx: false,
                docs: String::new(),
            });
        }
        for i in 0..3 {
            s.ports.push(PortDef {
                name: name(i).into(),
                port_id: i as u32,
                kind: PortKind::Async,
                background: false,
                methods: (0..(20 - i))
                    .map(|m| method(name(i), name(m), vec![], TypeRef::String, true))
                    .collect(),
                docs: String::new(),
            });
        }
        for i in 0..17 {
            s.queries.push(QueryDef {
                name: name(i).into(),
                query_id: i as u32,
                kind: if i % 2 == 0 {
                    QueryKind::Query
                } else {
                    QueryKind::Mutation
                },
                key: format!("k{i}"),
                params: Vec::new(),
                returns: TypeRef::Unit,
                stale_ms: None,
                persist: false,
                idempotent: false,
                interval_ms: None,
                poll_in_background: false,
                infinite: None,
            });
        }
        s
    }

    /// What `canonicalized` produced before `crate::sort` (ADR-052): `slice::sort_by` /
    /// `sort_by_key` with the same keys, list by list.
    fn canonicalized_with_std_sort(schema: &Schema) -> Schema {
        let mut s = schema.without_docs();
        for en in &mut s.enums {
            en.variants.sort_by_key(|v| v.index);
        }
        for object in &mut s.objects {
            object.constructors.sort_by(|a, b| a.name.cmp(&b.name));
            object.methods.sort_by(|a, b| a.name.cmp(&b.name));
        }
        for port in &mut s.ports {
            port.methods.sort_by(|a, b| a.name.cmp(&b.name));
        }
        s.records.sort_by(|a, b| a.name.cmp(&b.name));
        s.enums.sort_by(|a, b| a.name.cmp(&b.name));
        s.objects.sort_by(|a, b| a.name.cmp(&b.name));
        s.functions.sort_by(|a, b| a.name.cmp(&b.name));
        s.ports.sort_by(|a, b| a.name.cmp(&b.name));
        s.queries.sort_by(|a, b| a.name.cmp(&b.name));
        s
    }

    #[test]
    fn equal_keys_keep_the_order_and_the_hash_they_had_before_the_schema_sort() {
        let schema = equal_keyed_schema();
        assert_eq!(schema.canonicalized(), canonicalized_with_std_sort(&schema));
        let representative = representative_schema();
        assert_eq!(
            representative.canonicalized(),
            canonicalized_with_std_sort(&representative)
        );
        // The hash `main` computed for this schema at a0d638f, before `crate::sort` existed.
        assert_eq!(schema.hash(), 0xef9b_4b0c_dcd2_89c2);

        // The fixture is sensitive to the order of equal keys: the same lists in reverse
        // declaration order canonicalize differently, so a sort that was not stable would move
        // the hash above.
        let mut reversed = schema.clone();
        reversed.records.reverse();
        assert_ne!(reversed.hash(), schema.hash());
        for en in &mut reversed.enums {
            en.variants.reverse();
        }
        assert_eq!(
            reversed.canonicalized(),
            canonicalized_with_std_sort(&reversed)
        );

        // The writer visits the lists in that order without sorting a clone (`sort::order_by`):
        // the same bytes as `serde_json` on the clone, equal keys and long lists included.
        for s in [&schema, &reversed] {
            assert_eq!(s.canonical_json(), s.canonical_json_by_serde());
        }
    }

    #[test]
    fn newtype_and_paging_flags_are_written_only_when_set() {
        // ADR-042 / ADR-043: no schema that does not use them hashes differently.
        let plain = representative_schema().canonical_json();
        for absent in [
            "transparent",
            "interval_ms",
            "poll_in_background",
            "infinite",
            "decimal",
        ] {
            assert!(!plain.contains(absent), "{absent} leaked into {plain}");
        }
        let mut schema = Schema::new("t");
        schema.records.push(RecordDef {
            name: "UserId".into(),
            type_id: 1,
            fields: vec![FieldDef {
                name: "value".into(),
                ty: TypeRef::Decimal,
                default: false,
                docs: String::new(),
            }],
            transparent: true,
            docs: String::new(),
        });
        let json = schema.canonical_json();
        assert!(json.contains(r#""transparent":true"#), "{json}");
        assert!(json.contains(r#""kind":"decimal""#), "{json}");
        let back = Schema::from_json(&schema.to_json()).unwrap();
        assert_eq!(back, schema);
        assert_ne!(
            schema.hash(),
            {
                let mut flat = schema.clone();
                flat.records[0].transparent = false;
                flat.hash()
            },
            "being a newtype is part of the wire-relevant schema"
        );
    }
}
