//! The files behind the published numbers.
//!
//! Every figure in `bench/RESULTS.md` or on the landing page traces to a JSON file in
//! `bench/results/`, written by the harness itself when `UNDRA_BENCH_RESULTS_DIR` names a
//! directory: one file per sustained scenario (`<date>-<scenario>.json`), one for the layer A rows
//! and the ratio gates (`<date>-layer-a.json`) and one for a soak (`<date>-soak-<N>s.json`). Each
//! carries the numbers, the command that produced them, the machine (CPU, cores, OS, compiler,
//! commit) and how busy it was (the one-minute load average before and after), so a number is
//! never separated from the conditions it was measured in. `UNDRA_BENCH_RESULTS_TAG=name` puts
//! the tag in the file names and in the file, for a run that is evidence and not a published
//! number (the commit-path experiment of RESULTS.md, say). `UNDRA_BENCH_DATE` overrides the date.
//!
//! The files are written by hand (no serde): the crate has no runtime dependency to spare, and
//! the shapes are small.

use std::path::{Path, PathBuf};

use crate::hostinfo;

/// The directory `UNDRA_BENCH_RESULTS_DIR` names, if it is set and not empty.
pub fn dir_from_env() -> Option<PathBuf> {
    std::env::var_os("UNDRA_BENCH_RESULTS_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
}

/// The `UNDRA_BENCH_RESULTS_TAG`, if set and not empty.
pub fn tag_from_env() -> Option<String> {
    std::env::var("UNDRA_BENCH_RESULTS_TAG")
        .ok()
        .filter(|t| !t.is_empty())
}

/// A scenario name as a file name part: `firehose/sustained` is `firehose-sustained`.
///
/// # Example
///
/// ```
/// assert_eq!(undra_bench::results::slug("completions/8_threads"), "completions-8_threads");
/// ```
pub fn slug(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// `<dir>/<date>[-<tag>]-<slug>.json`.
pub fn path_for(dir: &Path, date: &str, tag: Option<&str>, slug: &str) -> PathBuf {
    match tag {
        Some(tag) => dir.join(format!("{date}-{tag}-{slug}.json")),
        None => dir.join(format!("{date}-{slug}.json")),
    }
}

/// `text` as a JSON string literal.
///
/// # Example
///
/// ```
/// use undra_bench::results::json_string;
///
/// assert_eq!(json_string("a \"b\"\n"), "\"a \\\"b\\\"\\n\"");
/// ```
pub fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A float as JSON: the number, or `null` where it is not finite.
pub fn json_number(value: f64) -> String {
    if value.is_finite() {
        format!("{value}")
    } else {
        "null".to_owned()
    }
}

/// An optional float as JSON.
pub fn json_opt(value: Option<f64>) -> String {
    value.map_or_else(|| "null".to_owned(), json_number)
}

/// The command that produced a run: the environment variables that shaped it (the ones of `vars`
/// that are set) in front of `base`.
///
/// # Example
///
/// ```
/// let command = undra_bench::results::command("cargo test", &["UNDRA_BENCH_NOT_SET_ANYWHERE"]);
/// assert_eq!(command, "cargo test");
/// ```
pub fn command(base: &str, vars: &[&str]) -> String {
    let mut parts: Vec<String> = vars
        .iter()
        .filter_map(|v| {
            let value = std::env::var(v).ok().filter(|x| !x.is_empty())?;
            Some(format!("{v}={value}"))
        })
        .collect();
    parts.push(base.to_owned());
    parts.join(" ")
}

/// `"machine": {..}` and `"load_average": {..}` as JSON members, to splice into an object.
pub fn machine_members(load_before: Option<f64>, load_after: Option<f64>) -> String {
    format!(
        "\"machine\": {{\"cpu\": {}, \"cores\": {}, \"os\": {}, \"rustc\": {}, \"git\": {}}},\n  \
         \"load_average\": {{\"before\": {}, \"after\": {}}}",
        json_string(&hostinfo::cpu()),
        hostinfo::cores(),
        json_string(&hostinfo::os()),
        json_string(&hostinfo::rustc()),
        json_string(&hostinfo::git_revision()),
        json_opt(load_before),
        json_opt(load_after),
    )
}

/// Writes `text` to `path`, creating the directory; panics with the path on failure (a results
/// file that silently does not exist would break the one promise it makes).
pub fn write(path: &Path, text: &str) {
    let created = path.parent().map(std::fs::create_dir_all).transpose();
    if let Err(e) = created {
        panic!("cannot create the directory of {}: {e}", path.display());
    }
    if let Err(e) = std::fs::write(path, text) {
        panic!("cannot write {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_become_file_names() {
        assert_eq!(slug("firehose/sustained"), "firehose-sustained");
        assert_eq!(slug("completions/8_threads"), "completions-8_threads");
        let dir = Path::new("/r");
        assert_eq!(
            path_for(dir, "2026-09-30", None, "soak-60s"),
            Path::new("/r/2026-09-30-soak-60s.json")
        );
        assert_eq!(
            path_for(dir, "2026-09-30", Some("spin"), "layer-a"),
            Path::new("/r/2026-09-30-spin-layer-a.json")
        );
    }

    #[test]
    fn strings_and_numbers_are_valid_json() {
        assert_eq!(json_string("plain"), "\"plain\"");
        assert_eq!(json_string("a\\b\t"), "\"a\\\\b\\u0009\"");
        assert_eq!(json_number(1.5), "1.5");
        assert_eq!(json_number(f64::NAN), "null");
        assert_eq!(json_opt(None), "null");
        assert_eq!(json_opt(Some(3.0)), "3");
    }

    #[test]
    fn the_command_names_only_the_variables_that_are_set() {
        assert_eq!(
            command(
                "x",
                &["UNDRA_BENCH_SURELY_UNSET_1", "UNDRA_BENCH_SURELY_UNSET_2"]
            ),
            "x"
        );
    }

    #[test]
    fn machine_members_splice_into_an_object() {
        let text = format!("{{{}}}", machine_members(Some(1.5), None));
        assert!(
            text.contains("\"cpu\":") && text.contains("\"before\": 1.5"),
            "{text}"
        );
        assert!(text.contains("\"after\": null"), "{text}");
    }
}
