//! Names derived from the project name, and path helpers.
//!
//! `undra init todo-app` needs a crate name (`todo-app-core`), a library name (`todo_app_core`),
//! an iOS app name (`TodoApp`), an application id (`com.example.todoapp`) and a Swift module
//! (`TodoAppCore`). They are all computed here, in one place, so the scaffold, `undra.toml` and the
//! bindings agree.

use std::path::{Component, Path, PathBuf};

use crate::error::{CliError, Code, Result};

/// Words that cannot be a project name because the derived crate name would be reserved or
/// would shadow something the generated code names.
const RESERVED: &[&str] = &[
    "test",
    "undra",
    "std",
    "core",
    "alloc",
    "proc-macro",
    "proc_macro",
    "self",
    "super",
    "crate",
    "build",
    "target",
];

/// The names derived from a project name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Names {
    /// The project name as given (`todo-app`).
    pub project: String,
    /// `kebab-case` (`todo-app`).
    pub kebab: String,
    /// `snake_case` (`todo_app`).
    pub snake: String,
    /// `PascalCase` (`TodoApp`).
    pub pascal: String,
    /// The core's Cargo package name (`todo-app-core`).
    pub core_package: String,
    /// The core's Rust library name (`todo_app_core`).
    pub core_lib: String,
    /// The Swift module of the bindings (`TodoAppCore`).
    pub swift_module: String,
}

impl Names {
    /// Derives every name from `project`, which must satisfy [`validate_project_name`]:
    /// `todo-app` gives the package `todo-app-core`, the library `todo_app_core` and the Swift
    /// module `TodoAppCore`.
    #[must_use]
    pub fn derive(project: &str) -> Names {
        let kebab = project.replace('_', "-").to_ascii_lowercase();
        let snake = kebab.replace('-', "_");
        let pascal = pascal(&kebab);
        let core_package = format!("{kebab}-core");
        Names {
            project: project.to_owned(),
            core_lib: format!("{snake}_core"),
            swift_module: format!("{pascal}Core"),
            kebab,
            snake,
            pascal,
            core_package,
        }
    }

    /// The default application id: `com.example.<name without separators>`.
    #[must_use]
    pub fn default_app_id(&self) -> String {
        format!("com.example.{}", self.snake.replace('_', ""))
    }
}

/// `todo-app` to `TodoApp`.
#[must_use]
pub fn pascal(name: &str) -> String {
    let mut out = String::new();
    for word in name.split(['-', '_', ' ', '.']).filter(|w| !w.is_empty()) {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

/// Checks that `name` can be a project name.
///
/// # Errors
///
/// Explains what is wrong and suggests a name that would do.
pub fn validate_project_name(name: &str) -> Result<()> {
    let problem = if name.is_empty() {
        Some("it is empty".to_owned())
    } else if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Some("it contains characters other than ASCII letters, digits, `-` and `_`".to_owned())
    } else if !name.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        Some("it does not start with a letter".to_owned())
    } else if RESERVED.contains(&name.to_ascii_lowercase().as_str()) {
        Some(format!(
            "`{name}` is reserved (it would clash with a Rust or Undra crate name)"
        ))
    } else if name.len() > 48 {
        Some("it is longer than 48 characters".to_owned())
    } else {
        None
    };
    match problem {
        None => Ok(()),
        Some(problem) => Err(CliError::new(
            Code::BadArgument,
            format!("`{name}` cannot be a project name: {problem}"),
            "the name becomes a Cargo package (`<name>-core`), a Swift module and a Kotlin package, which are stricter than file names",
            format!(
                "choose a name like `todo-app` or `acme_notes`\nyou can still call the folder anything: `undra init {} --dir <parent>`",
                suggest(name)
            ),
        )),
    }
}

/// Turns something like `My App!` into a name that would pass [`validate_project_name`].
#[must_use]
pub fn suggest(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_owned();
    let out = out
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .to_owned();
    if out.is_empty() || RESERVED.contains(&out.as_str()) {
        "my-app".to_owned()
    } else {
        out
    }
}

/// Checks an application id (`com.example.todo`): reverse-DNS segments that are valid in an
/// Android application id and an iOS bundle identifier.
///
/// # Errors
///
/// Explains what is wrong.
pub fn validate_app_id(id: &str) -> Result<()> {
    let segments: Vec<&str> = id.split('.').collect();
    let ok = segments.len() >= 2
        && segments.iter().all(|s| {
            let mut chars = s.chars();
            chars.next().is_some_and(|c| c.is_ascii_lowercase())
                && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        });
    if ok {
        Ok(())
    } else {
        Err(CliError::bad_argument(
            format!("`{id}` is not a valid application id"),
            "the id is an Android applicationId and an iOS bundle identifier and the Kotlin package of the bindings: at least two dot-separated lowercase segments that start with a letter",
            "use something like `com.example.todo`",
        ))
    }
}

/// The path from directory `from` to `to`, both absolute, using `..` where needed. `None` when
/// they have no common root (different drives).
#[must_use]
pub fn relative_path(from: &Path, to: &Path) -> Option<PathBuf> {
    let from: Vec<Component<'_>> = from.components().collect();
    let to: Vec<Component<'_>> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return None;
    }
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for component in &to[common..] {
        out.push(component.as_os_str());
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    Some(out)
}

/// `path` with `/` separators, for files that are read on every platform (`Package.swift`,
/// `settings.gradle.kts`, `package.json`).
#[must_use]
pub fn portable(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_every_name() {
        let n = Names::derive("todo-app");
        assert_eq!(n.kebab, "todo-app");
        assert_eq!(n.snake, "todo_app");
        assert_eq!(n.pascal, "TodoApp");
        assert_eq!(n.core_package, "todo-app-core");
        assert_eq!(n.core_lib, "todo_app_core");
        assert_eq!(n.swift_module, "TodoAppCore");
        assert_eq!(n.default_app_id(), "com.example.todoapp");
        assert_eq!(Names::derive("Acme_Notes").kebab, "acme-notes");
    }

    #[test]
    fn swift_module_matches_the_bindgen_default() {
        let n = Names::derive("todo-app");
        assert_eq!(
            undra_bindgen::naming::pascal(&n.core_package),
            n.swift_module
        );
    }

    #[test]
    fn project_names_are_validated() {
        for ok in ["todo", "todo-app", "acme_notes", "A1"] {
            validate_project_name(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for bad in ["", "1app", "my app", "todo!", "undra", "test", "-x", "é"] {
            let e = validate_project_name(bad).unwrap_err();
            assert_eq!(e.code, Code::BadArgument, "{bad}");
        }
    }

    #[test]
    fn suggestions_are_valid_names() {
        for input in ["My App!", "1st thing", "  ", "undra", "Ünï", "already-ok"] {
            let s = suggest(input);
            validate_project_name(&s).unwrap_or_else(|e| panic!("{input:?} -> {s:?}: {e}"));
        }
        assert_eq!(suggest("My App!"), "my-app");
    }

    #[test]
    fn app_ids_are_validated() {
        validate_app_id("com.example.todo").unwrap();
        validate_app_id("dev.undra.playground").unwrap();
        for bad in ["todo", "com..x", "Com.example", "com.1x", "com.ex-ample"] {
            assert!(validate_app_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn relative_paths() {
        let r = |a: &str, b: &str| relative_path(Path::new(a), Path::new(b)).map(|p| portable(&p));
        assert_eq!(r("/a/b/c", "/a/b/c"), Some(".".into()));
        assert_eq!(r("/a/b/c", "/a/b/d/e"), Some("../d/e".into()));
        assert_eq!(r("/a/b", "/a/b/c/d"), Some("c/d".into()));
        assert_eq!(r("/x/y/z", "/a"), Some("../../../a".into()));
    }
}
