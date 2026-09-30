//! Case conversion and reserved-word escaping for the three target languages.
//!
//! Rust `snake_case` field, parameter and method names become `camelCase`
//! (`UPPER_SNAKE` for Kotlin enum entries and constants). Type names are
//! already `PascalCase` in Rust and are kept exactly as declared, so a name in
//! the schema is the name in every language. Identifiers that collide with a
//! keyword of the target language are escaped the way that language spells it:
//!
//! * Swift: backticks (`` `default` ``);
//! * Kotlin: backticks for hard keywords (`` `object` ``);
//! * TypeScript: a trailing underscore for binding identifiers (`default_`);
//!   property and method names may be reserved words in TypeScript and are not
//!   escaped.
//!
//! ```
//! use keel_bindgen::naming;
//!
//! assert_eq!(naming::camel("set_filter"), "setFilter");
//! assert_eq!(naming::pascal("todo_store"), "TodoStore");
//! assert_eq!(naming::upper_snake("EmptyTitle"), "EMPTY_TITLE");
//! assert_eq!(naming::swift_ident("default"), "`default`");
//! assert_eq!(naming::kotlin_ident("object"), "`object`");
//! assert_eq!(naming::ts_ident("delete"), "delete_");
//! ```

use heck::{ToLowerCamelCase, ToShoutySnakeCase, ToUpperCamelCase};

/// `snake_case` or `PascalCase` to `lowerCamelCase`: `set_filter` to
/// `setFilter`, `EmptyTitle` to `emptyTitle`.
#[must_use]
pub fn camel(name: &str) -> String {
    name.to_lower_camel_case()
}

/// `snake_case` to `PascalCase`: `todo_store` to `TodoStore`.
#[must_use]
pub fn pascal(name: &str) -> String {
    name.to_upper_camel_case()
}

/// `snake_case` or `PascalCase` to `UPPER_SNAKE_CASE`: `EmptyTitle` to
/// `EMPTY_TITLE`.
#[must_use]
pub fn upper_snake(name: &str) -> String {
    name.to_shouty_snake_case()
}

/// Whether `name` is usable as an identifier in all three languages: ASCII
/// letters, digits and underscores, not starting with a digit, not empty.
#[must_use]
pub fn is_valid_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_') && name.chars().any(|c| c != '_')
}

/// Swift keywords that cannot be used as identifiers without backticks.
const SWIFT_RESERVED: &[&str] = &[
    "Any",
    "Protocol",
    "Self",
    "Type",
    "as",
    "associatedtype",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "continue",
    "default",
    "defer",
    "deinit",
    "do",
    "else",
    "enum",
    "extension",
    "fallthrough",
    "false",
    "fileprivate",
    "for",
    "func",
    "guard",
    "if",
    "import",
    "in",
    "init",
    "inout",
    "internal",
    "is",
    "let",
    "nil",
    "open",
    "operator",
    "precedencegroup",
    "private",
    "protocol",
    "public",
    "repeat",
    "rethrows",
    "return",
    "self",
    "static",
    "struct",
    "subscript",
    "super",
    "switch",
    "throw",
    "throws",
    "true",
    "try",
    "typealias",
    "var",
    "where",
    "while",
];

/// Kotlin hard keywords (soft keywords and modifiers are legal identifiers).
const KOTLIN_RESERVED: &[&str] = &[
    "as",
    "break",
    "class",
    "continue",
    "do",
    "else",
    "false",
    "for",
    "fun",
    "if",
    "in",
    "interface",
    "is",
    "null",
    "object",
    "package",
    "return",
    "super",
    "this",
    "throw",
    "true",
    "try",
    "typealias",
    "typeof",
    "val",
    "var",
    "when",
    "while",
];

/// TypeScript / JavaScript reserved words, including the strict-mode ones and
/// the names a module may not bind.
const TS_RESERVED: &[&str] = &[
    "arguments",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "eval",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// Member names a TypeScript class cannot declare as methods or fields.
const TS_RESERVED_MEMBERS: &[&str] = &["constructor", "prototype", "__proto__"];

/// Whether `name` is a Swift keyword.
#[must_use]
pub fn is_swift_reserved(name: &str) -> bool {
    SWIFT_RESERVED.contains(&name)
}

/// Whether `name` is a Kotlin hard keyword.
#[must_use]
pub fn is_kotlin_reserved(name: &str) -> bool {
    KOTLIN_RESERVED.contains(&name)
}

/// Whether `name` is a TypeScript reserved word.
#[must_use]
pub fn is_ts_reserved(name: &str) -> bool {
    TS_RESERVED.contains(&name)
}

/// A Swift identifier: backticked when `name` is a keyword.
#[must_use]
pub fn swift_ident(name: &str) -> String {
    if is_swift_reserved(name) {
        format!("`{name}`")
    } else {
        name.to_owned()
    }
}

/// A Swift stored-property name on an `@Observable` class: the macro rejects
/// backticked stored properties, so a reserved word gets a trailing underscore
/// instead (`default` becomes `default_`). The schema name is unchanged; only
/// the Swift spelling moves.
#[must_use]
pub fn swift_stored_property(name: &str) -> String {
    if is_swift_reserved(name) {
        format!("{name}_")
    } else {
        name.to_owned()
    }
}

/// A Kotlin identifier: backticked when `name` is a hard keyword.
#[must_use]
pub fn kotlin_ident(name: &str) -> String {
    if is_kotlin_reserved(name) {
        format!("`{name}`")
    } else {
        name.to_owned()
    }
}

/// A TypeScript binding identifier (parameter, local): a trailing underscore
/// when `name` is a reserved word.
#[must_use]
pub fn ts_ident(name: &str) -> String {
    if is_ts_reserved(name) {
        format!("{name}_")
    } else {
        name.to_owned()
    }
}

/// A TypeScript property or method name. Reserved words are legal there, so
/// only the few names a class cannot declare are escaped.
#[must_use]
pub fn ts_member(name: &str) -> String {
    if TS_RESERVED_MEMBERS.contains(&name) {
        format!("{name}_")
    } else {
        name.to_owned()
    }
}

/// `name` with underscores appended until it is not in `taken`. Generated
/// locals use this so a schema parameter called `w` or `body` never shadows
/// the writer or the reply.
#[must_use]
pub fn avoid(name: &str, taken: &[&str]) -> String {
    let mut out = name.to_owned();
    while taken.contains(&out.as_str()) {
        out.push('_');
    }
    out
}

/// The name of field number `index` of a tuple variant with `count` fields:
/// `value` for a single field, `value0`, `value1`, ... otherwise.
#[must_use]
pub fn tuple_field(index: usize, count: usize) -> String {
    if count == 1 {
        "value".to_owned()
    } else {
        format!("value{index}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_conversions() {
        assert_eq!(camel("set_filter"), "setFilter");
        assert_eq!(camel("EmptyTitle"), "emptyTitle");
        assert_eq!(camel("id"), "id");
        assert_eq!(camel("http_2_error"), "http2Error");
        assert_eq!(pascal("todo_store"), "TodoStore");
        assert_eq!(pascal("todos"), "Todos");
        assert_eq!(upper_snake("EmptyTitle"), "EMPTY_TITLE");
        assert_eq!(upper_snake("set_filter"), "SET_FILTER");
        assert_eq!(upper_snake("Wifi"), "WIFI");
        assert_eq!(camel("HTTPError"), "httpError");
    }

    #[test]
    fn reserved_words_are_escaped_per_language() {
        for word in ["default", "in", "where", "init", "self", "case"] {
            assert!(swift_ident(word).starts_with('`'), "{word}");
        }
        for word in ["object", "when", "fun", "in", "is", "val"] {
            assert!(kotlin_ident(word).starts_with('`'), "{word}");
        }
        for word in ["default", "delete", "new", "function", "let", "class"] {
            assert_eq!(ts_ident(word), format!("{word}_"));
        }
        assert_eq!(swift_ident("title"), "title");
        assert_eq!(kotlin_ident("title"), "title");
        assert_eq!(ts_ident("title"), "title");
        // Kotlin soft keywords and Swift contextual keywords stay bare.
        assert_eq!(kotlin_ident("data"), "data");
        assert_eq!(kotlin_ident("open"), "open");
        assert_eq!(swift_ident("some"), "some");
    }

    #[test]
    fn ts_members_may_be_reserved_words() {
        assert_eq!(ts_member("delete"), "delete");
        assert_eq!(ts_member("default"), "default");
        assert_eq!(ts_member("constructor"), "constructor_");
    }

    #[test]
    fn identifier_validation() {
        assert!(is_valid_ident("Todo"));
        assert!(is_valid_ident("_x1"));
        assert!(!is_valid_ident(""));
        assert!(!is_valid_ident("1x"));
        assert!(!is_valid_ident("a-b"));
        assert!(!is_valid_ident("_"));
        assert!(!is_valid_ident("Ünï"));
        assert!(!is_valid_ident("a::b"));
    }

    #[test]
    fn avoid_appends_underscores_until_free() {
        assert_eq!(avoid("w", &["a", "b"]), "w");
        assert_eq!(avoid("w", &["w"]), "w_");
        assert_eq!(avoid("w", &["w", "w_"]), "w__");
    }

    #[test]
    fn tuple_field_names() {
        assert_eq!(tuple_field(0, 1), "value");
        assert_eq!(tuple_field(0, 2), "value0");
        assert_eq!(tuple_field(1, 2), "value1");
    }
}
