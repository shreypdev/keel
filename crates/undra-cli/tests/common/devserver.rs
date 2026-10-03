//! A running `undra dev` and a raw WebSocket client for it, shared by the dev integration tests.
#![allow(dead_code)]

use std::cell::Cell;
use std::io::{BufRead, BufReader, Read};
use std::net::TcpStream;
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tungstenite::{Message, WebSocket};
use undra_wire::{Envelope, Kind, Writer};

use super::Project;

/// How long `undra dev` may stay silent: a cold build of the core and everything it depends on, on a slow runner.
const SILENCE: Duration = Duration::from_secs(900);

/// The tests that run `undra dev` take turns, whole. Every one of them builds the core into the same cargo
/// directories, so its builds queue on cargo's locks ("Blocking waiting for file lock on build directory") behind the
/// builds of the others: run side by side on a small machine, the ninth test waited for the eight before it, past any
/// deadline that is meant for one build, and the parallelism bought nothing. In turn, each deadline counts the
/// test's own work, and the first one pays for the cold build that warms the directories for the rest.
static TURN: Mutex<()> = Mutex::new(());

thread_local! {
    /// How many `Dev`s this thread (a test) has alive: only the first takes the turn, so that a test that runs two
    /// servers does not wait for itself.
    static ALIVE: Cell<usize> = const { Cell::new(0) };
}

/// A running `undra dev`.
pub struct Dev {
    /// Held from the first server of a test to the end of the test's last.
    _turn: Option<MutexGuard<'static, ()>>,
    started: Instant,
    pub child: Child,
    pub lines: Receiver<String>,
    pub url: String,
    pub hash: u64,
    pub log: std::sync::Arc<std::sync::Mutex<String>>,
}

impl Dev {
    pub fn start(project: &Project, extra: &[&str]) -> Dev {
        Dev::start_command(project.undra(), extra)
    }

    /// Starts `undra dev` from a prepared `undra -C <project>` command (environment set).
    pub fn start_command(mut cmd: std::process::Command, extra: &[&str]) -> Dev {
        let turn = ALIVE
            .with(|alive| {
                let first = alive.get() == 0;
                alive.set(alive.get() + 1);
                first
            })
            .then(|| TURN.lock().unwrap_or_else(PoisonError::into_inner));
        let started = Instant::now();
        cmd.args(["dev", "--addr", "127.0.0.1:0"])
            .args(extra)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("undra dev starts");
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let log = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let sink = log.clone();
        std::thread::spawn(move || {
            let mut buf = [0_u8; 4096];
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }
                sink.lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        });
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut dev = Dev {
            _turn: turn,
            started,
            child,
            lines,
            url: String::new(),
            hash: 0,
            log,
        };
        // The banner: the URL on a line of its own, then `schema hash   0x...`.
        let deadline = Instant::now() + SILENCE;
        while dev.url.is_empty() || dev.hash == 0 {
            let line = dev.next_line(deadline);
            if line.starts_with("ws://") {
                dev.url = line.trim().to_owned();
            } else if let Some(hash) = line.trim().strip_prefix("schema hash") {
                dev.hash = u64::from_str_radix(hash.trim().trim_start_matches("0x"), 16)
                    .expect("a hex hash");
            }
        }
        dev
    }

    pub fn next_line(&self, deadline: Instant) -> String {
        let left = deadline.saturating_duration_since(Instant::now());
        self.lines.recv_timeout(left).unwrap_or_else(|_| {
            panic!(
                "undra dev printed nothing in time (it has been running for {:.0?}, and was given {SILENCE:?} for a build \
                 on its own; this test's turn began when its server was started); stderr:\n{}",
                self.started.elapsed(),
                self.log.lock().unwrap()
            )
        })
    }

    /// Waits until what `undra dev` wrote to stderr (warnings, the runner's log) contains `needle`.
    pub fn wait_log(&self, needle: &str, limit: Duration) {
        let deadline = Instant::now() + limit;
        loop {
            if self.log.lock().unwrap().contains(needle) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "`{needle}` was not reported; stderr:\n{}",
                self.log.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Waits for a line containing `needle` and returns it.
    pub fn wait_line(&self, needle: &str, limit: Duration) -> String {
        let deadline = Instant::now() + limit;
        loop {
            let line = self.next_line(deadline);
            if line.contains(needle) {
                return line;
            }
        }
    }

    /// Waits for the restart that an edit of the schema causes: the first `Restarted:` line whose schema hash is not `old`.
    /// A restart that leaves the schema as it was (a rebuild of what had not changed, which a loaded machine can add: the
    /// watcher sees a save in two bursts further apart than its debounce, and the first rebuild reads the file before the
    /// write) is not the one under test, and is skipped, said on stderr.
    pub fn wait_restart_changing_schema(&self, old: u64, limit: Duration) -> String {
        let deadline = Instant::now() + limit;
        loop {
            let line = self.wait_line(
                "Restarted: ws://",
                deadline.saturating_duration_since(Instant::now()),
            );
            let hash = line
                .split("schema hash ")
                .nth(1)
                .and_then(|rest| rest.split(')').next())
                .and_then(|hash| u64::from_str_radix(hash.trim_start_matches("0x"), 16).ok());
            if hash.is_some_and(|hash| hash != old) {
                return line;
            }
            eprintln!("(skipped a restart that left the schema as it was: {line})");
        }
    }

    /// Waits for a line containing `needle`.
    pub fn wait_for(&self, needle: &str, limit: Duration) {
        let deadline = Instant::now() + limit;
        loop {
            if self.next_line(deadline).contains(needle) {
                return;
            }
        }
    }

    pub fn addr(&self) -> String {
        self.url.trim_start_matches("ws://").to_owned()
    }

    /// Stops `undra dev` the hard way (as a closed terminal or a crash would) and checks that the
    /// core it was serving goes away with it.
    pub fn kill_and_expect_the_port_to_close(mut self) {
        let addr = self.addr();
        let _ = self.child.kill();
        let _ = self.child.wait();
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if TcpStream::connect(&addr).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("the dev runner outlived `undra dev`: {addr} still accepts connections");
    }
}

impl Drop for Dev {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        ALIVE.with(|alive| alive.set(alive.get().saturating_sub(1)));
    }
}

pub fn send(ws: &mut WebSocket<TcpStream>, schema: u64, kind: Kind, seq: u32, payload: &[u8]) {
    let mut w = Writer::new();
    Envelope::write(&mut w, kind, seq, schema, payload);
    ws.send(Message::Binary(w.into_vec())).unwrap();
}
