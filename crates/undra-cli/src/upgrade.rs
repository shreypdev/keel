//! `undra upgrade`: finding every place a project pins its Undra version, and the edits that
//! move them all to this `undra`'s version in lockstep.
//!
//! A project pins Undra in six kinds of file, each in the shape `undra init` writes it:
//!
//! | File | The pin | After an upgrade to `1.2.3` |
//! |---|---|---|
//! | `Cargo.toml` of the core (and any crate of the project) | `undra = { git = "...", tag = "v1.0.0" }`, or a registry version | `tag = "v1.2.3"` / `"1.2.3"` |
//! | `undra.toml` | `[undra] version = "1.0"` | `"1.2"` |
//! | `web/package.json` (any `package.json`) | `"@undra/runtime": "^1.0.0"`, `"@undra/react-native"` | `"^1.2.0"` |
//! | `android/**/*.gradle(.kts)` | `dev.undra:runtime:1.0.0`, `dev.undra:android-adapters:1.0.0` | `1.2.0` |
//! | `ios/**/project.pbxproj` | the `undra-swift` package's `minimumVersion = 1.0.0;` | `1.2.0` |
//! | `.github/workflows/*.yml` | `UNDRA_VERSION: "1.0.0"` | `"1.2.3"` |
//!
//! The runtimes are pinned to the release line (`major.minor.0`, the registries' compatible range),
//! the crates and the workflow to the exact release, exactly as `undra init` pins them, so an
//! upgraded project and a fresh one agree. A dependency on a checkout of the repository (a `path`)
//! is not a pin a release can move: [`Plan::path_pins`] lists them and the command changes nothing.
//!
//! The editors work on lines and change only the version text (or, for a git dependency, the one
//! `tag`/`rev`/`branch` pair), so a file's comments and formatting survive. A version that is not
//! written out (a Gradle variable, a dynamic `+`) and anything in a comment is left alone. Each returns the new
//! text, what it changed and what it saw ([`FileResult`]); the same pass that plans the edits
//! therefore also reads the version the project is on.

use std::path::{Path, PathBuf};

use crate::config::{UNDRA_REPO_URL, UNDRA_SWIFT_PACKAGE_URL};
use crate::semver::Semver;

/// One edit: a line, what it pins, and the text before and after.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The 1-based line.
    pub line: usize,
    /// What the line pins (`undra dependency`, `@undra/runtime`).
    pub what: String,
    /// The line before.
    pub old: String,
    /// The line after.
    pub new: String,
}

/// A pin the editors saw, whether or not it needed to change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pin {
    /// What it pins (`undra dependency`, `@undra/runtime`).
    pub what: String,
    /// The version it names; `None` when it names a commit or a branch.
    pub version: Option<Semver>,
    /// The pin as written (`tag v0.1.0`, `^0.1.0`, `rev 3a4f9c1`).
    pub shown: String,
    /// The 1-based line.
    pub line: usize,
    /// Whether it names one exact release (a crate's tag or version, the workflow's
    /// `UNDRA_VERSION`) rather than a release line (`0.1`, `^0.1.0`): only an exact pin says which
    /// release the project was last on.
    pub exact: bool,
}

/// A dependency on something that is not a release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unmovable {
    /// The 1-based line.
    pub line: usize,
    /// The line, trimmed.
    pub text: String,
    /// Why a release cannot move it.
    pub why: Why,
}

/// Why a dependency is not moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// A `path` dependency: a checkout of the Undra repository.
    Path,
    /// A git dependency on a repository that is not Undra's (a fork).
    Fork,
    /// A version that is not written out (a Gradle variable such as `$undraVersion`, a dynamic
    /// `+`): it is not a pin `undra init` writes, so the author moves it where it is defined.
    NotALiteral,
}

/// What an editor found in one file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileResult {
    /// The file with the pins moved (equal to the input when nothing changed).
    pub after: String,
    /// What changed.
    pub changes: Vec<Change>,
    /// Every pin seen, changed or not.
    pub pins: Vec<Pin>,
    /// Dependencies a release does not move.
    pub unmovable: Vec<Unmovable>,
}

/// The release a pin of each kind is moved to, as text.
#[derive(Clone, Debug)]
pub struct Target {
    /// The version of this `undra`.
    pub version: Semver,
}

impl Target {
    /// `1.2.3`: the crates' tag (with a `v`) and the workflow.
    #[must_use]
    pub fn full(&self) -> String {
        self.version.to_string()
    }

    /// `1.2.0`: what the registries' runtimes are asked for.
    #[must_use]
    pub fn runtime(&self) -> String {
        format!("{}.0", self.version.line())
    }

    /// `1.2`: `[undra] version` of `undra.toml`.
    #[must_use]
    pub fn line(&self) -> String {
        self.version.line()
    }
}

/// The directories nothing is scanned in: build output, dependencies, other tools' state.
const SKIPPED_DIRS: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    "build",
    "dist",
    "DerivedData",
    ".gradle",
    "Pods",
    ".build",
    ".idea",
    ".next",
    ".turbo",
    "xcuserdata",
];

/// The crates of the Undra repository: they are released in lockstep, so a project that names any
/// of them moves them all together. (A third-party crate that happens to start with `undra-` is
/// not one of them.)
const UNDRA_CRATES: &[&str] = &[
    "undra",
    "undra-meta",
    "undra-wire",
    "undra-macros",
    "undra-signals",
    "undra-runtime",
    "undra-ports",
    "undra-query",
    "undra-ffi",
    "undra-transport",
    "undra-bindgen",
    "undra-cli",
];

/// Whether `name` is a crate of the Undra repository.
fn is_undra_crate(name: &str) -> bool {
    UNDRA_CRATES.contains(&name)
}

/// Whether `url` is Undra's repository: with or without `.git` or a trailing slash, in any case,
/// over https, http or ssh (`git@github.com:shreypdev/undra.git`, `ssh://git@github.com/...`).
fn is_undra_repo(url: &str) -> bool {
    same_repository(url, UNDRA_REPO_URL)
}

/// Whether `url` names the repository `wanted` (an `https://` URL), in any of the spellings
/// [`is_undra_repo`] accepts.
fn same_repository(url: &str, wanted: &str) -> bool {
    let wanted = wanted
        .trim_end_matches('/')
        .trim_start_matches("https://")
        .to_ascii_lowercase();
    let given = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let given = given
        .strip_prefix("https://")
        .or_else(|| given.strip_prefix("http://"))
        .or_else(|| given.strip_prefix("ssh://git@"))
        .or_else(|| given.strip_prefix("git@"))
        .unwrap_or(given)
        .replacen(':', "/", 1)
        .to_ascii_lowercase();
    given == wanted
}

/// The pieces of `line` around the quoted value of `key = "value"`: the text before the value, the
/// value, and the text after it (the closing quote on). `None` when the line is not that.
fn quoted(line: &str, key: &str) -> Option<(String, String, String)> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix(key)?;
    let after_key = rest.trim_start();
    let after_eq = after_key.strip_prefix('=')?.trim_start();
    let open = after_eq.strip_prefix('"')?;
    let end = open.find('"')?;
    let value_start = line.len() - open.len();
    Some((
        line[..value_start].to_owned(),
        open[..end].to_owned(),
        line[value_start + end..].to_owned(),
    ))
}

/// Splits a text into lines that keep their line endings.
fn lines_of(text: &str) -> impl Iterator<Item = (usize, &str, &str)> {
    text.split_inclusive('\n').enumerate().map(|(i, raw)| {
        let line = raw.trim_end_matches(['\n', '\r']);
        (i + 1, line, &raw[line.len()..])
    })
}

// ---------------------------------------------------------------------------------------------
// undra.toml

/// Moves `[undra] version`.
#[must_use]
pub fn edit_undra_toml(text: &str, target: &Target) -> FileResult {
    let mut result = FileResult::default();
    let mut table = String::new();
    for (n, line, eol) in lines_of(text) {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            table = trimmed.trim_matches(['[', ']']).trim().to_owned();
        }
        if table != "undra" {
            result.after.push_str(line);
            result.after.push_str(eol);
            continue;
        }
        if let Some((before, value, after)) = quoted(line, "version") {
            result.pins.push(Pin {
                what: "[undra] version".to_owned(),
                version: Semver::parse(&value),
                shown: value.clone(),
                line: n,
                exact: false,
            });
            let new = format!("{before}{}{after}", target.line());
            if new != line {
                result.changes.push(Change {
                    line: n,
                    what: "[undra] version".to_owned(),
                    old: line.trim().to_owned(),
                    new: new.trim().to_owned(),
                });
            }
            result.after.push_str(&new);
        } else {
            if quoted(line, "path").is_some() {
                result.unmovable.push(Unmovable {
                    line: n,
                    text: trimmed.to_owned(),
                    why: Why::Path,
                });
            }
            result.after.push_str(line);
        }
        result.after.push_str(eol);
    }
    result
}

// ---------------------------------------------------------------------------------------------
// Cargo.toml

/// One `key = value` pair of an inline table, as written.
#[derive(Clone, Debug)]
struct Field {
    key: String,
    /// The value without its quotes when it is a string, else the raw text.
    value: String,
    /// The pair as written (`tag = "v0.1.0"`).
    raw: String,
}

/// Splits `{ a = 1, b = "x, y", c = ["p", "q"] }` into its pairs; `None` when it is not one line of
/// an inline table.
fn inline_table(text: &str) -> Option<Vec<Field>> {
    let inner = text.trim().strip_prefix('{')?;
    // The closing brace is the last one before an optional trailing comment.
    let end = inner.rfind('}')?;
    let inner = &inner[..end];
    let mut fields = Vec::new();
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut start = 0;
    let mut chars = inner.char_indices().peekable();
    let push = |piece: &str, fields: &mut Vec<Field>| -> Option<()> {
        let piece = piece.trim();
        if piece.is_empty() {
            return Some(());
        }
        let (key, value) = piece.split_once('=')?;
        let value = value.trim();
        let unquoted = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(value);
        fields.push(Field {
            key: key.trim().to_owned(),
            value: unquoted.to_owned(),
            raw: piece.to_owned(),
        });
        Some(())
    };
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => in_string = !in_string,
            '\\' if in_string => {
                chars.next();
            }
            '[' | '{' if !in_string => depth += 1,
            ']' | '}' if !in_string => depth -= 1,
            ',' if !in_string && depth == 0 => {
                push(&inner[start..i], &mut fields)?;
                start = i + 1;
            }
            _ => {}
        }
    }
    push(&inner[start..], &mut fields)?;
    Some(fields)
}

fn render_inline(fields: &[Field]) -> String {
    format!(
        "{{ {} }}",
        fields
            .iter()
            .map(|f| f.raw.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// What a dependency's fields say.
struct Dep<'a> {
    git: Option<&'a str>,
    tag: Option<&'a str>,
    rev: Option<&'a str>,
    branch: Option<&'a str>,
    version: Option<&'a str>,
    path: Option<&'a str>,
}

impl<'a> Dep<'a> {
    fn of(fields: &'a [Field]) -> Dep<'a> {
        let get = |key: &str| {
            fields
                .iter()
                .find(|f| f.key == key)
                .map(|f| f.value.as_str())
        };
        Dep {
            git: get("git"),
            tag: get("tag"),
            rev: get("rev"),
            branch: get("branch"),
            version: get("version"),
            path: get("path"),
        }
    }
}

/// `=0.1.0`, `^0.1`, `0.1.0` into the operator and the version.
fn split_requirement(text: &str) -> (&str, &str) {
    let text = text.trim();
    let at = text
        .find(|c: char| c.is_ascii_digit())
        .unwrap_or(text.len());
    (&text[..at], &text[at..])
}

/// How a crate dependency is read and moved. `fields` are the dependency's keys (an inline table, or
/// the lines of a `[dependencies.undra]` section); the returned fields are what it becomes.
fn move_dependency(
    name: &str,
    line: usize,
    text: &str,
    fields: &[Field],
    target: &Target,
    result: &mut FileResult,
) -> Option<Vec<Field>> {
    let dep = Dep::of(fields);
    let what = format!("{name} dependency");
    if dep.path.is_some() {
        result.unmovable.push(Unmovable {
            line,
            text: text.trim().to_owned(),
            why: Why::Path,
        });
        return None;
    }
    let tag = format!("v{}", target.full());
    if let Some(url) = dep.git {
        if !is_undra_repo(url) {
            result.unmovable.push(Unmovable {
                line,
                text: text.trim().to_owned(),
                why: Why::Fork,
            });
            return None;
        }
        let (version, shown) = match (dep.tag, dep.rev, dep.branch) {
            (Some(tag), _, _) => (Semver::parse(tag), format!("tag {tag}")),
            (None, Some(rev), _) => (None, format!("rev {rev}")),
            (None, None, Some(branch)) => (None, format!("branch {branch}")),
            (None, None, None) => (None, "the default branch".to_owned()),
        };
        result.pins.push(Pin {
            what: what.clone(),
            version,
            shown,
            line,
            exact: true,
        });
        // The same repository, pinned by the release tag: `tag` replaces whichever of
        // `tag`/`rev`/`branch` the dependency had, in its place.
        let new_pair = Field {
            key: "tag".to_owned(),
            value: tag.clone(),
            raw: format!("tag = \"{tag}\""),
        };
        let mut out = Vec::new();
        let mut placed = false;
        for f in fields {
            if matches!(f.key.as_str(), "tag" | "rev" | "branch") {
                if !placed {
                    out.push(new_pair.clone());
                    placed = true;
                }
            } else {
                out.push(f.clone());
                if f.key == "git"
                    && dep.tag.is_none()
                    && dep.rev.is_none()
                    && dep.branch.is_none()
                    && !placed
                {
                    out.push(new_pair.clone());
                    placed = true;
                }
            }
        }
        return Some(out);
    }
    if let Some(requirement) = dep.version {
        let (operator, version) = split_requirement(requirement);
        result.pins.push(Pin {
            what,
            version: Semver::parse(version),
            shown: requirement.to_owned(),
            line,
            exact: true,
        });
        let new_version = format!("{operator}{}", target.full());
        return Some(
            fields
                .iter()
                .map(|f| {
                    if f.key == "version" {
                        Field {
                            key: f.key.clone(),
                            value: new_version.clone(),
                            raw: format!("version = \"{new_version}\""),
                        }
                    } else {
                        f.clone()
                    }
                })
                .collect(),
        );
    }
    None
}

/// Whether a table header names a dependency table; the crate name when it is a dotted one
/// (`[dependencies.undra]`).
fn dependency_table(table: &str) -> (bool, Option<String>) {
    let kinds = ["dependencies", "dev-dependencies", "build-dependencies"];
    let is_plain = |t: &str| {
        kinds.contains(&t)
            || t == "workspace.dependencies"
            || kinds.iter().any(|k| t.ends_with(&format!(".{k}")))
    };
    if is_plain(table) {
        return (true, None);
    }
    for kind in kinds {
        if let Some(at) = table.rfind(&format!("{kind}.")) {
            let name = table[at + kind.len() + 1..].trim_matches(['"', '\'']);
            let before = &table[..at];
            if (before.is_empty() || before.ends_with('.') || before == "workspace.")
                && is_undra_crate(name)
            {
                return (false, Some(name.to_owned()));
            }
        }
    }
    (false, None)
}

/// Moves the Undra crates a manifest depends on.
#[must_use]
pub fn edit_cargo_toml(text: &str, target: &Target) -> FileResult {
    let mut result = FileResult::default();
    let mut in_deps = false;
    // The `[dependencies.undra]` section being collected.
    let mut dotted: Option<DottedSection> = None;

    /// A `[dependencies.<crate>]` section: the crate, the line of its header and its lines
    /// (number, text, line ending).
    struct DottedSection {
        name: String,
        first: usize,
        lines: Vec<(usize, String, String)>,
    }

    fn finish(dotted: &mut Option<DottedSection>, target: &Target, result: &mut FileResult) {
        let Some(DottedSection { name, first, lines }) = dotted.take() else {
            return;
        };
        // The section's keys as fields: `git = "url"` and so on.
        let fields: Vec<Field> = lines
            .iter()
            .filter_map(|(_, line, _)| {
                let (key, value) = line.split_once('=')?;
                let value = value.split('#').next().unwrap_or(value).trim();
                Some(Field {
                    key: key.trim().to_owned(),
                    value: value.trim_matches('"').to_owned(),
                    raw: line.trim().to_owned(),
                })
            })
            .collect();
        let header = format!("[dependencies.{name}]");
        let moved = move_dependency(&name, first, &header, &fields, target, result);
        if moved.is_none() {
            for (_, line, eol) in lines {
                result.after.push_str(&line);
                result.after.push_str(&eol);
            }
            return;
        }
        let dep = Dep::of(&fields);
        let had_ref = dep.tag.is_some() || dep.rev.is_some() || dep.branch.is_some();
        let tag_line = format!("tag = \"v{}\"", target.full());
        let what = format!("{name} dependency");
        let mut placed_tag = false;
        for (n, line, eol) in lines {
            let key = line.split('=').next().unwrap_or("").trim().to_owned();
            let indent = line[..line.len() - line.trim_start().len()].to_owned();
            // What follows the value (` # the release`) stays on the line.
            let tail = line
                .split_once('=')
                .and_then(|(_, value)| {
                    let value = value.trim_start().strip_prefix('"')?;
                    let end = value.find('"')?;
                    Some(value[end + 1..].to_owned())
                })
                .unwrap_or_default();
            let record = |new: &str, result: &mut FileResult| {
                result.changes.push(Change {
                    line: n,
                    what: what.clone(),
                    old: line.trim().to_owned(),
                    new: new.trim().to_owned(),
                });
            };
            match key.as_str() {
                // `tag`, `rev` and `branch` are one pair: the first of them becomes the release tag.
                "tag" | "rev" | "branch" if !placed_tag => {
                    placed_tag = true;
                    let new = format!("{indent}{tag_line}{tail}");
                    if new != line {
                        record(&new, result);
                    }
                    result.after.push_str(&new);
                    result.after.push_str(&eol);
                }
                "tag" | "rev" | "branch" => record("", result),
                "git" => {
                    result.after.push_str(&line);
                    result.after.push_str(&eol);
                    if !had_ref && !placed_tag {
                        placed_tag = true;
                        result.changes.push(Change {
                            line: n,
                            what: what.clone(),
                            old: String::new(),
                            new: tag_line.clone(),
                        });
                        result.after.push_str(&format!("{indent}{tag_line}{eol}"));
                    }
                }
                "version" if dep.version.is_some() => {
                    let (operator, _) = split_requirement(dep.version.unwrap_or(""));
                    let new = format!("{indent}version = \"{operator}{}\"{tail}", target.full());
                    if new != line {
                        record(&new, result);
                    }
                    result.after.push_str(&new);
                    result.after.push_str(&eol);
                }
                _ => {
                    result.after.push_str(&line);
                    result.after.push_str(&eol);
                }
            }
        }
    }

    for (n, line, eol) in lines_of(text) {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            finish(&mut dotted, target, &mut result);
            let table = trimmed
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or("")
                .trim();
            let (plain, dotted_name) = dependency_table(table);
            in_deps = plain;
            if let Some(name) = dotted_name {
                dotted = Some(DottedSection {
                    name,
                    first: n,
                    lines: Vec::new(),
                });
            }
            result.after.push_str(line);
            result.after.push_str(eol);
            continue;
        }
        if let Some(section) = dotted.as_mut() {
            section.lines.push((n, line.to_owned(), eol.to_owned()));
            continue;
        }
        if !in_deps {
            result.after.push_str(line);
            result.after.push_str(eol);
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            result.after.push_str(line);
            result.after.push_str(eol);
            continue;
        };
        let name = key.trim();
        let value = value.trim();
        if !is_undra_crate(name) {
            result.after.push_str(line);
            result.after.push_str(eol);
            continue;
        }
        let indent = &line[..line.len() - line.trim_start().len()];
        // `undra = "0.1"`, with an optional comment.
        if let Some(rest) = value.strip_prefix('"') {
            if let Some(end) = rest.find('"') {
                let requirement = &rest[..end];
                let comment = &rest[end + 1..];
                let fields = vec![Field {
                    key: "version".to_owned(),
                    value: requirement.to_owned(),
                    raw: format!("version = \"{requirement}\""),
                }];
                if let Some(moved) = move_dependency(name, n, line, &fields, target, &mut result) {
                    let new_version = moved[0].value.clone();
                    let new = format!("{indent}{name} = \"{new_version}\"{comment}");
                    if new != line {
                        result.changes.push(Change {
                            line: n,
                            what: format!("{name} dependency"),
                            old: line.trim().to_owned(),
                            new: new.trim().to_owned(),
                        });
                    }
                    result.after.push_str(&new);
                    result.after.push_str(eol);
                    continue;
                }
            }
        } else if let Some(fields) = inline_table(value) {
            if let Some(moved) = move_dependency(name, n, line, &fields, target, &mut result) {
                let close = value.rfind('}').map_or(value.len(), |i| i + 1);
                let new = format!(
                    "{indent}{name} = {}{}",
                    render_inline(&moved),
                    &value[close..]
                );
                // Keep the line as it was when nothing in the table changed (spacing included).
                if moved.iter().map(|f| f.raw.as_str()).collect::<Vec<_>>()
                    != fields.iter().map(|f| f.raw.as_str()).collect::<Vec<_>>()
                {
                    result.changes.push(Change {
                        line: n,
                        what: format!("{name} dependency"),
                        old: line.trim().to_owned(),
                        new: new.trim().to_owned(),
                    });
                    result.after.push_str(&new);
                    result.after.push_str(eol);
                    continue;
                }
            }
        }
        result.after.push_str(line);
        result.after.push_str(eol);
    }
    finish(&mut dotted, target, &mut result);
    result
}

// ---------------------------------------------------------------------------------------------
// package.json

/// The npm packages of the Undra repository, released in lockstep: the runtime and the React Native
/// host over it (`@undra/react-native` peer-depends on the same line of `@undra/runtime`).
const UNDRA_NPM_PACKAGES: &[&str] = &["@undra/runtime", "@undra/react-native"];

/// Moves `"@undra/runtime"` and `"@undra/react-native"` to `^<major>.<minor>.0`.
#[must_use]
pub fn edit_package_json(text: &str, target: &Target) -> FileResult {
    let mut result = FileResult::default();
    for (n, line, eol) in lines_of(text) {
        let trimmed = line.trim_start();
        let package = UNDRA_NPM_PACKAGES
            .iter()
            .find(|name| trimmed.starts_with(&format!("\"{name}\"")));
        if let Some(&package) = package {
            if let Some(colon) = line.find(':') {
                let after = &line[colon + 1..];
                if let Some(open) = after.find('"') {
                    if let Some(len) = after[open + 1..].find('"') {
                        let value = &after[open + 1..open + 1 + len];
                        let is_local = ["file:", "link:", "workspace:", "portal:"]
                            .iter()
                            .any(|p| value.starts_with(p));
                        if is_local {
                            result.unmovable.push(Unmovable {
                                line: n,
                                text: trimmed.trim().to_owned(),
                                why: Why::Path,
                            });
                        } else {
                            let (_, version) = split_requirement(value);
                            result.pins.push(Pin {
                                what: package.to_owned(),
                                version: Semver::parse(version),
                                shown: value.to_owned(),
                                line: n,
                                exact: false,
                            });
                            let new_value = format!("^{}", target.runtime());
                            let start = colon + 1 + open + 1;
                            let new =
                                format!("{}{new_value}{}", &line[..start], &line[start + len..]);
                            if new != line {
                                result.changes.push(Change {
                                    line: n,
                                    what: package.to_owned(),
                                    old: line.trim().to_owned(),
                                    new: new.trim().to_owned(),
                                });
                            }
                            result.after.push_str(&new);
                            result.after.push_str(eol);
                            continue;
                        }
                    }
                }
            }
        }
        result.after.push_str(line);
        result.after.push_str(eol);
    }
    result
}

// ---------------------------------------------------------------------------------------------
// Gradle

/// For each byte of `line`, whether it is in a comment of a Gradle script (Kotlin or Groovy: `//` to
/// the end of the line, `/* ... */` across lines, never inside a string). `in_block` carries an
/// open block comment from one line to the next.
fn gradle_comment_mask(line: &str, in_block: &mut bool) -> Vec<bool> {
    let bytes = line.as_bytes();
    let mut mask = vec![false; bytes.len()];
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        if *in_block {
            mask[i] = true;
            if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                mask[i + 1] = true;
                *in_block = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        match (quote, bytes[i]) {
            (Some(_), b'\\') => i += 1,
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(bytes[i]),
            (None, b'/') if bytes.get(i + 1) == Some(&b'/') => {
                for m in &mut mask[i..] {
                    *m = true;
                }
                break;
            }
            (None, b'/') if bytes.get(i + 1) == Some(&b'*') => {
                *in_block = true;
                mask[i] = true;
            }
            (None, _) => {}
        }
        i += 1;
    }
    mask
}

/// Moves `dev.undra:runtime:<v>` and `dev.undra:android-adapters:<v>` to `<major>.<minor>.0`.
///
/// Only a version written out is moved; a variable (`$undraVersion`), a dynamic version (`+`) and
/// anything in a comment are left as they are (a variable is reported: its definition is where the
/// author moves it).
#[must_use]
pub fn edit_gradle(text: &str, target: &Target) -> FileResult {
    let mut result = FileResult::default();
    let mut in_block = false;
    for (n, line, eol) in lines_of(text) {
        let comment = gradle_comment_mask(line, &mut in_block);
        let mut out = line.to_owned();
        let mut search_from = 0;
        while let Some(at) = out[search_from..].find("dev.undra:") {
            let start = search_from + at;
            if comment.get(start).copied().unwrap_or(false) {
                search_from = start + "dev.undra:".len();
                continue;
            }
            let rest = &out[start + "dev.undra:".len()..];
            let Some(colon) = rest.find(':') else { break };
            let module = &rest[..colon];
            let after = &rest[colon + 1..];
            let end = after
                .find(['"', '\'', ')', ' ', ','])
                .unwrap_or(after.len());
            let version = &after[..end];
            let version_start = start + "dev.undra:".len() + colon + 1;
            search_from = version_start + end;
            if !matches!(module, "runtime" | "android-adapters") || version.is_empty() {
                continue;
            }
            if version.contains("SNAPSHOT") {
                // The Kotlin runtime built from a checkout (`includeBuild`): not a release.
                result.unmovable.push(Unmovable {
                    line: n,
                    text: line.trim().to_owned(),
                    why: Why::Path,
                });
                continue;
            }
            if Semver::parse(version).is_none() {
                result.unmovable.push(Unmovable {
                    line: n,
                    text: line.trim().to_owned(),
                    why: Why::NotALiteral,
                });
                continue;
            }
            result.pins.push(Pin {
                what: format!("dev.undra:{module}"),
                version: Semver::parse(version),
                shown: version.to_owned(),
                line: n,
                exact: false,
            });
            let new_version = target.runtime();
            out.replace_range(version_start..version_start + end, &new_version);
            search_from = version_start + new_version.len();
        }
        if out != line {
            result.changes.push(Change {
                line: n,
                what: "dev.undra runtime".to_owned(),
                old: line.trim().to_owned(),
                new: out.trim().to_owned(),
            });
        }
        result.after.push_str(&out);
        result.after.push_str(eol);
    }
    result
}

// ---------------------------------------------------------------------------------------------
// Xcode project

/// Moves the Undra Swift package requirement of a `project.pbxproj` to `<major>.<minor>.0`.
#[must_use]
pub fn edit_pbxproj(text: &str, target: &Target) -> FileResult {
    let mut result = FileResult::default();
    // The lines of each XCRemoteSwiftPackageReference object, to know whether it is Undra's.
    let lines: Vec<(usize, &str, &str)> = lines_of(text).collect();
    let mut is_undra_block = vec![false; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if lines[i].1.contains("isa = XCRemoteSwiftPackageReference;") {
            // The object ends at the next `};` that is indented as the object itself.
            let mut end = i;
            while end + 1 < lines.len() && lines[end + 1].1 != "\t\t};" {
                end += 1;
            }
            // Undra's package by its URL, not by a substring: `acme/undra-charts` is someone else's.
            let ours = lines[i..=end].iter().any(|(_, l, _)| {
                quoted(l, "repositoryURL")
                    .is_some_and(|(_, url, _)| same_repository(&url, UNDRA_SWIFT_PACKAGE_URL))
            });
            if ours {
                for flag in &mut is_undra_block[i..=end] {
                    *flag = true;
                }
            }
            i = end + 1;
        } else {
            i += 1;
        }
    }
    for (i, (n, line, eol)) in lines.iter().enumerate() {
        let mut done = false;
        if is_undra_block[i] {
            for key in ["minimumVersion", "version"] {
                let trimmed = line.trim_start();
                let Some(rest) = trimmed.strip_prefix(key) else {
                    continue;
                };
                let Some(rest) = rest.trim_start().strip_prefix('=') else {
                    continue;
                };
                let value = rest.trim().trim_end_matches(';').trim_matches('"');
                if value.is_empty() {
                    continue;
                }
                result.pins.push(Pin {
                    what: "the Undra Swift package".to_owned(),
                    version: Semver::parse(value),
                    shown: value.to_owned(),
                    line: *n,
                    exact: false,
                });
                let indent = &line[..line.len() - trimmed.len()];
                let new = format!("{indent}{key} = {};", target.runtime());
                if new != *line {
                    result.changes.push(Change {
                        line: *n,
                        what: "the Undra Swift package".to_owned(),
                        old: line.trim().to_owned(),
                        new: new.trim().to_owned(),
                    });
                }
                result.after.push_str(&new);
                result.after.push_str(eol);
                done = true;
                break;
            }
        }
        if !done {
            result.after.push_str(line);
            result.after.push_str(eol);
        }
    }
    // A local package reference to a checkout is a path pin.
    for (n, line, _) in &lines {
        if line.contains("isa = XCLocalSwiftPackageReference;") {
            continue;
        }
        if line.contains("relativePath") && line.contains("runtimes/swift/UndraRuntime") {
            result.unmovable.push(Unmovable {
                line: *n,
                text: line.trim().to_owned(),
                why: Why::Path,
            });
        }
    }
    result
}

// ---------------------------------------------------------------------------------------------
// GitHub workflows

/// Moves `UNDRA_VERSION` of a workflow to the full version.
#[must_use]
pub fn edit_workflow(text: &str, target: &Target) -> FileResult {
    let mut result = FileResult::default();
    for (n, line, eol) in lines_of(text) {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("UNDRA_VERSION:") {
            let value_part = rest.trim_start();
            let quote = value_part
                .chars()
                .next()
                .filter(|c| matches!(c, '"' | '\''));
            let body = match quote {
                Some(q) => value_part[1..].split(q).next().unwrap_or(""),
                None => value_part.split(['#', ' ']).next().unwrap_or(""),
            };
            if !body.is_empty() && Semver::parse(body).is_some() {
                result.pins.push(Pin {
                    what: "UNDRA_VERSION".to_owned(),
                    version: Semver::parse(body),
                    shown: body.to_owned(),
                    line: n,
                    exact: true,
                });
                let at = line.len() - value_part.len() + usize::from(quote.is_some());
                let new = format!(
                    "{}{}{}",
                    &line[..at],
                    target.full(),
                    &line[at + body.len()..]
                );
                if new != line {
                    result.changes.push(Change {
                        line: n,
                        what: "UNDRA_VERSION".to_owned(),
                        old: line.trim().to_owned(),
                        new: new.trim().to_owned(),
                    });
                }
                result.after.push_str(&new);
                result.after.push_str(eol);
                continue;
            }
        }
        result.after.push_str(line);
        result.after.push_str(eol);
    }
    result
}

// ---------------------------------------------------------------------------------------------
// The plan

/// Everything `undra upgrade` found in a project and the edits it makes.
#[derive(Clone, Debug)]
pub struct Plan {
    /// The release being moved to.
    pub target: Semver,
    /// The files that change, relative to the project root, with their new text.
    pub files: Vec<FileEdit>,
    /// Every pin seen, with the file it is in.
    pub pins: Vec<(PathBuf, Pin)>,
    /// Dependencies a release does not move.
    pub unmovable: Vec<(PathBuf, Unmovable)>,
}

/// A file with edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEdit {
    /// The file, relative to the project root.
    pub path: PathBuf,
    /// The text before.
    pub before: String,
    /// The text after.
    pub after: String,
    /// The lines that changed.
    pub changes: Vec<Change>,
}

impl Plan {
    /// The version the project is on. The exact pins (the core's crates, the workflow) say which
    /// release it was last on: the oldest of them. A project that has none (commits and branches
    /// only) is placed by its release lines, as far along as the newest of them says. `None` when
    /// no pin names a version at all.
    #[must_use]
    pub fn current(&self) -> Option<Semver> {
        let versions = |exact: bool| {
            self.pins
                .iter()
                .filter(move |(_, p)| p.exact == exact)
                .filter_map(|(_, p)| p.version.clone())
        };
        versions(true).min().or_else(|| versions(false).max())
    }

    /// A pin that names a release newer than the target: the project is ahead of this `undra`.
    ///
    /// A release-line pin (`^1.0.0`, `"1.0"`) is compared with the target's release: a
    /// pre-release `undra` (`1.0.0-rc.1`) writes the line it belongs to, `1.0`, and is not behind
    /// it. An exact pin is compared with the target itself.
    #[must_use]
    pub fn ahead(&self) -> Option<&(PathBuf, Pin)> {
        let release = Semver {
            pre: Vec::new(),
            ..self.target.clone()
        };
        self.pins.iter().find(|(_, p)| {
            p.version.as_ref().is_some_and(|v| {
                if p.exact {
                    *v > self.target
                } else {
                    *v > release
                }
            })
        })
    }

    /// Whether any dependency is on a checkout of the repository.
    #[must_use]
    pub fn on_a_checkout(&self) -> bool {
        self.unmovable.iter().any(|(_, u)| u.why == Why::Path)
    }

    /// How many lines change.
    #[must_use]
    pub fn change_count(&self) -> usize {
        self.files.iter().map(|f| f.changes.len()).sum()
    }
}

/// The files of the project that can hold a pin, relative to `root`, sorted.
#[must_use]
pub fn candidate_files(root: &Path, generated: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, dir: &Path, generated: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 8 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if SKIPPED_DIRS.contains(&name.as_str()) || path == generated {
                    continue;
                }
                walk(root, &path, generated, depth + 1, out);
            } else if is_candidate(root, &path) {
                if let Ok(rel) = path.strip_prefix(root) {
                    out.push(rel.to_path_buf());
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, generated, 0, &mut out);
    out.sort();
    out
}

/// What kind of pin file `rel` (relative to the project root) is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    UndraToml,
    Cargo,
    Package,
    Gradle,
    Pbxproj,
    Workflow,
}

fn kind_of(rel: &Path) -> Option<Kind> {
    let name = rel.file_name()?.to_string_lossy();
    let in_workflows = rel
        .parent()
        .is_some_and(|p| p == Path::new(".github/workflows"));
    match name.as_ref() {
        "undra.toml" if rel.parent().is_some_and(|p| p.as_os_str().is_empty()) => {
            Some(Kind::UndraToml)
        }
        "Cargo.toml" => Some(Kind::Cargo),
        "package.json" => Some(Kind::Package),
        "project.pbxproj" => Some(Kind::Pbxproj),
        n if n.ends_with(".gradle") || n.ends_with(".gradle.kts") => Some(Kind::Gradle),
        n if in_workflows && (n.ends_with(".yml") || n.ends_with(".yaml")) => Some(Kind::Workflow),
        _ => None,
    }
}

fn is_candidate(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).ok().and_then(kind_of).is_some()
}

/// Reads the project's files and plans the edits that move every pin to `target`.
#[must_use]
pub fn plan(root: &Path, generated: &Path, target: &Semver) -> Plan {
    let target_text = Target {
        version: target.clone(),
    };
    let mut plan = Plan {
        target: target.clone(),
        files: Vec::new(),
        pins: Vec::new(),
        unmovable: Vec::new(),
    };
    for rel in candidate_files(root, generated) {
        let Some(kind) = kind_of(&rel) else { continue };
        let Ok(text) = std::fs::read_to_string(root.join(&rel)) else {
            continue;
        };
        let result = match kind {
            Kind::UndraToml => edit_undra_toml(&text, &target_text),
            Kind::Cargo => edit_cargo_toml(&text, &target_text),
            Kind::Package => edit_package_json(&text, &target_text),
            Kind::Gradle => edit_gradle(&text, &target_text),
            Kind::Pbxproj => edit_pbxproj(&text, &target_text),
            Kind::Workflow => edit_workflow(&text, &target_text),
        };
        plan.pins
            .extend(result.pins.into_iter().map(|p| (rel.clone(), p)));
        plan.unmovable
            .extend(result.unmovable.into_iter().map(|u| (rel.clone(), u)));
        if result.after != text {
            plan.files.push(FileEdit {
                path: rel,
                before: text,
                after: result.after,
                changes: result.changes,
            });
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(text: &str) -> Target {
        Target {
            version: Semver::parse(text).unwrap(),
        }
    }

    fn cargo(text: &str) -> FileResult {
        edit_cargo_toml(text, &target("0.2.1"))
    }

    // ----- Cargo.toml

    #[test]
    fn a_git_tag_moves_to_the_release_tag() {
        let r = cargo(
            "[dependencies]\nundra = { git = \"https://github.com/shreypdev/undra\", tag = \"v0.1.0\" }\nserde = \"1\"\n",
        );
        assert_eq!(
            r.after,
            "[dependencies]\nundra = { git = \"https://github.com/shreypdev/undra\", tag = \"v0.2.1\" }\nserde = \"1\"\n"
        );
        assert_eq!(r.changes.len(), 1);
        assert_eq!(r.changes[0].line, 2);
        assert_eq!(r.pins[0].version, Semver::parse("0.1.0"));
        assert_eq!(r.pins[0].shown, "tag v0.1.0");
    }

    #[test]
    fn a_commit_or_branch_becomes_the_release_tag() {
        let rev = cargo(
            "[dependencies]\nundra = { git = \"https://github.com/shreypdev/undra.git\", rev = \"3a4f9c1\", features = [\"x\"] }\n",
        );
        assert!(rev.after.contains("{ git = \"https://github.com/shreypdev/undra.git\", tag = \"v0.2.1\", features = [\"x\"] }"), "{}", rev.after);
        assert_eq!(rev.pins[0].version, None);
        assert_eq!(rev.pins[0].shown, "rev 3a4f9c1");
        let branch = cargo(
            "[dependencies]\nundra = { git = \"https://github.com/shreypdev/undra\", branch = \"main\" }\n",
        );
        assert!(
            branch.after.contains("tag = \"v0.2.1\" }") && !branch.after.contains("branch"),
            "{}",
            branch.after
        );
        let none =
            cargo("[dependencies]\nundra = { git = \"https://github.com/shreypdev/undra\" }\n");
        assert!(
            none.after
                .contains("{ git = \"https://github.com/shreypdev/undra\", tag = \"v0.2.1\" }"),
            "{}",
            none.after
        );
    }

    #[test]
    fn registry_requirements_keep_their_operator() {
        let r = cargo(
            "[dependencies]\nundra = \"0.1\"\nundra-ffi = { version = \"=0.1.0\", features = [\"jni\"] }\nundra-wire = { version = \"^0.1\" }\n",
        );
        assert_eq!(
            r.after,
            "[dependencies]\nundra = \"0.2.1\"\nundra-ffi = { version = \"=0.2.1\", features = [\"jni\"] }\nundra-wire = { version = \"^0.2.1\" }\n"
        );
        assert_eq!(r.changes.len(), 3);
        assert_eq!(r.pins.len(), 3);
    }

    #[test]
    fn a_dependency_already_on_the_release_is_left_byte_for_byte() {
        let text = "[dependencies]\nundra    =    { git = \"https://github.com/shreypdev/undra\",   tag = \"v0.2.1\" }   # pinned\n";
        let r = cargo(text);
        assert_eq!(r.after, text);
        assert!(r.changes.is_empty());
        assert_eq!(r.pins.len(), 1);
    }

    #[test]
    fn comments_features_and_other_dependencies_survive() {
        let text = "# the core\n[package]\nname = \"x\"\nversion = \"0.1.0\"\n\n[dependencies]\n# Undra\nundra = { git = \"https://github.com/shreypdev/undra\", tag = \"v0.1.0\" } # the release\nundra-ffi-extra = \"1\"\nundrawn = \"2\"\nserde = { version = \"1\", features = [\"derive\"] }\n";
        let r = cargo(text);
        assert_eq!(
            r.after,
            text.replace("v0.1.0\" } # the release", "v0.2.1\" } # the release")
        );
        // `[package] version` is not a dependency, and a crate that only starts with the letters is not Undra's.
        assert_eq!(r.pins.len(), 1, "{:?}", r.pins);
    }

    #[test]
    fn dev_build_target_and_workspace_tables_are_read() {
        let text = "[dev-dependencies]\nundra-wire = \"0.1\"\n[build-dependencies]\nundra-meta = \"0.1\"\n[target.'cfg(unix)'.dependencies]\nundra-ffi = \"0.1\"\n[workspace.dependencies]\nundra = { git = \"https://github.com/shreypdev/undra\", tag = \"v0.1.0\" }\n";
        let r = cargo(text);
        assert_eq!(r.changes.len(), 4, "{:?}", r.changes);
        assert!(
            !r.after.contains("0.1\"") && !r.after.contains("v0.1.0"),
            "{}",
            r.after
        );
    }

    #[test]
    fn a_dotted_dependency_section_is_moved_key_by_key() {
        let text = "[dependencies.undra]\ngit = \"https://github.com/shreypdev/undra\"\ntag = \"v0.1.0\"\nfeatures = [\"x\"]\n\n[dependencies.undra-ffi]\ngit = \"https://github.com/shreypdev/undra\"\nrev = \"abc\"\n\n[dependencies.undra-wire]\ngit = \"https://github.com/shreypdev/undra\"\n\n[dependencies.undra-meta]\nversion = \"=0.1.0\"\n";
        let r = cargo(text);
        assert_eq!(
            r.after,
            "[dependencies.undra]\ngit = \"https://github.com/shreypdev/undra\"\ntag = \"v0.2.1\"\nfeatures = [\"x\"]\n\n[dependencies.undra-ffi]\ngit = \"https://github.com/shreypdev/undra\"\ntag = \"v0.2.1\"\n\n[dependencies.undra-wire]\ngit = \"https://github.com/shreypdev/undra\"\ntag = \"v0.2.1\"\n\n[dependencies.undra-meta]\nversion = \"=0.2.1\"\n"
        );
        assert_eq!(r.pins.len(), 4);
    }

    #[test]
    fn paths_and_forks_are_not_moved_and_are_reported() {
        let text = "[dependencies]\nundra = { path = \"../../undra/crates/undra\" }\nundra-ffi = { git = \"https://github.com/someone/undra\", tag = \"v0.1.0\" }\nundra-wire = { workspace = true }\n";
        let r = cargo(text);
        assert_eq!(r.after, text);
        assert_eq!(r.unmovable.len(), 2);
        assert_eq!((r.unmovable[0].why, r.unmovable[0].line), (Why::Path, 2));
        assert_eq!((r.unmovable[1].why, r.unmovable[1].line), (Why::Fork, 3));
        assert!(r.pins.is_empty());
    }

    #[test]
    fn crlf_line_endings_are_kept() {
        let r = cargo("[dependencies]\r\nundra = \"0.1\"\r\n");
        assert_eq!(r.after, "[dependencies]\r\nundra = \"0.2.1\"\r\n");
    }

    #[test]
    fn the_repository_url_is_recognised_in_its_spellings() {
        for url in [
            "https://github.com/shreypdev/undra",
            "https://github.com/shreypdev/undra.git",
            "https://github.com/shreypdev/undra/",
            "https://GitHub.com/ShreyPDev/Undra",
            "http://github.com/shreypdev/undra",
            "git@github.com:shreypdev/undra.git",
            "ssh://git@github.com/shreypdev/undra",
        ] {
            assert!(is_undra_repo(url), "{url}");
        }
        assert!(!is_undra_repo("https://github.com/shreypdev/undra-fork"));
        assert!(!is_undra_repo("https://example.com/undra"));
    }

    // ----- the other files

    #[test]
    fn undra_toml_moves_the_release_line_only() {
        let text = "[project]\nname = \"a\"\nid = \"com.example.a\"\n\n[undra]\n# comment\nversion = \"0.1\"   # the line\n\n[bindings]\nversion = \"keep\"\n";
        let r = edit_undra_toml(text, &target("0.2.1"));
        assert_eq!(
            r.after,
            text.replace(
                "version = \"0.1\"   # the line",
                "version = \"0.2\"   # the line"
            )
        );
        assert_eq!(r.pins[0].version, Semver::parse("0.1"));
        let path = edit_undra_toml(
            "[undra]\nversion = \"0.1\"\npath = \"../undra\"\n",
            &target("0.2.1"),
        );
        assert_eq!(path.unmovable.len(), 1);
    }

    #[test]
    fn package_json_moves_the_runtime_range() {
        let text = "{\n  \"dependencies\": {\n    \"@undra/runtime\": \"^0.1.0\",\n    \"react\": \"^19.0.0\"\n  }\n}\n";
        let r = edit_package_json(text, &target("0.2.1"));
        assert_eq!(r.after, text.replace("^0.1.0", "^0.2.0"));
        assert_eq!(r.pins[0].version, Semver::parse("0.1.0"));
        let local = edit_package_json(
            "{ \"dependencies\": {\n \"@undra/runtime\": \"file:../rt\"\n} }\n",
            &target("0.2.1"),
        );
        assert_eq!(local.unmovable.len(), 1);
        assert!(local.changes.is_empty());
        // Already there: untouched.
        let same = edit_package_json("  \"@undra/runtime\": \"^0.2.0\",\n", &target("0.2.1"));
        assert!(same.changes.is_empty());
    }

    #[test]
    fn gradle_moves_both_coordinates_and_leaves_a_checkout_alone() {
        let text = "dependencies {\n    implementation(\"dev.undra:runtime:0.1.0\")\n    implementation(\"dev.undra:android-adapters:0.1.0\")\n    implementation(\"com.other:lib:1.0\")\n}\n";
        let r = edit_gradle(text, &target("0.2.1"));
        assert_eq!(r.after, text.replace("0.1.0", "0.2.0"));
        assert_eq!(r.changes.len(), 2);
        let groovy = edit_gradle(
            "implementation 'dev.undra:runtime:0.1.0'\n",
            &target("0.2.1"),
        );
        assert_eq!(groovy.after, "implementation 'dev.undra:runtime:0.2.0'\n");
        let snapshot = edit_gradle(
            "implementation(\"dev.undra:runtime:0.1.0-SNAPSHOT\")\n",
            &target("0.2.1"),
        );
        assert!(snapshot.changes.is_empty());
        assert_eq!(snapshot.unmovable[0].why, Why::Path);
    }

    #[test]
    fn pbxproj_moves_the_swift_package_requirement_of_undra_only() {
        let text = "/* Begin XCRemoteSwiftPackageReference section */\n\t\tA1 /* UndraRuntime package */ = {\n\t\t\tisa = XCRemoteSwiftPackageReference;\n\t\t\trepositoryURL = \"https://github.com/shreypdev/undra-swift\";\n\t\t\trequirement = {\n\t\t\t\tkind = upToNextMajorVersion;\n\t\t\t\tminimumVersion = 0.1.0;\n\t\t\t};\n\t\t};\n\t\tA2 /* Other */ = {\n\t\t\tisa = XCRemoteSwiftPackageReference;\n\t\t\trepositoryURL = \"https://github.com/other/pkg\";\n\t\t\trequirement = {\n\t\t\t\tkind = upToNextMajorVersion;\n\t\t\t\tminimumVersion = 1.0.0;\n\t\t\t};\n\t\t};\n/* End XCRemoteSwiftPackageReference section */\n";
        let r = edit_pbxproj(text, &target("0.2.1"));
        assert_eq!(
            r.after,
            text.replace("minimumVersion = 0.1.0;", "minimumVersion = 0.2.0;")
        );
        assert_eq!(r.changes.len(), 1);
        assert_eq!(r.pins[0].version, Semver::parse("0.1.0"));
        let local = edit_pbxproj(
            "\t\t\trelativePath = \"../../runtimes/swift/UndraRuntime\";\n",
            &target("0.2.1"),
        );
        assert_eq!(local.unmovable.len(), 1);
    }

    #[test]
    fn a_workflow_moves_undra_version_and_keeps_its_quotes() {
        let text = "env:\n  UNDRA_VERSION: \"0.1.0\"\n  OTHER: \"0.1.0\"\n";
        let r = edit_workflow(text, &target("0.2.1"));
        assert_eq!(
            r.after,
            "env:\n  UNDRA_VERSION: \"0.2.1\"\n  OTHER: \"0.1.0\"\n"
        );
        let bare = edit_workflow("  UNDRA_VERSION: 0.1.0 # pinned\n", &target("0.2.1"));
        assert_eq!(bare.after, "  UNDRA_VERSION: 0.2.1 # pinned\n");
        let single = edit_workflow("  UNDRA_VERSION: '0.1.0'\n", &target("0.2.1"));
        assert_eq!(single.after, "  UNDRA_VERSION: '0.2.1'\n");
        // A value that is not a version (an expression, a variable) is not touched.
        let expr = edit_workflow("  UNDRA_VERSION: ${{ vars.UNDRA }}\n", &target("0.2.1"));
        assert!(expr.changes.is_empty() && expr.pins.is_empty());
    }

    #[test]
    fn file_kinds_are_recognised_by_name_and_place() {
        for (path, kind) in [
            ("undra.toml", Some(Kind::UndraToml)),
            ("core/Cargo.toml", Some(Kind::Cargo)),
            ("web/package.json", Some(Kind::Package)),
            ("android/app/build.gradle.kts", Some(Kind::Gradle)),
            ("android/settings.gradle", Some(Kind::Gradle)),
            ("ios/App.xcodeproj/project.pbxproj", Some(Kind::Pbxproj)),
            (".github/workflows/undra.yml", Some(Kind::Workflow)),
            (".github/workflows/ci.yaml", Some(Kind::Workflow)),
            ("sub/undra.toml", None),
            ("docs/workflow.yml", None),
            ("README.md", None),
        ] {
            assert_eq!(kind_of(Path::new(path)), kind, "{path}");
        }
    }

    #[test]
    fn the_current_version_is_what_the_exact_pins_say_else_the_newest_release_line() {
        let pin = |version: &str, exact: bool| {
            (
                PathBuf::from("f"),
                Pin {
                    what: "p".into(),
                    version: Semver::parse(version),
                    shown: version.into(),
                    line: 1,
                    exact,
                },
            )
        };
        let mut plan = Plan {
            target: Semver::parse("1.0.0").unwrap(),
            files: Vec::new(),
            pins: vec![pin("0.3.0", false), pin("0.5.2", true), pin("0.4.9", true)],
            unmovable: Vec::new(),
        };
        // The release the crates and the workflow are on, not what the registries' ranges floor at.
        assert_eq!(plan.current(), Semver::parse("0.4.9"));
        plan.pins = vec![pin("0.0.0", false), pin("0.0.5", false)];
        assert_eq!(plan.current(), Semver::parse("0.0.5"));
        plan.pins = vec![(
            PathBuf::from("f"),
            Pin {
                version: None,
                ..pin("0.0.1", true).1
            },
        )];
        assert_eq!(plan.current(), None);
        // Ahead: any pin newer than the target.
        plan.pins = vec![pin("1.0.1", false)];
        assert!(plan.ahead().is_some());
        plan.pins = vec![pin("1.0.0", true)];
        assert!(plan.ahead().is_none());
    }

    #[test]
    fn the_target_spells_each_pin_the_way_init_does() {
        let t = target("1.2.3");
        assert_eq!(
            (t.full().as_str(), t.runtime().as_str(), t.line().as_str()),
            ("1.2.3", "1.2.0", "1.2")
        );
    }

    // ----- look-alikes: only the real pins move (review, 2026-10-01)

    #[test]
    fn a_swift_package_whose_url_merely_contains_undra_is_not_moved() {
        let other = "https://github.com/acme/undra-charts";
        let text = format!(
            "/* Begin XCRemoteSwiftPackageReference section */\n\t\tA1 /* UndraRuntime package */ = {{\n\t\t\tisa = XCRemoteSwiftPackageReference;\n\t\t\trepositoryURL = \"https://github.com/shreypdev/undra-swift\";\n\t\t\trequirement = {{\n\t\t\t\tkind = upToNextMajorVersion;\n\t\t\t\tminimumVersion = 0.1.0;\n\t\t\t}};\n\t\t}};\n\t\tA2 /* Charts */ = {{\n\t\t\tisa = XCRemoteSwiftPackageReference;\n\t\t\trepositoryURL = \"{other}\";\n\t\t\trequirement = {{\n\t\t\t\tkind = exactVersion;\n\t\t\t\tversion = 3.4.5;\n\t\t\t}};\n\t\t}};\n/* End XCRemoteSwiftPackageReference section */\n"
        );
        let r = edit_pbxproj(&text, &target("0.2.1"));
        assert_eq!(
            r.after,
            text.replace("minimumVersion = 0.1.0;", "minimumVersion = 0.2.0;"),
            "only Undra's package moves; {other} keeps 3.4.5"
        );
        assert_eq!(r.pins.len(), 1, "{:?}", r.pins);
        // Xcode's other spellings of the same package are Undra's.
        for url in [
            "https://github.com/shreypdev/undra-swift.git",
            "git@github.com:shreypdev/undra-swift.git",
            "https://github.com/ShreyPDev/Undra-Swift/",
        ] {
            let r = edit_pbxproj(
                &text.replace("https://github.com/shreypdev/undra-swift", url),
                &target("0.2.1"),
            );
            assert_eq!(r.pins.len(), 1, "{url}");
        }
    }

    #[test]
    fn gradle_moves_literal_versions_only_and_never_a_comment() {
        // A variable, a dynamic version and a comment are not pins `undra init` writes: they are
        // left as they are, and a variable is reported so the author moves it.
        let text = "dependencies {\n    // was dev.undra:runtime:0.0.1 before the rename\n    implementation(\"dev.undra:runtime:$undraVersion\")\n    implementation(\"dev.undra:android-adapters:${undraVersion}\")\n    implementation(\"dev.undra:runtime-extras:0.1.0\")\n    testImplementation(\"dev.undra:runtime:+\")\n    /* dev.undra:runtime:0.0.2 */\n    implementation(\"dev.undra:runtime:0.1.0\") // dev.undra:android-adapters:0.0.3\n}\n";
        let r = edit_gradle(text, &target("0.2.1"));
        assert_eq!(
            r.after,
            text.replace(
                "implementation(\"dev.undra:runtime:0.1.0\")",
                "implementation(\"dev.undra:runtime:0.2.0\")"
            ),
            "{}",
            r.after
        );
        assert_eq!(r.pins.len(), 1, "{:?}", r.pins);
        assert_eq!(
            r.unmovable
                .iter()
                .map(|u| (u.line, u.why))
                .collect::<Vec<_>>(),
            [
                (3, Why::NotALiteral),
                (4, Why::NotALiteral),
                (6, Why::NotALiteral)
            ]
        );
    }

    #[test]
    fn package_json_moves_the_runtime_and_react_native_and_nothing_that_looks_like_them() {
        let text = "{\n  \"description\": \"needs \\\"@undra/runtime\\\": ^0.0.1\",\n  \"dependencies\": {\n    \"@undra/runtime-extras\": \"^0.1.0\",\n    \"@undra/react-native\": \"^0.1.0\",\n    \"react\": \"^19.0.0\"\n  },\n  \"devDependencies\": {\n    \"@undra/runtime\": \"^0.1.0\"\n  }\n}\n";
        let r = edit_package_json(text, &target("0.2.1"));
        assert_eq!(
            r.after,
            text.replace(
                "\"@undra/react-native\": \"^0.1.0\"",
                "\"@undra/react-native\": \"^0.2.0\""
            )
            .replace(
                "\"@undra/runtime\": \"^0.1.0\"",
                "\"@undra/runtime\": \"^0.2.0\""
            ),
            "{}",
            r.after
        );
        assert!(r.after.contains("\"@undra/runtime-extras\": \"^0.1.0\""));
        assert_eq!(r.pins.len(), 2, "{:?}", r.pins);
        let local = edit_package_json(
            "  \"@undra/react-native\": \"file:../../runtimes/rn/@undra/react-native\",\n",
            &target("0.2.1"),
        );
        assert_eq!(local.unmovable.len(), 1);
    }

    #[test]
    fn cargo_look_alikes_are_left_alone() {
        let text = "[dependencies]\n# undra = { git = \"https://github.com/shreypdev/undra\", tag = \"v0.0.1\" }\nundra-something = { git = \"https://github.com/shreypdev/undra\", tag = \"v0.0.9\" }\nundra = { git = \"https://github.com/shreypdev/undra\", tag = \"v0.1.0\" }\n\n[dependencies.undra-ffi]\n# tag = \"v0.0.2\"\ngit = \"https://github.com/shreypdev/undra\"\ntag = \"v0.1.0\" # the release\n";
        let r = cargo(text);
        assert_eq!(
            r.after,
            text.replace(
                "undra = { git = \"https://github.com/shreypdev/undra\", tag = \"v0.1.0\" }",
                "undra = { git = \"https://github.com/shreypdev/undra\", tag = \"v0.2.1\" }"
            )
            .replace(
                "tag = \"v0.1.0\" # the release",
                "tag = \"v0.2.1\" # the release"
            ),
            "{}",
            r.after
        );
        assert_eq!(r.pins.len(), 2, "{:?}", r.pins);
    }

    #[test]
    fn every_editor_keeps_crlf_line_endings() {
        let t = target("0.2.1");
        let crlf = |s: &str| s.replace('\n', "\r\n");
        type Editor = fn(&str, &Target) -> FileResult;
        let cases: [(&str, Editor, &str, &str); 6] = [
            (
                "Cargo.toml",
                edit_cargo_toml,
                "[dependencies.undra]\ngit = \"https://github.com/shreypdev/undra\"\nrev = \"abc\"\n",
                "[dependencies.undra]\ngit = \"https://github.com/shreypdev/undra\"\ntag = \"v0.2.1\"\n",
            ),
            (
                "undra.toml",
                edit_undra_toml,
                "[undra]\nversion = \"0.1\"\n",
                "[undra]\nversion = \"0.2\"\n",
            ),
            (
                "package.json",
                edit_package_json,
                "{\n  \"@undra/runtime\": \"^0.1.0\"\n}\n",
                "{\n  \"@undra/runtime\": \"^0.2.0\"\n}\n",
            ),
            (
                "build.gradle",
                edit_gradle,
                "implementation 'dev.undra:runtime:0.1.0'\n",
                "implementation 'dev.undra:runtime:0.2.0'\n",
            ),
            (
                "project.pbxproj",
                edit_pbxproj,
                "\t\tA1 = {\n\t\t\tisa = XCRemoteSwiftPackageReference;\n\t\t\trepositoryURL = \"https://github.com/shreypdev/undra-swift\";\n\t\t\trequirement = {\n\t\t\t\tminimumVersion = 0.1.0;\n\t\t\t};\n\t\t};\n",
                "\t\tA1 = {\n\t\t\tisa = XCRemoteSwiftPackageReference;\n\t\t\trepositoryURL = \"https://github.com/shreypdev/undra-swift\";\n\t\t\trequirement = {\n\t\t\t\tminimumVersion = 0.2.0;\n\t\t\t};\n\t\t};\n",
            ),
            (
                "undra.yml",
                edit_workflow,
                "env:\n  UNDRA_VERSION: \"0.1.0\"\n",
                "env:\n  UNDRA_VERSION: \"0.2.1\"\n",
            ),
        ];
        for (name, edit, before, after) in cases {
            let r = edit(&crlf(before), &t);
            assert_eq!(r.after, crlf(after), "{name}");
            assert_eq!(r.changes.len(), 1, "{name}");
        }
    }

    #[test]
    fn a_prerelease_undra_is_not_behind_the_release_line_it_writes() {
        // `undra init` from 1.0.0-rc.1 writes the release line 1.0 (`^1.0.0`, `1.0.0`, "1.0") and the
        // exact pins 1.0.0-rc.1; the same `undra upgrade` must not call that project "ahead".
        let rc = Semver::parse("1.0.0-rc.1").unwrap();
        let pin = |version: &str, exact: bool| {
            (
                PathBuf::from("f"),
                Pin {
                    what: "p".into(),
                    version: Semver::parse(version),
                    shown: version.into(),
                    line: 1,
                    exact,
                },
            )
        };
        let plan = Plan {
            target: rc.clone(),
            files: Vec::new(),
            pins: vec![
                pin("1.0.0", false),
                pin("1.0", false),
                pin("1.0.0-rc.1", true),
            ],
            unmovable: Vec::new(),
        };
        assert!(plan.ahead().is_none(), "{:?}", plan.ahead());
        // A project on the final release is ahead of its release candidate, and the next line is ahead.
        for (version, exact) in [("1.0.0", true), ("1.1.0", false), ("1.0.1", false)] {
            let plan = Plan {
                pins: vec![pin(version, exact)],
                ..plan.clone()
            };
            assert!(plan.ahead().is_some(), "{version} exact={exact}");
        }
    }
}
