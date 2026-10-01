//! The schema of the standard ports: what `undra-meta` can describe (constitution R1), locked so a
//! change to the wire surface is a visible, deliberate diff (R7).
//!
//! `tests/golden/schema.json` is the canonical schema JSON of this crate. If a change to the
//! ports or records is intended (it needs an ADR: the three platform runtimes hard-code this
//! surface), regenerate it with `UNDRA_BLESS=1 cargo test -p undra-ports --test schema` and update
//! `SCHEMA_HASH` below.

use undra_bindgen::Generator;
use undra_meta::{PortKind, Schema, TypeRef, collect_schema, ids};
use undra_ports::HttpMethod;

/// `Schema::hash()` of the standard ports.
/// The standard surface's hash since ADR-049 (`StorageError`, the storage ports' signatures, two
/// `FsError` variants); it was `0x35fa_e635_f800_25f2`.
const SCHEMA_HASH: u64 = 0xbbf6_f70d_0c56_7f47;

const GOLDEN: &str = "tests/golden/schema.json";

fn schema() -> Schema {
    // Referencing a registered type links the crate's registrations into this test binary.
    let _ = HttpMethod::Get;
    collect_schema("undra-ports")
}

fn t(name: &str) -> TypeRef {
    TypeRef::named(name)
}

#[test]
fn the_schema_has_exactly_the_standard_surface() {
    let schema = schema();
    let names = |mut names: Vec<&str>| {
        names.sort_unstable();
        names.join(",")
    };
    assert_eq!(
        names(schema.records.iter().map(|r| r.name.as_str()).collect()),
        "Header,HttpRequest,HttpResponse"
    );
    assert_eq!(
        names(schema.enums.iter().map(|e| e.name.as_str()).collect()),
        "AppState,FsError,HttpError,HttpMethod,NetKind,StorageError"
    );
    assert_eq!(
        names(schema.ports.iter().map(|p| p.name.as_str()).collect()),
        "Clock,Connectivity,Fs,Http,Kv,Lifecycle,Log,Rng,SecureStore,Timer"
    );
    assert!(schema.objects.is_empty(), "a ports crate has no objects");
    assert!(schema.functions.is_empty());
    assert!(schema.queries.is_empty());
}

#[test]
fn type_ids_are_hard_coded() {
    let schema = schema();
    let expected = [
        ("Header", 0x114f_9980),
        ("HttpRequest", 0xbbbc_6e52),
        ("HttpResponse", 0xcc45_59fe),
        ("HttpMethod", 0x77bf_0650),
        ("HttpError", 0xee63_c1f1),
        ("FsError", 0xd15e_c208),
        ("StorageError", 0x3d40_b010),
        ("NetKind", 0x0371_71aa),
        ("AppState", 0xcfb6_6091),
    ];
    for (name, id) in expected {
        assert_eq!(ids::type_id(name), id, "{name}");
        let registered = schema
            .records
            .iter()
            .map(|r| (r.name.as_str(), r.type_id))
            .chain(schema.enums.iter().map(|e| (e.name.as_str(), e.type_id)))
            .find(|(n, _)| *n == name)
            .map(|(_, id)| id);
        assert_eq!(registered, Some(id), "{name} registered id");
    }
}

#[test]
fn record_fields_are_in_wire_order() {
    let schema = schema();
    let fields = |record: &str| -> Vec<(String, TypeRef)> {
        schema
            .records
            .iter()
            .find(|r| r.name == record)
            .unwrap_or_else(|| panic!("{record} is not registered"))
            .fields
            .iter()
            .map(|f| (f.name.clone(), f.ty.clone()))
            .collect()
    };
    let field = |name: &str, ty: TypeRef| (name.to_owned(), ty);
    assert_eq!(
        fields("Header"),
        [
            field("name", TypeRef::String),
            field("value", TypeRef::String)
        ]
    );
    assert_eq!(
        fields("HttpRequest"),
        [
            field("method", t("HttpMethod")),
            field("url", TypeRef::String),
            field("headers", TypeRef::vec(t("Header"))),
            field("body", TypeRef::option(TypeRef::Bytes)),
            field("timeout_ms", TypeRef::option(TypeRef::U32)),
        ]
    );
    assert_eq!(
        fields("HttpResponse"),
        [
            field("status", TypeRef::U16),
            field("headers", TypeRef::vec(t("Header"))),
            field("body", TypeRef::Bytes),
        ]
    );
}

#[test]
fn enum_and_error_variants_are_in_wire_order() {
    let schema = schema();
    let variants = |name: &str| -> Vec<(u16, String, Vec<TypeRef>)> {
        let def = schema
            .enums
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("{name} is not registered"));
        def.variants
            .iter()
            .map(|v| {
                (
                    v.index,
                    v.name.clone(),
                    v.fields.iter().map(|f| f.ty.clone()).collect(),
                )
            })
            .collect()
    };
    let unit = |names: &[&str]| -> Vec<(u16, String, Vec<TypeRef>)> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| (u16::try_from(i).unwrap(), (*n).to_owned(), vec![]))
            .collect()
    };
    assert_eq!(
        variants("HttpMethod"),
        unit(&["Get", "Post", "Put", "Delete", "Patch", "Head", "Options"])
    );
    assert_eq!(
        variants("NetKind"),
        unit(&["Wifi", "Cellular", "Wired", "Unknown", "None"])
    );
    assert_eq!(
        variants("AppState"),
        unit(&["Active", "Inactive", "Background"])
    );
    assert_eq!(
        variants("HttpError"),
        [
            (0, "Network".to_owned(), vec![TypeRef::String]),
            (1, "Timeout".to_owned(), vec![]),
            (2, "Cancelled".to_owned(), vec![]),
            (3, "InvalidUrl".to_owned(), vec![TypeRef::String]),
        ]
    );
    assert_eq!(
        variants("FsError"),
        [
            (0, "NotFound".to_owned(), vec![]),
            (1, "Denied".to_owned(), vec![]),
            (2, "Io".to_owned(), vec![TypeRef::String]),
            (3, "Full".to_owned(), vec![]),
            (4, "Unavailable".to_owned(), vec![TypeRef::String]),
        ]
    );
    assert_eq!(
        variants("StorageError"),
        [
            (0, "Unavailable".to_owned(), vec![TypeRef::String]),
            (1, "Full".to_owned(), vec![]),
            (2, "Locked".to_owned(), vec![]),
            (3, "Corrupt".to_owned(), vec![TypeRef::String]),
            (4, "Io".to_owned(), vec![TypeRef::String]),
        ]
    );
    for name in ["HttpError", "FsError", "StorageError"] {
        assert!(
            schema
                .enums
                .iter()
                .find(|e| e.name == name)
                .unwrap()
                .is_error
        );
    }
    for name in ["HttpMethod", "NetKind", "AppState"] {
        assert!(
            !schema
                .enums
                .iter()
                .find(|e| e.name == name)
                .unwrap()
                .is_error
        );
    }
}

#[test]
fn method_signatures_are_the_ones_of_spec_8() {
    let schema = schema();
    let bytes_result = |err: &str| TypeRef::result(TypeRef::Bytes, t(err));
    let unit_result = |err: &str| TypeRef::result(TypeRef::Unit, t(err));
    let strings = TypeRef::vec(TypeRef::String);
    // ADR-049: every storage method has the `StorageError` channel.
    let storage = |ok: TypeRef| TypeRef::result(ok, t("StorageError"));
    let kv_methods = || {
        vec![
            (
                "get",
                vec![("key", TypeRef::String)],
                storage(TypeRef::option(TypeRef::Bytes)),
            ),
            (
                "set",
                vec![("key", TypeRef::String), ("value", TypeRef::Bytes)],
                storage(TypeRef::Unit),
            ),
            (
                "delete",
                vec![("key", TypeRef::String)],
                storage(TypeRef::Unit),
            ),
            (
                "list",
                vec![("prefix", TypeRef::String)],
                storage(TypeRef::vec(TypeRef::String)),
            ),
        ]
    };
    #[allow(clippy::type_complexity)]
    let expected: Vec<(&str, PortKind, Vec<(&str, Vec<(&str, TypeRef)>, TypeRef)>)> = vec![
        (
            "Clock",
            PortKind::Sync,
            vec![
                ("now_ms", vec![], TypeRef::I64),
                ("monotonic_ns", vec![], TypeRef::U64),
            ],
        ),
        (
            "Rng",
            PortKind::Sync,
            vec![("fill", vec![("len", TypeRef::U32)], TypeRef::Bytes)],
        ),
        (
            "Log",
            PortKind::Sync,
            vec![(
                "log",
                vec![
                    ("level", TypeRef::U8),
                    ("target", TypeRef::String),
                    ("message", TypeRef::String),
                ],
                TypeRef::Unit,
            )],
        ),
        (
            "Http",
            PortKind::Async,
            vec![(
                "request",
                vec![("req", t("HttpRequest"))],
                TypeRef::result(t("HttpResponse"), t("HttpError")),
            )],
        ),
        ("Kv", PortKind::Async, kv_methods()),
        ("SecureStore", PortKind::Async, kv_methods()),
        (
            "Fs",
            PortKind::Async,
            vec![
                (
                    "read",
                    vec![("path", TypeRef::String)],
                    bytes_result("FsError"),
                ),
                (
                    "write",
                    vec![("path", TypeRef::String), ("data", TypeRef::Bytes)],
                    unit_result("FsError"),
                ),
                (
                    "delete",
                    vec![("path", TypeRef::String)],
                    unit_result("FsError"),
                ),
                (
                    "list",
                    vec![("dir", TypeRef::String)],
                    TypeRef::result(strings, t("FsError")),
                ),
            ],
        ),
        (
            "Timer",
            PortKind::Sync,
            vec![(
                "set",
                vec![("timer_id", TypeRef::U32), ("delay_ms", TypeRef::U64)],
                TypeRef::Unit,
            )],
        ),
        (
            "Connectivity",
            PortKind::Event,
            vec![(
                "changed",
                vec![("online", TypeRef::Bool), ("kind", t("NetKind"))],
                TypeRef::Unit,
            )],
        ),
        (
            "Lifecycle",
            PortKind::Event,
            vec![("changed", vec![("state", t("AppState"))], TypeRef::Unit)],
        ),
    ];
    for (port, kind, methods) in expected {
        let def = schema
            .ports
            .iter()
            .find(|p| p.name == port)
            .unwrap_or_else(|| panic!("{port} is not registered"));
        assert_eq!(def.kind, kind, "{port} kind");
        assert_eq!(def.methods.len(), methods.len(), "{port} method count");
        for (name, params, returns) in methods {
            let method = def
                .methods
                .iter()
                .find(|m| m.name == name)
                .unwrap_or_else(|| panic!("{port}.{name} is not registered"));
            let got: Vec<(String, TypeRef)> = method
                .params
                .iter()
                .map(|p| (p.name.clone(), p.ty.clone()))
                .collect();
            let want: Vec<(String, TypeRef)> = params
                .into_iter()
                .map(|(n, ty)| (n.to_owned(), ty))
                .collect();
            assert_eq!(got, want, "{port}.{name} params");
            assert_eq!(method.returns, returns, "{port}.{name} returns");
        }
    }
}

#[test]
fn the_schema_validates() {
    schema()
        .validate()
        .unwrap_or_else(|errors| panic!("the standard schema is invalid: {errors:#?}"));
}

#[test]
fn the_schema_hash_and_canonical_json_are_locked() {
    let schema = schema();
    let json = schema.canonical_json();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN);
    if std::env::var_os("UNDRA_BLESS").is_some() {
        std::fs::write(&path, format!("{json}\n")).expect("write the golden schema");
    }
    let golden = std::fs::read_to_string(&path).expect("tests/golden/schema.json is missing");
    assert_eq!(
        json,
        golden.trim_end(),
        "the standard ports' schema changed: this breaks the platform runtimes and needs an ADR \
         (regenerate with UNDRA_BLESS=1 if it is intended)"
    );
    assert_eq!(
        schema.hash(),
        SCHEMA_HASH,
        "schema hash changed (now {:#x}); update SCHEMA_HASH together with the golden file",
        schema.hash()
    );
    assert_eq!(schema.hash(), ids::fnv1a64(json.as_bytes()));
}

#[test]
fn bindgen_generates_all_three_languages_from_the_standard_schema() {
    let schema = schema();
    // Apps leave the standard library out of their bindings (ADR-024); this test is about the
    // library itself, so it asks for it.
    let mut generator = Generator::for_crate("undra-ports");
    generator.emit_standard_library = true;
    let swift = generator.swift(&schema).expect("Swift generation");
    let kotlin = generator.kotlin(&schema).expect("Kotlin generation");
    let ts = generator
        .typescript(&schema)
        .expect("TypeScript generation");
    for (language, files) in [("Swift", &swift), ("Kotlin", &kotlin), ("TypeScript", &ts)] {
        assert!(!files.is_empty(), "{language}: no files");
        let all: String = files.iter().map(|f| f.contents.as_str()).collect();
        for needle in ["HttpRequest", "HttpResponse", "SecureStore", "Connectivity"] {
            assert!(
                all.contains(needle),
                "{language}: generated code lacks {needle}"
            );
        }
    }
}
