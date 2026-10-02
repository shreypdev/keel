//! Schemas of the golden cases and small builders to write them with.
//!
//! `tests/golden/<case>/schema.json` is the input of a case. It is written
//! from the builders below when a case is regenerated with
//! `UPDATE_GOLDEN=1`, and `schema_json_matches_builders` fails if the two
//! drift, so the JSON files never go stale.

#![allow(dead_code)]

use undra_bindgen::Generator;
use undra_meta::{
    EnumDef, FieldDef, FunctionDef, InfiniteDef, MethodDef, ObjectDef, ParamDef, PortDef, PortKind,
    QueryDef, QueryKind, RecordDef, Schema, SignalDef, StoreDef, TypeRef, VariantDef, ids,
};

// ----- toolchain helpers -----------------------------------------------------------

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// `crates/undra-bindgen`.
pub fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The repository root.
pub fn repo_root() -> PathBuf {
    manifest_dir().join("../..")
}

/// A fresh, empty directory below the target directory.
pub fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Reports a skipped toolchain test; a failure when
/// `UNDRA_REQUIRE_TOOLCHAINS=1` (CI sets it).
pub fn skip(what: &str) {
    if std::env::var("UNDRA_REQUIRE_TOOLCHAINS").is_ok_and(|v| v == "1") {
        panic!("{what} (UNDRA_REQUIRE_TOOLCHAINS=1)");
    }
    eprintln!("skipping: {what}");
}

/// Whether `program --version` runs (`kotlinc` 2.x only knows `-version`).
pub fn on_path(program: &str) -> bool {
    ["--version", "-version"].iter().any(|flag| {
        Command::new(program)
            .arg(flag)
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

/// A `tsc` invocation: `tsc` on the path, else a locally installed one.
pub fn tsc() -> Option<Command> {
    if on_path("tsc") {
        return Some(Command::new("tsc"));
    }
    if on_path("npx") {
        let mut probe = Command::new("npx");
        probe.args(["--no-install", "tsc", "--version"]);
        if probe.output().is_ok_and(|o| o.status.success()) {
            let mut cmd = Command::new("npx");
            cmd.args(["--no-install", "tsc"]);
            return Some(cmd);
        }
    }
    None
}

/// Builds the TypeScript runtime with `extra` compiler flags into `out`.
fn build_ts_runtime(out: &Path, extra: &[&str]) -> Option<()> {
    let mut cmd = tsc()?;
    let runtime = repo_root().join("runtimes/ts/@undra/runtime");
    let output = cmd
        .args(["-p"])
        .arg(runtime.join("tsconfig.build.json"))
        .args(["--declarationMap", "false", "--sourceMap", "false"])
        .args(extra)
        .arg("--outDir")
        .arg(out)
        .output()
        .ok()?;
    assert!(
        output.status.success(),
        "building the TypeScript runtime failed:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    Some(())
}

/// The declarations of the real wire layer, built once from
/// `runtimes/ts/@undra/runtime/src`.
pub fn ts_runtime_declarations() -> Option<&'static Path> {
    static OUT: OnceLock<Option<PathBuf>> = OnceLock::new();
    OUT.get_or_init(|| {
        let out = scratch("ts-runtime-declarations");
        build_ts_runtime(&out, &["--declaration", "--emitDeclarationOnly"])?;
        Some(out)
    })
    .as_deref()
}

/// The JavaScript of the real wire layer, built once.
pub fn ts_runtime_js() -> Option<&'static Path> {
    static OUT: OnceLock<Option<PathBuf>> = OnceLock::new();
    OUT.get_or_init(|| {
        let out = scratch("ts-runtime-js");
        build_ts_runtime(&out, &["--declaration", "false"])?;
        Some(out)
    })
    .as_deref()
}

/// Installs `@undra/runtime` below `root/node_modules`: the real wire layer and
/// standard types (declarations, and compiled JavaScript when `with_js`) plus
/// the hand-written base API from `tests/fixtures/ts-base`.
pub fn install_ts_runtime(root: &Path, declarations: &Path, js: Option<&Path>) {
    let module = root.join("node_modules/@undra/runtime");
    copy_dir(&declarations.join("wire"), &module.join("wire"));
    fs::copy(declarations.join("fnv.d.ts"), module.join("fnv.d.ts")).unwrap();
    // The standard types and their codecs, which generated code imports from the runtime
    // instead of declaring them (ADR-024), and the error base class they extend.
    for name in [
        "errors.d.ts",
        "base-error.d.ts",
        "call-error.d.ts",
        "platform.d.ts",
    ] {
        fs::copy(declarations.join(name), module.join(name)).unwrap();
    }
    fs::create_dir_all(module.join("adapters")).unwrap();
    for name in ["types.d.ts", "codecs.d.ts"] {
        fs::copy(
            declarations.join("adapters").join(name),
            module.join("adapters").join(name),
        )
        .unwrap();
    }
    let base = manifest_dir().join("tests/fixtures/ts-base");
    fs::copy(base.join("index.d.ts"), module.join("index.d.ts")).unwrap();
    fs::write(
        module.join("package.json"),
        r#"{"name":"@undra/runtime","version":"0.1.0","type":"module","types":"./index.d.ts","exports":{".":{"types":"./index.d.ts","default":"./index.js"}}}"#,
    )
    .unwrap();
    if let Some(js) = js {
        copy_dir(js, &module.join("dist"));
        fs::copy(base.join("index.js"), module.join("index.js")).unwrap();
    }
}

/// Copies a directory tree.
pub fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
        }
    }
}

/// The names of the golden cases, in the order they are documented.
pub const CASES: &[&str] = &[
    "object_graph",
    "callbacks",
    "records",
    "enums",
    "errors",
    "objects",
    "stores",
    "ports",
    "queries",
    "full",
    "stdlib",
    "recursive",
    "newtypes",
    "generics",
    "decimal",
    "polling",
    "infinite",
    "lazy",
    "generic_functions",
    "generic_objects",
];

/// The cases whose Swift output is also locked in the iOS 15 / 16 mode (`ObservableObject` stores,
/// `UndraDuration`; ADR-045), as `tests/golden/<case>/swift-observable-object/`. They are the cases that
/// have stores or query handles, and the ones with a `Duration` field.
pub const FLOOR_CASES: &[&str] = &[
    "stores",
    "queries",
    "full",
    "newtypes",
    "polling",
    "infinite",
    "lazy",
    "generic_functions",
    "generic_objects",
];

/// `generator_for` in the iOS 15 mode: `ObservableObject` stores and a floor of iOS 15 (the strictest one).
pub fn floor_generator_for(case: &str, schema: &Schema) -> Generator {
    let mut generator = generator_for(case, schema);
    generator.swift_observation = undra_bindgen::SwiftObservation::ObservableObject;
    generator.swift_min_ios = 15;
    generator
}

/// The generator configuration of a case: default names, and a Kotlin package
/// per case so every case can be compiled together.
pub fn generator_for(case: &str, schema: &Schema) -> Generator {
    let mut generator = Generator::for_crate(&schema.crate_name);
    generator.kotlin_package = format!("golden.{case}");
    generator
}

/// The schema of golden case `name`.
pub fn case(name: &str) -> Schema {
    match name {
        "object_graph" => object_graph(),
        "callbacks" => callbacks(),
        "records" => records(),
        "enums" => enums(),
        "errors" => errors(),
        "objects" => objects(),
        "stores" => stores(),
        "ports" => ports(),
        "queries" => queries(),
        "full" => full(),
        "stdlib" => stdlib(),
        "recursive" => recursive(),
        "newtypes" => newtypes(),
        "generics" => generics(),
        "decimal" => decimal(),
        "polling" => polling(),
        "infinite" => infinite(),
        "lazy" => lazy(),
        "generic_functions" => generic_functions(),
        "generic_objects" => generic_objects(),
        other => panic!("unknown golden case {other}"),
    }
}

// ----- builders ---------------------------------------------------------------

pub fn obj(name: &str) -> TypeRef {
    TypeRef::object(name)
}

pub fn cb(name: &str) -> TypeRef {
    TypeRef::callback(name)
}

pub fn field(name: &str, ty: TypeRef) -> FieldDef {
    FieldDef {
        name: name.into(),
        ty,
        default: false,
        docs: String::new(),
    }
}

pub fn default_field(name: &str, ty: TypeRef) -> FieldDef {
    FieldDef {
        default: true,
        ..field(name, ty)
    }
}

pub fn documented(mut f: FieldDef, docs: &str) -> FieldDef {
    f.docs = docs.into();
    f
}

pub fn param(name: &str, ty: TypeRef) -> ParamDef {
    ParamDef {
        name: name.into(),
        ty,
    }
}

pub fn named(name: &str) -> TypeRef {
    TypeRef::named(name)
}

pub fn record(name: &str, docs: &str, fields: Vec<FieldDef>) -> RecordDef {
    RecordDef {
        name: name.into(),
        type_id: ids::type_id(name),
        fields,
        transparent: false,
        docs: docs.into(),
    }
}

/// A newtype (ADR-042): a transparent record with the one field `value`.
pub fn newtype_record(name: &str, docs: &str, inner: TypeRef) -> RecordDef {
    RecordDef {
        transparent: true,
        ..record(name, docs, vec![field("value", inner)])
    }
}

pub fn unit_variant(name: &str, index: u16) -> VariantDef {
    VariantDef {
        name: name.into(),
        index,
        fields: Vec::new(),
        tuple: false,
        message: None,
        docs: String::new(),
    }
}

/// A variant with named fields.
pub fn struct_variant(name: &str, index: u16, fields: Vec<FieldDef>) -> VariantDef {
    VariantDef {
        fields,
        ..unit_variant(name, index)
    }
}

/// A tuple variant; the fields are named "0", "1", ...
pub fn tuple_variant(name: &str, index: u16, types: Vec<TypeRef>) -> VariantDef {
    VariantDef {
        fields: types
            .into_iter()
            .enumerate()
            .map(|(i, ty)| field(&i.to_string(), ty))
            .collect(),
        tuple: true,
        ..unit_variant(name, index)
    }
}

pub fn with_message(mut v: VariantDef, message: &str) -> VariantDef {
    v.message = Some(message.into());
    v
}

pub fn with_docs(mut v: VariantDef, docs: &str) -> VariantDef {
    v.docs = docs.into();
    v
}

pub fn enum_def(name: &str, docs: &str, variants: Vec<VariantDef>) -> EnumDef {
    EnumDef {
        name: name.into(),
        type_id: ids::type_id(name),
        is_error: false,
        variants,
        docs: docs.into(),
    }
}

pub fn error_def(name: &str, docs: &str, variants: Vec<VariantDef>) -> EnumDef {
    EnumDef {
        is_error: true,
        ..enum_def(name, docs, variants)
    }
}

pub fn method(
    owner: &str,
    name: &str,
    docs: &str,
    params: Vec<ParamDef>,
    returns: TypeRef,
    is_async: bool,
) -> MethodDef {
    MethodDef {
        name: name.into(),
        method_id: ids::method_id(owner, name),
        params,
        returns,
        is_async,
        takes_ctx: false,
        coalesce: false,
        docs: docs.into(),
        generic: None,
    }
}

pub fn ctor(owner: &str, name: &str, params: Vec<ParamDef>, is_async: bool) -> MethodDef {
    method(owner, name, "", params, named(owner), is_async)
}

pub fn fallible_ctor(
    owner: &str,
    name: &str,
    params: Vec<ParamDef>,
    err: &str,
    is_async: bool,
) -> MethodDef {
    method(
        owner,
        name,
        "",
        params,
        TypeRef::result(named(owner), named(err)),
        is_async,
    )
}

pub fn object(
    name: &str,
    docs: &str,
    constructors: Vec<MethodDef>,
    methods: Vec<MethodDef>,
) -> ObjectDef {
    ObjectDef {
        name: name.into(),
        type_id: ids::type_id(name),
        constructors,
        methods,
        store: None,
        docs: docs.into(),
    }
}

pub fn store(mut o: ObjectDef, signals: Vec<(&str, TypeRef, bool, Option<&str>)>) -> ObjectDef {
    o.store = Some(StoreDef {
        signals: signals
            .into_iter()
            .enumerate()
            .map(|(i, (name, ty, computed, key))| SignalDef {
                name: name.into(),
                signal_id: u32::try_from(i).unwrap(),
                ty,
                computed,
                key: key.map(str::to_owned),
                no_coalesce: false,
                default: false,
            })
            .collect(),
    });
    o
}

pub fn function(
    name: &str,
    docs: &str,
    params: Vec<ParamDef>,
    returns: TypeRef,
    is_async: bool,
) -> FunctionDef {
    FunctionDef {
        name: name.into(),
        method_id: ids::function_id(name),
        params,
        returns,
        is_async,
        takes_ctx: false,
        docs: docs.into(),
        generic: None,
    }
}

/// The label of the instantiation `of<arg>` of a generic function (ADR-058).
pub fn label(of: &str, arg: &str, inferred: bool) -> undra_meta::GenericOf {
    undra_meta::GenericOf {
        of: of.into(),
        args: vec![undra_meta::GenericArg {
            param: "T".into(),
            ty: named(arg),
            inferred,
        }],
    }
}

/// `of<arg>` as a free function: the definition the macro makes for one listed type.
pub fn instance_fn(
    of: &str,
    arg: &str,
    inferred: bool,
    docs: &str,
    params: Vec<ParamDef>,
    returns: TypeRef,
    is_async: bool,
) -> FunctionDef {
    let name = format!("{of}<{arg}>");
    FunctionDef {
        generic: Some(label(of, arg, inferred)),
        ..function(&name, docs, params, returns, is_async)
    }
}

/// `owner.of<arg>`: the method the macro makes for one listed type.
#[allow(clippy::too_many_arguments)] // a test builder that spells out every field of a definition
pub fn instance_method(
    owner: &str,
    of: &str,
    arg: &str,
    inferred: bool,
    docs: &str,
    params: Vec<ParamDef>,
    returns: TypeRef,
    is_async: bool,
) -> MethodDef {
    let name = format!("{of}<{arg}>");
    MethodDef {
        generic: Some(label(of, arg, inferred)),
        ..method(owner, &name, docs, params, returns, is_async)
    }
}

pub fn port(name: &str, docs: &str, kind: PortKind, methods: Vec<MethodDef>) -> PortDef {
    PortDef {
        name: name.into(),
        port_id: ids::port_id(name),
        kind,
        background: false,
        methods,
        docs: docs.into(),
    }
}

pub fn port_method(
    port: &str,
    name: &str,
    docs: &str,
    params: Vec<ParamDef>,
    returns: TypeRef,
    is_async: bool,
) -> MethodDef {
    MethodDef {
        method_id: ids::port_method_id(port, name),
        ..method(port, name, docs, params, returns, is_async)
    }
}

pub fn query(
    name: &str,
    kind: QueryKind,
    key: &str,
    params: Vec<ParamDef>,
    returns: TypeRef,
    stale_ms: Option<u64>,
) -> QueryDef {
    QueryDef {
        name: name.into(),
        query_id: match kind {
            QueryKind::Query => ids::query_id(name),
            QueryKind::Mutation => ids::mutation_id(name),
        },
        kind,
        key: key.into(),
        params,
        returns,
        stale_ms,
        persist: false,
        idempotent: false,
        interval_ms: None,
        poll_in_background: false,
        infinite: None,
    }
}

fn opt(t: TypeRef) -> TypeRef {
    TypeRef::option(t)
}

fn vec_of(t: TypeRef) -> TypeRef {
    TypeRef::vec(t)
}

fn err_result(ok: TypeRef, err: &str) -> TypeRef {
    TypeRef::result(ok, named(err))
}

// ----- the cases ----------------------------------------------------------------

fn records() -> Schema {
    let mut s = Schema::new("golden-records");
    s.records.push(record(
        "Todo",
        "A todo item.\nShown in the list and synced to the server.",
        vec![
            documented(field("id", TypeRef::Uuid), "Stable identifier."),
            field("title", TypeRef::String),
            default_field("done", TypeRef::Bool),
            default_field("tags", vec_of(TypeRef::String)),
            field("due", opt(TypeRef::Timestamp)),
            field("priority", named("Priority")),
        ],
    ));
    s.records.push(record(
        "Numbers",
        "Every fixed-width number type.",
        vec![
            field("a", TypeRef::I8),
            field("b", TypeRef::I16),
            field("c", TypeRef::I32),
            field("d", TypeRef::I64),
            field("e", TypeRef::U8),
            field("f", TypeRef::U16),
            field("g", TypeRef::U32),
            field("h", TypeRef::U64),
            field("i", TypeRef::F32),
            field("j", TypeRef::F64),
            field("elapsed", TypeRef::Duration),
        ],
    ));
    s.records.push(record(
        "Containers",
        "Nested containers and bytes.",
        vec![
            field("lines", vec_of(opt(TypeRef::String))),
            field("scores", TypeRef::map(TypeRef::String, TypeRef::I32)),
            field("by_id", TypeRef::map(TypeRef::Uuid, vec_of(named("Todo")))),
            field("blob", TypeRef::Bytes),
            field("maybe_blob", opt(TypeRef::Bytes)),
            field("matrix", vec_of(vec_of(TypeRef::F64))),
            default_field("counts", TypeRef::map(TypeRef::U32, TypeRef::U64)),
        ],
    ));
    s.records.push(record(
        "Keywords",
        "Field names that are reserved words in some target language.",
        vec![
            field("default", TypeRef::String),
            field("in", TypeRef::I32),
            field("object", TypeRef::Bool),
            field("delete", TypeRef::Bool),
            field("new", TypeRef::Bool),
            field("class_name", TypeRef::String),
        ],
    ));
    s.records
        .push(record("Empty", "A record without fields.", vec![]));
    s.records.push(record(
        "Page",
        "A recursive record.",
        vec![
            field("items", vec_of(named("Todo"))),
            field("next", opt(TypeRef::String)),
            field("children", vec_of(named("Page"))),
        ],
    ));
    s.enums.push(enum_def(
        "Priority",
        "",
        vec![
            unit_variant("Low", 0),
            unit_variant("Normal", 1),
            unit_variant("High", 2),
        ],
    ));
    s
}

fn enums() -> Schema {
    let mut s = Schema::new("golden-enums");
    s.enums.push(enum_def(
        "Filter",
        "Which todos the list shows.",
        vec![
            with_docs(unit_variant("All", 0), "Everything."),
            with_docs(unit_variant("Active", 1), "Not done yet."),
            with_docs(unit_variant("Done", 2), "Finished."),
        ],
    ));
    s.enums.push(enum_def(
        "NetKind",
        "",
        vec![
            unit_variant("Wifi", 0),
            unit_variant("Cellular", 1),
            unit_variant("None", 2),
            unit_variant("Default", 3),
        ],
    ));
    s.enums.push(enum_def(
        "Sparse",
        "Variant indexes need not be dense.",
        vec![unit_variant("First", 1), unit_variant("Second", 7)],
    ));
    s.enums.push(enum_def(
        "Shape",
        "A shape with data.",
        vec![
            with_docs(
                struct_variant("Circle", 0, vec![field("radius", TypeRef::F64)]),
                "A circle.",
            ),
            tuple_variant("Rect", 1, vec![TypeRef::F64, TypeRef::F64]),
            struct_variant(
                "Labelled",
                2,
                vec![
                    field("label", TypeRef::String),
                    field("kind", TypeRef::I32),
                    field("inner", opt(named("Filter"))),
                ],
            ),
            tuple_variant("Single", 3, vec![TypeRef::String]),
            unit_variant("Empty", 4),
        ],
    ));
    s.enums.push(enum_def(
        "Value",
        "A JSON-like value: variant names shadow type names.",
        vec![
            tuple_variant("String", 0, vec![TypeRef::String]),
            tuple_variant("Int", 1, vec![TypeRef::I64]),
            tuple_variant("Bool", 2, vec![TypeRef::Bool]),
            tuple_variant("List", 3, vec![vec_of(named("Value"))]),
            tuple_variant("Shape", 4, vec![named("Shape")]),
            tuple_variant("Filter", 5, vec![named("Filter")]),
            unit_variant("Null", 6),
        ],
    ));
    s
}

fn errors() -> Schema {
    let mut s = Schema::new("golden-errors");
    s.enums.push(error_def(
        "HttpError",
        "A network failure.",
        vec![
            with_message(unit_variant("Timeout", 0), "timed out"),
            with_message(tuple_variant("Status", 1, vec![TypeRef::U16]), "status {0}"),
            with_message(
                tuple_variant("Network", 2, vec![TypeRef::String]),
                "network error: {0:?}",
            ),
            with_message(unit_variant("Cancelled", 3), "cancelled"),
        ],
    ));
    s.enums.push(error_def(
        "TodoError",
        "What can go wrong with todos.",
        vec![
            with_docs(
                with_message(unit_variant("EmptyTitle", 0), "title cannot be empty"),
                "The title was blank after trimming.",
            ),
            // `#[error(transparent)] Http(#[from] HttpError)`
            tuple_variant("Http", 1, vec![named("HttpError")]),
            with_message(
                tuple_variant("NotFound", 2, vec![TypeRef::String]),
                "todo \"{0}\" not found, {{sorry}}",
            ),
            with_message(
                struct_variant(
                    "Storage",
                    3,
                    vec![
                        field("reason", TypeRef::String),
                        field("code", TypeRef::I32),
                        field("message", TypeRef::String),
                    ],
                ),
                "storage failure {code}: {reason} (${message})",
            ),
            with_message(
                tuple_variant("Range", 4, vec![TypeRef::U32, TypeRef::U32]),
                "{0} is not below {1}",
            ),
            with_message(
                struct_variant("Detail", 5, vec![field("default", TypeRef::String)]),
                "detail {default}",
            ),
        ],
    ));
    s.enums.push(error_def(
        "Boxed",
        "An error that wraps a record, so errors and types reference each other.",
        vec![with_message(
            tuple_variant("Holding", 0, vec![named("Payload")]),
            "holding a payload",
        )],
    ));
    s.records.push(record(
        "Payload",
        "",
        vec![
            field("text", TypeRef::String),
            field("failure", opt(named("HttpError"))),
            field("history", vec_of(named("Boxed"))),
        ],
    ));
    s
}

fn objects() -> Schema {
    let mut s = Schema::new("golden-objects");
    s.enums.push(error_def(
        "CalcError",
        "",
        vec![
            with_message(unit_variant("Overflow", 0), "overflow"),
            with_message(unit_variant("DivideByZero", 1), "divide by zero"),
        ],
    ));
    s.records.push(record(
        "Todo",
        "",
        vec![field("id", TypeRef::Uuid), field("title", TypeRef::String)],
    ));
    s.records.push(record(
        "Stats",
        "",
        vec![field("count", TypeRef::U32), field("total", TypeRef::I64)],
    ));
    s.enums.push(enum_def(
        "Mode",
        "",
        vec![unit_variant("Fast", 0), unit_variant("Exact", 1)],
    ));
    s.objects.push(object(
        "Calculator",
        "Adds numbers.",
        vec![
            ctor("Calculator", "new", vec![], false),
            fallible_ctor(
                "Calculator",
                "with_precision",
                vec![param("digits", TypeRef::U8)],
                "CalcError",
                false,
            ),
            fallible_ctor(
                "Calculator",
                "open",
                vec![param("path", TypeRef::String), param("mode", named("Mode"))],
                "CalcError",
                true,
            ),
        ],
        vec![
            method(
                "Calculator",
                "add",
                "Adds two numbers.",
                vec![param("a", TypeRef::I32), param("b", TypeRef::I32)],
                TypeRef::I32,
                false,
            ),
            method("Calculator", "reset", "", vec![], TypeRef::Unit, false),
            method(
                "Calculator",
                "divide",
                "Divides, failing on zero.",
                vec![param("a", TypeRef::I64), param("b", TypeRef::I64)],
                err_result(TypeRef::I64, "CalcError"),
                false,
            ),
            method(
                "Calculator",
                "check",
                "",
                vec![],
                err_result(TypeRef::Unit, "CalcError"),
                false,
            ),
            method(
                "Calculator",
                "lookup",
                "Fetches a todo.",
                vec![param("id", TypeRef::Uuid)],
                err_result(named("Todo"), "CalcError"),
                true,
            ),
            method(
                "Calculator",
                "compute",
                "An async method without an error type.",
                vec![param("input", opt(TypeRef::F64))],
                TypeRef::F64,
                true,
            ),
            method("Calculator", "warm_up", "", vec![], TypeRef::Unit, true),
            method(
                "Calculator",
                "ticks",
                "Counts up.",
                vec![param("n", TypeRef::U32)],
                TypeRef::stream(TypeRef::U32),
                true,
            ),
            method(
                "Calculator",
                "watch",
                "Streams todos, failing to open with a typed error.",
                vec![param("mode", named("Mode"))],
                TypeRef::result(TypeRef::stream(named("Todo")), named("CalcError")),
                true,
            ),
            method("Calculator", "stats", "", vec![], named("Stats"), false),
            method(
                "Calculator",
                "delete",
                "A method named like a keyword.",
                vec![
                    param("w", TypeRef::I32),
                    param("body", TypeRef::I32),
                    param("core", TypeRef::I32),
                    param("default", TypeRef::I32),
                    param("signal", TypeRef::I32),
                ],
                TypeRef::I32,
                true,
            ),
        ],
    ));
    s.functions.push(function(
        "greet",
        "Says hello.",
        vec![param("name", TypeRef::String)],
        TypeRef::String,
        false,
    ));
    s.functions.push(function(
        "ping",
        "",
        vec![],
        err_result(TypeRef::Unit, "CalcError"),
        true,
    ));
    s.functions.push(function(
        "numbers",
        "",
        vec![param("upto", TypeRef::U32)],
        TypeRef::stream(TypeRef::U32),
        true,
    ));
    s
}

fn stores() -> Schema {
    let mut s = Schema::new("golden-stores");
    s.enums.push(error_def(
        "TodoError",
        "",
        vec![
            with_message(unit_variant("EmptyTitle", 0), "title cannot be empty"),
            with_message(unit_variant("Storage", 1), "storage failure"),
        ],
    ));
    s.enums.push(enum_def(
        "Filter",
        "",
        vec![
            unit_variant("All", 0),
            unit_variant("Active", 1),
            unit_variant("Done", 2),
        ],
    ));
    s.records.push(record(
        "Todo",
        "",
        vec![
            field("id", TypeRef::Uuid),
            field("title", TypeRef::String),
            field("done", TypeRef::Bool),
        ],
    ));
    s.records.push(record(
        "Counter",
        "",
        vec![field("label", TypeRef::String), field("n", TypeRef::I32)],
    ));
    s.objects.push(store(
        object(
            "Todos",
            "The todo list.",
            vec![
                ctor("Todos", "new", vec![], false),
                fallible_ctor(
                    "Todos",
                    "open",
                    vec![param("path", TypeRef::String)],
                    "TodoError",
                    true,
                ),
            ],
            vec![
                method(
                    "Todos",
                    "set_filter",
                    "Shows only the todos matching `f`.",
                    vec![param("f", named("Filter"))],
                    TypeRef::Unit,
                    false,
                ),
                method(
                    "Todos",
                    "add",
                    "Adds a todo.",
                    vec![param("title", TypeRef::String)],
                    err_result(named("Todo"), "TodoError"),
                    true,
                ),
                method(
                    "Todos",
                    "remaining_after",
                    "",
                    vec![param("id", TypeRef::Uuid)],
                    TypeRef::U32,
                    false,
                ),
                method(
                    "Todos",
                    "changes",
                    "",
                    vec![],
                    TypeRef::stream(named("Todo")),
                    true,
                ),
            ],
        ),
        vec![
            ("todos", vec_of(named("Todo")), false, Some("id")),
            ("filter", named("Filter"), false, None),
            ("visible", vec_of(named("Todo")), true, Some("id")),
            ("remaining", TypeRef::U32, true, None),
            ("selected", opt(named("Todo")), false, None),
            ("title", TypeRef::String, false, None),
            ("counter", named("Counter"), false, None),
            (
                "tags",
                TypeRef::map(TypeRef::String, TypeRef::U32),
                false,
                None,
            ),
            ("last_error", opt(named("TodoError")), false, None),
            ("elapsed", TypeRef::Duration, false, None),
            ("created", TypeRef::Timestamp, false, None),
            ("blob", TypeRef::Bytes, false, None),
            ("total", TypeRef::U64, false, None),
            ("default", TypeRef::Bool, false, None),
            ("uuid", TypeRef::Uuid, false, None),
        ],
    ));
    s.objects.push(store(
        object(
            "Clock",
            "A store with a single signal and a constructor argument.",
            vec![ctor(
                "Clock",
                "new",
                vec![param("zone", TypeRef::String)],
                false,
            )],
            vec![],
        ),
        vec![("now", TypeRef::Timestamp, false, None)],
    ));
    // A clock that ticks wants every tick delivered: `#[undra(no_coalesce)]` (ADR-031).
    let clock = s.objects.last_mut().and_then(|o| o.store.as_mut()).unwrap();
    clock.signals[0].no_coalesce = true;
    s
}

fn ports() -> Schema {
    let mut s = Schema::new("golden-ports");
    s.enums.push(error_def(
        "HttpError",
        "",
        vec![
            with_message(unit_variant("Timeout", 0), "timed out"),
            with_message(tuple_variant("Network", 1, vec![TypeRef::String]), "{0}"),
        ],
    ));
    s.enums.push(error_def(
        "FsError",
        "",
        vec![
            with_message(unit_variant("NotFound", 0), "not found"),
            with_message(tuple_variant("Io", 1, vec![TypeRef::String]), "io: {0}"),
        ],
    ));
    s.enums.push(enum_def(
        "NetKind",
        "",
        vec![
            unit_variant("Wifi", 0),
            unit_variant("Cellular", 1),
            unit_variant("None", 2),
        ],
    ));
    s.records.push(record(
        "HttpRequest",
        "",
        vec![
            field("url", TypeRef::String),
            field("headers", TypeRef::map(TypeRef::String, TypeRef::String)),
            field("body", opt(TypeRef::Bytes)),
        ],
    ));
    s.records.push(record(
        "HttpResponse",
        "",
        vec![field("status", TypeRef::U16), field("body", TypeRef::Bytes)],
    ));
    s.ports.push(port(
        "Clock",
        "Wall-clock and monotonic time.",
        PortKind::Sync,
        vec![
            port_method(
                "Clock",
                "now_ms",
                "Milliseconds since the epoch.",
                vec![],
                TypeRef::I64,
                false,
            ),
            port_method("Clock", "monotonic_ns", "", vec![], TypeRef::U64, false),
            port_method(
                "Clock",
                "log",
                "Logs a line.",
                vec![
                    param("level", TypeRef::U8),
                    param("target", TypeRef::String),
                    param("message", TypeRef::String),
                ],
                TypeRef::Unit,
                false,
            ),
        ],
    ));
    s.ports.push(port(
        "Http",
        "Performs HTTP requests.",
        PortKind::Async,
        vec![port_method(
            "Http",
            "request",
            "Sends a request.",
            vec![param("req", named("HttpRequest"))],
            err_result(named("HttpResponse"), "HttpError"),
            true,
        )],
    ));
    s.ports.push(port(
        "Kv",
        "A key-value store.",
        PortKind::Async,
        vec![
            port_method(
                "Kv",
                "get",
                "",
                vec![param("key", TypeRef::String)],
                opt(TypeRef::Bytes),
                true,
            ),
            port_method(
                "Kv",
                "set",
                "",
                vec![
                    param("key", TypeRef::String),
                    param("value", TypeRef::Bytes),
                ],
                TypeRef::Unit,
                true,
            ),
            port_method(
                "Kv",
                "list",
                "",
                vec![param("prefix", TypeRef::String)],
                vec_of(TypeRef::String),
                true,
            ),
            port_method(
                "Kv",
                "flush",
                "A synchronous method on an async port.",
                vec![],
                err_result(TypeRef::Unit, "FsError"),
                false,
            ),
        ],
    ));
    s.ports.push(port(
        "Connectivity",
        "Connectivity changes, sent by the platform.",
        PortKind::Event,
        vec![
            port_method(
                "Connectivity",
                "changed",
                "The network changed.",
                vec![
                    param("online", TypeRef::Bool),
                    param("kind", named("NetKind")),
                ],
                TypeRef::Unit,
                false,
            ),
            port_method("Connectivity", "reset", "", vec![], TypeRef::Unit, false),
        ],
    ));
    s
}

/// The schema of an app core: the standard library exactly as `undra-ports` registers it (every
/// core links it, so every schema has it), plus the app's own items, some of which refer to the
/// standard types. Nothing of the standard library is generated; the references resolve to the
/// runtimes' own types (ADR-024).
fn stdlib() -> Schema {
    // Naming a registered type links the registrations of `undra-ports` into this binary.
    let _ = undra_ports::HttpMethod::Get;
    let mut s = undra_meta::collect_schema("golden-stdlib");
    s.records.push(record(
        "Endpoint",
        "A request the app makes again and again.",
        vec![
            field("name", TypeRef::String),
            field("request", named("HttpRequest")),
            field("fallback", opt(named("HttpResponse"))),
            field("accepted", vec_of(named("HttpMethod"))),
            field("extra", vec_of(named("Header"))),
        ],
    ));
    // The opt-in standard types (ADR-047, ADR-048) resolve to the runtimes' own as well.
    s.records.push(record(
        "Feed",
        "What a live screen keeps.",
        vec![
            field("last", named("WsMessage")),
            field("event", opt(named("SseEvent"))),
            field("cells", vec_of(named("DbValue"))),
            field("page", opt(named("DbRows"))),
        ],
    ));
    // A user type may share the name of a standard *port*: the ports are not generated.
    s.records.push(record(
        "Connectivity",
        "What the app last learned about the network.",
        vec![
            field("online", TypeRef::Bool),
            field("kind", named("NetKind")),
            field("app", opt(named("AppState"))),
        ],
    ));
    s.enums.push(error_def(
        "SyncError",
        "Why a sync failed.",
        vec![
            with_message(unit_variant("Offline", 0), "offline"),
            tuple_variant("Http", 1, vec![named("HttpError")]),
            with_message(
                tuple_variant("Disk", 2, vec![named("FsError"), TypeRef::String]),
                "disk failure at {1}",
            ),
            with_message(
                struct_variant("Rejected", 3, vec![field("status", TypeRef::U16)]),
                "status {status}",
            ),
        ],
    ));
    s.objects.push(object(
        "Syncer",
        "Talks to the server.",
        vec![ctor("Syncer", "new", vec![], false)],
        vec![
            method(
                "Syncer",
                "send",
                "Performs one request.",
                vec![param("request", named("HttpRequest"))],
                err_result(named("HttpResponse"), "HttpError"),
                true,
            ),
            method(
                "Syncer",
                "follow",
                "Streams the responses of a request that repeats.",
                vec![param("endpoint", named("Endpoint"))],
                TypeRef::result(TypeRef::stream(named("HttpResponse")), named("HttpError")),
                true,
            ),
            method(
                "Syncer",
                "save",
                "Writes the last response to disk.",
                vec![param("path", TypeRef::String)],
                err_result(TypeRef::Unit, "FsError"),
                true,
            ),
            method(
                "Syncer",
                "sync",
                "",
                vec![],
                err_result(TypeRef::Unit, "SyncError"),
                true,
            ),
            method(
                "Syncer",
                "local",
                "Reads the local database.",
                vec![param("sql", TypeRef::String)],
                err_result(named("DbRows"), "DbError"),
                true,
            ),
            method(
                "Syncer",
                "listen",
                "Streams the server's events.",
                vec![],
                TypeRef::result(TypeRef::stream(named("SseEvent")), named("SseError")),
                true,
            ),
            method(
                "Syncer",
                "push",
                "Sends one message on the live connection.",
                vec![param("message", named("WsMessage"))],
                err_result(TypeRef::Unit, "WsError"),
                true,
            ),
        ],
    ));
    s.objects.push(store(
        object(
            "Link",
            "Mirrors what the platform reports about the connection.",
            vec![ctor("Link", "new", vec![], false)],
            vec![],
        ),
        vec![
            ("state", named("AppState"), false, None),
            ("kind", named("NetKind"), false, None),
            ("last", opt(named("HttpResponse")), false, None),
            ("failure", opt(named("HttpError")), false, None),
            ("pending", vec_of(named("HttpRequest")), false, None),
        ],
    ));
    s.ports.push(port(
        "Uploader",
        "The platform uploads a request in the background.",
        PortKind::Async,
        vec![port_method(
            "Uploader",
            "upload",
            "",
            vec![param("request", named("HttpRequest"))],
            err_result(named("HttpResponse"), "HttpError"),
            true,
        )],
    ));
    s.queries.push(query(
        "latest_response",
        QueryKind::Query,
        "latest",
        vec![],
        err_result(named("HttpResponse"), "HttpError"),
        Some(30_000),
    ));
    s.queries.push(query(
        "retry",
        QueryKind::Mutation,
        "retry",
        vec![param("request", named("HttpRequest"))],
        err_result(named("HttpResponse"), "HttpError"),
        None,
    ));
    s
}

fn queries() -> Schema {
    let mut s = Schema::new("golden-queries");
    s.enums.push(error_def(
        "TodoError",
        "",
        vec![with_message(unit_variant("Offline", 0), "offline")],
    ));
    s.records.push(record(
        "Todo",
        "",
        vec![field("id", TypeRef::Uuid), field("title", TypeRef::String)],
    ));
    s.records.push(record(
        "Page",
        "",
        vec![
            field("items", vec_of(named("Todo"))),
            field("total", TypeRef::U64),
        ],
    ));
    let mut todos = query(
        "todos",
        QueryKind::Query,
        "todos:{page}",
        vec![param("page", TypeRef::U32)],
        err_result(named("Page"), "TodoError"),
        Some(30_000),
    );
    todos.persist = true;
    s.queries.push(todos);
    s.queries.push(query(
        "todo_count",
        QueryKind::Query,
        "todo-count",
        vec![],
        TypeRef::U32,
        None,
    ));
    s.queries.push(query(
        "todo_by_id",
        QueryKind::Query,
        "todo:{id}",
        vec![param("id", TypeRef::Uuid), param("fresh", TypeRef::Bool)],
        err_result(named("Todo"), "TodoError"),
        Some(5_000),
    ));
    let mut add = query(
        "add_todo",
        QueryKind::Mutation,
        "todos",
        vec![param("title", TypeRef::String)],
        err_result(named("Todo"), "TodoError"),
        None,
    );
    add.idempotent = true;
    s.queries.push(add);
    s.queries.push(query(
        "clear_todos",
        QueryKind::Mutation,
        "todos",
        vec![],
        TypeRef::Unit,
        None,
    ));
    s
}

/// A playground-like core: records, enums, errors, an object, a store, ports,
/// queries and free functions.
fn full() -> Schema {
    let mut s = Schema::new("playground-core");
    s.records.push(record(
        "Todo",
        "A todo item.",
        vec![
            field("id", TypeRef::Uuid),
            documented(field("title", TypeRef::String), "Shown in the list."),
            default_field("done", TypeRef::Bool),
            field("tags", vec_of(TypeRef::String)),
            field("due", opt(TypeRef::Timestamp)),
            field("priority", named("Priority")),
        ],
    ));
    s.records.push(record(
        "Page",
        "One page of todos.",
        vec![
            field("items", vec_of(named("Todo"))),
            field("next", opt(TypeRef::String)),
            field("total", TypeRef::U64),
        ],
    ));
    s.records.push(record(
        "HttpRequest",
        "",
        vec![
            field("method", TypeRef::String),
            field("url", TypeRef::String),
            field("headers", TypeRef::map(TypeRef::String, TypeRef::String)),
            field("body", opt(TypeRef::Bytes)),
        ],
    ));
    s.records.push(record(
        "HttpResponse",
        "",
        vec![
            field("status", TypeRef::U16),
            field("headers", TypeRef::map(TypeRef::String, TypeRef::String)),
            field("body", TypeRef::Bytes),
            field("elapsed", TypeRef::Duration),
        ],
    ));
    s.enums.push(enum_def(
        "Priority",
        "",
        vec![
            unit_variant("Low", 0),
            unit_variant("Normal", 1),
            unit_variant("High", 2),
        ],
    ));
    s.enums.push(enum_def(
        "Filter",
        "Which todos the list shows.",
        vec![
            unit_variant("All", 0),
            unit_variant("Active", 1),
            unit_variant("Done", 2),
        ],
    ));
    s.enums.push(enum_def(
        "Shape",
        "",
        vec![
            struct_variant("Circle", 0, vec![field("radius", TypeRef::F64)]),
            struct_variant(
                "Rect",
                1,
                vec![field("w", TypeRef::F64), field("h", TypeRef::F64)],
            ),
            unit_variant("Empty", 2),
        ],
    ));
    s.enums.push(enum_def(
        "NetKind",
        "",
        vec![
            unit_variant("Wifi", 0),
            unit_variant("Cellular", 1),
            unit_variant("None", 2),
        ],
    ));
    s.enums.push(error_def(
        "HttpError",
        "",
        vec![
            with_message(unit_variant("Timeout", 0), "timed out"),
            with_message(tuple_variant("Status", 1, vec![TypeRef::U16]), "status {0}"),
        ],
    ));
    s.enums.push(error_def(
        "TodoError",
        "",
        vec![
            with_message(unit_variant("EmptyTitle", 0), "title cannot be empty"),
            tuple_variant("Http", 1, vec![named("HttpError")]),
            with_message(
                tuple_variant("NotFound", 2, vec![TypeRef::String]),
                "todo {0} not found",
            ),
            with_message(
                struct_variant("Storage", 3, vec![field("reason", TypeRef::String)]),
                "storage failure ({reason})",
            ),
        ],
    ));
    s.objects.push(object(
        "Calculator",
        "Adds numbers.",
        vec![ctor("Calculator", "new", vec![], false)],
        vec![
            method(
                "Calculator",
                "add",
                "",
                vec![param("a", TypeRef::I32), param("b", TypeRef::I32)],
                TypeRef::I32,
                false,
            ),
            method(
                "Calculator",
                "fetch",
                "",
                vec![param("url", TypeRef::String)],
                err_result(TypeRef::String, "HttpError"),
                true,
            ),
            method(
                "Calculator",
                "ticks",
                "",
                vec![],
                TypeRef::stream(TypeRef::U32),
                true,
            ),
            method(
                "Calculator",
                "watch",
                "",
                vec![param("priority", named("Priority"))],
                TypeRef::result(TypeRef::stream(named("Todo")), named("TodoError")),
                true,
            ),
        ],
    ));
    s.objects.push(store(
        object(
            "TodoStore",
            "The todo list store.",
            vec![
                ctor("TodoStore", "new", vec![], false),
                fallible_ctor(
                    "TodoStore",
                    "open",
                    vec![param("path", TypeRef::String)],
                    "TodoError",
                    true,
                ),
            ],
            vec![
                method(
                    "TodoStore",
                    "set_filter",
                    "",
                    vec![param("f", named("Filter"))],
                    TypeRef::Unit,
                    false,
                ),
                method(
                    "TodoStore",
                    "add",
                    "",
                    vec![param("title", TypeRef::String)],
                    err_result(named("Todo"), "TodoError"),
                    true,
                ),
                method(
                    "TodoStore",
                    "toggle",
                    "",
                    vec![param("id", TypeRef::Uuid)],
                    TypeRef::Unit,
                    false,
                ),
            ],
        ),
        vec![
            ("todos", vec_of(named("Todo")), false, Some("id")),
            ("filter", named("Filter"), false, None),
            ("visible", vec_of(named("Todo")), true, Some("id")),
            ("remaining", TypeRef::U32, true, None),
            ("selected", opt(named("Todo")), false, None),
        ],
    ));
    s.functions.push(function(
        "greet",
        "",
        vec![param("name", TypeRef::String)],
        TypeRef::String,
        false,
    ));
    s.functions.push(function(
        "ping",
        "",
        vec![],
        err_result(TypeRef::Unit, "TodoError"),
        true,
    ));
    // Not `Clock`: a port that is exactly the standard `Clock` is left out of the output
    // (ADR-024), and this case is about a sync port of the app's own.
    s.ports.push(port(
        "WallClock",
        "",
        PortKind::Sync,
        vec![
            port_method("WallClock", "now_ms", "", vec![], TypeRef::I64, false),
            port_method("WallClock", "monotonic_ns", "", vec![], TypeRef::U64, false),
        ],
    ));
    s.ports.push(port(
        "Http",
        "",
        PortKind::Async,
        vec![port_method(
            "Http",
            "request",
            "",
            vec![param("req", named("HttpRequest"))],
            err_result(named("HttpResponse"), "HttpError"),
            true,
        )],
    ));
    s.ports.push(port(
        "Kv",
        "",
        PortKind::Async,
        vec![
            port_method(
                "Kv",
                "get",
                "",
                vec![param("key", TypeRef::String)],
                opt(TypeRef::Bytes),
                true,
            ),
            port_method(
                "Kv",
                "set",
                "",
                vec![
                    param("key", TypeRef::String),
                    param("value", TypeRef::Bytes),
                ],
                TypeRef::Unit,
                true,
            ),
        ],
    ));
    s.ports.push(port(
        "Connectivity",
        "",
        PortKind::Event,
        vec![port_method(
            "Connectivity",
            "changed",
            "",
            vec![
                param("online", TypeRef::Bool),
                param("kind", named("NetKind")),
            ],
            TypeRef::Unit,
            false,
        )],
    ));
    let mut todos = query(
        "todos",
        QueryKind::Query,
        "todos:{page}",
        vec![param("page", TypeRef::U32)],
        err_result(named("Page"), "TodoError"),
        Some(30_000),
    );
    todos.persist = true;
    s.queries.push(todos);
    s.queries.push(query(
        "add_todo",
        QueryKind::Mutation,
        "todos",
        vec![param("title", TypeRef::String)],
        err_result(named("Todo"), "TodoError"),
        None,
    ));
    s
}

/// Types that hold themselves: a linked record, a tree, two records that hold each other, a record
/// and an enum that hold each other, an enum and an error enum that hold themselves, an enum whose
/// base case is its last variant (a signal's placeholder must find it), and a store,
/// a method and a function that take and return them (SPEC section 10.1, recursive types).
fn recursive() -> Schema {
    let mut s = Schema::new("golden-recursive");
    s.records.push(record(
        "ListNode",
        "A singly linked list: each node holds the rest of the list.",
        vec![
            field("value", TypeRef::I32),
            documented(
                field("next", opt(named("ListNode"))),
                "The rest of the list; `None` at the end.",
            ),
        ],
    ));
    s.records.push(record(
        "Tree",
        "A tree: the children are in an array, which needs no indirection.",
        vec![
            field("label", TypeRef::String),
            field("children", vec_of(named("Tree"))),
        ],
    ));
    s.records.push(record(
        "Parent",
        "Holds a `Child` that holds a `Parent`.",
        vec![
            field("name", TypeRef::String),
            field("child", opt(named("Child"))),
        ],
    ));
    s.records.push(record(
        "Child",
        "Holds a `Parent` that holds a `Child`.",
        vec![
            field("name", TypeRef::String),
            field("parent", opt(named("Parent"))),
        ],
    ));
    s.records.push(record(
        "Group",
        "A record that holds an expression, which holds the record again.",
        vec![
            field("label", TypeRef::String),
            field("inner", opt(named("Expr"))),
        ],
    ));
    s.enums.push(enum_def(
        "Expr",
        "An expression that holds a group, which holds an expression.",
        vec![
            tuple_variant("Num", 0, vec![TypeRef::F64]),
            tuple_variant("Grouped", 1, vec![named("Group")]),
        ],
    ));
    s.enums.push(enum_def(
        "Path",
        "A path whose steps hold the rest of it.",
        vec![
            unit_variant("End", 0),
            struct_variant(
                "Step",
                1,
                vec![
                    field("label", TypeRef::String),
                    field("rest", named("Path")),
                ],
            ),
        ],
    ));
    s.enums.push(enum_def(
        "Sum",
        "A term whose base case is its last variant: a placeholder cannot start from `Add`.",
        vec![
            tuple_variant("Add", 0, vec![named("Sum"), named("Sum")]),
            tuple_variant("Neg", 1, vec![named("Sum")]),
            unit_variant("Zero", 2),
        ],
    ));
    s.enums.push(error_def(
        "ParseError",
        "A parse failure that can wrap the failure it came from.",
        vec![
            with_message(unit_variant("Eof", 0), "unexpected end of input"),
            with_message(
                struct_variant(
                    "Nested",
                    1,
                    vec![
                        field("depth", TypeRef::U32),
                        field("inner", named("ParseError")),
                    ],
                ),
                "failed at depth {depth}",
            ),
        ],
    ));
    s.objects.push(store(
        object(
            "Outline",
            "A store whose signals are recursive types.",
            vec![ctor("Outline", "new", vec![], false)],
            vec![
                method(
                    "Outline",
                    "push_front",
                    "Returns `list` with a new first node.",
                    vec![
                        param("list", named("ListNode")),
                        param("value", TypeRef::I32),
                    ],
                    named("ListNode"),
                    false,
                ),
                method(
                    "Outline",
                    "replace_tree",
                    "Shows `tree`.",
                    vec![param("tree", named("Tree"))],
                    TypeRef::Unit,
                    false,
                ),
                method(
                    "Outline",
                    "parse",
                    "Parses `text`.",
                    vec![param("text", TypeRef::String)],
                    err_result(named("Expr"), "ParseError"),
                    true,
                ),
            ],
        ),
        vec![
            ("head", named("ListNode"), false, None),
            ("maybe_head", opt(named("ListNode")), false, None),
            ("tree", named("Tree"), false, None),
            ("family", named("Parent"), false, None),
            ("expr", named("Expr"), false, None),
            ("path", named("Path"), false, None),
            ("last_error", opt(named("ParseError")), false, None),
            ("total", named("Sum"), false, None),
        ],
    ));
    s.functions.push(function(
        "reverse",
        "Reverses a list.",
        vec![param("list", named("ListNode"))],
        named("ListNode"),
        false,
    ));
    s.functions.push(function(
        "evaluate",
        "Evaluates an expression.",
        vec![param("expr", named("Expr"))],
        err_result(TypeRef::F64, "ParseError"),
        true,
    ));
    s
}

/// Objects as parameters and returns (ADR-040): an account that hands out mailboxes, threads and a
/// child store, takes them back, and a free function that returns an object.
fn object_graph() -> Schema {
    let mut s = Schema::new("golden-object-graph");
    s.enums.push(error_def(
        "MailError",
        "",
        vec![with_message(unit_variant("NoThread", 0), "no such thread")],
    ));
    s.records.push(record(
        "Message",
        "A message in a chat.",
        vec![field("id", TypeRef::U32), field("text", TypeRef::String)],
    ));
    s.objects.push(object(
        "Account",
        "An account that hands out its mailboxes.",
        vec![ctor("Account", "new", vec![], false)],
        vec![
            method(
                "Account",
                "mailbox",
                "The mailbox for `folder`, created on first use.",
                vec![param("folder", TypeRef::String)],
                obj("Mailbox"),
                false,
            ),
            method(
                "Account",
                "open_thread",
                "Opens a thread.",
                vec![param("id", TypeRef::U32)],
                TypeRef::result(obj("Thread"), named("MailError")),
                true,
            ),
            method(
                "Account",
                "move_to",
                "Moves a message to `target`.",
                vec![
                    param("message", TypeRef::U32),
                    param("target", obj("Mailbox")),
                ],
                TypeRef::Unit,
                false,
            ),
            method(
                "Account",
                "drafts",
                "The drafts folder, if there is one.",
                vec![],
                opt(obj("Mailbox")),
                false,
            ),
            method(
                "Account",
                "mailboxes",
                "Every mailbox.",
                vec![],
                TypeRef::vec(obj("Mailbox")),
                false,
            ),
            method(
                "Account",
                "chat",
                "The chat with `peer`: a store.",
                vec![param("peer", TypeRef::U32)],
                obj("ChatStore"),
                false,
            ),
            method(
                "Account",
                "merge",
                "Merges the given mailboxes into the first and returns how many there were.",
                vec![
                    param("boxes", TypeRef::vec(obj("Mailbox"))),
                    param("extra", opt(obj("Mailbox"))),
                ],
                TypeRef::U32,
                false,
            ),
            method(
                "Account",
                "find_thread",
                "Finds a thread, if it exists.",
                vec![param("id", TypeRef::U32)],
                TypeRef::result(opt(obj("Thread")), named("MailError")),
                false,
            ),
            method(
                "Account",
                "follow",
                "Follows the unread count of `target` and, when there is one, of `extra`.",
                vec![
                    param("target", obj("Mailbox")),
                    param("extra", opt(obj("Mailbox"))),
                ],
                TypeRef::Stream(Box::new(TypeRef::U32)),
                false,
            ),
            method(
                "Account",
                "follow_checked",
                "Follows `boxes`; a stream that can fail to open.",
                vec![param("boxes", TypeRef::vec(obj("Mailbox")))],
                TypeRef::result(TypeRef::Stream(Box::new(TypeRef::U32)), named("MailError")),
                false,
            ),
        ],
    ));
    s.objects.push(object(
        "Vault",
        "Opens asynchronously, and can fail to: its `new` takes parameters.",
        vec![fallible_ctor(
            "Vault",
            "new",
            vec![param("name", TypeRef::String)],
            "MailError",
            true,
        )],
        vec![method("Vault", "size", "", vec![], TypeRef::U32, false)],
    ));
    s.objects.push(object(
        "Mailbox",
        "A folder.",
        vec![],
        vec![method(
            "Mailbox",
            "name",
            "",
            vec![],
            TypeRef::String,
            false,
        )],
    ));
    s.objects.push(object(
        "Thread",
        "A conversation.",
        vec![],
        vec![method("Thread", "id", "", vec![], TypeRef::U32, false)],
    ));
    s.objects.push(store(
        object(
            "ChatStore",
            "A conversation the platform observes.",
            vec![ctor("ChatStore", "new", vec![], false)],
            vec![method(
                "ChatStore",
                "send",
                "Sends a message.",
                vec![param("text", TypeRef::String)],
                TypeRef::Unit,
                false,
            )],
        ),
        vec![(
            "messages",
            TypeRef::vec(named("Message")),
            false,
            Some("id"),
        )],
    ));
    s.functions.push(function(
        "mailbox_of",
        "The mailbox of `account` for `folder`.",
        vec![
            param("account", obj("Account")),
            param("folder", TypeRef::String),
        ],
        obj("Mailbox"),
        false,
    ));
    s.functions.push(function(
        "follow_all",
        "Follows the unread count of every mailbox of `account`.",
        vec![param("account", obj("Account"))],
        TypeRef::Stream(Box::new(TypeRef::U32)),
        false,
    ));
    s
}

/// Host callback interfaces (ADR-041): a listener delivered on the main thread (with a coalesced
/// progress report, a fire-and-forget note and an async question), a token provider delivered off
/// it, and objects and functions that take them.
fn callbacks() -> Schema {
    let mut s = Schema::new("golden-callbacks");
    s.enums.push(error_def(
        "PromptError",
        "",
        vec![
            with_message(unit_variant("Declined", 0), "the user declined"),
            with_message(
                tuple_variant("Unavailable", 1, vec![TypeRef::String]),
                "{0}",
            ),
        ],
    ));
    s.enums.push(error_def(
        "AuthError",
        "",
        vec![with_message(
            unit_variant("Expired", 0),
            "the token expired",
        )],
    ));
    s.enums.push(error_def(
        "UploadError",
        "",
        vec![with_message(unit_variant("Failed", 0), "the upload failed")],
    ));
    s.ports.push(port(
        "UploadListener",
        "Hears about an upload.",
        PortKind::Callback,
        vec![
            MethodDef {
                coalesce: true,
                ..port_method(
                    "UploadListener",
                    "progress",
                    "Bytes sent so far.",
                    vec![param("sent", TypeRef::U64), param("total", TypeRef::U64)],
                    TypeRef::Unit,
                    false,
                )
            },
            port_method(
                "UploadListener",
                "finished",
                "The upload is over.",
                vec![param("name", TypeRef::String)],
                TypeRef::Unit,
                false,
            ),
            port_method(
                "UploadListener",
                "confirm_replace",
                "Asks the user whether to replace an existing file.",
                vec![param("name", TypeRef::String)],
                TypeRef::result(TypeRef::Bool, named("PromptError")),
                true,
            ),
        ],
    ));
    s.ports.push(PortDef {
        background: true,
        ..port(
            "TokenProvider",
            "Provides tokens off the main thread.",
            PortKind::Callback,
            vec![
                port_method(
                    "TokenProvider",
                    "token",
                    "The token for `account`.",
                    vec![param("account", TypeRef::String)],
                    TypeRef::result(TypeRef::String, named("AuthError")),
                    true,
                ),
                port_method(
                    "TokenProvider",
                    "refreshed",
                    "A token was refreshed.",
                    vec![param("account", TypeRef::String)],
                    TypeRef::Unit,
                    false,
                ),
            ],
        )
    });
    s.objects.push(object(
        "Uploader",
        "Uploads files.",
        vec![ctor(
            "Uploader",
            "new",
            vec![param("listener", opt(cb("UploadListener")))],
            false,
        )],
        vec![
            method(
                "Uploader",
                "upload",
                "Uploads `file`, reporting to `listener`.",
                vec![
                    param("file", TypeRef::String),
                    param("listener", cb("UploadListener")),
                ],
                TypeRef::result(TypeRef::U32, named("UploadError")),
                true,
            ),
            method(
                "Uploader",
                "watch",
                "Keeps `listener` until the returned watch is closed.",
                vec![param("listener", cb("UploadListener"))],
                obj("Watch"),
                false,
            ),
            method(
                "Uploader",
                "set_provider",
                "Uses `provider` for the tokens.",
                vec![param("provider", cb("TokenProvider"))],
                TypeRef::Unit,
                false,
            ),
            method(
                "Uploader",
                "notify",
                "Tells the optional `listener` the upload is over.",
                vec![param("listener", opt(cb("UploadListener")))],
                TypeRef::Unit,
                false,
            ),
            method(
                "Uploader",
                "follow",
                "Follows the progress of `watch`, telling `listener`.",
                vec![
                    param("watch", obj("Watch")),
                    param("listener", cb("UploadListener")),
                ],
                TypeRef::Stream(Box::new(TypeRef::U32)),
                false,
            ),
            method(
                "Uploader",
                "follow_checked",
                "Follows the uploads, telling `listener`; a stream that can fail to open.",
                vec![param("listener", opt(cb("UploadListener")))],
                TypeRef::result(
                    TypeRef::Stream(Box::new(TypeRef::U32)),
                    named("UploadError"),
                ),
                false,
            ),
        ],
    ));
    s.objects.push(object(
        "Watch",
        "A subscription.",
        vec![],
        vec![method("Watch", "id", "", vec![], TypeRef::U32, false)],
    ));
    s.functions.push(function(
        "with_listener",
        "Calls `listener` once and returns how many arguments it had.",
        vec![param("listener", cb("UploadListener"))],
        TypeRef::U32,
        false,
    ));
    s.functions.push(function(
        "tail",
        "Tells `listener` about every upload as it happens.",
        vec![param("listener", cb("UploadListener"))],
        TypeRef::Stream(Box::new(TypeRef::U32)),
        false,
    ));
    s
}

// ----- ADR-042 and ADR-043 ------------------------------------------------------

/// Newtypes (ADR-042) in every position: fields, parameters, returns, signals, stream items, errors,
/// port and callback parameters, map keys and a keyed list's key.
fn newtypes() -> Schema {
    let mut s = Schema::new("golden-newtypes");
    s.enums.push(enum_def(
        "Priority",
        "",
        vec![unit_variant("Low", 0), unit_variant("High", 1)],
    ));
    // A newtype of each kind of wrapped type.
    s.records
        .push(newtype_record("UserId", "A user's id.", TypeRef::Uuid));
    s.records
        .push(newtype_record("TodoId", "A todo's id.", TypeRef::String));
    s.records.push(newtype_record("OrderNo", "", TypeRef::U64));
    s.records.push(newtype_record(
        "Meters",
        "A length: it has an order.",
        TypeRef::F64,
    ));
    s.records.push(newtype_record(
        "Span",
        "A newtype of a newtype with an order is ordered too.",
        named("Meters"),
    ));
    s.records
        .push(newtype_record("Timeout", "", TypeRef::Duration));
    s.records
        .push(newtype_record("Created", "", TypeRef::Timestamp));
    s.records.push(newtype_record(
        "Flag",
        "A bool has no order.",
        TypeRef::Bool,
    ));
    s.records.push(newtype_record("Blob", "", TypeRef::Bytes));
    s.records
        .push(newtype_record("Tags", "", vec_of(TypeRef::String)));
    s.records
        .push(newtype_record("Nickname", "", opt(TypeRef::String)));
    s.records
        .push(newtype_record("Level", "", named("Priority")));
    s.records.push(newtype_record(
        "Wrapped",
        "A newtype of a record.",
        named("Todo"),
    ));
    // Newtypes of newtypes, and an option of one.
    s.records.push(newtype_record(
        "Owner",
        "A newtype of a newtype.",
        named("UserId"),
    ));
    s.records
        .push(newtype_record("Boss", "Two levels down.", named("Owner")));
    s.records.push(newtype_record(
        "Maybe",
        "A newtype of an option of a newtype.",
        opt(named("UserId")),
    ));
    s.records.push(newtype_record(
        "Ratings",
        "A newtype of a map with a newtype key.",
        TypeRef::map(named("UserId"), named("Meters")),
    ));
    s.records.push(record(
        "Todo",
        "A todo with a newtype as its key.",
        vec![
            field("id", named("TodoId")),
            field("title", TypeRef::String),
            field("owner", named("UserId")),
            field("due", opt(named("Created"))),
            field("level", named("Level")),
        ],
    ));
    s.records.push(record(
        "Account",
        "Newtypes inside a record, in containers and as map keys.",
        vec![
            field("id", named("UserId")),
            field("nickname", named("Nickname")),
            field("tags", named("Tags")),
            field("boss", opt(named("Boss"))),
            field("friends", vec_of(named("UserId"))),
            field("scores", TypeRef::map(named("UserId"), named("Meters"))),
            field("by_order", TypeRef::map(named("OrderNo"), named("UserId"))),
            field("timeout", named("Timeout")),
            field("blob", named("Blob")),
            field("flag", named("Flag")),
            field("maybe", named("Maybe")),
            field("reach", named("Span")),
        ],
    ));
    s.enums.push(error_def(
        "BoardError",
        "",
        vec![
            with_message(
                tuple_variant("NotFound", 0, vec![named("TodoId")]),
                "todo {0:?} not found",
            ),
            with_message(
                struct_variant("Taken", 1, vec![field("owner", named("UserId"))]),
                "taken by {owner:?}",
            ),
        ],
    ));
    s.ports.push(port(
        "Directory",
        "Looks users up on the host.",
        PortKind::Async,
        vec![port_method(
            "Directory",
            "find",
            "The user called `nickname`.",
            vec![param("nickname", named("Nickname"))],
            err_result(named("UserId"), "BoardError"),
            true,
        )],
    ));
    s.ports.push(port(
        "Listener",
        "Hears about the board.",
        PortKind::Callback,
        vec![port_method(
            "Listener",
            "assigned",
            "A todo changed hands.",
            vec![
                param("todo", named("TodoId")),
                param("owner", named("UserId")),
            ],
            TypeRef::Unit,
            false,
        )],
    ));
    s.objects.push(store(
        object(
            "Board",
            "A board of todos.",
            vec![ctor("Board", "new", vec![], false)],
            vec![
                method(
                    "Board",
                    "assign",
                    "Gives `todo` to `owner`.",
                    vec![
                        param("todo", named("TodoId")),
                        param("owner", named("UserId")),
                    ],
                    err_result(named("Todo"), "BoardError"),
                    true,
                ),
                method(
                    "Board",
                    "owner_of",
                    "",
                    vec![param("todo", named("TodoId"))],
                    opt(named("UserId")),
                    false,
                ),
                method(
                    "Board",
                    "distance",
                    "",
                    vec![param("from", named("Meters")), param("to", named("Meters"))],
                    named("Meters"),
                    false,
                ),
                method(
                    "Board",
                    "changes",
                    "The ids of the todos as they change.",
                    vec![],
                    TypeRef::stream(named("TodoId")),
                    true,
                ),
                method(
                    "Board",
                    "listen",
                    "Tells `listener` about every assignment.",
                    vec![param("listener", cb("Listener"))],
                    TypeRef::Unit,
                    false,
                ),
            ],
        ),
        vec![
            ("todos", vec_of(named("Todo")), false, Some("id")),
            ("owner", named("UserId"), false, None),
            (
                "accounts",
                TypeRef::map(named("UserId"), named("Account")),
                false,
                None,
            ),
            ("limit", named("Timeout"), false, None),
            ("blob", named("Blob"), false, None),
            ("nickname", named("Nickname"), false, None),
        ],
    ));
    s.functions.push(function(
        "make_id",
        "Makes the next order number.",
        vec![param("after", named("OrderNo"))],
        named("OrderNo"),
        false,
    ));
    s.functions.push(function(
        "lookup",
        "Finds an account by id.",
        vec![param("id", named("UserId"))],
        err_result(named("Account"), "BoardError"),
        true,
    ));
    s
}

/// Instantiations of generic templates (ADR-042) as the macro makes them: plain records and enums.
fn generics() -> Schema {
    let mut s = Schema::new("golden-generics");
    s.records.push(record(
        "Todo",
        "",
        vec![field("id", TypeRef::U32), field("title", TypeRef::String)],
    ));
    s.records.push(record(
        "User",
        "",
        vec![field("id", TypeRef::U32), field("name", TypeRef::String)],
    ));
    // `#[undra::api(generic)] struct Page<T> { items: Vec<T>, next: Option<String> }`, twice.
    s.records.push(record(
        "TodoPage",
        "A page of todos: `Page<Todo>`.",
        vec![
            field("items", vec_of(named("Todo"))),
            field("next", opt(TypeRef::String)),
        ],
    ));
    s.records.push(record(
        "UserPage",
        "A page of users: `Page<User>`.",
        vec![
            field("items", vec_of(named("User"))),
            field("next", opt(TypeRef::String)),
        ],
    ));
    // `#[undra::api(generic)] enum Loadable<T> { Loading, Loaded(T), Failed(String) }`, twice.
    for (name, docs, inner) in [
        (
            "LoadableTodo",
            "A todo on its way: `Loadable<Todo>`.",
            "Todo",
        ),
        (
            "LoadableUser",
            "A user on its way: `Loadable<User>`.",
            "User",
        ),
    ] {
        s.enums.push(enum_def(
            name,
            docs,
            vec![
                unit_variant("Loading", 0),
                tuple_variant("Loaded", 1, vec![named(inner)]),
                tuple_variant("Failed", 2, vec![TypeRef::String]),
            ],
        ));
    }
    s.enums.push(error_def(
        "BrowseError",
        "",
        vec![with_message(unit_variant("Offline", 0), "offline")],
    ));
    s.objects.push(store(
        object(
            "Browser",
            "Pages through todos and users.",
            vec![ctor("Browser", "new", vec![], false)],
            vec![
                method(
                    "Browser",
                    "load_todos",
                    "",
                    vec![param("cursor", opt(TypeRef::String))],
                    err_result(named("TodoPage"), "BrowseError"),
                    true,
                ),
                method(
                    "Browser",
                    "load_users",
                    "",
                    vec![param("cursor", opt(TypeRef::String))],
                    err_result(named("UserPage"), "BrowseError"),
                    true,
                ),
                method(
                    "Browser",
                    "select",
                    "",
                    vec![param("todo", named("LoadableTodo"))],
                    TypeRef::Unit,
                    false,
                ),
            ],
        ),
        vec![
            ("todos", named("TodoPage"), false, None),
            ("users", named("UserPage"), false, None),
            ("selected", named("LoadableTodo"), false, None),
            ("owner", named("LoadableUser"), false, None),
        ],
    ));
    s.functions
        .push(function("first_page", "", vec![], named("TodoPage"), false));
    s
}

/// `Decimal` (ADR-042) in a record, in containers, as a map value, wrapped, in a signal, a
/// parameter, a return and a query.
fn decimal() -> Schema {
    let mut s = Schema::new("golden-decimal");
    s.records.push(newtype_record(
        "Price",
        "A price: a decimal with an order.",
        TypeRef::Decimal,
    ));
    s.records.push(record(
        "Invoice",
        "Money amounts of every shape.",
        vec![
            field("total", TypeRef::Decimal),
            field("lines", vec_of(TypeRef::Decimal)),
            field("discount", opt(TypeRef::Decimal)),
            field("price", named("Price")),
            field("by_sku", TypeRef::map(TypeRef::String, TypeRef::Decimal)),
            field("prices", TypeRef::map(TypeRef::U32, named("Price"))),
            default_field("tax", TypeRef::Decimal),
        ],
    ));
    s.enums.push(error_def(
        "PayError",
        "",
        vec![with_message(unit_variant("Declined", 0), "declined")],
    ));
    s.objects.push(store(
        object(
            "Till",
            "Takes payments.",
            vec![ctor("Till", "new", vec![], false)],
            vec![
                method(
                    "Till",
                    "charge",
                    "Takes `amount` and returns the balance.",
                    vec![param("amount", TypeRef::Decimal)],
                    err_result(TypeRef::Decimal, "PayError"),
                    true,
                ),
                method(
                    "Till",
                    "refund",
                    "",
                    vec![param("amount", opt(TypeRef::Decimal))],
                    TypeRef::Unit,
                    false,
                ),
                method(
                    "Till",
                    "totals",
                    "",
                    vec![],
                    TypeRef::stream(TypeRef::Decimal),
                    true,
                ),
            ],
        ),
        vec![
            ("balance", TypeRef::Decimal, false, None),
            ("last", opt(named("Price")), false, None),
            ("history", vec_of(TypeRef::Decimal), false, None),
            ("invoice", named("Invoice"), false, None),
        ],
    ));
    s.functions.push(function(
        "quote",
        "Prices `qty` items.",
        vec![param("qty", TypeRef::U32)],
        named("Invoice"),
        false,
    ));
    s.queries.push(query(
        "price_of",
        QueryKind::Query,
        "price/{sku}",
        vec![param("sku", TypeRef::String)],
        err_result(TypeRef::Decimal, "PayError"),
        Some(60_000),
    ));
    s
}

/// Queries that poll (ADR-043): `interval` with and without `poll_in_background`, and one that
/// does not poll; every handle can set its own interval.
fn polling() -> Schema {
    let mut s = Schema::new("golden-polling");
    s.enums.push(error_def(
        "MarketError",
        "",
        vec![with_message(unit_variant("Offline", 0), "offline")],
    ));
    s.records.push(record(
        "Quote",
        "",
        vec![
            field("symbol", TypeRef::String),
            field("price", TypeRef::F64),
        ],
    ));
    let mut ticker = query(
        "ticker",
        QueryKind::Query,
        "ticker/{symbol}",
        vec![param("symbol", TypeRef::String)],
        err_result(named("Quote"), "MarketError"),
        Some(2_000),
    );
    ticker.interval_ms = Some(5_000);
    s.queries.push(ticker);
    let mut headlines = query(
        "headlines",
        QueryKind::Query,
        "headlines",
        vec![],
        err_result(vec_of(TypeRef::String), "MarketError"),
        Some(30_000),
    );
    headlines.interval_ms = Some(60_000);
    headlines.poll_in_background = true;
    s.queries.push(headlines);
    s.queries.push(query(
        "status",
        QueryKind::Query,
        "status",
        vec![],
        err_result(TypeRef::Bool, "MarketError"),
        None,
    ));
    s.queries.push(query(
        "buy",
        QueryKind::Mutation,
        "orders",
        vec![param("symbol", TypeRef::String)],
        err_result(named("Quote"), "MarketError"),
        None,
    ));
    s
}

/// Infinite queries (ADR-043): a feed whose rows are keyed by `id` (a newtype), and a search
/// polled every 30 s whose rows are keyed by another field.
fn infinite() -> Schema {
    let mut s = Schema::new("golden-infinite");
    s.enums.push(enum_def(
        "Filter",
        "",
        vec![unit_variant("All", 0), unit_variant("Mine", 1)],
    ));
    s.records
        .push(newtype_record("PostId", "A post's id.", TypeRef::U64));
    s.records.push(record(
        "Post",
        "One post of the feed.",
        vec![
            field("id", named("PostId")),
            field("author", TypeRef::String),
            field("body", TypeRef::String),
        ],
    ));
    s.records.push(record(
        "Hit",
        "One search result.",
        vec![
            field("slug", TypeRef::String),
            field("title", TypeRef::String),
        ],
    ));
    let mut feed = query(
        "feed",
        QueryKind::Query,
        "feed/{filter}",
        vec![param("filter", named("Filter"))],
        vec_of(named("Post")),
        Some(60_000),
    );
    feed.infinite = Some(InfiniteDef {
        cursor: TypeRef::String,
        item_key: "id".into(),
    });
    s.queries.push(feed);
    let mut search = query(
        "search",
        QueryKind::Query,
        "search/{term}",
        vec![param("term", TypeRef::String)],
        vec_of(named("Hit")),
        None,
    );
    search.infinite = Some(InfiniteDef {
        cursor: TypeRef::U32,
        item_key: "slug".into(),
    });
    search.interval_ms = Some(30_000);
    search.persist = true;
    s.queries.push(search);
    s.queries.push(query(
        "profile",
        QueryKind::Query,
        "profile",
        vec![],
        TypeRef::String,
        None,
    ));
    s
}

/// `Lazy<T>` signals (ADR-043): a keyed one, a derived one, next to ordinary signals.
fn lazy() -> Schema {
    let mut s = Schema::new("golden-lazy");
    s.records.push(record(
        "Book",
        "One book of the library.",
        vec![
            field("id", TypeRef::U64),
            field("title", TypeRef::String),
            field("year", TypeRef::U16),
        ],
    ));
    s.records.push(record(
        "Shelf",
        "",
        vec![field("name", TypeRef::String), field("count", TypeRef::U32)],
    ));
    s.records.push(record(
        "Chapter",
        "",
        vec![
            field("number", TypeRef::U32),
            field("title", TypeRef::String),
        ],
    ));
    s.enums.push(error_def(
        "LibraryError",
        "",
        vec![with_message(unit_variant("Missing", 0), "missing")],
    ));
    s.objects.push(store(
        object(
            "Library",
            "A library with more books than a screen holds.",
            vec![ctor("Library", "new", vec![], false)],
            vec![
                method(
                    "Library",
                    "add",
                    "Adds a book.",
                    vec![param("title", TypeRef::String)],
                    err_result(named("Book"), "LibraryError"),
                    true,
                ),
                method("Library", "clear", "", vec![], TypeRef::Unit, false),
            ],
        ),
        vec![
            ("shelves", vec_of(named("Shelf")), false, Some("name")),
            ("books", TypeRef::lazy(named("Book")), false, Some("id")),
            ("selected", opt(named("Book")), false, None),
            ("recent", TypeRef::lazy(named("Book")), true, None),
            ("total", TypeRef::U64, false, None),
        ],
    ));
    // A store that is nothing but a lazy list.
    s.objects.push(store(
        object(
            "Archive",
            "Chapters, paged.",
            vec![ctor("Archive", "new", vec![], false)],
            vec![],
        ),
        vec![("chapters", TypeRef::lazy(named("Chapter")), false, None)],
    ));
    s
}

/// Generic functions and methods (ADR-058): one definition per listed type, labelled. A free family
/// whose type parameter the arguments fix (`newest`), one they do not (`draft`), an `async` one that
/// takes the context and has an error type (`load`), a stream (`rows`), a command (`forget`), an
/// `async` one that returns nothing (`save`), a parameter called `type` (`make`), a family of methods
/// on an object (`pinned`, `remember`) and on a store (`open`, `tidy`).
fn generic_functions() -> Schema {
    let mut s = Schema::new("golden-generic-functions");
    s.records.push(record(
        "Todo",
        "A todo.",
        vec![field("id", TypeRef::U32), field("title", TypeRef::String)],
    ));
    s.records.push(record(
        "Note",
        "A note.",
        vec![field("id", TypeRef::U32), field("body", TypeRef::String)],
    ));
    s.enums.push(error_def(
        "LoadError",
        "",
        vec![
            with_message(
                tuple_variant("Missing", 0, vec![TypeRef::U32]),
                "no row {0}",
            ),
            with_message(unit_variant("Offline", 1), "offline"),
        ],
    ));
    let rows = ["Todo", "Note"];
    for arg in rows {
        s.functions.push(instance_fn(
            "newest",
            arg,
            true,
            "The row that changed last, if there is one.",
            vec![param("rows", vec_of(named(arg)))],
            opt(named(arg)),
            false,
        ));
    }
    for arg in rows {
        s.functions.push(instance_fn(
            "draft",
            arg,
            false,
            "An empty row with a fresh id, ready to edit.",
            vec![],
            named(arg),
            false,
        ));
    }
    for arg in rows {
        let mut load = instance_fn(
            "load",
            arg,
            false,
            "Loads a row from the server.",
            vec![param("id", TypeRef::U32)],
            err_result(named(arg), "LoadError"),
            true,
        );
        load.takes_ctx = true;
        s.functions.push(load);
    }
    for arg in rows {
        s.functions.push(instance_fn(
            "rows",
            arg,
            false,
            "A stream of the first rows.",
            vec![param("count", TypeRef::U32)],
            TypeRef::stream(named(arg)),
            false,
        ));
    }
    for arg in rows {
        s.functions.push(instance_fn(
            "forget",
            arg,
            true,
            "Drops a row from the cache.",
            vec![param("row", named(arg))],
            TypeRef::Unit,
            false,
        ));
    }
    for arg in rows {
        s.functions.push(instance_fn(
            "save",
            arg,
            true,
            "Writes a row back.",
            vec![param("row", named(arg))],
            err_result(TypeRef::Unit, "LoadError"),
            true,
        ));
    }
    for arg in rows {
        s.functions.push(instance_fn(
            "make",
            arg,
            false,
            "Makes a row of the given kind: a parameter called `type` does not collide with the type token.",
            vec![param("type", TypeRef::String)],
            named(arg),
            false,
        ));
    }
    // A function that is not generic, between the families.
    s.functions.push(function(
        "count_rows",
        "How many rows there are.",
        vec![],
        TypeRef::U32,
        false,
    ));
    s.objects.push(object(
        "Library",
        "A shelf of rows.",
        vec![ctor("Library", "new", vec![], false)],
        vec![
            instance_method(
                "Library",
                "pinned",
                "Todo",
                false,
                "The pinned rows of a type.",
                vec![],
                vec_of(named("Todo")),
                false,
            ),
            instance_method(
                "Library",
                "pinned",
                "Note",
                false,
                "The pinned rows of a type.",
                vec![],
                vec_of(named("Note")),
                false,
            ),
            instance_method(
                "Library",
                "remember",
                "Todo",
                true,
                "Remembers a row and answers how many there are.",
                vec![param("row", named("Todo"))],
                TypeRef::U32,
                true,
            ),
            instance_method(
                "Library",
                "remember",
                "Note",
                true,
                "Remembers a row and answers how many there are.",
                vec![param("row", named("Note"))],
                TypeRef::U32,
                true,
            ),
            method(
                "Library",
                "count",
                "How many rows are pinned.",
                vec![],
                TypeRef::U32,
                false,
            ),
        ],
    ));
    s.objects.push(store(
        object(
            "Shelf",
            "A store with generic methods.",
            vec![ctor("Shelf", "new", vec![], false)],
            vec![
                instance_method(
                    "Shelf",
                    "tidy",
                    "Todo",
                    true,
                    "Sorts rows.",
                    vec![param("rows", vec_of(named("Todo")))],
                    TypeRef::Unit,
                    false,
                ),
                instance_method(
                    "Shelf",
                    "tidy",
                    "Note",
                    true,
                    "Sorts rows.",
                    vec![param("rows", vec_of(named("Note")))],
                    TypeRef::Unit,
                    false,
                ),
            ],
        ),
        vec![("size", TypeRef::U32, false, None)],
    ));
    s
}

/// What generic objects and stores produce (ADR-058): the `ObjectDef`s of two aliases of an object
/// (`Cache<T>`) and of a store (`Selection<T>`), which are byte for byte what hand-written objects
/// of those names produce (locking that no generator changes), and the functions that name them
/// through their alias (`open_selection<T>`, `ticked<T>`, `page_of<T>`).
fn generic_objects() -> Schema {
    let mut s = Schema::new("golden-generic-objects");
    s.records.push(record(
        "Todo",
        "A todo.",
        vec![field("id", TypeRef::U32), field("title", TypeRef::String)],
    ));
    s.records.push(record(
        "Note",
        "A note.",
        vec![field("id", TypeRef::U32), field("body", TypeRef::String)],
    ));
    s.records.push(record(
        "TodoPage",
        "A page of todos: `Page<Todo>`.",
        vec![
            field("items", vec_of(named("Todo"))),
            field("next", opt(TypeRef::String)),
        ],
    ));
    s.records.push(record(
        "NotePage",
        "A page of notes: `Page<Note>`.",
        vec![
            field("items", vec_of(named("Note"))),
            field("next", opt(TypeRef::String)),
        ],
    ));
    for (alias, row) in [("TodoCache", "Todo"), ("NoteCache", "Note")] {
        s.objects.push(object(
            alias,
            "Keeps rows by id.",
            vec![ctor(alias, "new", vec![], false)],
            vec![
                method(
                    alias,
                    "put",
                    "Stores a row, replacing the one with its id.",
                    vec![param("row", named(row))],
                    TypeRef::Unit,
                    false,
                ),
                method(
                    alias,
                    "get",
                    "",
                    vec![param("id", TypeRef::U32)],
                    opt(named(row)),
                    false,
                ),
                method(alias, "all", "", vec![], vec_of(named(row)), true),
            ],
        ));
    }
    for (alias, row) in [("TodoSelection", "Todo"), ("NoteSelection", "Note")] {
        s.objects.push(store(
            object(
                alias,
                "The rows the user has ticked.",
                vec![ctor(
                    alias,
                    "new",
                    vec![param("limit", TypeRef::U32)],
                    false,
                )],
                vec![
                    method(
                        alias,
                        "toggle",
                        "Ticks a row, or unticks it.",
                        vec![param("row", named(row))],
                        TypeRef::Unit,
                        false,
                    ),
                    method(
                        alias,
                        "contains",
                        "",
                        vec![param("id", TypeRef::U32)],
                        TypeRef::Bool,
                        false,
                    ),
                    method(alias, "clear", "", vec![], TypeRef::Unit, false),
                ],
            ),
            vec![
                ("rows", vec_of(named(row)), false, Some("id")),
                ("count", TypeRef::U32, true, None),
            ],
        ));
    }
    for arg in ["Todo", "Note"] {
        s.functions.push(instance_fn(
            "open_selection",
            arg,
            false,
            "A fresh selection of rows.",
            vec![param("limit", TypeRef::U32)],
            obj(&format!("{arg}Selection")),
            false,
        ));
    }
    for arg in ["Todo", "Note"] {
        s.functions.push(instance_fn(
            "ticked",
            arg,
            true,
            "How many rows the selection holds.",
            vec![param("selection", obj(&format!("{arg}Selection")))],
            TypeRef::U32,
            false,
        ));
    }
    for arg in ["Todo", "Note"] {
        s.functions.push(instance_fn(
            "page_of",
            arg,
            true,
            "Wraps rows in a page.",
            vec![param("rows", vec_of(named(arg)))],
            named(&format!("{arg}Page")),
            false,
        ));
    }
    s
}
