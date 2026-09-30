//! A small reader for the subset of TOML that `keel.toml` uses.
//!
//! `keel.toml` is written by `keel init` and edited by hand, and its shape is fixed: tables
//! (`[project]`, `[bindings]`, ...) holding strings, booleans, integers and arrays of those.
//! Reading that subset here keeps the dependency list of the CLI what `docs/SPEC.md` section 13
//! says it is. Anything outside the subset (inline tables, floats, dates, arrays of tables) is
//! refused with the line it is on rather than guessed at.

use std::collections::BTreeMap;

/// A parsed value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// A string (basic or literal).
    Str(String),
    /// `true` or `false`.
    Bool(bool),
    /// An integer.
    Int(i64),
    /// An array of values.
    Array(Vec<Value>),
}

impl Value {
    /// The kind of value, for messages ("a string", "an array").
    #[must_use]
    pub fn describe(&self) -> &'static str {
        match self {
            Value::Str(_) => "a string",
            Value::Bool(_) => "a boolean",
            Value::Int(_) => "an integer",
            Value::Array(_) => "an array",
        }
    }
}

/// A key with the line it was written on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The value.
    pub value: Value,
    /// The 1-based line of the key.
    pub line: usize,
}

/// A parsed document: the root table (named `""`) and every `[table]`, keyed by dotted name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    /// Table name to its keys.
    pub tables: BTreeMap<String, BTreeMap<String, Entry>>,
}

/// A syntax error and the line it is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// The 1-based line.
    pub line: usize,
    /// What is wrong.
    pub message: String,
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Parses `text`.
///
/// # Errors
///
/// The first construct that is not valid TOML or not in the supported subset.
pub fn parse(text: &str) -> Result<Document, ParseError> {
    let mut doc = Document::default();
    doc.tables.insert(String::new(), BTreeMap::new());
    let mut current = String::new();
    let mut lines = text.lines().enumerate().peekable();
    while let Some((index, raw)) = lines.next() {
        let line_no = index + 1;
        let line = strip_comment(raw).trim().to_owned();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            if rest.starts_with('[') {
                return Err(err(
                    line_no,
                    "arrays of tables (`[[name]]`) are not supported in keel.toml",
                ));
            }
            let Some(name) = rest.strip_suffix(']') else {
                return Err(err(line_no, "a table header must end with `]`"));
            };
            let name = name.trim();
            if name.is_empty() || !name.split('.').all(is_bare_key) {
                return Err(err(line_no, format!("`{name}` is not a valid table name")));
            }
            if doc.tables.contains_key(name) {
                return Err(err(line_no, format!("the table [{name}] is defined twice")));
            }
            doc.tables.insert(name.to_owned(), BTreeMap::new());
            current = name.to_owned();
            continue;
        }
        let Some((key, value_text)) = line.split_once('=') else {
            return Err(err(line_no, "expected `key = value` or a `[table]` header"));
        };
        let key = key.trim();
        if !is_bare_key(key) {
            return Err(err(
                line_no,
                format!("`{key}` is not a valid key (use letters, digits, `-` and `_`)"),
            ));
        }
        // An array may continue over several lines: gather until the brackets balance.
        let mut value_text = value_text.trim().to_owned();
        while value_text.starts_with('[') && !brackets_balanced(&value_text) {
            let Some((_, next)) = lines.next() else {
                return Err(err(line_no, "this array is never closed with `]`"));
            };
            value_text.push(' ');
            value_text.push_str(strip_comment(next).trim());
        }
        let value = parse_value(&value_text, line_no)?;
        let table = doc
            .tables
            .get_mut(&current)
            .expect("the current table exists");
        if table.contains_key(key) {
            return Err(err(line_no, format!("the key `{key}` is set twice")));
        }
        table.insert(
            key.to_owned(),
            Entry {
                value,
                line: line_no,
            },
        );
    }
    Ok(doc)
}

fn err(line: usize, message: impl Into<String>) -> ParseError {
    ParseError {
        line,
        message: message.into(),
    }
}

fn is_bare_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Removes a `#` comment that is not inside a string.
fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' && q == '"' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '#' => return &line[..i],
                _ => {}
            },
        }
    }
    line
}

fn brackets_balanced(text: &str) -> bool {
    let mut depth = 0_i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in text.chars() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' && q == '"' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            },
        }
    }
    depth <= 0
}

fn parse_value(text: &str, line: usize) -> Result<Value, ParseError> {
    let text = text.trim();
    if let Some(inner) = text.strip_prefix('[') {
        let Some(inner) = inner.strip_suffix(']') else {
            return Err(err(line, "an array must end with `]`"));
        };
        let mut items = Vec::new();
        for part in split_top_level(inner, line)? {
            let part = part.trim();
            if part.is_empty() {
                continue; // a trailing comma
            }
            items.push(parse_value(part, line)?);
        }
        return Ok(Value::Array(items));
    }
    if let Some(rest) = text.strip_prefix('"') {
        return parse_basic_string(rest, line).map(Value::Str);
    }
    if let Some(rest) = text.strip_prefix('\'') {
        return match rest.strip_suffix('\'') {
            Some(body) if !body.contains('\'') => Ok(Value::Str(body.to_owned())),
            _ => Err(err(
                line,
                "a '...' string must end with `'` and contain no other `'`",
            )),
        };
    }
    match text {
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        "" => return Err(err(line, "a key needs a value")),
        _ => {}
    }
    if text.starts_with('{') {
        return Err(err(
            line,
            "inline tables are not supported in keel.toml; use a [table] instead",
        ));
    }
    let digits: String = text.chars().filter(|c| *c != '_').collect();
    if let Ok(n) = digits.parse::<i64>() {
        return Ok(Value::Int(n));
    }
    Err(err(
        line,
        format!("`{text}` is not a string, boolean, integer or array (strings need quotes)"),
    ))
}

/// Splits array contents on commas that are outside strings and nested arrays.
fn split_top_level(text: &str, line: usize) -> Result<Vec<&str>, ParseError> {
    let mut parts = Vec::new();
    let mut depth = 0_i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' && q == '"' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '[' => depth += 1,
                ']' => depth -= 1,
                ',' if depth == 0 => {
                    parts.push(&text[start..i]);
                    start = i + 1;
                }
                _ => {}
            },
        }
    }
    if quote.is_some() {
        return Err(err(line, "a string is never closed"));
    }
    parts.push(&text[start..]);
    Ok(parts)
}

fn parse_basic_string(after_open: &str, line: usize) -> Result<String, ParseError> {
    let mut out = String::new();
    let mut chars = after_open.chars();
    loop {
        let Some(c) = chars.next() else {
            return Err(err(line, "a string is never closed"));
        };
        match c {
            '"' => {
                return if chars.as_str().trim().is_empty() {
                    Ok(out)
                } else {
                    Err(err(line, "unexpected text after the closing quote"))
                };
            }
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    let code = u32::from_str_radix(&hex, 16)
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or_else(|| err(line, format!("`\\u{hex}` is not a valid escape")))?;
                    out.push(code);
                }
                Some(other) => return Err(err(line, format!("`\\{other}` is not a valid escape"))),
                None => return Err(err(line, "a string is never closed")),
            },
            other => out.push(other),
        }
    }
}

/// Escapes `s` as the body of a basic TOML string.
#[must_use]
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(doc: &'a Document, table: &str, key: &str) -> &'a Value {
        &doc.tables[table][key].value
    }

    #[test]
    fn reads_tables_and_scalars() {
        let doc = parse(
            "# a comment\nname = \"todo\" # trailing\n[project]\nid = 'com.example.todo'\nflag = true\nn = 1_000\n",
        )
        .unwrap();
        assert_eq!(get(&doc, "", "name"), &Value::Str("todo".into()));
        assert_eq!(
            get(&doc, "project", "id"),
            &Value::Str("com.example.todo".into())
        );
        assert_eq!(get(&doc, "project", "flag"), &Value::Bool(true));
        assert_eq!(get(&doc, "project", "n"), &Value::Int(1000));
    }

    #[test]
    fn reads_arrays_over_several_lines() {
        let doc =
            parse("platforms = [\n  \"ios\", # first\n  \"android\",\n  'web',\n]\nempty = []\n")
                .unwrap();
        assert_eq!(
            get(&doc, "", "platforms"),
            &Value::Array(vec![
                Value::Str("ios".into()),
                Value::Str("android".into()),
                Value::Str("web".into())
            ])
        );
        assert_eq!(get(&doc, "", "empty"), &Value::Array(vec![]));
    }

    #[test]
    fn a_hash_inside_a_string_is_not_a_comment() {
        let doc = parse("a = \"x # y\"\nb = 'p#q'\n").unwrap();
        assert_eq!(get(&doc, "", "a"), &Value::Str("x # y".into()));
        assert_eq!(get(&doc, "", "b"), &Value::Str("p#q".into()));
    }

    #[test]
    fn escapes_round_trip_through_quote() {
        for s in [
            "plain",
            "with \"quotes\"",
            "back\\slash",
            "line\nbreak\ttab",
        ] {
            let doc = parse(&format!("k = {}\n", quote(s))).unwrap();
            assert_eq!(get(&doc, "", "k"), &Value::Str(s.into()), "{s:?}");
        }
    }

    #[test]
    fn dotted_table_names_are_kept_whole() {
        let doc = parse("[a.b]\nk = 1\n").unwrap();
        assert_eq!(get(&doc, "a.b", "k"), &Value::Int(1));
    }

    #[test]
    fn errors_name_the_line() {
        let cases = [
            ("a = \n", 1, "needs a value"),
            ("\n\nnot a pair\n", 3, "key = value"),
            ("a = nope\n", 1, "strings need quotes"),
            ("a = { b = 1 }\n", 1, "inline tables"),
            ("[[t]]\n", 1, "arrays of tables"),
            ("[t]\n[t]\n", 2, "defined twice"),
            ("a = 1\na = 2\n", 2, "set twice"),
            ("a = [\"x\"\n", 1, "never closed"),
            ("a = \"open\n", 1, "never closed"),
            ("a = \"x\" y\n", 1, "after the closing quote"),
            ("bad key = 1\n", 1, "not a valid key"),
        ];
        for (text, line, needle) in cases {
            let e = parse(text).unwrap_err();
            assert_eq!(e.line, line, "{text:?}: {e}");
            assert!(e.message.contains(needle), "{text:?}: {e}");
        }
    }
}
