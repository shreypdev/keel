//! The Kotlin generator (SPEC section 10.2).
//!
//! Output: `src/main/kotlin/<package path>/{Types,Errors,Objects,Stores,Ports,Queries,Ids}.kt`.
//! Generated code depends only on `dev.keel.runtime` and `dev.keel.runtime.wire`
//! (SPEC section 17.2) and kotlinx-coroutines.
//!
//! Codec composition is hoisted: a `List<String>` field does not build
//! `Codecs.vec(Codecs.string)` on every call, the file declares one private
//! property per composite at its end.
//!
//! Inside a sealed hierarchy the variant classes are nested, so a variant
//! called `String` or `Todo` would shadow the type of the same name in the
//! variant's own field types. Types that a variant of the enclosing sealed type
//! shadows are written fully qualified there.

use std::collections::{BTreeSet, HashMap, HashSet};

use keel_meta::{
    EnumDef, FieldDef, FunctionDef, MethodDef, ObjectDef, ParamDef, PortDef, PortKind, RecordDef,
    SignalDef, TypeRef, VariantDef,
};

use crate::emit::CodeWriter;
use crate::model::{Model, MsgPart, NamedKind, Ret, doc_lines, is_unit_enum, parse_message};
use crate::naming;
use crate::{GeneratedFile, Generator};

const FILES: [&str; 7] = [
    "Types", "Errors", "Objects", "Stores", "Ports", "Queries", "Ids",
];

pub(crate) fn generate(model: &Model, cfg: &Generator) -> Vec<GeneratedFile> {
    let kt = KtGen { model, cfg };
    let dir = format!("src/main/kotlin/{}", cfg.kotlin_package.replace('.', "/"));
    let bodies = [
        kt.types_file(),
        kt.errors_file(),
        kt.objects_file(),
        kt.stores_file(),
        kt.ports_file(),
        kt.queries_file(),
        kt.ids_file(),
    ];
    FILES
        .iter()
        .zip(bodies)
        .map(|(name, contents)| GeneratedFile {
            path: format!("{dir}/{name}.kt"),
            contents,
        })
        .collect()
}

/// Names of Kotlin standard-library types the generated code writes and their
/// fully qualified spelling.
const QUALIFIED: &[(&str, &str)] = &[
    ("Boolean", "kotlin.Boolean"),
    ("Byte", "kotlin.Byte"),
    ("ByteArray", "kotlin.ByteArray"),
    ("Double", "kotlin.Double"),
    ("Float", "kotlin.Float"),
    ("Int", "kotlin.Int"),
    ("List", "kotlin.collections.List"),
    ("Long", "kotlin.Long"),
    ("Map", "kotlin.collections.Map"),
    ("Short", "kotlin.Short"),
    ("String", "kotlin.String"),
    ("UByte", "kotlin.UByte"),
    ("UInt", "kotlin.UInt"),
    ("ULong", "kotlin.ULong"),
    ("UShort", "kotlin.UShort"),
    ("Unit", "kotlin.Unit"),
    ("Duration", "kotlin.time.Duration"),
    ("UUID", "java.util.UUID"),
    ("Timestamp", "dev.keel.runtime.wire.Timestamp"),
];

/// Kotlin names a `Throwable` already has; error fields called this are
/// renamed.
const THROWABLE_MEMBERS: &[&str] = &[
    "message",
    "cause",
    "stackTrace",
    "suppressed",
    "localizedMessage",
];

fn hex(id: u32) -> String {
    format!("0x{id:08x}u")
}

fn kt_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Literal text inside a string template: like [`kt_string`] without quotes.
fn kt_template_text(s: &str) -> String {
    let quoted = kt_string(s);
    quoted[1..quoted.len() - 1].to_owned()
}

/// A KDoc block; nothing for empty `docs` and no `extra` lines.
fn kdoc(w: &mut CodeWriter, docs: &str, extra: &[String]) {
    let mut lines = doc_lines(docs);
    lines.extend(extra.iter().cloned());
    if lines.is_empty() {
        return;
    }
    let lines: Vec<String> = lines
        .iter()
        .map(|l| l.replace("*/", "*&#47;").replace("/*", "&#47;*"))
        .collect();
    if lines.len() == 1 {
        w.line(format!("/** {} */", lines[0]));
    } else {
        w.line("/**");
        for l in &lines {
            if l.is_empty() {
                w.line(" *");
            } else {
                w.line(format!(" * {l}"));
            }
        }
        w.line(" */");
    }
}

fn ident(name: &str) -> String {
    naming::kotlin_ident(&naming::camel(name))
}

struct KtGen<'a> {
    model: &'a Model,
    cfg: &'a Generator,
}

struct Ctx<'a> {
    g: &'a KtGen<'a>,
    imports: BTreeSet<String>,
    hoisted: Vec<(String, String)>,
    hoist_names: HashMap<TypeRef, String>,
    body: CodeWriter,
}

type Shadow = HashSet<String>;

impl<'a> Ctx<'a> {
    fn new(g: &'a KtGen<'a>) -> Self {
        Ctx {
            g,
            imports: BTreeSet::new(),
            hoisted: Vec::new(),
            hoist_names: HashMap::new(),
            body: CodeWriter::new("    "),
        }
    }

    fn model(&self) -> &'a Model {
        self.g.model
    }

    fn package(&self) -> &'a str {
        &self.g.cfg.kotlin_package
    }

    fn import(&mut self, fqn: &str) {
        self.imports.insert(fqn.to_owned());
    }

    /// The spelling of a standard-library type: simple, or fully qualified
    /// when `shadow` hides the simple name.
    fn builtin(&mut self, simple: &str, shadow: &Shadow) -> String {
        if shadow.contains(simple) {
            return QUALIFIED
                .iter()
                .find(|(s, _)| *s == simple)
                .map_or_else(|| simple.to_owned(), |(_, q)| (*q).to_owned());
        }
        if let Some((_, q)) = QUALIFIED.iter().find(|(s, _)| *s == simple) {
            if !q.starts_with("kotlin.") || q.starts_with("kotlin.time.") {
                self.import(q);
            }
        }
        simple.to_owned()
    }

    fn named(&self, name: &str, shadow: &Shadow) -> String {
        if shadow.contains(name) {
            format!("{}.{name}", self.package())
        } else {
            name.to_owned()
        }
    }

    // ----- types ------------------------------------------------------------

    fn ty(&mut self, t: &TypeRef, sh: &Shadow) -> String {
        match t {
            TypeRef::Bool => self.builtin("Boolean", sh),
            TypeRef::I8 => self.builtin("Byte", sh),
            TypeRef::I16 => self.builtin("Short", sh),
            TypeRef::I32 => self.builtin("Int", sh),
            TypeRef::I64 => self.builtin("Long", sh),
            TypeRef::U8 => self.builtin("UByte", sh),
            TypeRef::U16 => self.builtin("UShort", sh),
            TypeRef::U32 => self.builtin("UInt", sh),
            TypeRef::U64 => self.builtin("ULong", sh),
            TypeRef::F32 => self.builtin("Float", sh),
            TypeRef::F64 => self.builtin("Double", sh),
            TypeRef::String => self.builtin("String", sh),
            TypeRef::Bytes => self.builtin("ByteArray", sh),
            TypeRef::Unit => self.builtin("Unit", sh),
            TypeRef::Duration => self.builtin("Duration", sh),
            TypeRef::Timestamp => self.builtin("Timestamp", sh),
            TypeRef::Uuid => self.builtin("UUID", sh),
            TypeRef::Option(inner) => format!("{}?", self.ty(inner, sh)),
            TypeRef::Vec(inner) => {
                let list = self.builtin("List", sh);
                format!("{list}<{}>", self.ty(inner, sh))
            }
            TypeRef::Map(k, v) => {
                let map = self.builtin("Map", sh);
                format!("{map}<{}, {}>", self.ty(k, sh), self.ty(v, sh))
            }
            TypeRef::Named(name) => self.named(name, sh),
            // Rejected by validation before generation starts.
            TypeRef::Lazy(_) | TypeRef::Result(..) | TypeRef::Stream(_) => "Nothing".to_owned(),
        }
    }

    // ----- codecs -----------------------------------------------------------

    fn prim(&mut self, name: &str) -> String {
        self.import("dev.keel.runtime.wire.Codecs");
        format!("Codecs.{name}")
    }

    /// A `KeelCodec<T>` expression for `t`.
    fn codec(&mut self, t: &TypeRef, sh: &Shadow) -> String {
        match t {
            TypeRef::Bool => self.prim("bool"),
            TypeRef::I8 => self.prim("i8"),
            TypeRef::I16 => self.prim("i16"),
            TypeRef::I32 => self.prim("i32"),
            TypeRef::I64 => self.prim("i64"),
            TypeRef::U8 => self.prim("u8"),
            TypeRef::U16 => self.prim("u16"),
            TypeRef::U32 => self.prim("u32"),
            TypeRef::U64 => self.prim("u64"),
            TypeRef::F32 => self.prim("f32"),
            TypeRef::F64 => self.prim("f64"),
            TypeRef::String => self.prim("string"),
            TypeRef::Bytes => self.prim("bytes"),
            TypeRef::Unit => self.prim("unit"),
            TypeRef::Duration => self.prim("duration"),
            TypeRef::Timestamp => self.prim("timestamp"),
            TypeRef::Uuid => self.prim("uuid"),
            TypeRef::Named(name) => self.named(name, sh),
            TypeRef::Option(_) | TypeRef::Vec(_) | TypeRef::Map(..) => {
                if let Some(name) = self.hoist_names.get(t) {
                    return name.clone();
                }
                // Hoisted properties live at file level, where nothing is
                // shadowed.
                let none = Shadow::new();
                let expr = match t {
                    TypeRef::Option(i) => {
                        self.import("dev.keel.runtime.wire.Codecs");
                        format!("Codecs.option({})", self.codec(i, &none))
                    }
                    TypeRef::Vec(i) => {
                        self.import("dev.keel.runtime.wire.Codecs");
                        format!("Codecs.vec({})", self.codec(i, &none))
                    }
                    TypeRef::Map(k, v) => {
                        self.import("dev.keel.runtime.wire.Codecs");
                        let k = self.codec(k, &none);
                        let v = self.codec(v, &none);
                        format!("Codecs.map({k}, {v})")
                    }
                    _ => String::new(),
                };
                let name = self.hoist_name(t);
                self.hoisted.push((name.clone(), expr));
                self.hoist_names.insert(t.clone(), name.clone());
                name
            }
            TypeRef::Lazy(_) | TypeRef::Result(..) | TypeRef::Stream(_) => "Codecs.unit".to_owned(),
        }
    }

    fn hoist_name(&self, t: &TypeRef) -> String {
        fn words(t: &TypeRef, out: &mut String) {
            match t {
                TypeRef::Option(i) => {
                    out.push_str("Option");
                    words(i, out);
                }
                TypeRef::Vec(i) => {
                    out.push_str("Vec");
                    words(i, out);
                }
                TypeRef::Map(k, v) => {
                    out.push_str("Map");
                    words(k, out);
                    words(v, out);
                }
                TypeRef::Named(n) => out.push_str(n),
                other => out.push_str(&naming::pascal(&other.to_string())),
            }
        }
        let mut base = String::from("codec");
        words(t, &mut base);
        let mut name = base.clone();
        let mut n = 2;
        while self.hoisted.iter().any(|(existing, _)| *existing == name) {
            name = format!("{base}{n}");
            n += 1;
        }
        name
    }

    // ----- statements ---------------------------------------------------------

    fn write_stmt(&mut self, t: &TypeRef, value: &str, w: &str, sh: &Shadow) -> String {
        let direct = match t {
            TypeRef::Bool => Some("writeBool"),
            TypeRef::I8 => Some("writeI8"),
            TypeRef::I16 => Some("writeI16"),
            TypeRef::I32 => Some("writeI32"),
            TypeRef::I64 => Some("writeI64"),
            TypeRef::U8 => Some("writeU8"),
            TypeRef::U16 => Some("writeU16"),
            TypeRef::U32 => Some("writeU32"),
            TypeRef::U64 => Some("writeU64"),
            TypeRef::F32 => Some("writeF32"),
            TypeRef::F64 => Some("writeF64"),
            TypeRef::String => Some("writeStr"),
            TypeRef::Bytes => Some("writeBytes"),
            _ => None,
        };
        match direct {
            Some(method) => format!("{w}.{method}({value})"),
            None => {
                let codec = self.codec(t, sh);
                format!("{codec}.encode({w}, {value})")
            }
        }
    }

    fn read_expr(&mut self, t: &TypeRef, r: &str, sh: &Shadow) -> String {
        let direct = match t {
            TypeRef::Bool => Some("readBool"),
            TypeRef::I8 => Some("readI8"),
            TypeRef::I16 => Some("readI16"),
            TypeRef::I32 => Some("readI32"),
            TypeRef::I64 => Some("readI64"),
            TypeRef::U8 => Some("readU8"),
            TypeRef::U16 => Some("readU16"),
            TypeRef::U32 => Some("readU32"),
            TypeRef::U64 => Some("readU64"),
            TypeRef::F32 => Some("readF32"),
            TypeRef::F64 => Some("readF64"),
            TypeRef::String => Some("readStr"),
            TypeRef::Bytes => Some("readBytes"),
            _ => None,
        };
        match direct {
            Some(method) => format!("{r}.{method}()"),
            None => {
                let codec = self.codec(t, sh);
                format!("{codec}.decode({r})")
            }
        }
    }

    /// An expression decoding the whole of `bytes` as a `t`.
    fn decode_all(&mut self, t: &TypeRef, bytes: &str) -> String {
        let codec = self.codec(t, &Shadow::new());
        self.import("dev.keel.runtime.wire.decodeAll");
        format!("{codec}.decodeAll({bytes})")
    }

    /// The zero value used as a signal's placeholder until the initial
    /// change-set arrives.
    fn zero(&mut self, t: &TypeRef, depth: usize) -> String {
        match t {
            TypeRef::Bool => "false".to_owned(),
            TypeRef::I8 => "0.toByte()".to_owned(),
            TypeRef::I16 => "0.toShort()".to_owned(),
            TypeRef::I32 => "0".to_owned(),
            TypeRef::I64 => "0L".to_owned(),
            TypeRef::U8 => "0u.toUByte()".to_owned(),
            TypeRef::U16 => "0u.toUShort()".to_owned(),
            TypeRef::U32 => "0u".to_owned(),
            TypeRef::U64 => "0uL".to_owned(),
            TypeRef::F32 => "0f".to_owned(),
            TypeRef::F64 => "0.0".to_owned(),
            TypeRef::String => "\"\"".to_owned(),
            TypeRef::Bytes => "ByteArray(0)".to_owned(),
            TypeRef::Duration => {
                self.import("kotlin.time.Duration");
                "Duration.ZERO".to_owned()
            }
            TypeRef::Timestamp => {
                self.import("dev.keel.runtime.wire.Timestamp");
                "Timestamp(0L)".to_owned()
            }
            TypeRef::Uuid => {
                self.import("java.util.UUID");
                "UUID(0L, 0L)".to_owned()
            }
            TypeRef::Option(_) => "null".to_owned(),
            TypeRef::Vec(_) => "emptyList()".to_owned(),
            TypeRef::Map(..) => "emptyMap()".to_owned(),
            TypeRef::Named(name) => self.zero_named(name, depth),
            TypeRef::Unit | TypeRef::Lazy(_) | TypeRef::Result(..) | TypeRef::Stream(_) => {
                "Unit".to_owned()
            }
        }
    }

    fn zero_named(&mut self, name: &str, depth: usize) -> String {
        let model = self.model();
        if depth > 8 {
            return "error(\"recursive default\")".to_owned();
        }
        match model.kind(name) {
            Some(NamedKind::Record) => {
                let Some(record) = model.record(name) else {
                    return String::new();
                };
                let args: Vec<String> = record
                    .fields
                    .iter()
                    .map(|f| format!("{} = {}", ident(&f.name), self.zero(&f.ty, depth + 1)))
                    .collect();
                format!("{name}({})", args.join(", "))
            }
            Some(NamedKind::UnitEnum) => model
                .enum_def(name)
                .and_then(|e| e.variants.first())
                .map(|v| format!("{name}.{}", naming::upper_snake(&v.name)))
                .unwrap_or_default(),
            Some(NamedKind::DataEnum | NamedKind::Error) => {
                let en = model.enum_def(name).or_else(|| model.error_def(name));
                let Some(variant) = en.and_then(|e| e.variants.first()) else {
                    return String::new();
                };
                if variant.fields.is_empty() {
                    format!("{name}.{}", variant.name)
                } else {
                    let names = self.variant_props(en, variant);
                    let args: Vec<String> = names
                        .iter()
                        .zip(&variant.fields)
                        .map(|(n, f)| format!("{n} = {}", self.zero(&f.ty, depth + 1)))
                        .collect();
                    format!("{name}.{}({})", variant.name, args.join(", "))
                }
            }
            Some(NamedKind::Object) | None => String::new(),
        }
    }

    // ----- variant fields -----------------------------------------------------

    fn is_cause_variant(&self, v: &VariantDef) -> bool {
        v.tuple
            && v.fields.len() == 1
            && matches!(&v.fields[0].ty, TypeRef::Named(n) if self.model().kind(n) == Some(NamedKind::Error))
    }

    /// The property names of a variant's fields (escaped for Kotlin).
    fn variant_props(&self, en: Option<&EnumDef>, v: &VariantDef) -> Vec<String> {
        let is_error = en.is_some_and(|e| e.is_error);
        if is_error && self.is_cause_variant(v) {
            return vec!["cause".to_owned()];
        }
        let count = v.fields.len();
        v.fields
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let base = if v.tuple {
                    naming::tuple_field(i, count)
                } else {
                    naming::camel(&f.name)
                };
                let base = if is_error {
                    naming::avoid(&base, THROWABLE_MEMBERS)
                } else {
                    base
                };
                naming::kotlin_ident(&base)
            })
            .collect()
    }

    /// The names of the variant classes of `en`, which shadow types inside the
    /// sealed hierarchy.
    fn shadow_of(en: &EnumDef) -> Shadow {
        en.variants.iter().map(|v| v.name.clone()).collect()
    }
}

impl KtGen<'_> {
    fn header(&self, imports: &BTreeSet<String>) -> String {
        let mut out = CodeWriter::new("    ");
        out.line(format!(
            "// Generated by keel-bindgen from the Keel schema of `{}` (schema hash 0x{:016x}). Do not edit.",
            self.model.crate_name, self.model.schema_hash
        ));
        out.blank();
        out.line(format!("package {}", self.cfg.kotlin_package));
        out.blank();
        for i in imports {
            out.line(format!("import {i}"));
        }
        out.finish()
    }

    /// Finishes a file: header, imports, body and the hoisted codecs.
    fn assemble(&self, mut cx: Ctx<'_>) -> String {
        if !cx.hoisted.is_empty() {
            let w = &mut cx.body;
            w.blank();
            for (name, expr) in &cx.hoisted {
                w.line(format!("private val {name} = {expr}"));
            }
        }
        let body = cx.body.finish();
        let mut out = CodeWriter::new("    ");
        out.line(self.header(&cx.imports));
        out.blank();
        out.line(body);
        out.finish()
    }

    fn types_file(&self) -> String {
        let mut cx = Ctx::new(self);
        let mut w = CodeWriter::new("    ");
        for record in &self.model.records {
            cx.record(&mut w, record);
            w.blank();
        }
        for en in &self.model.enums {
            if is_unit_enum(en) {
                cx.unit_enum(&mut w, en);
            } else {
                cx.data_enum(&mut w, en);
            }
            w.blank();
        }
        cx.body = w;
        self.assemble(cx)
    }

    fn errors_file(&self) -> String {
        let mut cx = Ctx::new(self);
        let mut w = CodeWriter::new("    ");
        for en in &self.model.errors {
            cx.error(&mut w, en);
            w.blank();
        }
        cx.body = w;
        self.assemble(cx)
    }

    fn objects_file(&self) -> String {
        let mut cx = Ctx::new(self);
        let mut w = CodeWriter::new("    ");
        for object in &self.model.objects {
            cx.object(&mut w, object);
            w.blank();
        }
        for function in &self.model.functions {
            cx.function(&mut w, function, "KeelIds.Functions");
            w.blank();
        }
        cx.body = w;
        self.assemble(cx)
    }

    fn stores_file(&self) -> String {
        let mut cx = Ctx::new(self);
        let mut w = CodeWriter::new("    ");
        for store in &self.model.stores {
            cx.object(&mut w, store);
            w.blank();
        }
        cx.body = w;
        self.assemble(cx)
    }

    fn ports_file(&self) -> String {
        let mut cx = Ctx::new(self);
        let mut w = CodeWriter::new("    ");
        for port in &self.model.ports {
            if port.kind == PortKind::Event {
                cx.event_port(&mut w, port);
            } else {
                cx.port(&mut w, port);
            }
            w.blank();
        }
        cx.body = w;
        self.assemble(cx)
    }

    fn queries_file(&self) -> String {
        let mut cx = Ctx::new(self);
        let mut w = CodeWriter::new("    ");
        for handle in &self.model.query_handles {
            cx.object(&mut w, handle);
            w.blank();
        }
        for mutation in &self.model.mutations {
            cx.function(&mut w, mutation, "KeelIds.Queries");
            w.blank();
        }
        cx.body = w;
        self.assemble(cx)
    }

    fn ids_file(&self) -> String {
        let m = self.model;
        let mut w = CodeWriter::new("    ");
        w.line("/**");
        w.line(" * Stable wire identifiers (SPEC section 1.1), for logs and debugging, plus the schema");
        w.line(" * hash to pass to `KeelCore.load` as `expectedSchemaHash`.");
        w.line(" */");
        w.block("object KeelIds", |w| {
            w.line(format!(
                "const val SCHEMA_HASH: ULong = 0x{:016x}uL",
                m.schema_hash
            ));
            w.blank();
            w.block("object Objects", |w| {
                for o in m.all_objects() {
                    w.block(format!("object {}", o.name), |w| {
                        w.line(format!("const val TYPE_ID: UInt = {}", hex(o.type_id)));
                        for method in o.constructors.iter().chain(&o.methods) {
                            w.line(format!(
                                "const val {}: UInt = {}",
                                naming::upper_snake(&method.name),
                                hex(method.method_id)
                            ));
                        }
                    });
                }
            });
            w.blank();
            w.block("object Functions", |w| {
                for f in &m.functions {
                    w.line(format!(
                        "const val {}: UInt = {}",
                        naming::upper_snake(&f.name),
                        hex(f.method_id)
                    ));
                }
            });
            w.blank();
            w.block("object Ports", |w| {
                for p in &m.ports {
                    w.block(format!("object {}", p.name), |w| {
                        w.line(format!("const val PORT_ID: UInt = {}", hex(p.port_id)));
                        for method in &p.methods {
                            w.line(format!(
                                "const val {}: UInt = {}",
                                naming::upper_snake(&method.name),
                                hex(method.method_id)
                            ));
                        }
                    });
                }
            });
            w.blank();
            w.block("object Queries", |w| {
                for q in &m.queries {
                    w.line(format!(
                        "const val {}: UInt = {}",
                        naming::upper_snake(&q.name),
                        hex(q.query_id)
                    ));
                }
            });
        });
        let mut out = CodeWriter::new("    ");
        out.line(self.header(&BTreeSet::new()));
        out.blank();
        out.line(w.finish());
        out.finish()
    }
}

// ===== declarations ============================================================

impl<'a> Ctx<'a> {
    /// `data class` properties with their KDoc, as constructor parameters.
    fn params_of_fields(
        &mut self,
        fields: &[FieldDef],
        names: &[String],
        sh: &Shadow,
        with_defaults: bool,
        overrides: &[bool],
    ) -> Vec<String> {
        fields
            .iter()
            .zip(names)
            .enumerate()
            .map(|(i, (f, name))| {
                let ty = self.ty(&f.ty, sh);
                let default = if with_defaults && f.default {
                    self.default_value(&f.ty)
                        .map(|d| format!(" = {d}"))
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                let over = if overrides.get(i).copied().unwrap_or(false) {
                    "override "
                } else {
                    ""
                };
                format!("{over}val {name}: {ty}{default}")
            })
            .collect()
    }

    /// The value of a `#[keel(default)]` field, for the types whose default is
    /// unambiguous.
    fn default_value(&mut self, t: &TypeRef) -> Option<String> {
        match t {
            TypeRef::Bool
            | TypeRef::I8
            | TypeRef::I16
            | TypeRef::I32
            | TypeRef::I64
            | TypeRef::U8
            | TypeRef::U16
            | TypeRef::U32
            | TypeRef::U64
            | TypeRef::F32
            | TypeRef::F64
            | TypeRef::String
            | TypeRef::Bytes
            | TypeRef::Duration
            | TypeRef::Timestamp
            | TypeRef::Uuid
            | TypeRef::Option(_)
            | TypeRef::Vec(_)
            | TypeRef::Map(..) => Some(self.zero(t, 0)),
            _ => None,
        }
    }

    /// Whether `ty` is a `ByteArray` or a nullable one, which a data class
    /// compares by identity unless `equals` is overridden.
    fn is_bytes_like(t: &TypeRef) -> bool {
        matches!(t, TypeRef::Bytes)
            || matches!(t, TypeRef::Option(inner) if matches!(**inner, TypeRef::Bytes))
    }

    /// `equals` and `hashCode` overrides that compare byte arrays by content.
    fn bytes_equality(w: &mut CodeWriter, class: &str, names: &[String], fields: &[FieldDef]) {
        if !fields.iter().any(|f| Self::is_bytes_like(&f.ty)) {
            return;
        }
        w.blank();
        w.block("override fun equals(other: Any?): Boolean", |w| {
            w.line("if (this === other) return true");
            w.line(format!("if (other !is {class}) return false"));
            let terms: Vec<String> = names
                .iter()
                .zip(fields)
                .map(|(n, f)| {
                    if Self::is_bytes_like(&f.ty) {
                        format!("{n}.contentEquals(other.{n})")
                    } else {
                        format!("{n} == other.{n}")
                    }
                })
                .collect();
            w.line(format!("return {}", terms.join(" && ")));
        });
        w.blank();
        w.block("override fun hashCode(): Int", |w| {
            for (i, (n, f)) in names.iter().zip(fields).enumerate() {
                let hash = if Self::is_bytes_like(&f.ty) {
                    format!("{n}.contentHashCode()")
                } else {
                    format!("{n}.hashCode()")
                };
                if i == 0 {
                    w.line(format!("var result = {hash}"));
                } else {
                    w.line(format!("result = 31 * result + {hash}"));
                }
            }
            w.line("return result");
        });
    }

    fn record(&mut self, w: &mut CodeWriter, r: &RecordDef) {
        self.import("dev.keel.runtime.KeelRecord");
        self.import("dev.keel.runtime.wire.KeelCodec");
        self.import("dev.keel.runtime.wire.KeelReader");
        self.import("dev.keel.runtime.wire.KeelWriter");
        let none = Shadow::new();
        kdoc(w, &r.docs, &[]);
        let names: Vec<String> = r.fields.iter().map(|f| ident(&f.name)).collect();
        if r.fields.is_empty() {
            w.line(format!("class {} : KeelRecord {{", r.name));
            w.indented(|w| {
                w.line(format!(
                    "override fun equals(other: Any?): Boolean = other is {}",
                    r.name
                ));
                w.blank();
                w.line("override fun hashCode(): Int = 0");
                w.blank();
                w.line(format!(
                    "override fun toString(): String = \"{}()\"",
                    r.name
                ));
                w.blank();
                self.record_codec(w, r, &names, &none);
            });
            w.line("}");
            return;
        }
        w.line(format!("data class {}(", r.name));
        w.indented(|w| {
            let params = self.params_of_fields(&r.fields, &names, &none, true, &[]);
            for (f, p) in r.fields.iter().zip(&params) {
                kdoc(w, &f.docs, &[]);
                w.line(format!("{p},"));
            }
        });
        w.line(") : KeelRecord {");
        w.indented(|w| {
            Self::bytes_equality(w, &r.name, &names, &r.fields);
            if r.fields.iter().any(|f| Self::is_bytes_like(&f.ty)) {
                w.blank();
            }
            self.record_codec(w, r, &names, &none);
        });
        w.line("}");
    }

    fn record_codec(&mut self, w: &mut CodeWriter, r: &RecordDef, names: &[String], sh: &Shadow) {
        w.block(format!("companion object : KeelCodec<{}>", r.name), |w| {
            w.block(
                format!("override fun encode(w: KeelWriter, v: {})", r.name),
                |w| {
                    for (f, n) in r.fields.iter().zip(names) {
                        w.line(self.write_stmt(&f.ty, &format!("v.{n}"), "w", sh));
                    }
                },
            );
            w.blank();
            if r.fields.is_empty() {
                w.line(format!(
                    "override fun decode(r: KeelReader): {} = {}()",
                    r.name, r.name
                ));
            } else {
                let args: Vec<String> = r
                    .fields
                    .iter()
                    .zip(names)
                    .map(|(f, n)| format!("{n} = {}", self.read_expr(&f.ty, "r", sh)))
                    .collect();
                w.call(
                    format!(
                        "override fun decode(r: KeelReader): {} = {}",
                        r.name, r.name
                    ),
                    &args,
                    "",
                    true,
                );
            }
        });
    }

    fn unit_enum(&mut self, w: &mut CodeWriter, en: &EnumDef) {
        self.import("dev.keel.runtime.KeelEnum");
        self.import("dev.keel.runtime.wire.KeelCodec");
        self.import("dev.keel.runtime.wire.KeelReader");
        self.import("dev.keel.runtime.wire.KeelWriter");
        self.import("dev.keel.runtime.wire.WireException");
        kdoc(w, &en.docs, &[]);
        w.block(
            format!("enum class {}(val index: UShort) : KeelEnum", en.name),
            |w| {
                let last = en.variants.len().saturating_sub(1);
                for (i, v) in en.variants.iter().enumerate() {
                    kdoc(w, &v.docs, &[]);
                    let end = if i == last { ";" } else { "," };
                    w.line(format!(
                        "{}({}u){end}",
                        naming::upper_snake(&v.name),
                        v.index
                    ));
                }
                w.blank();
                w.block(format!("companion object : KeelCodec<{}>", en.name), |w| {
                    w.line(format!(
                        "override fun encode(w: KeelWriter, v: {}) = w.writeU16(v.index)",
                        en.name
                    ));
                    w.blank();
                    w.block(
                        format!("override fun decode(r: KeelReader): {}", en.name),
                        |w| {
                            w.line("val at = r.position");
                            w.block("return when (val tag = r.readU16().toInt())", |w| {
                                for v in &en.variants {
                                    w.line(format!(
                                        "{} -> {}",
                                        v.index,
                                        naming::upper_snake(&v.name)
                                    ));
                                }
                                w.line(format!(
                                    "else -> throw WireException.InvalidTag(tag.toUInt(), at, {})",
                                    kt_string(&en.name)
                                ));
                            });
                        },
                    );
                });
            },
        );
    }

    fn data_enum(&mut self, w: &mut CodeWriter, en: &EnumDef) {
        self.import("dev.keel.runtime.KeelEnum");
        self.import("dev.keel.runtime.wire.KeelCodec");
        self.import("dev.keel.runtime.wire.KeelReader");
        self.import("dev.keel.runtime.wire.KeelWriter");
        self.import("dev.keel.runtime.wire.WireException");
        let shadow = Self::shadow_of(en);
        kdoc(w, &en.docs, &[]);
        w.block(format!("sealed interface {} : KeelEnum", en.name), |w| {
            for v in &en.variants {
                self.variant_class(w, en, v, &shadow, &en.name, None);
            }
            w.blank();
            self.sealed_codec(w, en, &shadow);
        });
    }

    /// One nested variant class (`data class` / `data object`) extending
    /// `parent`; `super_call` is the superclass constructor call for sealed
    /// classes.
    fn variant_class(
        &mut self,
        w: &mut CodeWriter,
        en: &EnumDef,
        v: &VariantDef,
        shadow: &Shadow,
        parent: &str,
        super_call: Option<String>,
    ) {
        let supertype = super_call.unwrap_or_else(|| parent.to_owned());
        kdoc(w, &v.docs, &[]);
        if v.fields.is_empty() {
            w.line(format!("data object {} : {supertype}", v.name));
            return;
        }
        let names = self.variant_props(Some(en), v);
        let overrides: Vec<bool> = names.iter().map(|n| n == "cause" && en.is_error).collect();
        let params = self.params_of_fields(&v.fields, &names, shadow, false, &overrides);
        let needs_body = v.fields.iter().any(|f| Self::is_bytes_like(&f.ty));
        let head = format!("data class {}(", v.name);
        if params.iter().map(String::len).sum::<usize>() + head.len() < 80 {
            let line = format!("{head}{}) : {supertype}", params.join(", "));
            if needs_body {
                w.line(format!("{line} {{"));
                w.indented(|w| Self::bytes_equality(w, &v.name, &names, &v.fields));
                w.line("}");
            } else {
                w.line(line);
            }
        } else {
            w.line(head);
            w.indented(|w| {
                for p in &params {
                    w.line(format!("{p},"));
                }
            });
            if needs_body {
                w.line(format!(") : {supertype} {{"));
                w.indented(|w| Self::bytes_equality(w, &v.name, &names, &v.fields));
                w.line("}");
            } else {
                w.line(format!(") : {supertype}"));
            }
        }
    }

    /// The companion codec of a sealed type (data enum or error). Errors also
    /// get `fromReply`.
    fn sealed_codec(&mut self, w: &mut CodeWriter, en: &EnumDef, shadow: &Shadow) {
        w.block(format!("companion object : KeelCodec<{}>", en.name), |w| {
            w.block(
                format!("override fun encode(w: KeelWriter, v: {})", en.name),
                |w| {
                    w.block("when (v)", |w| {
                        for v in &en.variants {
                            let names = self.variant_props(Some(en), v);
                            if v.fields.is_empty() {
                                w.line(format!("{} -> w.writeU16({}u)", v.name, v.index));
                            } else {
                                w.line(format!("is {} -> {{", v.name));
                                w.indented(|w| {
                                    w.line(format!("w.writeU16({}u)", v.index));
                                    for (f, n) in v.fields.iter().zip(&names) {
                                        w.line(self.write_stmt(
                                            &f.ty,
                                            &format!("v.{n}"),
                                            "w",
                                            shadow,
                                        ));
                                    }
                                });
                                w.line("}");
                            }
                        }
                    });
                },
            );
            w.blank();
            w.block(format!("override fun decode(r: KeelReader): {}", en.name), |w| {
                w.line("val at = r.position");
                w.block("return when (val tag = r.readU16().toInt())", |w| {
                    for v in &en.variants {
                        if v.fields.is_empty() {
                            w.line(format!("{} -> {}", v.index, v.name));
                        } else {
                            let names = self.variant_props(Some(en), v);
                            let args: Vec<String> = v
                                .fields
                                .iter()
                                .zip(&names)
                                .map(|(f, n)| {
                                    format!("{n} = {}", self.read_expr(&f.ty, "r", shadow))
                                })
                                .collect();
                            w.call(format!("{} -> {}", v.index, v.name), &args, "", true);
                        }
                    }
                    w.line(format!(
                        "else -> throw WireException.InvalidTag(tag.toUInt(), at, {})",
                        kt_string(&en.name)
                    ));
                });
            });
            if en.is_error {
                w.blank();
                kdoc(
                    w,
                    "The typed error a failed call carries; any other failure is returned unchanged.",
                    &[],
                );
                w.block("fun fromReply(error: KeelReplyException): Throwable", |w| {
                    w.line("return if (error.status == ReplyStatus.ERROR) decodeAll(error.body) else error");
                });
            }
        });
    }

    // ----- errors ---------------------------------------------------------------

    /// The message expression passed to the superclass constructor.
    fn error_message(&mut self, v: &VariantDef, props: &[String]) -> String {
        let Some(template) = &v.message else {
            return match (props.first(), v.fields.first()) {
                (Some(prop), Some(f)) if self.is_error_type(&f.ty) => {
                    format!("{prop}.message.orEmpty()")
                }
                (Some(prop), _) => format!("{prop}.toString()"),
                _ => "\"\"".to_owned(),
            };
        };
        let parts = parse_message(v, template).unwrap_or_default();
        let mut out = String::from("\"");
        for part in &parts {
            match part {
                MsgPart::Text(t) => out.push_str(&kt_template_text(t)),
                MsgPart::Field(i) => {
                    let prop = &props[*i];
                    if self.is_error_type(&v.fields[*i].ty) {
                        out.push_str(&format!("${{{prop}.message}}"));
                    } else {
                        out.push_str(&format!("${{{prop}}}"));
                    }
                }
            }
        }
        out.push('"');
        out
    }

    fn is_error_type(&self, t: &TypeRef) -> bool {
        matches!(t, TypeRef::Named(n) if self.model().kind(n) == Some(NamedKind::Error))
    }

    fn error(&mut self, w: &mut CodeWriter, en: &EnumDef) {
        self.import("dev.keel.runtime.KeelException");
        self.import("dev.keel.runtime.KeelReplyException");
        self.import("dev.keel.runtime.wire.KeelCodec");
        self.import("dev.keel.runtime.wire.KeelReader");
        self.import("dev.keel.runtime.wire.KeelWriter");
        self.import("dev.keel.runtime.wire.Payloads.ReplyStatus");
        self.import("dev.keel.runtime.wire.WireException");
        self.import("dev.keel.runtime.wire.decodeAll");
        let shadow = Self::shadow_of(en);
        kdoc(w, &en.docs, &[]);
        w.block(
            format!(
                "sealed class {}(message: String) : KeelException(message)",
                en.name
            ),
            |w| {
                for v in &en.variants {
                    let props = self.variant_props(Some(en), v);
                    let message = self.error_message(v, &props);
                    self.variant_class(
                        w,
                        en,
                        v,
                        &shadow,
                        &en.name,
                        Some(format!("{}({message})", en.name)),
                    );
                }
                w.blank();
                self.sealed_codec(w, en, &shadow);
            },
        );
    }

    // ----- objects --------------------------------------------------------------

    fn param_list(&mut self, params: &[ParamDef]) -> Vec<String> {
        let none = Shadow::new();
        params
            .iter()
            .map(|p| {
                let ty = self.ty(&p.ty, &none);
                format!("{}: {ty}", ident(&p.name))
            })
            .collect()
    }

    /// Writes `val w = KeelWriter(); w.writeX(..)` and returns the expression
    /// holding the encoded arguments.
    fn encode_args(&mut self, w: &mut CodeWriter, params: &[ParamDef], writer: &str) -> String {
        if params.is_empty() {
            return "ByteArray(0)".to_owned();
        }
        self.import("dev.keel.runtime.wire.KeelWriter");
        w.line(format!("val {writer} = KeelWriter()"));
        let none = Shadow::new();
        for p in params {
            let stmt = self.write_stmt(&p.ty, &ident(&p.name), writer, &none);
            w.line(stmt);
        }
        format!("{writer}.toByteArray()")
    }

    fn object(&mut self, w: &mut CodeWriter, o: &ObjectDef) {
        let is_store = o.store.is_some();
        let base = if is_store { "KeelStore" } else { "KeelObject" };
        self.import("dev.keel.runtime.KeelCore");
        self.import(&format!("dev.keel.runtime.{base}"));
        let signals: Vec<&SignalDef> = o.store.iter().flat_map(|s| s.signals.iter()).collect();
        kdoc(w, &o.docs, &[]);
        w.block(
            format!(
                "class {} private constructor(core: KeelCore, handle: Long) : {base}(core, handle)",
                o.name
            ),
            |w| {
                for g in &signals {
                    self.signal_property(w, o, g);
                }
                let simple_new = o.constructors.iter().find(|c| {
                    c.name == "new"
                        && c.params.is_empty()
                        && !c.is_async
                        && Ret::classify(&c.returns).is_some_and(|r| r.error().is_none())
                });
                if is_store {
                    w.blank();
                    w.block("init", |w| {
                        w.line("core.observe(handle, UInt.MAX_VALUE, true)");
                    });
                }
                if let Some(c) = simple_new {
                    w.blank();
                    kdoc(w, &c.docs, &[]);
                    let ids = format!("KeelIds.Objects.{}", o.name);
                    w.call(
                        "constructor(ctx: KeelCore = KeelCore.shared) : this",
                        &[
                            "ctx".to_owned(),
                            format!("ctx.construct({ids}.TYPE_ID, {ids}.NEW, ByteArray(0))"),
                        ],
                        "",
                        true,
                    );
                }
                for m in &o.methods {
                    w.blank();
                    let ids = format!("KeelIds.Objects.{}", o.name);
                    self.callable(
                        w,
                        &Callable::from_method(m),
                        &Site::Method {
                            id: format!("{ids}.{}", naming::upper_snake(&m.name)),
                        },
                    );
                }
                if is_store {
                    w.blank();
                    self.store_apply(w, &signals);
                }
                w.blank();
                w.block("companion object", |w| {
                    for (i, c) in o.constructors.iter().enumerate() {
                        if i > 0 {
                            w.blank();
                        }
                        self.constructor(w, o, c);
                    }
                });
            },
        );
    }

    fn signal_property(&mut self, w: &mut CodeWriter, o: &ObjectDef, g: &SignalDef) {
        self.import("kotlinx.coroutines.flow.MutableStateFlow");
        self.import("kotlinx.coroutines.flow.StateFlow");
        self.import("kotlinx.coroutines.flow.asStateFlow");
        let none = Shadow::new();
        let ty = self.ty(&g.ty, &none);
        let zero = self.zero(&g.ty, 0);
        let name = ident(&g.name);
        let backing = format!("_{}", naming::camel(&g.name));
        w.line(format!(
            "private val {backing}: MutableStateFlow<{ty}> = signal({zero})"
        ));
        if let Some(doc) = self.model().signal_doc(o, g) {
            kdoc(w, doc, &[]);
        }
        w.line(format!(
            "val {name}: StateFlow<{ty}> = {backing}.asStateFlow()"
        ));
    }

    fn constructor(&mut self, w: &mut CodeWriter, o: &ObjectDef, c: &MethodDef) {
        let name = if c.name == "new" {
            "create".to_owned()
        } else {
            ident(&c.name)
        };
        let ret = Ret::classify(&c.returns);
        let err = ret.as_ref().and_then(Ret::error).map(str::to_owned);
        let taken: Vec<String> = c.params.iter().map(|p| ident(&p.name)).collect();
        let taken_refs: Vec<&str> = taken.iter().map(String::as_str).collect();
        let ctx = naming::avoid("ctx", &taken_refs);
        let writer = naming::avoid("w", &taken_refs);
        let handle = naming::avoid("handle", &taken_refs);
        let mut params = self.param_list(&c.params);
        params.push(format!("{ctx}: KeelCore = KeelCore.shared"));
        let mut extra = Vec::new();
        if let Some(err) = &err {
            extra.push(format!("@throws {err}"));
        }
        kdoc(w, &c.docs, &extra);
        let suspend = if c.is_async { "suspend " } else { "" };
        let prefix = format!("{suspend}fun {name}");
        let suffix = format!(": {}", o.name);
        let ids = format!("KeelIds.Objects.{}", o.name);
        let member = naming::upper_snake(&c.name);
        w.call_block(prefix, &params, suffix, true, |w| {
            let args = self.encode_args(w, &c.params, &writer);
            let call = if c.is_async {
                self.import("dev.keel.runtime.wire.Payloads.CallTarget");
                self.import("dev.keel.runtime.wire.Codecs");
                format!("{ctx}.call(CallTarget.Constructor({ids}.TYPE_ID, {ids}.{member}), {ids}.{member}, {args})")
            } else {
                format!("{ctx}.construct({ids}.TYPE_ID, {ids}.{member}, {args})")
            };
            let decode = |cx: &mut Ctx<'a>, expr: String| -> String {
                if c.is_async {
                    cx.import("dev.keel.runtime.wire.decodeAll");
                    format!("Codecs.handle.decodeAll({expr})")
                } else {
                    expr
                }
            };
            if let Some(err) = &err {
                self.import("dev.keel.runtime.KeelReplyException");
                w.line(format!("val {handle} = try {{"));
                w.indented(|w| w.line(decode(self, call.clone())));
                w.line("} catch (e: KeelReplyException) {");
                w.indented(|w| w.line(format!("throw {err}.fromReply(e)")));
                w.line("}");
            } else {
                w.line(format!("val {handle} = {}", decode(self, call)));
            }
            w.line(format!("return {}({ctx}, {handle})", o.name));
        });
    }

    fn function(&mut self, w: &mut CodeWriter, f: &FunctionDef, ids: &str) {
        let id = format!("{ids}.{}", naming::upper_snake(&f.name));
        self.callable(w, &Callable::from_function(f), &Site::Function { id });
    }

    /// One method or free function.
    fn callable(&mut self, w: &mut CodeWriter, c: &Callable<'_>, site: &Site) {
        let ret = Ret::classify(c.returns).unwrap_or(Ret::Plain(c.returns));
        let taken: Vec<String> = c.params.iter().map(|p| ident(&p.name)).collect();
        let taken_refs: Vec<&str> = taken.iter().map(String::as_str).collect();
        let writer = naming::avoid("w", &taken_refs);
        let body_var = naming::avoid("body", &taken_refs);
        let stream_var = naming::avoid("stream", &taken_refs);
        let (core, target, id, is_function) = match site {
            Site::Method { id } => {
                self.import("dev.keel.runtime.wire.Handle");
                (
                    "this.core".to_owned(),
                    format!("CallTarget.ObjectMethod(Handle(this.handle), {id})"),
                    id.clone(),
                    false,
                )
            }
            Site::Function { id } => (
                naming::avoid("ctx", &taken_refs),
                format!("CallTarget.FreeFunction({id})"),
                id.clone(),
                true,
            ),
        };
        self.import("dev.keel.runtime.wire.Payloads.CallTarget");
        let mut params = self.param_list(c.params);
        if is_function {
            self.import("dev.keel.runtime.KeelCore");
            params.push(format!("{core}: KeelCore = KeelCore.shared"));
        }
        let err = ret.error().map(str::to_owned);
        let mut extra = Vec::new();
        if let Some(err) = &err {
            extra.push(format!("@throws {err}"));
        }
        kdoc(w, c.docs, &extra);
        let name = ident(c.name);

        if let Ret::Stream(item) | Ret::ResultStream { item, .. } = &ret {
            self.import("kotlinx.coroutines.flow.Flow");
            self.import("kotlinx.coroutines.flow.map");
            let item_ty = self.ty(item, &Shadow::new());
            let prefix = format!("fun {name}");
            let suffix = format!(": Flow<{item_ty}>");
            w.call_block(prefix, &params, suffix, true, |w| {
                let args = self.encode_args(w, c.params, &writer);
                let decode = self.decode_all(item, "bytes");
                w.call(
                    format!("val {stream_var} = {core}.stream"),
                    &[target.clone(), id.clone(), args],
                    "",
                    true,
                );
                if let Some(err) = &err {
                    self.import("dev.keel.runtime.KeelReplyException");
                    self.import("kotlinx.coroutines.flow.catch");
                    w.line(format!("return {stream_var}"));
                    w.indented(|w| {
                        w.line(format!(".map {{ bytes -> {decode} }}"));
                        w.block_with(".catch { error ->", "}", |w| {
                            w.line(format!(
                                "throw if (error is KeelReplyException) {err}.fromReply(error) else error"
                            ));
                        });
                    });
                } else {
                    w.line(format!("return {stream_var}.map {{ bytes -> {decode} }}"));
                }
            });
            return;
        }

        let (ok_ty, is_unit) = match &ret {
            Ret::Plain(t) | Ret::Result { ok: t, .. } => {
                (self.ty(t, &Shadow::new()), matches!(t, TypeRef::Unit))
            }
            Ret::Stream(_) | Ret::ResultStream { .. } => ("Unit".to_owned(), true),
        };
        let suspend = if c.is_async { "suspend " } else { "" };
        let prefix = format!("{suspend}fun {name}");
        let suffix = if is_unit {
            String::new()
        } else {
            format!(": {ok_ty}")
        };
        w.call_block(prefix, &params, suffix, true, |w| {
            let args = self.encode_args(w, c.params, &writer);
            let method = if c.is_async { "call" } else { "callSync" };
            let call_args = [target.clone(), id.clone(), args];
            let call = format!("{core}.{method}");
            if let Some(err) = &err {
                self.import("dev.keel.runtime.KeelReplyException");
                let bind = if is_unit {
                    String::new()
                } else {
                    format!("val {body_var} = ")
                };
                w.line(format!("{bind}try {{"));
                w.indented(|w| w.call(call.clone(), &call_args, "", true));
                w.line("} catch (e: KeelReplyException) {");
                w.indented(|w| w.line(format!("throw {err}.fromReply(e)")));
                w.line("}");
            } else if is_unit {
                w.call(call, &call_args, "", true);
            } else {
                w.call(format!("val {body_var} = {call}"), &call_args, "", true);
            }
            if !is_unit {
                let ok = match &ret {
                    Ret::Plain(t) | Ret::Result { ok: t, .. } => Some(*t),
                    _ => None,
                };
                if let Some(ok) = ok {
                    let expr = self.decode_all(ok, &body_var);
                    w.line(format!("return {expr}"));
                }
            }
        });
    }

    // ----- stores ----------------------------------------------------------------

    fn store_apply(&mut self, w: &mut CodeWriter, signals: &[&SignalDef]) {
        self.import("dev.keel.runtime.wire.KeelReader");
        self.import("dev.keel.runtime.wire.Payloads.ChangeOp");
        let keyed = signals
            .iter()
            .any(|g| g.key.is_some() && matches!(g.ty, TypeRef::Vec(_)));
        w.block(
            "override fun apply(signalId: UInt, op: ChangeOp, reader: KeelReader)",
            |w| {
                w.block("when (signalId)", |w| {
                    for g in signals {
                        let backing = format!("_{}", naming::camel(&g.name));
                        w.line(format!("{}u -> {{", g.signal_id));
                        w.indented(|w| {
                            let codec = self.codec(&g.ty, &Shadow::new());
                            w.line("if (op == ChangeOp.FULL) {");
                            w.indented(|w| {
                                w.line(format!("{backing}.value = {codec}.decode(reader)"));
                                w.line("reader.finish()");
                            });
                            if let (Some(_), TypeRef::Vec(item)) = (&g.key, &g.ty) {
                                self.import("dev.keel.runtime.wire.KeyedPatch");
                                self.import("dev.keel.runtime.wire.WireException");
                                let item_codec = self.codec(item, &Shadow::new());
                                w.line("} else if (op == ChangeOp.PATCH) {");
                                w.indented(|w| {
                                    w.line(format!(
                                        "val ops = KeyedPatch.decodePatch(reader, {item_codec})"
                                    ));
                                    w.line("reader.finish()");
                                    w.line("try {");
                                    w.indented(|w| {
                                        w.line(format!(
                                            "{backing}.value = KeyedPatch.applyPatch({backing}.value, ops)"
                                        ));
                                    });
                                    w.line("} catch (e: WireException.PatchOutOfBounds) {");
                                    w.indented(|w| w.line(format!("resync({}u)", g.signal_id)));
                                    w.line("}");
                                });
                            }
                            w.line("}");
                        });
                        w.line("}");
                    }
                    w.line("else -> Unit");
                });
            },
        );
        if keyed {
            w.blank();
            kdoc(
                w,
                "Re-observes a signal whose mirror diverged from the core, to receive a full value.",
                &[],
            );
            w.block("private fun resync(signalId: UInt)", |w| {
                w.line("core.observe(handle, signalId, false)");
                w.line("core.observe(handle, signalId, true)");
            });
        }
    }

    // ----- ports -----------------------------------------------------------------

    fn port(&mut self, w: &mut CodeWriter, p: &PortDef) {
        let sync = p.kind == PortKind::Sync;
        self.import("dev.keel.runtime.KeelPort");
        kdoc(w, &p.docs, &[]);
        w.block(format!("interface {} : KeelPort", p.name), |w| {
            for m in &p.methods {
                let ret = Ret::classify(&m.returns).unwrap_or(Ret::Plain(&m.returns));
                let ok = match &ret {
                    Ret::Plain(t) | Ret::Result { ok: t, .. } => Some(self.ty(t, &Shadow::new())),
                    _ => None,
                };
                let mut extra = Vec::new();
                if let Some(err) = ret.error() {
                    extra.push(format!("@throws {err}"));
                }
                kdoc(w, &m.docs, &extra);
                let params = self.param_list(&m.params);
                let suspend = if m.is_async { "suspend " } else { "" };
                let suffix = match (&ret, ok) {
                    (
                        Ret::Plain(TypeRef::Unit)
                        | Ret::Result {
                            ok: TypeRef::Unit, ..
                        },
                        _,
                    ) => String::new(),
                    (_, Some(ok)) => format!(": {ok}"),
                    _ => String::new(),
                };
                w.call(
                    format!("{suspend}fun {}", ident(&m.name)),
                    &params,
                    suffix,
                    true,
                );
            }
        });
        w.blank();
        self.import("dev.keel.runtime.PortImpl");
        kdoc(
            w,
            &format!(
                "Adapts an implementation of `{0}` to `KeelCore.registerPort(KeelIds.Ports.{0}.PORT_ID, ..)`.",
                p.name
            ),
            &[],
        );
        let fn_name = format!("{}PortImpl", naming::camel(&p.name));
        w.block(format!("fun {fn_name}(impl: {}): PortImpl", p.name), |w| {
            w.line("val methods = mutableMapOf<UInt, suspend (ByteArray) -> ByteArray>()");
            for m in &p.methods {
                self.port_method(w, p, m);
            }
            w.line(format!("return PortImpl(sync = {sync}, methods = methods)"));
        });
    }

    fn port_method(&mut self, w: &mut CodeWriter, p: &PortDef, m: &MethodDef) {
        let ret = Ret::classify(&m.returns).unwrap_or(Ret::Plain(&m.returns));
        let member = ident(&m.name);
        let key = format!("KeelIds.Ports.{}.{}", p.name, naming::upper_snake(&m.name));
        let taken: Vec<String> = m.params.iter().map(|a| ident(&a.name)).collect();
        let idents: Vec<String> = taken
            .iter()
            .map(|n| naming::avoid(n, &["r", "args", "impl", "e", "result"]))
            .collect();
        let none = Shadow::new();
        let args_param = if m.params.is_empty() { "" } else { " args ->" };
        w.line(format!("methods[{key}] = {{{args_param}"));
        w.indented(|w| {
            if !m.params.is_empty() {
                self.import("dev.keel.runtime.wire.KeelReader");
                w.line("val r = KeelReader(args)");
                for (a, id) in m.params.iter().zip(&idents) {
                    let expr = self.read_expr(&a.ty, "r", &none);
                    w.line(format!("val {id} = {expr}"));
                }
                w.line("r.finish()");
            }
            let call = format!("impl.{member}({})", idents.join(", "));
            let encode_result = |cx: &mut Ctx<'a>, w: &mut CodeWriter, ok: &TypeRef| {
                if matches!(ok, TypeRef::Unit) {
                    w.line(call.clone());
                    w.line("ByteArray(0)");
                } else {
                    cx.import("dev.keel.runtime.wire.encodeToByteArray");
                    let codec = cx.codec(ok, &Shadow::new());
                    w.line(format!("val result = {call}"));
                    w.line(format!("{codec}.encodeToByteArray(result)"));
                }
            };
            match &ret {
                Ret::Result { ok, err } => {
                    self.import("dev.keel.runtime.KeelPortException");
                    self.import("dev.keel.runtime.wire.encodeToByteArray");
                    w.line("try {");
                    w.indented(|w| encode_result(self, w, ok));
                    w.line(format!("}} catch (e: {err}) {{"));
                    w.indented(|w| {
                        w.line(format!(
                            "throw KeelPortException({err}.encodeToByteArray(e))"
                        ));
                    });
                    w.line("}");
                }
                Ret::Plain(t) => encode_result(self, w, t),
                Ret::Stream(_) | Ret::ResultStream { .. } => {}
            }
        });
        w.line("}");
    }

    fn event_port(&mut self, w: &mut CodeWriter, p: &PortDef) {
        self.import("dev.keel.runtime.KeelCore");
        kdoc(
            w,
            &p.docs,
            &["Sends the events of this port from the host to the core.".to_owned()],
        );
        w.block(
            format!(
                "class {}Events(private val core: KeelCore = KeelCore.shared)",
                p.name
            ),
            |w| {
                for (i, m) in p.methods.iter().enumerate() {
                    if i > 0 {
                        w.blank();
                    }
                    kdoc(w, &m.docs, &[]);
                    let params = self.param_list(&m.params);
                    let taken: Vec<String> = m.params.iter().map(|a| ident(&a.name)).collect();
                    let taken_refs: Vec<&str> = taken.iter().map(String::as_str).collect();
                    let writer = naming::avoid("w", &taken_refs);
                    w.call_block(format!("fun {}", ident(&m.name)), &params, "", true, |w| {
                        let args = self.encode_args(w, &m.params, &writer);
                        w.call(
                            "this.core.event",
                            &[
                                format!("KeelIds.Ports.{}.PORT_ID", p.name),
                                format!(
                                    "KeelIds.Ports.{}.{}",
                                    p.name,
                                    naming::upper_snake(&m.name)
                                ),
                                args,
                            ],
                            "",
                            true,
                        );
                    });
                }
            },
        );
    }
}

/// A method or free function, whichever the schema calls it.
struct Callable<'a> {
    name: &'a str,
    params: &'a [ParamDef],
    returns: &'a TypeRef,
    is_async: bool,
    docs: &'a str,
}

impl<'a> Callable<'a> {
    fn from_method(m: &'a MethodDef) -> Self {
        Callable {
            name: &m.name,
            params: &m.params,
            returns: &m.returns,
            is_async: m.is_async,
            docs: &m.docs,
        }
    }

    fn from_function(f: &'a FunctionDef) -> Self {
        Callable {
            name: &f.name,
            params: &f.params,
            returns: &f.returns,
            is_async: f.is_async,
            docs: &f.docs,
        }
    }
}

/// Where a call is made from.
enum Site {
    /// A method of the generated class; `id` is the Kotlin expression of its
    /// method id.
    Method { id: String },
    /// A top-level function.
    Function { id: String },
}
