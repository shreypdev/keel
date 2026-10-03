//! The text of a schema file: what `undra schema export` writes and `undra schema diff` reads
//! (ADR-062).
//!
//! It is the exchange form of the schema (`Schema::to_json_pretty`, SPEC 2.3) with one change for
//! the reader: every type is written on one line (`"ty": {"kind":"option","of":{"kind":"named","of":"Todo"}}`,
//! the compact form SPEC 2.1 gives) where the pretty printer spreads it over three to nine. The file
//! is committed and reviewed, and a field that became optional should be one changed line, not six.
//! Both forms are the same JSON: `parse_schema_json` reads either, and so does `undra bindgen --schema`.

use undra_meta::Schema;

/// The schema as the text of a schema file: pretty JSON, every type on one line, ending in a newline.
///
/// ```
/// use undra_meta::{FieldDef, RecordDef, Schema, TypeRef};
///
/// let mut schema = Schema::new("demo-core");
/// schema.records.push(RecordDef {
///     name: "Todo".into(),
///     type_id: 1,
///     fields: vec![FieldDef {
///         name: "due".into(),
///         ty: TypeRef::option(TypeRef::Timestamp),
///         default: true,
///         docs: String::new(),
///     }],
///     transparent: false,
///     docs: String::new(),
/// });
/// let text = undra_cli::schema_file::render(&schema);
/// assert!(text.contains(r#""ty": {"kind":"option","of":{"kind":"timestamp"}}"#));
/// assert_eq!(Schema::from_json(&text).unwrap(), schema);
/// ```
#[must_use]
pub fn render(schema: &Schema) -> String {
    let pretty = schema.to_json_pretty();
    let lines: Vec<&str> = pretty.lines().collect();
    let mut out = String::with_capacity(pretty.len());
    let mut at = 0;
    while at < lines.len() {
        match collapsed_type(&lines, at) {
            Some((line, next)) => {
                out.push_str(&line);
                at = next;
            }
            None => {
                out.push_str(lines[at]);
                at += 1;
            }
        }
        out.push('\n');
    }
    out
}

/// A type that starts at `lines[at]` (`<indent>"key": {` followed by a `"kind"` member), as one
/// line, and the index of the line after it. `None` when `lines[at]` does not open a type.
///
/// A type is the only object the pretty printer writes with `kind` as its first member: every
/// definition starts with its `name`. The block up to the closing brace of the same indentation is
/// parsed and written compactly, so what comes out is the same JSON.
fn collapsed_type(lines: &[&str], at: usize) -> Option<(String, usize)> {
    let open = lines[at];
    let head = open.strip_suffix('{')?;
    if !lines.get(at + 1)?.trim_start().starts_with("\"kind\": ") {
        return None;
    }
    let indent = open.len() - open.trim_start().len();
    let close = (at + 1..lines.len()).find(|&i| {
        let line = lines[i];
        line.len() - line.trim_start().len() == indent && matches!(line.trim(), "}" | "},")
    })?;
    let comma = lines[close].trim() == "},";
    let mut text = String::from("{");
    for line in &lines[at + 1..close] {
        text.push_str(line.trim());
    }
    text.push('}');
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let object = value.as_object()?;
    if !object.keys().all(|k| k == "kind" || k == "of") {
        return None;
    }
    let compact = serde_json::to_string(&value).ok()?;
    Some((
        format!("{head}{compact}{}", if comma { "," } else { "" }),
        close + 1,
    ))
}

#[cfg(test)]
mod tests {
    use undra_meta::{
        FieldDef, FunctionDef, MethodDef, ParamDef, RecordDef, SignalDef, StoreDef, TypeRef, ids,
    };

    use super::*;

    fn schema() -> Schema {
        let mut s = Schema::new("demo-core");
        s.records.push(RecordDef {
            name: "Todo".into(),
            type_id: ids::type_id("Todo"),
            fields: vec![
                FieldDef {
                    name: "title".into(),
                    ty: TypeRef::String,
                    default: false,
                    docs: "The title.".into(),
                },
                FieldDef {
                    name: "tags".into(),
                    ty: TypeRef::map(TypeRef::String, TypeRef::vec(TypeRef::named("Tag"))),
                    default: true,
                    docs: String::new(),
                },
            ],
            transparent: false,
            docs: String::new(),
        });
        s.functions.push(FunctionDef {
            name: "find".into(),
            method_id: ids::function_id("find"),
            params: vec![ParamDef {
                name: "id".into(),
                ty: TypeRef::option(TypeRef::U32),
            }],
            returns: TypeRef::result(TypeRef::named("Todo"), TypeRef::named("TodoError")),
            is_async: true,
            takes_ctx: false,
            generic: None,
            docs: String::new(),
        });
        s.objects.push(undra_meta::ObjectDef {
            name: "Todos".into(),
            type_id: ids::type_id("Todos"),
            constructors: vec![],
            methods: vec![MethodDef {
                name: "clear".into(),
                method_id: ids::method_id("Todos", "clear"),
                params: vec![],
                returns: TypeRef::Unit,
                is_async: false,
                takes_ctx: false,
                coalesce: false,
                generic: None,
                docs: String::new(),
            }],
            store: Some(StoreDef {
                signals: vec![SignalDef {
                    name: "items".into(),
                    signal_id: 0,
                    ty: TypeRef::vec(TypeRef::named("Todo")),
                    computed: false,
                    key: Some("id".into()),
                    no_coalesce: false,
                    default: false,
                }],
            }),
            docs: String::new(),
        });
        s
    }

    #[test]
    fn every_type_is_one_line_and_the_json_is_the_same() {
        let schema = schema();
        let text = render(&schema);
        for expected in [
            r#""ty": {"kind":"string"},"#,
            r#""ty": {"kind":"map","of":[{"kind":"string"},{"kind":"vec","of":{"kind":"named","of":"Tag"}}]},"#,
            r#""returns": {"kind":"result","of":[{"kind":"named","of":"Todo"},{"kind":"named","of":"TodoError"}]},"#,
            r#""ty": {"kind":"option","of":{"kind":"u32"}}"#,
        ] {
            assert!(text.contains(expected), "missing {expected} in:\n{text}");
        }
        assert!(text.ends_with("}\n"));
        assert_eq!(Schema::from_json(&text).unwrap(), schema, "{text}");
        // A reader that does not know the form (a hash check of the canonical JSON) sees the same schema.
        assert_eq!(Schema::from_json(&text).unwrap().hash(), schema.hash());
        // Shorter than the pretty printer's, and a field's type no longer spans lines.
        assert!(text.lines().count() < schema.to_json_pretty().lines().count());
    }

    #[test]
    fn what_is_not_a_type_is_left_alone() {
        let text = render(&schema());
        // Definitions keep their members on their own lines, in declaration order.
        assert!(
            text.contains("\"name\": \"Todo\",\n      \"type_id\":")
                || text.contains("\"name\": \"Todo\",\n"),
            "{text}"
        );
        assert!(text.contains("\"crate_name\": \"demo-core\""));
        assert!(text.contains("\"key\": \"id\""));
        // A port's `kind` is a plain string among other members, not a type.
        let mut with_port = schema();
        with_port.ports.push(undra_meta::PortDef {
            name: "Kv".into(),
            port_id: 1,
            kind: undra_meta::PortKind::Async,
            background: false,
            methods: vec![],
            docs: String::new(),
        });
        let text = render(&with_port);
        assert!(text.contains("\"kind\": \"async\""), "{text}");
        assert_eq!(Schema::from_json(&text).unwrap(), with_port);
    }

    #[test]
    fn the_text_is_the_same_every_time() {
        assert_eq!(render(&schema()), render(&schema()));
    }
}
