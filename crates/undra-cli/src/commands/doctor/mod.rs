//! `undra doctor`: what this machine has against what the toolchain uses.
//!
//! Every prerequisite that `undra init`, `build`, `dev` and `bench` touch anywhere is checked, and
//! each [`Finding`] says what was found (`ok`, `missing`, `wrong-version`), the value it observed,
//! the exact command that fixes it and the heading of `docs/ONBOARDING.md` that explains it.
//! `--fix` prints those commands as one block to paste (they are never run) and `--json` prints
//! the whole report for tools.
//!
//! Every check is a function of a [`Sys`], so the tests describe machines (a Mac with Xcode but no
//! NDK, a Linux box with only Rust) and assert the findings without depending on the one they run
//! on. Each check lives in the module of its platform and is declared once as a [`Check`], which
//! is what makes a finding name its documentation.

pub mod android;
pub mod finding;
pub mod ios;
pub mod rust;
pub mod system;
pub mod web;

#[cfg(test)]
mod testing;

use std::path::Path;

use serde_json::json;

use crate::cli::DoctorArgs;
use crate::config::Platform;
use crate::error::Result;
use crate::project::Project;
use crate::runtimes::enclosing_checkout;
use crate::sys::Sys;
use crate::toolchain::Toolchain;
use crate::ui::Ui;

pub use finding::{Finding, Report, Section, Status};

use super::Env;

/// Everything a check looks at: the machine, what the project asks of it and who is asking.
pub struct Context<'a> {
    /// The machine.
    pub sys: &'a dyn Sys,
    /// Where the toolchains were found.
    pub toolchain: &'a Toolchain,
    /// The platforms to check.
    pub scope: &'a [Platform],
    /// The ABIs the project builds for Android (`[android] abis`).
    pub android_abis: &'a [String],
    /// The simulator architectures of the iOS build (`[ios] simulator_archs`).
    pub simulator_archs: &'a [String],
    /// The project root, when the command runs inside a project.
    pub project: Option<&'a Path>,
    /// Whether the person works on Undra itself (see [`is_contributor`]).
    pub contributor: bool,
}

/// The first line of a tool's version output; `None` when it does not run or prints nothing.
pub(crate) fn version_line(cx: &Context<'_>, program: &Path, args: &[&str]) -> Option<String> {
    let out = cx.sys.run(program, args, &cx.toolchain.env_pairs())?;
    if !out.success {
        return None;
    }
    out.text()
        .lines()
        .next()
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty())
}

/// Runs every check that applies to `cx.scope`.
#[must_use]
pub fn check(cx: &Context<'_>) -> Report {
    let mut sections = vec![Section {
        title: "Rust",
        findings: rust::check(cx),
    }];
    if cx.scope.contains(&Platform::Ios) {
        sections.push(Section {
            title: "iOS",
            findings: ios::check(cx),
        });
    }
    if cx.scope.contains(&Platform::Android) {
        sections.push(Section {
            title: "Android",
            findings: android::check(cx),
        });
    }
    if cx.scope.contains(&Platform::Web) {
        sections.push(Section {
            title: "Web",
            findings: web::check(cx),
        });
    }
    sections.push(Section {
        title: "System",
        findings: system::check(cx),
    });
    let mut contributors = system::contributors(cx);
    if cx.contributor && cx.scope.contains(&Platform::Android) {
        contributors.push(android::undra_avd(cx));
    }
    sections.push(Section {
        title: "Contributors",
        findings: contributors,
    });
    Report { sections }
}

/// Whether the person works on Undra itself: `UNDRA_CONTRIBUTOR=1`, a project that uses a local
/// checkout (`[undra] path`), or a working directory inside a checkout of the repository.
#[must_use]
pub fn is_contributor(sys: &dyn Sys, project: Option<&Project>, start: &Path) -> bool {
    sys.env("UNDRA_CONTRIBUTOR").is_some_and(|v| v == "1")
        || project.is_some_and(|p| p.config.undra_path.is_some())
        || enclosing_checkout(start).is_some()
}

/// Runs `undra doctor`.
///
/// # Errors
///
/// `C0009` for an unknown platform name; a broken undra.toml is reported as such.
pub fn run(env: &Env<'_>, args: &DoctorArgs) -> Result<bool> {
    let start = env.start_dir()?;
    let project = match Project::discover(&start) {
        Ok(p) => Some(p),
        Err(e) if e.code == crate::error::Code::NoProject => None,
        Err(e) => return Err(e),
    };
    let scope = match (&args.platform, &project) {
        (Some(list), _) => Platform::parse_list(list)?,
        (None, Some(p)) => p.config.platforms.clone(),
        (None, None) => Platform::ALL.to_vec(),
    };
    let toolchain = Toolchain::detect(env.sys);
    let config = project.as_ref().map(|p| &p.config);
    let android_abis = config.map_or_else(
        || crate::config::AndroidConfig::default().abis,
        |c| c.android.abis.clone(),
    );
    let simulator_archs = config.map_or_else(
        || crate::config::IosConfig::default().simulator_archs,
        |c| c.ios.simulator_archs.clone(),
    );
    let cx = Context {
        sys: env.sys,
        toolchain: &toolchain,
        scope: &scope,
        android_abis: &android_abis,
        simulator_archs: &simulator_archs,
        project: project.as_ref().map(|p| p.root.as_path()),
        contributor: is_contributor(env.sys, project.as_ref(), &start),
    };
    let report = check(&cx);
    let name = project.as_ref().map(|p| p.config.name.as_str());
    if args.json {
        let doc = to_json(
            &report,
            name,
            project.as_ref().map(|p| p.root.as_path()),
            &scope,
            cx.contributor,
        );
        env.ui
            .line(&serde_json::to_string_pretty(&doc).unwrap_or_default());
    } else if args.fix {
        // Only the block on stdout, so it can be read, saved or piped; the tally goes to stderr.
        env.ui.line(report.fix_script().trim_end());
        let [ok, warn, fail, _] = report.counts();
        eprintln!("undra doctor: {ok} ok, {warn} warning(s), {fail} failure(s); nothing was run");
    } else {
        env.ui.line(&render(&report, &env.ui, name, &scope));
    }
    Ok(report.passed())
}

/// The report as the JSON document `--json` prints.
#[must_use]
pub fn to_json(
    report: &Report,
    project: Option<&str>,
    root: Option<&Path>,
    scope: &[Platform],
    contributor: bool,
) -> serde_json::Value {
    let [ok, warn, fail, skip] = report.counts();
    json!({
        "undra": crate::version::SEMVER,
        "ok": report.passed(),
        "project": project.map(|name| json!({
            "name": name,
            "root": root.map(|r| r.display().to_string()),
        })),
        "platforms": scope.iter().map(|p| p.name()).collect::<Vec<_>>(),
        "contributor": contributor,
        "summary": { "ok": ok, "warn": warn, "fail": fail, "skip": skip },
        "sections": report.sections.iter().map(|s| json!({
            "title": s.title,
            "findings": s.findings.iter().map(Finding::to_json).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// Renders the report for a terminal.
#[must_use]
pub fn render(report: &Report, ui: &Ui, project: Option<&str>, scope: &[Platform]) -> String {
    let mut out = String::new();
    let names: Vec<&str> = scope.iter().map(|p| p.name()).collect();
    match project {
        Some(name) => out.push_str(&format!(
            "{} checking {name} ({})\n",
            ui.bold_out("undra doctor:"),
            names.join(", ")
        )),
        None => out.push_str(&format!(
            "{} no project here, checking {}\n",
            ui.bold_out("undra doctor:"),
            names.join(", ")
        )),
    }
    for section in &report.sections {
        out.push_str(&format!("\n{}\n", ui.bold_out(section.title)));
        for f in &section.findings {
            let tag = match f.status {
                Status::Ok => ui.green_out("  ok   "),
                Status::Warn => ui.yellow_out("  warn "),
                Status::Fail => ui.red_out("  FAIL "),
                Status::Skip => ui.dim_out("  skip "),
            };
            out.push_str(&format!("{tag} {}\n", f.message));
            if f.needs_fix() {
                for (i, line) in f.fix.iter().enumerate() {
                    out.push_str(&format!(
                        "         {}{line}\n",
                        if i == 0 { "fix: " } else { "     " }
                    ));
                }
                out.push_str(&format!("         docs: {}\n", f.docs_url()));
            }
        }
    }
    let [ok, warn, fail, _] = report.counts();
    out.push_str(&format!(
        "\n{ok} ok, {warn} warning{}, {fail} failure{}\n",
        if warn == 1 { "" } else { "s" },
        if fail == 1 { "" } else { "s" },
    ));
    if warn + fail > 0 {
        out.push_str("`undra doctor --fix` prints the commands that close these gaps as one block (it runs nothing).\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use crate::commands::doctor::finding::Check;

    /// Every check anyone can declare, in one list.
    fn declared() -> Vec<Check> {
        let mut all = Vec::new();
        for list in [rust::ALL, ios::ALL, android::ALL, web::ALL, system::ALL] {
            all.extend_from_slice(list);
        }
        all
    }

    #[test]
    fn a_fully_configured_machine_has_no_gaps() {
        let sys = fully_configured_machine();
        let report = scan(&sys, &["ios", "android", "web"]);
        for f in report.findings() {
            assert!(
                matches!(f.status, Status::Ok | Status::Skip),
                "{} is {:?}: {}",
                f.id,
                f.status,
                f.message
            );
        }
        assert!(report.passed());
    }

    #[test]
    fn a_bare_machine_fails_with_fixes_and_the_docs_to_read() {
        let report = scan(&bare_machine(), &["web"]);
        assert!(!report.passed());
        for f in report.findings().filter(|f| f.status == Status::Fail) {
            assert!(!f.fix.is_empty(), "{} fails without a fix", f.id);
            assert!(!f.anchor.is_empty(), "{} has no docs anchor", f.id);
        }
    }

    #[test]
    fn every_declared_check_is_reported_on_some_machine() {
        // The union of what a bare Mac, a bare Linux box and a fully configured Mac report, for
        // every platform, as a contributor, in a project: each declared check shows up somewhere.
        let mut reported = std::collections::BTreeSet::new();
        for sys in [
            bare_mac(),
            bare_machine(),
            fully_configured_machine(),
            good_linux_machine(),
        ] {
            for report in [
                run_as_contributor(&sys, &["ios", "android", "web"]),
                run_in_project(&sys, &["ios", "android", "web"], &["arm64", "x86_64"]),
                run_with_abis(&sys, &["arm64-v8a", "x86_64", "armeabi-v7a", "x86"]),
            ] {
                reported.extend(report.findings().map(|f| f.id));
            }
        }
        for check in declared() {
            assert!(
                reported.contains(check.id),
                "{} is never reported",
                check.id
            );
        }
        // And nothing is reported that was not declared.
        let declared: std::collections::BTreeSet<&str> = declared().iter().map(|c| c.id).collect();
        for id in &reported {
            assert!(declared.contains(id), "{id} is reported but not declared");
        }
    }

    #[test]
    fn check_ids_are_unique_and_every_anchor_names_a_heading_of_the_onboarding_guide() {
        let mut ids = std::collections::BTreeSet::new();
        for c in declared() {
            assert!(ids.insert(c.id), "{} is declared twice", c.id);
        }
        let guide = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/ONBOARDING.md"),
        )
        .expect("docs/ONBOARDING.md");
        let mut in_fence = false;
        let mut anchors = std::collections::BTreeSet::new();
        for line in guide.lines() {
            if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
            } else if !in_fence {
                anchors.extend(heading_anchor(line));
            }
        }
        for c in declared() {
            assert!(
                anchors.contains(c.anchor),
                "{} points at #{}, which is not a heading of docs/ONBOARDING.md (headings: {anchors:?})",
                c.id,
                c.anchor
            );
        }
    }

    /// GitHub's anchor of a Markdown heading: lowercase, punctuation dropped, spaces to dashes.
    fn heading_anchor(line: &str) -> Option<String> {
        let text = line.trim_start_matches('#');
        if text.len() == line.len() || !text.starts_with(' ') {
            return None;
        }
        let slug: String = text
            .trim()
            .to_lowercase()
            .chars()
            .filter_map(|c| match c {
                'a'..='z' | '0'..='9' | '-' | '_' => Some(c),
                ' ' => Some('-'),
                _ => None,
            })
            .collect();
        Some(slug)
    }

    #[test]
    fn the_report_renders_fixes_and_docs_under_each_gap() {
        let report = scan(&bare_mac(), &["web"]);
        let text = render(&report, &Ui::plain(), Some("todo"), &[Platform::Web]);
        assert!(text.contains("checking todo (web)"), "{text}");
        assert!(text.contains("  FAIL  node was not found"), "{text}");
        assert!(
            text.contains("         fix: brew install node\n         docs: https://github.com/shreypdev/undra/blob/main/docs/ONBOARDING.md#node-and-npm"),
            "{text}"
        );
        assert!(text.contains("`undra doctor --fix`"), "{text}");
        let clean = render(
            &scan(&fully_configured_machine(), &["web"]),
            &Ui::plain(),
            None,
            &[Platform::Web],
        );
        assert!(clean.contains("no project here, checking web"), "{clean}");
        assert!(!clean.contains("--fix"), "{clean}");
    }

    #[test]
    fn json_has_the_documented_shape() {
        let report = scan(&bare_mac(), &["web"]);
        let doc = to_json(
            &report,
            Some("todo"),
            Some(Path::new("/work/todo")),
            &[Platform::Web],
            false,
        );
        assert_eq!(doc["undra"], env!("CARGO_PKG_VERSION"));
        assert_eq!(doc["ok"], false);
        assert_eq!(doc["project"]["name"], "todo");
        assert_eq!(doc["platforms"], json!(["web"]));
        assert_eq!(doc["contributor"], false);
        let sections = doc["sections"].as_array().unwrap();
        let titles: Vec<&str> = sections
            .iter()
            .map(|s| s["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, ["Rust", "Web", "System", "Contributors"]);
        let node = sections
            .iter()
            .flat_map(|s| s["findings"].as_array().unwrap())
            .find(|f| f["id"] == "web.node")
            .expect("web.node");
        assert_eq!(node["status"], "missing");
        assert_eq!(node["severity"], "fail");
        assert_eq!(node["fix"], json!(["brew install node"]));
        assert_eq!(node["docs"], "docs/ONBOARDING.md#node-and-npm");
        assert!(
            node["docs_url"]
                .as_str()
                .unwrap()
                .ends_with("#node-and-npm")
        );
        assert_eq!(node["audience"], "everyone");
        assert_eq!(
            doc["summary"]["fail"].as_u64().unwrap() as usize,
            report.counts()[2]
        );
        let none = to_json(&report, None, None, &[Platform::Web], true);
        assert!(none["project"].is_null());
        assert_eq!(none["contributor"], true);
    }

    #[test]
    fn contributors_are_recognised_by_variable_checkout_or_project() {
        use crate::config::ProjectConfig;
        let sys = bare_machine();
        assert!(!is_contributor(&sys, None, Path::new("/nonexistent/dir")));
        assert!(is_contributor(
            &bare_machine().with_env("UNDRA_CONTRIBUTOR", "1"),
            None,
            Path::new("/nonexistent/dir")
        ));
        assert!(!is_contributor(
            &bare_machine().with_env("UNDRA_CONTRIBUTOR", "0"),
            None,
            Path::new("/nonexistent/dir")
        ));
        let mut config = ProjectConfig::new("a", "com.example.a", vec![Platform::Web]);
        let mut project = Project {
            root: "/work/a".into(),
            config: config.clone(),
        };
        assert!(!is_contributor(
            &sys,
            Some(&project),
            Path::new("/nonexistent/dir")
        ));
        config.undra_path = Some("../undra".into());
        project.config = config;
        assert!(is_contributor(
            &sys,
            Some(&project),
            Path::new("/nonexistent/dir")
        ));
        // A directory inside the repository this test runs from is a checkout.
        assert!(is_contributor(
            &sys,
            None,
            Path::new(env!("CARGO_MANIFEST_DIR"))
        ));
    }
}
