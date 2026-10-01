//! `undra dev`: serve the template core, talk to it the way a platform runtime does (a raw
//! WebSocket client speaking the envelope of docs/SPEC.md 3.2), change the core, and watch it
//! come back.

mod common;

use std::io::{BufRead, BufReader, Read};
use std::net::TcpStream;
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use common::{Project, init_project};
use tungstenite::{Message, WebSocket};
use undra_wire::payload::{Call, CallTarget, Hello, Reply};
use undra_wire::{Decode, Encode, Envelope, Kind, Reader, Writer};

/// A running `undra dev`.
struct Dev {
    child: Child,
    lines: Receiver<String>,
    url: String,
    hash: u64,
    log: std::sync::Arc<std::sync::Mutex<String>>,
}

impl Dev {
    fn start(project: &Project, extra: &[&str]) -> Dev {
        let mut cmd = project.undra();
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
            child,
            lines,
            url: String::new(),
            hash: 0,
            log,
        };
        // The banner: the URL on a line of its own, then `schema hash   0x...`.
        let deadline = Instant::now() + Duration::from_secs(600);
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

    fn next_line(&self, deadline: Instant) -> String {
        let left = deadline.saturating_duration_since(Instant::now());
        self.lines.recv_timeout(left).unwrap_or_else(|_| {
            panic!(
                "undra dev printed nothing in time; stderr:\n{}",
                self.log.lock().unwrap()
            )
        })
    }

    /// Waits for a line containing `needle`.
    fn wait_for(&self, needle: &str, limit: Duration) {
        let deadline = Instant::now() + limit;
        loop {
            if self.next_line(deadline).contains(needle) {
                return;
            }
        }
    }

    fn addr(&self) -> String {
        self.url.trim_start_matches("ws://").to_owned()
    }

    /// Stops `undra dev` the hard way (as a closed terminal or a crash would) and checks that the
    /// core it was serving goes away with it.
    fn kill_and_expect_the_port_to_close(mut self) {
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
    }
}

fn send(ws: &mut WebSocket<TcpStream>, schema: u64, kind: Kind, seq: u32, payload: &[u8]) {
    let mut w = Writer::new();
    Envelope::write(&mut w, kind, seq, schema, payload);
    ws.send(Message::Binary(w.into_vec())).unwrap();
}

/// Connects as a web client would and completes the handshake; returns the socket and the schema
/// hash the server reported in its Hello.
fn connect(dev: &Dev) -> (WebSocket<TcpStream>, u64) {
    let tcp = TcpStream::connect(dev.addr()).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let (mut ws, _) = tungstenite::client(dev.url.as_str(), tcp).expect("the WebSocket upgrade");
    let mut hello = Writer::new();
    Hello {
        undra_version: "0.1.0",
        schema_hash: dev.hash,
        platform: "test",
        mode: "dev",
    }
    .encode(&mut hello);
    send(&mut ws, dev.hash, Kind::Hello, 0, hello.as_slice());
    let Message::Binary(reply) = ws.read().unwrap() else {
        panic!("binary envelopes only")
    };
    let envelope = Envelope::parse(&reply).unwrap();
    assert_eq!(
        envelope.kind,
        Kind::Hello,
        "the server answers a Hello with its own"
    );
    (ws, envelope.schema)
}

/// Calls the free function `greeting(name)` and returns its reply.
fn greeting(ws: &mut WebSocket<TcpStream>, schema: u64, name: &str) -> String {
    let mut args = Writer::new();
    name.to_owned().encode(&mut args);
    let target = CallTarget::Function {
        method_id: undra_meta::ids::function_id("greeting"),
    };
    let mut call = Writer::new();
    Call {
        target,
        call_id: 1,
        args: args.as_slice(),
    }
    .encode(&mut call);
    send(ws, schema, Kind::Call, 1, call.as_slice());
    loop {
        let Message::Binary(bytes) = ws.read().expect("a reply") else {
            continue;
        };
        let envelope = Envelope::parse(&bytes).unwrap();
        if envelope.kind == Kind::Reply {
            let reply = Reply::decode(&mut Reader::new(envelope.payload)).unwrap();
            return String::decode_exact(reply.body).unwrap();
        }
    }
}

#[test]
fn the_dev_server_serves_the_core_and_reports_its_schema() {
    let project = init_project("devsmoke", "web");
    let dev = Dev::start(&project, &["--no-watch"]);

    // The hash `undra dev` printed is the hash of the bindings `undra init` generated.
    let ids = std::fs::read_to_string(project.root.join("generated/ts/src/ids.ts")).unwrap();
    assert!(
        ids.contains(&format!("schemaHash: {:#018x}n", dev.hash)),
        "{ids}"
    );
    assert!(dev.url.starts_with("ws://127.0.0.1:"), "{}", dev.url);

    let (mut ws, server_hash) = connect(&dev);
    assert_eq!(
        server_hash, dev.hash,
        "the server's Hello carries the core's schema hash"
    );
    assert_eq!(
        greeting(&mut ws, dev.hash, "Ada"),
        "Hello, Ada, from the devsmoke core"
    );
    drop(ws);

    // The core's dev records reach the terminal (docs/SPEC.md 5.10).
    std::thread::sleep(Duration::from_millis(300));
    let log = dev.log.lock().unwrap().clone();
    assert!(
        log.contains("undra::transport: serving on ws://127.0.0.1:"),
        "{log}"
    );
    assert!(log.contains("client connected: platform=test"), "{log}");

    dev.kill_and_expect_the_port_to_close();
}

#[test]
fn an_edit_rebuilds_and_restarts_the_core_on_the_same_address() {
    let project = init_project("devwatch", "web");
    let dev = Dev::start(&project, &[]);
    let (mut ws, hash) = connect(&dev);
    assert_eq!(
        greeting(&mut ws, hash, "x"),
        "Hello, x, from the devwatch core"
    );
    drop(ws);

    // A broken edit keeps the old core serving.
    let lib = project.root.join("core/src/lib.rs");
    let original = std::fs::read_to_string(&lib).unwrap();
    std::fs::write(&lib, format!("{original}\npub fn broken( {{\n")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if dev
            .log
            .lock()
            .unwrap()
            .contains("the rebuild failed; still serving the previous build")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no rebuild failure was reported:\n{}",
            dev.log.lock().unwrap()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    let (mut ws, hash) = connect(&dev);
    assert_eq!(
        greeting(&mut ws, hash, "x"),
        "Hello, x, from the devwatch core",
        "the previous core still answers"
    );
    drop(ws);

    // A good edit is picked up: same address, new behaviour.
    std::fs::write(
        &lib,
        original.replace("from the devwatch core", "again, from the rebuilt core"),
    )
    .unwrap();
    dev.wait_for("Restarted: ws://", Duration::from_secs(180));
    // The banner line carries the same URL: clients reconnect where they were.
    let (mut ws, hash) = connect(&dev);
    assert_eq!(
        greeting(&mut ws, hash, "x"),
        "Hello, x, again, from the rebuilt core"
    );
    drop(ws);
    dev.kill_and_expect_the_port_to_close();
}

#[test]
fn a_taken_address_is_explained() {
    let project = init_project("devbusy", "web");
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = taken.local_addr().unwrap().to_string();
    let out = project
        .undra()
        .args(["dev", "--no-watch", "--addr", &addr])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("error[undra::C0013]")
            && stderr.contains("could not start")
            && stderr.contains("--addr 127.0.0.1:0"),
        "{stderr}"
    );
}
