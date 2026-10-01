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
//!
//! A second kind of table gates the **sustained** ("harsh conditions") scenarios of
//! `bench/tests/stress.rs` and the soak binary. Every key is optional but at least one gate must
//! be present:
//!
//! ```toml
//! [stress."firehose/sustained"]       # the scenario name, quoted
//! min_per_sec = 1100000               # gate: operations per second must be at least this
//! p99_ns = 1100                       # gate: the 99th percentile latency must be at most this
//! p999_ns = 3000                      # gate: the 99.9th percentile latency must be at most this
//! bytes_per_op = 37                   # gate: change-set bytes per operation, at most this
//! rss_growth_pct = 1                  # gate: RSS growth after warm-up, at most this percent
//! measured_per_sec = 5699209          # what the harness measured when the gates were set
//! measured_p99_ns = 209               # (kept for humans, and checked to be inside the gates)
//! measured_p999_ns = 292
//! measured_bytes_per_op = 37
//! ```
//!
//! `UNDRA_BENCH_SCALE` divides `min_per_sec` and multiplies the two latency ceilings; it never
//! touches `bytes_per_op`, `rss_growth_pct` or a scenario's own invariants (nothing lost,
//! nothing reordered), which are not negotiable on a slower machine. `bytes_per_op` is a
//! **ceiling**: a run that ships fewer bytes passes (what each scenario asserts about its own
//! exact byte count is in its invariants, in code).
//!
//! A third kind of table gates a **ratio between two layer A rows measured in the same run**,
//! so the speed of the machine cancels:
//!
//! ```toml
//! [ratio."commit_vs_call"]            # a name for the relationship, quoted
//! num = "stress/firehose/call_set"    # required: the [bench."..."] row on top
//! den = "dispatch/call_sync/add"      # required: the [bench."..."] row below
//! num_div = 1                         # optional: operations per iteration of `num` (1)
//! den_div = 1                         # optional: operations per iteration of `den` (1)
//! max = 3.4                           # required: (num p50 / num_div) / (den p50 / den_div) at most
//! measured = 2.9                      # the ratio when the gate was set, kept for humans
//! ```
//!
//! `UNDRA_BENCH_SCALE` does not touch a ratio: a slower machine slows both rows.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use crate::rss::RssGrowth;

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

/// A ratio between two layer A rows measured in the same run (a `[ratio."name"]` table): the
/// machine's speed cancels, so the gate holds on any hardware as long as the relationship does.
#[derive(Clone, Debug, PartialEq)]
pub struct RatioBudget {
    /// The row on top: a `[bench."..."]` name.
    pub num: String,
    /// The row below: a `[bench."..."]` name.
    pub den: String,
    /// Operations per iteration of `num` (a row timed as a batch of 1,000 says 1000).
    pub num_div: f64,
    /// Operations per iteration of `den`.
    pub den_div: f64,
    /// The ratio of the per-operation p50s must be at most this.
    pub max: f64,
    /// The ratio measured when the gate was set, if recorded.
    pub measured: Option<f64>,
}

impl RatioBudget {
    /// The ratio of two p50s (nanoseconds per iteration), per operation.
    ///
    /// # Example
    ///
    /// ```
    /// use undra_bench::budget::Budgets;
    ///
    /// let b = Budgets::parse(
    ///     "[ratio.\"r\"]\nnum = \"a\"\nden = \"b\"\nnum_div = 1000\nmax = 2\n",
    /// )
    /// .unwrap();
    /// // 80 us per 1,000 operations against 40 ns per operation: a ratio of 2.
    /// assert_eq!(b.ratios["r"].value(80_000.0, 40.0), 2.0);
    /// ```
    pub fn value(&self, num_p50_ns: f64, den_p50_ns: f64) -> f64 {
        (num_p50_ns / self.num_div) / (den_p50_ns / self.den_div)
    }

    /// Whether `value` (a ratio from [`value`](RatioBudget::value)) is within the gate.
    pub fn holds(&self, value: f64) -> bool {
        value <= self.max
    }

    /// Problems with the table itself: a recorded measurement already over its own gate.
    pub fn self_check(&self) -> Vec<String> {
        match self.measured {
            Some(m) if m > self.max => vec![format!("measured {m} is over max {}", self.max)],
            _ => Vec::new(),
        }
    }
}

/// One sustained scenario's gates (a `[stress."name"]` table). A gate that is `None` is not
/// checked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StressBudget {
    /// Operations per second of wall time must be at least this (before scaling).
    pub min_per_sec: Option<f64>,
    /// The 99th percentile latency, in nanoseconds, must be at most this (before scaling).
    pub p99_ns: Option<f64>,
    /// The 99.9th percentile latency, in nanoseconds, must be at most this (before scaling).
    pub p999_ns: Option<f64>,
    /// Bytes per operation must be at most this. It is a **ceiling**: fewer bytes pass, so the
    /// gate alone cannot say a scenario shipped exactly what it should. Where the load is a
    /// fixed cycle at fixed widths the ceiling is the measured value, and the scenario asserts
    /// the exact count in code (an invariant), which is what makes it exact.
    pub bytes_per_op: Option<f64>,
    /// RSS growth from after the warm-up to the end, in percent (see [`RssGrowth::within`]).
    pub rss_growth_pct: Option<f64>,
    /// Throughput measured when the gates were set.
    pub measured_per_sec: Option<f64>,
    /// p99 measured when the gates were set, nanoseconds.
    pub measured_p99_ns: Option<f64>,
    /// p999 measured when the gates were set, nanoseconds.
    pub measured_p999_ns: Option<f64>,
    /// Bytes per operation measured when the gates were set.
    pub measured_bytes_per_op: Option<f64>,
}

/// What a sustained run observed, in the terms a [`StressBudget`] gates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StressObserved {
    /// Operations per second over the run's wall time.
    pub per_sec: f64,
    /// 99th percentile latency in nanoseconds, when the scenario times operations.
    pub p99_ns: Option<f64>,
    /// 99.9th percentile latency in nanoseconds, when the scenario times operations.
    pub p999_ns: Option<f64>,
    /// Change-set bytes per operation, when the scenario counts them.
    pub bytes_per_op: Option<f64>,
    /// RSS growth, when it could be measured on this platform.
    pub rss: Option<RssGrowth>,
}

/// The outcome of checking a run against a [`StressBudget`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StressVerdict {
    /// Gates that were not met, one sentence each.
    pub failures: Vec<String>,
    /// Gates that could not be checked (RSS on a platform that cannot measure it): printed, and
    /// never a silent pass.
    pub notices: Vec<String>,
}

impl StressVerdict {
    /// Whether every gate that could be checked was met.
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

impl StressBudget {
    /// Checks `observed` against the gates. `scale` (`UNDRA_BENCH_SCALE`) divides the throughput
    /// floor and multiplies the latency ceilings; bytes and RSS are never scaled.
    pub fn check(&self, observed: &StressObserved, scale: f64) -> StressVerdict {
        let mut verdict = StressVerdict::default();
        let fail = |v: &mut StressVerdict, text: String| v.failures.push(text);
        if let Some(floor) = self.min_per_sec {
            let floor = floor / scale;
            if observed.per_sec < floor {
                fail(
                    &mut verdict,
                    format!(
                        "throughput {:.0}/s is under the floor of {floor:.0}/s",
                        observed.per_sec
                    ),
                );
            }
        }
        for (name, limit, got) in [
            ("p99", self.p99_ns, observed.p99_ns),
            ("p999", self.p999_ns, observed.p999_ns),
        ] {
            let Some(limit) = limit else { continue };
            let limit = limit * scale;
            match got {
                Some(got) if got <= limit => {}
                Some(got) => fail(
                    &mut verdict,
                    format!("{name} {got:.0} ns is over the ceiling of {limit:.0} ns"),
                ),
                None => fail(
                    &mut verdict,
                    format!("{name} is gated but was not measured"),
                ),
            }
        }
        if let Some(limit) = self.bytes_per_op {
            match observed.bytes_per_op {
                Some(got) if got <= limit * (1.0 + 1e-9) => {}
                Some(got) => fail(
                    &mut verdict,
                    format!("{got:.2} bytes per operation is over the ceiling of {limit}"),
                ),
                None => fail(
                    &mut verdict,
                    "bytes per operation is gated but was not counted".into(),
                ),
            }
        }
        if let Some(pct) = self.rss_growth_pct {
            match observed.rss {
                Some(growth) if growth.within(pct) => {}
                Some(growth) => fail(
                    &mut verdict,
                    format!(
                        "RSS grew {:.2}% ({} to {} bytes) after warm-up; the gate is {pct}% or 64 KiB",
                        growth.growth_pct, growth.baseline_bytes, growth.final_bytes
                    ),
                ),
                None => verdict.notices.push(
                    "RSS could not be measured here (no samples or no source on this platform): \
                     the RSS gate was skipped"
                        .into(),
                ),
            }
        }
        verdict
    }

    /// Problems with the table itself: a recorded measurement that is already outside its own
    /// gate, which would mean the gate was set below what the harness measured.
    pub fn self_check(&self) -> Vec<String> {
        // (measured key, gate key, measured, gate, the gate is a floor)
        let pairs = [
            (
                "measured_per_sec",
                "min_per_sec",
                self.measured_per_sec,
                self.min_per_sec,
                true,
            ),
            (
                "measured_p99_ns",
                "p99_ns",
                self.measured_p99_ns,
                self.p99_ns,
                false,
            ),
            (
                "measured_p999_ns",
                "p999_ns",
                self.measured_p999_ns,
                self.p999_ns,
                false,
            ),
            (
                "measured_bytes_per_op",
                "bytes_per_op",
                self.measured_bytes_per_op,
                self.bytes_per_op,
                false,
            ),
        ];
        let mut problems = Vec::new();
        for (measured_key, gate_key, measured, gate, is_floor) in pairs {
            if let (Some(m), Some(g)) = (measured, gate) {
                if is_floor && m < g {
                    problems.push(format!("{measured_key} {m} is under {gate_key} {g}"));
                } else if !is_floor && m > g {
                    problems.push(format!("{measured_key} {m} is over {gate_key} {g}"));
                }
            }
        }
        problems
    }

    /// Whether the table sets at least one gate.
    fn has_gate(&self) -> bool {
        self.min_per_sec.is_some()
            || self.p99_ns.is_some()
            || self.p999_ns.is_some()
            || self.bytes_per_op.is_some()
            || self.rss_growth_pct.is_some()
    }
}

/// The parsed `budgets.toml`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Budgets {
    /// The `[meta]` table: the machine and the reasoning, for humans.
    pub meta: BTreeMap<String, String>,
    /// The per-operation budgets, by workload name.
    pub benches: BTreeMap<String, Budget>,
    /// The sustained-scenario budgets, by scenario name (`[stress."name"]`).
    pub stress: BTreeMap<String, StressBudget>,
    /// The ratio gates between two layer A rows, by name (`[ratio."name"]`).
    pub ratios: BTreeMap<String, RatioBudget>,
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
    /// A `[stress."name"]` table sets none of `min_per_sec`, `p99_ns`, `p999_ns`,
    /// `bytes_per_op` and `rss_growth_pct`: it would gate nothing.
    MissingGate {
        /// The scenario the table names.
        name: String,
    },
    /// Two `[stress."name"]` tables name the same scenario.
    DuplicateStress {
        /// The scenario named twice.
        name: String,
    },
    /// A `[ratio."name"]` table lacks `num`, `den` or `max`.
    IncompleteRatio {
        /// The ratio the table names.
        name: String,
        /// The key that is missing.
        missing: &'static str,
    },
    /// Two `[ratio."name"]` tables have the same name.
    DuplicateRatio {
        /// The ratio named twice.
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
            BudgetError::MissingGate { name } => write!(
                f,
                "budgets file: [stress.\"{name}\"] sets no gate (min_per_sec, p99_ns, p999_ns, \
                 bytes_per_op or rss_growth_pct)"
            ),
            BudgetError::DuplicateStress { name } => {
                write!(f, "budgets file: [stress.\"{name}\"] appears twice")
            }
            BudgetError::IncompleteRatio { name, missing } => {
                write!(f, "budgets file: [ratio.\"{name}\"] has no {missing}")
            }
            BudgetError::DuplicateRatio { name } => {
                write!(f, "budgets file: [ratio.\"{name}\"] appears twice")
            }
        }
    }
}

impl std::error::Error for BudgetError {}

/// A scalar value on the right of `key = value`.
pub(crate) enum Value {
    Number(f64),
    Text(String),
}

enum Section {
    None,
    Meta,
    Bench(String),
    Stress(String),
    Ratio(String),
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
        let mut stress_order: Vec<String> = Vec::new();
        let mut ratios: BTreeMap<String, PartialRatio> = BTreeMap::new();
        let mut ratio_order: Vec<String> = Vec::new();
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
                } else if let Some(name) = header.strip_prefix("stress.") {
                    let name = parse_string(name.trim())
                        .ok_or_else(|| syntax("a stress table is written [stress.\"name\"]"))?;
                    if budgets.stress.contains_key(&name) {
                        return Err(BudgetError::DuplicateStress { name });
                    }
                    budgets.stress.insert(name.clone(), StressBudget::default());
                    stress_order.push(name.clone());
                    Section::Stress(name)
                } else if let Some(name) = header.strip_prefix("ratio.") {
                    let name = parse_string(name.trim())
                        .ok_or_else(|| syntax("a ratio table is written [ratio.\"name\"]"))?;
                    if ratios.contains_key(&name) {
                        return Err(BudgetError::DuplicateRatio { name });
                    }
                    ratios.insert(name.clone(), PartialRatio::default());
                    ratio_order.push(name.clone());
                    Section::Ratio(name)
                } else {
                    return Err(syntax(
                        "the only tables are [meta], [bench.\"name\"], [stress.\"name\"] and \
                         [ratio.\"name\"]",
                    ));
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
                Section::Stress(name) => {
                    let entry = budgets.stress.entry(name.clone()).or_default();
                    let Value::Number(n) = value else {
                        return Err(syntax("a stress table takes numbers only"));
                    };
                    match key {
                        "min_per_sec" => entry.min_per_sec = Some(n),
                        "p99_ns" => entry.p99_ns = Some(n),
                        "p999_ns" => entry.p999_ns = Some(n),
                        "bytes_per_op" => entry.bytes_per_op = Some(n),
                        "rss_growth_pct" => entry.rss_growth_pct = Some(n),
                        "measured_per_sec" => entry.measured_per_sec = Some(n),
                        "measured_p99_ns" => entry.measured_p99_ns = Some(n),
                        "measured_p999_ns" => entry.measured_p999_ns = Some(n),
                        "measured_bytes_per_op" => entry.measured_bytes_per_op = Some(n),
                        _ => {
                            return Err(syntax(
                                "a stress table takes min_per_sec, p99_ns, p999_ns, bytes_per_op, \
                                 rss_growth_pct, measured_per_sec, measured_p99_ns, \
                                 measured_p999_ns and measured_bytes_per_op",
                            ));
                        }
                    }
                }
                Section::Ratio(name) => {
                    let entry = ratios.entry(name.clone()).or_default();
                    match (key, value) {
                        ("num", Value::Text(t)) => entry.num = Some(t),
                        ("den", Value::Text(t)) => entry.den = Some(t),
                        ("num_div", Value::Number(n)) if n > 0.0 => entry.num_div = Some(n),
                        ("den_div", Value::Number(n)) if n > 0.0 => entry.den_div = Some(n),
                        ("max", Value::Number(n)) if n > 0.0 => entry.max = Some(n),
                        ("measured", Value::Number(n)) => entry.measured = Some(n),
                        _ => {
                            return Err(syntax(
                                "a ratio table takes num and den (strings), num_div, den_div and \
                                 max (positive numbers) and measured (a number)",
                            ));
                        }
                    }
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
        for name in stress_order {
            if !budgets
                .stress
                .get(&name)
                .is_some_and(StressBudget::has_gate)
            {
                return Err(BudgetError::MissingGate { name });
            }
        }
        for name in ratio_order {
            let partial = ratios.remove(&name).unwrap_or_default();
            let missing = |key: &'static str| BudgetError::IncompleteRatio {
                name: name.clone(),
                missing: key,
            };
            let ratio = RatioBudget {
                num: partial.num.clone().ok_or_else(|| missing("num"))?,
                den: partial.den.clone().ok_or_else(|| missing("den"))?,
                num_div: partial.num_div.unwrap_or(1.0),
                den_div: partial.den_div.unwrap_or(1.0),
                max: partial.max.ok_or_else(|| missing("max"))?,
                measured: partial.measured,
            };
            budgets.ratios.insert(name, ratio);
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
struct PartialRatio {
    num: Option<String>,
    den: Option<String>,
    num_div: Option<f64>,
    den_div: Option<f64>,
    max: Option<f64>,
    measured: Option<f64>,
}

#[derive(Default)]
struct PartialBudget {
    budget_ns: Option<f64>,
    measured_ns: Option<f64>,
    blueprint_ns: Option<f64>,
    blueprint: Option<String>,
}

/// Removes a `#` comment, unless the `#` is inside a double-quoted string.
pub(crate) fn strip_comment(line: &str) -> &str {
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
pub(crate) fn parse_string(text: &str) -> Option<String> {
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

pub(crate) fn parse_value(text: &str) -> Option<Value> {
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
            let budgets = Budgets::load(&path).expect("bench/budgets.toml parses");
            for (name, table) in &budgets.stress {
                assert!(table.has_gate(), "{name} sets no gate");
                assert!(
                    table.self_check().is_empty(),
                    "{name}: {:?}",
                    table.self_check()
                );
            }
            for (name, ratio) in &budgets.ratios {
                assert!(
                    ratio.self_check().is_empty(),
                    "{name}: {:?}",
                    ratio.self_check()
                );
                assert!(
                    budgets.benches.contains_key(&ratio.num)
                        && budgets.benches.contains_key(&ratio.den),
                    "{name}: names a row that has no [bench] table"
                );
            }
        }
    }

    const RATIOS: &str = r#"
[bench."a"]
budget_ns = 10
[bench."b"]
budget_ns = 10

[ratio."a_vs_b"]
num = "a"
den = "b"
num_div = 1000
max = 2.5
measured = 1.8
"#;

    #[test]
    fn parses_ratio_tables_and_computes_per_operation_ratios() {
        let b = Budgets::parse(RATIOS).expect("parses");
        let r = &b.ratios["a_vs_b"];
        assert_eq!((r.num.as_str(), r.den.as_str()), ("a", "b"));
        assert_eq!((r.num_div, r.den_div, r.max), (1000.0, 1.0, 2.5));
        assert_eq!(r.measured, Some(1.8));
        // 100 us per 1,000 operations (100 ns each) over a 50 ns row is 2.
        assert_eq!(r.value(100_000.0, 50.0), 2.0);
        assert!(r.holds(2.5), "the gate is inclusive");
        assert!(!r.holds(2.51));
        assert!(r.self_check().is_empty());
    }

    #[test]
    fn a_ratio_needs_both_rows_and_a_maximum_and_rejects_typos() {
        for (text, missing) in [
            ("[ratio.\"r\"]\nden = \"b\"\nmax = 2\n", "num"),
            ("[ratio.\"r\"]\nnum = \"a\"\nmax = 2\n", "den"),
            ("[ratio.\"r\"]\nnum = \"a\"\nden = \"b\"\n", "max"),
        ] {
            let err = Budgets::parse(text).unwrap_err();
            assert_eq!(
                err,
                BudgetError::IncompleteRatio {
                    name: "r".into(),
                    missing
                },
                "{text}"
            );
            assert!(err.to_string().contains(missing), "{err}");
        }
        let err = Budgets::parse(
            "[ratio.\"r\"]\nnum = \"a\"\nden = \"b\"\nmax = 2\n[ratio.\"r\"]\nmax = 3\n",
        )
        .unwrap_err();
        assert_eq!(err, BudgetError::DuplicateRatio { name: "r".into() });
        assert!(err.to_string().contains("appears twice"));
        // A zero divisor or maximum, a number where a string goes, an unknown key: line numbers.
        for text in [
            "[ratio.\"r\"]\nmax = 0\n",
            "[ratio.\"r\"]\nnum_div = 0\n",
            "[ratio.\"r\"]\nnum = 3\n",
            "[ratio.\"r\"]\nbudget_ns = 3\n",
            "[ratio.r]\n",
        ] {
            let err = Budgets::parse(text).unwrap_err();
            assert!(matches!(err, BudgetError::Syntax { .. }), "{text}: {err}");
        }
    }

    #[test]
    fn a_ratio_measured_over_its_own_maximum_is_flagged() {
        let b =
            Budgets::parse("[ratio.\"r\"]\nnum = \"a\"\nden = \"b\"\nmax = 2\nmeasured = 2.5\n")
                .unwrap();
        assert_eq!(b.ratios["r"].self_check().len(), 1);
    }

    const STRESS: &str = r#"
[bench."a/b"]
budget_ns = 10

[stress."firehose/sustained"]
min_per_sec = 1_100_000
p99_ns = 1100
p999_ns = 3000
bytes_per_op = 37          # exact
rss_growth_pct = 1
measured_per_sec = 5699209
measured_p99_ns = 209
measured_p999_ns = 292
measured_bytes_per_op = 37

[stress."soak/mixed"]
rss_growth_pct = 1
"#;

    #[test]
    fn parses_stress_tables_next_to_bench_tables() {
        let b = Budgets::parse(STRESS).expect("parses");
        assert_eq!(b.benches.len(), 1);
        assert_eq!(b.stress.len(), 2);
        let f = &b.stress["firehose/sustained"];
        assert_eq!(f.min_per_sec, Some(1_100_000.0));
        assert_eq!(f.p99_ns, Some(1100.0));
        assert_eq!(f.p999_ns, Some(3000.0));
        assert_eq!(f.bytes_per_op, Some(37.0));
        assert_eq!(f.rss_growth_pct, Some(1.0));
        assert_eq!(f.measured_per_sec, Some(5_699_209.0));
        assert_eq!(f.measured_p99_ns, Some(209.0));
        assert_eq!(f.measured_p999_ns, Some(292.0));
        assert_eq!(f.measured_bytes_per_op, Some(37.0));
        assert!(f.self_check().is_empty());
        let soak = &b.stress["soak/mixed"];
        assert_eq!(soak.rss_growth_pct, Some(1.0));
        assert_eq!(soak.min_per_sec, None);
    }

    #[test]
    fn stress_tables_reject_what_they_do_not_understand_with_a_line_number() {
        let err = Budgets::parse("[stress.\"a\"]\nbudget_ns = 4\n").unwrap_err();
        assert!(matches!(err, BudgetError::Syntax { line: 2, .. }), "{err}");
        let err = Budgets::parse("[stress.\"a\"]\np99_ns = \"fast\"\n").unwrap_err();
        assert!(matches!(err, BudgetError::Syntax { line: 2, .. }), "{err}");
        let err = Budgets::parse("[stress.a]\n").unwrap_err();
        assert!(matches!(err, BudgetError::Syntax { line: 1, .. }), "{err}");
        let err = Budgets::parse("[stress.\"a\"]\np99_ns = -1\n").unwrap_err();
        assert!(matches!(err, BudgetError::Syntax { line: 2, .. }), "{err}");
        // A bench key in a stress table is a typo, not a second meaning.
        let err = Budgets::parse("[stress.\"a\"]\nmeasured_ns = 4\n").unwrap_err();
        assert!(matches!(err, BudgetError::Syntax { line: 2, .. }), "{err}");
    }

    #[test]
    fn a_stress_table_without_a_gate_or_repeated_is_an_error() {
        // `measured_*` alone gates nothing.
        let err = Budgets::parse("[stress.\"a\"]\nmeasured_per_sec = 4\n").unwrap_err();
        assert_eq!(err, BudgetError::MissingGate { name: "a".into() });
        let err = Budgets::parse("[stress.\"a\"]\n").unwrap_err();
        assert_eq!(err, BudgetError::MissingGate { name: "a".into() });
        let err =
            Budgets::parse("[stress.\"a\"]\nmin_per_sec = 1\n[stress.\"a\"]\nmin_per_sec = 2\n")
                .unwrap_err();
        assert_eq!(err, BudgetError::DuplicateStress { name: "a".into() });
        // The same name in the two kinds of table is two different things.
        Budgets::parse("[bench.\"a\"]\nbudget_ns = 1\n[stress.\"a\"]\nmin_per_sec = 2\n")
            .expect("a bench and a stress table may share a name");
        assert!(err.to_string().contains("appears twice"));
        assert!(
            BudgetError::MissingGate { name: "x".into() }
                .to_string()
                .contains("[stress.\"x\"] sets no gate")
        );
    }

    fn rss(baseline: u64, last: u64) -> Option<RssGrowth> {
        Some(RssGrowth {
            baseline_bytes: baseline,
            final_bytes: last,
            peak_bytes: last.max(baseline),
            growth_pct: (last as f64 - baseline as f64) / baseline as f64 * 100.0,
        })
    }

    fn good() -> StressObserved {
        StressObserved {
            per_sec: 5_700_000.0,
            p99_ns: Some(209.0),
            p999_ns: Some(292.0),
            bytes_per_op: Some(37.0),
            rss: rss(10_000_000, 10_000_000),
        }
    }

    #[test]
    fn a_run_inside_every_gate_passes() {
        let b = Budgets::parse(STRESS).unwrap();
        let v = b.stress["firehose/sustained"].check(&good(), 1.0);
        assert!(v.passed(), "{v:?}");
        assert!(v.notices.is_empty());
    }

    #[test]
    fn each_gate_fails_on_its_own() {
        let b = Budgets::parse(STRESS).unwrap();
        let t = &b.stress["firehose/sustained"];
        let cases: [(StressObserved, &str); 5] = [
            (
                StressObserved {
                    per_sec: 1_000_000.0,
                    ..good()
                },
                "throughput",
            ),
            (
                StressObserved {
                    p99_ns: Some(1_101.0),
                    ..good()
                },
                "p99",
            ),
            (
                StressObserved {
                    p999_ns: Some(3_001.0),
                    ..good()
                },
                "p999",
            ),
            (
                StressObserved {
                    bytes_per_op: Some(37.5),
                    ..good()
                },
                "bytes per operation",
            ),
            (
                StressObserved {
                    rss: rss(10_000_000, 10_500_000),
                    ..good()
                },
                "RSS",
            ),
        ];
        for (observed, what) in cases {
            let v = t.check(&observed, 1.0);
            assert_eq!(v.failures.len(), 1, "{what}: {v:?}");
            assert!(v.failures[0].contains(what), "{what}: {v:?}");
        }
        // Gated but not measured is a failure, not a pass.
        let blind = StressObserved {
            p99_ns: None,
            p999_ns: None,
            bytes_per_op: None,
            ..good()
        };
        assert_eq!(t.check(&blind, 1.0).failures.len(), 3);
    }

    #[test]
    fn scale_loosens_rates_and_latencies_but_never_bytes_or_memory() {
        let b = Budgets::parse(STRESS).unwrap();
        let t = &b.stress["firehose/sustained"];
        // Half the throughput and double the latency pass at a scale of 2...
        let slow = StressObserved {
            per_sec: 600_000.0,
            p99_ns: Some(2_150.0),
            p999_ns: Some(5_900.0),
            ..good()
        };
        assert!(!t.check(&slow, 1.0).passed());
        assert!(t.check(&slow, 2.0).passed(), "{:?}", t.check(&slow, 2.0));
        // ...but more bytes and more memory fail at any scale.
        let fat = StressObserved {
            bytes_per_op: Some(38.0),
            ..good()
        };
        assert!(!t.check(&fat, 100.0).passed());
        let leaky = StressObserved {
            rss: rss(10_000_000, 12_000_000),
            ..good()
        };
        assert!(!t.check(&leaky, 100.0).passed());
    }

    #[test]
    fn an_unmeasurable_rss_is_a_notice_never_a_silent_pass() {
        let b = Budgets::parse(STRESS).unwrap();
        let v = b.stress["soak/mixed"].check(&StressObserved::default(), 1.0);
        assert!(v.passed());
        assert_eq!(v.notices.len(), 1);
        assert!(v.notices[0].contains("RSS"), "{v:?}");
    }

    #[test]
    fn self_check_flags_a_measurement_outside_its_own_gate() {
        let t = StressBudget {
            min_per_sec: Some(100.0),
            p99_ns: Some(50.0),
            p999_ns: Some(60.0),
            bytes_per_op: Some(10.0),
            measured_per_sec: Some(99.0),
            measured_p99_ns: Some(51.0),
            measured_p999_ns: Some(61.0),
            measured_bytes_per_op: Some(11.0),
            ..StressBudget::default()
        };
        assert_eq!(t.self_check().len(), 4, "{:?}", t.self_check());
    }
}
