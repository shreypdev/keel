//! Behaviour of the generators that the golden files do not pin: configuration
//! options, determinism, file layout and structural sanity of every output.

mod common;

use undra_bindgen::{GeneratedFile, Generator};
use undra_meta::Schema;

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

    assert_eq!(
        paths(generator.swift(&schema).unwrap()),
        [
            "Types", "Errors", "Objects", "Stores", "Ports", "Queries", "Ids"
        ]
        .map(|n| format!("Sources/PlaygroundCore/Generated/{n}.swift"))
    );
    assert_eq!(
        paths(generator.kotlin(&schema).unwrap()),
        [
            "Types", "Errors", "Objects", "Stores", "Ports", "Queries", "Ids"
        ]
        .map(|n| format!("src/main/kotlin/dev/undra/generated/playground_core/{n}.kt"))
    );
    let mut expected: Vec<String> = [
        "types", "errors", "objects", "stores", "ports", "queries", "ids", "index",
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
fn typed_throws_can_be_turned_off() {
    let typed = all_files("objects", |_| {});
    let untyped = all_files("objects", |g| g.swift_typed_throws = false);
    let typed = file(&typed[0].1, "Objects.swift");
    let untyped = file(&untyped[0].1, "Objects.swift");
    assert!(typed.contains("throws(CalcError)"));
    assert!(typed.contains("undraUnexpected(error)"));
    assert!(!untyped.contains("throws(CalcError)"));
    assert!(!untyped.contains("guard let typed"));
    assert!(untyped.contains("throw CalcError.undraFromReply(error) ?? error"));
    // Plain `throws` on the same methods.
    assert!(untyped.contains("public func divide(a: Int64, b: Int64) throws -> Int64"));
    assert!(untyped.contains("public func lookup(id: UUID) async throws -> Todo"));
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
    // A typed error maps the failure; an untyped stream leaves the runtime's default.
    assert!(swift.contains("mapError: { CalcError.undraFromReply($0) ?? $0 }"));
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
        // Synchronous methods without a `Result` keep the shape of SPEC section 10.1.
        assert!(swift.contains("public func add(a: Int32, b: Int32) -> Int32 {"));
    }
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
                assert!(
                    f.contents.starts_with("// Generated by undra-bindgen")
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
