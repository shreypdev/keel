//! `undra upgrade`: move a project to the version of this `undra`.
//!
//! The planning is in [`crate::upgrade`] (every pin, and the edits), the notes are in
//! [`crate::migrations`]; this module reads the project, prints the plan, writes the files,
//! regenerates the bindings and prints the notes of every release the project crosses.

use std::path::Path;

use crate::cli::{BindgenArgs, UpgradeArgs};
use crate::error::{CliError, Code, Result};
use crate::fsutil::write_if_changed;
use crate::migrations::{self, Migration};
use crate::project::Project;
use crate::semver::Semver;
use crate::upgrade::{self, FileEdit, Plan, Why};

use super::Env;

/// Runs `undra upgrade`.
///
/// # Errors
///
/// `C0001` outside a project, `C0014` when the project is on a newer release than this `undra`,
/// `C0010` when a file cannot be written, and whatever `undra bindgen` reports (the pins are
/// already moved then, and the message says so).
pub fn run(env: &Env<'_>, args: &UpgradeArgs) -> Result<()> {
    let project = Project::discover(&env.start_dir()?)?;
    let ui = env.ui;
    let target = Semver::parse(crate::version::SEMVER)
        .expect("the CLI's own version is a version: a test checks it");
    let plan = upgrade::plan(&project.root, &project.generated_dir(), &target);

    // A dependency on a checkout of the repository is the version of that checkout.
    if plan.on_a_checkout() {
        ui.line(&path_message(&project, &plan));
        return Ok(());
    }
    if let Some((file, pin)) = plan.ahead() {
        return Err(ahead(&project, file, &pin.shown, &target));
    }

    let current = plan.current();
    ui.line(&format!(
        "{} {} is on Undra {}; this undra is {target}",
        ui.bold_out("undra upgrade:"),
        project.config.name,
        current.as_ref().map_or_else(
            || "an unreleased commit (no pin names a version)".to_owned(),
            ToString::to_string
        ),
    ));
    for (file, unmovable) in &plan.unmovable {
        if unmovable.why == Why::Fork {
            ui.warn(&format!(
                "{}:{}: depends on a fork of Undra, which has no release tags to move to; left as it is ({})",
                file.display(),
                unmovable.line,
                unmovable.text
            ));
        }
    }

    if plan.files.is_empty() {
        ui.line(&format!(
            "Every pin already names {target} (or the release line it belongs to): nothing to change."
        ));
        if plan.pins.is_empty() {
            ui.hint("no pin of an Undra version was found (core/Cargo.toml, undra.toml, the apps' runtime references and the workflow are where `undra init` writes them)");
        }
        return Ok(());
    }

    ui.line("");
    for edit in &plan.files {
        ui.line(&render_edit(edit));
    }
    ui.line(&format!(
        "{} line{} in {} file{}.",
        plan.change_count(),
        if plan.change_count() == 1 { "" } else { "s" },
        plan.files.len(),
        if plan.files.len() == 1 { "" } else { "s" }
    ));

    if args.dry_run {
        ui.line("");
        print_notes(env, current.as_ref(), &target);
        ui.line("");
        ui.line("--dry-run: nothing was written. Run `undra upgrade` to apply it.");
        return Ok(());
    }

    for edit in &plan.files {
        write_if_changed(&project.root.join(&edit.path), &edit.after)?;
    }
    ui.line(&format!(
        "Updated {} file{}.",
        plan.files.len(),
        if plan.files.len() == 1 { "" } else { "s" }
    ));
    for hint in lock_file_hints(&plan) {
        ui.hint(&hint);
    }

    if args.no_bindgen {
        ui.hint("bindings not regenerated (--no-bindgen): run `undra bindgen` before you build");
    } else {
        ui.line("");
        ui.step("Regenerating the bindings (undra bindgen)");
        let bindgen_args = BindgenArgs {
            schema: None,
            out: None,
            release: false,
            platforms: None,
            check: false,
            docs: args.docs,
            crate_name: None,
        };
        if let Err(e) = super::bindgen::run(env, &bindgen_args) {
            ui.warn("the pins above are already moved; fix what `undra bindgen` reports below and run it again");
            return Err(e);
        }
    }
    ui.line("");
    print_notes(env, current.as_ref(), &target);
    ui.line("");
    ui.line("Review with `git diff`, then build.");
    Ok(())
}

/// What to say about a project that depends on a checkout of the repository.
fn path_message(project: &Project, plan: &Plan) -> String {
    let mut out = format!(
        "{} {} depends on a checkout of the Undra repository, so its version is whatever that checkout is:\n",
        "undra upgrade:", project.config.name
    );
    for (file, u) in plan.unmovable.iter().filter(|(_, u)| u.why == Why::Path) {
        out.push_str(&format!("  {}:{}  {}\n", file.display(), u.line, u.text));
    }
    out.push_str("Nothing was changed. Update the checkout itself (`git pull` there, then `undra bindgen` here) to move the project; to follow releases instead, create a project without `--undra-path`.");
    out
}

/// `C0014`: the project is ahead of this `undra`.
fn ahead(project: &Project, file: &Path, shown: &str, target: &Semver) -> CliError {
    CliError::new(
        Code::UndraMismatch,
        format!(
            "{} pins Undra at {shown}, newer than this undra ({target})",
            file.display()
        ),
        format!(
            "`undra upgrade` moves a project forward only, and a CLI older than the project's runtimes would generate bindings they do not match ({} is {})",
            project.config.name,
            project.root.display()
        ),
        "update this `undra` first (`curl -fsSL https://shreypdev.github.io/undra/install.sh | sh`, `brew upgrade undra` or `npm update -g @undra/cli`) and run `undra upgrade` again",
    )
}

/// One file's edits as a diff: the lines that go and the lines that come.
fn render_edit(edit: &FileEdit) -> String {
    let mut out = format!("{}\n", edit.path.display());
    for change in &edit.changes {
        out.push_str(&format!("  line {}  ({})\n", change.line, change.what));
        if !change.old.is_empty() {
            out.push_str(&format!("    - {}\n", change.old));
        }
        if !change.new.is_empty() {
            out.push_str(&format!("    + {}\n", change.new));
        }
    }
    out.trim_end().to_owned()
}

/// What to run after the pins moved so the lock files follow.
fn lock_file_hints(plan: &Plan) -> Vec<String> {
    let mut hints = Vec::new();
    if plan.files.iter().any(|f| f.path.ends_with("package.json")) {
        hints.push("`npm install` in the web app refreshes its package-lock.json".to_owned());
    }
    if plan.files.iter().any(|f| f.path.ends_with("Cargo.toml")) {
        hints.push("Cargo.lock follows at the next build (it fetches the new tag, so it needs the network)".to_owned());
    }
    hints
}

/// Prints the notes of every release between `from` and `to`.
fn print_notes(env: &Env<'_>, from: Option<&Semver>, to: &Semver) {
    let ui = env.ui;
    let crossed = migrations::between(from, to);
    let heading = match from {
        Some(from) => format!("Migration notes, {from} to {to}"),
        None => format!("Migration notes, up to {to} (the project's version is not known)"),
    };
    if crossed.is_empty() {
        ui.line(&format!(
            "{heading}: none. No release in between changed anything an app author has to know."
        ));
        return;
    }
    ui.line(&ui.bold_out(&heading));
    for migration in crossed {
        ui.line(&render_migration(migration));
    }
}

/// A release's notes, wrapped for a terminal.
fn render_migration(migration: &Migration) -> String {
    let mut out = format!("\n{}  {}\n", migration.version, migration.title);
    for note in migration.notes {
        let marker = format!("  [{}] ", note.kind.marker());
        out.push_str(&wrap(note.text, &marker, 100));
    }
    out.trim_end().to_owned()
}

/// Wraps `text` at `width` columns; the first line starts with `marker`, the others are indented
/// to match.
fn wrap(text: &str, marker: &str, width: usize) -> String {
    let indent = " ".repeat(marker.chars().count());
    let mut out = String::new();
    let mut line = marker.to_owned();
    let mut empty = true;
    for word in text.split_whitespace() {
        if !empty && line.chars().count() + 1 + word.chars().count() > width {
            out.push_str(&line);
            out.push('\n');
            line = indent.clone();
            empty = true;
        }
        if !empty {
            line.push(' ');
        }
        line.push_str(word);
        empty = false;
    }
    out.push_str(&line);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::{Kind, Note};

    #[test]
    fn notes_are_wrapped_with_a_hanging_indent() {
        let text = wrap(
            "one two three four five six seven eight nine ten",
            "  [do] ",
            24,
        );
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines.len() > 2, "{text}");
        assert!(lines[0].starts_with("  [do] one"), "{text}");
        assert!(
            lines[1].starts_with("       ") && !lines[1].starts_with("        "),
            "{text}"
        );
        assert!(lines.iter().all(|l| l.chars().count() <= 24), "{text}");
    }

    #[test]
    fn a_migration_prints_its_version_title_and_marked_notes() {
        let m = Migration {
            version: "9.9.9",
            title: "Something changed",
            notes: &[
                Note {
                    kind: Kind::Action,
                    text: "Do this.",
                },
                Note {
                    kind: Kind::New,
                    text: "A new thing.",
                },
            ],
        };
        let text = render_migration(&m);
        assert_eq!(
            text,
            "\n9.9.9  Something changed\n  [do] Do this.\n  [new] A new thing."
        );
    }
}
