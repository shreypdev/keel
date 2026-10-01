//! Reading encoded values by their schema: cache-key rendering and error classification.
//!
//! Two things in the query layer need to look *inside* encoded values without knowing their
//! Rust types, because the traits the macros implement ([`QueryDef`](crate::QueryDef),
//! [`MutationDef`](crate::MutationDef)) carry types but not names:
//!
//! * **Key rendering.** A cache key template such as `todos:{page}` names its parameters, and
//!   `invalidate("todos:3")` must match the entry of page 3. The parameter names live in the
//!   schema (`QueryMeta`), the values in the encoded parameter bytes; [`KeyPlan`] joins them.
//! * **Network errors.** The offline queue (SPEC 9) replays an idempotent mutation that failed
//!   with `HttpError::Network`. The error type is the app's own enum, which usually wraps
//!   `HttpError` (`#[from]`), so [`probe_network_error`] walks the encoded error by its schema
//!   type looking for that variant, at any depth.
//!
//! Both are total: a value that does not match its schema is an `Err`, never a panic.

use undra_meta::{EnumDef, ParamDef, RecordDef, Schema, TypeRef};
use undra_wire::{Reader, Uuid, WireError};

/// How deeply values may nest before the walk gives up (recursive schema types).
const MAX_DEPTH: u32 = 24;

/// The name of the standard error whose `Network` variant means "no connectivity".
const HTTP_ERROR: &str = "HttpError";
/// The variant of [`HTTP_ERROR`] that the offline queue treats as a network failure.
const NETWORK_VARIANT: &str = "Network";

/// What one walk over an encoded value produced.
#[derive(Debug, Default)]
struct Walk {
    /// The value as text (for keys).
    text: String,
    /// Whether an `HttpError::Network` was met while walking.
    network: bool,
}

fn bad(ty: &'static str, reader: &Reader<'_>) -> WireError {
    WireError::InvalidTag {
        tag: 0,
        at: reader.position(),
        ty,
    }
}

fn hex(bytes: &[u8], out: &mut String) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
}

fn find_enum<'a>(schema: &'a Schema, name: &str) -> Option<&'a EnumDef> {
    schema.enums.iter().find(|e| e.name == name)
}

fn find_record<'a>(schema: &'a Schema, name: &str) -> Option<&'a RecordDef> {
    schema.records.iter().find(|r| r.name == name)
}

/// Consumes one value of type `ty` from `r`, appending its text and noting network errors.
fn walk(
    schema: &Schema,
    ty: &TypeRef,
    r: &mut Reader<'_>,
    depth: u32,
    out: &mut Walk,
) -> Result<(), WireError> {
    if depth > MAX_DEPTH {
        return Err(WireError::NestingTooDeep { at: r.position() });
    }
    let text = &mut out.text;
    match ty {
        TypeRef::Bool => text.push_str(if r.read_bool()? { "true" } else { "false" }),
        TypeRef::I8 => text.push_str(&r.read_i8()?.to_string()),
        TypeRef::I16 => text.push_str(&r.read_i16()?.to_string()),
        TypeRef::I32 => text.push_str(&r.read_i32()?.to_string()),
        TypeRef::I64 | TypeRef::Duration | TypeRef::Timestamp => {
            text.push_str(&r.read_i64()?.to_string());
        }
        TypeRef::U8 => text.push_str(&r.read_u8()?.to_string()),
        TypeRef::U16 => text.push_str(&r.read_u16()?.to_string()),
        TypeRef::U32 => text.push_str(&r.read_u32()?.to_string()),
        TypeRef::U64 => text.push_str(&r.read_u64()?.to_string()),
        TypeRef::F32 => text.push_str(&r.read_f32()?.to_string()),
        TypeRef::F64 => text.push_str(&r.read_f64()?.to_string()),
        TypeRef::String => text.push_str(r.read_str()?),
        TypeRef::Bytes => hex(r.read_bytes()?, text),
        TypeRef::Unit => {}
        TypeRef::Uuid => text.push_str(&Uuid(r.read_array::<16>()?).to_string()),
        TypeRef::Option(inner) => match r.read_u8()? {
            0 => text.push_str("none"),
            1 => walk(schema, inner, r, depth + 1, out)?,
            _ => return Err(bad("Option", r)),
        },
        TypeRef::Vec(item) => {
            let count = r.read_count(1)?;
            out.text.push('[');
            for i in 0..count {
                if i > 0 {
                    out.text.push(',');
                }
                walk(schema, item, r, depth + 1, out)?;
            }
            out.text.push(']');
        }
        TypeRef::Map(key, value) => {
            let count = r.read_count(1)?;
            out.text.push('{');
            for i in 0..count {
                if i > 0 {
                    out.text.push(',');
                }
                walk(schema, key, r, depth + 1, out)?;
                out.text.push(':');
                walk(schema, value, r, depth + 1, out)?;
            }
            out.text.push('}');
        }
        TypeRef::Named(name) => {
            if let Some(en) = find_enum(schema, name) {
                let at = r.position();
                let index = r.read_u16()?;
                let Some(variant) = en.variants.iter().find(|v| v.index == index) else {
                    return Err(WireError::InvalidTag {
                        tag: u32::from(index),
                        at,
                        ty: "enum",
                    });
                };
                if en.name == HTTP_ERROR && variant.name == NETWORK_VARIANT {
                    out.network = true;
                }
                out.text.push_str(&variant.name);
                walk_fields(schema, &variant.fields, r, depth, out, '(', ')')?;
            } else if let Some(record) = find_record(schema, name) {
                walk_fields(schema, &record.fields, r, depth, out, '(', ')')?;
            } else {
                // An object handle or a name the schema does not define: not a value.
                return Err(bad("named type", r));
            }
        }
        TypeRef::Lazy(_) | TypeRef::Result(..) | TypeRef::Stream(_) => {
            return Err(bad("type that cannot be a value", r));
        }
    }
    Ok(())
}

/// Walks `fields` in order; wraps them in `open` and `close` unless there are none.
fn walk_fields(
    schema: &Schema,
    fields: &[undra_meta::FieldDef],
    r: &mut Reader<'_>,
    depth: u32,
    out: &mut Walk,
    open: char,
    close: char,
) -> Result<(), WireError> {
    if fields.is_empty() {
        return Ok(());
    }
    out.text.push(open);
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            out.text.push(',');
        }
        walk(schema, &field.ty, r, depth + 1, out)?;
    }
    out.text.push(close);
    Ok(())
}

/// Whether the encoded error `bytes` of type `ty` contains an `HttpError::Network`, at any
/// depth (a `TodoError::Http(HttpError::Network(..))`, say). A value that does not decode as
/// `ty` is not a network error.
pub(crate) fn probe_network_error(schema: &Schema, ty: &TypeRef, bytes: &[u8]) -> bool {
    let mut r = Reader::new(bytes);
    let mut out = Walk::default();
    walk(schema, ty, &mut r, 0, &mut out).is_ok() && out.network
}

/// The error type `E` of a `Result<T, E>` return type.
pub(crate) fn error_type(returns: &TypeRef) -> Option<&TypeRef> {
    match returns {
        TypeRef::Result(_, err) => Some(err),
        _ => None,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Segment {
    Text(String),
    Param(usize),
}

/// A cache key template compiled against the parameter list of its query: renders the
/// `{param}` placeholders from the encoded parameters.
#[derive(Debug)]
pub(crate) struct KeyPlan {
    segments: Vec<Segment>,
    params: Vec<TypeRef>,
}

impl KeyPlan {
    /// Compiles `template`. With no parameter list (the query has no schema entry, as in a
    /// hand-written definition) the template is kept as it is; a placeholder that names no
    /// parameter stays literal too.
    pub(crate) fn new(template: &str, params: Option<&[ParamDef]>) -> KeyPlan {
        let params = params.unwrap_or_default();
        let mut segments = Vec::new();
        let mut text = String::new();
        let mut rest = template;
        while let Some(open) = rest.find('{') {
            let Some(len) = rest[open..].find('}') else {
                break;
            };
            let name = &rest[open + 1..open + len];
            match params.iter().position(|p| p.name == name) {
                Some(index) => {
                    text.push_str(&rest[..open]);
                    if !text.is_empty() {
                        segments.push(Segment::Text(std::mem::take(&mut text)));
                    }
                    segments.push(Segment::Param(index));
                }
                None => text.push_str(&rest[..open + len + 1]),
            }
            rest = &rest[open + len + 1..];
        }
        text.push_str(rest);
        if !text.is_empty() {
            segments.push(Segment::Text(text));
        }
        KeyPlan {
            segments,
            params: params.iter().map(|p| p.ty.clone()).collect(),
        }
    }

    /// The key for `encoded` parameters. A placeholder whose parameter cannot be read renders
    /// as `?`, so a malformed value still yields a stable key.
    pub(crate) fn render(&self, schema: &Schema, encoded: &[u8]) -> String {
        if self.segments.iter().all(|s| matches!(s, Segment::Text(_))) {
            return self.segments.iter().fold(String::new(), |mut acc, s| {
                if let Segment::Text(t) = s {
                    acc.push_str(t);
                }
                acc
            });
        }
        let mut r = Reader::new(encoded);
        let mut rendered = Vec::with_capacity(self.params.len());
        for ty in &self.params {
            let mut out = Walk::default();
            match walk(schema, ty, &mut r, 0, &mut out) {
                Ok(()) => rendered.push(Some(out.text)),
                Err(_) => break,
            }
        }
        let mut key = String::new();
        for segment in &self.segments {
            match segment {
                Segment::Text(t) => key.push_str(t),
                Segment::Param(i) => match rendered.get(*i) {
                    Some(Some(text)) => key.push_str(text),
                    _ => key.push('?'),
                },
            }
        }
        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use undra_meta::{FieldDef, VariantDef};
    use undra_wire::Encode;

    fn field(name: &str, ty: TypeRef) -> FieldDef {
        FieldDef {
            name: name.to_owned(),
            ty,
            default: false,
            docs: String::new(),
        }
    }

    fn variant(name: &str, index: u16, fields: Vec<FieldDef>) -> VariantDef {
        VariantDef {
            name: name.to_owned(),
            index,
            tuple: true,
            message: None,
            fields,
            docs: String::new(),
        }
    }

    fn enum_def(name: &str, is_error: bool, variants: Vec<VariantDef>) -> EnumDef {
        EnumDef {
            name: name.to_owned(),
            type_id: 0,
            is_error,
            variants,
            docs: String::new(),
        }
    }

    /// A schema with the standard `HttpError` (as undra-ports declares it), an app error that
    /// wraps it, an enum, and a record.
    fn schema() -> Schema {
        let mut schema = Schema::new("test");
        schema.enums.push(enum_def(
            "HttpError",
            true,
            vec![
                variant("Network", 0, vec![field("0", TypeRef::String)]),
                variant("Timeout", 1, vec![]),
                variant("Cancelled", 2, vec![]),
                variant("InvalidUrl", 3, vec![field("0", TypeRef::String)]),
            ],
        ));
        schema.enums.push(enum_def(
            "AppError",
            true,
            vec![
                variant("Empty", 0, vec![]),
                variant("Http", 1, vec![field("0", TypeRef::named("HttpError"))]),
                variant(
                    "Batch",
                    2,
                    vec![field("0", TypeRef::vec(TypeRef::named("AppError")))],
                ),
            ],
        ));
        schema.enums.push(enum_def(
            "Filter",
            false,
            vec![variant("All", 0, vec![]), variant("Done", 1, vec![])],
        ));
        schema.records.push(RecordDef {
            name: "Point".to_owned(),
            type_id: 0,
            fields: vec![field("x", TypeRef::I32), field("y", TypeRef::I32)],
            docs: String::new(),
        });
        schema
    }

    use proptest::prelude::{any, prop_assert_eq, proptest};

    proptest! {
        #[test]
        fn rendering_matches_plain_formatting_for_arbitrary_values(
            a in any::<u32>(), b in "[a-z0-9 :_-]{0,12}", c in any::<bool>(), d in any::<Option<i64>>(),
        ) {
            let params = [
                param("a", TypeRef::U32),
                param("b", TypeRef::String),
                param("c", TypeRef::Bool),
                param("d", TypeRef::option(TypeRef::I64)),
            ];
            let mut w = undra_wire::Writer::new();
            a.encode(&mut w);
            b.encode(&mut w);
            c.encode(&mut w);
            d.encode(&mut w);
            let expected = format!(
                "k:{a}:{b}:{c}:{}",
                d.map_or_else(|| "none".to_owned(), |n| n.to_string())
            );
            prop_assert_eq!(render("k:{a}:{b}:{c}:{d}", &params, w.as_slice()), expected);
        }

        /// Random bytes are never a panic: a walk either reads a value or says it cannot.
        #[test]
        fn arbitrary_bytes_never_panic_the_walk(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let schema = schema();
            let plan = KeyPlan::new(
                "{a}/{b}/{c}",
                Some(&[
                    param("a", TypeRef::named("AppError")),
                    param("b", TypeRef::vec(TypeRef::named("Point"))),
                    param("c", TypeRef::map(TypeRef::String, TypeRef::option(TypeRef::named("Filter")))),
                ]),
            );
            let _ = plan.render(&schema, &bytes);
            let _ = probe_network_error(&schema, &TypeRef::named("AppError"), &bytes);
            let _ = probe_network_error(&schema, &TypeRef::named("HttpError"), &bytes);
        }
    }

    fn param(name: &str, ty: TypeRef) -> ParamDef {
        ParamDef {
            name: name.to_owned(),
            ty,
        }
    }

    fn render(template: &str, params: &[ParamDef], encoded: &[u8]) -> String {
        KeyPlan::new(template, Some(params)).render(&schema(), encoded)
    }

    #[test]
    fn primitives_render_as_their_natural_text() {
        let params = [
            param("page", TypeRef::U32),
            param("q", TypeRef::String),
            param("on", TypeRef::Bool),
            param("delta", TypeRef::I64),
        ];
        let mut w = undra_wire::Writer::new();
        3_u32.encode(&mut w);
        "milk".to_owned().encode(&mut w);
        true.encode(&mut w);
        (-7_i64).encode(&mut w);
        assert_eq!(
            render("todos:{page}:{q}:{on}:{delta}", &params, w.as_slice()),
            "todos:3:milk:true:-7"
        );
    }

    #[test]
    fn text_between_and_after_placeholders_is_kept_and_order_is_the_templates() {
        let params = [param("a", TypeRef::U8), param("b", TypeRef::U8)];
        assert_eq!(render("x{b}-{a}!", &params, &[1, 2]), "x2-1!");
        assert_eq!(render("plain", &params, &[1, 2]), "plain");
        assert_eq!(render("{a}{a}", &params, &[9, 2]), "99");
        assert_eq!(render("", &params, &[9, 2]), "");
    }

    #[test]
    fn a_template_without_parameters_or_schema_is_literal() {
        assert_eq!(render("count", &[], &[]), "count");
        let plan = KeyPlan::new("todos:{page}", None);
        assert_eq!(plan.render(&schema(), &[1, 0, 0, 0]), "todos:{page}");
        // A placeholder that names no parameter stays as written.
        let params = [param("page", TypeRef::U32)];
        assert_eq!(
            render("t:{nope}:{page}", &params, &[5, 0, 0, 0]),
            "t:{nope}:5"
        );
        // An unterminated brace is text.
        assert_eq!(render("t:{page", &params, &[5, 0, 0, 0]), "t:{page");
    }

    #[test]
    fn compound_values_render_structurally() {
        let params = [
            param("ids", TypeRef::vec(TypeRef::U8)),
            param("opt", TypeRef::option(TypeRef::U8)),
            param("none", TypeRef::option(TypeRef::U8)),
            param("filter", TypeRef::named("Filter")),
            param("at", TypeRef::named("Point")),
            param("id", TypeRef::Uuid),
            param("blob", TypeRef::Bytes),
        ];
        let mut w = undra_wire::Writer::new();
        vec![1_u8, 2, 3].encode(&mut w);
        Some(7_u8).encode(&mut w);
        None::<u8>.encode(&mut w);
        w.write_u16(1); // Filter::Done
        4_i32.encode(&mut w);
        (-5_i32).encode(&mut w);
        w.write_raw(&[0xab; 16]);
        w.write_bytes(&[0xde, 0xad]);
        assert_eq!(
            render(
                "k:{ids}:{opt}:{none}:{filter}:{at}:{id}:{blob}",
                &params,
                w.as_slice()
            ),
            "k:[1,2,3]:7:none:Done:(4,-5):abababab-abab-abab-abab-abababababab:dead"
        );
    }

    #[test]
    fn a_value_that_does_not_decode_renders_as_a_question_mark() {
        let params = [param("page", TypeRef::U32), param("q", TypeRef::String)];
        // Only two bytes: `page` cannot be read, and neither can anything after it.
        assert_eq!(render("t:{page}:{q}", &params, &[1, 0]), "t:?:?");
        // The first parameter is fine, the second is cut off.
        assert_eq!(render("t:{page}:{q}", &params, &[1, 0, 0, 0, 9]), "t:1:?");
    }

    #[test]
    fn network_errors_are_found_at_any_depth() {
        let schema = schema();
        let ty = TypeRef::named("AppError");
        let direct = {
            let mut w = undra_wire::Writer::new();
            w.write_u16(1); // AppError::Http
            w.write_u16(0); // HttpError::Network
            "dns".to_owned().encode(&mut w);
            w.into_vec()
        };
        assert!(probe_network_error(&schema, &ty, &direct));

        let timeout = {
            let mut w = undra_wire::Writer::new();
            w.write_u16(1);
            w.write_u16(1); // HttpError::Timeout
            w.into_vec()
        };
        assert!(!probe_network_error(&schema, &ty, &timeout));
        assert!(!probe_network_error(&schema, &ty, &[0, 0])); // AppError::Empty

        // Nested inside a Vec of the error itself.
        let nested = {
            let mut w = undra_wire::Writer::new();
            w.write_u16(2); // AppError::Batch
            w.write_u32(2);
            w.write_u16(0); // Empty
            w.write_u16(1); // Http
            w.write_u16(0); // Network
            "x".to_owned().encode(&mut w);
            w.into_vec()
        };
        assert!(probe_network_error(&schema, &ty, &nested));

        // HttpError itself.
        let mut w = undra_wire::Writer::new();
        w.write_u16(0);
        "x".to_owned().encode(&mut w);
        assert!(probe_network_error(
            &schema,
            &TypeRef::named("HttpError"),
            w.as_slice()
        ));
    }

    #[test]
    fn garbage_is_not_a_network_error() {
        let schema = schema();
        let ty = TypeRef::named("AppError");
        assert!(!probe_network_error(&schema, &ty, &[]));
        assert!(!probe_network_error(&schema, &ty, &[9, 9]));
        assert!(!probe_network_error(
            &schema,
            &ty,
            &[1, 0, 0, 0, 200, 0, 0, 0]
        ));
        assert!(!probe_network_error(
            &schema,
            &TypeRef::named("Nope"),
            &[0, 0]
        ));
        assert!(!probe_network_error(&schema, &TypeRef::U8, &[1]));
    }

    #[test]
    fn deep_nesting_is_refused_not_overflowed() {
        // Batch inside Batch inside Batch ... one level per 6 bytes; deeper than MAX_DEPTH.
        let schema = schema();
        let mut bytes = Vec::new();
        for _ in 0..200 {
            bytes.extend_from_slice(&2_u16.to_le_bytes());
            bytes.extend_from_slice(&1_u32.to_le_bytes());
        }
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        assert!(!probe_network_error(
            &schema,
            &TypeRef::named("AppError"),
            &bytes
        ));
    }

    #[test]
    fn error_type_extracts_the_err_side() {
        let returns = TypeRef::result(TypeRef::U8, TypeRef::named("E"));
        assert_eq!(error_type(&returns), Some(&TypeRef::named("E")));
        assert_eq!(error_type(&TypeRef::U8), None);
    }
}
