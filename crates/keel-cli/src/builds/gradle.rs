//! The Gradle app's side of the Android build: where does it look for `libkeel_core.so`?
//!
//! `keel build --platform android` writes `build/android/jniLibs/<abi>/libkeel_core.so`; the app
//! packages whatever its module's `sourceSets { ... jniLibs.srcDir(...) }` names. The two are
//! written in different places (keel.toml's `[paths] build`, a path in `app/build.gradle.kts`
//! that is relative to the *module*, not the project), and they drift: when they do, Gradle
//! ignores the missing directory without a word, the APK ships no core, and the app dies at
//! start with an `UnsatisfiedLinkError`. After a build the CLI therefore reads the app's script,
//! and either says nothing (the app packages the directory), puts the libraries where the script
//! looks (inside the project), or says what to change.
//!
//! The reading is deliberately line-level, like [`crate::detect`]: it looks for `jniLibs` with
//! `srcDir` / `srcDirs` and quoted paths, and gives up quietly (`Outcome::Unknown`) on anything it
//! cannot resolve (a variable, `$rootDir/...`) instead of guessing.

use std::path::{Component, Path, PathBuf};

use crate::detect;
use crate::error::Result;
use crate::fsutil::copy_file;
use crate::names::{portable, relative_path};

/// What the Gradle app of a project does with the libraries of a build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// There is no Android app in the project that could be read: nothing to check.
    NoApp,
    /// The app packages the directory the build wrote.
    Packaged,
    /// The script names its libraries in a way that cannot be resolved here.
    Unknown,
    /// The script packages another directory of the project; the libraries were copied there too.
    Copied {
        /// The Gradle script.
        script: PathBuf,
        /// The directories it names, which now hold the libraries.
        to: Vec<PathBuf>,
        /// The `jniLibs.srcDir` line that would make the copy unnecessary.
        fix: String,
    },
    /// The script names no `jniLibs` directory at all: the app ships without the core.
    NotPackaged {
        /// The Gradle script.
        script: PathBuf,
        /// The `jniLibs.srcDir` line to add.
        fix: String,
    },
    /// The script names directories outside the project; nothing was written there.
    Elsewhere {
        /// The Gradle script.
        script: PathBuf,
        /// The directories it names.
        declared: Vec<PathBuf>,
        /// The `jniLibs.srcDir` line that points at the build output.
        fix: String,
    },
}

impl Outcome {
    /// What to tell the developer, if anything.
    #[must_use]
    pub fn message(&self, project_root: &Path, expected: &Path) -> Option<String> {
        let shown = |p: &Path| {
            p.strip_prefix(project_root)
                .unwrap_or(p)
                .display()
                .to_string()
        };
        let built = shown(expected);
        match self {
            Outcome::NoApp | Outcome::Packaged | Outcome::Unknown => None,
            Outcome::Copied { script, to, fix } => Some(format!(
                "{} packages native libraries from {}, not from {built} where the build writes them; copied the libraries there too so the app has them. Point it at the build output to drop the copy: {fix}",
                shown(script),
                to.iter().map(|d| shown(d)).collect::<Vec<_>>().join(", "),
            )),
            Outcome::NotPackaged { script, fix } => Some(format!(
                "{} does not package {built}, so the app would ship without libkeel_core.so (Gradle ignores a missing jniLibs directory and the app fails at start with UnsatisfiedLinkError). Inside `android {{ }}` add: {fix}",
                shown(script),
            )),
            Outcome::Elsewhere {
                script,
                declared,
                fix,
            } => Some(format!(
                "{} packages native libraries from {}, outside the project, so the libraries in {built} are not packaged. Point it at the build output: {fix}",
                shown(script),
                declared
                    .iter()
                    .map(|d| d.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            )),
        }
    }
}

/// What a Gradle script says about `jniLibs`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Declared {
    /// Whether any `jniLibs ... srcDir` was found.
    pub any: bool,
    /// The directories it names, resolved and normalized.
    pub dirs: Vec<PathBuf>,
    /// Whether some `srcDir` could not be resolved (a variable, an interpolated path).
    pub opaque: bool,
}

/// The directories `script` names for native libraries.
///
/// `module` is the directory of the app module (plain paths are relative to it), `gradle_root`
/// the directory of `settings.gradle(.kts)` (what `rootProject.file(...)` is relative to).
#[must_use]
pub fn declared_jni_dirs(script: &str, module: &Path, gradle_root: &Path) -> Declared {
    let mut declared = Declared::default();
    // Greater than zero while inside a `jniLibs { ... }` block, whose lines are read as well.
    let mut block_depth: i32 = 0;
    for line in script.lines() {
        let code = line.split("//").next().unwrap_or(line);
        let relevant = if block_depth > 0 {
            Some(code)
        } else {
            code.find("jniLibs").map(|at| &code[at..])
        };
        let call = relevant.and_then(|r| r.find("srcDir").map(|at| &r[at..]));
        if let Some(call) = call {
            declared.any = true;
            let base = if code.contains("rootProject") {
                gradle_root
            } else {
                module
            };
            let literals = quoted(call);
            if literals.is_empty() || literals.iter().any(|l| l.contains('$')) {
                declared.opaque = true;
            }
            for literal in literals.iter().filter(|l| !l.contains('$')) {
                declared.dirs.push(normalize(&base.join(literal)));
            }
        }
        if block_depth > 0 {
            block_depth = (block_depth + braces(code)).max(0);
        } else if let Some(at) = code.find("jniLibs") {
            block_depth = braces(&code[at..]).max(0);
        }
    }
    declared.dirs.dedup();
    declared
}

/// Opening minus closing braces of `text`.
fn braces(text: &str) -> i32 {
    text.chars().fold(0, |depth, c| match c {
        '{' => depth + 1,
        '}' => depth - 1,
        _ => depth,
    })
}

/// The string literals (`"..."` or `'...'`) of `text`.
fn quoted(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '"' || c == '\'' {
            let literal: String = chars.by_ref().take_while(|n| *n != c).collect();
            out.push(literal);
        }
    }
    out
}

/// `path` with `.` and `..` resolved by reading it, without touching the file system.
#[must_use]
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn same_dir(a: &Path, b: &Path) -> bool {
    normalize(a) == normalize(b)
        || matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

/// The line that makes the app at `script` package `expected`.
fn fix_line(script: &Path, module: &Path, expected: &Path) -> String {
    let rel = portable(&relative_path(module, expected).unwrap_or_else(|| expected.to_path_buf()));
    if script.extension().is_some_and(|e| e == "kts") {
        format!("sourceSets {{ getByName(\"main\").jniLibs.srcDir(\"{rel}\") }}")
    } else {
        format!("sourceSets {{ main {{ jniLibs.srcDir '{rel}' }} }}")
    }
}

/// Checks the project's Gradle app against the directory `expected` (`build/android/jniLibs`),
/// which holds `<abi>/libkeel_core.so` for each of `abis`, and puts the libraries where the app
/// looks when that is another directory inside the project.
///
/// # Errors
///
/// `C0010` when a library cannot be copied.
pub fn reconcile(project_root: &Path, expected: &Path, abis: &[String]) -> Result<Outcome> {
    let Some(app) = detect::detect(project_root).android else {
        return Ok(Outcome::NoApp);
    };
    let Some(module) = app.app_module.clone() else {
        return Ok(Outcome::NoApp);
    };
    let script_name = if app.kotlin_dsl {
        "build.gradle.kts"
    } else {
        "build.gradle"
    };
    let script = module.join(script_name);
    let Ok(text) = std::fs::read_to_string(&script) else {
        return Ok(Outcome::NoApp);
    };
    let declared = declared_jni_dirs(&text, &module, &app.root);
    let fix = fix_line(&script, &module, expected);
    if declared.dirs.iter().any(|d| same_dir(d, expected)) {
        return Ok(Outcome::Packaged);
    }
    if declared.opaque {
        return Ok(Outcome::Unknown); // a variable may well name the build output
    }
    if !declared.any {
        return Ok(Outcome::NotPackaged { script, fix });
    }
    let (inside, outside): (Vec<PathBuf>, Vec<PathBuf>) = declared
        .dirs
        .into_iter()
        .partition(|d| d.starts_with(project_root));
    if !inside.is_empty() {
        for dir in &inside {
            for abi in abis {
                copy_file(
                    &expected.join(abi).join("libkeel_core.so"),
                    &dir.join(abi).join("libkeel_core.so"),
                )?;
            }
        }
        return Ok(Outcome::Copied {
            script,
            to: inside,
            fix,
        });
    }
    Ok(Outcome::Elsewhere {
        script,
        declared: outside,
        fix,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::unique_temp_dir;

    const MODULE: &str = "/p/android/app";
    const ROOT: &str = "/p/android";

    fn dirs(script: &str) -> Declared {
        declared_jni_dirs(script, Path::new(MODULE), Path::new(ROOT))
    }

    #[test]
    fn the_template_line_is_relative_to_the_module() {
        let d = dirs(
            "    sourceSets {\n        getByName(\"main\").jniLibs.srcDir(\"../../build/android/jniLibs\")\n    }\n",
        );
        assert!(d.any && !d.opaque);
        assert_eq!(d.dirs, [PathBuf::from("/p/build/android/jniLibs")]);
    }

    #[test]
    fn rootproject_file_is_relative_to_the_gradle_root() {
        let d = dirs(
            "getByName(\"main\").jniLibs.srcDir(rootProject.file(\"../build/android/jniLibs\"))\n",
        );
        assert_eq!(d.dirs, [PathBuf::from("/p/build/android/jniLibs")]);
    }

    #[test]
    fn groovy_and_several_directories_are_read() {
        let d = dirs("sourceSets { main { jniLibs.srcDirs = ['libs', \"../native\"] } }\n");
        assert_eq!(
            d.dirs,
            [
                PathBuf::from("/p/android/app/libs"),
                PathBuf::from("/p/android/native")
            ]
        );
        let d = dirs("    sourceSets.main.jniLibs.srcDir '../../build/android/jniLibs'\n");
        assert_eq!(d.dirs, [PathBuf::from("/p/build/android/jniLibs")]);
    }

    #[test]
    fn a_jnilibs_block_is_read_across_lines() {
        let d = dirs(
            "sourceSets {\n  getByName(\"main\") {\n    jniLibs {\n      srcDir(\"../../build/android/jniLibs\")\n    }\n    java.srcDir(\"elsewhere\")\n  }\n}\n",
        );
        assert_eq!(
            d.dirs,
            [PathBuf::from("/p/build/android/jniLibs")],
            "only the lines inside the jniLibs block count"
        );
    }

    #[test]
    fn what_cannot_be_resolved_is_marked_not_guessed() {
        let d = dirs("jniLibs.srcDir(\"$rootDir/../build/android/jniLibs\")\n");
        assert!(d.any && d.opaque && d.dirs.is_empty(), "{d:?}");
        let d = dirs("jniLibs.srcDir(keelLibs)\n");
        assert!(d.any && d.opaque && d.dirs.is_empty(), "{d:?}");
        let d = dirs("// jniLibs.srcDir(\"old\")\nandroid { }\n");
        assert!(!d.any, "a commented-out line does not count");
    }

    #[test]
    fn paths_are_normalized_without_the_file_system() {
        assert_eq!(normalize(Path::new("/a/b/../c/./d")), Path::new("/a/c/d"));
        assert_eq!(normalize(Path::new("/../a")), Path::new("/a"));
        assert_eq!(normalize(Path::new("a/../../b")), Path::new("../b"));
    }

    fn project(script: &str) -> (PathBuf, PathBuf) {
        let root = unique_temp_dir("gradle-wiring");
        let write = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("android/settings.gradle.kts", "include(\":app\")\n");
        write(
            "android/app/build.gradle.kts",
            &format!("plugins {{ id(\"com.android.application\") }}\nandroid {{\n{script}\n}}\n"),
        );
        let expected = root.join("build/android/jniLibs");
        for abi in ["arm64-v8a", "x86_64"] {
            write(&format!("build/android/jniLibs/{abi}/libkeel_core.so"), abi);
        }
        (root, expected)
    }

    fn abis() -> Vec<String> {
        vec!["arm64-v8a".into(), "x86_64".into()]
    }

    #[test]
    fn an_app_that_packages_the_build_output_is_left_alone() {
        let (root, expected) = project(
            "sourceSets { getByName(\"main\").jniLibs.srcDir(\"../../build/android/jniLibs\") }",
        );
        let outcome = reconcile(&root, &expected, &abis()).unwrap();
        assert_eq!(outcome, Outcome::Packaged);
        assert_eq!(outcome.message(&root, &expected), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_path_relative_to_the_gradle_root_instead_of_the_module_is_served() {
        // Relative to the Gradle root (`android/`) instead of the module (`android/app`): one
        // level short, so it names `android/build/android/jniLibs`, which nothing writes to.
        let (root, expected) = project(
            "sourceSets { getByName(\"main\").jniLibs.srcDir(\"../build/android/jniLibs\") }",
        );
        let outcome = reconcile(&root, &expected, &abis()).unwrap();
        let Outcome::Copied { to, fix, .. } = &outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(
            to,
            &[root.join("android/build/android/jniLibs")],
            "a directory inside the project that the app does look in"
        );
        assert!(
            fix.contains("jniLibs.srcDir(\"../../build/android/jniLibs\")"),
            "{fix}"
        );
        for abi in ["arm64-v8a", "x86_64"] {
            let copied = std::fs::read_to_string(to[0].join(abi).join("libkeel_core.so")).unwrap();
            assert_eq!(copied, abi);
        }
        let message = outcome.message(&root, &expected).unwrap();
        assert!(
            message.contains("android/app/build.gradle.kts")
                && message.contains("build/android/jniLibs")
                && message.contains("copied"),
            "{message}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_app_that_names_no_directory_is_told_what_to_add() {
        let (root, expected) = project("namespace = \"x\"");
        let outcome = reconcile(&root, &expected, &abis()).unwrap();
        assert!(
            matches!(outcome, Outcome::NotPackaged { .. }),
            "{outcome:?}"
        );
        let message = outcome.message(&root, &expected).unwrap();
        assert!(
            message.contains("UnsatisfiedLinkError")
                && message.contains("sourceSets { getByName(\"main\").jniLibs.srcDir(\"../../build/android/jniLibs\") }"),
            "{message}"
        );
        assert!(
            !root.join("android/app/src").exists(),
            "nothing is written when the app names no directory"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_directory_outside_the_project_is_reported_not_written() {
        let (root, expected) = project(
            "sourceSets { getByName(\"main\").jniLibs.srcDir(\"../../../somewhere/else\") }",
        );
        let outcome = reconcile(&root, &expected, &abis()).unwrap();
        assert!(matches!(outcome, Outcome::Elsewhere { .. }), "{outcome:?}");
        assert!(
            outcome
                .message(&root, &expected)
                .unwrap()
                .contains("outside the project")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_path_built_from_variables_is_not_second_guessed() {
        let (root, expected) =
            project("sourceSets { getByName(\"main\").jniLibs.srcDir(keelLibs) }");
        assert_eq!(
            reconcile(&root, &expected, &abis()).unwrap(),
            Outcome::Unknown
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_project_without_an_android_app_has_nothing_to_check() {
        let root = unique_temp_dir("gradle-none");
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(
            reconcile(&root, &root.join("build/android/jniLibs"), &abis()).unwrap(),
            Outcome::NoApp
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
