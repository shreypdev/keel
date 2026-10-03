//! The text of a schema file: what `undra schema export` writes and `undra schema diff` reads
//! (ADR-062).
//!
//! It is the exchange form of the schema (`Schema::to_json_pretty`, SPEC 2.3) written for a reviewer:
//! every type is one line (`"ty": {"kind":"option","of":{"kind":"named","of":"Todo"}}`, the compact
//! form SPEC 2.1 gives) where the pretty printer spreads it over three to nine, and so is every small
//! definition that holds nothing nested (a field, a parameter, a case without a payload) when it fits
//! in [`WIDTH`] columns. The file is committed and reviewed: a field that became optional is one
//! changed line, not six, and the whole file is a third shorter than the pretty form. Both forms are
//! the same JSON: `parse_schema_json` reads either, and so does `undra bindgen --schema`.

use undra_meta::Schema;

/// A pass over the lines of the pretty form: the collapsed line starting at an index and the index after it.
type Collapse = fn(&[String], usize) -> Option<(String, usize)>;

/// The widest line a small definition is collapsed into.
pub const WIDTH: usize = 140;

/// The schema as the text of a schema file: pretty JSON, every type and every small definition on one
/// line, ending in a newline.
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
/// assert!(text.contains(
///     r#"{ "name": "due", "ty": {"kind":"option","of":{"kind":"timestamp"}}, "default": true }"#
/// ));
/// assert_eq!(Schema::from_json(&text).unwrap(), schema);
/// ```
#[must_use]
pub fn render(schema: &Schema) -> String {
    let pretty = schema.to_json_pretty();
    let pass = |lines: &[String], collapse: Collapse| {
        let mut out = Vec::with_capacity(lines.len());
        let mut at = 0;
        while at < lines.len() {
            match collapse(lines, at) {
                Some((line, next)) => {
                    out.push(line);
                    at = next;
                }
                None => {
                    out.push(lines[at].clone());
                    at += 1;
                }
            }
        }
        out
    };
    let lines: Vec<String> = pretty.lines().map(str::to_owned).collect();
    let mut text = pass(&pass(&lines, collapsed_type), collapsed_leaf).join("\n");
    text.push('\n');
    text
}

/// A type that starts at `lines[at]` (`<indent>"key": {` followed by a `"kind"` member), as one
/// line, and the index of the line after it. `None` when `lines[at]` does not open a type.
///
/// A type is the only object the pretty printer writes with `kind` as its first member: every
/// definition starts with its `name`. The block up to the closing brace of the same indentation is
/// parsed and written compactly, so what comes out is the same JSON.
fn collapsed_type(lines: &[String], at: usize) -> Option<(String, usize)> {
    let open = lines[at].as_str();
    let head = open.strip_suffix('{')?;
    if !lines.get(at + 1)?.trim_start().starts_with("\"kind\": ") {
        return None;
    }
    let indent = open.len() - open.trim_start().len();
    let close = closing_line(lines, at, indent)?;
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

/// The line of `lines` after `at` that closes the object opened there: the first of the same
/// indentation that is `}` or `},`.
fn closing_line(lines: &[String], at: usize, indent: usize) -> Option<usize> {
    (at + 1..lines.len()).find(|&i| {
        let line = &lines[i];
        line.len() - line.trim_start().len() == indent && matches!(line.trim(), "}" | "},")
    })
}

/// A small definition that starts at `lines[at]` (`<indent>{` or `<indent>"key": {`) and holds
/// nothing nested, as one line `{ "name": "id", "ty": {..}, "default": false }` of at most [`WIDTH`]
/// columns, and the index of the line after it. A line that ends in `{` or `[` opens something, so
/// an object with such a line inside it (after the types are collapsed) is not small. Members keep
/// their order: the line is the text of the pretty form joined, not a reserialisation.
fn collapsed_leaf(lines: &[String], at: usize) -> Option<(String, usize)> {
    let open = lines[at].as_str();
    let head = open.strip_suffix('{')?;
    let indent = open.len() - open.trim_start().len();
    let close = closing_line(lines, at, indent)?;
    let inner = &lines[at + 1..close];
    if inner.is_empty() || inner.iter().any(|l| l.ends_with('{') || l.ends_with('[')) {
        return None;
    }
    let comma = lines[close].trim() == "},";
    let members = inner.iter().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
    let object = format!("{{ {members} }}");
    let line = format!("{head}{object}{}", if comma { "," } else { "" });
    if line.len() > WIDTH || serde_json::from_str::<serde_json::Value>(&object).is_err() {
        return None;
    }
    Some((line, close + 1))
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
    fn every_type_and_small_definition_is_one_line_and_the_json_is_the_same() {
        let schema = schema();
        let text = render(&schema);
        for expected in [
            // A field is one line, its type in the compact form of SPEC 2.1.
            r#"{ "name": "title", "ty": {"kind":"string"}, "default": false, "docs": "The title." }"#,
            // A type is one line even when its field is too wide to be.
            r#""ty": {"kind":"map","of":[{"kind":"string"},{"kind":"vec","of":{"kind":"named","of":"Tag"}}]},"#,
            r#""returns": {"kind":"result","of":[{"kind":"named","of":"Todo"},{"kind":"named","of":"TodoError"}]},"#,
            r#"{ "name": "id", "ty": {"kind":"option","of":{"kind":"u32"}} }"#,
            r#"{ "name": "items", "signal_id": 0, "ty": {"kind":"vec","of":{"kind":"named","of":"Todo"}}, "computed": false, "key": "id" }"#,
        ] {
            assert!(text.contains(expected), "missing {expected} in:\n{text}");
        }
        assert!(text.ends_with("}\n"));
        assert_eq!(Schema::from_json(&text).unwrap(), schema, "{text}");
        // A reader that does not know the form (a hash check of the canonical JSON) sees the same schema.
        assert_eq!(Schema::from_json(&text).unwrap().hash(), schema.hash());
        // Much shorter than the pretty printer's.
        let pretty = schema.to_json_pretty().lines().count();
        assert!(
            text.lines().count() * 3 < pretty * 2,
            "{} lines against {pretty}:\n{text}",
            text.lines().count()
        );
        // No line is wider than a small definition may be, but for one that is not small.
        assert!(text.lines().all(|l| l.len() <= WIDTH), "{text}");
    }

    #[test]
    fn what_is_not_small_is_left_alone_and_members_keep_their_order() {
        let text = render(&schema());
        // A definition that nests other definitions keeps one member to a line, in declaration order.
        assert!(
            text.contains("      \"name\": \"Todo\",\n      \"type_id\":"),
            "{text}"
        );
        assert!(text.contains("\"crate_name\": \"demo-core\""));
        // Declaration order, not alphabetical: `name` before `ty` before `default`.
        let line = text.lines().find(|l| l.contains("\"title\"")).unwrap();
        assert!(
            line.find("\"name\"") < line.find("\"ty\"")
                && line.find("\"ty\"") < line.find("\"default\""),
            "{line}"
        );
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
    fn a_definition_wider_than_the_limit_stays_on_its_lines_and_text_with_braces_survives() {
        let mut s = schema();
        s.records[0].fields.push(FieldDef {
            name: "long".into(),
            ty: TypeRef::map(TypeRef::String, TypeRef::String),
            default: true,
            docs: "x".repeat(WIDTH),
        });
        s.records[0].docs = "A { brace, a [ bracket and a \": \" in a doc.".into();
        let text = render(&s);
        assert_eq!(Schema::from_json(&text).unwrap(), s, "{text}");
        assert!(
            text.lines().any(|l| l.trim() == "\"name\": \"long\","),
            "{text}"
        );
    }

    #[test]
    fn the_text_is_the_same_every_time() {
        assert_eq!(render(&schema()), render(&schema()));
    }
}
