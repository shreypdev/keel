//! Getting the schema: from a built core library (`dlopen`) or from a JSON file.
//!
//! The schema is the only truth (constitution R1): everything `undra bindgen` writes derives from
//! it. `docs/SPEC.md` section 13 says how to get it: build the core as a cdylib, `dlopen` it, look
//! up its one export `<namespace>_undra_api` (C ABI version 2, ADR-044), and call the table's
//! `schema_json`. This is the only module of the CLI that uses `unsafe`: calling into a library
//! loaded at run time cannot be checked by the compiler, so each call says why it is sound.
//!
//! The library returns the whole schema, doc comments and labels included (`Schema::to_json`,
//! ADR-050), so `undra bindgen --docs` needs no second build. The hash does not cover docs, so the
//! loader checks the table's own `schema_hash` against the hash of the JSON it parsed.
//! [`parse_schema_json`] accepts that, the full JSON of `Schema::to_json_pretty` (what a
//! `schema.json` file holds) and the label-free canonical form (a core built before the export
//! carried docs), adding the labels the canonical form leaves out. A core of that age still loads,
//! but not for `--docs`: it has none to give, and bindings without them are refused rather than
//! written as if the source had no comments.

#![allow(unsafe_code)]

use std::ffi::{CStr, c_char};
use std::path::Path;

use undra_meta::Schema;

use crate::error::{CliError, Code, Result};

/// The C ABI version this CLI speaks (the `abi_version` of a core's `UndraApi` table).
pub const ABI_VERSION: u32 = 2;

/// The `UndraBuf` of SPEC 6: a buffer the core owns, freed with the table's `buf_free`.
#[repr(C)]
struct UndraBuf {
    ptr: *mut u8,
    len: u32,
    cap: u32,
}

/// The head of the `UndraApi` table of `undra.h` (version 2), as far as the loader reads it: the
/// two constants, the namespace and the first entry, then the rest of the 17 entries, whose
/// layout is pinned by `undra-ffi`'s tests. Only `schema_json` and `buf_free` are called.
#[repr(C)]
struct UndraApi {
    abi_version: u32,
    size: u32,
    schema_hash: u64,
    name_space: *const c_char,
    schema_json: unsafe extern "C" fn() -> UndraBuf,
    /// `init` .. `stats_json`: never called here.
    _entries: [*const (); 15],
    buf_free: unsafe extern "C" fn(UndraBuf),
}

/// `const UndraApi *<namespace>_undra_api(void)`.
type ApiFn = unsafe extern "C" fn() -> *const UndraApi;

/// Loads the core library at `library` and reads its schema through `<namespace>_undra_api`.
/// `crate_name` labels the result: the library only knows itself as `undra-core`. With `docs` the
/// schema keeps the doc comments the library exports; without, they are dropped (generated bindings
/// carry none by default).
///
/// # Errors
///
/// `C0006` when the library cannot be loaded, does not export `<namespace>_undra_api` (another
/// namespace, or a core of C ABI version 1), its table is of another ABI version or names another
/// namespace, it reports JSON that is not UTF-8 or does not parse, its schema's hash does not match
/// the table's, or, with `docs`, it exports the docless canonical form of a core built before
/// `schema_json` carried the doc comments.
pub fn load_from_library(
    library: &Path,
    namespace: &str,
    crate_name: &str,
    docs: bool,
) -> Result<Schema> {
    let fail = |what: String, why: &str, fix: &str| CliError::new(Code::Schema, what, why, fix);

    // SAFETY: loading a library runs its initialisers. The library is the one this process just
    // built from the user's own core (or one they named), and `inventory` registration, which is
    // all those initialisers do, is what reading the schema needs.
    let lib = unsafe { libloading::Library::new(library) }.map_err(|e| {
        fail(
            format!("cannot load {}: {e}", library.display()),
            "the schema is read from the built core library (docs/SPEC.md 13), which has to be loadable on this machine",
            "build the core for this machine with `undra build --platform host` and check that the file exists",
        )
    })?;

    let symbol = format!("{namespace}_undra_api");
    let mut name = symbol.clone().into_bytes();
    name.push(0);
    // SAFETY: `<namespace>_undra_api` is the one export of a core (C ABI version 2, docs/SPEC.md
    // 6, `undra_ffi::export_core!`), `extern "C" fn() -> const UndraApi *`; the function pointer
    // does not outlive `lib`, which is intentionally leaked below.
    let entry: libloading::Symbol<'_, ApiFn> = unsafe { lib.get(&name) }.map_err(|_| {
        let v1 = {
            // SAFETY: only resolves a symbol to learn whether it exists; nothing is called.
            unsafe { lib.get::<*const ()>(b"undra_abi_version\0") }.is_ok()
        };
        if v1 {
            fail(
                format!("{} is an Undra core of C ABI version 1", library.display()),
                "it exports the global `undra_*` functions; this undra-cli reads a core's table (C ABI version 2, ADR-044)",
                "rebuild the core with the `undra` version of this undra-cli (`undra build --platform host`)",
            )
        } else {
            fail(
                format!("{} has no `{symbol}`", library.display()),
                "an Undra core exports one function named after its namespace (`[core] namespace` in undra.toml, default the core's package name); this library is not the core of that namespace",
                "build it with `undra build` (which links `undra-ffi` and exports the core under its namespace), or pass a schema file with `undra bindgen --schema`",
            )
        }
    })?;
    // SAFETY: the entry takes no arguments and returns a pointer to the core's immutable table,
    // valid for the life of the library (which is never unloaded).
    let api = unsafe { entry() };
    if api.is_null() {
        return Err(fail(
            format!("`{symbol}` of {} returned no table", library.display()),
            "a core's entry returns its C ABI table, never null",
            "rebuild the core with `undra build --platform host`",
        ));
    }
    // SAFETY: `abi_version` is the first field of every version of the table; read it before
    // trusting any other field.
    let abi = unsafe { (*api).abi_version };
    if abi != ABI_VERSION {
        return Err(fail(
            format!(
                "the core library speaks C ABI version {abi}; this undra-cli speaks {ABI_VERSION}"
            ),
            "the two were built from different Undra releases and cannot talk to each other",
            "use the undra-cli that matches the `undra` version of the core (`cargo install undra-cli --version <undra version>`)",
        ));
    }
    // SAFETY: a version 2 table: `size` is its second field.
    let size = unsafe { (*api).size } as usize;
    if size < std::mem::size_of::<UndraApi>() {
        return Err(fail(
            format!(
                "the C ABI table of {} is {size} bytes, too small for version 2",
                library.display()
            ),
            "the library's table does not have the layout undra.h version 2 declares",
            "rebuild the core with the `undra` version of this undra-cli",
        ));
    }
    // SAFETY: the table has at least the fields of `UndraApi` (checked above); it is immutable and
    // lives as long as the library.
    let api = unsafe { &*api };
    // SAFETY: `name_space` is a static NUL-terminated string (undra.h).
    let own = unsafe { CStr::from_ptr(api.name_space) }.to_string_lossy();
    if own != namespace {
        return Err(fail(
            format!(
                "`{symbol}` of {} says its namespace is `{own}`",
                library.display()
            ),
            "the exported entry and the table must name the same core",
            "rebuild the core with `undra build --platform host`",
        ));
    }

    let hash = api.schema_hash;
    // SAFETY: `schema_json` takes no arguments and returns an owned `UndraBuf`; its bytes are
    // valid for `len` bytes until `buf_free`, which is called below on the same buffer.
    let buf = unsafe { (api.schema_json)() };
    // Copied out (strictly decoded: the docs are not covered by the hash, so a damaged byte in one
    // would pass the check below) before the buffer goes back to the core.
    let text = if buf.ptr.is_null() {
        Ok(String::new())
    } else {
        // SAFETY: `ptr` is non-null and the core promises `len` initialised bytes at it.
        let bytes = unsafe { std::slice::from_raw_parts(buf.ptr, buf.len as usize) };
        std::str::from_utf8(bytes).map(str::to_owned)
    };
    // SAFETY: `buf` came from `schema_json` of this table and is freed exactly once.
    unsafe { (api.buf_free)(buf) };

    // A Rust cdylib that has started threads or thread-locals does not always survive `dlclose`;
    // the process is short-lived, so keep the library mapped instead.
    std::mem::forget(lib);

    let text = text.map_err(|e| {
        fail(
            format!("the core library's schema is not UTF-8: {e}"),
            "the table's `schema_json` returns UTF-8 JSON (docs/SPEC.md 6); the library is damaged or is not what `undra build` wrote",
            "rebuild the core with `undra build --platform host` and run `undra bindgen` again",
        )
    })?;
    let (schema, export) = from_library_json(&text, crate_name).map_err(|e| {
        fail(
            format!("the core library's schema cannot be read: {}", e.what),
            "the table's `schema_json` returned JSON this undra-cli does not understand (a newer `undra-meta`?)",
            "update undra-cli to the version of Undra the core uses",
        )
    })?;
    let computed = schema.hash();
    if computed != hash {
        return Err(fail(
            format!(
                "the schema hash does not match: the library says {hash:#018x}, its JSON hashes to {computed:#018x}"
            ),
            "undra-cli reads the schema with its own copy of `undra-meta`, and the core's differs in what it puts in the canonical form",
            "use the undra-cli that matches the `undra` version of the core",
        ));
    }
    with_docs_or_without(schema, export, docs, library)
}

/// The library's schema with its doc comments when `docs` is asked for, without them otherwise.
/// A library that exported the canonical form has none to give, and `--docs` says so instead of
/// writing bindings that look as if the Rust source had no comments.
fn with_docs_or_without(
    schema: Schema,
    export: Export,
    docs: bool,
    library: &Path,
) -> Result<Schema> {
    match (docs, export) {
        (false, _) => Ok(schema.without_docs()),
        (true, Export::Whole) => Ok(schema),
        (true, Export::Canonical) => Err(CliError::new(
            Code::Schema,
            format!(
                "{} exports its schema without doc comments, so `--docs` has nothing to write",
                library.display()
            ),
            "the core was built with an `undra-ffi` older than this undra-cli: its `undra_schema_json` returns the canonical form the hash covers, which leaves the docs out (ADR-050)",
            "update the core's `undra` dependency to the version of this undra-cli (`cargo update -p undra`), or run `undra bindgen` without --docs",
        )),
    }
}

/// Which form a core library's `undra_schema_json` returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Export {
    /// The whole schema, labels and doc comments included (`Schema::to_json`, ADR-050).
    Whole,
    /// The canonical form, without labels or docs: a core built before ADR-050.
    Canonical,
}

/// The schema a core library's `undra_schema_json` returned, and which form it came in. Its
/// `crate_name` label is replaced by `crate_name` (the library's own is the generic
/// `undra-core`); everything else, docs included, is the library's.
fn from_library_json(text: &str, crate_name: &str) -> Result<(Schema, Export)> {
    let mut schema = parse_schema_json(text, crate_name)?;
    // `Schema::to_json` always writes the `undra_version` label; the canonical form never does.
    let labelled = serde_json::from_str::<serde_json::Value>(text)
        .is_ok_and(|value| value.get("undra_version").is_some());
    crate_name.clone_into(&mut schema.crate_name);
    let export = if labelled {
        Export::Whole
    } else {
        Export::Canonical
    };
    Ok((schema, export))
}

/// Whether each of `symbols` is an exported, resolvable symbol of the core library at `library`.
///
/// Loads the library the way a platform runtime does (`dlopen`) and looks each name up
/// (`dlsym`), returning one bool per input name in order. Used to check what a built cdylib
/// exports: its table entry `<namespace>_undra_api` and `JNI_OnLoad` (ADR-044), and none of the
/// global `undra_*` functions of C ABI version 1.
///
/// # Errors
///
/// `C0006` when the library cannot be loaded at all.
pub fn symbols_present(library: &Path, symbols: &[&str]) -> Result<Vec<bool>> {
    // SAFETY: loading the core library runs its initialisers, exactly as reading its schema does
    // (see `load_from_library`); it is a library this process just built.
    let lib = unsafe { libloading::Library::new(library) }.map_err(|e| {
        CliError::new(
            Code::Schema,
            format!("cannot load {}: {e}", library.display()),
            "the library has to be loadable to check its exported symbols",
            "build it with `undra build --platform host`",
        )
    })?;
    let found = symbols
        .iter()
        .map(|name| {
            let mut c = name.as_bytes().to_vec();
            c.push(0);
            // SAFETY: `get` only resolves a symbol; the returned pointer is not called, and it does
            // not outlive `lib`, which is leaked below.
            unsafe { lib.get::<*const ()>(&c) }.is_ok()
        })
        .collect();
    // A Rust cdylib does not always survive `dlclose`; keep it mapped (the process is short-lived).
    std::mem::forget(lib);
    Ok(found)
}

/// Parses schema JSON: full (`Schema::to_json` from `undra_schema_json`, or `to_json_pretty`) or
/// canonical (`Schema::canonical_json`). `crate_name` is used when the JSON has no `crate_name`
/// label.
///
/// # Errors
///
/// `C0002` with the JSON error (line and column included) when it does not describe a schema.
pub fn parse_schema_json(text: &str, crate_name: &str) -> Result<Schema> {
    let bad = |what: String| {
        CliError::new(
            Code::BadConfig,
            what,
            "a schema file is what the bindings are generated from, so it has to be exactly the JSON Undra writes",
            "regenerate it: `undra bindgen` extracts it from the built core, and `Schema::to_json_pretty` writes one",
        )
    };
    let mut value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| bad(format!("the schema is not valid JSON: {e}")))?;
    let Some(object) = value.as_object_mut() else {
        return Err(bad("the schema must be a JSON object".to_owned()));
    };
    let defaults = Schema::new(crate_name);
    object
        .entry("undra_version")
        .or_insert_with(|| serde_json::Value::String(defaults.undra_version.clone()));
    object
        .entry("crate_name")
        .or_insert_with(|| serde_json::Value::String(crate_name.to_owned()));
    let labelled = serde_json::to_string(&value).map_err(|e| bad(e.to_string()))?;
    let mut schema = Schema::from_json(&labelled)
        .map_err(|e| bad(format!("the schema does not have the expected shape: {e}")))?;
    normalize(&mut schema);
    Ok(schema)
}

/// Orders the lists whose order is not part of the wire layout the way the canonical form does
/// (methods, constructors and port methods by name, variants by index), so the same core
/// generates the same files whichever way its schema was obtained: a canonical JSON is sorted, the
/// registrations in a library's or the dev runner's full JSON are in declaration order.
pub fn normalize(schema: &mut Schema) {
    for en in &mut schema.enums {
        en.variants.sort_by_key(|v| v.index);
    }
    for object in &mut schema.objects {
        object.constructors.sort_by(|a, b| a.name.cmp(&b.name));
        object.methods.sort_by(|a, b| a.name.cmp(&b.name));
    }
    for port in &mut schema.ports {
        port.methods.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

#[cfg(test)]
mod tests {
    use undra_meta::{FieldDef, RecordDef, TypeRef, ids};

    use super::*;

    fn sample() -> Schema {
        let mut schema = Schema::new("demo-core");
        schema.records.push(RecordDef {
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
                    name: "done".into(),
                    ty: TypeRef::Bool,
                    default: true,
                    docs: String::new(),
                },
            ],
            docs: "An item.".into(),
        });
        schema
    }

    #[test]
    fn full_json_round_trips() {
        let schema = sample();
        let back = parse_schema_json(&schema.to_json_pretty(), "ignored").unwrap();
        assert_eq!(back, schema);
    }

    #[test]
    fn a_library_schema_keeps_its_docs_and_takes_the_callers_label() {
        // What `undra_schema_json` returns: compact, labelled `undra-core`, docs included.
        let mut schema = sample();
        "undra-core".clone_into(&mut schema.crate_name);
        let (back, export) = from_library_json(&schema.to_json(), "demo-core").unwrap();
        assert_eq!(export, Export::Whole);
        assert_eq!(back.crate_name, "demo-core");
        assert_eq!(back.records[0].docs, "An item.");
        assert_eq!(back.records[0].fields[0].docs, "The title.");
        assert_eq!(back.hash(), schema.hash(), "the docs are not in the hash");
        assert_eq!(back.without_docs().hash(), schema.hash());
        // A core without a single doc comment still exports the whole form: the labels say so.
        let bare = sample().without_docs();
        let (_, export) = from_library_json(&bare.to_json(), "demo-core").unwrap();
        assert_eq!(export, Export::Whole);
    }

    #[test]
    fn a_library_from_before_the_export_carried_docs_still_loads() {
        // `load_from_library` refuses this form for `--docs` (it has none to give) and accepts it
        // otherwise.
        let schema = sample();
        let (back, export) = from_library_json(&schema.canonical_json(), "demo-core").unwrap();
        assert_eq!(export, Export::Canonical);
        assert_eq!(back.crate_name, "demo-core");
        assert_eq!(back.records[0].docs, "");
        assert_eq!(back.hash(), schema.hash());
    }

    #[test]
    fn docs_are_kept_only_when_asked_for_and_refused_when_the_library_has_none() {
        let library = Path::new("/build/host/libundra_core.dylib");
        let kept = with_docs_or_without(sample(), Export::Whole, true, library).unwrap();
        assert_eq!(kept.records[0].docs, "An item.");
        let dropped = with_docs_or_without(sample(), Export::Whole, false, library).unwrap();
        assert_eq!(dropped, sample().without_docs());
        // A core from before ADR-050 still generates its (docless) bindings by default ...
        let old = sample().without_docs();
        assert_eq!(
            with_docs_or_without(old.clone(), Export::Canonical, false, library).unwrap(),
            old
        );
        // ... but `--docs` on it is an error that says why and what to do, not silently docless
        // bindings.
        let e = with_docs_or_without(old, Export::Canonical, true, library).unwrap_err();
        assert_eq!(e.code, Code::Schema);
        assert!(
            e.what.contains("--docs") && e.what.contains("libundra_core"),
            "{e}"
        );
        assert!(e.why.contains("older"), "{e}");
        assert!(e.fix.contains("cargo update -p undra"), "{e}");
    }

    #[test]
    fn canonical_json_gets_its_labels_and_keeps_the_hash() {
        let schema = sample();
        let back = parse_schema_json(&schema.canonical_json(), "demo-core").unwrap();
        assert_eq!(back.crate_name, "demo-core");
        assert_eq!(back.records[0].name, "Todo");
        assert_eq!(back.records[0].docs, "", "canonical JSON carries no docs");
        assert_eq!(back.hash(), schema.hash());
    }

    #[test]
    fn the_same_core_generates_the_same_order_whichever_way_its_schema_arrives() {
        use undra_meta::{MethodDef, ObjectDef};
        let method = |name: &str| MethodDef {
            name: name.into(),
            method_id: ids::method_id("Calc", name),
            params: vec![],
            returns: TypeRef::Unit,
            is_async: false,
            takes_ctx: false,
            docs: format!("Docs of {name}."),
        };
        let mut schema = Schema::new("demo-core");
        schema.objects.push(ObjectDef {
            name: "Calc".into(),
            type_id: ids::type_id("Calc"),
            constructors: vec![method("new")],
            methods: vec![method("zeta"), method("alpha")],
            store: None,
            docs: String::new(),
        });
        let from_full = parse_schema_json(&schema.to_json_pretty(), "x").unwrap();
        let from_canonical = parse_schema_json(&schema.canonical_json(), "demo-core").unwrap();
        let names = |s: &Schema| {
            s.objects[0]
                .methods
                .iter()
                .map(|m| m.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&from_full), ["alpha", "zeta"]);
        assert_eq!(names(&from_full), names(&from_canonical));
        assert_eq!(from_full.hash(), from_canonical.hash());
    }

    #[test]
    fn unparseable_schemas_are_explained() {
        for (text, needle) in [
            ("", "not valid JSON"),
            ("[]", "must be a JSON object"),
            ("{\"records\": 3}", "expected shape"),
        ] {
            let e = parse_schema_json(text, "x").unwrap_err();
            assert_eq!(e.code, Code::BadConfig, "{text}");
            assert!(e.what.contains(needle), "{text}: {e}");
            assert!(e.fix.contains("undra bindgen"), "{e}");
        }
    }

    #[test]
    fn a_library_that_is_not_there_is_explained() {
        let e = load_from_library(Path::new("/definitely/not/here.dylib"), "x", "x", false)
            .unwrap_err();
        assert_eq!(e.code, Code::Schema);
        assert!(e.what.contains("cannot load"), "{e}");
        assert!(e.fix.contains("undra build --platform host"), "{e}");
    }
}
