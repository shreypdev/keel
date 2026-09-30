//! The command line as a user meets it: help text, usage errors, and the shape of the errors.

mod common;

use common::{TempDir, keel, run_err, run_ok};

#[test]
fn top_level_help_teaches_the_workflow() {
    let out = run_ok(keel().arg("--help"));
    let text = String::from_utf8_lossy(&out.stdout);
    for needle in [
        "keel init",
        "keel dev",
        "keel build",
        "keel bindgen",
        "keel adopt",
        "keel doctor",
        "error[keel::C00NN]",
    ] {
        assert!(text.contains(needle), "--help lacks {needle:?}:\n{text}");
    }
}

#[test]
fn every_command_has_a_help_with_examples() {
    for command in ["init", "bindgen", "build", "dev", "doctor", "adopt"] {
        let out = run_ok(keel().args([command, "--help"]));
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            text.contains("EXAMPLES"),
            "keel {command} --help has no examples:\n{text}"
        );
        assert!(
            text.contains(&format!("keel {command}")),
            "keel {command} --help does not show its own usage:\n{text}"
        );
    }
}

#[test]
fn specific_help_texts_say_what_matters() {
    let build =
        String::from_utf8_lossy(&run_ok(keel().args(["build", "--help"])).stdout).into_owned();
    assert!(
        build.contains("KeelCore.xcframework")
            && build.contains("jniLibs")
            && build.contains("keel_core.wasm"),
        "{build}"
    );
    assert!(build.contains("-force_load"), "{build}");
    let dev = String::from_utf8_lossy(&run_ok(keel().args(["dev", "--help"])).stdout).into_owned();
    assert!(
        dev.contains("127.0.0.1:7443") && dev.contains("no authentication"),
        "{dev}"
    );
    let bindgen =
        String::from_utf8_lossy(&run_ok(keel().args(["bindgen", "--help"])).stdout).into_owned();
    assert!(
        bindgen.contains("--schema") && bindgen.contains("keel_schema_json"),
        "{bindgen}"
    );
}

#[test]
fn the_version_is_printed() {
    let out = run_ok(keel().arg("--version"));
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("keel 0."));
}

#[test]
fn usage_errors_exit_with_2() {
    let (code, stderr) = run_err(keel().arg("frobnicate"));
    assert_eq!(code, 2, "{stderr}");
    let (code, _) = run_err(&mut keel());
    assert_eq!(code, 2, "no command is a usage error");
    let (code, stderr) = run_err(keel().args(["build", "--platform"]));
    assert_eq!(code, 2, "{stderr}");
}

#[test]
fn outside_a_project_the_error_says_what_why_and_how_to_fix() {
    let dir = TempDir::new("noproject");
    let (code, stderr) = run_err(keel().args(["build"]).current_dir(dir.path()));
    assert_eq!(code, 1);
    assert!(
        stderr.starts_with("error[keel::C0001]: no keel.toml found"),
        "{stderr}"
    );
    assert!(
        stderr.contains("= note:") && stderr.contains("= help:") && stderr.contains("keel init"),
        "{stderr}"
    );
    assert!(stderr.contains("https://keel.dev/errors/C0001"), "{stderr}");
}

#[test]
fn a_bad_project_name_suggests_a_good_one() {
    let dir = TempDir::new("badname");
    let (code, stderr) = run_err(
        keel()
            .args(["init", "My App!"])
            .arg("--dir")
            .arg(dir.path()),
    );
    assert_eq!(code, 1);
    assert!(
        stderr.contains("error[keel::C0009]") && stderr.contains("my-app"),
        "{stderr}"
    );
    assert!(
        std::fs::read_dir(dir.path()).unwrap().next().is_none(),
        "nothing was created"
    );
}

#[test]
fn an_invalid_keel_toml_names_the_line() {
    let dir = TempDir::new("badtoml");
    std::fs::write(
        dir.path().join("keel.toml"),
        "[project]\nname = \"x\"\nid = \"com.example.x\"\nplatfroms = []\n",
    )
    .unwrap();
    let (_, stderr) = run_err(keel().args(["build"]).current_dir(dir.path()));
    assert!(
        stderr.contains("error[keel::C0002]")
            && stderr.contains("line 4")
            && stderr.contains("platfroms"),
        "{stderr}"
    );
}

#[test]
fn doctor_reports_each_area_and_gives_an_exit_status() {
    let dir = TempDir::new("doctor");
    let out = keel()
        .arg("doctor")
        .current_dir(dir.path())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("keel doctor: no project here"), "{text}");
    for section in ["Rust", "iOS", "Android", "Web"] {
        assert!(text.contains(section), "no {section} section:\n{text}");
    }
    let failed = text.contains("FAIL");
    assert_eq!(
        out.status.success(),
        !failed,
        "the exit status follows the failures:\n{text}"
    );
    // A gap always comes with its fix.
    for (i, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("FAIL") {
            let next = text.lines().nth(i + 1).unwrap_or_default();
            assert!(
                next.trim_start().starts_with("fix:"),
                "FAIL without a fix: {line}\n{text}"
            );
        }
    }
}

#[test]
fn doctor_can_be_limited_to_a_platform() {
    let dir = TempDir::new("doctor-web");
    let out = keel()
        .args(["doctor", "--platform", "web"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("Web") && !text.contains("\nAndroid\n") && !text.contains("\niOS\n"),
        "{text}"
    );
    let (code, stderr) = run_err(
        keel()
            .args(["doctor", "--platform", "tv"])
            .current_dir(dir.path()),
    );
    assert_eq!(code, 1);
    assert!(stderr.contains("C0009"), "{stderr}");
}
