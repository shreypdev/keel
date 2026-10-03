//! `undra schema diff` and `undra schema export` (ADR-062): the public API as a file, and what
//! changed in it.

use std::path::{Path, PathBuf};

use undra_meta::Schema;

use crate::cli::{SchemaArgs, SchemaCommand, SchemaDiffArgs, SchemaExportArgs};
use crate::error::{CliError, Result};
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
    Ok(!(args.exit_code && diff.has_breaking()))
}

/// Runs `undra schema export`: builds the core, reads its schema and writes it.
///
/// # Errors
///
/// `C0001` outside a project, `C0004`/`C0006` when the core cannot be built or its schema read, and
/// `C0010` when the file cannot be written.
pub fn run_export(env: &Env<'_>, args: &SchemaExportArgs) -> Result<()> {
    let session = env.session()?;
    let schema = super::bindgen::schema_from_core(&session, args.release, args.docs)?;
    let text = crate::schema_file::render(&schema);
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

/// The two schemas of a comparison, as text, with the words that name them.
#[derive(Debug)]
struct Sides {
    old_text: String,
    old_label: String,
    new_text: String,
    new_label: String,
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
    let shown = file
        .strip_prefix(start)
        .unwrap_or(&file)
        .display()
        .to_string();
    Ok(Sides {
        old_text,
        old_label: format!("{git_ref}:{shown}"),
        new_text,
        new_label: shown,
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
