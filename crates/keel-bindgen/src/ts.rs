//! The TypeScript generator (SPEC section 10.3).
//!
//! Output: `src/{types,errors,objects,stores,ports,queries,ids,index}.ts`
//! plus `package.json` and `tsconfig.json`. Generated code depends only on
//! `@keel/runtime` (SPEC section 17.1).
//!
//! Codec composition is hoisted: a `Vec<String>` field does not build a new
//! `codecs.vec(codecs.string)` on every call, the file declares it once
//! (`const vecString = ...`) below the named codecs. The two files that can
//! import each other at module-evaluation time, `types.ts` and `errors.ts`,
//! fall back to inline composition for cross-file composites when each
//! references the other, so the module graph never reads an uninitialised
//! binding.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use keel_meta::{
    EnumDef, FunctionDef, MethodDef, ObjectDef, ParamDef, PortDef, PortKind, RecordDef, SignalDef,
    TypeRef, VariantDef,
};

use crate::emit::CodeWriter;
use crate::model::{Model, MsgPart, NamedKind, Ret, doc_lines, parse_message};
use crate::naming;
use crate::{GeneratedFile, Generator};

/// The generated modules that can hold a named type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Module {
    Types,
    Errors,
    Objects,
    Stores,
    Ports,
    Queries,
}

impl Module {
    fn stem(self) -> &'static str {
        match self {
            Module::Types => "types",
            Module::Errors => "errors",
            Module::Objects => "objects",
            Module::Stores => "stores",
            Module::Ports => "ports",
            Module::Queries => "queries",
        }
    }
}

const RUNTIME: &str = "@keel/runtime";

pub(crate) fn generate(model: &Model, cfg: &Generator) -> Vec<GeneratedFile> {
    let ts = TsGen {
        model,
        cfg,
        cyclic: types_and_errors_reference_each_other(model),
    };
    let mut files = vec![
        ts.types_file(),
        ts.errors_file(),
        ts.objects_file(),
        ts.stores_file(),
        ts.ports_file(),
        ts.queries_file(),
        ts.ids_file(),
        ts.index_file(),
    ];
    files.push(GeneratedFile {
        path: "package.json".to_owned(),
        contents: ts.package_json(),
    });
    files.push(GeneratedFile {
        path: "tsconfig.json".to_owned(),
        contents: TSCONFIG.to_owned(),
    });
    files
}

const TSCONFIG: &str = r#"{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "lib": ["ES2022", "DOM"],
    "strict": true,
    "noUncheckedIndexedAccess": true,
    "exactOptionalPropertyTypes": true,
    "noImplicitOverride": true,
    "noImplicitReturns": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noFallthroughCasesInSwitch": true,
    "verbatimModuleSyntax": true,
    "isolatedModules": true,
    "skipLibCheck": true,
    "declaration": true,
    "sourceMap": true,
    "rootDir": "src",
    "outDir": "dist"
  },
  "include": ["src"]
}
"#;

/// Whether `ty` mentions a named type for which `pred` holds.
fn mentions(ty: &TypeRef, pred: &dyn Fn(&str) -> bool) -> bool {
    match ty {
        TypeRef::Named(n) => pred(n),
        TypeRef::Option(t) | TypeRef::Vec(t) | TypeRef::Lazy(t) | TypeRef::Stream(t) => {
            mentions(t, pred)
        }
        TypeRef::Map(k, v) | TypeRef::Result(k, v) => mentions(k, pred) || mentions(v, pred),
        _ => false,
    }
}

fn types_and_errors_reference_each_other(model: &Model) -> bool {
    let is_error = |n: &str| model.kind(n) == Some(NamedKind::Error);
    let is_type = |n: &str| {
        matches!(
            model.kind(n),
            Some(NamedKind::Record | NamedKind::UnitEnum | NamedKind::DataEnum)
        )
    };
    let types_use_errors = model
        .records
        .iter()
        .flat_map(|r| r.fields.iter().map(|f| &f.ty))
        .chain(model.enums.iter().flat_map(|e| {
            e.variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|f| &f.ty))
        }))
        .any(|t| mentions(t, &is_error));
    let errors_use_types = model
        .errors
        .iter()
        .flat_map(|e| {
            e.variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|f| &f.ty))
        })
        .any(|t| mentions(t, &is_type));
    types_use_errors && errors_use_types
}

/// Escapes `s` as a double-quoted JavaScript string literal.
fn js_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Escapes literal text for use inside a JavaScript template literal.
fn template_text(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push_str("\\\\"),
            '`' => out.push_str("\\`"),
            '$' if chars.peek() == Some(&'{') => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

fn hex(id: u32) -> String {
    format!("0x{id:08x}")
}

/// Writes a JSDoc block. Nothing is written for empty `docs` and no `extra`.
fn jsdoc(w: &mut CodeWriter, docs: &str, extra: &[String]) {
    let mut lines = doc_lines(docs);
    lines.extend(extra.iter().cloned());
    if lines.is_empty() {
        return;
    }
    let lines: Vec<String> = lines.iter().map(|l| l.replace("*/", "*\\/")).collect();
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

struct TsGen<'a> {
    model: &'a Model,
    cfg: &'a Generator,
    cyclic: bool,
}

/// The imports a file collects while it is written.
#[derive(Default)]
struct Imports {
    rt_values: BTreeSet<String>,
    rt_types: BTreeSet<String>,
    local: BTreeMap<&'static str, (BTreeSet<String>, BTreeSet<String>)>,
}

impl Imports {
    fn render(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut names: Vec<(String, bool)> = self
            .rt_values
            .iter()
            .map(|n| (n.clone(), false))
            .chain(
                self.rt_types
                    .iter()
                    .filter(|n| !self.rt_values.contains(*n))
                    .map(|n| (n.clone(), true)),
            )
            .collect();
        names.sort();
        if !names.is_empty() {
            out.push(import_line(&names, RUNTIME));
        }
        for (stem, (values, types)) in &self.local {
            let mut names: Vec<(String, bool)> = values
                .iter()
                .map(|n| (n.clone(), false))
                .chain(
                    types
                        .iter()
                        .filter(|n| !values.contains(*n))
                        .map(|n| (n.clone(), true)),
                )
                .collect();
            names.sort();
            if !names.is_empty() {
                out.push(import_line(&names, &format!("./{stem}.js")));
            }
        }
        out
    }
}

fn import_line(names: &[(String, bool)], from: &str) -> String {
    let items: Vec<String> = names
        .iter()
        .map(|(n, ty)| if *ty { format!("type {n}") } else { n.clone() })
        .collect();
    let single = format!("import {{ {} }} from \"{from}\";", items.join(", "));
    if single.chars().count() <= 100 {
        single
    } else {
        let mut s = String::from("import {\n");
        for i in &items {
            s.push_str(&format!("  {i},\n"));
        }
        s.push_str(&format!("}} from \"{from}\";"));
        s
    }
}

/// Per-file generation state.
struct Ctx<'a> {
    g: &'a TsGen<'a>,
    module: Module,
    imports: Imports,
    hoisted: Vec<(String, String)>,
    hoist_names: HashMap<TypeRef, String>,
    needs_decode_stream: bool,
    body: CodeWriter,
}

impl<'a> Ctx<'a> {
    fn new(g: &'a TsGen<'a>, module: Module) -> Self {
        Ctx {
            g,
            module,
            imports: Imports::default(),
            hoisted: Vec::new(),
            hoist_names: HashMap::new(),
            needs_decode_stream: false,
            body: CodeWriter::new("  "),
        }
    }

    fn model(&self) -> &'a Model {
        self.g.model
    }

    fn rt_value(&mut self, name: &str) {
        self.imports.rt_values.insert(name.to_owned());
    }

    fn rt_type(&mut self, name: &str) {
        self.imports.rt_types.insert(name.to_owned());
    }

    /// Imports the `KeelIds` namespace.
    fn ids(&mut self) {
        self.imports
            .local
            .entry("ids")
            .or_default()
            .0
            .insert("KeelIds".to_owned());
    }

    /// The module that defines the type `name`.
    fn home(&self, name: &str) -> Option<Module> {
        match self.model().kind(name)? {
            NamedKind::Record | NamedKind::UnitEnum | NamedKind::DataEnum => Some(Module::Types),
            NamedKind::Error => Some(Module::Errors),
            NamedKind::Object => None,
        }
    }

    fn local_value(&mut self, home: Module, name: &str) {
        if home != self.module {
            self.imports
                .local
                .entry(home.stem())
                .or_default()
                .0
                .insert(name.to_owned());
        }
    }

    fn local_type(&mut self, home: Module, name: &str) {
        if home != self.module {
            self.imports
                .local
                .entry(home.stem())
                .or_default()
                .1
                .insert(name.to_owned());
        }
    }

    /// Imports the type `name` for use in a type position.
    fn use_type(&mut self, name: &str) {
        if let Some(home) = self.home(name) {
            self.local_type(home, name);
        }
    }

    /// Imports the class, enum or codec value `symbol` defined next to the
    /// type `name`.
    fn use_value(&mut self, name: &str, symbol: &str) {
        if let Some(home) = self.home(name) {
            self.local_value(home, symbol);
        }
    }

    // ----- types ------------------------------------------------------------

    fn ty(&mut self, t: &TypeRef) -> String {
        match t {
            TypeRef::Bool => "boolean".to_owned(),
            TypeRef::I8
            | TypeRef::I16
            | TypeRef::I32
            | TypeRef::U8
            | TypeRef::U16
            | TypeRef::U32
            | TypeRef::F32
            | TypeRef::F64 => "number".to_owned(),
            TypeRef::I64 | TypeRef::U64 => if self.g.cfg.ts_js_number {
                "number"
            } else {
                "bigint"
            }
            .to_owned(),
            TypeRef::String | TypeRef::Uuid => "string".to_owned(),
            TypeRef::Bytes => "Uint8Array".to_owned(),
            TypeRef::Unit => "void".to_owned(),
            TypeRef::Duration => {
                self.rt_type("Duration");
                "Duration".to_owned()
            }
            TypeRef::Timestamp => {
                self.rt_type("Timestamp");
                "Timestamp".to_owned()
            }
            TypeRef::Option(inner) => format!("{} | null", self.ty(inner)),
            TypeRef::Vec(inner) => {
                let item = self.ty(inner);
                if matches!(**inner, TypeRef::Option(_)) {
                    format!("({item})[]")
                } else {
                    format!("{item}[]")
                }
            }
            TypeRef::Map(k, v) => format!("Map<{}, {}>", self.ty(k), self.ty(v)),
            TypeRef::Named(name) => {
                self.use_type(name);
                name.clone()
            }
            // Rejected by validation before generation starts.
            TypeRef::Lazy(_) | TypeRef::Result(..) | TypeRef::Stream(_) => "never".to_owned(),
        }
    }

    // ----- codecs -----------------------------------------------------------

    fn int_codec(&self, t: &TypeRef) -> &'static str {
        match (t, self.g.cfg.ts_js_number) {
            (TypeRef::I64, true) => "i64Number",
            (TypeRef::U64, true) => "u64Number",
            (TypeRef::I64, false) => "i64",
            _ => "u64",
        }
    }

    /// A `Codec<T>` expression for `t`.
    fn codec(&mut self, t: &TypeRef) -> String {
        match t {
            TypeRef::Bool => self.prim("bool"),
            TypeRef::I8 => self.prim("i8"),
            TypeRef::I16 => self.prim("i16"),
            TypeRef::I32 => self.prim("i32"),
            TypeRef::U8 => self.prim("u8"),
            TypeRef::U16 => self.prim("u16"),
            TypeRef::U32 => self.prim("u32"),
            TypeRef::I64 | TypeRef::U64 => {
                let name = self.int_codec(t);
                self.prim(name)
            }
            TypeRef::F32 => self.prim("f32"),
            TypeRef::F64 => self.prim("f64"),
            TypeRef::String => self.prim("string"),
            TypeRef::Bytes => self.prim("bytes"),
            TypeRef::Unit => self.prim("unit"),
            TypeRef::Duration => self.prim("duration"),
            TypeRef::Timestamp => self.prim("timestamp"),
            TypeRef::Uuid => self.prim("uuid"),
            TypeRef::Named(name) => {
                let symbol = format!("{name}Codec");
                self.use_value(name, &symbol);
                symbol
            }
            TypeRef::Option(_) | TypeRef::Vec(_) | TypeRef::Map(..) => {
                if !self.hoistable(t) {
                    return self.inline_codec(t);
                }
                if let Some(name) = self.hoist_names.get(t) {
                    return name.clone();
                }
                let expr = self.compose(t, false);
                let name = self.hoist_name(t);
                self.hoisted.push((name.clone(), expr));
                self.hoist_names.insert(t.clone(), name.clone());
                name
            }
            TypeRef::Lazy(_) | TypeRef::Result(..) | TypeRef::Stream(_) => "codecs.unit".to_owned(),
        }
    }

    fn prim(&mut self, name: &str) -> String {
        self.rt_value("codecs");
        format!("codecs.{name}")
    }

    fn inline_codec(&mut self, t: &TypeRef) -> String {
        match t {
            TypeRef::Option(_) | TypeRef::Vec(_) | TypeRef::Map(..) => self.compose(t, true),
            other => self.codec(other),
        }
    }

    /// `codecs.option(..)`, `codecs.vec(..)` or `codecs.map(..)` of the parts.
    fn compose(&mut self, t: &TypeRef, inline: bool) -> String {
        self.rt_value("codecs");
        let part = |cx: &mut Ctx<'a>, ty: &TypeRef| {
            if inline {
                cx.inline_codec(ty)
            } else {
                cx.codec(ty)
            }
        };
        match t {
            TypeRef::Option(i) => format!("codecs.option({})", part(self, i)),
            TypeRef::Vec(i) => format!("codecs.vec({})", part(self, i)),
            TypeRef::Map(k, v) => {
                let k = part(self, k);
                let v = part(self, v);
                format!("codecs.map({k}, {v})")
            }
            other => part(self, other),
        }
    }

    /// Whether a composite codec may be built once at module level. In the
    /// cyclic `types.ts` / `errors.ts` case a composite that names a codec of
    /// the other file must be built when it is used.
    fn hoistable(&self, t: &TypeRef) -> bool {
        if !self.g.cyclic {
            return true;
        }
        let model = self.model();
        match self.module {
            Module::Types => !mentions(t, &|n| model.kind(n) == Some(NamedKind::Error)),
            Module::Errors => !mentions(t, &|n| {
                matches!(
                    model.kind(n),
                    Some(NamedKind::Record | NamedKind::UnitEnum | NamedKind::DataEnum)
                )
            }),
            _ => true,
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
        let mut base = String::new();
        words(t, &mut base);
        let base = naming::camel(&base);
        let mut name = base.clone();
        let mut n = 2;
        while self.hoisted.iter().any(|(existing, _)| *existing == name) {
            name = format!("{base}{n}");
            n += 1;
        }
        name
    }

    // ----- statements -------------------------------------------------------

    /// A statement that writes `value` of type `t` to the writer `w`.
    fn write_stmt(&mut self, t: &TypeRef, value: &str, w: &str) -> String {
        let direct = match t {
            TypeRef::Bool => Some("writeBool"),
            TypeRef::I8 => Some("writeI8"),
            TypeRef::I16 => Some("writeI16"),
            TypeRef::I32 => Some("writeI32"),
            TypeRef::U8 => Some("writeU8"),
            TypeRef::U16 => Some("writeU16"),
            TypeRef::U32 => Some("writeU32"),
            TypeRef::I64 => Some(if self.g.cfg.ts_js_number {
                "writeI64Number"
            } else {
                "writeI64"
            }),
            TypeRef::U64 => Some(if self.g.cfg.ts_js_number {
                "writeU64Number"
            } else {
                "writeU64"
            }),
            TypeRef::F32 => Some("writeF32"),
            TypeRef::F64 => Some("writeF64"),
            TypeRef::String => Some("writeStr"),
            TypeRef::Bytes => Some("writeBytes"),
            TypeRef::Uuid => Some("writeUuid"),
            TypeRef::Timestamp => Some("writeI64Number"),
            _ => None,
        };
        match direct {
            Some(method) => format!("{w}.{method}({value});"),
            None => {
                let codec = self.codec(t);
                format!("{codec}.encode({w}, {value});")
            }
        }
    }

    /// An expression that reads a value of type `t` from the reader `r`.
    fn read_expr(&mut self, t: &TypeRef, r: &str) -> String {
        let direct = match t {
            TypeRef::Bool => Some("readBool"),
            TypeRef::I8 => Some("readI8"),
            TypeRef::I16 => Some("readI16"),
            TypeRef::I32 => Some("readI32"),
            TypeRef::U8 => Some("readU8"),
            TypeRef::U16 => Some("readU16"),
            TypeRef::U32 => Some("readU32"),
            TypeRef::I64 => Some(if self.g.cfg.ts_js_number {
                "readI64Number"
            } else {
                "readI64"
            }),
            TypeRef::U64 => Some(if self.g.cfg.ts_js_number {
                "readU64Number"
            } else {
                "readU64"
            }),
            TypeRef::F32 => Some("readF32"),
            TypeRef::F64 => Some("readF64"),
            TypeRef::String => Some("readStr"),
            TypeRef::Uuid => Some("readUuid"),
            TypeRef::Timestamp => Some("readI64Number"),
            _ => None,
        };
        match direct {
            Some(method) => format!("{r}.{method}()"),
            None => {
                let codec = self.codec(t);
                format!("{codec}.decode({r})")
            }
        }
    }

    /// An expression decoding the whole of `bytes` as a `t`.
    fn decode_all(&mut self, t: &TypeRef, bytes: &str) -> String {
        let codec = self.codec(t);
        self.rt_value("decodeValue");
        format!("decodeValue({codec}, {bytes})")
    }

    /// The zero value used as a signal's placeholder until the initial
    /// change-set arrives.
    fn zero(&mut self, t: &TypeRef) -> String {
        match t {
            TypeRef::Bool => "false".to_owned(),
            TypeRef::I64 | TypeRef::U64 if !self.g.cfg.ts_js_number => "0n".to_owned(),
            TypeRef::I8
            | TypeRef::I16
            | TypeRef::I32
            | TypeRef::I64
            | TypeRef::U8
            | TypeRef::U16
            | TypeRef::U32
            | TypeRef::U64
            | TypeRef::F32
            | TypeRef::F64
            | TypeRef::Duration
            | TypeRef::Timestamp => "0".to_owned(),
            TypeRef::String => "\"\"".to_owned(),
            TypeRef::Uuid => "\"00000000-0000-0000-0000-000000000000\"".to_owned(),
            TypeRef::Bytes => "new Uint8Array(0)".to_owned(),
            TypeRef::Option(_) => "null".to_owned(),
            TypeRef::Vec(_) => "[]".to_owned(),
            TypeRef::Map(..) => "new Map()".to_owned(),
            TypeRef::Named(name) => self.zero_named(name),
            TypeRef::Unit | TypeRef::Lazy(_) | TypeRef::Result(..) | TypeRef::Stream(_) => {
                "undefined".to_owned()
            }
        }
    }

    fn zero_named(&mut self, name: &str) -> String {
        let model = self.model();
        match model.kind(name) {
            Some(NamedKind::Record) => {
                let Some(record) = model.record(name) else {
                    return "undefined".to_owned();
                };
                let fields: Vec<String> = record
                    .fields
                    .iter()
                    .map(|f| format!("{}: {}", naming::camel(&f.name), self.zero(&f.ty)))
                    .collect();
                if fields.is_empty() {
                    "{}".to_owned()
                } else {
                    format!("{{ {} }}", fields.join(", "))
                }
            }
            Some(NamedKind::UnitEnum) => model
                .enum_def(name)
                .and_then(|e| e.variants.first())
                .map(|v| js_string(&naming::camel(&v.name)))
                .unwrap_or_else(|| "undefined".to_owned()),
            Some(NamedKind::DataEnum) => {
                let Some(variant) = model.enum_def(name).and_then(|e| e.variants.first()) else {
                    return "undefined".to_owned();
                };
                let mut parts = vec![format!(
                    "kind: {}",
                    js_string(&naming::camel(&variant.name))
                )];
                let names = self.variant_props(model.enum_def(name), variant);
                for (prop, field) in names.iter().zip(&variant.fields) {
                    parts.push(format!("{prop}: {}", self.zero(&field.ty)));
                }
                format!("{{ {} }}", parts.join(", "))
            }
            Some(NamedKind::Error) => {
                let Some(en) = model.error_def(name) else {
                    return "undefined".to_owned();
                };
                let Some(variant) = en.variants.first() else {
                    return "undefined".to_owned();
                };
                self.use_value(name, name);
                let args: Vec<String> = variant.fields.iter().map(|f| self.zero(&f.ty)).collect();
                format!(
                    "new {name}.{}({})",
                    variant_class(&variant.name),
                    args.join(", ")
                )
            }
            Some(NamedKind::Object) | None => "undefined".to_owned(),
        }
    }

    // ----- variant fields ---------------------------------------------------

    /// The property names of a variant's fields. Data enums reserve `kind`;
    /// error classes also reserve the `Error` members and use binding-safe
    /// names because the fields are constructor parameters.
    fn variant_props(&self, en: Option<&EnumDef>, v: &VariantDef) -> Vec<String> {
        let is_error = en.is_some_and(|e| e.is_error);
        let count = v.fields.len();
        let cause = is_error && self.is_cause_variant(v);
        v.fields
            .iter()
            .enumerate()
            .map(|(i, f)| {
                if cause {
                    return "cause".to_owned();
                }
                let base = if v.tuple {
                    naming::tuple_field(i, count)
                } else {
                    naming::camel(&f.name)
                };
                let base = if is_error {
                    naming::ts_ident(&base)
                } else {
                    base
                };
                let reserved: &[&str] = if is_error {
                    &["kind", "message", "name", "stack", "cause"]
                } else {
                    &["kind"]
                };
                naming::avoid(&base, reserved)
            })
            .collect()
    }

    /// A tuple variant with exactly one field that is itself an error: the
    /// field is the `cause` of the `Error`.
    fn is_cause_variant(&self, v: &VariantDef) -> bool {
        v.tuple
            && v.fields.len() == 1
            && matches!(&v.fields[0].ty, TypeRef::Named(n) if self.model().kind(n) == Some(NamedKind::Error))
    }
}

/// The class name of an error variant, kept as declared.
fn variant_class(name: &str) -> String {
    name.to_owned()
}

impl TsGen<'_> {
    fn header(&self) -> String {
        format!(
            "// Generated by keel-bindgen from the Keel schema of `{}` (schema hash 0x{:016x}). Do not edit.",
            self.model.crate_name, self.model.schema_hash
        )
    }

    /// Finishes a file: header, imports, body, hoisted codecs and helpers.
    fn assemble(&self, path: &str, mut cx: Ctx<'_>) -> GeneratedFile {
        if cx.needs_decode_stream {
            cx.rt_type("Codec");
            cx.rt_value("decodeValue");
            let w = &mut cx.body;
            w.blank();
            w.line(
                "/** Decodes every item of a core stream; a failure passes through `mapError`. */",
            );
            w.block(
                "async function* decodeStream<T>(\n  source: AsyncIterable<Uint8Array>,\n  codec: Codec<T>,\n  mapError: (error: unknown) => unknown = (error) => error,\n): AsyncGenerator<T, void, undefined>",
                |w| {
                    try_catch(
                        w,
                        |w| w.line("for await (const body of source) yield decodeValue(codec, body);"),
                        |w| w.line("throw mapError(error);"),
                    );
                },
            );
        }
        if !cx.hoisted.is_empty() {
            let w = &mut cx.body;
            w.blank();
            for (name, expr) in &cx.hoisted {
                w.line(format!("const {name} = {expr};"));
            }
        }
        let mut out = CodeWriter::new("  ");
        out.line(self.header());
        out.blank();
        let imports = cx.imports.render();
        let body = cx.body.finish();
        for i in &imports {
            out.line(i);
        }
        out.blank();
        if body.trim().is_empty() && imports.is_empty() {
            out.line("export {};");
        } else {
            out.line(body);
        }
        GeneratedFile {
            path: path.to_owned(),
            contents: out.finish(),
        }
    }

    // ===== types.ts ==========================================================

    fn types_file(&self) -> GeneratedFile {
        let mut cx = Ctx::new(self, Module::Types);
        let mut w = CodeWriter::new("  ");
        for record in &self.model.records {
            cx.record(&mut w, record);
            w.blank();
        }
        for en in &self.model.enums {
            if crate::model::is_unit_enum(en) {
                cx.unit_enum(&mut w, en);
            } else {
                cx.data_enum(&mut w, en);
            }
            w.blank();
        }
        cx.body = w;
        self.assemble("src/types.ts", cx)
    }

    // ===== errors.ts =========================================================

    fn errors_file(&self) -> GeneratedFile {
        let mut cx = Ctx::new(self, Module::Errors);
        let mut w = CodeWriter::new("  ");
        for en in &self.model.errors {
            cx.error(&mut w, en);
            w.blank();
        }
        cx.body = w;
        self.assemble("src/errors.ts", cx)
    }

    // ===== objects.ts ========================================================

    fn objects_file(&self) -> GeneratedFile {
        let mut cx = Ctx::new(self, Module::Objects);
        let mut w = CodeWriter::new("  ");
        for object in &self.model.objects {
            cx.object(&mut w, object);
            w.blank();
        }
        for function in &self.model.functions {
            cx.function(&mut w, function, "KeelIds.Functions");
            w.blank();
        }
        cx.body = w;
        self.assemble("src/objects.ts", cx)
    }

    // ===== stores.ts =========================================================

    fn stores_file(&self) -> GeneratedFile {
        let mut cx = Ctx::new(self, Module::Stores);
        let mut w = CodeWriter::new("  ");
        for store in &self.model.stores {
            cx.object(&mut w, store);
            w.blank();
        }
        cx.body = w;
        self.assemble("src/stores.ts", cx)
    }

    // ===== ports.ts ==========================================================

    fn ports_file(&self) -> GeneratedFile {
        let mut cx = Ctx::new(self, Module::Ports);
        let mut w = CodeWriter::new("  ");
        for port in &self.model.ports {
            if port.kind == PortKind::Event {
                cx.event_port(&mut w, port);
            } else {
                cx.port(&mut w, port);
            }
            w.blank();
        }
        cx.body = w;
        self.assemble("src/ports.ts", cx)
    }

    // ===== queries.ts ========================================================

    fn queries_file(&self) -> GeneratedFile {
        let mut cx = Ctx::new(self, Module::Queries);
        let mut w = CodeWriter::new("  ");
        for handle in &self.model.query_handles {
            cx.object(&mut w, handle);
            w.blank();
        }
        for mutation in &self.model.mutations {
            cx.function(&mut w, mutation, "KeelIds.Queries");
            w.blank();
        }
        cx.body = w;
        self.assemble("src/queries.ts", cx)
    }

    // ===== ids.ts ============================================================

    fn ids_file(&self) -> GeneratedFile {
        let m = self.model;
        let mut w = CodeWriter::new("  ");
        w.line("/**");
        w.line(" * Stable wire identifiers (SPEC section 1.1), for logs and debugging, plus the schema");
        w.line(" * hash to pass to `KeelCore.load` as `expectedSchemaHash`.");
        w.line(" */");
        w.block_with("export const KeelIds = {", "} as const;", |w| {
            w.line(format!("schemaHash: 0x{:016x}n,", m.schema_hash));
            w.block_with("Objects: {", "},", |w| {
                for o in m.all_objects() {
                    w.block_with(format!("{}: {{", o.name), "},", |w| {
                        w.line(format!("typeId: {},", hex(o.type_id)));
                        for method in o.constructors.iter().chain(&o.methods) {
                            w.line(format!(
                                "{}: {},",
                                naming::ts_member(&naming::camel(&method.name)),
                                hex(method.method_id)
                            ));
                        }
                    });
                }
            });
            w.block_with("Functions: {", "},", |w| {
                for f in &m.functions {
                    w.line(format!(
                        "{}: {},",
                        naming::ts_member(&naming::camel(&f.name)),
                        hex(f.method_id)
                    ));
                }
            });
            w.block_with("Ports: {", "},", |w| {
                for p in &m.ports {
                    w.block_with(format!("{}: {{", p.name), "},", |w| {
                        w.line(format!("portId: {},", hex(p.port_id)));
                        for method in &p.methods {
                            w.line(format!(
                                "{}: {},",
                                naming::ts_member(&naming::camel(&method.name)),
                                hex(method.method_id)
                            ));
                        }
                    });
                }
            });
            w.block_with("Queries: {", "},", |w| {
                for q in &m.queries {
                    w.line(format!(
                        "{}: {},",
                        naming::ts_member(&naming::camel(&q.name)),
                        hex(q.query_id)
                    ));
                }
            });
        });
        GeneratedFile {
            path: "src/ids.ts".to_owned(),
            contents: format!("{}\n\n{}", self.header(), w.finish()),
        }
    }

    // ===== index.ts / package.json ===========================================

    fn index_file(&self) -> GeneratedFile {
        let mut w = CodeWriter::new("  ");
        w.line(self.header());
        w.blank();
        for module in [
            Module::Types,
            Module::Errors,
            Module::Objects,
            Module::Stores,
            Module::Ports,
            Module::Queries,
        ] {
            w.line(format!("export * from \"./{}.js\";", module.stem()));
        }
        w.line("export * from \"./ids.js\";");
        GeneratedFile {
            path: "src/index.ts".to_owned(),
            contents: w.finish(),
        }
    }

    fn package_json(&self) -> String {
        let name = self.cfg.ts_package_name();
        let description = format!("Keel bindings for `{}`.", self.model.crate_name);
        let json = serde_json::json!({
            "name": name,
            "version": self.cfg.package_version,
            "description": description,
            "type": "module",
            "sideEffects": false,
            "exports": {
                ".": {
                    "types": "./dist/index.d.ts",
                    "default": "./dist/index.js"
                }
            },
            "main": "./dist/index.js",
            "types": "./dist/index.d.ts",
            "files": ["dist", "src"],
            "scripts": {
                "build": "tsc -p tsconfig.json",
                "typecheck": "tsc -p tsconfig.json --noEmit"
            },
            "peerDependencies": {
                "@keel/runtime": "^0.1.0"
            },
            "devDependencies": {
                "@keel/runtime": "^0.1.0",
                "typescript": "^5.5.0"
            }
        });
        // Serializing a `json!` value cannot fail.
        let mut text = serde_json::to_string_pretty(&json).unwrap_or_default();
        text.push('\n');
        text
    }
}

// ===== declarations ===========================================================

impl<'a> Ctx<'a> {
    fn record(&mut self, w: &mut CodeWriter, r: &RecordDef) {
        jsdoc(w, &r.docs, &[]);
        w.block(format!("export interface {}", r.name), |w| {
            for f in &r.fields {
                jsdoc(w, &f.docs, &[]);
                let ty = self.ty(&f.ty);
                w.line(format!("{}: {ty};", naming::camel(&f.name)));
            }
        });
        w.blank();
        self.rt_type("Codec");
        let empty = r.fields.is_empty();
        let (wv, vv, rv) = if empty {
            ("_w", "_v", "_r")
        } else {
            ("w", "v", "r")
        };
        w.block_with(
            format!("export const {0}Codec: Codec<{0}> = {{", r.name),
            "};",
            |w| {
                w.block_with(format!("encode({wv}, {vv}) {{"), "},", |w| {
                    for f in &r.fields {
                        let value = format!("v.{}", naming::camel(&f.name));
                        w.line(self.write_stmt(&f.ty, &value, "w"));
                    }
                });
                w.block_with(format!("decode({rv}) {{"), "},", |w| {
                    if empty {
                        w.line("return {};");
                    } else {
                        w.line("return {");
                        w.indented(|w| {
                            for f in &r.fields {
                                let expr = self.read_expr(&f.ty, "r");
                                w.line(format!("{}: {expr},", naming::camel(&f.name)));
                            }
                        });
                        w.line("};");
                    }
                });
            },
        );
    }

    fn unit_enum(&mut self, w: &mut CodeWriter, en: &EnumDef) {
        jsdoc(w, &en.docs, &[]);
        let literals: Vec<String> = en
            .variants
            .iter()
            .map(|v| js_string(&naming::camel(&v.name)))
            .collect();
        let has_docs = en.variants.iter().any(|v| !v.docs.is_empty());
        let single = format!("export type {} = {};", en.name, literals.join(" | "));
        if !has_docs && single.chars().count() <= 100 {
            w.line(single);
        } else {
            w.line(format!("export type {} =", en.name));
            w.indented(|w| {
                for (i, v) in en.variants.iter().enumerate() {
                    jsdoc(w, &v.docs, &[]);
                    let end = if i + 1 == en.variants.len() { ";" } else { "" };
                    w.line(format!("| {}{end}", literals[i]));
                }
            });
        }
        w.blank();
        self.rt_type("Codec");
        self.rt_value("WireError");
        w.block_with(
            format!("export const {0}Codec: Codec<{0}> = {{", en.name),
            "};",
            |w| {
                w.block_with("encode(w, v) {", "},", |w| {
                    w.block("switch (v)", |w| {
                        for (v, lit) in en.variants.iter().zip(&literals) {
                            w.line(format!("case {lit}:"));
                            w.indented(|w| {
                                w.line(format!("w.writeU16({});", v.index));
                                w.line("break;");
                            });
                        }
                    });
                });
                w.block_with("decode(r) {", "},", |w| {
                    w.line("const at = r.position;");
                    w.line("const tag = r.readU16();");
                    w.block("switch (tag)", |w| {
                        for (v, lit) in en.variants.iter().zip(&literals) {
                            w.line(format!("case {}:", v.index));
                            w.indented(|w| w.line(format!("return {lit};")));
                        }
                        w.line("default:");
                        w.indented(|w| {
                            w.line(format!(
                                "throw new WireError({{ code: \"invalid_tag\", tag, at, ty: {} }});",
                                js_string(&en.name)
                            ));
                        });
                    });
                });
            },
        );
    }

    fn data_enum(&mut self, w: &mut CodeWriter, en: &EnumDef) {
        jsdoc(w, &en.docs, &[]);
        w.line(format!("export type {} =", en.name));
        let last = en.variants.len().saturating_sub(1);
        w.indented(|w| {
            for (i, v) in en.variants.iter().enumerate() {
                jsdoc(w, &v.docs, &[]);
                let props = self.variant_props(Some(en), v);
                let mut members = vec![format!("kind: {}", js_string(&naming::camel(&v.name)))];
                for (prop, f) in props.iter().zip(&v.fields) {
                    members.push(format!("{prop}: {}", self.ty(&f.ty)));
                }
                let end = if i == last { ";" } else { "" };
                w.line(format!("| {{ {} }}{end}", members.join("; ")));
            }
        });
        w.blank();
        self.rt_type("Codec");
        self.rt_value("WireError");
        w.block_with(
            format!("export const {0}Codec: Codec<{0}> = {{", en.name),
            "};",
            |w| {
                w.block_with("encode(w, v) {", "},", |w| {
                    w.block("switch (v.kind)", |w| {
                        for v_def in &en.variants {
                            let props = self.variant_props(Some(en), v_def);
                            w.line(format!("case {}:", js_string(&naming::camel(&v_def.name))));
                            w.indented(|w| {
                                w.line(format!("w.writeU16({});", v_def.index));
                                for (prop, f) in props.iter().zip(&v_def.fields) {
                                    let stmt = self.write_stmt(&f.ty, &format!("v.{prop}"), "w");
                                    w.line(stmt);
                                }
                                w.line("break;");
                            });
                        }
                    });
                });
                w.block_with("decode(r) {", "},", |w| {
                    w.line("const at = r.position;");
                    w.line("const tag = r.readU16();");
                    w.block("switch (tag)", |w| {
                        for v_def in &en.variants {
                            let props = self.variant_props(Some(en), v_def);
                            w.line(format!("case {}:", v_def.index));
                            w.indented(|w| {
                                let kind = js_string(&naming::camel(&v_def.name));
                                if v_def.fields.is_empty() {
                                    w.line(format!("return {{ kind: {kind} }};"));
                                } else {
                                    w.line("return {");
                                    w.indented(|w| {
                                        w.line(format!("kind: {kind},"));
                                        for (prop, f) in props.iter().zip(&v_def.fields) {
                                            let expr = self.read_expr(&f.ty, "r");
                                            w.line(format!("{prop}: {expr},"));
                                        }
                                    });
                                    w.line("};");
                                }
                            });
                        }
                        w.line("default:");
                        w.indented(|w| {
                            w.line(format!(
                                "throw new WireError({{ code: \"invalid_tag\", tag, at, ty: {} }});",
                                js_string(&en.name)
                            ));
                        });
                    });
                });
            },
        );
    }

    // ----- errors -------------------------------------------------------------

    /// The `super(..)` message argument of an error variant.
    fn error_message(&mut self, v: &VariantDef, props: &[String]) -> String {
        let Some(template) = &v.message else {
            // `#[error(transparent)]`: the wrapped error's own message.
            return match (props.first(), v.fields.first()) {
                (Some(prop), Some(f)) if self.is_error_type(&f.ty) => format!("{prop}.message"),
                (Some(prop), _) => format!("String({prop})"),
                _ => "\"\"".to_owned(),
            };
        };
        let parts = parse_message(v, template).unwrap_or_default();
        if parts.iter().all(|p| matches!(p, MsgPart::Text(_))) {
            let text: String = parts
                .iter()
                .map(|p| match p {
                    MsgPart::Text(t) => t.as_str(),
                    MsgPart::Field(_) => "",
                })
                .collect();
            return js_string(&text);
        }
        let mut out = String::from("`");
        for part in &parts {
            match part {
                MsgPart::Text(t) => out.push_str(&template_text(t)),
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
        out.push('`');
        out
    }

    fn is_error_type(&self, t: &TypeRef) -> bool {
        matches!(t, TypeRef::Named(n) if self.model().kind(n) == Some(NamedKind::Error))
    }

    fn error(&mut self, w: &mut CodeWriter, en: &EnumDef) {
        let name = &en.name;
        self.rt_value("KeelError");
        self.rt_value("KeelReplyError");
        self.rt_value("ReplyStatus");
        self.rt_value("decodeValue");
        let kinds: Vec<String> = en
            .variants
            .iter()
            .map(|v| js_string(&naming::camel(&v.name)))
            .collect();
        w.line(format!("export type {name}Kind = {};", kinds.join(" | ")));
        w.blank();
        jsdoc(w, &en.docs, &[]);
        w.block(format!("export abstract class {name} extends KeelError"), |w| {
            w.line(format!("declare readonly kind: {name}Kind;"));
            w.blank();
            jsdoc(
                w,
                "The typed error a failed call carries; any other failure is returned unchanged.",
                &[],
            );
            w.block("static fromReply(error: unknown): unknown", |w| {
                w.block(
                    "if (error instanceof KeelReplyError && error.status === ReplyStatus.Error)",
                    |w| w.line(format!("return decodeValue({name}Codec, error.body);")),
                );
                w.line("return error;");
            });
        });
        w.blank();
        w.block(format!("export namespace {name}"), |w| {
            for (i, v) in en.variants.iter().enumerate() {
                if i > 0 {
                    w.blank();
                }
                self.error_variant(w, en, v);
            }
        });
        w.blank();
        self.rt_type("Codec");
        self.rt_value("WireError");
        w.block_with(
            format!("export const {name}Codec: Codec<{name}> = {{"),
            "};",
            |w| {
                w.block_with("encode(w, v) {", "},", |w| {
                    for (i, v) in en.variants.iter().enumerate() {
                        let props = self.variant_props(Some(en), v);
                        let head = if i == 0 { "if" } else { "} else if" };
                        w.line(format!(
                            "{head} (v instanceof {name}.{}) {{",
                            variant_class(&v.name)
                        ));
                        w.indented(|w| {
                            w.line(format!("w.writeU16({});", v.index));
                            for (prop, f) in props.iter().zip(&v.fields) {
                                let stmt = self.write_stmt(&f.ty, &format!("v.{prop}"), "w");
                                w.line(stmt);
                            }
                        });
                    }
                    w.line("} else {");
                    w.indented(|w| {
                        w.line(format!(
                            "throw new TypeError(`unknown {name} variant: ${{v.kind}}`);"
                        ));
                    });
                    w.line("}");
                });
                w.block_with("decode(r) {", "},", |w| {
                    w.line("const at = r.position;");
                    w.line("const tag = r.readU16();");
                    w.block("switch (tag)", |w| {
                        for v in &en.variants {
                            let args: Vec<String> = v
                                .fields
                                .iter()
                                .map(|f| self.read_expr(&f.ty, "r"))
                                .collect();
                            w.line(format!("case {}:", v.index));
                            w.indented(|w| {
                                w.line(format!(
                                    "return new {name}.{}({});",
                                    variant_class(&v.name),
                                    args.join(", ")
                                ));
                            });
                        }
                        w.line("default:");
                        w.indented(|w| {
                            w.line(format!(
                                "throw new WireError({{ code: \"invalid_tag\", tag, at, ty: {} }});",
                                js_string(name)
                            ));
                        });
                    });
                });
            },
        );
    }

    fn error_variant(&mut self, w: &mut CodeWriter, en: &EnumDef, v: &VariantDef) {
        let props = self.variant_props(Some(en), v);
        let message = self.error_message(v, &props);
        let kind = js_string(&naming::camel(&v.name));
        let cause = self.is_cause_variant(v);
        jsdoc(w, &v.docs, &[]);
        w.block(
            format!(
                "export class {} extends {}",
                variant_class(&v.name),
                en.name
            ),
            |w| {
                w.line(format!("declare readonly kind: {kind};"));
                if cause {
                    let ty = self.ty(&v.fields[0].ty);
                    w.line(format!("declare readonly cause: {ty};"));
                }
                w.blank();
                if v.fields.is_empty() {
                    w.block("constructor()", |w| {
                        w.line(format!("super({kind}, {message});"));
                    });
                } else if cause {
                    let ty = self.ty(&v.fields[0].ty);
                    w.block(format!("constructor(cause: {ty})"), |w| {
                        w.line(format!("super({kind}, {message}, {{ cause }});"));
                    });
                } else {
                    let params: Vec<String> = props
                        .iter()
                        .zip(&v.fields)
                        .map(|(prop, f)| format!("readonly {prop}: {}", self.ty(&f.ty)))
                        .collect();
                    let head = format!("constructor({})", params.join(", "));
                    if head.chars().count() + 4 <= 100 {
                        w.block(head, |w| w.line(format!("super({kind}, {message});")));
                    } else {
                        w.line("constructor(");
                        w.indented(|w| {
                            for p in &params {
                                w.line(format!("{p},"));
                            }
                        });
                        w.line(") {");
                        w.indented(|w| w.line(format!("super({kind}, {message});")));
                        w.line("}");
                    }
                }
            },
        );
    }

    // ----- objects --------------------------------------------------------------

    fn param_list(&mut self, params: &[ParamDef]) -> Vec<String> {
        params
            .iter()
            .map(|p| {
                let ty = self.ty(&p.ty);
                format!("{}: {ty}", param_ident(&p.name))
            })
            .collect()
    }

    /// Writes `const w = new KeelWriter(); w.writeX(..);` for `params` and
    /// returns the expression holding the encoded arguments.
    fn encode_args(&mut self, w: &mut CodeWriter, params: &[ParamDef], writer: &str) -> String {
        if params.is_empty() {
            return "new Uint8Array(0)".to_owned();
        }
        self.rt_value("KeelWriter");
        w.line(format!("const {writer} = new KeelWriter();"));
        for p in params {
            let stmt = self.write_stmt(&p.ty, &param_ident(&p.name), writer);
            w.line(stmt);
        }
        format!("{writer}.finish()")
    }

    fn object(&mut self, w: &mut CodeWriter, o: &ObjectDef) {
        self.ids();
        let is_store = o.store.is_some();
        let base = if is_store { "KeelStore" } else { "KeelObject" };
        self.rt_value(base);
        self.rt_value("KeelCore");
        jsdoc(w, &o.docs, &[]);
        let signals: Vec<&SignalDef> = o.store.iter().flat_map(|s| s.signals.iter()).collect();
        w.block(format!("export class {} extends {base}", o.name), |w| {
            for g in &signals {
                let ty = self.ty(&g.ty);
                let zero = self.zero(&g.ty);
                self.rt_value("Signal");
                if let Some(doc) = self.model().signal_doc(o, g) {
                    jsdoc(w, doc, &[]);
                }
                w.line(format!(
                    "readonly {}: Signal<{ty}> = new Signal<{ty}>({zero});",
                    signal_prop(g)
                ));
            }
            if !signals.is_empty() {
                w.blank();
            }
            w.block("private constructor(core: KeelCore, handle: bigint)", |w| {
                w.line("super(core, handle);");
                if !signals.is_empty() {
                    let list: Vec<String> = signals
                        .iter()
                        .map(|g| format!("this.{}", signal_prop(g)))
                        .collect();
                    array_assignment(w, "this._signals", &list);
                }
            });
            for c in &o.constructors {
                w.blank();
                self.constructor(w, o, c, is_store);
            }
            for m in &o.methods {
                w.blank();
                let id = format!(
                    "KeelIds.Objects.{}.{}",
                    o.name,
                    naming::ts_member(&naming::camel(&m.name))
                );
                self.callable(w, &Callable::from_method(m), &Site::Method { id });
            }
            if is_store {
                w.blank();
                self.store_apply(w, &signals);
            }
        });
    }

    fn constructor(&mut self, w: &mut CodeWriter, o: &ObjectDef, c: &MethodDef, is_store: bool) {
        let name = if c.name == "new" {
            "create".to_owned()
        } else {
            naming::ts_member(&naming::camel(&c.name))
        };
        let ret = Ret::classify(&c.returns);
        let err = ret.as_ref().and_then(Ret::error).map(str::to_owned);
        let taken: Vec<String> = c.params.iter().map(|p| param_ident(&p.name)).collect();
        let taken_refs: Vec<&str> = taken.iter().map(String::as_str).collect();
        let core = naming::avoid("core", &taken_refs);
        let writer = naming::avoid("w", &taken_refs);
        let handle = naming::avoid("handle", &taken_refs);
        let store = naming::avoid("store", &taken_refs);
        let mut params = self.param_list(&c.params);
        params.push(format!("{core}: KeelCore = KeelCore.shared"));
        let mut extra = Vec::new();
        if let Some(err) = &err {
            extra.push(format!("@throws {{{err}}}"));
        }
        jsdoc(w, &c.docs, &extra);
        let prefix = format!("static async {name}");
        let suffix = format!(": Promise<{}>", o.name);
        w.call_block(prefix, &params, suffix, true, |w| {
            let args = self.encode_args(w, &c.params, &writer);
            let construct_args = [
                format!("KeelIds.Objects.{}.typeId", o.name),
                format!(
                    "KeelIds.Objects.{}.{}",
                    o.name,
                    naming::ts_member(&naming::camel(&c.name))
                ),
                args,
            ];
            let construct = format!("await {core}.construct");
            if let Some(err) = &err {
                self.use_value(err, err);
                w.line(format!("let {handle}: bigint;"));
                try_catch(
                    w,
                    |w| {
                        w.call(
                            format!("{handle} = {construct}"),
                            &construct_args,
                            ";",
                            true,
                        )
                    },
                    |w| w.line(format!("throw {err}.fromReply(error);")),
                );
            } else {
                w.call(
                    format!("const {handle} = {construct}"),
                    &construct_args,
                    ";",
                    true,
                );
            }
            if is_store {
                self.rt_value("ALL_SIGNALS");
                w.line(format!("const {store} = new {}({core}, {handle});", o.name));
                w.line(format!(
                    "await {core}.observe({handle}, ALL_SIGNALS, true);"
                ));
                w.line(format!("return {store};"));
            } else {
                w.line(format!("return new {}({core}, {handle});", o.name));
            }
        });
    }

    fn function(&mut self, w: &mut CodeWriter, f: &FunctionDef, ids: &str) {
        self.ids();
        let id = format!("{ids}.{}", naming::ts_member(&naming::camel(&f.name)));
        self.callable(w, &Callable::from_function(f), &Site::Function { id });
    }

    /// One method or free function.
    fn callable(&mut self, w: &mut CodeWriter, c: &Callable<'_>, site: &Site) {
        let ret = Ret::classify(c.returns).unwrap_or(Ret::Plain(c.returns));
        let taken: Vec<String> = c.params.iter().map(|p| param_ident(&p.name)).collect();
        let taken_refs: Vec<&str> = taken.iter().map(String::as_str).collect();
        let writer = naming::avoid("w", &taken_refs);
        let body_var = naming::avoid("body", &taken_refs);
        let signal = naming::avoid("signal", &taken_refs);
        let source = naming::avoid("source", &taken_refs);
        let (core, target, id, prefix, is_function) = match site {
            Site::Method { id } => (
                "this.core".to_owned(),
                "{ target: CallTarget.ObjectMethod, handle: this.handle }".to_owned(),
                id.clone(),
                "",
                false,
            ),
            Site::Function { id } => (
                naming::avoid("core", &taken_refs),
                "{ target: CallTarget.FreeFunction }".to_owned(),
                id.clone(),
                "export ",
                true,
            ),
        };
        self.rt_value("CallTarget");

        let mut params = self.param_list(c.params);
        if is_function {
            self.rt_value("KeelCore");
            params.push(format!("{core}: KeelCore = KeelCore.shared"));
        }
        let is_stream = ret.is_stream();
        if c.is_async && !is_stream {
            params.push(format!("{signal}?: AbortSignal"));
        }
        let err = ret.error().map(str::to_owned);
        let mut extra = Vec::new();
        if let Some(err) = &err {
            extra.push(format!("@throws {{{err}}}"));
        }
        jsdoc(w, c.docs, &extra);

        let name = if is_function {
            naming::ts_ident(&naming::camel(c.name))
        } else {
            naming::ts_member(&naming::camel(c.name))
        };
        let function_kw = if is_function { "function " } else { "" };

        if let Ret::Stream(item) | Ret::ResultStream { item, .. } = &ret {
            let item_ty = self.ty(item);
            let head = format!("{prefix}{function_kw}{name}");
            let suffix = format!(": AsyncIterable<{item_ty}>");
            w.call_block(head, &params, suffix, true, |w| {
                let args = self.encode_args(w, c.params, &writer);
                self.needs_decode_stream = true;
                let codec = self.codec(item);
                w.call(
                    format!("const {source} = {core}.stream"),
                    &[target.clone(), id.clone(), args],
                    ";",
                    true,
                );
                let mut call_args = vec![source.clone(), codec];
                if let Some(err) = &err {
                    self.use_value(err, err);
                    call_args.push(format!("(error) => {err}.fromReply(error)"));
                }
                w.call("return decodeStream", &call_args, ";", true);
            });
            return;
        }

        let (ok_ty, is_unit) = match &ret {
            Ret::Plain(t) | Ret::Result { ok: t, .. } => (self.ty(t), matches!(t, TypeRef::Unit)),
            Ret::Stream(_) | Ret::ResultStream { .. } => ("void".to_owned(), true),
        };
        let head = format!("{prefix}async {function_kw}{name}");
        let suffix = format!(": Promise<{ok_ty}>");
        w.call_block(head, &params, suffix, true, |w| {
            let args = self.encode_args(w, c.params, &writer);
            let mut call_args = vec![target.clone(), id.clone(), args];
            if c.is_async {
                call_args.push(signal.clone());
            }
            let call = format!("await {core}.call");
            if let Some(err) = &err {
                self.use_value(err, err);
                let assign = if is_unit {
                    call.clone()
                } else {
                    w.line(format!("let {body_var}: Uint8Array;"));
                    format!("{body_var} = {call}")
                };
                try_catch(
                    w,
                    |w| w.call(assign, &call_args, ";", true),
                    |w| w.line(format!("throw {err}.fromReply(error);")),
                );
                if let Ret::Result { ok, .. } = &ret {
                    if !is_unit {
                        let expr = self.decode_all(ok, &body_var);
                        w.line(format!("return {expr};"));
                    }
                }
            } else if is_unit {
                w.call(call, &call_args, ";", true);
            } else if let Ret::Plain(t) = &ret {
                w.call(format!("const {body_var} = {call}"), &call_args, ";", true);
                let expr = self.decode_all(t, &body_var);
                w.line(format!("return {expr};"));
            }
        });
    }

    // ----- stores ----------------------------------------------------------------

    /// `_apply`: decodes full values and applies keyed patches per signal.
    fn store_apply(&mut self, w: &mut CodeWriter, signals: &[&SignalDef]) {
        self.rt_value("ChangeOp");
        let keyed = signals
            .iter()
            .any(|g| g.key.is_some() && matches!(g.ty, TypeRef::Vec(_)));
        w.block(
            "protected override _apply(signalId: number, op: ChangeOp, value: Uint8Array): void",
            |w| {
                w.block("switch (signalId)", |w| {
                    for g in signals {
                        let prop = format!("this.{}", signal_prop(g));
                        w.line(format!("case {}:", g.signal_id));
                        w.indented(|w| {
                            let full = self.decode_all(&g.ty, "value");
                            w.line("if (op === ChangeOp.FullValue) {");
                            w.indented(|w| w.line(format!("{prop}._set({full});")));
                            if let (Some(_), TypeRef::Vec(item)) = (&g.key, &g.ty) {
                                self.rt_value("KeelReader");
                                self.rt_value("decodePatch");
                                self.rt_value("applyPatch");
                                self.rt_value("PatchError");
                                let codec = self.codec(item);
                                w.line("} else if (op === ChangeOp.KeyedPatch) {");
                                w.indented(|w| {
                                    w.line("const r = new KeelReader(value);");
                                    w.line(format!("const ops = decodePatch(r, {codec});"));
                                    w.line("r.finish();");
                                    try_catch(
                                        w,
                                        |w| {
                                            w.line(format!(
                                                "{prop}._set(applyPatch({prop}.peek(), ops));"
                                            ));
                                        },
                                        |w| {
                                            w.line(
                                                "if (!(error instanceof PatchError)) throw error;",
                                            );
                                            w.line(format!("this._resync({});", g.signal_id));
                                        },
                                    );
                                });
                            }
                            w.line("}");
                            w.line("break;");
                        });
                    }
                    w.line("default:");
                    w.indented(|w| w.line("break;"));
                });
            },
        );
        if keyed {
            w.blank();
            jsdoc(
                w,
                "Re-observes a signal whose mirror diverged from the core, to receive a full value.",
                &[],
            );
            w.block("private _resync(signalId: number): void", |w| {
                w.line("void this.core.observe(this.handle, signalId, false);");
                w.line("void this.core.observe(this.handle, signalId, true);");
            });
        }
    }

    // ----- ports -----------------------------------------------------------------

    fn port(&mut self, w: &mut CodeWriter, p: &PortDef) {
        self.ids();
        let sync = p.kind == PortKind::Sync;
        self.rt_type("KeelPort");
        jsdoc(w, &p.docs, &[]);
        w.block(
            format!("export interface {} extends KeelPort", p.name),
            |w| {
                for m in &p.methods {
                    let ret = Ret::classify(&m.returns).unwrap_or(Ret::Plain(&m.returns));
                    let ok = match &ret {
                        Ret::Plain(t) | Ret::Result { ok: t, .. } => self.ty(t),
                        _ => "void".to_owned(),
                    };
                    let ty = if m.is_async {
                        format!("Promise<{ok}>")
                    } else {
                        ok
                    };
                    let mut extra = Vec::new();
                    if let Some(err) = ret.error() {
                        extra.push(format!("@throws {{{err}}}"));
                    }
                    jsdoc(w, &m.docs, &extra);
                    let params = self.param_list(&m.params);
                    w.call(
                        naming::ts_member(&naming::camel(&m.name)),
                        &params,
                        format!(": {ty};"),
                        true,
                    );
                }
            },
        );
        w.blank();
        self.rt_type("PortImpl");
        jsdoc(
            w,
            &format!(
                "Adapts an implementation of `{0}` to `KeelCore.registerPort(KeelIds.Ports.{0}.portId, ..)`.",
                p.name
            ),
            &[],
        );
        let fn_name = format!("{}PortImpl", naming::camel(&p.name));
        w.block(
            format!("export function {fn_name}(impl: {}): PortImpl", p.name),
            |w| {
                w.line("return {");
                w.indented(|w| {
                    w.line(format!("sync: {sync},"));
                    w.line("methods: {");
                    w.indented(|w| {
                        for m in &p.methods {
                            self.port_method(w, p, m, sync);
                        }
                    });
                    w.line("},");
                });
                w.line("};");
            },
        );
    }

    fn port_method(&mut self, w: &mut CodeWriter, p: &PortDef, m: &MethodDef, sync: bool) {
        let ret = Ret::classify(&m.returns).unwrap_or(Ret::Plain(&m.returns));
        let member = naming::ts_member(&naming::camel(&m.name));
        let key = format!("[KeelIds.Ports.{}.{member}]", p.name);
        let asyncw = if sync { "" } else { "async " };
        let args_param = if m.params.is_empty() { "()" } else { "(args)" };
        let taken: Vec<String> = m.params.iter().map(|a| param_ident(&a.name)).collect();
        let idents: Vec<String> = taken
            .iter()
            .map(|n| {
                naming::avoid(
                    n,
                    &["r", "args", "impl", "error", "result", "KeelPortError"],
                )
            })
            .collect();
        w.block_with(format!("{key}: {asyncw}{args_param} => {{"), "},", |w| {
            if !m.params.is_empty() {
                self.rt_value("KeelReader");
                w.line("const r = new KeelReader(args);");
                for (a, ident) in m.params.iter().zip(&idents) {
                    let expr = self.read_expr(&a.ty, "r");
                    w.line(format!("const {ident} = {expr};"));
                }
                w.line("r.finish();");
            }
            let call = format!(
                "{}impl.{member}({})",
                if m.is_async { "await " } else { "" },
                idents.join(", ")
            );
            self.rt_value("encodeValue");
            let encode_result = |cx: &mut Ctx<'a>, w: &mut CodeWriter, ok: &TypeRef| {
                if matches!(ok, TypeRef::Unit) {
                    w.line(format!("{call};"));
                    w.line("return new Uint8Array(0);");
                } else {
                    let codec = cx.codec(ok);
                    w.line(format!("return encodeValue({codec}, {call});"));
                }
            };
            match &ret {
                Ret::Result { ok, err } => {
                    self.rt_value("KeelPortError");
                    self.use_value(err, err);
                    let err_codec = self.codec(&TypeRef::named(*err));
                    w.line("try {");
                    w.indented(|w| encode_result(self, w, ok));
                    w.line("} catch (error) {");
                    w.indented(|w| {
                        w.block(format!("if (error instanceof {err})"), |w| {
                            w.line(format!(
                                "throw new KeelPortError(encodeValue({err_codec}, error));"
                            ));
                        });
                        w.line("throw error;");
                    });
                    w.line("}");
                }
                Ret::Plain(t) => encode_result(self, w, t),
                Ret::Stream(_) | Ret::ResultStream { .. } => {}
            }
        });
    }

    fn event_port(&mut self, w: &mut CodeWriter, p: &PortDef) {
        self.ids();
        self.rt_value("KeelCore");
        jsdoc(
            w,
            &p.docs,
            &["Sends the events of this port from the host to the core.".to_owned()],
        );
        w.block(format!("export class {}Events", p.name), |w| {
            w.line("constructor(private readonly core: KeelCore = KeelCore.shared) {}");
            for m in &p.methods {
                w.blank();
                jsdoc(w, &m.docs, &[]);
                let params = self.param_list(&m.params);
                let member = naming::ts_member(&naming::camel(&m.name));
                let taken: Vec<String> = m.params.iter().map(|a| param_ident(&a.name)).collect();
                let taken_refs: Vec<&str> = taken.iter().map(String::as_str).collect();
                let writer = naming::avoid("w", &taken_refs);
                w.block(format!("{member}({}): void", params.join(", ")), |w| {
                    let args = self.encode_args(w, &m.params, &writer);
                    w.call(
                        "this.core.event",
                        &[
                            format!("KeelIds.Ports.{}.portId", p.name),
                            format!("KeelIds.Ports.{}.{member}", p.name),
                            args,
                        ],
                        ";",
                        true,
                    );
                });
            }
        });
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
    /// A method of the generated class; `id` is the TypeScript expression of
    /// its method id.
    Method { id: String },
    /// A top-level function.
    Function { id: String },
}

fn param_ident(name: &str) -> String {
    naming::ts_ident(&naming::camel(name))
}

fn signal_prop(g: &SignalDef) -> String {
    naming::ts_member(&naming::camel(&g.name))
}

/// `try { .. } catch (error) { .. }`.
fn try_catch(
    w: &mut CodeWriter,
    try_body: impl FnOnce(&mut CodeWriter),
    catch_body: impl FnOnce(&mut CodeWriter),
) {
    w.line("try {");
    w.indented(try_body);
    w.line("} catch (error) {");
    w.indented(catch_body);
    w.line("}");
}

/// `target = [a, b, c];`, one item per line when it does not fit on one.
fn array_assignment(w: &mut CodeWriter, target: &str, items: &[String]) {
    let single = format!("{target} = [{}];", items.join(", "));
    if single.chars().count() + 4 <= crate::emit::MAX_WIDTH {
        w.line(single);
    } else {
        w.line(format!("{target} = ["));
        w.indented(|w| {
            for item in items {
                w.line(format!("{item},"));
            }
        });
        w.line("];");
    }
}
