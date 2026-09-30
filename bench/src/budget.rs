//! `budgets.toml`: the per-operation host budgets the budgets test enforces.
//!
//! The file is a small, strict subset of TOML (no dependency is needed to read it):
//!
//! ```toml
//! [meta]
//! machine = "Apple M-series, macOS"   # free-form strings and numbers, kept for humans
//!
//! [bench."wire/u32/roundtrip"]        # the workload name, quoted
//! budget_ns = 400                     # required: p50 above this fails the test
//! measured_ns = 80                    # criterion median when the budget was set
//! blueprint_ns = 60                   # the blueprint's section 14 target, when the row exists
//! blueprint = "<= 60 ns on iOS (A15)" # the target in words
//! ```
//!
//! Numbers are nanoseconds and may use `_` separators. Anything else is a syntax error, with the
//! line number, so a typo cannot silently disable a budget.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

/// One operation's budget.
#[derive(Clone, Debug, PartialEq)]
pub struct Budget {
    /// The p50, in nanoseconds per iteration, above which the budgets test fails.
    pub budget_ns: f64,
    /// What criterion measured on the reference machine when the budget was set, if recorded.
    pub measured_ns: Option<f64>,
    /// The blueprint section 14 target for this operation on its reference device, if it has one.
    pub blueprint_ns: Option<f64>,
    /// The same target in words (device and unit), for reports.
    pub blueprint: Option<String>,
}

/// The parsed `budgets.toml`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Budgets {
    /// The `[meta]` table: the machine and the reasoning, for humans.
    pub meta: BTreeMap<String, String>,
    /// The per-operation budgets, by workload name.
    pub benches: BTreeMap<String, Budget>,
}

/// Why `budgets.toml` could not be read.
#[derive(Clone, Debug, PartialEq)]
pub enum BudgetError {
    /// The file could not be read.
    Io(String),
    /// A line is not part of the accepted subset.
    Syntax {
        /// One-based line number.
        line: usize,
        /// What is wrong.
        message: String,
    },
    /// A `[bench."name"]` table has no `budget_ns`.
    MissingBudget {
        /// The workload the table names.
        name: String,
    },
    /// Two tables name the same workload.
    Duplicate {
        /// The workload named twice.
        name: String,
    },
}

impl fmt::Display for BudgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BudgetError::Io(e) => write!(f, "cannot read the budgets file: {e}"),
            BudgetError::Syntax { line, message } => {
                write!(f, "budgets file, line {line}: {message}")
            }
            BudgetError::MissingBudget { name } => {
                write!(f, "budgets file: [bench.\"{name}\"] has no budget_ns")
            }
            BudgetError::Duplicate { name } => {
                write!(f, "budgets file: [bench.\"{name}\"] appears twice")
            }
        }
    }
}

impl std::error::Error for BudgetError {}

/// A scalar value on the right of `key = value`.
enum Value {
    Number(f64),
    Text(String),
}

enum Section {
    None,
    Meta,
    Bench(String),
}

impl Budgets {
    /// Reads and parses the file at `path`.
    pub fn load(path: &Path) -> Result<Budgets, BudgetError> {
        let text = std::fs::read_to_string(path).map_err(|e| BudgetError::Io(e.to_string()))?;
        Budgets::parse(&text)
    }

    /// Parses the text of a budgets file.
    pub fn parse(text: &str) -> Result<Budgets, BudgetError> {
        let mut budgets = Budgets::default();
        let mut section = Section::None;
        let mut pending: BTreeMap<String, PartialBudget> = BTreeMap::new();
        let mut order: Vec<String> = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let line = index + 1;
            let syntax = |message: &str| BudgetError::Syntax {
                line,
                message: message.to_owned(),
            };
            let trimmed = strip_comment(raw).trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(header) = trimmed.strip_prefix('[') {
                let header = header
                    .strip_suffix(']')
                    .ok_or_else(|| syntax("a table header must end with `]`"))?;
                section = if header == "meta" {
                    Section::Meta
                } else if let Some(name) = header.strip_prefix("bench.") {
                    let name = parse_string(name.trim())
                        .ok_or_else(|| syntax("a bench table is written [bench.\"name\"]"))?;
                    if pending.contains_key(&name) {
                        return Err(BudgetError::Duplicate { name });
                    }
                    pending.insert(name.clone(), PartialBudget::default());
                    order.push(name.clone());
                    Section::Bench(name)
                } else {
                    return Err(syntax("the only tables are [meta] and [bench.\"name\"]"));
                };
                continue;
            }
            let (key, value) = trimmed
                .split_once('=')
                .ok_or_else(|| syntax("expected `key = value`"))?;
            let key = key.trim();
            let value = parse_value(value.trim())
                .ok_or_else(|| syntax("a value is a number or a \"double quoted\" string"))?;
            match &section {
                Section::None => return Err(syntax("a key must be inside a table")),
                Section::Meta => {
                    let text = match value {
                        Value::Text(t) => t,
                        Value::Number(n) => n.to_string(),
                    };
                    budgets.meta.insert(key.to_owned(), text);
                }
                Section::Bench(name) => {
                    let entry = pending.entry(name.clone()).or_default();
                    match (key, value) {
                        ("budget_ns", Value::Number(n)) => entry.budget_ns = Some(n),
                        ("measured_ns", Value::Number(n)) => entry.measured_ns = Some(n),
                        ("blueprint_ns", Value::Number(n)) => entry.blueprint_ns = Some(n),
                        ("blueprint", Value::Text(t)) => entry.blueprint = Some(t),
                        _ => {
                            return Err(syntax(
                                "a bench table takes budget_ns, measured_ns, blueprint_ns \
                                 (numbers) and blueprint (a string)",
                            ));
                        }
                    }
                }
            }
        }
        for name in order {
            let partial = pending.remove(&name).unwrap_or_default();
            let Some(budget_ns) = partial.budget_ns else {
                return Err(BudgetError::MissingBudget { name });
            };
            budgets.benches.insert(
                name,
                Budget {
                    budget_ns,
                    measured_ns: partial.measured_ns,
                    blueprint_ns: partial.blueprint_ns,
                    blueprint: partial.blueprint,
                },
            );
        }
        Ok(budgets)
    }
}

#[derive(Default)]
struct PartialBudget {
    budget_ns: Option<f64>,
    measured_ns: Option<f64>,
    blueprint_ns: Option<f64>,
    blueprint: Option<String>,
}

/// Removes a `#` comment, unless the `#` is inside a double-quoted string.
fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        match c {
            '\\' if in_string => escaped = !escaped,
            '"' if !escaped => in_string = !in_string,
            '#' if !in_string => return &line[..i],
            _ => escaped = false,
        }
    }
    line
}

/// `"text"` with `\"` and `\\` escapes, nothing after the closing quote.
fn parse_string(text: &str) -> Option<String> {
    let inner = text.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(match chars.next()? {
                '"' => '"',
                '\\' => '\\',
                _ => return None,
            }),
            '"' => return None,
            c => out.push(c),
        }
    }
    Some(out)
}

fn parse_value(text: &str) -> Option<Value> {
    if text.starts_with('"') {
        return parse_string(text).map(Value::Text);
    }
    let digits: String = text.chars().filter(|c| *c != '_').collect();
    let number: f64 = digits.parse().ok()?;
    (number.is_finite() && number >= 0.0).then_some(Value::Number(number))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# a comment
[meta]
machine = "Apple M-series # not a comment"
factor = 5

[bench."wire/u32/roundtrip"]
budget_ns = 1_000
measured_ns = 80.5    # trailing comment
blueprint_ns = 60
blueprint = "<= 60 ns on iOS (A15)"

[bench."x/y"]
budget_ns = 7
"#;

    #[test]
    fn parses_meta_and_benches() {
        let b = Budgets::parse(SAMPLE).expect("parses");
        assert_eq!(b.meta["machine"], "Apple M-series # not a comment");
        assert_eq!(b.meta["factor"], "5");
        let w = &b.benches["wire/u32/roundtrip"];
        assert_eq!(w.budget_ns, 1000.0);
        assert_eq!(w.measured_ns, Some(80.5));
        assert_eq!(w.blueprint_ns, Some(60.0));
        assert_eq!(w.blueprint.as_deref(), Some("<= 60 ns on iOS (A15)"));
        let x = &b.benches["x/y"];
        assert_eq!(
            (x.budget_ns, x.measured_ns, x.blueprint_ns),
            (7.0, None, None)
        );
    }

    #[test]
    fn rejects_what_it_does_not_understand_with_a_line_number() {
        let err = Budgets::parse("[meta]\nmachine = unquoted").unwrap_err();
        assert!(matches!(err, BudgetError::Syntax { line: 2, .. }), "{err}");
        let err = Budgets::parse("[other]\n").unwrap_err();
        assert!(matches!(err, BudgetError::Syntax { line: 1, .. }), "{err}");
        let err = Budgets::parse("budget_ns = 1\n").unwrap_err();
        assert!(matches!(err, BudgetError::Syntax { line: 1, .. }), "{err}");
        let err = Budgets::parse("[bench.\"a\"]\nbudget_ns = \"fast\"\n").unwrap_err();
        assert!(matches!(err, BudgetError::Syntax { line: 2, .. }), "{err}");
        let err = Budgets::parse("[bench.\"a\"]\nbudget = 4\n").unwrap_err();
        assert!(matches!(err, BudgetError::Syntax { line: 2, .. }), "{err}");
        let err = Budgets::parse("[bench.a]\n").unwrap_err();
        assert!(matches!(err, BudgetError::Syntax { line: 1, .. }), "{err}");
        let err = Budgets::parse("[bench.\"a\"]\nbudget_ns = -3\n").unwrap_err();
        assert!(matches!(err, BudgetError::Syntax { line: 2, .. }), "{err}");
    }

    #[test]
    fn a_table_without_a_budget_or_a_repeated_table_is_an_error() {
        let err = Budgets::parse("[bench.\"a\"]\nmeasured_ns = 4\n").unwrap_err();
        assert_eq!(err, BudgetError::MissingBudget { name: "a".into() });
        let err = Budgets::parse("[bench.\"a\"]\nbudget_ns = 1\n[bench.\"a\"]\nbudget_ns = 2\n")
            .unwrap_err();
        assert_eq!(err, BudgetError::Duplicate { name: "a".into() });
    }

    #[test]
    fn display_names_the_problem() {
        let err = Budgets::parse("x").unwrap_err();
        assert!(err.to_string().contains("line 1"), "{err}");
        assert!(BudgetError::Io("nope".into()).to_string().contains("nope"));
    }

    #[test]
    fn the_shipped_budgets_file_parses() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("budgets.toml");
        if path.exists() {
            Budgets::load(&path).expect("bench/budgets.toml parses");
        }
    }
}
