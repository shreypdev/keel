//! `undra dev`: serve the core over a WebSocket and rebuild it when the code changes.
//!
//! The core runs in the dev runner (a child process, see [`crate::runner`]); this command builds
//! it, starts it, watches the core's sources and, on a change, rebuilds and swaps the running
//! runner for the new one on the same address, so a client only has to reconnect. While a rebuild
//! is failing the old core keeps serving: a typo does not take the app down.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};

use crate::cli::DevArgs;
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
    announce(&session, &url, &hash, args, &core.local_dirs, false);
    // A port of 0 asked the OS to choose; keep that port across restarts so clients find the
    // server where they left it.
    addr = socket_of(&url).unwrap_or(addr);

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
                        announce(&session, &url, &new_hash, args, &core.local_dirs, true);
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

/// Prints the banner: where to connect and how.
fn announce(
    _session: &Session<'_>,
    url: &str,
    hash: &str,
    args: &DevArgs,
    watched: &[PathBuf],
    restarted: bool,
) {
    let say = |s: &str| println!("{s}");
    if restarted {
        say("");
        say(&format!(
            "Restarted: {url}  (schema hash {hash}); reload the app to reconnect"
        ));
        return;
    }
    say("");
    say("Undra dev server is running.");
    say("");
    say(url);
    say("");
    say(&format!("  schema hash   {hash}"));
    say(
        "  web           UndraCore.load({ mode: \"remote\", url, expectedSchemaHash: UndraIds.schemaHash })",
    );
    say("  iOS           launch the app with UNDRA_DEV_URL set to the URL above");
    say("  JVM           LoadOptions(mode = REMOTE, remoteUrl = url)");
    if args.addr.starts_with("0.0.0.0") || args.addr.starts_with("[::]") {
        say(
            "  (listening on every interface: any device on your network can use this core, there is no authentication)",
        );
    }
    say("");
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
}
