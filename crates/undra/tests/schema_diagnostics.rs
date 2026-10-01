//! The diagnostics of whole-schema validation, reached the way a user reaches them: a core written
//! with the macros compiles (each item is fine alone), and `undra build` / `undra bindgen` reject
//! the schema the registrations add up to. The macros cannot raise these (they see one item at a
//! time); `crates/undra-bindgen/tests/diagnostics.rs` locks their messages for hand-built schemas
//! and `crates/undra-macros/tests/catalogue.rs` audits that every code has an emitter and a golden.
//!
//! This test binary registers only the types below (plus whatever `undra` links in), so what
//! `collect_schema` returns is what a core made of these items would hand to the generators.

#![allow(non_snake_case, dead_code)]

use undra_bindgen::validate;
use undra_meta::collect_schema;

mod a {
    /// One `Todo`.
    #[undra::api]
    pub struct Todo {
        pub id: u32,
    }
}

mod b {
    /// Another `Todo`: a different module, so Rust is happy; the schema names types, not paths.
    #[undra::api]
    pub struct Todo {
        pub title: String,
    }
}

/// Two fields that the generated code names the same: `due_date` and `dueDate` are both
/// `dueDate` in Swift, Kotlin and TypeScript.
#[undra::api]
pub struct Task {
    pub due_date: u64,
    pub dueDate: u64,
}

/// A type named like something the generated bindings use (`Signal` is a runtime type).
#[undra::api]
pub struct Signal {
    pub level: u8,
}

/// What `undra bindgen` says about `schema` with `code`.
fn messages_of(schema: &undra_meta::Schema, code: &str) -> Vec<String> {
    validate(schema)
        .expect_err("the schema of this core is not valid")
        .into_iter()
        .filter(|e| e.code() == code)
        .map(|e| e.to_string())
        .collect()
}

/// The schema of the whole core: validation stops at the whole-schema errors (here the duplicate
/// `Todo`) and reports nothing the generators would add, as it does for a user.
fn messages(code: &str) -> Vec<String> {
    messages_of(&collect_schema("schema-diagnostics"), code)
}

/// The schema without the two `Todo`s, so that what is left reaches the generators' own checks.
fn messages_without_the_duplicate(code: &str) -> Vec<String> {
    let mut schema = collect_schema("schema-diagnostics");
    schema.records.retain(|r| r.name != "Todo");
    messages_of(&schema, code)
}

#[test]
fn two_types_with_one_name_are_e0050() {
    let all = messages("E0050");
    let duplicate = all
        .iter()
        .find(|m| m.contains("duplicate type name `Todo`"))
        .unwrap_or_else(|| panic!("no duplicate `Todo` in {all:#?}"));
    let lines: Vec<&str> = duplicate.lines().collect();
    assert_eq!(lines.len(), 4, "{duplicate}");
    assert!(lines[0].starts_with("error[undra::E0050]: "), "{duplicate}");
    assert_eq!(
        lines[3],
        "  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0050"
    );
}

#[test]
fn a_type_named_like_a_generated_name_is_e0050() {
    let all = messages_without_the_duplicate("E0050");
    assert!(
        all.iter()
            .any(|m| m.contains("record `Signal` has a name the generated code depends on")),
        "{all:#?}"
    );
}

#[test]
fn fields_that_become_the_same_name_are_e0051() {
    let all = messages_without_the_duplicate("E0051");
    let message = all
        .iter()
        .find(|m| m.contains("`due_date`, `dueDate` all become `dueDate`"))
        .unwrap_or_else(|| panic!("no collision in {all:#?}"));
    assert!(message.contains("at record Task"), "{message}");
    assert!(message.contains("= help: rename one of them"), "{message}");
}
