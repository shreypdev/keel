//! Getting the schema: from a built core library (`dlopen`) or from a JSON file.
//!
//! The schema is the only truth (constitution R1): everything `keel bindgen` writes derives from
//! it. `docs/SPEC.md` section 13 says how to get it: build the core as a cdylib, `dlopen` it, and
//! call `keel_schema_json`. This is the only module of the CLI that uses `unsafe`: calling into a
//! library loaded at run time cannot be checked by the compiler, so each call says why it is sound.
//!
//! The library returns the **canonical** JSON (docs stripped, no labels, lists sorted) because
//! that is what the schema hash is computed over. [`parse_schema_json`] accepts that and the full
//! JSON of `Schema::to_json_pretty` (what a `schema.json` file holds), adding the labels the
//! canonical form leaves out.

#![allow(unsafe_code)]

use std::path::Path;

use keel_meta::Schema;

use crate::error::{CliError, Code, Result};

/// The C ABI version this CLI speaks (`keel_abi_version`).
pub const ABI_VERSION: u32 = 1;

/// The `KeelBuf` of SPEC 6: a buffer the core owns, freed with `keel_buf_free`.
#[repr(C)]
struct KeelBuf {
    ptr: *mut u8,
    len: u32,
    cap: u32,
}

type AbiVersionFn = unsafe extern "C" fn() -> u32;
type SchemaHashFn = unsafe extern "C" fn() -> u64;
type SchemaJsonFn = unsafe extern "C" fn() -> KeelBuf;
type BufFreeFn = unsafe extern "C" fn(KeelBuf);

/// Loads the core library at `library` and reads its schema. `crate_name` labels the result
/// (the canonical JSON carries no labels).
///
/// # Errors
///
/// `C0006` when the library cannot be loaded, is not a Keel core of this ABI version, reports
/// JSON that does not parse, or reports a schema whose hash does not match its own.
pub fn load_from_library(library: &Path, crate_name: &str) -> Result<Schema> {
    let fail = |what: String, why: &str, fix: &str| CliError::new(Code::Schema, what, why, fix);

    // SAFETY: loading a library runs its initialisers. The library is the one this process just
    // built from the user's own core (or one they named), and `inventory` registration, which is
    // all those initialisers do, is what reading the schema needs.
    let lib = unsafe { libloading::Library::new(library) }.map_err(|e| {
        fail(
            format!("cannot load {}: {e}", library.display()),
            "the schema is read from the built core library (docs/SPEC.md 13), which has to be loadable on this machine",
            "build the core for this machine with `keel build --platform host` and check that the file exists",
        )
    })?;

    let missing = |symbol: &str| {
        fail(
            format!("{} has no `{symbol}`", library.display()),
            "it is not a Keel core library: the C ABI of docs/SPEC.md 6 is what `keel-ffi` exports, and the library was not linked with it",
            "build it with `keel build` (which links `keel-ffi` into a library named keel_core), or pass a schema file with `keel bindgen --schema`",
        )
    };

    // SAFETY (the four `get` calls below): the symbol names and signatures are those of the C ABI
    // in docs/SPEC.md 6 (`keel_abi_version`, `keel_schema_hash`, `keel_schema_json`,
    // `keel_buf_free`), which `keel-ffi` implements and its tests pin; the function pointers do
    // not outlive `lib`, which is intentionally leaked below.
    // SAFETY: see above; `keel_abi_version` is `extern "C" fn() -> u32`.
    let abi_version: libloading::Symbol<'_, AbiVersionFn> =
        unsafe { lib.get(b"keel_abi_version\0") }.map_err(|_| missing("keel_abi_version"))?;
    // SAFETY: see above; `keel_schema_hash` is `extern "C" fn() -> u64`.
    let schema_hash: libloading::Symbol<'_, SchemaHashFn> =
        unsafe { lib.get(b"keel_schema_hash\0") }.map_err(|_| missing("keel_schema_hash"))?;
    // SAFETY: see above; `keel_schema_json` is `extern "C" fn() -> KeelBuf`.
    let schema_json: libloading::Symbol<'_, SchemaJsonFn> =
        unsafe { lib.get(b"keel_schema_json\0") }.map_err(|_| missing("keel_schema_json"))?;
    // SAFETY: see above; `keel_buf_free` is `extern "C" fn(KeelBuf)`.
    let buf_free: libloading::Symbol<'_, BufFreeFn> =
        unsafe { lib.get(b"keel_buf_free\0") }.map_err(|_| missing("keel_buf_free"))?;

    // SAFETY: `keel_abi_version` takes no arguments and returns an integer.
    let abi = unsafe { abi_version() };
    if abi != ABI_VERSION {
        return Err(fail(
            format!(
                "the core library speaks C ABI version {abi}; this keel-cli speaks {ABI_VERSION}"
            ),
            "the two were built from different Keel releases and cannot talk to each other",
            "use the keel-cli that matches the `keel` version of the core (`cargo install keel-cli --version <keel version>`)",
        ));
    }

    // SAFETY: `keel_schema_hash` takes no arguments and returns an integer.
    let hash = unsafe { schema_hash() };
    // SAFETY: `keel_schema_json` takes no arguments and returns an owned `KeelBuf`; its bytes are
    // valid for `len` bytes until `keel_buf_free`, which is called below on the same buffer.
    let buf = unsafe { schema_json() };
    let text = if buf.ptr.is_null() {
        String::new()
    } else {
        // SAFETY: `ptr` is non-null and the core promises `len` initialised bytes at it.
        let bytes = unsafe { std::slice::from_raw_parts(buf.ptr, buf.len as usize) };
        String::from_utf8_lossy(bytes).into_owned()
    };
    // SAFETY: `buf` came from `keel_schema_json` of this library and is freed exactly once.
    unsafe { buf_free(buf) };

    // A Rust cdylib that has started threads or thread-locals does not always survive `dlclose`;
    // the process is short-lived, so keep the library mapped instead.
    std::mem::forget(lib);

    let schema = parse_schema_json(&text, crate_name).map_err(|e| {
        fail(
            format!("the core library's schema cannot be read: {}", e.what),
            "`keel_schema_json` returned JSON this keel-cli does not understand (a newer `keel-meta`?)",
            "update keel-cli to the version of Keel the core uses",
        )
    })?;
    let computed = schema.hash();
    if computed != hash {
        return Err(fail(
            format!(
                "the schema hash does not match: the library says {hash:#018x}, its JSON hashes to {computed:#018x}"
            ),
            "keel-cli reads the schema with its own copy of `keel-meta`, and the core's differs in what it puts in the canonical form",
            "use the keel-cli that matches the `keel` version of the core",
        ));
    }
    Ok(schema)
}

/// Whether each of `symbols` is an exported, resolvable symbol of the core library at `library`.
///
/// Loads the library the way a platform runtime does (`dlopen`) and looks each name up
/// (`dlsym`), returning one bool per input name in order. Used to check that a built cdylib kept
/// keel-ffi's `#[no_mangle]` exports across the link — the JNI natives the Kotlin runtime binds
/// through `System.loadLibrary` (`JNI_OnLoad`, `Java_dev_keel_runtime_KeelNative_*`, SPEC 6.1),
/// which an incremental macOS build would otherwise dead-strip (ADR-029).
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
            "build it with `keel build --platform host`",
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

/// Parses schema JSON: canonical (from `keel_schema_json`) or full (`Schema::to_json_pretty`).
/// `crate_name` is used when the JSON has no `crate_name` label.
///
/// # Errors
///
/// `C0002` with the JSON error (line and column included) when it does not describe a schema.
pub fn parse_schema_json(text: &str, crate_name: &str) -> Result<Schema> {
    let bad = |what: String| {
        CliError::new(
            Code::BadConfig,
            what,
            "a schema file is what the bindings are generated from, so it has to be exactly the JSON Keel writes",
            "regenerate it: `keel bindgen` extracts it from the built core, and `Schema::to_json_pretty` writes one",
        )
    };
    let mut value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| bad(format!("the schema is not valid JSON: {e}")))?;
    let Some(object) = value.as_object_mut() else {
        return Err(bad("the schema must be a JSON object".to_owned()));
    };
    let defaults = Schema::new(crate_name);
    object
        .entry("keel_version")
        .or_insert_with(|| serde_json::Value::String(defaults.keel_version.clone()));
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
/// generates the same files whichever way its schema was obtained: the canonical JSON of the
/// library is sorted, the registrations collected by the dev runner are in declaration order.
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
    use keel_meta::{FieldDef, RecordDef, TypeRef, ids};

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
        use keel_meta::{MethodDef, ObjectDef};
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
            assert!(e.fix.contains("keel bindgen"), "{e}");
        }
    }

    #[test]
    fn a_library_that_is_not_there_is_explained() {
        let e = load_from_library(Path::new("/definitely/not/here.dylib"), "x").unwrap_err();
        assert_eq!(e.code, Code::Schema);
        assert!(e.what.contains("cannot load"), "{e}");
        assert!(e.fix.contains("keel build --platform host"), "{e}");
    }
}
