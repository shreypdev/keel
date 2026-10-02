//! The debugger path into Rust (ADR-046 decision 2): a debug build of the core keeps its full
//! DWARF, so LLDB stops at a breakpoint set by Rust file and line, with the toolchain's Rust
//! formatters loaded from the `.lldbinit` `undra init` writes.
//!
//! The playground core is built in debug (`undra build`, no `--release`) and a small C host
//! (`tests/symbols/harness.c`) calls its `explode` function through the C ABI. LLDB runs in batch
//! mode (`lldb -b`), sets `breakpoint set -f lab.rs -l <the line of the panic in explode>`, runs,
//! and the stop must be in that file at that line, in `playground_core::lab::explode`, called from
//! the generated dispatcher; with the formatters the `reason` argument reads `"kaboom"`.
//!
//! Two processes are debugged: the host one (the macOS dylib, LLDB attached to the process) and, through the
//! prelinked iOS object that the app links (ADR-044), a process in a booted iOS simulator that LLDB
//! attaches to. Tests skip with a message when a toolchain is absent and fail when
//! `UNDRA_REQUIRE_TOOLCHAINS=1`.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use common::{
    StableProject, has_rust_target, has_tool, playground_at, repo_root, run_ok, serial, skip_unless,
};

fn project() -> &'static StableProject {
    static PROJECT: OnceLock<StableProject> = OnceLock::new();
    PROJECT.get_or_init(|| playground_at("debugging"))
}

/// The line of the `panic!` in `explode`.
fn explode_line() -> u32 {
    let text = std::fs::read_to_string(project().root.join("core/src/lab.rs")).unwrap();
    let mut lines = text.lines().enumerate();
    lines
        .find(|(_, l)| l.contains("pub fn explode(reason: String)"))
        .expect("explode");
    let (index, _) = lines
        .find(|(_, l)| l.contains("panic!("))
        .expect("its panic!");
    index as u32 + 1
}

fn harness_source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/symbols/harness.c")
}

fn header_dir() -> PathBuf {
    repo_root().join("runtimes/swift/UndraRuntime/Sources/UndraFFI/include")
}

/// The `.lldbinit` `undra init` writes.
fn lldbinit() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/project/lldbinit")
}

fn scratch(tag: &str) -> PathBuf {
    let dir = repo_root()
        .join("target/undra-cli-projects/scratch-debugging")
        .join(tag);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_text(cmd: &mut Command) -> String {
    let out = cmd.output().expect("the command starts");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "{cmd:?} failed ({}):\n{text}",
        out.status
    );
    text
}

/// Runs a debugger session to its end, at most `seconds`: a debugger that waits for an authorisation
/// nobody can give (macOS asks before one process controls another, and cannot ask without a login
/// session) must fail the test with what it printed, not hang it.
fn run_debugger(cmd: &mut Command, seconds: u64, work: &Path) -> String {
    let log = work.join("debugger.out");
    let file = std::fs::File::create(&log).unwrap();
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(file.try_clone().unwrap())
        .stderr(file)
        .spawn()
        .expect("the debugger starts");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    match status {
        Some(status) => assert!(status.success(), "{cmd:?} failed ({status}):\n{text}"),
        None => panic!(
            "{cmd:?} did not finish in {seconds} s (a debugger needs Developer Tools access to the process: allow it for this terminal, \
and run the test from a logged-in session, not a detached one):\n{text}"
        ),
    }
    text
}

/// What a batch LLDB session printed when it stopped at the breakpoint in `explode`.
fn assert_stopped_in_explode(output: &str, line: u32) {
    let stop = output
        .find("stop reason = breakpoint 1.1")
        .unwrap_or_else(|| panic!("no stop at the breakpoint:\n{output}"));
    // From the stop on (an attach stops first, in dyld, before the breakpoint is reached).
    let after = &output[stop..];
    let frame0 = after
        .lines()
        .find(|l| l.contains("frame #0:"))
        .unwrap_or_else(|| panic!("no frame #0 after the stop in\n{output}"));
    assert!(
        frame0.contains("playground_core::lab::explode")
            && frame0.contains(&format!("lab.rs:{line}")),
        "the stop is not in explode at lab.rs:{line}: {frame0}"
    );
    // The caller is the generated dispatcher of the core, a few lines above `explode` in the same file.
    assert!(
        after
            .lines()
            .any(|l| l.contains("frame #1:") && l.contains("__undra_dispatch_fn_explode")),
        "{output}"
    );
    // The source line is shown: LLDB read the file the DWARF names.
    assert!(after.contains("panic!(\"{reason}\")"), "{output}");
}

#[test]
fn lldb_stops_at_a_rust_line_in_a_debug_core_on_the_host() {
    let _serial = serial();
    if skip_unless(
        cfg!(target_os = "macos"),
        "this test drives the macOS dylib with Xcode's LLDB",
    ) {
        return;
    }
    if skip_unless(
        has_tool("xcrun", "--version"),
        "Xcode's tools (xcrun) are not installed",
    ) {
        return;
    }
    let p = project();
    let line = explode_line();
    run_ok(p.undra().args(["build", "--platform", "host"]));
    let library_dir = p.root.join("build/host");
    assert!(library_dir.join("libplayground_core.dylib").is_file());

    let work = scratch("host");
    let harness = work.join("harness");
    run_text(
        Command::new("xcrun")
            .args(["clang", "-g"])
            .arg(format!("-I{}", header_dir().display()))
            .arg("-DUNDRA_NS=playground_core")
            .arg(harness_source())
            .arg(format!("-L{}", library_dir.display()))
            .args(["-lplayground_core", "-o"])
            .arg(&harness)
            .arg(format!("-Wl,-rpath,{}", library_dir.display())),
    );
    // A method that does not panic runs through the same library: the harness itself works.
    let sum = run_text(Command::new(&harness).arg("add"));
    assert!(sum.contains("sum=42"), "{sum}");

    // The host process stops itself at its start (`UNDRA_HARNESS_WAIT`) and LLDB attaches, as to the
    // simulator process below: `lldb -- harness` (a launch) waits forever on some machines under load
    // for debugserver's own permission checks, which has nothing to do with the core.
    let mut app = Command::new(&harness)
        .arg("explode")
        .env("UNDRA_HARNESS_WAIT", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .expect("the harness starts");
    wait_until_stopped(app.id());
    let attach = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_debugger(
            Command::new("xcrun")
                .args(["lldb", "-b"])
                .args(["-o", &format!("command source {}", lldbinit().display())])
                .args(["-o", &format!("process attach -p {}", app.id())])
                .args(["-o", &format!("breakpoint set -f lab.rs -l {line}")])
                .args(["-o", "continue"])
                .args(["-o", "bt 2"])
                .args(["-o", "frame variable reason"])
                .args(["-o", "detach"]),
            120,
            &work,
        )
    }));
    let _ = app.kill();
    let _ = app.wait();
    let output = attach.unwrap_or_else(|e| std::panic::resume_unwind(e));
    eprintln!("{output}");
    assert!(
        output.contains("Breakpoint 1: ") && !output.contains("no locations (pending)"),
        "the breakpoint set by file and line resolved:\n{output}"
    );
    assert_stopped_in_explode(&output, line);
    // The Rust formatters of `.lldbinit` are loaded: a Rust `String` reads as its text.
    assert!(
        output.contains("reason = \"kaboom\""),
        "the formatters are not loaded:\n{output}"
    );
}

#[test]
fn lldb_stops_at_a_rust_line_in_a_simulator_process_linked_with_the_prelinked_core() {
    let _serial = serial();
    if skip_unless(cfg!(target_os = "macos"), "iOS needs macOS") {
        return;
    }
    if skip_unless(
        has_rust_target("aarch64-apple-ios") && has_rust_target("aarch64-apple-ios-sim"),
        "rustup target add aarch64-apple-ios aarch64-apple-ios-sim",
    ) {
        return;
    }
    if skip_unless(
        has_tool("xcrun", "--version"),
        "Xcode's tools (xcrun) are not installed",
    ) {
        return;
    }
    let Some(udid) = booted_simulator() else {
        skip_unless(
            false,
            "no booted iOS simulator (xcrun simctl boot <device>)",
        );
        return;
    };
    let p = project();
    let line = explode_line();
    // A debug build prelinks with `-all_load` and keeps full DWARF (the debug map names it).
    run_ok(p.undra().args(["build", "--platform", "ios"]));
    let library = p
        .root
        .join("build/ios/PlaygroundCore.xcframework/ios-arm64-simulator/libplayground_core.a");
    let work = scratch("ios");
    let harness = work.join("harness");
    run_text(
        Command::new("xcrun")
            .args([
                "--sdk",
                "iphonesimulator",
                "clang",
                "-arch",
                "arm64",
                "-mios-simulator-version-min=17.0",
                "-g",
            ])
            .arg(format!("-I{}", header_dir().display()))
            .arg("-DUNDRA_NS=playground_core")
            .arg(harness_source())
            .arg(&library)
            .arg("-o")
            .arg(&harness),
    );

    // The app starts in the simulator and waits for a debugger (`simctl spawn -w`); LLDB attaches.
    let log = std::fs::File::create(work.join("app.out")).unwrap();
    let mut app = Command::new("xcrun")
        .args(["simctl", "spawn", "-w", &udid])
        .arg(&harness)
        .arg("explode")
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .stdin(Stdio::null())
        .spawn()
        .expect("simctl spawn starts");
    let pattern = format!("{} explode", harness.display());
    let mut pid = None;
    for _ in 0..100 {
        let found = Command::new("pgrep")
            .args(["-f", &pattern])
            .output()
            .unwrap();
        // The process of the app, not `simctl` (whose command line has the same words).
        pid = String::from_utf8_lossy(&found.stdout)
            .lines()
            .filter_map(|l| l.trim().parse::<u32>().ok())
            .find(|candidate| {
                let comm = Command::new("ps")
                    .args(["-o", "comm=", "-p", &candidate.to_string()])
                    .output()
                    .unwrap();
                String::from_utf8_lossy(&comm.stdout)
                    .trim()
                    .ends_with("harness")
            });
        if pid.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let Some(pid) = pid else {
        let _ = app.kill();
        let _ = app.wait();
        panic!("the app did not start in the simulator");
    };
    let attach = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_debugger(
            Command::new("xcrun")
                .args(["lldb", "-b"])
                .args(["-o", &format!("process attach -p {pid}")])
                .args(["-o", &format!("breakpoint set -f lab.rs -l {line}")])
                .args(["-o", "continue"])
                .args(["-o", "bt 2"])
                .args(["-o", "detach"]),
            120,
            &work,
        )
    }));
    // The attached process runs on to its end once detached; make sure nothing is left behind.
    let _ = Command::new("kill").arg(pid.to_string()).output();
    let _ = app.kill();
    let _ = app.wait();
    let output = attach.unwrap_or_else(|e| std::panic::resume_unwind(e));
    eprintln!("{output}");
    assert_stopped_in_explode(&output, line);
}

/// Waits until process `pid` is stopped (`SIGSTOP`), which is how the harness waits for a debugger.
fn wait_until_stopped(pid: u32) {
    for _ in 0..100 {
        let state = Command::new("ps")
            .args(["-o", "state=", "-p", &pid.to_string()])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default();
        if state.starts_with('T') {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("the harness did not stop itself for the debugger");
}

/// A booted simulator's UDID.
fn booted_simulator() -> Option<String> {
    let out = Command::new("xcrun")
        .args(["simctl", "list", "devices", "booted", "-j"])
        .output()
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    json["devices"]
        .as_object()?
        .values()
        .flat_map(|runtime| runtime.as_array().cloned().unwrap_or_default())
        .find(|d| d["state"] == "Booted")
        .and_then(|d| d["udid"].as_str().map(ToOwned::to_owned))
}

#[test]
fn the_lldbinit_undra_init_writes_loads_the_formatters_and_says_nothing_without_them() {
    // The file is one `script` line that asks the toolchain on this machine for its formatters; a
    // machine without `rustc` on PATH gets an empty stdout, not an error, and no formatters.
    let text = std::fs::read_to_string(lldbinit()).unwrap();
    let script: Vec<&str> = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .collect();
    assert_eq!(script.len(), 1, "{text}");
    assert!(script[0].starts_with("script "), "{}", script[0]);
    for needle in [
        "rustc",
        "--print",
        "sysroot",
        "lldb_lookup.py",
        "lldb_commands",
        "os.path.exists",
    ] {
        assert!(script[0].contains(needle), "{needle}: {}", script[0]);
    }
}
