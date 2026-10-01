//! `undra dev`: serve the core over a WebSocket and rebuild it when the code changes.
//!
//! The core runs in the dev runner (a child process, see [`crate::runner`]); this command builds
//! it, starts it, watches the core's sources and, on a change, rebuilds and swaps the running
//! runner for the new one on the same address; the clients reconnect by themselves (ADR-051).
//! While a rebuild is failing the old core keeps serving: a typo does not take the app down.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};

use crate::adb;
use crate::cli::DevArgs;
use crate::config::Platform;
use crate::error::{CliError, Code, Result};
use crate::runner::{self, RunnerEvent, Running};
use crate::session::Session;

use super::Env;

/// How long file events are collected before a rebuild starts (editors write a file in several
/// steps, and a `git checkout` touches many).
const DEBOUNCE: Duration = Duration::from_millis(300);

/// How long the runner has to start listening.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// Everything the loop waits for.
enum Event {
    /// Something in the watched sources changed.
    Changed,
    /// The runner said something.
    Runner(RunnerEvent),
}

/// Runs `undra dev`.
///
/// # Errors
///
/// `C0013` when the server cannot start; build errors as for `undra build`.
pub fn run(env: &Env<'_>, args: &DevArgs) -> Result<()> {
    let session = env.session()?;
    let ui = env.ui;
    let core = session.core()?.clone();

    ui.step(&format!("Building the dev server for {}", core.package));
    let started = Instant::now();
    let mut exe = runner::build(&session)?;
    ui.detail(&format!("built in {:.1}s", started.elapsed().as_secs_f32()));

    let (tx, rx) = mpsc::channel::<Event>();
    let (runner_tx, runner_rx) = mpsc::channel::<RunnerEvent>();
    forward(runner_rx, tx.clone());

    let mut run_id = 0_u64;
    let mut addr = args.addr.clone();
    let (first, ready, _) = start(&exe, &addr, args.log_level, &mut run_id, &runner_tx, &rx)?;
    let mut running = (first, ready);
    let (mut url, hash) = running.1.clone();

    // The watcher lives as long as the loop. It is running before the banner is printed: the
    // banner says "Watching", and a save right after it (an editor, a test) must not be missed.
    let _watcher = if args.no_watch {
        None
    } else {
        Some(watch(&core.local_dirs, tx.clone())?)
    };
    announce(&session, &url, &hash, args, &core.local_dirs, None);
    // A port of 0 asked the OS to choose; keep that port across restarts so clients find the
    // server where they left it.
    addr = socket_of(&url).unwrap_or(addr);
    let mut hash = hash;
    if args.android {
        reverse_android(&session, &url);
    }

    loop {
        match rx.recv() {
            Ok(Event::Changed) => {
                // Collect the burst of events an editor save makes.
                loop {
                    match rx.recv_timeout(DEBOUNCE) {
                        Ok(Event::Changed) => {}
                        Ok(Event::Runner(event)) => {
                            if let Some(done) = handle_runner_event(&event, &running.0) {
                                return done;
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => return Ok(()),
                    }
                }
                ui.step("Change detected, rebuilding");
                let started = Instant::now();
                match runner::build(&session) {
                    Ok(new_exe) => {
                        exe = new_exe;
                        ui.detail(&format!("built in {:.1}s", started.elapsed().as_secs_f32()));
                        // Stop the old core first: the new one takes its address.
                        let (old, _) = running;
                        old.stop();
                        drain_closed(&rx);
                        let (next, ready, changed) =
                            start(&exe, &addr, args.log_level, &mut run_id, &runner_tx, &rx)?;
                        running = (next, ready);
                        if changed {
                            let _ = tx.send(Event::Changed);
                        }
                        let (new_url, new_hash) = running.1.clone();
                        url = new_url;
                        announce(
                            &session,
                            &url,
                            &new_hash,
                            args,
                            &core.local_dirs,
                            Some(&hash),
                        );
                        hash = new_hash;
                        if args.android {
                            reverse_android(&session, &url);
                        }
                    }
                    Err(e) => {
                        ui.warn(&format!(
                            "the rebuild failed; still serving the previous build at {url}\n{}",
                            e.what
                        ));
                    }
                }
            }
            Ok(Event::Runner(event)) => {
                if let Some(done) = handle_runner_event(&event, &running.0) {
                    return done;
                }
            }
            Err(_) => return Ok(()),
        }
    }
}

/// Reacts to a runner event during serving: its own lines are printed, its exit ends the command.
fn handle_runner_event(event: &RunnerEvent, current: &Running) -> Option<Result<()>> {
    match event {
        RunnerEvent::Line { id, text } if *id == current.id => {
            println!("{text}");
            None
        }
        RunnerEvent::Closed { id } if *id == current.id => Some(Err(CliError::new(
            Code::Dev,
            "the dev server stopped",
            "the process that runs the core exited on its own, usually because the core aborted (see the output above)",
            "fix what the output says and run `undra dev` again",
        ))),
        _ => None,
    }
}

/// Starts a runner and waits for it to listen. Returns it with its URL and schema hash, and
/// whether a source changed while it started (the caller queues that change again: it may not be
/// in the build that was just started).
fn start(
    exe: &Path,
    addr: &str,
    log_level: u8,
    run_id: &mut u64,
    runner_tx: &Sender<RunnerEvent>,
    rx: &Receiver<Event>,
) -> Result<(Running, (String, String), bool)> {
    *run_id += 1;
    let id = *run_id;
    let running = runner::spawn(exe, addr, log_level, id, runner_tx.clone())?;
    let deadline = Instant::now() + READY_TIMEOUT;
    let mut changed = false;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(Event::Runner(RunnerEvent::Ready { id: got, url, hash })) if got == id => {
                return Ok((running, (url, hash), changed));
            }
            Ok(Event::Changed) => changed = true,
            Ok(Event::Runner(RunnerEvent::Closed { id: got })) if got == id => {
                return Err(CliError::new(
                    Code::Dev,
                    format!("the dev server could not start on {addr}"),
                    "the process that runs the core exited before it listened (its message is above); the usual cause is that the address is already in use, often by another `undra dev`",
                    "stop the other server, or choose another port: `undra dev --addr 127.0.0.1:0` picks a free one",
                ));
            }
            Ok(Event::Runner(RunnerEvent::Line { id: got, text })) if got == id => {
                println!("{text}")
            }
            Ok(_) => {}
            Err(_) => {
                running.stop();
                return Err(CliError::new(
                    Code::Dev,
                    format!(
                        "the dev server did not start listening within {} seconds",
                        READY_TIMEOUT.as_secs()
                    ),
                    "the core's startup (a constructor, a hydration hook) is taking too long or is stuck",
                    "run the core's tests to find what blocks, then run `undra dev` again",
                ));
            }
        }
    }
}

/// Discards the `Closed` event of a runner that was stopped on purpose.
fn drain_closed(rx: &Receiver<Event>) {
    while let Ok(event) = rx.try_recv() {
        let _ = event;
    }
}

/// Forwards runner events into the main channel.
fn forward(from: Receiver<RunnerEvent>, to: Sender<Event>) {
    std::thread::spawn(move || {
        for event in from {
            if to.send(Event::Runner(event)).is_err() {
                return;
            }
        }
    });
}

/// The `host:port` of a `ws://host:port` URL.
fn socket_of(url: &str) -> Option<String> {
    url.strip_prefix("ws://").map(ToOwned::to_owned)
}

/// Starts watching `dirs` (their `src/`, `Cargo.toml` and `build.rs`).
fn watch(dirs: &[PathBuf], tx: Sender<Event>) -> Result<notify::RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else { return };
        if !matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) {
            return;
        }
        if event.paths.iter().any(|p| is_source(p)) {
            let _ = tx.send(Event::Changed);
        }
    })
    .map_err(|e| watch_error(&e))?;
    for dir in dirs {
        let src = dir.join("src");
        if src.is_dir() {
            watcher
                .watch(&src, RecursiveMode::Recursive)
                .map_err(|e| watch_error(&e))?;
        }
        for file in ["Cargo.toml", "build.rs"] {
            let path = dir.join(file);
            if path.is_file() {
                watcher
                    .watch(&path, RecursiveMode::NonRecursive)
                    .map_err(|e| watch_error(&e))?;
            }
        }
    }
    Ok(watcher)
}

fn watch_error(e: &notify::Error) -> CliError {
    CliError::new(
        Code::Dev,
        format!("cannot watch the core's sources: {e}"),
        "`undra dev` rebuilds when a file changes, which needs file-system notifications",
        "use `undra dev --no-watch` and restart it yourself after a change (on Linux, raise fs.inotify.max_user_watches if that is the error)",
    )
}

/// Whether a changed path is one that should trigger a rebuild: Rust sources and manifests, not
/// editor swap files or build output.
fn is_source(path: &Path) -> bool {
    if path.components().any(|c| c.as_os_str() == "target") {
        return false;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.starts_with('.') || name.ends_with('~') || name.ends_with(".swp") {
        return false;
    }
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("rs" | "toml")
    ) || name == "Cargo.lock"
}

/// The host and port of a `ws://host:port` URL's socket.
fn port_of(url: &str) -> Option<u16> {
    socket_of(url)?.rsplit(':').next()?.parse().ok()
}

/// Whether the project has an Android app (the Android lines of the banner are for it).
fn has_android(session: &Session<'_>) -> bool {
    session
        .project
        .config
        .platforms
        .contains(&Platform::Android)
}

/// How an Android emulator reaches this server: `10.0.2.2` is the emulator's name for the host's
/// loopback interface, so the port is all that carries over.
fn emulator_url(url: &str) -> Option<String> {
    Some(format!("ws://10.0.2.2:{}", port_of(url)?))
}

/// Prints the banner: where to connect and how. `previous` is the schema hash of the core that
/// ran before, on a restart.
fn announce(
    session: &Session<'_>,
    url: &str,
    hash: &str,
    args: &DevArgs,
    watched: &[PathBuf],
    previous: Option<&str>,
) {
    let say = |s: &str| println!("{s}");
    if let Some(before) = previous {
        say("");
        say(&format!(
            "Restarted: {url}  (schema hash {hash}); connected apps reconnect by themselves"
        ));
        if before != hash {
            say(&format!(
                "  The schema changed (was {before}): an app built from the old bindings reports a schema mismatch and stops. Run `undra bindgen`, rebuild the app, relaunch it."
            ));
        }
        return;
    }
    say("");
    say("Undra dev server is running.");
    say("");
    say(url);
    say("");
    say(&format!("  schema hash   {hash}"));
    say(
        "  web           UndraCore.load({ mode: \"remote\", url, expectedSchemaHash: UndraIds.schemaHash })  or ?undra=<url> in the page URL",
    );
    say("  iOS           launch the app with UNDRA_DEV_URL set to the URL above");
    if has_android(session) {
        if let (Some(emulator), Some(port)) = (emulator_url(url), port_of(url)) {
            say(&format!("  Android       emulator  {emulator}"));
            say(&format!(
                "                device    adb reverse tcp:{port} tcp:{port}  (`undra dev --android` runs it), then {}",
                url.replace("0.0.0.0", "127.0.0.1")
            ));
            say(&format!(
                "                launch    adb shell am start -n {}/.MainActivity --es undra_dev_url {emulator}",
                session.project.config.id
            ));
        }
    }
    say("  JVM           LoadOptions(mode = REMOTE, remoteUrl = url)");
    if args.addr.starts_with("0.0.0.0") || args.addr.starts_with("[::]") {
        say(
            "  (listening on every interface: any device on your network can use this core, there is no authentication)",
        );
    }
    say("");
    say("Apps reconnect by themselves when the connection drops or the core is rebuilt.");
    if args.no_watch {
        say("Serving. Press Ctrl-C to stop.");
    } else {
        let shown: Vec<String> = watched
            .iter()
            .map(|d| d.join("src").display().to_string())
            .collect();
        say(&format!(
            "Watching {} ; press Ctrl-C to stop.",
            shown.join(", ")
        ));
    }
    say("");
}

/// `undra dev --android`: `adb reverse` for the server's port on every attached device, and a
/// line each about what happened (a device that is not ready, or none at all, is not an error:
/// the emulator does not need it).
fn reverse_android(session: &Session<'_>, url: &str) {
    let Some(port) = port_of(url) else { return };
    let only = session.sys.env("ANDROID_SERIAL");
    let result = adb::reverse_all(session.sys, &session.toolchain, port, only.as_deref());
    for done in &result.reversed {
        match &done.result {
            Ok(()) => println!(
                "adb reverse   {}: tcp:{port} reaches this server (ws://127.0.0.1:{port} on the device)",
                done.serial
            ),
            Err(why) => session
                .ui
                .warn(&format!("adb reverse failed for {}: {why}", done.serial)),
        }
    }
    for device in &result.not_ready {
        session.ui.warn(&format!(
            "{} is {}: `adb reverse` was skipped (unlock it and accept the debugging prompt, then save a file or restart `undra dev`)",
            device.serial, device.state
        ));
    }
    if let Some(problem) = &result.problem {
        session.ui.warn(&format!(
            "--android: {problem}. An emulator does not need it: it reaches this server at ws://10.0.2.2:{port}."
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_sources_trigger_rebuilds() {
        for yes in [
            "/p/core/src/lib.rs",
            "/p/core/Cargo.toml",
            "/p/core/src/a/b.rs",
            "/p/Cargo.lock",
        ] {
            assert!(is_source(Path::new(yes)), "{yes}");
        }
        for no in [
            "/p/core/target/debug/x.rs",
            "/p/core/src/.lib.rs.swp",
            "/p/core/src/lib.rs~",
            "/p/core/src/.#lib.rs",
            "/p/core/README.md",
            "/p/core/src/data.json",
        ] {
            assert!(!is_source(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn the_socket_is_taken_from_the_url() {
        assert_eq!(
            socket_of("ws://127.0.0.1:7443").as_deref(),
            Some("127.0.0.1:7443")
        );
        assert_eq!(socket_of("http://x"), None);
    }

    #[test]
    fn the_port_and_the_emulator_address_come_from_the_url() {
        assert_eq!(port_of("ws://127.0.0.1:7443"), Some(7443));
        assert_eq!(port_of("ws://[::1]:9000"), Some(9000));
        assert_eq!(port_of("http://x:1"), None);
        assert_eq!(
            emulator_url("ws://127.0.0.1:7443").as_deref(),
            Some("ws://10.0.2.2:7443")
        );
        assert_eq!(emulator_url("nonsense"), None);
    }
}
