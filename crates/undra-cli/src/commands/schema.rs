//! `undra schema diff` and `undra schema export` (ADR-062): the public API as a file, and what
//! changed in it.

use std::path::{Path, PathBuf};

use undra_bindgen::provenance;
use undra_meta::Schema;

use crate::cli::{SchemaArgs, SchemaCommand, SchemaDiffArgs, SchemaExportArgs};
use crate::error::{CliError, Code, Result};
use crate::fsutil;
use crate::project::Project;
use crate::schema::parse_schema_json;
use crate::schema_diff::{Severity, diff};
use crate::sys::Sys;

use super::Env;

/// The schema file `--against` reads when none is named, in the project's directory.
pub const DEFAULT_FILE: &str = "schema.json";

/// Runs `undra schema`. `Ok(false)` is `diff --exit-code` finding a breaking change: the exit
/// status is 1, and the report has been printed.
///
/// # Errors
///
/// See [`run_diff`] and [`run_export`].
pub fn run(env: &Env<'_>, args: &SchemaArgs) -> Result<bool> {
    match &args.command {
        SchemaCommand::Diff(args) => run_diff(env, args),
        SchemaCommand::Export(args) => run_export(env, args).map(|()| true),
    }
}

/// Runs `undra schema diff`.
///
/// # Errors
///
/// `C0009` when the arguments do not make a comparison (the wrong number of files, a ref that is not
/// usable, a file outside a repository, a ref that has no such file), `C0002` for a file that is not
/// a schema, `C0010` for one that cannot be read, `C0003` when `git` is needed and missing.
pub fn run_diff(env: &Env<'_>, args: &SchemaDiffArgs) -> Result<bool> {
    let start = env.start_dir()?;
    let sides = match &args.against {
        Some(git_ref) => against(env, &start, git_ref, &args.files)?,
        None => two_files(&start, &args.files)?,
    };
    let old = parse(&sides.old_text, &sides.old_label)?;
    let new = parse(&sides.new_text, &sides.new_label)?;
    let diff = diff(&old, &new);
    let ui = env.ui;
    let report =
        diff.render_with(
            &sides.old_label,
            &sides.new_label,
            |severity, label| match severity {
                Severity::Breaking => ui.red_out(label),
                Severity::Additive => ui.green_out(label),
            },
        );
    for line in report.lines() {
        ui.line(line);
    }
    // The file is what was compared and nothing was built, so a file nobody exported again after
    // an API change makes a report that is not the API change. The bindings beside it say which
    // schema they were generated from; where the two disagree, say so.
    let mut stale_now = false;
    if let Some(check) = &sides.bindings {
        if let Some(hash) = check.now
            && hash != new.hash()
        {
            stale_now = true;
            ui.warn(&format!(
                "{file} (schema {:#018x}) is not the schema the bindings in {dir} were generated from \
                 (schema {hash:#018x}): one of them is stale, so this report is not the API change; \
                 `undra schema export -o {file}` (and `undra bindgen`) bring both up to date",
                new.hash(),
                file = sides.new_label,
                dir = check.dir,
            ));
        }
        if let Some(hash) = check.then
            && hash != old.hash()
        {
            ui.warn(&format!(
                "at {git_ref}, {file} (schema {:#018x}) is not the schema of the bindings committed beside it \
                 (schema {hash:#018x}): it was not exported there, so this report also counts changes made \
                 before {git_ref}, or misses them",
                old.hash(),
                git_ref = check.git_ref,
                file = sides.new_label,
            ));
        }
    }
    Ok(!(args.exit_code && (diff.has_breaking() || stale_now)))
}

/// Runs `undra schema export`: builds the core, reads its schema and writes it, or with `--check`
/// compares it with the file instead.
///
/// # Errors
///
/// `C0001` outside a project, `C0004`/`C0006` when the core cannot be built or its schema read,
/// `C0010` when the file cannot be written, and with `--check` `C0007` when the file is not what
/// the export would write.
pub fn run_export(env: &Env<'_>, args: &SchemaExportArgs) -> Result<()> {
    let session = env.session()?;
    let schema = super::bindgen::schema_from_core(&session, args.release, args.docs)?;
    let text = crate::schema_file::render(&schema);
    if args.check {
        let start = env.start_dir()?;
        let (path, shown) = match &args.output {
            Some(path) => (resolve(&start, path), path.display().to_string()),
            None => (
                session.project.root.join(DEFAULT_FILE),
                DEFAULT_FILE.to_owned(),
            ),
        };
        return check_export(env, &path, &shown, &text, &schema, args.docs);
    }
    match &args.output {
        Some(path) => {
            let path = resolve(&env.start_dir()?, path);
            let written = fsutil::write_if_changed(&path, &text)?;
            env.ui.line(&format!(
                "{} {} (schema hash {:#018x}).",
                if written { "Wrote" } else { "Unchanged:" },
                path.display(),
                schema.hash()
            ));
        }
        None => env.ui.line(text.trim_end()),
    }
    Ok(())
}

/// `undra schema export --check`: the file at `path` is exactly `text`, what the export would write.
///
/// # Errors
///
/// `C0007` when it is not (another schema, the same one written with other flags, a hand edit) or
/// is not there, as `undra bindgen --check` reports bindings that are out of date.
fn check_export(
    env: &Env<'_>,
    path: &Path,
    shown: &str,
    text: &str,
    schema: &Schema,
    docs: bool,
) -> Result<()> {
    let export = format!(
        "undra schema export -o {shown}{}",
        if docs { " --docs" } else { "" }
    );
    let Ok(found) = std::fs::read_to_string(path) else {
        return Err(CliError::new(
            Code::Bindgen,
            format!("{shown} does not exist"),
            "`--check` compares the committed schema file with the core's schema, and there is no file to compare",
            format!("run `{export}` and commit the file"),
        ));
    };
    if found == text {
        env.ui.line(&format!(
            "{shown} is up to date (schema hash {:#018x}).",
            schema.hash()
        ));
        return Ok(());
    }
    // Say whether the API moved or only the text did (flags, a hand edit, an older `undra`).
    let detail = match parse_schema_json(&found, &schema.crate_name) {
        Ok(committed) if committed.hash() == schema.hash() => format!(
            "  the file has the core's API (schema {:#018x}) in other text: exported with other flags \
             (`--docs` keeps the doc comments), by another `undra`, or edited by hand",
            schema.hash()
        ),
        Ok(committed) => format!(
            "  {shown}: schema {:#018x}\n  the core: schema {:#018x}",
            committed.hash(),
            schema.hash()
        ),
        Err(_) => format!("  {shown} is not a schema file"),
    };
    Err(CliError::new(
        Code::Bindgen,
        format!("{shown} is not the schema of the core"),
        "the file is what `undra schema diff --against` reads and what a review sees, and it was exported before the \
         core's public API last changed (or with other flags, or edited by hand)",
        format!("run `{export}` and commit the file"),
    )
    .with_detail(detail))
}

/// The two schemas of a comparison, as text, with the words that name them.
#[derive(Debug)]
struct Sides {
    old_text: String,
    old_label: String,
    new_text: String,
    new_label: String,
    /// With `--against` in a project that commits its bindings: the schema hashes they carry.
    bindings: Option<BindingsCheck>,
}

/// The schema hashes the project's generated bindings carry in their headers
/// (`undra_bindgen::provenance::schema_hash_in`), in the working tree and at the ref: what a schema
/// file that was exported when the bindings were generated has too.
#[derive(Debug)]
struct BindingsCheck {
    /// The bindings' directory as shown (`generated/`).
    dir: String,
    /// The ref of `--against`.
    git_ref: String,
    /// The working tree's bindings' hash, when there are bindings.
    now: Option<u64>,
    /// The hash of the bindings committed at the ref, when it has them.
    then: Option<u64>,
}

/// Whether a manifest entry is a Swift, Kotlin or TypeScript source, whose first line names the
/// schema hash (`Package.swift` does not depend on the schema and has none).
fn is_bindings_source(path: &str) -> bool {
    (path.ends_with(".swift") && !path.ends_with("Package.swift"))
        || path.ends_with(".kt")
        || path.ends_with(".ts")
}

/// The schema hash of a tree of bindings, `read` giving the text of a path relative to its
/// directory: the manifest `undra bindgen` writes (`.undra-generated`) names the files, and the
/// first source among them names the hash. `None` without a manifest or a header with a hash.
fn bindings_hash(read: impl Fn(&str) -> Option<String>) -> Option<u64> {
    let manifest = read(crate::bindgen::MANIFEST)?;
    manifest
        .lines()
        .map(str::trim)
        .filter(|p| is_bindings_source(p))
        // The first source has the header; a tree of an older generator has none in any, so a few
        // reads are enough to tell.
        .take(3)
        .find_map(|p| read(p).as_deref().and_then(provenance::schema_hash_in))
}

/// The bindings of the project that holds `file`, in the working tree and as committed at `git_ref`
/// (`git show <ref>:./<generated>/..` from the project's directory). `None` outside a project, or
/// when neither has bindings: then there is nothing to compare the file with.
fn bindings_check(sys: &dyn Sys, git: &Path, file: &Path, git_ref: &str) -> Option<BindingsCheck> {
    let project = Project::discover(file.parent()?).ok()?;
    let generated = project.config.generated.trim_end_matches('/').to_owned();
    let dir = project.generated_dir();
    let now = bindings_hash(|rel| std::fs::read_to_string(dir.join(rel)).ok());
    let root = project.root.to_str()?;
    let then = if Path::new(&generated).is_absolute() {
        None
    } else {
        bindings_hash(|rel| {
            let object = format!("{git_ref}:./{generated}/{rel}");
            sys.run(git, &["-C", root, "show", &object], &[])
                .filter(|out| out.success)
                .map(|out| out.stdout)
        })
    };
    (now.is_some() || then.is_some()).then(|| BindingsCheck {
        dir: format!("{generated}/"),
        git_ref: git_ref.to_owned(),
        now,
        then,
    })
}

/// `path` as the command sees it: relative paths are relative to where `-C` (or the shell) says.
fn resolve(start: &Path, path: &Path) -> PathBuf {
    start.join(path)
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| CliError::io("read", path, &e))
}

/// `undra schema diff OLD NEW`.
fn two_files(start: &Path, files: &[PathBuf]) -> Result<Sides> {
    let [old, new] = files else {
        return Err(CliError::bad_argument(
            format!(
                "`undra schema diff` compares two schema files, and {} {} given",
                files.len(),
                if files.len() == 1 { "was" } else { "were" }
            ),
            "the old schema and the new one are the two things a diff is between",
            "give the old file and then the new one (`undra schema diff old.json new.json`), or compare a file with its \
             version at a git ref (`undra schema diff --against origin/main`)",
        ));
    };
    let (old_path, new_path) = (resolve(start, old), resolve(start, new));
    Ok(Sides {
        old_text: read(&old_path)?,
        old_label: old.display().to_string(),
        new_text: read(&new_path)?,
        new_label: new.display().to_string(),
        bindings: None,
    })
}

/// `undra schema diff --against REF [FILE]`.
fn against(env: &Env<'_>, start: &Path, git_ref: &str, files: &[PathBuf]) -> Result<Sides> {
    let file = match files {
        [] => default_file(start),
        [one] => resolve(start, one),
        more => {
            return Err(CliError::bad_argument(
                format!(
                    "`--against` compares one schema file with its version at a git ref, and {} files were given",
                    more.len()
                ),
                "the old side is the file as committed at the ref and the new side is the file in the working tree",
                "name one file (`undra schema diff --against origin/main schema.json`), or none for `schema.json`; \
                 to compare two files, leave out --against",
            ));
        }
    };
    let new_text = read(&file)?;
    let old_text = schema_at_ref(env.sys, &file, git_ref)?;
    // Relative to where the command runs: the default file is found through the canonical project
    // root, so a relative or symlinked `-C` is compared in its canonical form too.
    let canonical_start = start.canonicalize().ok();
    let shown = file
        .strip_prefix(start)
        .ok()
        .or_else(|| {
            canonical_start
                .as_deref()
                .and_then(|s| file.strip_prefix(s).ok())
        })
        .unwrap_or(&file)
        .display()
        .to_string();
    // `schema_at_ref` found git, or it would have failed.
    let bindings = env
        .sys
        .which("git", &[])
        .and_then(|git| bindings_check(env.sys, &git, &file, git_ref));
    Ok(Sides {
        old_text,
        old_label: format!("{git_ref}:{shown}"),
        new_text,
        new_label: shown,
        bindings,
    })
}

/// `schema.json` in the project's directory, or in `start` outside a project.
fn default_file(start: &Path) -> PathBuf {
    let root = Project::discover(start).map_or_else(|_| start.to_path_buf(), |p| p.root);
    root.join(DEFAULT_FILE)
}

/// `file` as committed at `git_ref`: `git show <ref>:<path>`, with the path from the repository's
/// root, which `git rev-parse --show-prefix` says without resolving a symlink.
///
/// # Errors
///
/// `C0003` without `git`; `C0009` when `file` is not in a repository, the ref begins with `-` or
/// names nothing, or the ref has no such file.
pub fn schema_at_ref(sys: &dyn Sys, file: &Path, git_ref: &str) -> Result<String> {
    let (Some(dir), Some(name)) = (
        file.parent().and_then(Path::to_str),
        file.file_name().and_then(|n| n.to_str()),
    ) else {
        return Err(CliError::bad_argument(
            format!("`{}` is not a path `--against` can look up", file.display()),
            "git is asked for the file by its place in the repository",
            "name a file with a UTF-8 path inside the repository",
        ));
    };
    if git_ref.is_empty() || git_ref.starts_with('-') {
        return Err(CliError::bad_argument(
            format!("`--against {git_ref}` is not a git ref"),
            "a ref is a branch, a tag or a commit, and one that starts with `-` would be read as an option of git",
            "give a branch, tag or commit (`origin/main`, `v1.0.0`, `HEAD~1`)",
        ));
    }
    let git = sys.which("git", &[]).ok_or_else(|| {
        CliError::missing_tool(
            "git",
            "`undra schema diff --against` (it reads the schema as committed at a ref)",
            "install git (https://git-scm.com), or compare two files: `undra schema diff old.json new.json`",
        )
    })?;
    let run = |args: &[&str]| sys.run(&git, args, &[]);
    let not_a_repository = || {
        CliError::bad_argument(
            format!("{} is not inside a git repository", file.display()),
            "`--against` reads the file as it was committed at a ref, so it has to live in one",
            "run it in a repository, or compare two files: `undra schema diff old.json new.json`",
        )
    };
    let prefix = run(&["-C", dir, "rev-parse", "--show-prefix"])
        .filter(|out| out.success)
        .ok_or_else(not_a_repository)?
        .stdout
        .trim()
        .to_owned();
    let object = format!("{git_ref}:{prefix}{name}");
    let shown = run(&["-C", dir, "show", &object]).ok_or_else(|| {
        CliError::missing_tool("git", "`undra schema diff --against`", "install git")
    })?;
    if shown.success {
        return Ok(shown.stdout);
    }
    Err(CliError::bad_argument(
        format!("`{git_ref}` has no `{prefix}{name}`"),
        "`--against` compares the schema file committed at the ref with the one in the working tree, so the ref has to have it",
        "commit the schema file there (`undra schema export -o schema.json`), name another file, or compare two files \
         (`undra schema diff old.json new.json`)",
    )
    .with_detail(shown.stderr.trim().to_owned()))
}

/// Parses `text`, naming `label` in the error. The crate name is `core` when the file has none (the
/// canonical form), which the comparison does not read.
fn parse(text: &str, label: &str) -> Result<Schema> {
    parse_schema_json(text, "core").map_err(|mut e| {
        e.what = format!("{label}: {}", e.what);
        e
    })
}

#[cfg(test)]
mod tests {
    use crate::error::Code;
    use crate::sys::fake::FakeSys;

    use super::*;

    const FILE: &str = "/repo/proj/schema.json";

    fn machine() -> FakeSys {
        FakeSys::linux().with_tool("git", "/usr/bin/git")
    }

    #[test]
    fn the_file_is_asked_for_by_its_place_in_the_repository() {
        let sys = machine()
            .with_output("git", "-C /repo/proj rev-parse --show-prefix", "proj/\n")
            .with_output(
                "git",
                "-C /repo/proj show origin/main:proj/schema.json",
                "{}",
            );
        assert_eq!(
            schema_at_ref(&sys, Path::new(FILE), "origin/main").unwrap(),
            "{}"
        );
    }

    #[test]
    fn a_file_at_the_top_of_the_repository_has_an_empty_prefix() {
        let sys = machine()
            .with_output("git", "-C /repo rev-parse --show-prefix", "\n")
            .with_output("git", "-C /repo show HEAD:schema.json", "{\"a\":1}");
        assert_eq!(
            schema_at_ref(&sys, Path::new("/repo/schema.json"), "HEAD").unwrap(),
            "{\"a\":1}"
        );
    }

    #[test]
    fn without_git_the_error_names_the_install_and_the_alternative() {
        let e = schema_at_ref(&FakeSys::linux(), Path::new(FILE), "HEAD").unwrap_err();
        assert_eq!(e.code, Code::MissingTool);
        assert!(e.fix.contains("undra schema diff old.json new.json"), "{e}");
    }

    #[test]
    fn a_file_outside_a_repository_says_so() {
        let sys = machine().with_failing_output(
            "git",
            "-C /repo/proj rev-parse --show-prefix",
            "fatal: not a git repository",
        );
        let e = schema_at_ref(&sys, Path::new(FILE), "HEAD").unwrap_err();
        assert_eq!(e.code, Code::BadArgument);
        assert!(e.what.contains("not inside a git repository"), "{e}");
    }

    #[test]
    fn a_ref_without_the_file_names_both_and_carries_gits_words() {
        let sys = machine()
            .with_output("git", "-C /repo/proj rev-parse --show-prefix", "proj/\n")
            .with_failing_output(
                "git",
                "-C /repo/proj show v1:proj/schema.json",
                "fatal: path 'proj/schema.json' exists on disk, but not in 'v1'",
            );
        let e = schema_at_ref(&sys, Path::new(FILE), "v1").unwrap_err();
        assert_eq!(e.code, Code::BadArgument);
        assert!(e.what.contains("`v1` has no `proj/schema.json`"), "{e}");
        assert!(e.fix.contains("undra schema export -o schema.json"), "{e}");
        assert!(
            e.detail
                .as_deref()
                .unwrap_or_default()
                .contains("not in 'v1'"),
            "{e}"
        );
    }

    #[test]
    fn a_ref_that_looks_like_an_option_is_refused_before_git_sees_it() {
        for bad in ["", "--output=/tmp/x", "-1"] {
            let e = schema_at_ref(&machine(), Path::new(FILE), bad).unwrap_err();
            assert_eq!(e.code, Code::BadArgument, "{bad:?}");
            assert!(e.what.contains("not a git ref"), "{e}");
        }
    }

    #[test]
    fn the_wrong_number_of_files_is_a_teaching_error() {
        let e = two_files(Path::new("/p"), &[PathBuf::from("a.json")]).unwrap_err();
        assert_eq!(e.code, Code::BadArgument);
        assert!(e.what.contains("1 was given"), "{e}");
        assert!(e.fix.contains("--against"), "{e}");
        let e = two_files(Path::new("/p"), &[]).unwrap_err();
        assert!(e.what.contains("0 were given"), "{e}");
    }

    #[test]
    fn a_file_that_is_not_a_schema_names_itself() {
        let e = parse("not json", "old.json").unwrap_err();
        assert_eq!(e.code, Code::BadConfig);
        assert!(
            e.what.starts_with("old.json: the schema is not valid JSON"),
            "{e}"
        );
    }

    #[test]
    fn the_default_file_is_schema_json_beside_the_project_file() {
        // Outside a project the directory itself; the project case is covered by the CLI test.
        assert_eq!(
            default_file(Path::new("/nonexistent/dir")),
            Path::new("/nonexistent/dir/schema.json")
        );
    }
}
