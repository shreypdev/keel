//! Behaviour of the generators that the golden files do not pin: configuration
//! options, determinism, file layout and structural sanity of every output.

mod common;

use undra_bindgen::{GeneratedFile, Generator};
use undra_meta::{Schema, TypeRef};

fn all_files(case: &str, configure: impl Fn(&mut Generator)) -> Vec<(String, Vec<GeneratedFile>)> {
    let schema = common::case(case);
    let mut generator = common::generator_for(case, &schema);
    configure(&mut generator);
    vec![
        ("swift".to_owned(), generator.swift(&schema).unwrap()),
        ("kotlin".to_owned(), generator.kotlin(&schema).unwrap()),
        ("ts".to_owned(), generator.typescript(&schema).unwrap()),
    ]
}

fn file<'a>(files: &'a [GeneratedFile], suffix: &str) -> &'a str {
    &files
        .iter()
        .find(|f| f.path.ends_with(suffix))
        .unwrap_or_else(|| panic!("no file ends with {suffix}"))
        .contents
}

// ----- layout ---------------------------------------------------------------------------

#[test]
fn every_language_writes_the_documented_files() {
    let schema = common::case("full");
    let generator = Generator::for_crate(&schema.crate_name);
    let paths =
        |files: Vec<GeneratedFile>| -> Vec<String> { files.into_iter().map(|f| f.path).collect() };

    let mut swift: Vec<String> = [
        "Types", "Errors", "Objects", "Stores", "Ports", "Queries", "Ids", "Core",
    ]
    .map(|n| format!("Sources/PlaygroundCore/Generated/{n}.swift"))
    .to_vec();
    // ADR-044: the C module that declares the core's entry, `playground_core_undra_api`.
    swift.extend(
        [
            "include/playground_core_undra.h",
            "include/module.modulemap",
            "playground_core_undra.c",
        ]
        .map(|f| format!("Sources/PlaygroundCoreFFI/{f}")),
    );
    assert_eq!(paths(generator.swift(&schema).unwrap()), swift);
    let mut kotlin: Vec<String> = [
        "Types", "Errors", "Objects", "Stores", "Ports", "Queries", "Ids", "Core",
    ]
    .map(|n| format!("src/main/kotlin/dev/undra/generated/playground_core/{n}.kt"))
    .to_vec();
    // ADR-044: the R8 rules that keep the natives `JNI_OnLoad` registers by name.
    kotlin.push("src/main/resources/META-INF/proguard/undra-playground_core.pro".to_owned());
    assert_eq!(paths(generator.kotlin(&schema).unwrap()), kotlin);
    let mut expected: Vec<String> = [
        "types", "errors", "objects", "stores", "ports", "queries", "ids", "core", "index",
    ]
    .iter()
    .map(|n| format!("src/{n}.ts"))
    .collect();
    expected.extend(["package.json".to_owned(), "tsconfig.json".to_owned()]);
    assert_eq!(paths(generator.typescript(&schema).unwrap()), expected);
}

#[test]
fn an_empty_schema_still_writes_valid_files() {
    let schema = Schema::new("empty-core");
    let generator = Generator::for_crate("empty-core");
    let ts = generator.typescript(&schema).unwrap();
    // A TypeScript file must be a module for `export *` to accept it.
    for f in ts.iter().filter(|f| f.path.ends_with(".ts")) {
        assert!(
            f.contents.contains("export "),
            "{} has no export:\n{}",
            f.path,
            f.contents
        );
    }
    assert!(file(&ts, "src/types.ts").contains("export {};"));
    let swift = generator.swift(&schema).unwrap();
    assert!(file(&swift, "Ids.swift").contains("public enum UndraIds"));
    let kotlin = generator.kotlin(&schema).unwrap();
    assert!(file(&kotlin, "Ids.kt").contains("object UndraIds"));
}

#[test]
fn the_configuration_names_the_output() {
    let schema = common::case("full");
    let mut generator = Generator::for_crate(&schema.crate_name);
    generator.swift_module = "Acme".into();
    generator.kotlin_package = "com.acme.core".into();
    generator.ts_scope = "acme".into();
    generator.ts_package = "core".into();
    generator.package_version = "2.3.4".into();

    let swift = generator.swift(&schema).unwrap();
    assert!(swift[0].path.starts_with("Sources/Acme/Generated/"));
    let kotlin = generator.kotlin(&schema).unwrap();
    assert!(kotlin[0].path.starts_with("src/main/kotlin/com/acme/core/"));
    assert!(kotlin[0].contents.contains("\npackage com.acme.core\n"));
    let ts = generator.typescript(&schema).unwrap();
    let package: serde_json::Value = serde_json::from_str(file(&ts, "package.json")).unwrap();
    assert_eq!(package["name"], "@acme/core");
    assert_eq!(package["version"], "2.3.4");
    assert_eq!(package["peerDependencies"]["@undra/runtime"], "^0.1.0");
    assert_eq!(package["type"], "module");

    generator.ts_scope = String::new();
    let ts = generator.typescript(&schema).unwrap();
    let package: serde_json::Value = serde_json::from_str(file(&ts, "package.json")).unwrap();
    assert_eq!(package["name"], "core");
}

// ----- options -------------------------------------------------------------------------

#[test]
fn swift_typed_throws_only_changes_port_requirements() {
    // ADR-032: calls never use typed throws; only the requirements of a port, which the host
    // implements, do.
    let typed = all_files("ports", |_| {});
    let untyped = all_files("ports", |g| g.swift_typed_throws = false);
    let typed_ports = file(&typed[0].1, "Ports.swift");
    let untyped_ports = file(&untyped[0].1, "Ports.swift");
    assert!(typed_ports.contains("throws(HttpError)"));
    assert!(!untyped_ports.contains("throws("));
    assert!(untyped_ports.contains("async throws -> HttpResponse"));

    let typed = all_files("objects", |_| {});
    let untyped = all_files("objects", |g| g.swift_typed_throws = false);
    for name in ["Objects.swift", "Stores.swift", "Queries.swift"] {
        let (a, b) = (file(&typed[0].1, name), file(&untyped[0].1, name));
        assert_eq!(a, b, "{name} must not depend on swift_typed_throws");
        assert!(
            !a.contains("throws("),
            "{name}: a call must not use typed throws"
        );
    }
}

#[test]
fn swift_records_derive_codable_only_from_codable_fields() {
    let files = all_files("records", |_| {});
    let types = file(&files[0].1, "Types.swift");
    // `Swift.Duration` is `Codable` only from the Swift 6.0 standard library, above the
    // runtime's deployment floor, so a record holding one must not derive it.
    assert!(types.contains("public struct Numbers: UndraRecord, Sendable, Hashable {"));
    assert!(types.contains("public struct Containers: UndraRecord, Sendable, Hashable, Codable {"));
}

#[test]
fn swift_streams_are_decoded_by_the_runtime_so_credit_follows_the_consumer() {
    // A stream method must hand the core's pull-based stream to `UndraCore.stream(..decode:)`
    // (SPEC 3.7). Copying it into an `AsyncThrowingStream` with `yield` buffers without bound and
    // the core runs ahead of the consumer (playground finding 3).
    for case in ["objects", "stores", "full", "queries", "stdlib"] {
        let out = all_files(case, |_| {});
        for f in &out[0].1 {
            assert!(
                !f.contents.contains("continuation.yield")
                    && !f.contents.contains("undraDecodeStream"),
                "{case}/{}: a generated stream must not copy items into its own buffer",
                f.path
            );
        }
    }
    let out = all_files("objects", |_| {});
    let swift = file(&out[0].1, "Objects.swift");
    assert!(swift.contains("return self.core.stream("));
    assert!(swift.contains("decode: { try UInt32.undraDecoded(from: $0) }"));
    // Every stream maps its failure in the runtime (ADR-032); a typed error is the domain.
    assert!(swift.contains(
        "mapError: { UndraCallError.mapped(streamFailure: $0, domain: CalcError.self) }"
    ));
    assert!(swift.contains("mapError: { UndraCallError.mapped(streamFailure: $0) }"));
    assert!(!swift.contains("mapError: { $0 }"));
}

#[test]
fn swift_stores_and_objects_restate_unchecked_sendable() {
    // `UndraObject` and `UndraStore` are `@unchecked Sendable`; Swift 6 warns when a subclass does
    // not say so again (playground finding 6).
    for case in ["objects", "stores", "full", "queries", "stdlib"] {
        let out = all_files(case, |_| {});
        for f in &out[0].1 {
            for line in f
                .contents
                .lines()
                .filter(|l| l.starts_with("public final class "))
            {
                assert!(
                    line.contains("@unchecked Sendable"),
                    "{case}/{}: {line}",
                    f.path
                );
            }
        }
    }
}

#[test]
fn asynchronous_methods_without_a_result_can_throw_in_both_modes() {
    for typed in [true, false] {
        let out = all_files("objects", |g| g.swift_typed_throws = typed);
        let swift = file(&out[0].1, "Objects.swift");
        assert!(swift.contains("public func compute(input: Double?) async throws -> Double"));
        assert!(swift.contains("public func warmUp() async throws {"));
        // A synchronous method that returns a value throws too (ADR-032); only a command, which
        // returns nothing, stays non-throwing.
        assert!(swift.contains("public func add(a: Int32, b: Int32) throws -> Int32 {"));
    }
}

#[test]
fn swift_generated_code_never_stops_the_process() {
    // ADR-032, decision 1: no outcome of a call reaches a trap, in any case or configuration.
    const TRAPS: [&str; 8] = [
        "fatalError",
        "undraUnexpected",
        "preconditionFailure",
        "precondition(",
        "assertionFailure",
        "assert(",
        "try!",
        "as!",
    ];
    for case in common::CASES {
        for typed in [true, false] {
            let out = all_files(case, |g| g.swift_typed_throws = typed);
            for f in &out[0].1 {
                for trap in TRAPS {
                    assert!(
                        !f.contents.contains(trap),
                        "{case}/{} (typed throws {typed}) contains `{trap}`",
                        f.path
                    );
                }
            }
        }
    }
}

/// The text of the method that starts at `head` in `swift`, up to the next method.
fn swift_method<'a>(swift: &'a str, head: &str) -> &'a str {
    let start = swift
        .find(head)
        .unwrap_or_else(|| panic!("no method starts with `{head}`"));
    let rest = &swift[start + head.len()..];
    let end = rest.find("\n    public ").unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn swift_call_shapes_follow_adr_032() {
    let out = all_files("objects", |_| {});
    let swift = file(&out[0].1, "Objects.swift");
    // A call throws plain `throws`, whatever its Rust signature.
    for shape in [
        "public func add(a: Int32, b: Int32) throws -> Int32",
        "public func divide(a: Int64, b: Int64) throws -> Int64",
        "public func check() throws {",
        "public func lookup(id: UUID) async throws -> Todo",
        "public func compute(input: Double?) async throws -> Double",
        ") throws -> Calculator {",
        ") async throws -> Calculator {",
    ] {
        assert!(swift.contains(shape), "missing `{shape}`");
    }
    assert!(
        !swift.contains("throws("),
        "calls must not use typed throws"
    );
    // The mapping is the runtime's, once: the typed error is its `domain`.
    assert!(swift.contains("throw UndraCallError.mapped(error, domain: CalcError.self)"));
    assert!(swift.contains("throw UndraCallError.mapped(error)\n"));
    // A command (a synchronous method that returns nothing and has no error type) does not throw;
    // it reports and returns.
    let reset = swift_method(swift, "public func reset() {");
    assert!(reset.contains("self.core.report(error, operation: \"Calculator.reset\")"));
    assert!(!reset.contains("throw "));
    // The error the call can throw is documented on the method.
    let divide = swift_method(
        swift,
        "public func divide(a: Int64, b: Int64) throws -> Int64",
    );
    assert!(divide.contains("UndraCallError.mapped(error, domain: CalcError.self)"));
    assert!(swift.contains(
        "/// - Throws: ``CalcError``, or ``UndraCallError`` if the call fails in the core or cannot reach it."
    ));
    assert!(swift.contains(
        "/// - Throws: ``CalcError``, `CancellationError` if the task is cancelled, or ``UndraCallError``."
    ));
    assert!(swift.contains(
        "/// - Throws: `CancellationError` if the task is cancelled, or ``UndraCallError``."
    ));
    assert!(swift.contains(
        "/// - Throws: ``UndraCallError`` if the call fails in the core or cannot reach it."
    ));
    // Streams map their failures; they are not `throws`.
    assert!(swift.contains(
        "mapError: { UndraCallError.mapped(streamFailure: $0, domain: CalcError.self) }"
    ));
    assert!(swift.contains("mapError: { UndraCallError.mapped(streamFailure: $0) }"));
    // A stream and a command say in their docs where a failure goes.
    assert!(swift.contains(
        "/// - Note: Iterating throws ``CalcError``, or ``UndraCallError`` if the call fails in the core or cannot reach it; cancelling the iterating task ends the loop quietly."
    ));
    assert!(swift.contains(
        "/// - Note: Iterating throws ``UndraCallError`` if the call fails in the core or cannot reach it; cancelling the iterating task ends the loop quietly."
    ));
    assert!(swift.contains(
        "/// - Note: A failure is logged and passed to `LoadOptions.onError`; the method does not throw.\n    public func reset() {"
    ));
    // An async constructor checks the handle the way `UndraCore.construct` does.
    assert!(swift.contains(
        "if handle.isNull {\n                throw UndraProtocolError.nullHandle\n            }"
    ));
    // The per-error helpers of the old policy are gone.
    let errors = file(&out[0].1, "Errors.swift");
    assert!(!errors.contains("undraFromReply"));
    assert!(!errors.contains("undraUnexpected"));
}

#[test]
fn swift_free_function_commands_report_through_their_context() {
    // A command on a free function reports through its own `ctx`, by its Swift name without the
    // backticks that escape a keyword; it is not `throws`.
    let mut schema = common::case("objects");
    schema.functions.push(common::function(
        "fire_and_forget",
        "",
        vec![],
        undra_meta::TypeRef::Unit,
        false,
    ));
    schema.functions.push(common::function(
        "default",
        "",
        vec![],
        undra_meta::TypeRef::Unit,
        false,
    ));
    let generator = common::generator_for("objects", &schema);
    let files = generator.swift(&schema).unwrap();
    let swift = file(&files, "Objects.swift");
    let command = swift_method_at_top_level(
        swift,
        "public func fireAndForget(ctx: UndraCore = UndraGoldenObjects.core) {",
    );
    assert!(command.contains("ctx.report(error, operation: \"fireAndForget\")"));
    assert!(!command.contains("throw "));
    let keyword = swift_method_at_top_level(
        swift,
        "public func `default`(ctx: UndraCore = UndraGoldenObjects.core) {",
    );
    assert!(keyword.contains("ctx.report(error, operation: \"default\")"));
}

/// The text of the top-level function that starts at `head`, up to the next top-level item.
fn swift_method_at_top_level<'a>(swift: &'a str, head: &str) -> &'a str {
    let start = swift
        .find(head)
        .unwrap_or_else(|| panic!("no function starts with `{head}`"));
    let rest = &swift[start + head.len()..];
    let end = rest.find("\n}\n").map_or(rest.len(), |i| i + 3);
    &rest[..end]
}

#[test]
fn swift_store_apply_reports_undecodable_changes() {
    let out = all_files("stores", |_| {});
    let stores = file(&out[0].1, "Stores.swift");
    assert!(stores.contains("self.core.report(error, operation: \""));
    assert!(stores.contains(".apply(signal: \\(signal))\")"));
    assert!(!stores.contains("assertionFailure"));
    // A `PatchError` still re-observes the signal.
    assert!(stores.contains("} catch is PatchError {"));
    // A full value is stored only once it decoded whole, so a change with trailing bytes is
    // skipped, not half applied.
    assert!(stores.contains("let value = try "));
    assert!(stores.contains("try reader.finish()\n") && stores.contains(" = value\n"));
    let finish = stores.find("try reader.finish()").unwrap();
    let store = stores.find(" = value\n").unwrap();
    assert!(
        finish < store,
        "a full value is stored before `finish()` checks it"
    );
}

#[test]
fn js_number_maps_wide_integers_to_number() {
    let big = all_files("records", |_| {});
    let small = all_files("records", |g| g.ts_js_number = true);
    let big = file(&big[2].1, "src/types.ts");
    let small = file(&small[2].1, "src/types.ts");
    assert!(
        big.contains("d: bigint;")
            && big.contains("w.writeI64(v.d)")
            && big.contains("r.readU64()")
    );
    assert!(small.contains("d: number;") && small.contains("w.writeI64Number(v.d)"));
    assert!(small.contains("r.readU64Number()") && small.contains("codecs.u64Number"));
    assert!(!small.contains("bigint"));
}

// ----- determinism -----------------------------------------------------------------------

fn shuffled(schema: &Schema) -> Schema {
    let mut s = schema.clone();
    s.records.reverse();
    s.enums.reverse();
    s.objects.reverse();
    s.functions.reverse();
    s.ports.reverse();
    s.queries.reverse();
    for en in &mut s.enums {
        en.variants.reverse();
    }
    s
}

#[test]
fn the_output_does_not_depend_on_the_order_of_the_schema() {
    for case in common::CASES {
        let schema = common::case(case);
        let generator = common::generator_for(case, &schema);
        let other = shuffled(&schema);
        assert_eq!(
            generator.swift(&schema).unwrap(),
            generator.swift(&other).unwrap(),
            "{case}/swift"
        );
        assert_eq!(
            generator.kotlin(&schema).unwrap(),
            generator.kotlin(&other).unwrap(),
            "{case}/kotlin"
        );
        assert_eq!(
            generator.typescript(&schema).unwrap(),
            generator.typescript(&other).unwrap(),
            "{case}/ts"
        );
    }
}

#[test]
fn docs_do_not_change_the_ids_but_do_change_the_output() {
    // The schema hash embedded in every header excludes docs.
    let schema = common::case("full");
    let mut documented = schema.clone();
    documented.records[0].docs = "A different doc.".into();
    let generator = Generator::for_crate("playground-core");
    let a = generator.typescript(&schema).unwrap();
    let b = generator.typescript(&documented).unwrap();
    assert_eq!(file(&a, "ids.ts"), file(&b, "ids.ts"));
    assert_ne!(file(&a, "types.ts"), file(&b, "types.ts"));
}

// ----- structural sanity ------------------------------------------------------------------

/// Checks that brackets balance outside strings and comments.
fn balanced(path: &str, text: &str) -> Result<(), String> {
    let chars: Vec<char> = text.chars().collect();
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut i = 0;
    let mut line = 1;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\n' => line += 1,
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
                i += 2;
                continue;
            }
            '"' | '`' if path.ends_with(".swift") && c == '`' => {
                // A Swift backtick identifier: skip to the closing backtick.
                i += 1;
                while i < chars.len() && chars[i] != '`' {
                    i += 1;
                }
            }
            '"' | '`' => {
                let quote = c;
                i += 1;
                while i < chars.len() && chars[i] != quote {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    if chars.get(i) == Some(&'\n') {
                        line += 1;
                    }
                    i += 1;
                }
            }
            '(' | '[' | '{' => stack.push((c, line)),
            ')' | ']' | '}' => {
                let want = match c {
                    ')' => '(',
                    ']' => '[',
                    _ => '{',
                };
                match stack.pop() {
                    Some((open, _)) if open == want => {}
                    other => return Err(format!("{path}:{line}: `{c}` closes {other:?}")),
                }
            }
            _ => {}
        }
        i += 1;
    }
    match stack.pop() {
        None => Ok(()),
        Some((open, at)) => Err(format!(
            "{path}: `{open}` opened on line {at} is never closed"
        )),
    }
}

#[test]
fn every_generated_file_is_well_formed() {
    for case in common::CASES {
        for (language, files) in all_files(case, |_| {}) {
            for f in files {
                let path = format!("{case}/{language}/{}", f.path);
                assert!(
                    f.contents.ends_with('\n'),
                    "{path} does not end in a newline"
                );
                assert!(!f.contents.ends_with("\n\n"), "{path} ends in a blank line");
                assert!(!f.contents.contains('\t'), "{path} contains a tab");
                // `TODO` is legitimate inside identifiers such as `TODOS` (a query called `todos`).
                for marker in ["TODO(", "// TODO", "unimplemented"] {
                    assert!(!f.contents.contains(marker), "{path} contains {marker}");
                }
                for (n, line) in f.contents.lines().enumerate() {
                    assert!(
                        line == line.trim_end(),
                        "{path}:{}: trailing whitespace",
                        n + 1
                    );
                }
                if f.path.ends_with(".swift") || f.path.ends_with(".kt") || f.path.ends_with(".ts")
                {
                    if let Err(e) = balanced(&f.path, &f.contents) {
                        panic!("{case}/{language}: {e}");
                    }
                }
                // C headers, module maps and R8 rules say it in their own comment syntax.
                assert!(
                    f.contents.starts_with("// Generated by undra-bindgen")
                        || f.contents.lines().next().is_some_and(|line| {
                            line.contains("Generated by undra")
                                && ["/*", "//", "#"].iter().any(|c| line.starts_with(c))
                        })
                        || f.path.ends_with(".json"),
                    "{path} has no generated header"
                );
            }
        }
    }
}

#[test]
fn the_balance_check_catches_a_missing_brace() {
    assert!(balanced("x.kt", "class A { fun f() { } }").is_ok());
    assert!(balanced("x.kt", "class A { fun f() { }").is_err());
    assert!(balanced("x.kt", "val s = \"}\" // )\nclass A { }").is_ok());
    assert!(balanced("x.swift", "let `default` = 1\nstruct A { }").is_ok());
}

#[test]
fn generated_code_never_mentions_a_type_the_schema_does_not_define() {
    // `Todo` is defined by `full`; a Kotlin/TS/Swift reference to it needs no import beyond the
    // generated files themselves, and the runtime is imported explicitly.
    let out = all_files("full", |_| {});
    let kotlin = file(&out[1].1, "Objects.kt");
    assert!(kotlin.contains("import dev.undra.runtime.UndraCore"));
    assert!(kotlin.contains("import dev.undra.runtime.wire.Payloads.CallTarget"));
    let ts = file(&out[2].1, "src/objects.ts");
    assert!(ts.contains("from \"@undra/runtime\""));
    assert!(ts.contains("from \"./types.js\""));
    assert!(ts.contains("from \"./errors.js\""));
}

#[test]
fn documentation_is_carried_into_every_language() {
    let schema = common::case("records");
    let generator = Generator::for_crate(&schema.crate_name);
    let swift = generator.swift(&schema).unwrap();
    let swift = file(&swift, "Types.swift");
    assert!(swift.contains(
        "/// A todo item.\n/// Shown in the list and synced to the server.\npublic struct Todo"
    ));
    assert!(swift.contains("    /// Stable identifier.\n    public var id: UUID"));
    let kotlin = generator.kotlin(&schema).unwrap();
    let kotlin = file(&kotlin, "Types.kt");
    assert!(kotlin.contains("/**\n * A todo item.\n * Shown in the list and synced to the server.\n */\ndata class Todo("));
    assert!(kotlin.contains("    /** Stable identifier. */\n    val id: UUID,"));
    let ts = generator.typescript(&schema).unwrap();
    let ts = file(&ts, "src/types.ts");
    assert!(ts.contains("/**\n * A todo item.\n * Shown in the list and synced to the server.\n */\nexport interface Todo"));
    assert!(ts.contains("  /** Stable identifier. */\n  id: string;"));
}

#[test]
fn comment_terminators_in_docs_cannot_break_out() {
    let mut schema = Schema::new("t");
    schema.records.push(common::record(
        "R",
        "closes */ early and opens /* nested",
        vec![],
    ));
    let generator = Generator::for_crate("t");
    for text in [
        file(&generator.kotlin(&schema).unwrap(), "Types.kt").to_owned(),
        file(&generator.typescript(&schema).unwrap(), "src/types.ts").to_owned(),
    ] {
        assert!(balanced("x.kt", &text).is_ok(), "{text}");
        assert!(!text.contains("closes */ early"), "{text}");
    }
}

// ----- recursive types (Swift) ---------------------------------------------------------------

/// `Types.swift` of `schema`.
fn swift_types(schema: &Schema) -> String {
    let files = Generator::for_crate("t").swift(schema).unwrap();
    file(&files, "Types.swift").to_owned()
}

/// The text of the Swift declaration that starts with `head`, up to its closing line.
fn swift_decl<'a>(swift: &'a str, head: &str) -> &'a str {
    let start = swift
        .find(head)
        .unwrap_or_else(|| panic!("no declaration starts with `{head}` in:\n{swift}"));
    let rest = &swift[start..];
    let end = rest.find("\n}\n").map_or(rest.len(), |i| i + 3);
    &rest[..end]
}

/// A record whose fields are `(name, type)`.
fn record_of(name: &str, fields: Vec<(&str, TypeRef)>) -> undra_meta::RecordDef {
    common::record(
        name,
        "",
        fields
            .into_iter()
            .map(|(n, t)| common::field(n, t))
            .collect(),
    )
}

fn optional(name: &str) -> TypeRef {
    TypeRef::option(common::named(name))
}

#[test]
fn swift_keeps_the_public_shape_of_a_recursive_record_and_boxes_only_its_storage() {
    let types = swift_types(&common::case("recursive"));
    let node = swift_decl(&types, "public struct ListNode");
    // A Swift engineer reads this as an ordinary optional property.
    assert!(
        node.contains("    public var next: ListNode? {\n"),
        "{node}"
    );
    assert!(node.contains("get { _next?.value }"), "{node}");
    assert!(node.contains("set { _next = newValue.map(UndraIndirect.init) }"));
    assert!(node.contains("    private var _next: UndraIndirect<ListNode>?\n"));
    // The public initializer takes the plain optional; the key on the wire and in `Codable`
    // stays `next`, so a missing key decodes as nil and a nil child is omitted.
    assert!(node.contains("public init(value: Int32, next: ListNode?)"));
    assert!(node.contains("self._next = next.map(UndraIndirect.init)"));
    assert!(node.contains("case _next = \"next\""));
    assert!(node.contains("UndraRecord, Sendable, Hashable, Codable"));
    // The wire code reads and writes the public property.
    assert!(node.contains("self.next.undraEncode(&w)"));
    assert!(node.contains("next: Optional<ListNode>.undraDecode(&r)"));
    // An array keeps its elements on the heap: no box, no storage, no coding keys.
    let tree = swift_decl(&types, "public struct Tree");
    assert!(tree.contains("    public var children: [Tree]\n"));
    assert!(
        !tree.contains("UndraIndirect") && !tree.contains("CodingKeys"),
        "{tree}"
    );
}

#[test]
fn swift_emits_the_indirect_box_once_and_only_when_a_record_needs_it() {
    for case in common::CASES {
        let out = all_files(case, |_| {});
        let types = file(&out[0].1, "Types.swift");
        let declarations = types.matches("final class UndraIndirect<").count();
        let uses = out[0]
            .1
            .iter()
            .filter(|f| !f.path.ends_with("Types.swift"))
            .any(|f| f.contents.contains("UndraIndirect"));
        assert!(!uses, "{case}: only Types.swift mentions the box");
        assert_eq!(
            declarations,
            usize::from(*case == "recursive"),
            "{case}: the box is declared exactly when a record holds itself"
        );
    }
    // `records` already has a record with a `Vec<Self>` field and an optional that is not a
    // cycle: neither needs the box.
    let types = swift_types(&common::case("records"));
    assert!(!types.contains("UndraIndirect"));
}

#[test]
fn swift_boxes_each_field_of_a_cycle_through_records_and_only_those() {
    let mut s = Schema::new("t");
    // A holds B, B holds C, C holds B again: B and C are on a cycle, A only leads into it.
    s.records.push(record_of(
        "A",
        vec![("b", optional("B")), ("n", TypeRef::I32)],
    ));
    s.records.push(record_of("B", vec![("c", optional("C"))]));
    s.records.push(record_of(
        "C",
        vec![("b", optional("B")), ("a", optional("A"))],
    ));
    // D and E hold each other through arrays, which never need a box.
    s.records.push(record_of(
        "D",
        vec![("e", TypeRef::vec(common::named("E")))],
    ));
    s.records.push(record_of(
        "E",
        vec![
            ("d", optional("D")),
            ("m", TypeRef::map(TypeRef::String, common::named("D"))),
        ],
    ));
    let types = swift_types(&s);
    // C -> A -> B -> C closes a cycle too, so every edge among A, B and C is on one: A.b, B.c,
    // C.b and C.a.
    assert!(swift_decl(&types, "public struct A").contains("private var _b: UndraIndirect<B>?"));
    assert!(swift_decl(&types, "public struct B").contains("private var _c: UndraIndirect<C>?"));
    let c = swift_decl(&types, "public struct C");
    assert!(c.contains("private var _b: UndraIndirect<B>?"), "{c}");
    assert!(c.contains("private var _a: UndraIndirect<A>?"), "{c}");
    // `n` is not a field that holds anything.
    assert!(swift_decl(&types, "public struct A").contains("    public var n: Int32\n"));
    // D -> E through an array, E -> D through an optional: the array breaks the cycle.
    let d = swift_decl(&types, "public struct D");
    assert!(
        d.contains("    public var e: [E]\n") && !d.contains("UndraIndirect"),
        "{d}"
    );
    let e = swift_decl(&types, "public struct E");
    assert!(
        e.contains("    public var d: D?\n") && !e.contains("UndraIndirect"),
        "{e}"
    );
    assert_eq!(types.matches("final class UndraIndirect<").count(), 1);

    // Break C -> A: A is no longer on a cycle, so its field goes back to a plain property.
    s.records[2].fields.retain(|f| f.name != "a");
    let types = swift_types(&s);
    assert!(swift_decl(&types, "public struct A").contains("    public var b: B?\n"));
    assert!(swift_decl(&types, "public struct B").contains("private var _c: UndraIndirect<C>?"));
    assert!(swift_decl(&types, "public struct C").contains("private var _b: UndraIndirect<B>?"));
}

#[test]
fn swift_names_the_storage_after_the_field_even_when_the_field_is_a_keyword() {
    let mut s = Schema::new("t");
    // A field called `self` is a keyword and is backticked; its storage, `_self`, is not.
    s.records.push(record_of(
        "Node",
        vec![("self", optional("Node")), ("class_name", TypeRef::String)],
    ));
    let types = swift_types(&s);
    let node = swift_decl(&types, "public struct Node");
    assert!(node.contains("    public var `self`: Node? {\n"), "{node}");
    assert!(
        node.contains("private var _self: UndraIndirect<Node>?"),
        "{node}"
    );
    assert!(node.contains("case _self = \"self\""), "{node}");
    assert!(node.contains("case className\n"), "{node}");
}

#[test]
fn swift_makes_an_enum_indirect_when_a_payload_lies_on_a_cycle() {
    let mut s = Schema::new("t");
    // Direct, optional and mutual self reference: all `indirect`.
    s.enums.push(common::enum_def(
        "Direct",
        "",
        vec![
            common::unit_variant("End", 0),
            common::tuple_variant("Next", 1, vec![common::named("Direct")]),
        ],
    ));
    s.enums.push(common::enum_def(
        "Maybe",
        "",
        vec![common::tuple_variant("Next", 0, vec![optional("Maybe")])],
    ));
    s.enums.push(common::enum_def(
        "Ping",
        "",
        vec![common::tuple_variant(
            "Pong",
            0,
            vec![common::named("Pong")],
        )],
    ));
    s.enums.push(common::enum_def(
        "Pong",
        "",
        vec![
            common::tuple_variant("Ping", 0, vec![optional("Ping")]),
            common::unit_variant("Stop", 1),
        ],
    ));
    // An enum that leads into a cycle without being on it, an array, and a map: none of them.
    s.enums.push(common::enum_def(
        "Into",
        "",
        vec![common::tuple_variant(
            "Ping",
            0,
            vec![common::named("Ping")],
        )],
    ));
    s.enums.push(common::enum_def(
        "Listy",
        "",
        vec![
            common::tuple_variant("Items", 0, vec![TypeRef::vec(common::named("Listy"))]),
            common::tuple_variant(
                "Named",
                1,
                vec![TypeRef::map(TypeRef::String, common::named("Listy"))],
            ),
        ],
    ));
    // A record on a cycle with an enum: the enum is `indirect`, the record boxes its field.
    s.enums.push(common::enum_def(
        "Expr",
        "",
        vec![
            common::tuple_variant("Num", 0, vec![TypeRef::F64]),
            common::tuple_variant("Block", 1, vec![common::named("Block")]),
        ],
    ));
    s.records
        .push(record_of("Block", vec![("last", optional("Expr"))]));
    // An error that wraps itself.
    s.enums.push(common::error_def(
        "Failure",
        "",
        vec![
            common::with_message(common::unit_variant("Leaf", 0), "leaf"),
            common::with_message(
                common::tuple_variant("Wrapped", 1, vec![TypeRef::U8, common::named("Failure")]),
                "wrapped",
            ),
        ],
    ));
    let files = Generator::for_crate("t").swift(&s).unwrap();
    let types = file(&files, "Types.swift");
    for name in ["Direct", "Maybe", "Ping", "Pong", "Expr"] {
        assert!(
            types.contains(&format!("public indirect enum {name}: UndraEnum")),
            "{name} must be indirect:\n{types}"
        );
    }
    for name in ["Into", "Listy"] {
        assert!(
            types.contains(&format!("public enum {name}: UndraEnum")),
            "{name} must not be indirect:\n{types}"
        );
    }
    assert!(
        swift_decl(types, "public struct Block")
            .contains("private var _last: UndraIndirect<Expr>?")
    );
    assert!(file(&files, "Errors.swift").contains("public indirect enum Failure: UndraError"));
}

/// Store signals whose types recurse: an enum whose base case is its last variant, a record that
/// holds it, and two enums that need each other unless one takes its other variant. A signal is
/// initialised with a placeholder until the core's first change-set arrives; the placeholder of a
/// recursive type is built from the first variant that does not need the type itself, wherever the
/// schema lists it, and never from an optional, array or map.
fn recursive_placeholders() -> Schema {
    let mut s = Schema::new("t");
    s.enums.push(common::enum_def(
        "Sum",
        "",
        vec![
            common::tuple_variant("Add", 0, vec![common::named("Sum"), common::named("Sum")]),
            common::tuple_variant("Neg", 1, vec![common::named("Sum")]),
            common::unit_variant("Zero", 2),
        ],
    ));
    s.records.push(record_of(
        "Frame",
        vec![
            ("sum", common::named("Sum")),
            ("next", optional("Frame")),
            ("kids", TypeRef::vec(common::named("Frame"))),
        ],
    ));
    // Two types that need each other unless one of them takes its other variant.
    s.enums.push(common::enum_def(
        "Left",
        "",
        vec![
            common::tuple_variant("Over", 0, vec![common::named("Right")]),
            common::unit_variant("Done", 1),
        ],
    ));
    s.enums.push(common::enum_def(
        "Right",
        "",
        vec![common::tuple_variant(
            "Over",
            0,
            vec![common::named("Left")],
        )],
    ));
    s.objects.push(common::store(
        common::object(
            "Box",
            "",
            vec![common::ctor("Box", "new", vec![], false)],
            vec![],
        ),
        vec![
            ("sum", common::named("Sum"), false, None),
            ("frame", common::named("Frame"), false, None),
            ("left", common::named("Left"), false, None),
            ("right", common::named("Right"), false, None),
        ],
    ));
    s
}

#[test]
fn swift_placeholders_of_a_recursive_type_use_its_base_case() {
    // A signal is initialised with a placeholder until the core's first change-set arrives. The
    // placeholder of a recursive type is built from its first variant that does not need the
    // type itself, wherever the schema lists it, and never from an optional, array or map.
    let s = recursive_placeholders();
    let files = Generator::for_crate("t").swift(&s).unwrap();
    let stores = file(&files, "Stores.swift");
    assert!(
        stores.contains("public private(set) var sum: Sum = Sum.zero\n"),
        "{stores}"
    );
    assert!(
        stores.contains(
            "public private(set) var frame: Frame = Frame(sum: Sum.zero, next: nil, kids: [])\n"
        ),
        "{stores}"
    );
    // `Left` cannot take `Over` (it needs a `Right`, which needs a `Left` again): it takes `Done`.
    assert!(
        stores.contains("public private(set) var left: Left = Left.done\n"),
        "{stores}"
    );
    assert!(
        stores.contains("public private(set) var right: Right = Right.over(Left.done)\n"),
        "{stores}"
    );
    for f in &files {
        for trap in ["fatalError", "preconditionFailure", "try!"] {
            assert!(!f.contents.contains(trap), "{}: `{trap}`", f.path);
        }
    }
}

#[test]
fn kotlin_and_typescript_placeholders_of_a_recursive_type_use_its_base_case() {
    // The same search as Swift's (`crate::zero`). Before it, Kotlin expanded the first variant of
    // `Sum` exponentially until a depth guard wrote `error("recursive default")`, which throws
    // when the store is created, and TypeScript overflowed the generator's stack.
    let s = recursive_placeholders();
    let kotlin = Generator::for_crate("t").kotlin(&s).unwrap();
    let stores = file(&kotlin, "Stores.kt");
    for expected in [
        "private val _sum: MutableStateFlow<Sum> = signal(Sum.Zero)\n",
        "private val _frame: MutableStateFlow<Frame> = signal(Frame(sum = Sum.Zero, next = null, kids = emptyList()))\n",
        "private val _left: MutableStateFlow<Left> = signal(Left.Done)\n",
        "private val _right: MutableStateFlow<Right> = signal(Right.Over(value = Left.Done))\n",
    ] {
        assert!(stores.contains(expected), "{expected}\n{stores}");
    }
    let ts = Generator::for_crate("t").typescript(&s).unwrap();
    let stores = file(&ts, "stores.ts");
    for expected in [
        "readonly sum: Signal<Sum> = new Signal<Sum>({ kind: \"zero\" });\n",
        "readonly frame: Signal<Frame> = new Signal<Frame>({ sum: { kind: \"zero\" }, next: null, kids: [] });\n",
        "readonly left: Signal<Left> = new Signal<Left>({ kind: \"done\" });\n",
        "readonly right: Signal<Right> = new Signal<Right>({ kind: \"over\", value: { kind: \"done\" } });\n",
    ] {
        assert!(stores.contains(expected), "{expected}\n{stores}");
    }
    for f in kotlin.iter().chain(&ts) {
        assert!(!f.contents.contains("recursive default"), "{}", f.path);
        assert!(!f.contents.contains("as never"), "{}", f.path);
    }
}

#[test]
fn kotlin_placeholders_of_deeply_nested_records_are_built_whole() {
    // The old depth guard stopped at nine levels and wrote a trap for a type that is not recursive
    // at all; the path-based search has no depth limit.
    let mut s = Schema::new("t");
    s.records.push(record_of("L0", vec![("v", TypeRef::I32)]));
    for level in 1..12 {
        let inner = format!("L{}", level - 1);
        s.records.push(record_of(
            &format!("L{level}"),
            vec![("inner", common::named(&inner))],
        ));
    }
    s.objects.push(common::store(
        common::object(
            "Deep",
            "",
            vec![common::ctor("Deep", "new", vec![], false)],
            vec![],
        ),
        vec![("top", common::named("L11"), false, None)],
    ));
    let kotlin = Generator::for_crate("t").kotlin(&s).unwrap();
    let stores = file(&kotlin, "Stores.kt");
    assert!(!stores.contains("recursive default"), "{stores}");
    assert!(
        stores.contains("signal(L11(inner = L10(inner = L9("),
        "{stores}"
    );
    assert!(stores.contains("L0(v = 0)"), "{stores}");
}

#[test]
fn kotlin_and_typescript_survive_a_type_that_has_no_value_at_all() {
    // As for Swift: a schema is data, and a type every way to build which needs itself must
    // neither loop nor overflow the generator's stack. No Rust core can have such a store.
    let mut s = Schema::new("t");
    s.enums.push(common::enum_def(
        "Never2",
        "",
        vec![common::tuple_variant(
            "Again",
            0,
            vec![common::named("Never2")],
        )],
    ));
    s.records
        .push(record_of("Own", vec![("again", common::named("Own"))]));
    s.objects.push(common::store(
        common::object(
            "Holder",
            "",
            vec![common::ctor("Holder", "new", vec![], false)],
            vec![],
        ),
        vec![
            ("never", common::named("Never2"), false, None),
            ("own", common::named("Own"), false, None),
        ],
    ));
    let kotlin = Generator::for_crate("t").kotlin(&s).unwrap();
    assert!(file(&kotlin, "Stores.kt").contains("class Holder"));
    let ts = Generator::for_crate("t").typescript(&s).unwrap();
    let stores = file(&ts, "stores.ts");
    assert!(
        stores.contains("new Signal<Never2>(undefined as never)"),
        "{stores}"
    );
}

#[test]
fn swift_survives_a_type_that_has_no_value_at_all() {
    // Nothing in Rust can be like this (`enum E { A(Box<E>) }` has no value), but a schema is
    // data: the generator must neither loop nor overflow the stack on it.
    let mut s = Schema::new("t");
    s.enums.push(common::enum_def(
        "Never2",
        "",
        vec![common::tuple_variant(
            "Again",
            0,
            vec![common::named("Never2")],
        )],
    ));
    s.records
        .push(record_of("Own", vec![("again", common::named("Own"))]));
    // Signals of both types need a placeholder, which no value can give.
    s.objects.push(common::store(
        common::object(
            "Holder",
            "",
            vec![common::ctor("Holder", "new", vec![], false)],
            vec![],
        ),
        vec![
            ("never", common::named("Never2"), false, None),
            ("own", common::named("Own"), false, None),
        ],
    ));
    let files = Generator::for_crate("t").swift(&s).unwrap();
    let types = file(&files, "Types.swift");
    assert!(types.contains("public indirect enum Never2"));
    // A record that holds itself outright is boxed too, so the layout stays finite.
    assert!(
        types.contains("private var _again: UndraIndirect<Own>\n"),
        "{types}"
    );
    assert!(file(&files, "Stores.swift").contains("public final class Holder"));
}

// ----- derived lists (ADR-039 decision 7) -------------------------------------------------

/// The declaration lines of the store property `name` (the doc line above included), with the
/// name replaced, so two properties can be compared modulo their names.
fn declaration(text: &str, lang: &str, name: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let is_decl = |line: &str| match lang {
        "swift" => line.contains(&format!(" var {name}: ")),
        "kotlin" => {
            line.contains(&format!(" val {name}: ")) || line.contains(&format!(" val _{name}: "))
        }
        _ => line.contains(&format!("readonly {name}: ")),
    };
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if is_decl(line) {
            if i > 0
                && (lines[i - 1].trim_start().starts_with("/*")
                    || lines[i - 1].trim_start().starts_with("///"))
            {
                out.push(lines[i - 1].trim().to_owned());
            }
            out.push(line.replace(name, "NAME").trim().to_owned());
        }
    }
    assert!(!out.is_empty(), "{lang}: no declaration of {name}");
    out
}

#[test]
fn a_derived_list_is_declared_exactly_as_a_computed_list_and_also_applies_patches() {
    // A derived list is `computed: true` with a `key` (ADR-039): `visible` in the stores case.
    // `done_todos`, added here, is the same type, computed, without a key. Their generated
    // declarations are the same; only `visible`'s `apply` has the keyed-patch case. No new
    // generated surface.
    let mut schema = common::case("stores");
    let todos = schema
        .objects
        .iter_mut()
        .find(|o| o.name == "Todos")
        .and_then(|o| o.store.as_mut())
        .unwrap();
    let visible = todos.signals.iter().find(|s| s.name == "visible").unwrap();
    assert!(visible.computed && visible.key.as_deref() == Some("id"));
    let mut done = visible.clone();
    done.name = "done_todos".into();
    done.key = None;
    done.signal_id = u32::try_from(todos.signals.len()).unwrap();
    todos.signals.push(done);
    let generator = common::generator_for("stores", &schema);
    let outputs = [
        ("swift".to_owned(), generator.swift(&schema).unwrap()),
        ("kotlin".to_owned(), generator.kotlin(&schema).unwrap()),
        ("ts".to_owned(), generator.typescript(&schema).unwrap()),
    ];
    for (lang, files) in outputs {
        let (stores, derived, plain, patch_of) = match lang.as_str() {
            "swift" => (
                "Stores.swift",
                "visible",
                "doneTodos",
                "try applyPatch(ops, to: &self.NAME)",
            ),
            "kotlin" => (
                "Stores.kt",
                "visible",
                "doneTodos",
                "_NAME.value = KeyedPatch.applyPatch(_NAME.value, ops)",
            ),
            _ => (
                "stores.ts",
                "visible",
                "doneTodos",
                "this.NAME._set(applyPatch(this.NAME.peek(), ops));",
            ),
        };
        let text = file(&files, stores);
        assert_eq!(
            declaration(text, &lang, derived),
            declaration(text, &lang, plain),
            "{lang}: a derived list's declaration is a computed list's"
        );
        assert!(
            declaration(text, &lang, derived)
                .iter()
                .any(|line| line.contains("Computed by the core; read-only.")),
            "{lang}: documented read-only"
        );
        assert!(
            text.contains(&patch_of.replace("NAME", derived)),
            "{lang}: the derived list applies keyed patches"
        );
        assert!(
            !text.contains(&patch_of.replace("NAME", plain)),
            "{lang}: a computed list without a key is only ever replaced"
        );
    }
}
