//! The JSON of a [`Schema`], written straight from the schema: without `serde`, and without
//! cloning it. Both forms a core produces come from here:
//!
//! * the **canonical** form the schema hash is computed over (SPEC 2.3, [`Schema::canonical_json`]):
//!   no labels, no docs, the unordered lists in sorted order;
//! * the **exchange** form the C ABI's `undra_schema_json` returns ([`Schema::to_json`]): labels,
//!   docs, every list in declaration order.
//!
//! Every core hashes its schema when it starts ([`Schema::hash`] in `Runtime::new`, and once more
//! for the C ABI's table), so the canonical form is built on every cold start, over the whole
//! schema: its cost is a cost per byte of schema that every app pays before its first screen. It
//! used to be `serde_json::to_string` of a *clone* of the schema with its docs cleared and its
//! unordered lists sorted: every name, type and doc comment copied (and freed) to be read once.
//! `.10x/decisions/sde/cold-restore-regression.md` has the measurements of both paths.
//!
//! The writer walks the schema in canonical order instead of making it canonical:
//!
//! * the docs and the labels (`undra_version`, `crate_name`) are not written;
//! * the unordered lists (the six top-level lists, an object's constructors and methods, a port's
//!   methods: by name; an enum's variants: by index) are visited through
//!   [`sort::order_by`](crate::sort::order_by), references in the order the stable sort would
//!   leave a clone in;
//! * everything else is written as `serde`'s derived impls write it: the same keys in declaration
//!   order, the same flags left out when unset.
//!
//! The exchange form is the same walk with the labels first, the docs where they are not empty
//! and nothing sorted, so one writer is all a core links: `serde`'s derived `Serialize` impls
//! and `serde_json`'s serializer are not in a shipped core (`Schema::to_json_pretty` and
//! `Schema::from_json`, which tools use, still are `serde`).
//!
//! The output is byte for byte what `serde_json` produced, so no schema hash moves: the tests
//! keep the previous paths (`Schema::canonical_json_by_serde`, `Schema::to_json_by_serde`) as
//! oracles and compare on the fixtures and on generated schemas. Every definition is
//! destructured in full below, so a field added to one does not compile here until this writer
//! says what the two forms do with it.

use crate::closure_json::{number, string, type_ref};
use crate::sort::order_by;
use crate::{
    EnumDef, FieldDef, FunctionDef, GenericArg, GenericOf, InfiniteDef, MethodDef, ObjectDef,
    ParamDef, PortDef, PortKind, QueryDef, QueryKind, RecordDef, Schema, SignalDef, StoreDef,
    TypeRef, VariantDef,
};

/// The canonical JSON of `schema`: what [`Schema::canonical_json`] returns.
pub(crate) fn canonical(schema: &Schema) -> String {
    Writer::new(schema, false).schema(schema)
}

/// The exchange JSON of `schema`: what [`Schema::to_json`] returns.
pub(crate) fn exchange(schema: &Schema) -> String {
    Writer::new(schema, true).schema(schema)
}

/// About how long the canonical form of `schema` is, so that the text is not grown and copied a
/// dozen times on the way: a guess from the item counts (a method or a field is about 120 bytes),
/// never a limit. The exchange form is longer by its docs and grows from here.
fn capacity_hint(schema: &Schema) -> usize {
    let methods: usize = schema
        .objects
        .iter()
        .map(|o| o.constructors.len() + o.methods.len())
        .chain(schema.ports.iter().map(|p| p.methods.len()))
        .sum();
    let fields: usize = schema
        .records
        .iter()
        .map(|r| r.fields.len())
        .chain(schema.enums.iter().map(|e| e.variants.len()))
        .sum();
    let items = schema.records.len()
        + schema.enums.len()
        + schema.objects.len()
        + schema.functions.len()
        + schema.ports.len()
        + schema.queries.len();
    128 + 96 * items + 160 * methods + 96 * fields
}

/// The text being written, and which form it is.
struct Writer {
    out: String,
    /// The exchange form: labels and docs are written and nothing is sorted. Otherwise the
    /// canonical form.
    full: bool,
}

impl Writer {
    fn new(schema: &Schema, full: bool) -> Writer {
        Writer {
            out: String::with_capacity(capacity_hint(schema)),
            full,
        }
    }

    fn schema(mut self, schema: &Schema) -> String {
        let Schema {
            undra_version,
            crate_name,
            records,
            enums,
            objects,
            functions,
            ports,
            queries,
        } = schema;
        if self.full {
            self.key("{\"undra_version\":");
            self.string(undra_version);
            self.key(",\"crate_name\":");
            self.string(crate_name);
            self.key(",\"records\":");
        } else {
            self.key("{\"records\":");
        }
        self.sorted(records, |r| r.name.as_str(), Writer::record);
        self.key(",\"enums\":");
        self.sorted(enums, |e| e.name.as_str(), Writer::enum_def);
        self.key(",\"objects\":");
        self.sorted(objects, |o| o.name.as_str(), Writer::object);
        self.key(",\"functions\":");
        self.sorted(functions, |f| f.name.as_str(), Writer::function);
        self.key(",\"ports\":");
        self.sorted(ports, |p| p.name.as_str(), Writer::port);
        self.key(",\"queries\":");
        self.sorted(queries, |q| q.name.as_str(), Writer::query);
        self.out.push('}');
        self.out
    }

    /// Punctuation and keys: `{"name":`, `,"type_id":`, `null`.
    fn key(&mut self, text: &'static str) {
        self.out.push_str(text);
    }

    fn string(&mut self, s: &str) {
        string(&mut self.out, s);
    }

    fn number(&mut self, n: u64) {
        number(&mut self.out, n);
    }

    fn boolean(&mut self, flag: bool) {
        self.out.push_str(if flag { "true" } else { "false" });
    }

    fn ty(&mut self, ty: &TypeRef) {
        type_ref(&mut self.out, ty);
    }

    /// `text` (`,"<flag>":true`), only when the flag is set (`skip_serializing_if = "is_false"`).
    fn flag(&mut self, text: &'static str, set: bool) {
        if set {
            self.key(text);
        }
    }

    /// `null` or the string, as `serde` writes an `Option<String>`.
    fn optional_string(&mut self, value: Option<&str>) {
        match value {
            Some(s) => self.string(s),
            None => self.key("null"),
        }
    }

    /// `{"name":"..","<id key>":N`: how every definition but a field and a parameter starts.
    fn named(&mut self, name: &str, id_key: &'static str, id: u32) {
        self.key("{\"name\":");
        self.string(name);
        self.key(id_key);
        self.number(u64::from(id));
    }

    /// `,"docs":".."}`: how every definition that has docs ends. The docs are written in the
    /// exchange form only, and only when there are any (`skip_serializing_if = "String::is_empty"`).
    fn end(&mut self, docs: &str) {
        if self.full && !docs.is_empty() {
            self.key(",\"docs\":");
            self.string(docs);
        }
        self.out.push('}');
    }

    /// `items` as a JSON array, in the order they are in.
    fn list<T>(&mut self, items: &[T], item: fn(&mut Writer, &T)) {
        self.out.push('[');
        for (at, x) in items.iter().enumerate() {
            if at > 0 {
                self.out.push(',');
            }
            item(self, x);
        }
        self.out.push(']');
    }

    /// `items` as a JSON array: in the canonical form, in the order a stable sort by `key` leaves
    /// them in; in the exchange form, as declared.
    fn sorted<'a, T, K: Ord>(
        &mut self,
        items: &'a [T],
        key: impl Fn(&'a T) -> K,
        item: fn(&mut Writer, &T),
    ) {
        let order = if self.full {
            None
        } else {
            order_by(items, key)
        };
        let Some(order) = order else {
            self.list(items, item);
            return;
        };
        self.out.push('[');
        for (at, x) in order.into_iter().enumerate() {
            if at > 0 {
                self.out.push(',');
            }
            item(self, x);
        }
        self.out.push(']');
    }

    fn record(&mut self, r: &RecordDef) {
        let RecordDef {
            name,
            type_id,
            fields,
            transparent,
            docs,
        } = r;
        self.named(name, ",\"type_id\":", *type_id);
        self.key(",\"fields\":");
        self.list(fields, Writer::field);
        self.flag(",\"transparent\":true", *transparent);
        self.end(docs);
    }

    fn field(&mut self, f: &FieldDef) {
        let FieldDef {
            name,
            ty,
            default,
            docs,
        } = f;
        self.key("{\"name\":");
        self.string(name);
        self.key(",\"ty\":");
        self.ty(ty);
        self.key(",\"default\":");
        self.boolean(*default);
        self.end(docs);
    }

    fn enum_def(&mut self, e: &EnumDef) {
        let EnumDef {
            name,
            type_id,
            is_error,
            variants,
            docs,
        } = e;
        self.named(name, ",\"type_id\":", *type_id);
        self.key(",\"is_error\":");
        self.boolean(*is_error);
        self.key(",\"variants\":");
        self.sorted(variants, |v| v.index, Writer::variant);
        self.end(docs);
    }

    fn variant(&mut self, v: &VariantDef) {
        let VariantDef {
            name,
            index,
            fields,
            tuple,
            message,
            docs,
        } = v;
        self.named(name, ",\"index\":", u32::from(*index));
        self.key(",\"fields\":");
        self.list(fields, Writer::field);
        self.key(",\"tuple\":");
        self.boolean(*tuple);
        self.key(",\"message\":");
        self.optional_string(message.as_deref());
        self.end(docs);
    }

    fn object(&mut self, o: &ObjectDef) {
        let ObjectDef {
            name,
            type_id,
            constructors,
            methods,
            store,
            docs,
        } = o;
        self.named(name, ",\"type_id\":", *type_id);
        self.key(",\"constructors\":");
        self.sorted(constructors, |m| m.name.as_str(), Writer::method);
        self.key(",\"methods\":");
        self.sorted(methods, |m| m.name.as_str(), Writer::method);
        match store {
            Some(StoreDef { signals }) => {
                self.key(",\"store\":{\"signals\":");
                self.list(signals, Writer::signal);
                self.out.push('}');
            }
            None => self.key(",\"store\":null"),
        }
        self.end(docs);
    }

    /// `,"params":[..],"returns":T`: what a method, a function and a query share.
    fn signature(&mut self, params: &[ParamDef], returns: &TypeRef) {
        self.key(",\"params\":");
        self.list(params, Writer::param);
        self.key(",\"returns\":");
        self.ty(returns);
    }

    /// `,"is_async":B,"takes_ctx":B`: what a method and a function share after their signature.
    fn call_flags(&mut self, is_async: bool, takes_ctx: bool) {
        self.key(",\"is_async\":");
        self.boolean(is_async);
        self.key(",\"takes_ctx\":");
        self.boolean(takes_ctx);
    }

    /// `,"generic":{"of":"..","args":[{"param":"..","ty":T,"inferred":B}, ..]}` (ADR-058), only for an
    /// instantiation of a generic function or method (`skip_serializing_if = "Option::is_none"`).
    fn generic(&mut self, generic: Option<&GenericOf>) {
        let Some(GenericOf { of, args }) = generic else {
            return;
        };
        self.key(",\"generic\":{\"of\":");
        self.string(of);
        self.key(",\"args\":");
        self.list(args, |w, arg| {
            let GenericArg {
                param,
                ty,
                inferred,
            } = arg;
            w.key("{\"param\":");
            w.string(param);
            w.key(",\"ty\":");
            w.ty(ty);
            w.key(",\"inferred\":");
            w.boolean(*inferred);
            w.out.push('}');
        });
        self.out.push('}');
    }

    fn method(&mut self, m: &MethodDef) {
        let MethodDef {
            name,
            method_id,
            params,
            returns,
            is_async,
            takes_ctx,
            coalesce,
            generic,
            docs,
        } = m;
        self.named(name, ",\"method_id\":", *method_id);
        self.signature(params, returns);
        self.call_flags(*is_async, *takes_ctx);
        self.flag(",\"coalesce\":true", *coalesce);
        self.generic(generic.as_ref());
        self.end(docs);
    }

    fn param(&mut self, p: &ParamDef) {
        let ParamDef { name, ty } = p;
        self.key("{\"name\":");
        self.string(name);
        self.key(",\"ty\":");
        self.ty(ty);
        self.out.push('}');
    }

    fn signal(&mut self, s: &SignalDef) {
        let SignalDef {
            name,
            signal_id,
            ty,
            computed,
            key,
            no_coalesce,
            default,
        } = s;
        self.named(name, ",\"signal_id\":", *signal_id);
        self.key(",\"ty\":");
        self.ty(ty);
        self.key(",\"computed\":");
        self.boolean(*computed);
        self.key(",\"key\":");
        self.optional_string(key.as_deref());
        self.flag(",\"no_coalesce\":true", *no_coalesce);
        self.flag(",\"default\":true", *default);
        self.out.push('}');
    }

    fn function(&mut self, f: &FunctionDef) {
        let FunctionDef {
            name,
            method_id,
            params,
            returns,
            is_async,
            takes_ctx,
            generic,
            docs,
        } = f;
        self.named(name, ",\"method_id\":", *method_id);
        self.signature(params, returns);
        self.call_flags(*is_async, *takes_ctx);
        self.generic(generic.as_ref());
        self.end(docs);
    }

    fn port(&mut self, p: &PortDef) {
        let PortDef {
            name,
            port_id,
            kind,
            background,
            methods,
            docs,
        } = p;
        self.named(name, ",\"port_id\":", *port_id);
        self.key(match kind {
            PortKind::Sync => ",\"kind\":\"sync\"",
            PortKind::Async => ",\"kind\":\"async\"",
            PortKind::Event => ",\"kind\":\"event\"",
            PortKind::Callback => ",\"kind\":\"callback\"",
        });
        self.flag(",\"background\":true", *background);
        self.key(",\"methods\":");
        self.sorted(methods, |m| m.name.as_str(), Writer::method);
        self.end(docs);
    }

    fn query(&mut self, q: &QueryDef) {
        let QueryDef {
            name,
            query_id,
            kind,
            key,
            params,
            returns,
            stale_ms,
            persist,
            idempotent,
            interval_ms,
            poll_in_background,
            infinite,
        } = q;
        self.named(name, ",\"query_id\":", *query_id);
        self.key(match kind {
            QueryKind::Query => ",\"kind\":\"query\",\"key\":",
            QueryKind::Mutation => ",\"kind\":\"mutation\",\"key\":",
        });
        self.string(key);
        self.signature(params, returns);
        match stale_ms {
            Some(ms) => {
                self.key(",\"stale_ms\":");
                self.number(*ms);
            }
            None => self.key(",\"stale_ms\":null"),
        }
        self.key(",\"persist\":");
        self.boolean(*persist);
        self.key(",\"idempotent\":");
        self.boolean(*idempotent);
        if let Some(ms) = interval_ms {
            self.key(",\"interval_ms\":");
            self.number(*ms);
        }
        self.flag(",\"poll_in_background\":true", *poll_in_background);
        if let Some(InfiniteDef { cursor, item_key }) = infinite {
            self.key(",\"infinite\":{\"cursor\":");
            self.ty(cursor);
            self.key(",\"item_key\":");
            self.string(item_key);
            self.out.push('}');
        }
        // A query has no docs.
        self.out.push('}');
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::closure_json::tests::{arb_name, arb_ty};
    use crate::fixtures::{representative_schema, with_docs};

    /// The writer against the paths it replaced (both forms), the hash against `fnv1a64` of the
    /// canonical path's bytes, and the exchange form read back.
    fn assert_same_as_serde(schema: &Schema) {
        let by_serde = schema.canonical_json_by_serde();
        assert_eq!(canonical(schema), by_serde);
        assert_eq!(schema.canonical_json(), by_serde);
        assert_eq!(schema.hash(), crate::ids::fnv1a64(by_serde.as_bytes()));
        assert_eq!(exchange(schema), schema.to_json_by_serde());
        assert_eq!(&Schema::from_json(&schema.to_json()).unwrap(), schema);
    }

    #[test]
    fn the_fixtures_are_written_as_serde_wrote_them() {
        assert_same_as_serde(&Schema::new("empty"));
        assert_same_as_serde(&representative_schema());
        assert_same_as_serde(&with_docs(representative_schema()));
        // Every unordered list in reverse declaration order, docs kept.
        let mut reversed = with_docs(representative_schema());
        reversed.records.reverse();
        reversed.enums.reverse();
        reversed.objects.reverse();
        reversed.functions.reverse();
        reversed.ports.reverse();
        reversed.queries.reverse();
        for en in &mut reversed.enums {
            en.variants.reverse();
        }
        for object in &mut reversed.objects {
            object.constructors.reverse();
            object.methods.reverse();
        }
        for port in &mut reversed.ports {
            port.methods.reverse();
        }
        assert_same_as_serde(&reversed);
        assert_eq!(reversed.hash(), representative_schema().hash());
    }

    /// The process's own registrations (this crate's tests register a few): what a core hashes.
    #[test]
    fn the_collected_schema_is_written_as_serde_wrote_it() {
        assert_same_as_serde(&crate::collect_schema("undra-meta"));
    }

    #[test]
    fn every_optional_key_is_written_when_set_and_left_out_when_not() {
        let mut schema = Schema::new("flags");
        schema.records.push(RecordDef {
            name: "Wrapper".into(),
            type_id: 7,
            fields: vec![FieldDef {
                name: "0".into(),
                ty: crate::TypeRef::U64,
                default: true,
                docs: "the value".into(),
            }],
            transparent: true,
            docs: "a newtype".into(),
        });
        schema.enums.push(EnumDef {
            name: "Failure".into(),
            type_id: 8,
            is_error: true,
            variants: vec![
                VariantDef {
                    name: "Late".into(),
                    index: 1,
                    fields: Vec::new(),
                    tuple: false,
                    message: Some("late by {0} \"ms\"".into()),
                    docs: "docs".into(),
                },
                VariantDef {
                    name: "Early".into(),
                    index: 0,
                    fields: Vec::new(),
                    tuple: true,
                    message: None,
                    docs: String::new(),
                },
            ],
            docs: String::new(),
        });
        // ADR-058: `set` and `get` are instantiations of a generic method, `new` is not.
        let generic = |of: &str| GenericOf {
            of: of.into(),
            args: vec![
                GenericArg {
                    param: "T".into(),
                    ty: crate::TypeRef::named("Todo"),
                    inferred: true,
                },
                GenericArg {
                    param: "U".into(),
                    ty: crate::TypeRef::vec(crate::TypeRef::U8),
                    inferred: false,
                },
            ],
        };
        let method = |name: &str, coalesce: bool| MethodDef {
            name: name.into(),
            method_id: 9,
            params: vec![ParamDef {
                name: "n".into(),
                ty: crate::TypeRef::U32,
            }],
            returns: crate::TypeRef::Unit,
            is_async: coalesce,
            takes_ctx: !coalesce,
            coalesce,
            generic: (name != "new").then(|| generic(name)),
            docs: "docs".into(),
        };
        let signal = |name: &str, flags: bool| SignalDef {
            name: name.into(),
            signal_id: 3,
            ty: crate::TypeRef::String,
            computed: !flags,
            key: flags.then(|| "id".to_owned()),
            no_coalesce: flags,
            default: flags,
        };
        schema.objects.push(ObjectDef {
            name: "Store".into(),
            type_id: 10,
            constructors: vec![method("new", false)],
            methods: vec![method("set", true), method("get", false)],
            store: Some(StoreDef {
                signals: vec![signal("a", true), signal("b", false)],
            }),
            docs: "docs".into(),
        });
        schema.objects.push(ObjectDef {
            name: "Plain".into(),
            type_id: 11,
            constructors: Vec::new(),
            methods: Vec::new(),
            store: None,
            docs: String::new(),
        });
        for (kind, background) in [
            (PortKind::Sync, false),
            (PortKind::Async, true),
            (PortKind::Event, false),
            (PortKind::Callback, true),
        ] {
            schema.ports.push(PortDef {
                name: format!("Port{}", schema.ports.len()),
                port_id: 12,
                kind,
                background,
                methods: vec![method("b", false), method("a", true)],
                docs: "docs".into(),
            });
        }
        let query = |name: &str, set: bool| QueryDef {
            name: name.into(),
            query_id: 13,
            kind: if set {
                QueryKind::Mutation
            } else {
                QueryKind::Query
            },
            key: "[\"todos\", id]".into(),
            params: Vec::new(),
            returns: crate::TypeRef::Bool,
            stale_ms: set.then_some(u64::MAX),
            persist: set,
            idempotent: !set,
            interval_ms: set.then_some(0),
            poll_in_background: set,
            infinite: set.then(|| InfiniteDef {
                cursor: crate::TypeRef::String,
                item_key: "id".into(),
            }),
        };
        schema.queries.push(query("set", true));
        schema.queries.push(query("bare", false));
        schema.functions.push(FunctionDef {
            name: "newest<Todo>".into(),
            method_id: 14,
            params: Vec::new(),
            returns: crate::TypeRef::Unit,
            is_async: false,
            takes_ctx: false,
            generic: Some(generic("newest")),
            docs: String::new(),
        });
        schema.functions.push(FunctionDef {
            name: "plain".into(),
            method_id: 15,
            params: Vec::new(),
            returns: crate::TypeRef::Unit,
            is_async: true,
            takes_ctx: true,
            generic: None,
            docs: "docs".into(),
        });
        assert_same_as_serde(&schema);

        let text = canonical(&schema);
        for key in [
            "\"transparent\":true",
            "\"coalesce\":true",
            "\"no_coalesce\":true",
            "\"default\":true",
            "\"background\":true",
            "\"poll_in_background\":true",
            "\"interval_ms\":0",
            "\"infinite\":{",
            "\"message\":null",
            "\"store\":null",
            "\"stale_ms\":null",
            "\"kind\":\"callback\"",
            "\"kind\":\"mutation\"",
            "\"generic\":{\"of\":\"newest\",\"args\":[{\"param\":\"T\",\"ty\":{\"kind\":\"named\",\"of\":\"Todo\"},\"inferred\":true},{\"param\":\"U\",\"ty\":{\"kind\":\"vec\",\"of\":{\"kind\":\"u8\"}},\"inferred\":false}]}",
        ] {
            assert!(text.contains(key), "{key} in {text}");
        }
        assert!(!text.contains("docs"), "{text}");
        assert!(!text.contains("\"coalesce\":false"), "{text}");
        assert!(!text.contains("\"generic\":null"), "{text}");
        assert_eq!(text.matches("\"generic\":{").count(), 1 + 2 * 5, "{text}");

        // The exchange form: labels first, the docs that are there, declaration order.
        let text = exchange(&schema);
        assert!(
            text.starts_with("{\"undra_version\":\"1.0.0\",\"crate_name\":\"flags\",\"records\":[")
        );
        // A record and its field, a variant, an object with three methods, four ports with two, a
        // function.
        assert_eq!(text.matches("\"docs\":").count(), 20, "{text}");
        assert!(text.contains("\"docs\":\"a newtype\"}"), "{text}");
        let (late, early) = (
            text.find("\"Late\"").unwrap(),
            text.find("\"Early\"").unwrap(),
        );
        assert!(late < early, "variants stay in declaration order: {text}");
    }

    #[test]
    fn the_capacity_hint_is_near_the_length() {
        let schema = with_docs(representative_schema());
        let (hint, len) = (capacity_hint(&schema), canonical(&schema).len());
        assert!(
            hint >= len / 2 && hint <= len * 3,
            "hint {hint}, length {len}"
        );
    }

    // ----- differential against the serde path on generated schemas -----------------------------

    fn arb_docs() -> BoxedStrategy<String> {
        prop_oneof![Just(String::new()), arb_name()].boxed()
    }

    fn arb_field() -> BoxedStrategy<FieldDef> {
        (arb_name(), arb_ty(), any::<bool>(), arb_docs())
            .prop_map(|(name, ty, default, docs)| FieldDef {
                name,
                ty,
                default,
                docs,
            })
            .boxed()
    }

    fn arb_params() -> BoxedStrategy<Vec<ParamDef>> {
        proptest::collection::vec(
            (arb_name(), arb_ty()).prop_map(|(name, ty)| ParamDef { name, ty }),
            0..3,
        )
        .boxed()
    }

    /// Few names, so that lists have equal keys and stability decides the output.
    fn arb_key() -> BoxedStrategy<String> {
        prop_oneof![
            4 => prop_oneof![Just("a"), Just("B"), Just("aa"), Just(""), Just("ü")]
                .prop_map(str::to_owned),
            1 => arb_name(),
        ]
        .boxed()
    }

    /// ADR-058's label: absent most of the time, with zero to three arguments otherwise.
    fn arb_generic() -> BoxedStrategy<Option<GenericOf>> {
        let arg =
            (arb_name(), arb_ty(), any::<bool>()).prop_map(|(param, ty, inferred)| GenericArg {
                param,
                ty,
                inferred,
            });
        prop_oneof![
            2 => Just(None),
            1 => (arb_name(), proptest::collection::vec(arg, 0..3))
                .prop_map(|(of, args)| Some(GenericOf { of, args })),
        ]
        .boxed()
    }

    fn arb_method() -> BoxedStrategy<MethodDef> {
        (
            arb_key(),
            any::<u32>(),
            arb_params(),
            arb_ty(),
            any::<(bool, bool, bool)>(),
            arb_generic(),
            arb_docs(),
        )
            .prop_map(
                |(
                    name,
                    method_id,
                    params,
                    returns,
                    (is_async, takes_ctx, coalesce),
                    generic,
                    docs,
                )| {
                    MethodDef {
                        name,
                        method_id,
                        params,
                        returns,
                        is_async,
                        takes_ctx,
                        coalesce,
                        generic,
                        docs,
                    }
                },
            )
            .boxed()
    }

    /// Lists on both sides of the sort's in-place threshold (16).
    fn arb_list<T: std::fmt::Debug + 'static>(item: BoxedStrategy<T>) -> BoxedStrategy<Vec<T>> {
        prop_oneof![
            4 => proptest::collection::vec(item.clone(), 0..4),
            1 => proptest::collection::vec(item, 14..20),
        ]
        .boxed()
    }

    fn arb_record() -> BoxedStrategy<RecordDef> {
        (
            arb_key(),
            any::<u32>(),
            proptest::collection::vec(arb_field(), 0..3),
            any::<bool>(),
            arb_docs(),
        )
            .prop_map(|(name, type_id, fields, transparent, docs)| RecordDef {
                name,
                type_id,
                fields,
                transparent,
                docs,
            })
            .boxed()
    }

    fn arb_enum() -> BoxedStrategy<EnumDef> {
        let variant = (
            arb_name(),
            prop_oneof![0..4_u16, any::<u16>()],
            proptest::collection::vec(arb_field(), 0..2),
            any::<bool>(),
            proptest::option::of(arb_name()),
            arb_docs(),
        )
            .prop_map(|(name, index, fields, tuple, message, docs)| VariantDef {
                name,
                index,
                fields,
                tuple,
                message,
                docs,
            });
        (
            arb_key(),
            any::<u32>(),
            any::<bool>(),
            arb_list(variant.boxed()),
            arb_docs(),
        )
            .prop_map(|(name, type_id, is_error, variants, docs)| EnumDef {
                name,
                type_id,
                is_error,
                variants,
                docs,
            })
            .boxed()
    }

    fn arb_object() -> BoxedStrategy<ObjectDef> {
        let signal = (
            arb_name(),
            any::<u32>(),
            arb_ty(),
            any::<(bool, bool, bool)>(),
            proptest::option::of(arb_name()),
        )
            .prop_map(
                |(name, signal_id, ty, (computed, no_coalesce, default), key)| SignalDef {
                    name,
                    signal_id,
                    ty,
                    computed,
                    key,
                    no_coalesce,
                    default,
                },
            );
        (
            arb_key(),
            any::<u32>(),
            arb_list(arb_method()),
            arb_list(arb_method()),
            proptest::option::of(
                proptest::collection::vec(signal, 0..3).prop_map(|signals| StoreDef { signals }),
            ),
            arb_docs(),
        )
            .prop_map(
                |(name, type_id, constructors, methods, store, docs)| ObjectDef {
                    name,
                    type_id,
                    constructors,
                    methods,
                    store,
                    docs,
                },
            )
            .boxed()
    }

    fn arb_function() -> BoxedStrategy<FunctionDef> {
        (
            arb_key(),
            any::<u32>(),
            arb_params(),
            arb_ty(),
            any::<(bool, bool)>(),
            arb_generic(),
            arb_docs(),
        )
            .prop_map(
                |(name, method_id, params, returns, (is_async, takes_ctx), generic, docs)| {
                    FunctionDef {
                        name,
                        method_id,
                        params,
                        returns,
                        is_async,
                        takes_ctx,
                        generic,
                        docs,
                    }
                },
            )
            .boxed()
    }

    fn arb_port() -> BoxedStrategy<PortDef> {
        (
            arb_key(),
            any::<u32>(),
            prop_oneof![
                Just(PortKind::Sync),
                Just(PortKind::Async),
                Just(PortKind::Event),
                Just(PortKind::Callback),
            ],
            any::<bool>(),
            arb_list(arb_method()),
            arb_docs(),
        )
            .prop_map(|(name, port_id, kind, background, methods, docs)| PortDef {
                name,
                port_id,
                kind,
                background,
                methods,
                docs,
            })
            .boxed()
    }

    fn arb_query() -> BoxedStrategy<QueryDef> {
        (
            (arb_key(), any::<u32>(), any::<bool>(), arb_name()),
            arb_params(),
            arb_ty(),
            proptest::option::of(prop_oneof![Just(0_u64), Just(u64::MAX), any::<u64>()]),
            any::<(bool, bool, bool)>(),
            proptest::option::of(any::<u64>()),
            proptest::option::of(
                (arb_ty(), arb_name())
                    .prop_map(|(cursor, item_key)| InfiniteDef { cursor, item_key }),
            ),
        )
            .prop_map(
                |(
                    (name, query_id, mutation, key),
                    params,
                    returns,
                    stale_ms,
                    (persist, idempotent, poll_in_background),
                    interval_ms,
                    infinite,
                )| QueryDef {
                    name,
                    query_id,
                    kind: if mutation {
                        QueryKind::Mutation
                    } else {
                        QueryKind::Query
                    },
                    key,
                    params,
                    returns,
                    stale_ms,
                    persist,
                    idempotent,
                    interval_ms,
                    poll_in_background,
                    infinite,
                },
            )
            .boxed()
    }

    fn arb_schema() -> BoxedStrategy<Schema> {
        (
            (arb_name(), arb_name()),
            arb_list(arb_record()),
            arb_list(arb_enum()),
            proptest::collection::vec(arb_object(), 0..3),
            arb_list(arb_function()),
            proptest::collection::vec(arb_port(), 0..3),
            arb_list(arb_query()),
        )
            .prop_map(
                |(
                    (undra_version, crate_name),
                    records,
                    enums,
                    objects,
                    functions,
                    ports,
                    queries,
                )| {
                    Schema {
                        undra_version,
                        crate_name,
                        records,
                        enums,
                        objects,
                        functions,
                        ports,
                        queries,
                    }
                },
            )
            .boxed()
    }

    proptest! {
        // A generated schema is large (six lists of nested definitions): 256 of them take as long
        // as the closure writer's 2048 closures.
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// On any schema (valid or not: any names, docs, flags and orders, equal keys, lists on
        /// both sides of the sort's threshold) the writer's bytes are the bytes `serde_json`
        /// writes: for the doc-stripped, sorted clone (so the hash is the hash it always was), and
        /// for the schema as it is (the exchange form, which `serde_json` reads back).
        #[test]
        fn the_writer_is_serde_json_in_both_forms(schema in arb_schema()) {
            let by_serde = schema.canonical_json_by_serde();
            prop_assert_eq!(&canonical(&schema), &by_serde);
            prop_assert_eq!(schema.hash(), crate::ids::fnv1a64(by_serde.as_bytes()));
            let full = exchange(&schema);
            prop_assert_eq!(&full, &schema.to_json_by_serde());
            prop_assert_eq!(Schema::from_json(&full).unwrap(), schema);
        }
    }
}
