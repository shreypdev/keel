//! `undra bindgen`: the schema in, the Swift, Kotlin and TypeScript bindings out.
//!
//! The work is [`plan_files`], a pure function from a schema and a [`Plan`] to the files of the
//! three trees, so it is tested without a build. [`apply`] writes them and removes the files an
//! earlier run wrote that no longer exist (tracked in `.undra-generated`, so nothing the user put
//! next to them is ever deleted); [`check`] compares instead of writing, for CI.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use undra_bindgen::{BindgenError, GeneratedFile, Generator};
use undra_meta::Schema;

use crate::config::Platform;
use crate::error::{CliError, Code, Result};
use crate::fsutil;
use crate::names::{portable, relative_path};
use crate::runtimes::{RuntimeRef, Runtimes};

/// The file listing what a previous run wrote, inside the output directory.
pub const MANIFEST: &str = ".undra-generated";

/// Everything that decides what is generated besides the schema.
#[derive(Clone, Debug)]
pub struct Plan {
    /// The generator configuration (module, package and scope names).
    pub generator: Generator,
    /// The languages to generate: iOS is Swift, Android is Kotlin, the web is TypeScript.
    pub platforms: Vec<Platform>,
    /// Where the runtimes are.
    pub runtimes: Runtimes,
    /// The output directory, absolute with symlinks resolved (relative paths to the runtimes are
    /// computed from it).
    pub out: PathBuf,
}

impl Plan {
    /// The Swift package directory (`<out>/swift`).
    #[must_use]
    pub fn swift_dir(&self) -> PathBuf {
        self.out.join("swift")
    }
}

/// Canonicalizes `path` even when its tail does not exist yet: the longest existing ancestor is
/// resolved and the rest appended.
#[must_use]
pub fn canonicalize_lenient(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut existing = absolute.as_path();
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if let Ok(resolved) = existing.canonicalize() {
            let mut out = resolved;
            for part in tail.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                existing = parent;
            }
            _ => return absolute,
        }
    }
}

/// Generates every file of the trees for `plan.platforms`, paths relative to `plan.out`.
///
/// # Errors
///
/// `C0007` with bindgen's own diagnostics when the schema cannot be turned into bindings.
pub fn plan_files(schema: &Schema, plan: &Plan) -> Result<Vec<GeneratedFile>> {
    let mut files = Vec::new();
    let prefixed = |dir: &str, generated: Vec<GeneratedFile>| -> Vec<GeneratedFile> {
        generated
            .into_iter()
            .map(|f| GeneratedFile {
                path: format!("{dir}/{}", f.path),
                contents: f.contents,
            })
            .collect()
    };
    if plan.platforms.contains(&Platform::Ios) {
        files.extend(prefixed(
            "swift",
            plan.generator.swift(schema).map_err(bindgen_failure)?,
        ));
        files.push(GeneratedFile {
            path: "swift/Package.swift".into(),
            contents: swift_package(
                &plan.generator.swift_module,
                &plan.runtimes.swift,
                &plan.swift_dir(),
            ),
        });
    }
    if plan.platforms.contains(&Platform::Android) {
        files.extend(prefixed(
            "kotlin",
            plan.generator.kotlin(schema).map_err(bindgen_failure)?,
        ));
        files.push(GeneratedFile {
            path: "kotlin/build.gradle.kts".into(),
            contents: kotlin_build(&plan.runtimes.kotlin),
        });
        files.push(GeneratedFile {
            path: "kotlin/.gitignore".into(),
            contents: KOTLIN_GITIGNORE.into(),
        });
    }
    if plan.platforms.contains(&Platform::Web) {
        files.extend(prefixed(
            "ts",
            plan.generator.typescript(schema).map_err(bindgen_failure)?,
        ));
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn bindgen_failure(errors: Vec<BindgenError>) -> CliError {
    let detail = errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    CliError::new(
        Code::Bindgen,
        format!(
            "the schema cannot be turned into bindings ({} problem{})",
            errors.len(),
            if errors.len() == 1 { "" } else { "s" }
        ),
        "the public surface of the core has to be expressible in Swift, Kotlin and TypeScript, and these items are not",
        "change the items named below in the core (each message says what to do), then run `undra bindgen` again",
    )
    .with_detail(detail)
}

/// `Package.swift` of the generated Swift package: the bindings as a library that depends on the
/// `UndraRuntime` package.
#[must_use]
pub fn swift_package(module: &str, runtime: &RuntimeRef, package_dir: &Path) -> String {
    let (dependency, package_id) = match runtime {
        RuntimeRef::Path(dir) => {
            let rel = relative_path(package_dir, dir).unwrap_or_else(|| dir.clone());
            (
                format!(".package(path: \"{}\")", portable(&rel)),
                dir.file_name().map_or_else(
                    || "UndraRuntime".to_owned(),
                    |n| n.to_string_lossy().into_owned(),
                ),
            )
        }
        RuntimeRef::Registry { version } => (
            format!(
                ".package(url: \"https://github.com/shreypdev/undra-swift\", from: \"{}.0\")",
                version_base(version)
            ),
            "undra-swift".to_owned(),
        ),
    };
    format!(
        "// swift-tools-version: 6.0\n\
         // Generated by `undra bindgen`: do not edit, it is rewritten with the bindings.\n\
         import PackageDescription\n\
         \n\
         let package = Package(\n\
         \x20   name: \"{module}\",\n\
         \x20   platforms: [.iOS(.v17), .macOS(.v14)],\n\
         \x20   products: [.library(name: \"{module}\", targets: [\"{module}\"])],\n\
         \x20   dependencies: [{dependency}],\n\
         \x20   targets: [\n\
         \x20       .target(\n\
         \x20           name: \"{module}\",\n\
         \x20           dependencies: [.product(name: \"UndraRuntime\", package: \"{package_id}\")],\n\
         \x20           path: \"Sources/{module}\"\n\
         \x20       ),\n\
         \x20   ],\n\
         \x20   swiftLanguageModes: [.v6]\n\
         )\n"
    )
}

/// `build.gradle.kts` of the generated Kotlin module: a JVM library that depends on the runtime.
/// The Android project includes it (`settings.gradle.kts`) and depends on it.
#[must_use]
pub fn kotlin_build(runtime: &RuntimeRef) -> String {
    let version = match runtime {
        RuntimeRef::Path(_) => "0.1.0-SNAPSHOT".to_owned(),
        RuntimeRef::Registry { version } => format!("{}.0", version_base(version)),
    };
    format!(
        "// Generated by `undra bindgen`: do not edit, it is rewritten with the bindings.\n\
         //\n\
         // A plain JVM library (the bindings use no Android API). The Android project includes it\n\
         // as a module and depends on it; the runtime comes from a composite build of the Undra\n\
         // checkout or from Maven Central.\n\
         plugins {{\n\
         \x20   id(\"org.jetbrains.kotlin.jvm\")\n\
         }}\n\
         \n\
         java {{\n\
         \x20   sourceCompatibility = JavaVersion.VERSION_11\n\
         \x20   targetCompatibility = JavaVersion.VERSION_11\n\
         }}\n\
         \n\
         kotlin {{\n\
         \x20   compilerOptions {{\n\
         \x20       jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_11)\n\
         \x20   }}\n\
         }}\n\
         \n\
         dependencies {{\n\
         \x20   api(\"dev.undra:runtime:{version}\")\n\
         }}\n"
    )
}

/// `.gitignore` of the generated Kotlin module, written with it.
///
/// The module is a Gradle project, so building the app fills it with Gradle's own output
/// (`build/`, `.gradle/`, `.kotlin/`), which does not belong in the committed `generated/` tree.
/// The patterns are anchored to the module: an unanchored `build/` would also hide a package
/// directory called `build` (an app id like `com.acme.build.app`).
///
/// The CLI owns this file, not the Kotlin generator: the generator writes the sources of the
/// bindings, and the module around them (`build.gradle.kts`, this file) is what `undra` adds.
pub const KOTLIN_GITIGNORE: &str = "\
# Generated by `undra bindgen`: do not edit, it is rewritten with the bindings.
# Gradle's own output, from building the app that includes this module.
/build/
/.gradle/
/.kotlin/
";

/// `0.1` (or `0.1.0`) as `0.1`: the version text without a patch component.
fn version_base(version: &str) -> String {
    let mut parts = version.split('.');
    let major = parts.next().unwrap_or("0");
    let minor = parts.next().unwrap_or("1");
    format!("{major}.{minor}")
}

/// What [`apply`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Files written (new or changed).
    pub written: Vec<String>,
    /// Files that were already up to date.
    pub unchanged: usize,
    /// Files removed because an earlier run wrote them and this one did not.
    pub removed: Vec<String>,
}

/// Writes `files` below `out` and removes what an earlier run wrote that this one did not.
///
/// # Errors
///
/// `C0010` when a file cannot be written.
pub fn apply(out: &Path, files: &[GeneratedFile]) -> Result<Applied> {
    fsutil::create_dir_all(out)?;
    let previous = read_manifest(out);
    let mut applied = Applied::default();
    for file in files {
        if fsutil::write_if_changed(&out.join(&file.path), &file.contents)? {
            applied.written.push(file.path.clone());
        } else {
            applied.unchanged += 1;
        }
    }
    let current: BTreeSet<&str> = files.iter().map(|f| f.path.as_str()).collect();
    for old in previous.iter().filter(|p| !current.contains(p.as_str())) {
        let path = out.join(old);
        if path.is_file() && std::fs::remove_file(&path).is_ok() {
            applied.removed.push(old.clone());
            prune_empty_parents(out, &path);
        }
    }
    let manifest: Vec<&str> = current.into_iter().collect();
    fsutil::write_if_changed(&out.join(MANIFEST), &(manifest.join("\n") + "\n"))?;
    Ok(applied)
}

fn read_manifest(out: &Path) -> Vec<String> {
    std::fs::read_to_string(out.join(MANIFEST))
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.contains("..") && !l.starts_with('/'))
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Removes directories left empty by deleting `file`, up to (not including) `root`.
fn prune_empty_parents(root: &Path, file: &Path) {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

/// Compares `files` with what is below `out`; returns one line per difference (empty: up to
/// date). Stale files of an earlier run count as differences.
#[must_use]
pub fn check(out: &Path, files: &[GeneratedFile]) -> Vec<String> {
    let mut problems = Vec::new();
    for file in files {
        match std::fs::read_to_string(out.join(&file.path)) {
            Ok(existing) if existing == file.contents => {}
            Ok(_) => problems.push(format!(
                "{} differs from what the schema generates",
                file.path
            )),
            Err(_) => problems.push(format!("{} is missing", file.path)),
        }
    }
    let current: BTreeSet<&str> = files.iter().map(|f| f.path.as_str()).collect();
    for old in read_manifest(out) {
        if !current.contains(old.as_str()) && out.join(&old).is_file() {
            problems.push(format!(
                "{old} is stale (the schema no longer generates it)"
            ));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use undra_meta::{FieldDef, RecordDef, TypeRef, ids};

    use super::*;

    fn schema() -> Schema {
        let mut schema = Schema::new("demo-core");
        schema.records.push(RecordDef {
            name: "Todo".into(),
            type_id: ids::type_id("Todo"),
            fields: vec![FieldDef {
                name: "title".into(),
                ty: TypeRef::String,
                default: false,
                docs: String::new(),
            }],
            docs: String::new(),
        });
        schema
    }

    fn plan(platforms: Vec<Platform>, runtimes: Runtimes) -> Plan {
        let mut generator = Generator::for_crate("demo-core");
        generator.kotlin_package = "com.example.demo.core".into();
        Plan {
            generator,
            platforms,
            runtimes,
            out: PathBuf::from("/proj/generated"),
        }
    }

    #[test]
    fn each_platform_gets_its_tree_and_manifest() {
        let files = plan_files(
            &schema(),
            &plan(Platform::ALL.to_vec(), Runtimes::from_registries("0.1")),
        )
        .unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"swift/Package.swift"), "{paths:?}");
        assert!(
            paths.contains(&"swift/Sources/DemoCore/Generated/Types.swift"),
            "{paths:?}"
        );
        assert!(paths.contains(&"kotlin/build.gradle.kts"), "{paths:?}");
        assert!(
            paths.contains(&"kotlin/.gitignore"),
            "Gradle output must not land in the committed tree: {paths:?}"
        );
        assert!(
            paths.contains(&"kotlin/src/main/kotlin/com/example/demo/core/Types.kt"),
            "{paths:?}"
        );
        assert!(
            paths.contains(&"ts/src/types.ts") && paths.contains(&"ts/package.json"),
            "{paths:?}"
        );
        assert!(paths.windows(2).all(|w| w[0] <= w[1]), "sorted");
    }

    #[test]
    fn the_kotlin_gitignore_is_anchored_to_the_module() {
        // `build/` alone would also ignore a package directory named `build`.
        let rules: Vec<&str> = KOTLIN_GITIGNORE
            .lines()
            .filter(|l| !l.starts_with('#'))
            .collect();
        assert_eq!(rules, ["/build/", "/.gradle/", "/.kotlin/"]);
    }

    #[test]
    fn only_the_chosen_platforms_are_generated() {
        let files = plan_files(
            &schema(),
            &plan(vec![Platform::Web], Runtimes::from_registries("0.1")),
        )
        .unwrap();
        assert!(files.iter().all(|f| f.path.starts_with("ts/")), "{files:?}");
    }

    #[test]
    fn the_swift_package_points_at_the_runtime_relative_to_itself() {
        let runtimes = Runtimes::in_repo(Path::new("/src/undra"));
        let text = swift_package(
            "DemoCore",
            &runtimes.swift,
            Path::new("/proj/generated/swift"),
        );
        assert!(
            text.contains(".package(path: \"../../../src/undra/runtimes/swift/UndraRuntime\")"),
            "{text}"
        );
        assert!(text.contains("package: \"UndraRuntime\""), "{text}");
        assert!(text.contains("path: \"Sources/DemoCore\""), "{text}");
    }

    #[test]
    fn the_registry_flavours_name_the_published_packages() {
        let swift = swift_package(
            "DemoCore",
            &RuntimeRef::Registry {
                version: "0.1".into(),
            },
            Path::new("/x"),
        );
        assert!(swift.contains("from: \"0.1.0\""), "{swift}");
        assert!(
            kotlin_build(&RuntimeRef::Registry {
                version: "0.1".into()
            })
            .contains("dev.undra:runtime:0.1.0\"")
        );
        assert!(kotlin_build(&RuntimeRef::Path(PathBuf::from("/k"))).contains("0.1.0-SNAPSHOT"));
    }

    #[test]
    fn invalid_schemas_are_reported_with_bindgens_diagnostics() {
        let mut bad = schema();
        bad.records.push(bad.records[0].clone()); // a duplicate type name: E0050
        let e = plan_files(
            &bad,
            &plan(vec![Platform::Web], Runtimes::from_registries("0.1")),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Bindgen);
        assert!(
            e.detail.as_deref().unwrap_or_default().contains("E0050"),
            "{e}"
        );
    }

    #[test]
    fn apply_writes_then_prunes_only_what_it_wrote() {
        let out = fsutil::unique_temp_dir("bindgen-apply");
        let file = |path: &str, contents: &str| GeneratedFile {
            path: path.into(),
            contents: contents.into(),
        };
        let first = apply(&out, &[file("ts/a.ts", "a"), file("ts/sub/b.ts", "b")]).unwrap();
        assert_eq!(first.written, ["ts/a.ts", "ts/sub/b.ts"]);
        // The user's own file next to the generated ones must survive.
        std::fs::write(out.join("ts/mine.ts"), "mine").unwrap();

        let second = apply(&out, &[file("ts/a.ts", "a")]).unwrap();
        assert_eq!(second.unchanged, 1);
        assert_eq!(second.removed, ["ts/sub/b.ts"]);
        assert!(
            !out.join("ts/sub").exists(),
            "the emptied directory is pruned"
        );
        assert!(out.join("ts/mine.ts").exists());
        assert!(check(&out, &[file("ts/a.ts", "a")]).is_empty());
        assert_eq!(check(&out, &[file("ts/a.ts", "changed")]).len(), 1);
        assert_eq!(
            check(&out, &[file("ts/a.ts", "a"), file("ts/new.ts", "n")]),
            ["ts/new.ts is missing"]
        );
        let _ = std::fs::remove_dir_all(out);
    }

    #[test]
    fn a_manifest_cannot_name_files_outside_the_output() {
        let out = fsutil::unique_temp_dir("bindgen-manifest");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join(MANIFEST), "../escape\n/abs\nok.txt\n").unwrap();
        assert_eq!(read_manifest(&out), ["ok.txt"]);
        let _ = std::fs::remove_dir_all(out);
    }

    #[test]
    fn lenient_canonicalization_handles_paths_that_do_not_exist_yet() {
        let base = fsutil::unique_temp_dir("canon");
        std::fs::create_dir_all(&base).unwrap();
        let resolved = canonicalize_lenient(&base.join("a/b/c"));
        assert_eq!(resolved, base.canonicalize().unwrap().join("a/b/c"));
        let _ = std::fs::remove_dir_all(base);
    }
}
