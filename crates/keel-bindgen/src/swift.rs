//! The Swift generator (SPEC section 10.1).
//!
//! Output: `Sources/<Module>/Generated/{Types,Errors,Objects,Stores,Ports,Queries,Ids}.swift`.
//! Generated code depends only on `KeelRuntime` (SPEC section 17.3), Foundation
//! and Observation. Swift cannot be compiled where this crate is developed, so
//! the output sticks to plain constructs: no clever generics, every type
//! written out.
//!
//! Failure policy. A method that returns `Result<T, E>` throws `E`
//! (`throws(E)` unless [`Generator::swift_typed_throws`] is off). Any other
//! failure of the call (a core panic, a malformed reply, schema drift) cannot
//! be expressed as `E`; in typed mode it stops the process through
//! `keelUnexpected`, in untyped mode it is rethrown as it is. Synchronous
//! methods without a `Result` do not throw, as SPEC section 10.1 shows, so they
//! stop the same way. Asynchronous methods without a `Result` use plain
//! `throws` so that task cancellation and transport failures propagate.

use std::collections::HashSet;

use keel_meta::{
    EnumDef, FunctionDef, MethodDef, ObjectDef, ParamDef, PortDef, PortKind, RecordDef, SignalDef,
    TypeRef, VariantDef,
};

use crate::emit::CodeWriter;
use crate::model::{Model, MsgPart, NamedKind, Ret, doc_lines, is_unit_enum, parse_message};
use crate::naming;
use crate::{GeneratedFile, Generator};

const FILES: [&str; 7] = [
    "Types", "Errors", "Objects", "Stores", "Ports", "Queries", "Ids",
];

pub(crate) fn generate(model: &Model, cfg: &Generator) -> Vec<GeneratedFile> {
    let sw = SwiftGen { model, cfg };
    let dir = format!("Sources/{}/Generated", cfg.swift_module);
    let bodies = [
        sw.types_file(),
        sw.errors_file(),
        sw.objects_file(),
        sw.stores_file(),
        sw.ports_file(),
        sw.queries_file(),
        sw.ids_file(),
    ];
    FILES
        .iter()
        .zip(bodies)
        .map(|(name, contents)| GeneratedFile {
            path: format!("{dir}/{name}.swift"),
            contents,
        })
        .collect()
}

fn hex(id: u32) -> String {
    format!("0x{id:08x}")
}

fn swift_string(s: &str) -> String {
    let mut out = String::from("\"");
    out.push_str(&swift_template_text(s));
    out.push('"');
    out
}

/// Literal text inside a Swift string literal.
fn swift_template_text(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// `///` documentation lines; nothing for empty `docs` and no `extra` lines.
fn doc(w: &mut CodeWriter, docs: &str, extra: &[String]) {
    let mut lines = doc_lines(docs);
    lines.extend(extra.iter().cloned());
    for l in lines {
        if l.is_empty() {
            w.line("///");
        } else {
            w.line(format!("/// {l}"));
        }
    }
}

fn id(name: &str) -> String {
    naming::swift_ident(&naming::camel(name))
}

struct SwiftGen<'a> {
    model: &'a Model,
    cfg: &'a Generator,
}

/// The generation helpers that need the model.
struct Types<'a> {
    model: &'a Model,
}

impl Types<'_> {
    fn ty(&self, t: &TypeRef) -> String {
        match t {
            TypeRef::Bool => "Bool".to_owned(),
            TypeRef::I8 => "Int8".to_owned(),
            TypeRef::I16 => "Int16".to_owned(),
            TypeRef::I32 => "Int32".to_owned(),
            TypeRef::I64 => "Int64".to_owned(),
            TypeRef::U8 => "UInt8".to_owned(),
            TypeRef::U16 => "UInt16".to_owned(),
            TypeRef::U32 => "UInt32".to_owned(),
            TypeRef::U64 => "UInt64".to_owned(),
            TypeRef::F32 => "Float".to_owned(),
            TypeRef::F64 => "Double".to_owned(),
            TypeRef::String => "String".to_owned(),
            TypeRef::Bytes => "[UInt8]".to_owned(),
            TypeRef::Unit => "Void".to_owned(),
            TypeRef::Duration => "Duration".to_owned(),
            TypeRef::Timestamp => "Date".to_owned(),
            TypeRef::Uuid => "UUID".to_owned(),
            TypeRef::Option(inner) => format!("{}?", self.ty(inner)),
            TypeRef::Vec(inner) => format!("[{}]", self.ty(inner)),
            TypeRef::Map(k, v) => format!("[{}: {}]", self.ty(k), self.ty(v)),
            TypeRef::Named(name) => name.clone(),
            // Rejected by validation before generation starts.
            TypeRef::Lazy(_) | TypeRef::Result(..) | TypeRef::Stream(_) => "Never".to_owned(),
        }
    }

    /// The type as written in an expression, where `Wrapped?` is spelled
    /// `Optional<Wrapped>` so that it cannot be read as optional chaining.
    fn expr_ty(&self, t: &TypeRef) -> String {
        match t {
            TypeRef::Option(inner) => format!("Optional<{}>", self.expr_ty(inner)),
            TypeRef::Vec(inner) => format!("[{}]", self.expr_ty(inner)),
            TypeRef::Map(k, v) => format!("[{}: {}]", self.expr_ty(k), self.expr_ty(v)),
            other => self.ty(other),
        }
    }

    /// A statement that writes `value` of type `t` to the writer `w`.
    fn write_stmt(&self, t: &TypeRef, value: &str, w: &str) -> String {
        match t {
            TypeRef::Bytes => format!("{w}.writeBytes({value})"),
            TypeRef::Option(inner) if matches!(**inner, TypeRef::Bytes) => {
                format!("{value}.map(KeelBytes.init).keelEncode(&{w})")
            }
            _ => format!("{value}.keelEncode(&{w})"),
        }
    }

    /// An expression (to be written after `try`) reading a `t` from `r`.
    fn read_expr(&self, t: &TypeRef, r: &str) -> String {
        match t {
            TypeRef::Bytes => format!("{r}.readBytes()"),
            TypeRef::Option(inner) if matches!(**inner, TypeRef::Bytes) => {
                format!("Optional<KeelBytes>.keelDecode(&{r})?.bytes")
            }
            _ => format!("{}.keelDecode(&{r})", self.expr_ty(t)),
        }
    }

    /// An expression (to be written after `try`) decoding all of `bytes`.
    fn decode_all(&self, t: &TypeRef, bytes: &str) -> String {
        match t {
            TypeRef::Bytes => format!("KeelBytes.keelDecoded(from: {bytes}).bytes"),
            TypeRef::Option(inner) if matches!(**inner, TypeRef::Bytes) => {
                format!("Optional<KeelBytes>.keelDecoded(from: {bytes})?.bytes")
            }
            _ => format!("{}.keelDecoded(from: {bytes})", self.expr_ty(t)),
        }
    }

    /// An expression producing the encoded `[UInt8]` of `value`.
    fn encoded(&self, t: &TypeRef, value: &str) -> String {
        match t {
            TypeRef::Bytes => format!("KeelBytes({value}).keelEncoded()"),
            TypeRef::Option(inner) if matches!(**inner, TypeRef::Bytes) => {
                format!("{value}.map(KeelBytes.init).keelEncoded()")
            }
            _ => format!("{value}.keelEncoded()"),
        }
    }

    /// The zero value used as a signal's placeholder until the initial
    /// change-set arrives.
    fn zero(&self, t: &TypeRef, depth: usize) -> String {
        match t {
            TypeRef::Bool => "false".to_owned(),
            TypeRef::I8
            | TypeRef::I16
            | TypeRef::I32
            | TypeRef::I64
            | TypeRef::U8
            | TypeRef::U16
            | TypeRef::U32
            | TypeRef::U64
            | TypeRef::F32
            | TypeRef::F64 => "0".to_owned(),
            TypeRef::String => "\"\"".to_owned(),
            TypeRef::Bytes | TypeRef::Vec(_) => "[]".to_owned(),
            TypeRef::Map(..) => "[:]".to_owned(),
            TypeRef::Option(_) => "nil".to_owned(),
            TypeRef::Duration => ".zero".to_owned(),
            TypeRef::Timestamp => "Date(timeIntervalSince1970: 0)".to_owned(),
            TypeRef::Uuid => {
                "UUID(uuid: (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0))".to_owned()
            }
            TypeRef::Named(name) => self.zero_named(name, depth),
            TypeRef::Unit | TypeRef::Lazy(_) | TypeRef::Result(..) | TypeRef::Stream(_) => {
                "()".to_owned()
            }
        }
    }

    fn zero_named(&self, name: &str, depth: usize) -> String {
        if depth > 8 {
            return "fatalError(\"recursive default\")".to_owned();
        }
        match self.model.kind(name) {
            Some(NamedKind::Record) => {
                let Some(record) = self.model.record(name) else {
                    return String::new();
                };
                let args: Vec<String> = record
                    .fields
                    .iter()
                    .map(|f| format!("{}: {}", id(&f.name), self.zero(&f.ty, depth + 1)))
                    .collect();
                format!("{name}({})", args.join(", "))
            }
            Some(NamedKind::UnitEnum) => self
                .model
                .enum_def(name)
                .and_then(|e| e.variants.first())
                .map(|v| format!(".{}", id(&v.name)))
                .unwrap_or_default(),
            Some(NamedKind::DataEnum | NamedKind::Error) => {
                let en = self
                    .model
                    .enum_def(name)
                    .or_else(|| self.model.error_def(name));
                let Some(variant) = en.and_then(|e| e.variants.first()) else {
                    return String::new();
                };
                if variant.fields.is_empty() {
                    format!("{name}.{}", id(&variant.name))
                } else if variant.tuple {
                    let args: Vec<String> = variant
                        .fields
                        .iter()
                        .map(|f| self.zero(&f.ty, depth + 1))
                        .collect();
                    format!("{name}.{}({})", id(&variant.name), args.join(", "))
                } else {
                    let args: Vec<String> = variant
                        .fields
                        .iter()
                        .map(|f| format!("{}: {}", id(&f.name), self.zero(&f.ty, depth + 1)))
                        .collect();
                    format!("{name}.{}({})", id(&variant.name), args.join(", "))
                }
            }
            Some(NamedKind::Object) | None => String::new(),
        }
    }

    /// The value of a `#[keel(default)]` field, for the types whose default is
    /// unambiguous.
    fn default_value(&self, t: &TypeRef) -> Option<String> {
        match t {
            TypeRef::Named(_)
            | TypeRef::Unit
            | TypeRef::Lazy(_)
            | TypeRef::Result(..)
            | TypeRef::Stream(_) => None,
            other => Some(self.zero(other, 0)),
        }
    }

    /// Whether the struct or enum built from `t` can derive `Codable`.
    fn codable(&self, t: &TypeRef, visiting: &mut HashSet<String>) -> bool {
        match t {
            TypeRef::Option(i) | TypeRef::Vec(i) => self.codable(i, visiting),
            TypeRef::Map(k, v) => self.codable(k, visiting) && self.codable(v, visiting),
            TypeRef::Named(name) => match self.model.kind(name) {
                Some(NamedKind::UnitEnum) => true,
                Some(NamedKind::Record) => {
                    if !visiting.insert(name.clone()) {
                        return true;
                    }
                    self.model
                        .record(name)
                        .is_some_and(|r| r.fields.iter().all(|f| self.codable(&f.ty, visiting)))
                }
                _ => false,
            },
            TypeRef::Lazy(_) | TypeRef::Result(..) | TypeRef::Stream(_) => false,
            _ => true,
        }
    }

    /// Whether some payload of `en` holds `en` itself without going through
    /// an array or dictionary, which Swift only allows in an `indirect` enum.
    fn recursive_payload(en: &EnumDef) -> bool {
        fn holds(t: &TypeRef, name: &str) -> bool {
            match t {
                TypeRef::Named(n) => n == name,
                TypeRef::Option(i) => holds(i, name),
                _ => false,
            }
        }
        en.variants
            .iter()
            .any(|v| v.fields.iter().any(|f| holds(&f.ty, &en.name)))
    }
}

impl SwiftGen<'_> {
    fn types(&self) -> Types<'_> {
        Types { model: self.model }
    }

    fn header(&self, imports: &[&str]) -> CodeWriter {
        let mut w = CodeWriter::new("    ");
        w.line(format!(
            "// Generated by keel-bindgen from the Keel schema of `{}` (schema hash 0x{:016x}). Do not edit.",
            self.model.crate_name, self.model.schema_hash
        ));
        if !imports.is_empty() {
            w.blank();
            for i in imports {
                w.line(format!("import {i}"));
            }
        }
        w.blank();
        w
    }

    /// Whether the typed-throws form is in use.
    fn typed(&self) -> bool {
        self.cfg.swift_typed_throws
    }

    // ===== Types.swift =======================================================

    fn types_file(&self) -> String {
        let mut w = self.header(&["Foundation", "KeelRuntime"]);
        for record in &self.model.records {
            self.record(&mut w, record);
            w.blank();
        }
        for en in &self.model.enums {
            if is_unit_enum(en) {
                self.unit_enum(&mut w, en);
            } else {
                self.data_enum(&mut w, en);
            }
            w.blank();
        }
        w.finish()
    }

    fn record(&self, w: &mut CodeWriter, r: &RecordDef) {
        let t = self.types();
        let codable = r
            .fields
            .iter()
            .all(|f| t.codable(&f.ty, &mut HashSet::from([r.name.clone()])));
        let conformances = if codable {
            "KeelRecord, Sendable, Hashable, Codable"
        } else {
            "KeelRecord, Sendable, Hashable"
        };
        doc(w, &r.docs, &[]);
        w.block(format!("public struct {}: {conformances}", r.name), |w| {
            for f in &r.fields {
                doc(w, &f.docs, &[]);
                w.line(format!("public var {}: {}", id(&f.name), t.ty(&f.ty)));
            }
            if !r.fields.is_empty() {
                w.blank();
            }
            let params: Vec<String> = r
                .fields
                .iter()
                .map(|f| {
                    let default = if f.default {
                        t.default_value(&f.ty)
                            .map(|d| format!(" = {d}"))
                            .unwrap_or_default()
                    } else {
                        String::new()
                    };
                    format!("{}: {}{default}", id(&f.name), t.ty(&f.ty))
                })
                .collect();
            w.call_block("public init", &params, "", false, |w| {
                for f in &r.fields {
                    w.line(format!("self.{0} = {0}", id(&f.name)));
                }
            });
            w.blank();
            w.block(
                format!(
                    "public static func keelDecode(_ r: inout KeelReader) throws -> {}",
                    r.name
                ),
                |w| {
                    if r.fields.is_empty() {
                        w.line(format!("return {}()", r.name));
                    } else {
                        let args: Vec<String> = r
                            .fields
                            .iter()
                            .map(|f| format!("{}: {}", id(&f.name), t.read_expr(&f.ty, "r")))
                            .collect();
                        w.call(format!("return try {}", r.name), &args, "", false);
                    }
                },
            );
            w.blank();
            w.block("public func keelEncode(_ w: inout KeelWriter)", |w| {
                for f in &r.fields {
                    w.line(t.write_stmt(&f.ty, &format!("self.{}", id(&f.name)), "w"));
                }
            });
        });
    }

    fn unit_enum(&self, w: &mut CodeWriter, en: &EnumDef) {
        doc(w, &en.docs, &[]);
        w.block(
            format!(
                "public enum {}: UInt16, KeelEnum, CaseIterable, Sendable, Codable",
                en.name
            ),
            |w| {
                for v in &en.variants {
                    doc(w, &v.docs, &[]);
                    w.line(format!("case {} = {}", id(&v.name), v.index));
                }
                w.blank();
                w.block(
                    format!(
                        "public static func keelDecode(_ r: inout KeelReader) throws -> {}",
                        en.name
                    ),
                    |w| {
                        w.line("let at = r.position");
                        w.line("let tag = try r.readU16()");
                        w.block(
                            format!("guard let value = {}(rawValue: tag) else", en.name),
                            |w| {
                                w.line(format!(
                                "throw WireError.invalidTag(tag: UInt32(tag), at: at, type: {})",
                                swift_string(&en.name)
                            ));
                            },
                        );
                        w.line("return value");
                    },
                );
                w.blank();
                w.block("public func keelEncode(_ w: inout KeelWriter)", |w| {
                    w.line("w.writeU16(self.rawValue)");
                });
            },
        );
    }

    /// The associated-value declaration of a variant: `(radius: Double)`,
    /// `(Double, Double)` or nothing.
    fn payload(&self, v: &VariantDef) -> String {
        if v.fields.is_empty() {
            return String::new();
        }
        let t = self.types();
        let items: Vec<String> = v
            .fields
            .iter()
            .map(|f| {
                if v.tuple {
                    t.ty(&f.ty)
                } else {
                    format!("{}: {}", id(&f.name), t.ty(&f.ty))
                }
            })
            .collect();
        format!("({})", items.join(", "))
    }

    /// The binder names of a variant's fields in a `case .x(let a, let b)`
    /// pattern, avoiding `taken`.
    fn binders(v: &VariantDef, taken: &[&str]) -> Vec<String> {
        let count = v.fields.len();
        v.fields
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let base = if v.tuple {
                    naming::tuple_field(i, count)
                } else {
                    id(&f.name)
                };
                naming::avoid(&base, taken)
            })
            .collect()
    }

    fn data_enum(&self, w: &mut CodeWriter, en: &EnumDef) {
        let t = self.types();
        doc(w, &en.docs, &[]);
        let indirect = if Types::recursive_payload(en) {
            "indirect "
        } else {
            ""
        };
        w.block(
            format!(
                "public {indirect}enum {}: KeelEnum, Sendable, Hashable",
                en.name
            ),
            |w| {
                for v in &en.variants {
                    doc(w, &v.docs, &[]);
                    w.line(format!("case {}{}", id(&v.name), self.payload(v)));
                }
                w.blank();
                self.enum_codec(w, en, &t);
            },
        );
    }

    /// `keelDecode` and `keelEncode` of a data enum or error.
    fn enum_codec(&self, w: &mut CodeWriter, en: &EnumDef, t: &Types<'_>) {
        w.block(
            format!(
                "public static func keelDecode(_ r: inout KeelReader) throws -> {}",
                en.name
            ),
            |w| {
                w.line("let at = r.position");
                w.line("let tag = try r.readU16()");
                switch_block(w, "tag", |w| {
                    for v in &en.variants {
                        w.line(format!("case {}:", v.index));
                        w.indented(|w| {
                            if v.fields.is_empty() {
                                w.line(format!("return {}.{}", en.name, id(&v.name)));
                            } else {
                                let args: Vec<String> = v
                                    .fields
                                    .iter()
                                    .map(|f| {
                                        let expr = t.read_expr(&f.ty, "r");
                                        if v.tuple {
                                            expr
                                        } else {
                                            format!("{}: {expr}", id(&f.name))
                                        }
                                    })
                                    .collect();
                                w.call(
                                    format!("return try {}.{}", en.name, id(&v.name)),
                                    &args,
                                    "",
                                    false,
                                );
                            }
                        });
                    }
                    w.line("default:");
                    w.indented(|w| {
                        w.line(format!(
                            "throw WireError.invalidTag(tag: UInt32(tag), at: at, type: {})",
                            swift_string(&en.name)
                        ));
                    });
                });
            },
        );
        w.blank();
        w.block("public func keelEncode(_ w: inout KeelWriter)", |w| {
            switch_block(w, "self", |w| {
                for v in &en.variants {
                    let binders = Self::binders(v, &["w"]);
                    let pattern: Vec<String> = binders.iter().map(|b| format!("let {b}")).collect();
                    if pattern.is_empty() {
                        w.line(format!("case .{}:", id(&v.name)));
                    } else {
                        w.line(format!("case .{}({}):", id(&v.name), pattern.join(", ")));
                    }
                    w.indented(|w| {
                        w.line(format!("w.writeU16({})", v.index));
                        for (f, b) in v.fields.iter().zip(&binders) {
                            w.line(t.write_stmt(&f.ty, b, "w"));
                        }
                    });
                }
            });
        });
    }

    // ===== Errors.swift ======================================================

    fn errors_file(&self) -> String {
        let mut w = self.header(&["Foundation", "KeelRuntime"]);
        for en in &self.model.errors {
            self.error(&mut w, en);
            w.blank();
        }
        w.line("/// Stops the process for a failure that the shape of the API cannot express: a core panic,");
        w.line("/// a malformed reply, or schema drift. Such a failure means the core and the bindings disagree,");
        w.line("/// so it is reported loudly instead of masquerading as a domain error.");
        w.block(
            "func keelUnexpected(_ error: any Error, file: StaticString = #fileID, line: UInt = #line) -> Never",
            |w| {
                w.line("fatalError(\"Keel: unexpected failure of a core call: \\(error)\", file: file, line: line)");
            },
        );
        w.finish()
    }

    /// Which fields of an error variant its message uses.
    fn used_fields(v: &VariantDef) -> Vec<bool> {
        let mut used = vec![false; v.fields.len()];
        match &v.message {
            Some(template) => {
                for part in parse_message(v, template).unwrap_or_default() {
                    if let MsgPart::Field(i) = part {
                        used[i] = true;
                    }
                }
            }
            // `#[error(transparent)]` uses its only field.
            None => {
                if let Some(first) = used.first_mut() {
                    *first = true;
                }
            }
        }
        used
    }

    /// The `description` of one error variant.
    fn error_description(&self, v: &VariantDef, binders: &[String]) -> String {
        let is_error = |t: &TypeRef| matches!(t, TypeRef::Named(n) if self.model.kind(n) == Some(NamedKind::Error));
        let Some(template) = &v.message else {
            // `#[error(transparent)]`: the wrapped error's own description.
            return match (binders.first(), v.fields.first()) {
                (Some(b), Some(f)) if is_error(&f.ty) => format!("{b}.description"),
                (Some(b), _) => format!("String(describing: {b})"),
                _ => "\"\"".to_owned(),
            };
        };
        let parts = parse_message(v, template).unwrap_or_default();
        let mut out = String::from("\"");
        for part in &parts {
            match part {
                MsgPart::Text(t) => out.push_str(&swift_template_text(t)),
                MsgPart::Field(i) => {
                    let b = &binders[*i];
                    if is_error(&v.fields[*i].ty) {
                        out.push_str(&format!("\\({b}.description)"));
                    } else {
                        out.push_str(&format!("\\({b})"));
                    }
                }
            }
        }
        out.push('"');
        out
    }

    fn error(&self, w: &mut CodeWriter, en: &EnumDef) {
        let t = self.types();
        doc(w, &en.docs, &[]);
        let indirect = if Types::recursive_payload(en) {
            "indirect "
        } else {
            ""
        };
        w.block(
            format!(
                "public {indirect}enum {}: KeelError, Error, Sendable, Hashable",
                en.name
            ),
            |w| {
                for v in &en.variants {
                    doc(w, &v.docs, &[]);
                    w.line(format!("case {}{}", id(&v.name), self.payload(v)));
                }
                w.blank();
                self.enum_codec(w, en, &t);
            },
        );
        w.blank();
        w.block(
            format!(
                "extension {}: CustomStringConvertible, LocalizedError",
                en.name
            ),
            |w| {
                w.block("public var description: String", |w| {
                    switch_block(w, "self", |w| {
                        for v in &en.variants {
                            let binders = Self::binders(v, &[]);
                            let used = Self::used_fields(v);
                            // Only the fields the message mentions are bound.
                            let pattern: Vec<String> = binders
                                .iter()
                                .zip(&used)
                                .map(|(b, used)| {
                                    if *used {
                                        format!("let {b}")
                                    } else {
                                        "_".to_owned()
                                    }
                                })
                                .collect();
                            if !used.contains(&true) {
                                w.line(format!("case .{}:", id(&v.name)));
                            } else {
                                w.line(format!("case .{}({}):", id(&v.name), pattern.join(", ")));
                            }
                            w.indented(|w| {
                                w.line(format!("return {}", self.error_description(v, &binders)));
                            });
                        }
                    });
                });
                w.blank();
                w.block("public var errorDescription: String?", |w| {
                    w.line("return description");
                });
            },
        );
        w.blank();
        w.block(format!("extension {}", en.name), |w| {
            doc(
                w,
                "The typed error that a failed call carries, or `nil` for any other failure.",
                &[],
            );
            w.block(
                format!(
                    "static func keelFromReply(_ error: any Error) -> {}?",
                    en.name
                ),
                |w| {
                    w.block(
                        "guard let reply = error as? KeelReplyError, reply.status == .error else",
                        |w| w.line("return nil"),
                    );
                    w.line(format!(
                        "return try? {}.keelDecoded(from: reply.body)",
                        en.name
                    ));
                },
            );
        });
    }

    // ===== callables =========================================================

    /// The declaration parameters of `params`: labeled, except that a single
    /// record or enum parameter goes unlabeled, as in `setFilter(_ f: Filter)`.
    fn param_decls(&self, params: &[ParamDef]) -> Vec<String> {
        let t = self.types();
        let unlabeled = self.unlabeled(params);
        params
            .iter()
            .map(|p| {
                let name = id(&p.name);
                let lead = if unlabeled { "_ " } else { "" };
                format!("{lead}{name}: {}", t.ty(&p.ty))
            })
            .collect()
    }

    /// Whether `params` is a single record or enum parameter, which is
    /// declared without an argument label.
    fn unlabeled(&self, params: &[ParamDef]) -> bool {
        params.len() == 1
            && matches!(&params[0].ty, TypeRef::Named(n) if matches!(
                self.model.kind(n),
                Some(NamedKind::Record | NamedKind::UnitEnum | NamedKind::DataEnum)
            ))
    }

    /// `var w = KeelWriter()` and the encoding of `params`; returns the
    /// expression holding the arguments.
    fn encode_args(&self, w: &mut CodeWriter, params: &[ParamDef], writer: &str) -> String {
        if params.is_empty() {
            return "[]".to_owned();
        }
        let t = self.types();
        w.line(format!("var {writer} = KeelWriter()"));
        for p in params {
            w.line(t.write_stmt(&p.ty, &id(&p.name), writer));
        }
        format!("{writer}.finish()")
    }

    /// The error-mapping `catch` of a call that can fail with `err`.
    fn catch_typed(&self, w: &mut CodeWriter, err: &str) {
        w.line("} catch {");
        w.indented(|w| {
            if self.typed() {
                w.line(format!(
                    "guard let typed = {err}.keelFromReply(error) else {{ keelUnexpected(error) }}"
                ));
                w.line("throw typed");
            } else {
                w.line(format!("throw {err}.keelFromReply(error) ?? error"));
            }
        });
        w.line("}");
    }

    /// `throws`, `throws(E)` or nothing.
    fn throws_clause(&self, err: Option<&str>) -> String {
        match err {
            Some(err) if self.typed() => format!(" throws({err})"),
            Some(_) => " throws".to_owned(),
            None => String::new(),
        }
    }

    // ===== Objects.swift =====================================================

    fn objects_file(&self) -> String {
        let mut w = self.header(&["Foundation", "KeelRuntime"]);
        let mut needs_stream = false;
        for object in &self.model.objects {
            needs_stream |= self.object(&mut w, object);
            w.blank();
        }
        for function in &self.model.functions {
            needs_stream |= self.function(&mut w, function, "KeelIds.Functions");
            w.blank();
        }
        if needs_stream {
            self.decode_stream_helper(&mut w);
        }
        w.finish()
    }

    fn decode_stream_helper(&self, w: &mut CodeWriter) {
        w.line("/// Decodes every item of a core stream; a failure passes through `mapError`.");
        w.line("fileprivate func keelDecodeStream<T: Sendable>(");
        w.indented(|w| {
            w.line("_ source: AsyncThrowingStream<[UInt8], Error>,");
            w.line("decode: @escaping @Sendable ([UInt8]) throws -> T,");
            w.line("mapError: @escaping @Sendable (any Error) -> any Error");
        });
        w.line(") -> AsyncThrowingStream<T, Error> {");
        w.indented(|w| {
            w.line("return AsyncThrowingStream { continuation in");
            w.indented(|w| {
                w.line("let task = Task {");
                w.indented(|w| {
                    w.line("do {");
                    w.indented(|w| {
                        w.line("for try await item in source {");
                        w.indented(|w| w.line("continuation.yield(try decode(item))"));
                        w.line("}");
                        w.line("continuation.finish()");
                    });
                    w.line("} catch {");
                    w.indented(|w| w.line("continuation.finish(throwing: mapError(error))"));
                    w.line("}");
                });
                w.line("}");
                w.line("continuation.onTermination = { _ in");
                w.indented(|w| w.line("task.cancel()"));
                w.line("}");
            });
            w.line("}");
        });
        w.line("}");
    }

    /// Writes an object; returns whether it uses `keelDecodeStream`.
    fn object(&self, w: &mut CodeWriter, o: &ObjectDef) -> bool {
        let store = o.store.is_some();
        let signals: Vec<&SignalDef> = o.store.iter().flat_map(|s| s.signals.iter()).collect();
        let base = if store { "KeelStore" } else { "KeelObject" };
        let mut uses_stream = false;
        doc(w, &o.docs, &[]);
        if store {
            w.line("@MainActor @Observable");
            w.line(format!("public final class {}: {base} {{", o.name));
        } else {
            w.line(format!(
                "public final class {}: {base}, @unchecked Sendable {{",
                o.name
            ));
        }
        w.indented(|w| {
            let t = self.types();
            for g in &signals {
                if let Some(d) = self.model.signal_doc(o, g) {
                    doc(w, d, &[]);
                }
                w.line(format!(
                    "public private(set) var {}: {} = {}",
                    id(&g.name),
                    t.ty(&g.ty),
                    t.zero(&g.ty, 0)
                ));
            }
            if !signals.is_empty() {
                w.blank();
            }
            w.block(
                "private init(adopting handle: KeelHandle, core: KeelCore)",
                |w| {
                    w.line("super.init(core: core, handle: handle)");
                    if store {
                        w.line("core.observe(handle, signal: Observe.allSignals, on: true)");
                    }
                },
            );
            for c in &o.constructors {
                w.blank();
                self.constructor(w, o, c);
            }
            for m in &o.methods {
                w.blank();
                let ids = format!("KeelIds.Objects.{}", o.name);
                uses_stream |= self.callable(
                    w,
                    &Callable::from_method(m),
                    &Site::Method {
                        id: format!("{ids}.{}", id(&m.name)),
                    },
                );
            }
            if store {
                w.blank();
                self.store_apply(w, o, &signals);
            }
        });
        w.line("}");
        uses_stream
    }

    fn constructor(&self, w: &mut CodeWriter, o: &ObjectDef, c: &MethodDef) {
        let ret = Ret::classify(&c.returns);
        let err = ret.as_ref().and_then(Ret::error).map(str::to_owned);
        let taken: Vec<String> = c.params.iter().map(|p| id(&p.name)).collect();
        let taken_refs: Vec<&str> = taken.iter().map(String::as_str).collect();
        let ctx = naming::avoid("ctx", &taken_refs);
        let writer = naming::avoid("w", &taken_refs);
        let handle = naming::avoid("handle", &taken_refs);
        let reply = naming::avoid("body", &taken_refs);
        let mut params = self.param_decls(&c.params);
        params.push(format!("{ctx}: KeelCore = .shared"));
        // A lone unlabeled parameter is still followed by `ctx`, which is labeled.
        let is_new = c.name == "new";
        let mut extra = Vec::new();
        if let Some(err) = &err {
            extra.push(format!("- Throws: ``{err}``."));
        }
        doc(w, &c.docs, &extra);
        let asyncw = if c.is_async { " async" } else { "" };
        let throws = match &err {
            Some(err) => self.throws_clause(Some(err)),
            None => " throws".to_owned(),
        };
        let ids = format!("KeelIds.Objects.{}", o.name);
        let member = id(&c.name);
        let (prefix, suffix) = if is_new {
            (
                "public convenience init".to_owned(),
                format!("{asyncw}{throws}"),
            )
        } else {
            (
                format!("public static func {member}"),
                format!("{asyncw}{throws} -> {}", o.name),
            )
        };
        w.call_block(prefix, &params, suffix, false, |w| {
            let args = self.encode_args(w, &c.params, &writer);
            let t = self.types();
            let obtain = |w: &mut CodeWriter, bind: &str| {
                if c.is_async {
                    w.call(
                        format!("let {reply} = try await {ctx}.call"),
                        &[
                            format!(".constructor(typeId: {ids}.typeId, methodId: {ids}.{member})"),
                            format!("method: {ids}.{member}"),
                            format!("args: {args}"),
                        ],
                        "",
                        false,
                    );
                    w.line(format!(
                        "{bind}try {}",
                        t.decode_all(&TypeRef::named("KeelHandle"), &reply)
                    ));
                } else {
                    w.call(
                        format!("{bind}try {ctx}.construct"),
                        &[
                            format!("type: {ids}.typeId"),
                            format!("method: {ids}.{member}"),
                            format!("args: {args}"),
                        ],
                        "",
                        false,
                    );
                }
            };
            if let Some(err) = &err {
                w.line(format!("let {handle}: KeelHandle"));
                w.line("do {");
                w.indented(|w| obtain(w, &format!("{handle} = ")));
                self.catch_typed(w, err);
            } else {
                obtain(w, &format!("let {handle} = "));
            }
            if is_new {
                w.line(format!("self.init(adopting: {handle}, core: {ctx})"));
            } else {
                w.line(format!(
                    "return {}(adopting: {handle}, core: {ctx})",
                    o.name
                ));
            }
        });
    }

    fn function(&self, w: &mut CodeWriter, f: &FunctionDef, ids: &str) -> bool {
        let id = format!("{ids}.{}", id(&f.name));
        self.callable(w, &Callable::from_function(f), &Site::Function { id })
    }

    /// One method or free function; returns whether it uses
    /// `keelDecodeStream`.
    fn callable(&self, w: &mut CodeWriter, c: &Callable<'_>, site: &Site) -> bool {
        let t = self.types();
        let ret = Ret::classify(c.returns).unwrap_or(Ret::Plain(c.returns));
        let taken: Vec<String> = c.params.iter().map(|p| id(&p.name)).collect();
        let taken_refs: Vec<&str> = taken.iter().map(String::as_str).collect();
        let writer = naming::avoid("w", &taken_refs);
        let body = naming::avoid("body", &taken_refs);
        let source = naming::avoid("source", &taken_refs);
        let (core, target, mid, is_function) = match site {
            Site::Method { id } => (
                "self.core".to_owned(),
                format!(".objectMethod(handle: self.handle, methodId: {id})"),
                id.clone(),
                false,
            ),
            Site::Function { id } => (
                naming::avoid("ctx", &taken_refs),
                format!(".freeFunction(methodId: {id})"),
                id.clone(),
                true,
            ),
        };
        let mut params = self.param_decls(c.params);
        if is_function {
            params.push(format!("{core}: KeelCore = .shared"));
        }
        let err = ret.error().map(str::to_owned);
        let mut extra = Vec::new();
        if let Some(err) = &err {
            extra.push(format!("- Throws: ``{err}``."));
        }
        doc(w, c.docs, &extra);
        let name = id(c.name);
        let head = format!("public func {name}");

        if let Ret::Stream(item) | Ret::ResultStream { item, .. } = &ret {
            let item_ty = t.ty(item);
            let suffix = format!(" -> AsyncThrowingStream<{item_ty}, Error>");
            w.call_block(head, &params, suffix, false, |w| {
                let args = self.encode_args(w, c.params, &writer);
                w.call(
                    format!("let {source} = {core}.stream"),
                    &[
                        target.clone(),
                        format!("method: {mid}"),
                        format!("args: {args}"),
                    ],
                    "",
                    false,
                );
                let decode = format!("{{ try {} }}", t.decode_all(item, "$0"));
                let map_error = match &err {
                    Some(err) => format!("{{ {err}.keelFromReply($0) ?? $0 }}"),
                    None => "{ $0 }".to_owned(),
                };
                w.call(
                    "return keelDecodeStream",
                    &[
                        source.clone(),
                        format!("decode: {decode}"),
                        format!("mapError: {map_error}"),
                    ],
                    "",
                    false,
                );
            });
            return true;
        }

        let (ok, is_unit) = match &ret {
            Ret::Plain(ty) | Ret::Result { ok: ty, .. } => (Some(*ty), matches!(ty, TypeRef::Unit)),
            Ret::Stream(_) | Ret::ResultStream { .. } => (None, true),
        };
        let asyncw = if c.is_async { " async" } else { "" };
        // Async methods without a typed error use plain `throws`, so task
        // cancellation and transport failures propagate.
        let throws = match &err {
            Some(err) => self.throws_clause(Some(err)),
            None if c.is_async => " throws".to_owned(),
            None => String::new(),
        };
        let returns = match ok {
            Some(ty) if !is_unit => format!(" -> {}", t.ty(ty)),
            _ => String::new(),
        };
        let suffix = format!("{asyncw}{throws}{returns}");
        w.call_block(head, &params, suffix, false, |w| {
            let args = self.encode_args(w, c.params, &writer);
            let method = if c.is_async { "call" } else { "callSync" };
            let awaited = if c.is_async { "await " } else { "" };
            let call_args = [
                target.clone(),
                format!("method: {mid}"),
                format!("args: {args}"),
            ];
            let guarded = err.is_some() || !c.is_async;
            let bind = if is_unit {
                "_ = ".to_owned()
            } else {
                format!("let {body} = ")
            };
            if guarded {
                w.line("do {");
                w.indented(|w| {
                    w.call(
                        format!("{bind}try {awaited}{core}.{method}"),
                        &call_args,
                        "",
                        false,
                    );
                    if let (Some(ty), false) = (ok, is_unit) {
                        w.line(format!("return try {}", t.decode_all(ty, &body)));
                    }
                });
                match &err {
                    Some(err) => self.catch_typed(w, err),
                    None => {
                        w.line("} catch {");
                        w.indented(|w| w.line("keelUnexpected(error)"));
                        w.line("}");
                    }
                }
            } else {
                w.call(
                    format!("{bind}try {awaited}{core}.{method}"),
                    &call_args,
                    "",
                    false,
                );
                if let (Some(ty), false) = (ok, is_unit) {
                    w.line(format!("return try {}", t.decode_all(ty, &body)));
                }
            }
        });
        false
    }

    // ===== Stores.swift ======================================================

    fn stores_file(&self) -> String {
        let mut w = self.header(&["Foundation", "KeelRuntime", "Observation"]);
        let mut needs_stream = false;
        for store in &self.model.stores {
            needs_stream |= self.object(&mut w, store);
            w.blank();
        }
        if needs_stream {
            self.decode_stream_helper(&mut w);
        }
        w.finish()
    }

    /// `apply(signal:op:reader:)`: decodes full values and applies keyed
    /// patches per signal.
    fn store_apply(&self, w: &mut CodeWriter, o: &ObjectDef, signals: &[&SignalDef]) {
        let t = self.types();
        w.block(
            "public override func apply(signal: UInt32, op: ChangeOp, reader: inout KeelReader)",
            |w| {
                w.line("do {");
                w.indented(|w| {
                    switch_block(w, "signal", |w| {
                        for g in signals {
                            let prop = format!("self.{}", id(&g.name));
                            w.line(format!("case {}:", g.signal_id));
                            w.indented(|w| {
                                switch_block(w, "op", |w| {
                                    w.line("case .fullValue:");
                                    w.indented(|w| {
                                        w.line(format!(
                                            "{prop} = try {}",
                                            t.read_expr(&g.ty, "reader")
                                        ));
                                        w.line("try reader.finish()");
                                    });
                                    w.line("case .keyedPatch:");
                                    w.indented(|w| {
                                        if let (Some(_), TypeRef::Vec(item)) = (&g.key, &g.ty) {
                                            w.line(format!(
                                                "let ops: [PatchOp<{}>] = try decodePatch(&reader)",
                                                t.ty(item)
                                            ));
                                            w.line("try reader.finish()");
                                            w.line(format!("try applyPatch(ops, to: &{prop})"));
                                        } else {
                                            w.line("break");
                                        }
                                    });
                                    w.line("case .lazyListInvalidated:");
                                    w.indented(|w| w.line("break"));
                                });
                            });
                        }
                        w.line("default:");
                        w.indented(|w| w.line("break"));
                    });
                });
                w.line("} catch is PatchError {");
                w.indented(|w| {
                    w.line("// The mirror diverged from the core: re-observe to receive a full value.");
                    w.line("self.core.observe(self.handle, signal: signal, on: false)");
                    w.line("self.core.observe(self.handle, signal: signal, on: true)");
                });
                w.line("} catch {");
                w.indented(|w| {
                    w.line(format!(
                        "assertionFailure(\"Keel: undecodable change for signal \\(signal) of {}: \\(error)\")",
                        o.name
                    ));
                });
                w.line("}");
            },
        );
    }

    // ===== Ports.swift =======================================================

    fn ports_file(&self) -> String {
        let mut w = self.header(&["Foundation", "KeelRuntime"]);
        for port in &self.model.ports {
            if port.kind == PortKind::Event {
                self.event_port(&mut w, port);
            } else {
                self.port(&mut w, port);
            }
            w.blank();
        }
        w.finish()
    }

    fn port(&self, w: &mut CodeWriter, p: &PortDef) {
        let t = self.types();
        let sync = p.kind == PortKind::Sync;
        doc(w, &p.docs, &[]);
        w.block(
            format!("public protocol {}: KeelPort, Sendable", p.name),
            |w| {
                for m in &p.methods {
                    let ret = Ret::classify(&m.returns).unwrap_or(Ret::Plain(&m.returns));
                    let mut extra = Vec::new();
                    if let Some(err) = ret.error() {
                        extra.push(format!("- Throws: ``{err}``."));
                    }
                    doc(w, &m.docs, &extra);
                    let params = self.param_decls(&m.params);
                    let asyncw = if m.is_async { " async" } else { "" };
                    let throws = self.throws_clause(ret.error());
                    let returns = match &ret {
                        Ret::Plain(ty) | Ret::Result { ok: ty, .. }
                            if !matches!(ty, TypeRef::Unit) =>
                        {
                            format!(" -> {}", t.ty(ty))
                        }
                        _ => String::new(),
                    };
                    w.call(
                        format!("func {}", id(&m.name)),
                        &params,
                        format!("{asyncw}{throws}{returns}"),
                        false,
                    );
                }
            },
        );
        w.blank();
        doc(
            w,
            &format!(
                "Adapts an implementation of ``{0}`` to `KeelCore.registerPort(KeelIds.Ports.{0}.portId, _:)`.",
                p.name
            ),
            &[],
        );
        let fn_name = format!("{}PortImpl", naming::camel(&p.name));
        w.block(
            format!("public func {fn_name}(_ impl: any {}) -> PortImpl", p.name),
            |w| {
                let factory = if sync { ".sync" } else { ".async" };
                w.line(format!("return {factory}(["));
                w.indented(|w| {
                    for m in &p.methods {
                        self.port_method(w, p, m, sync);
                    }
                });
                w.line("])");
            },
        );
    }

    fn port_method(&self, w: &mut CodeWriter, p: &PortDef, m: &MethodDef, sync: bool) {
        let t = self.types();
        let ret = Ret::classify(&m.returns).unwrap_or(Ret::Plain(&m.returns));
        let member = id(&m.name);
        let key = format!("KeelIds.Ports.{}.{member}", p.name);
        let taken: Vec<String> = m.params.iter().map(|a| id(&a.name)).collect();
        let idents: Vec<String> = taken
            .iter()
            .map(|n| naming::avoid(n, &["r", "args", "impl", "error", "result"]))
            .collect();
        let params = if m.params.is_empty() { "_" } else { "args" };
        w.line(format!("{key}: {{ {params} in"));
        w.indented(|w| {
            if !m.params.is_empty() {
                w.line("var r = KeelReader(args)");
                for (a, ident) in m.params.iter().zip(&idents) {
                    w.line(format!("let {ident} = try {}", t.read_expr(&a.ty, "r")));
                }
                w.line("try r.finish()");
            }
            let awaited = if m.is_async && !sync { "await " } else { "" };
            let tried = if ret.error().is_some() { "try " } else { "" };
            let unlabeled = self.unlabeled(&m.params);
            let call_args: Vec<String> = taken
                .iter()
                .zip(&idents)
                .map(|(label, ident)| {
                    if unlabeled {
                        ident.clone()
                    } else {
                        format!("{label}: {ident}")
                    }
                })
                .collect();
            let call = format!("{tried}{awaited}impl.{member}({})", call_args.join(", "));
            let finish = |w: &mut CodeWriter, ok: &TypeRef| {
                if matches!(ok, TypeRef::Unit) {
                    w.line(call.clone());
                    w.line("return []");
                } else {
                    w.line(format!("let result = {call}"));
                    w.line(format!("return {}", t.encoded(ok, "result")));
                }
            };
            match &ret {
                Ret::Result { ok, err } => {
                    w.line("do {");
                    w.indented(|w| finish(w, ok));
                    w.line(format!("}} catch let error as {err} {{"));
                    w.indented(|w| {
                        w.line("throw KeelPortError(body: error.keelEncoded())");
                    });
                    w.line("}");
                }
                Ret::Plain(ok) => finish(w, ok),
                Ret::Stream(_) | Ret::ResultStream { .. } => {}
            }
        });
        w.line("},");
    }

    fn event_port(&self, w: &mut CodeWriter, p: &PortDef) {
        doc(
            w,
            &p.docs,
            &["Sends the events of this port from the host to the core.".to_owned()],
        );
        w.block(format!("public struct {}Events: Sendable", p.name), |w| {
            w.line("private let core: KeelCore");
            w.blank();
            w.block("public init(ctx: KeelCore = .shared)", |w| {
                w.line("self.core = ctx");
            });
            for m in &p.methods {
                w.blank();
                doc(w, &m.docs, &[]);
                let params = self.param_decls(&m.params);
                let taken: Vec<String> = m.params.iter().map(|a| id(&a.name)).collect();
                let taken_refs: Vec<&str> = taken.iter().map(String::as_str).collect();
                let writer = naming::avoid("w", &taken_refs);
                w.call_block(
                    format!("public func {}", id(&m.name)),
                    &params,
                    "",
                    false,
                    |w| {
                        let args = self.encode_args(w, &m.params, &writer);
                        w.call(
                            "self.core.event",
                            &[
                                format!("port: KeelIds.Ports.{}.portId", p.name),
                                format!("method: KeelIds.Ports.{}.{}", p.name, id(&m.name)),
                                format!("payload: {args}"),
                            ],
                            "",
                            false,
                        );
                    },
                );
            }
        });
    }

    // ===== Queries.swift =====================================================

    fn queries_file(&self) -> String {
        let mut w = self.header(&["Foundation", "KeelRuntime", "Observation"]);
        let mut needs_stream = false;
        for handle in &self.model.query_handles {
            needs_stream |= self.object(&mut w, handle);
            w.blank();
        }
        for mutation in &self.model.mutations {
            needs_stream |= self.function(&mut w, mutation, "KeelIds.Queries");
            w.blank();
        }
        if needs_stream {
            self.decode_stream_helper(&mut w);
        }
        w.finish()
    }

    // ===== Ids.swift =========================================================

    fn ids_file(&self) -> String {
        let m = self.model;
        let mut w = self.header(&[]);
        w.line("/// Stable wire identifiers (SPEC section 1.1), for logs and debugging, plus the schema");
        w.line("/// hash to pass to `KeelCore.load` as the expected schema hash.");
        w.block("public enum KeelIds", |w| {
            w.line(format!(
                "public static let schemaHash: UInt64 = 0x{:016x}",
                m.schema_hash
            ));
            w.blank();
            w.block("public enum Objects", |w| {
                for o in m.all_objects() {
                    w.block(format!("public enum {}", o.name), |w| {
                        w.line(format!(
                            "public static let typeId: UInt32 = {}",
                            hex(o.type_id)
                        ));
                        for method in o.constructors.iter().chain(&o.methods) {
                            w.line(format!(
                                "public static let {}: UInt32 = {}",
                                id(&method.name),
                                hex(method.method_id)
                            ));
                        }
                    });
                }
            });
            w.blank();
            w.block("public enum Functions", |w| {
                for f in &m.functions {
                    w.line(format!(
                        "public static let {}: UInt32 = {}",
                        id(&f.name),
                        hex(f.method_id)
                    ));
                }
            });
            w.blank();
            w.block("public enum Ports", |w| {
                for p in &m.ports {
                    w.block(format!("public enum {}", p.name), |w| {
                        w.line(format!(
                            "public static let portId: UInt32 = {}",
                            hex(p.port_id)
                        ));
                        for method in &p.methods {
                            w.line(format!(
                                "public static let {}: UInt32 = {}",
                                id(&method.name),
                                hex(method.method_id)
                            ));
                        }
                    });
                }
            });
            w.blank();
            w.block("public enum Queries", |w| {
                for q in &m.queries {
                    w.line(format!(
                        "public static let {}: UInt32 = {}",
                        id(&q.name),
                        hex(q.query_id)
                    ));
                }
            });
        });
        w.finish()
    }
}

/// `switch subject { .. }` with the `case` labels at the level of the `switch`,
/// as Swift style has it.
fn switch_block(w: &mut CodeWriter, subject: &str, body: impl FnOnce(&mut CodeWriter)) {
    w.line(format!("switch {subject} {{"));
    body(w);
    w.line("}");
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
    /// A method of the generated class; `id` is the Swift expression of its
    /// method id.
    Method { id: String },
    /// A top-level function.
    Function { id: String },
}
