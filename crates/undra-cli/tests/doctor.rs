//! `undra doctor` on the machine the tests run on. What it finds depends on the machine, so these
//! tests only assert what must hold anywhere: it exits 0 or 1 without panicking, `--json` is the
//! documented document, `--fix` prints a block that only holds comments and commands. The findings
//! themselves are tested against described machines in `commands/doctor/` (unit tests).

mod common;

use common::{TempDir, run_err, undra};

fn json_report(args: &[&str], dir: &std::path::Path) -> (bool, serde_json::Value) {
    let out = undra()
        .args(["doctor", "--json"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("undra runs");
    let code = out
        .status
        .code()
        .expect("exited, was not killed by a signal");
    assert!(code == 0 || code == 1, "exit status {code}");
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}):\n{}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    (code == 0, doc)
}

#[test]
fn the_real_doctor_exits_zero_or_one_and_never_panics() {
    let dir = TempDir::new("doctor-real");
    let out = undra()
        .arg("doctor")
        .current_dir(dir.path())
        .output()
        .unwrap();
    let code = out.status.code().expect("not killed by a signal");
    assert!(code == 0 || code == 1, "exit status {code}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    let text = String::from_utf8_lossy(&out.stdout);
    for section in ["Rust", "System", "Contributors"] {
        assert!(
            text.contains(&format!("\n{section}\n")),
            "no {section} section:\n{text}"
        );
    }
    assert_eq!(
        code == 0,
        !text.contains("  FAIL "),
        "the status follows the failures:\n{text}"
    );
}

#[test]
fn json_is_the_documented_document_and_agrees_with_the_exit_status() {
    let dir = TempDir::new("doctor-json");
    let (passed, doc) = json_report(&[], dir.path());
    assert_eq!(doc["undra"], env!("CARGO_PKG_VERSION"));
    assert_eq!(doc["ok"], passed);
    assert!(doc["project"].is_null());
    assert_eq!(
        doc["platforms"],
        serde_json::json!(["ios", "android", "web"])
    );
    let mut fails = 0;
    let mut ids = std::collections::BTreeSet::new();
    for section in doc["sections"].as_array().unwrap() {
        for f in section["findings"].as_array().unwrap() {
            for key in [
                "id", "status", "severity", "audience", "optional", "message", "observed", "fix",
                "docs", "docs_url",
            ] {
                assert!(f.get(key).is_some(), "finding without {key}: {f}");
            }
            assert!(
                ["ok", "missing", "wrong-version", "not-applicable"]
                    .contains(&f["status"].as_str().unwrap()),
                "{f}"
            );
            assert!(
                ids.insert(f["id"].as_str().unwrap().to_owned()),
                "{} twice",
                f["id"]
            );
            assert!(
                f["docs"]
                    .as_str()
                    .unwrap()
                    .starts_with("docs/ONBOARDING.md#"),
                "{f}"
            );
            if f["severity"] == "fail" {
                fails += 1;
                assert!(
                    !f["fix"].as_array().unwrap().is_empty(),
                    "a failure without a fix: {f}"
                );
            }
        }
    }
    assert_eq!(doc["summary"]["fail"], fails);
    assert_eq!(passed, fails == 0);
    for id in [
        "rust.rustc",
        "system.disk",
        "system.undra-on-path",
        "contributors.kotlinc",
    ] {
        assert!(ids.contains(id), "no finding {id}: {ids:?}");
    }
}

#[test]
fn fix_prints_one_block_of_comments_and_commands_and_runs_nothing() {
    let dir = TempDir::new("doctor-fix");
    let out = undra()
        .args(["doctor", "--fix"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("# undra doctor --fix"), "{text}");
    assert!(text.contains("Nothing has been run"), "{text}");
    // Only comments and commands: no report lines that a shell would choke on.
    for line in text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
    {
        assert!(
            !line.trim_start().starts_with("ok ") && !line.trim_start().starts_with("FAIL"),
            "{line}"
        );
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("nothing was run"), "{stderr}");
}

#[test]
fn fix_and_json_are_alternatives() {
    let dir = TempDir::new("doctor-both");
    let (code, stderr) = run_err(
        undra()
            .args(["doctor", "--fix", "--json"])
            .current_dir(dir.path()),
    );
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("--json"), "{stderr}");
}

#[test]
fn inside_a_project_only_its_platforms_are_checked_and_the_project_is_named() {
    let project = common::init_project("doctored", "web");
    let (_, doc) = json_report(&[], &project.root);
    assert_eq!(doc["project"]["name"], "doctored");
    assert_eq!(doc["platforms"], serde_json::json!(["web"]));
    let titles: Vec<&str> = doc["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Rust", "Web", "System", "Contributors"]);
    // The project was created with `--undra-path`: a contributor's project.
    assert_eq!(doc["contributor"], true);
}
