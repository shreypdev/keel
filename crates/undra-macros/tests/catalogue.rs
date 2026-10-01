//! The audit of the diagnostic catalogue (constitution R8; SPEC section 12), kept as a test.
//!
//! Every code of SPEC section 12 is cross-checked against the rest of the repository:
//!
//! | Check | Where it comes from |
//! |---|---|
//! | the table of the SPEC and the code table of `diag.rs` list the same codes | `docs/SPEC.md`, `src/impl_/diag.rs` |
//! | every code that a macro emits has a constant, and every constant has a row | `src/impl_/diag.rs` |
//! | every code is emitted somewhere that is not a test | the macros, `undra-meta`, `undra-bindgen`, `undra-wire`, `undra-signals` |
//! | every code has a golden that shows its real message | `tests/ui/*.stderr`, `crates/*/tests/golden/diagnostics/*.txt` |
//! | every message in a golden has what, note, help and the link of its code | the goldens |
//! | every docs link in the source is the link of a code of the catalogue | `crates/**/*.rs` |
//! | the codes of the command line are the same | `undra-cli/src/error.rs` |
//!
//! A new code, or a message that loses a part, fails here; `site/scripts/build-errors.mjs` builds
//! the public error-codes page from the same files, so a code that passes this test is on it with
//! its real message.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const DOCS: &str = "https://shreypdev.github.io/undra/docs/errors.html";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root")
}

fn read(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Every file below `dir` (not `target`) that `keep` accepts.
fn files(dir: &Path, keep: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name == "target" || name.to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            out.extend(files(&path, keep));
        } else if keep(&path) {
            out.push(path);
        }
    }
    out.sort();
    out
}

fn is_code(word: &str) -> bool {
    word.len() == 5 && word.starts_with(['E', 'C']) && word[1..].chars().all(|c| c.is_ascii_digit())
}

/// The rows `| E0001 | .. | .. |` of the tables of SPEC section 12: code to (raised by, trigger).
fn spec_rows() -> BTreeMap<String, (String, String)> {
    let spec = read(root().join("docs/SPEC.md"));
    let start = spec.find("## 12. Diagnostics").expect("section 12");
    let end = spec[start..].find("## 13.").expect("section 13") + start;
    let mut rows = BTreeMap::new();
    for line in spec[start..end].lines() {
        let cells: Vec<&str> = line
            .trim()
            .trim_matches('|')
            .splitn(3, " | ")
            .map(str::trim)
            .collect();
        if cells.len() == 3 && is_code(cells[0]) {
            let previous = rows.insert(
                cells[0].to_owned(),
                (cells[1].to_owned(), cells[2].to_owned()),
            );
            assert!(
                previous.is_none(),
                "{} is listed twice in SPEC section 12",
                cells[0]
            );
        }
    }
    rows
}

fn diag_source() -> String {
    read(root().join("crates/undra-macros/src/impl_/diag.rs"))
}

/// `E0001` of every `pub(crate) const E0001: &str = "E0001";`.
fn diag_constants() -> BTreeSet<String> {
    diag_source()
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("pub(crate) const ")?;
            let (name, _) = rest.split_once(':')?;
            is_code(name).then(|| name.to_owned())
        })
        .collect()
}

/// The rows `/// | E0001 | meaning |` of the table in `diag.rs`.
fn diag_table() -> BTreeMap<String, String> {
    diag_source()
        .lines()
        .filter_map(|line| {
            let cells: Vec<&str> = line
                .strip_prefix("/// |")?
                .split(" | ")
                .map(str::trim)
                .collect();
            let code = cells.first()?.trim_matches('|').trim();
            is_code(code).then(|| {
                (
                    code.to_owned(),
                    cells[1..]
                        .join(" | ")
                        .trim_end_matches('|')
                        .trim()
                        .to_owned(),
                )
            })
        })
        .collect()
}

/// The text of `path` before its test module.
fn code_part(path: &Path) -> String {
    let text = read(path);
    match text.find("#[cfg(test)]") {
        Some(at) => text[..at].to_owned(),
        None => text,
    }
}

/// Where each code is emitted: the code that is not a test, in the crates that raise diagnostics.
fn emitters() -> BTreeMap<String, Vec<String>> {
    let root = root();
    let mut sources: Vec<PathBuf> = files(&root.join("crates/undra-macros/src/impl_"), &|p| {
        p.extension().is_some_and(|e| e == "rs")
    });
    sources.retain(|p| !p.ends_with("diag.rs"));
    for extra in [
        "crates/undra-meta/src/validate.rs",
        "crates/undra-bindgen/src/validate.rs",
        "crates/undra-wire/src/codec.rs",
        // E0065: the write check, a runtime message (ADR-035).
        "crates/undra-signals/src/error.rs",
    ] {
        sources.push(root.join(extra));
    }
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in sources {
        let text = code_part(&path);
        let shown = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        for (index, _) in text.match_indices('E') {
            let word = &text[index..(index + 5).min(text.len())];
            if is_code(word) && !text[index + 5..].starts_with(|c: char| c.is_ascii_digit()) {
                let before = &text[..index];
                // `code::E0001`, `"E0001"` and `error[undra::E0001]`; not a mention in a comment.
                let line = before.rsplit('\n').next().unwrap_or("");
                if line.trim_start().starts_with("//") {
                    continue;
                }
                found
                    .entry(word.to_owned())
                    .or_default()
                    .push(shown.clone());
            }
        }
    }
    for sites in found.values_mut() {
        sites.sort();
        sites.dedup();
    }
    found
}

/// One message of a golden: its code and what, and the lines after the first.
struct Message {
    code: String,
    what: String,
    lines: Vec<String>,
    golden: String,
}

/// The Undra diagnostics in `text`, wherever they sit: after `error: `, after `error[E0277]: `, or
/// after `evaluation panicked: `.
fn messages(golden: &str, text: &str) -> Vec<Message> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Some(at) = line.find("error[undra::") else {
            continue;
        };
        // A mention inside the message (`error[undra::E0001]` in a note) is not a start.
        if !line[..at].trim_end().ends_with(':') && !line[..at].is_empty() {
            continue;
        }
        let rest = &line[at + "error[undra::".len()..];
        let Some((code, what)) = rest.split_once("]: ") else {
            continue;
        };
        let mut message = Message {
            code: code.to_owned(),
            what: what.to_owned(),
            lines: Vec::new(),
            golden: golden.to_owned(),
        };
        for next in &lines[index + 1..] {
            let trimmed = next.trim_start();
            // The fix of a command-line diagnostic may take several lines, indented under `= help:`.
            if !trimmed.starts_with("= ") && next.starts_with("          ") && !trimmed.is_empty() {
                if let Some(last) = message.lines.last_mut() {
                    last.push(' ');
                    last.push_str(trimmed);
                }
                continue;
            }
            if !trimmed.starts_with("= ") {
                break;
            }
            message.lines.push(trimmed.to_owned());
            if trimmed.starts_with("= docs:") {
                break;
            }
        }
        out.push(message);
    }
    out
}

/// `(golden file, text)` of every golden that can hold a message.
fn goldens() -> Vec<(String, String)> {
    let root = root();
    let mut out = Vec::new();
    for path in files(&root.join("crates"), &|p| {
        let in_golden_dir = p.to_string_lossy().contains("tests/golden/diagnostics/");
        let ui = p.to_string_lossy().contains("undra-macros/tests/ui/")
            && p.extension().is_some_and(|e| e == "stderr");
        (in_golden_dir && p.extension().is_some_and(|e| e == "txt")) || ui
    }) {
        let shown = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        out.push((shown, read(&path)));
    }
    out
}

/// The codes whose message is the compiler's own (SPEC section 12 says "rustc, named by a macro"):
/// their golden is the ui test named after the code, and it must quote the name the macro gave the
/// thing `rustc` reports, which is how a reader finds the code. Every other code needs a branded
/// message in a golden: a ui test named after a code proves nothing if its message is gone.
const COMPILER_MESSAGES: &[(&str, &str)] = &[(
    "E0022",
    "_undra_error_E0022_the_future_of_an_async_method_must_be_Send",
)];

/// The codes the goldens show: a branded message of the code, or (the codes of
/// [`COMPILER_MESSAGES`]) a ui test named after it that quotes the macro's name.
fn golden_codes() -> BTreeMap<String, BTreeSet<String>> {
    let mut codes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (name, text) in goldens() {
        for message in messages(&name, &text) {
            codes.entry(message.code).or_default().insert(name.clone());
        }
        let Some(file) = name.rsplit('/').next() else {
            continue;
        };
        let upper = file.to_uppercase();
        let named = upper
            .get(..5)
            .filter(|c| is_code(c) && name.ends_with(".stderr"));
        if let Some(&(code, quoted)) =
            named.and_then(|c| COMPILER_MESSAGES.iter().find(|(k, _)| *k == c))
        {
            assert!(
                text.contains(quoted),
                "{name}: the compiler's message of {code} must quote `{quoted}`, the name that leads to the code"
            );
            codes
                .entry(code.to_owned())
                .or_default()
                .insert(name.clone());
        }
    }
    codes
}

fn cli_codes() -> BTreeSet<String> {
    read(root().join("crates/undra-cli/src/error.rs"))
        .lines()
        .filter_map(|line| {
            let (_, rest) = line.trim().split_once("=> \"")?;
            let code = rest.strip_suffix("\",")?;
            is_code(code).then(|| code.to_owned())
        })
        .collect()
}

fn macro_codes_in_spec(rows: &BTreeMap<String, (String, String)>) -> BTreeSet<String> {
    rows.keys()
        .filter(|c| c.starts_with('E'))
        .cloned()
        .collect()
}

#[test]
fn the_spec_and_the_code_table_list_the_same_codes() {
    let spec = spec_rows();
    let spec_codes = macro_codes_in_spec(&spec);
    let table: BTreeSet<String> = diag_table().into_keys().collect();
    assert_eq!(
        spec_codes, table,
        "SPEC section 12 and the code table of src/impl_/diag.rs list different codes"
    );
    for (code, (raised_by, trigger)) in &spec {
        assert!(
            !raised_by.is_empty(),
            "{code}: no `raised by` in SPEC section 12"
        );
        assert!(
            trigger.len() > 8,
            "{code}: the trigger in SPEC section 12 is too short"
        );
    }
    for (code, meaning) in diag_table() {
        assert!(
            meaning.len() > 10,
            "{code}: the meaning in diag.rs is too short"
        );
    }
}

#[test]
fn every_constant_is_in_the_catalogue_and_every_emitted_code_has_one() {
    let spec = spec_rows();
    let constants = diag_constants();
    for code in &constants {
        assert!(
            spec.contains_key(code),
            "{code} is a constant of diag.rs but not in SPEC section 12"
        );
    }
    // The codes the macros raise through `Diag` all have a constant: there is no other way to
    // name one (`code::E0001`), so a code that is only a string is one the macros do not raise.
    let emitters = emitters();
    let macro_sites = |code: &str| {
        emitters
            .get(code)
            .is_some_and(|sites| sites.iter().any(|s| s.starts_with("crates/undra-macros/")))
    };
    for code in macro_codes_in_spec(&spec) {
        if macro_sites(&code) {
            assert!(
                constants.contains(&code),
                "{code} is emitted by a macro without a constant in diag.rs"
            );
        }
    }
}

#[test]
fn every_code_is_emitted_by_code_that_is_not_a_test() {
    let spec = spec_rows();
    let emitters = emitters();
    for code in macro_codes_in_spec(&spec) {
        assert!(
            emitters.contains_key(&code),
            "{code} is in the catalogue but nothing emits it: implement it or remove it from SPEC section 12 and diag.rs"
        );
    }
    for (code, sites) in &emitters {
        assert!(
            spec.contains_key(code),
            "{code} is emitted ({sites:?}) but is not in SPEC section 12"
        );
    }
}

#[test]
fn every_code_has_a_golden_with_its_real_message() {
    let spec = spec_rows();
    let shown = golden_codes();
    for (code, _) in COMPILER_MESSAGES {
        let (raised_by, _) = &spec[*code];
        assert!(
            raised_by.starts_with("rustc"),
            "{code} is listed as the compiler's message, but SPEC section 12 says it is raised by {raised_by}"
        );
    }
    for code in spec.keys().filter(|c| c.starts_with('E')) {
        assert!(
            shown.contains_key(code),
            "{code} has no golden: add a compile-fail test (tests/ui) or a message golden (crates/*/tests/golden/diagnostics/{code}.txt)"
        );
    }
    for code in shown.keys() {
        assert!(
            spec.contains_key(code),
            "a golden shows {code}, which is not in SPEC section 12"
        );
    }
}

#[test]
fn every_message_in_a_golden_has_what_why_fix_and_the_link_of_its_code() {
    let mut count = 0;
    for (name, text) in goldens() {
        for message in messages(&name, &text) {
            count += 1;
            let at = format!("{} ({}): {}", message.code, message.golden, message.what);
            assert!(message.what.trim().len() > 8, "{at}: what is missing");
            let kinds: Vec<&str> = message
                .lines
                .iter()
                .map(|l| l.split(':').next().unwrap_or(""))
                .collect();
            assert_eq!(
                kinds,
                ["= note", "= help", "= docs"],
                "{at}: needs a note, a help and a docs line, in that order"
            );
            for (line, prefix) in [
                (&message.lines[0], "= note: "),
                (&message.lines[1], "= help: "),
            ] {
                let text = line.strip_prefix(prefix).unwrap();
                assert!(text.trim().len() > 8, "{at}: `{prefix}` is empty");
            }
            assert_eq!(
                message.lines[2],
                format!("= docs: {DOCS}#{}", message.code),
                "{at}: the docs link must be the link of the code"
            );
        }
    }
    assert!(
        count > 50,
        "only {count} messages found: the parser lost the goldens"
    );
}

#[test]
fn every_docs_link_in_the_source_is_the_link_of_a_code_of_the_catalogue() {
    let spec = spec_rows();
    let root = root();
    let mut links = 0;
    // The source of the crates: not the tests, which quote fragments of a link.
    for path in files(&root.join("crates"), &|p| {
        let shown = p
            .strip_prefix(&root)
            .unwrap_or(p)
            .to_string_lossy()
            .into_owned();
        p.extension().is_some_and(|e| e == "rs") && shown.contains("/src/")
    }) {
        let text = code_part(&path);
        for (at, _) in text.match_indices("errors.html") {
            let rest = &text[at + "errors.html".len()..];
            // A base URL that a format string finishes (`{DOCS_BASE}#{code}`) ends the literal.
            if !rest.starts_with('#') {
                continue;
            }
            let word = &rest[1..rest.len().min(6)];
            if !is_code(word) {
                continue; // `#{code}`, `#E00NN`
            }
            links += 1;
            assert!(
                spec.contains_key(word),
                "{}: a link to {word}, which is not in SPEC section 12",
                path.display()
            );
            let before = &text[..at];
            assert!(
                before.ends_with("https://shreypdev.github.io/undra/docs/"),
                "{}: the link to {word} must start with {DOCS}",
                path.display()
            );
        }
        // The base of the links that are assembled at run time or at expansion.
        for (at, _) in text.match_indices("/errors.html\"") {
            let before = &text[..at];
            assert!(
                before.ends_with("https://shreypdev.github.io/undra/docs"),
                "{}: a docs base that is not {DOCS}",
                path.display()
            );
        }
    }
    assert!(links > 5, "only {links} links found");
    assert_eq!(undra::meta::diag::DOCS_BASE, DOCS);
    assert!(
        diag_source().contains(&format!("pub(crate) const DOCS_BASE: &str = \"{DOCS}\";")),
        "the macros' DOCS_BASE is not {DOCS}"
    );
}

#[test]
fn the_command_line_codes_are_the_catalogues() {
    let spec = spec_rows();
    let in_spec: BTreeSet<String> = spec
        .keys()
        .filter(|c| c.starts_with('C'))
        .cloned()
        .collect();
    assert_eq!(
        cli_codes(),
        in_spec,
        "the codes of undra-cli's `Code` and the second table of SPEC section 12 differ"
    );
}

/// The command-line codes whose message cannot be produced by a test that needs nothing but the
/// binary: C0006 needs a built core to load, C0012 a platform that this machine is not, C0013 a
/// dev server that fails to start. Their messages are covered by unit tests in `undra-cli/src`;
/// the page shows their meaning and trigger. A code that can be produced cheaply does not belong
/// here: add its test to `crates/undra-cli/tests/diagnostics.rs`.
const CLI_WITHOUT_A_GOLDEN: &[&str] = &["C0006", "C0012", "C0013"];

#[test]
fn every_command_line_code_has_a_golden_or_a_reason() {
    let shown = golden_codes();
    let spec = spec_rows();
    for code in spec.keys().filter(|c| c.starts_with('C')) {
        let listed = CLI_WITHOUT_A_GOLDEN.contains(&code.as_str());
        assert_eq!(
            shown.contains_key(code),
            !listed,
            "{code}: {}",
            if listed {
                "has a golden now: remove it from CLI_WITHOUT_A_GOLDEN"
            } else {
                "needs a test in crates/undra-cli/tests/diagnostics.rs (or a reason in CLI_WITHOUT_A_GOLDEN)"
            }
        );
    }
}

#[test]
fn the_error_codes_page_is_built_from_the_catalogue() {
    // `site/scripts/build-errors.mjs` reads the same tables and goldens; this keeps the contract
    // that it needs: the SPEC rows have three cells, and the page's families cover every code.
    let spec = spec_rows();
    assert!(spec.len() >= 44, "{}", spec.len());
    for (code, (_, trigger)) in &spec {
        assert!(!trigger.contains('\n'), "{code}: a table row is one line");
    }
}
