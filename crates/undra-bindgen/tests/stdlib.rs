//! The standard library (ADR-024): what the table in `undra_bindgen::stdlib` pins, what the
//! generators leave out, what they refer to instead, and what happens to a type that only shares a
//! name with a standard one.

mod common;

use common::{
    case, enum_def, error_def, field, named, record, struct_variant, tuple_variant, unit_variant,
    with_message,
};
use undra_bindgen::stdlib::{self, PORTS, StandardKind, TYPES};
use undra_bindgen::{BindgenError, GeneratedFile, Generator};
use undra_meta::{Schema, TypeRef, collect_schema, ids};

/// A port: name, id and its methods as `(name, id)`.
type PortRow<'a> = (&'a str, u32, Vec<(String, u32)>);

/// The standard schema as every app core has it: `undra-ports` linked in.
fn standard_schema(name: &str) -> Schema {
    // Naming a registered type links the registrations of `undra-ports` into this binary.
    let _ = undra_ports::HttpMethod::Get;
    collect_schema(name)
}

fn text(files: &[GeneratedFile]) -> String {
    files.iter().map(|f| f.contents.as_str()).collect()
}

fn file<'a>(files: &'a [GeneratedFile], suffix: &str) -> &'a str {
    &files
        .iter()
        .find(|f| f.path.ends_with(suffix))
        .unwrap_or_else(|| panic!("no generated file ends with {suffix}"))
        .contents
}

fn generate_all(generator: &Generator, schema: &Schema) -> [Vec<GeneratedFile>; 3] {
    [
        generator.swift(schema).expect("Swift generates"),
        generator.kotlin(schema).expect("Kotlin generates"),
        generator.typescript(schema).expect("TypeScript generates"),
    ]
}

fn codes(schema: &Schema) -> Vec<&'static str> {
    undra_bindgen::validate(schema)
        .err()
        .unwrap_or_default()
        .iter()
        .map(BindgenError::code)
        .collect()
}

// ----- the table ---------------------------------------------------------------------------

#[test]
fn the_table_is_what_undra_ports_registers() {
    let schema = standard_schema("undra-ports");

    // Every registered type and port is in the table, with the pinned id, and nothing else is.
    let mut registered_types: Vec<(&str, u32)> = schema
        .records
        .iter()
        .map(|r| (r.name.as_str(), r.type_id))
        .chain(schema.enums.iter().map(|e| (e.name.as_str(), e.type_id)))
        .collect();
    registered_types.sort_unstable();
    let mut table_types: Vec<(&str, u32)> = TYPES.iter().map(|t| (t.name, t.type_id)).collect();
    table_types.sort_unstable();
    assert_eq!(table_types, registered_types, "the standard types");

    let mut registered_ports: Vec<PortRow> = schema
        .ports
        .iter()
        .map(|p| {
            let mut methods: Vec<(String, u32)> = p
                .methods
                .iter()
                .map(|m| (m.name.clone(), m.method_id))
                .collect();
            methods.sort();
            (p.name.as_str(), p.port_id, methods)
        })
        .collect();
    registered_ports.sort();
    let mut table_ports: Vec<PortRow> = PORTS
        .iter()
        .map(|p| {
            let mut methods: Vec<(String, u32)> = p
                .methods
                .iter()
                .map(|m| (m.name().to_owned(), m.id))
                .collect();
            methods.sort();
            (p.name, p.port_id, methods)
        })
        .collect();
    table_ports.sort();
    assert_eq!(table_ports, registered_ports, "the standard ports");

    // The kinds agree too, and the shapes are exact: the registrations are recognised as the
    // standard library, all of them.
    for t in TYPES {
        let is_error = schema
            .enums
            .iter()
            .find(|e| e.name == t.name)
            .is_some_and(|e| e.is_error);
        assert_eq!(is_error, t.kind == StandardKind::Error, "{}", t.name);
    }
    for p in PORTS {
        let kind = schema.ports.iter().find(|d| d.name == p.name).unwrap().kind;
        assert_eq!(kind, p.kind, "{}", p.name);
    }
    let covered = stdlib::covered(&schema);
    assert_eq!(covered.types.len(), 9, "{:?}", covered.types);
    assert_eq!(covered.ports.len(), 10, "{:?}", covered.ports);
}

#[test]
fn the_ids_are_derived_from_the_names() {
    for t in TYPES {
        assert_eq!(t.type_id, ids::type_id(t.name), "{}", t.name);
    }
    for p in PORTS {
        assert_eq!(
            p.port_id,
            ids::fnv1a32(&format!("port.{}", p.name)),
            "{}",
            p.name
        );
        for m in p.methods {
            assert_eq!(
                m.id,
                ids::fnv1a32(&format!("{}.{}", p.name, m.name())),
                "{}.{}",
                p.name,
                m.name()
            );
        }
    }
}

// ----- generation ----------------------------------------------------------------------------

/// Names that only the standard library declares.
const STANDARD_DECLARATIONS: &[&str] = &[
    "HttpMethod",
    "Header",
    "HttpRequest",
    "HttpResponse",
    "HttpError",
    "FsError",
    "NetKind",
    "AppState",
];

#[test]
fn an_app_that_only_links_the_standard_library_generates_nothing_of_it() {
    let schema = standard_schema("app-core");
    let generator = Generator::for_crate("app-core");
    for files in generate_all(&generator, &schema) {
        let all = text(&files);
        for name in STANDARD_DECLARATIONS {
            for pattern in [
                format!("struct {name}"),
                format!("enum {name}"),
                format!("class {name}"),
                format!("interface {name}"),
                format!("type {name} ="),
                format!("const {name}Codec"),
            ] {
                assert!(!all.contains(&pattern), "{pattern} is generated");
            }
        }
        for port in [
            "Clock",
            "Http",
            "Kv",
            "SecureStore",
            "Fs",
            "Timer",
            "Connectivity",
        ] {
            assert!(
                !all.contains(&format!("{port}PortImpl")),
                "{port} is generated"
            );
            assert!(
                !all.contains(&format!("0x{:08x}", ids::port_id(port))),
                "{port} is in the ids"
            );
        }
    }
    // The schema hash still covers the standard library (R1, R7): it is a different core
    // than one without.
    let hash = format!("{:016x}", schema.hash());
    assert!(text(&generator.typescript(&schema).unwrap()).contains(&hash));
    assert_ne!(schema.hash(), Schema::new("app-core").hash());
}

#[test]
fn the_standard_library_can_still_be_generated_on_request() {
    let schema = standard_schema("undra-ports");
    let mut generator = Generator::for_crate("undra-ports");
    generator.emit_standard_library = true;
    let [swift, kotlin, ts] = generate_all(&generator, &schema);
    assert!(text(&swift).contains("public struct HttpRequest"));
    assert!(text(&swift).contains("public protocol SecureStore"));
    assert!(text(&kotlin).contains("data class HttpRequest"));
    assert!(text(&kotlin).contains("class ConnectivityEvents"));
    assert!(text(&kotlin).contains("interface SecureStore"));
    assert!(text(&ts).contains("export interface HttpRequest"));
    assert!(text(&ts).contains("export class ConnectivityEvents"));
}

#[test]
fn a_generated_standard_port_claims_its_name_again() {
    let mut schema = standard_schema("app-core");
    schema
        .records
        .push(record("Timer", "", vec![field("x", TypeRef::U32)]));
    let mut generator = Generator::for_crate("app-core");
    assert!(generator.validate(&schema).is_ok());
    generator.emit_standard_library = true;
    let errors = generator.validate(&schema).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].code(), "E0050");
    assert!(generator.kotlin(&schema).is_err());
}

#[test]
fn references_resolve_to_the_runtimes_types() {
    let schema = case("stdlib");
    let generator = common::generator_for("stdlib", &schema);
    let [swift, kotlin, ts] = generate_all(&generator, &schema);

    // TypeScript: one import from the runtime, no declaration.
    let types = file(&ts, "src/types.ts");
    assert!(types.contains("type HttpRequest,"), "{types}");
    assert!(types.contains("HttpRequestCodec,"), "{types}");
    assert!(types.contains("} from \"@undra/runtime\";"), "{types}");
    assert!(!types.contains("export interface HttpRequest"));
    assert!(!file(&ts, "src/errors.ts").contains("class HttpError"));
    assert!(!file(&ts, "src/errors.ts").contains("class FsError"));

    // Kotlin: imports from `dev.undra.runtime.adapters`, no declaration.
    let types = file(&kotlin, "Types.kt");
    assert!(types.contains("import dev.undra.runtime.adapters.HttpRequest\n"));
    assert!(types.contains("import dev.undra.runtime.adapters.NetKind\n"));
    assert!(!text(&kotlin).contains("class HttpRequest"));
    assert!(!text(&kotlin).contains("class HttpError"));

    // Swift: `UndraRuntime` exports all nine, so they are referenced and none is declared;
    // `AppState` is the runtime's `UndraAppState`.
    let types = file(&swift, "Types.swift");
    assert!(types.contains("public var request: HttpRequest"), "{types}");
    assert!(types.contains("public var kind: NetKind"), "{types}");
    assert!(types.contains("var app: UndraAppState?"), "{types}");
    for declared in [
        "struct HttpRequest",
        "struct HttpResponse",
        "struct Header",
        "enum HttpMethod",
        "enum NetKind",
        "enum HttpError",
        "enum FsError",
        "enum AppState",
    ] {
        assert!(!text(&swift).contains(declared), "{declared} is declared");
    }
    assert!(!text(&swift).contains("AppState?") || text(&swift).contains("UndraAppState?"));
    // What refers to a standard error decodes it with the runtime's own type, which is an
    // `UndraError` (`UndraCallError.mapped(_:domain:)`).
    assert!(
        file(&swift, "Objects.swift").contains("domain: HttpError.self"),
        "{}",
        file(&swift, "Objects.swift")
    );
    // The `Codable` a record derives survives holding a standard type: the runtime's records
    // are `Codable`, so `Endpoint` (a request, a response and a list of methods and headers) is.
    assert!(
        types.contains("public struct Endpoint: UndraRecord, Sendable, Hashable, Codable"),
        "{types}"
    );
}

#[test]
fn no_language_declares_a_standard_type_something_refers_to() {
    // Swift used to declare each standard type that something referred to, because its runtime
    // kept them internal (ADR-024); the runtime exports all nine now, so the three languages
    // agree: reference, never declare.
    let mut schema = standard_schema("app-core");
    schema.records.push(record(
        "Banner",
        "",
        vec![field("lines", TypeRef::vec(named("Header")))],
    ));
    schema.records.push(record(
        "Call",
        "",
        vec![field("request", named("HttpRequest"))],
    ));
    let generator = Generator::for_crate("app-core");
    let swift = text(&generator.swift(&schema).unwrap());
    assert!(swift.contains("public struct Banner:"));
    assert!(swift.contains("public var lines: [Header]"), "{swift}");
    assert!(swift.contains("public var request: HttpRequest"), "{swift}");
    for absent in [
        "struct Header",
        "struct HttpRequest",
        "struct HttpResponse",
        "enum HttpMethod",
        "enum HttpError",
        "enum FsError",
        "enum NetKind",
        "enum AppState",
    ] {
        assert!(!swift.contains(absent), "{absent} is declared");
    }
    // TypeScript and Kotlin import the ones that are used and declare none of them.
    let ts = text(&generator.typescript(&schema).unwrap());
    assert!(ts.contains("type HttpRequest,") && !ts.contains("interface HttpRequest"));
    let kotlin = text(&generator.kotlin(&schema).unwrap());
    assert!(kotlin.contains("import dev.undra.runtime.adapters.HttpRequest"));
    assert!(!kotlin.contains("class HttpRequest"));
}

#[test]
fn a_swift_app_type_that_only_shares_a_standard_name_is_still_the_apps_own() {
    // `Header { text }` is not the standard `Header` (another shape), so it is generated, in the
    // app's module, where it shadows the runtime's public `Header`.
    let mut schema = standard_schema("app-core");
    schema.records.retain(|r| r.name != "Header");
    schema
        .records
        .push(record("Header", "", vec![field("text", TypeRef::String)]));
    let generator = Generator::for_crate("app-core");
    let swift = text(&generator.swift(&schema).unwrap());
    assert!(swift.contains("public struct Header:"), "{swift}");
    assert!(swift.contains("public var text: String"), "{swift}");
}

#[test]
fn a_user_type_named_like_a_standard_port_is_fine() {
    // The standard ports are not generated, so they do not claim their names.
    for name in ["Timer", "Log", "Clock", "Connectivity", "Kv"] {
        let mut schema = standard_schema("app-core");
        schema
            .records
            .push(record(name, "", vec![field("x", TypeRef::U32)]));
        let generator = Generator::for_crate("app-core");
        let [swift, kotlin, ts] = generate_all(&generator, &schema);
        assert!(
            text(&swift).contains(&format!("public struct {name}:")),
            "{name}"
        );
        assert!(
            text(&kotlin).contains(&format!("data class {name}(")),
            "{name}"
        );
        assert!(
            text(&ts).contains(&format!("export interface {name} ")),
            "{name}"
        );
    }
}

// ----- names that only look standard ------------------------------------------------------------

/// A record called `HttpRequest` that is not the standard one.
fn own_request() -> Schema {
    let mut schema = Schema::new("app-core");
    schema.records.push(record(
        "HttpRequest",
        "The app's own.",
        vec![field("url", TypeRef::String)],
    ));
    schema.records.push(record(
        "Job",
        "",
        vec![field("request", named("HttpRequest"))],
    ));
    schema
}

#[test]
fn a_type_that_only_shares_a_name_keeps_being_generated() {
    // Same name, same id (ids come from names), other shape: the app's own type.
    let schema = own_request();
    assert!(stdlib::covered(&schema).types.is_empty());
    let generator = Generator::for_crate("app-core");
    let [swift, kotlin, ts] = generate_all(&generator, &schema);
    assert!(text(&swift).contains("public struct HttpRequest: UndraRecord"));
    assert!(text(&kotlin).contains("data class HttpRequest("));
    let ts = text(&ts);
    assert!(ts.contains("export interface HttpRequest {"));
    assert!(
        !ts.contains("type HttpRequest,"),
        "no import of the runtime's"
    );
}

#[test]
fn a_standard_type_is_covered_only_with_everything_it_refers_to() {
    // The standard `HttpRequest`, but an app `Header` of another shape: the runtime's
    // `HttpRequest` holds the runtime's `Header`, so neither is left out.
    let mut schema = standard_schema("app-core");
    let header = schema
        .records
        .iter_mut()
        .find(|r| r.name == "Header")
        .unwrap();
    header.fields.push(field("extra", TypeRef::Bool));
    let covered = stdlib::covered(&schema);
    assert!(!covered.types.contains("Header"));
    assert!(!covered.types.contains("HttpRequest"));
    assert!(!covered.types.contains("HttpResponse"));
    assert!(covered.types.contains("FsError"));
    assert!(
        !covered.ports.contains("Http"),
        "the Http port needs HttpRequest"
    );
    assert!(covered.ports.contains("Kv"));
    let ts = text(
        &Generator::for_crate("app-core")
            .typescript(&schema)
            .unwrap(),
    );
    assert!(ts.contains("export interface HttpRequest"));
    assert!(ts.contains("export interface Header"));
    assert!(!ts.contains("export interface FsError"));
}

#[test]
fn an_item_with_a_standard_name_and_another_id_is_e0052() {
    // Ids come from names, so the macros never do this; a hand-written schema can.
    let mut schema = Schema::new("app-core");
    let mut request = record("HttpRequest", "", vec![field("url", TypeRef::String)]);
    request.type_id = 7;
    schema.records.push(request);
    assert_eq!(codes(&schema), ["E0052"]);

    let mut schema = Schema::new("app-core");
    let mut error = error_def(
        "FsError",
        "",
        vec![with_message(unit_variant("Gone", 0), "gone")],
    );
    error.type_id = 9;
    schema.enums.push(error);
    assert_eq!(codes(&schema), ["E0052"]);

    let mut schema = standard_schema("app-core");
    schema
        .ports
        .iter_mut()
        .find(|p| p.name == "Clock")
        .unwrap()
        .port_id = 3;
    assert_eq!(codes(&schema), ["E0052"]);
    let message = undra_bindgen::validate(&schema).unwrap_err()[0].to_string();
    assert!(message.starts_with("error[undra::E0052]: "), "{message}");
    assert!(message.contains("port `Clock`"), "{message}");
    assert!(message.contains("rename it"), "{message}");
    assert!(message.contains("0x00000003"), "{message}");

    // Generating fails the same way, so `undra bindgen` reports it.
    let generator = Generator::for_crate("app-core");
    let errors = generator.kotlin(&schema).unwrap_err();
    assert_eq!(errors[0].code(), "E0052");
}

#[test]
fn the_standard_entries_validate() {
    let schema = standard_schema("app-core");
    undra_bindgen::validate(&schema).expect("the standard library is a valid schema");
    assert!(codes(&schema).is_empty());
}

#[test]
fn a_second_declaration_of_a_standard_type_is_still_a_duplicate() {
    let mut schema = standard_schema("app-core");
    schema
        .records
        .push(record("Header", "", vec![field("k", TypeRef::String)]));
    assert_eq!(codes(&schema), ["E0050"]);
}

// ----- shadowing inside a sealed hierarchy -----------------------------------------------------------

#[test]
fn kotlin_qualifies_a_standard_type_that_a_variant_shadows() {
    let mut schema = standard_schema("app-core");
    schema.enums.push(error_def(
        "SyncError",
        "",
        vec![
            tuple_variant("Http", 0, vec![named("HttpError")]),
            with_message(
                struct_variant("HttpError", 1, vec![field("status", TypeRef::U16)]),
                "status {status}",
            ),
        ],
    ));
    let kotlin = text(
        &Generator::for_crate("app-core")
            .kotlin(&schema)
            .expect("Kotlin generates"),
    );
    // Inside the hierarchy `HttpError` is the variant; the runtime's type is spelled in full.
    assert!(
        kotlin
            .contains("data class Http(override val cause: dev.undra.runtime.adapters.HttpError)"),
        "{kotlin}"
    );
    assert!(kotlin.contains("dev.undra.runtime.adapters.HttpError.encode(w, v.cause)"));
    assert!(kotlin.contains("data class HttpError(val status: UShort)"));
}

#[test]
fn a_schema_without_standard_items_is_untouched_by_the_filter() {
    // The golden cases have none that is exactly standard: the filter changes nothing.
    let schema = case("records");
    let generator = common::generator_for("records", &schema);
    let with = generator.typescript(&schema).unwrap();
    let mut keep = generator.clone();
    keep.emit_standard_library = true;
    assert_eq!(with, keep.typescript(&schema).unwrap());
    // And an enum named like a standard one, with its own variants, is generated as usual.
    let mut schema = Schema::new("app-core");
    schema.enums.push(enum_def(
        "NetKind",
        "",
        vec![unit_variant("Fast", 0), unit_variant("Slow", 1)],
    ));
    let ts = text(
        &Generator::for_crate("app-core")
            .typescript(&schema)
            .unwrap(),
    );
    assert!(ts.contains("export type NetKind = \"fast\" | \"slow\";"));
}
