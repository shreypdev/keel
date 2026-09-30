//! JSON forms of a [`Schema`], the canonical form and the schema hash
//! (SPEC §2.3).
//!
//! * [`Schema::to_json_pretty`] and [`Schema::from_json`] are the
//!   human-facing exchange format (`schema.json`, `keel_schema_json`). Docs are
//!   included when present.
//! * [`Schema::canonical_json`] is the form the hash is computed over: docs
//!   stripped, the six top-level lists sorted by name, enum variants sorted by
//!   index, no whitespace, struct fields in declaration order.
//! * [`Schema::hash`] is `fnv1a64` of the canonical bytes.
//!
//! What is deliberately *not* normalised, because it is significant on the
//! wire or to generated code: record and variant field order, parameter order,
//! signal order (a `signal_id` is its index), and the order of methods and
//! constructors within an object or port.

use crate::ids::fnv1a64;
use crate::{Schema, VariantDef};

impl Schema {
    /// The canonical JSON the schema hash is computed over (SPEC §2.3).
    ///
    /// Two schemas with the same public surface produce identical output
    /// regardless of doc comments or of the order of top-level items.
    ///
    /// ```
    /// use keel_meta::Schema;
    ///
    /// let schema = Schema::new("demo");
    /// assert_eq!(
    ///     schema.canonical_json(),
    ///     r#"{"keel_version":"1.0.0","crate_name":"demo","records":[],"enums":[],"objects":[],"functions":[],"ports":[],"queries":[]}"#
    /// );
    /// ```
    #[must_use]
    pub fn canonical_json(&self) -> String {
        let canonical = self.canonicalized();
        // Serializing these types cannot fail: they contain only strings,
        // integers, booleans, options and sequences, and the writer is an
        // in-memory `String`.
        serde_json::to_string(&canonical).expect("schema types always serialize to JSON")
    }

    /// The schema hash: `fnv1a64(canonical_json())` (SPEC §1.1).
    ///
    /// ```
    /// use keel_meta::Schema;
    ///
    /// let a = Schema::new("demo");
    /// let b = Schema::new("other");
    /// assert_eq!(a.hash(), Schema::new("demo").hash());
    /// assert_ne!(a.hash(), b.hash());
    /// ```
    #[must_use]
    pub fn hash(&self) -> u64 {
        fnv1a64(self.canonical_json().as_bytes())
    }

    /// Pretty-printed JSON, with docs, for humans and `schema.json` files.
    ///
    /// The output is *not* canonical; use [`Schema::canonical_json`] for
    /// hashing and comparisons.
    #[must_use]
    pub fn to_json_pretty(&self) -> String {
        // Infallible for the same reason as `canonical_json`.
        serde_json::to_string_pretty(self).expect("schema types always serialize to JSON")
    }

    /// Parses a schema from JSON (canonical or pretty).
    ///
    /// Missing `docs` become empty strings. Unknown fields are ignored.
    /// This only checks the JSON shape; call [`Schema::validate`] for the
    /// semantic rules.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`serde_json::Error`] (with line and column) for
    /// malformed JSON, missing fields, unknown type kinds, or nesting deeper
    /// than `serde_json`'s recursion limit.
    ///
    /// ```
    /// use keel_meta::Schema;
    ///
    /// let schema = Schema::new("demo");
    /// let back = Schema::from_json(&schema.to_json_pretty()).unwrap();
    /// assert_eq!(back, schema);
    /// assert!(Schema::from_json("{}").is_err());
    /// ```
    pub fn from_json(json: &str) -> Result<Schema, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// A doc-stripped, sorted clone: the value the canonical JSON is
    /// serialized from.
    fn canonicalized(&self) -> Schema {
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
            en.variants.sort_by_key(|v| v.index);
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

        s.records.sort_by(|a, b| a.name.cmp(&b.name));
        s.enums.sort_by(|a, b| a.name.cmp(&b.name));
        s.objects.sort_by(|a, b| a.name.cmp(&b.name));
        s.functions.sort_by(|a, b| a.name.cmp(&b.name));
        s.ports.sort_by(|a, b| a.name.cmp(&b.name));
        s.queries.sort_by(|a, b| a.name.cmp(&b.name));
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
        assert_eq!(Schema::from_json(&pretty).unwrap(), schema);
        let canonical = schema.canonical_json();
        // The canonical form has no docs, so it round-trips to the doc-less
        // schema, and its own canonical form is a fixed point.
        let back = Schema::from_json(&canonical).unwrap();
        assert_eq!(back.canonical_json(), canonical);
    }

    #[test]
    fn round_trip_preserves_docs_in_pretty_json() {
        let schema = with_docs(representative_schema());
        let back = Schema::from_json(&schema.to_json_pretty()).unwrap();
        assert_eq!(back, schema);
        assert!(back.records[0].docs.contains("todo"));
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
            docs: String::new(),
        });
        let json = schema.to_json_pretty();
        assert!(!json.contains("docs"), "{json}");
        let back = Schema::from_json(&json).unwrap();
        assert_eq!(back.records[0].docs, "");
        assert_eq!(back.records[0].fields[0].docs, "");
    }

    #[test]
    fn canonical_json_is_compact_and_docs_free() {
        let canonical = with_docs(representative_schema()).canonical_json();
        assert!(!canonical.contains("docs"));
        assert!(!canonical.contains('\n'));
        assert!(!canonical.contains(": "));
        assert!(!canonical.contains(", "));
        assert!(!canonical.contains("todo item"));
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
            docs: "A todo.".into(),
        });
        assert_eq!(
            schema.canonical_json(),
            concat!(
                r#"{"keel_version":"1.0.0","crate_name":"golden","records":["#,
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
        assert_eq!(representative_schema().hash(), 0xc4d7_de51_1114_3c8a);
    }

    #[test]
    fn hash_ignores_docs() {
        assert_eq!(
            representative_schema().hash(),
            with_docs(representative_schema()).hash()
        );
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
    fn hash_changes_on_any_wire_relevant_edit() {
        let base = representative_schema();
        let edits: Vec<Edit> = vec![
            ("crate name", Box::new(|s| s.crate_name.push('x'))),
            (
                "keel version",
                Box::new(|s| s.keel_version = "1.0.1".into()),
            ),
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
            r#"{"keel_version":"1.0.0"}"#,
            r#"{"keel_version":1,"crate_name":"x","records":[],"enums":[],"objects":[],"functions":[],"ports":[],"queries":[]}"#,
        ] {
            assert!(Schema::from_json(bad).is_err(), "{bad:?}");
        }
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
            r#"{{"keel_version":"1.0.0","crate_name":"x","records":[{{"name":"R","type_id":1,"fields":[{{"name":"f","ty":{ty},"default":false}}]}}],"enums":[],"objects":[],"functions":[],"ports":[],"queries":[]}}"#
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
            docs: String::new(),
        });
        let back = Schema::from_json(&schema.to_json_pretty()).unwrap();
        assert_eq!(back, schema);
        let from_canonical = Schema::from_json(&schema.canonical_json()).unwrap();
        assert_eq!(from_canonical.hash(), schema.hash());
    }

    #[test]
    fn empty_schema_hash_is_stable_across_calls() {
        let schema = Schema::new("");
        assert_eq!(schema.hash(), schema.hash());
        assert_eq!(
            schema.canonical_json(),
            r#"{"keel_version":"1.0.0","crate_name":"","records":[],"enums":[],"objects":[],"functions":[],"ports":[],"queries":[]}"#
        );
    }
}
