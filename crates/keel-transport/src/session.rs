//! One connection's reader thread: the WebSocket upgrade, the Hello exchange, the loop that
//! turns client envelopes into runtime calls, and the teardown that gives back what the
//! client held.
//!
//! # The contract with the client runtimes
//!
//! The three shipped clients (`runtimes/ts`, `runtimes/kotlin`, `runtimes/swift`) define what
//! this server must do; where they and `docs/SPEC.md` differ, the clients win.
//!
//! * **The client speaks first.** Each sends `Hello` as soon as the socket opens and waits for
//!   the server's `Hello`; the server answers a `Hello` with its own, always, before anything
//!   else (the client reports a schema mismatch from that message, so it must be readable).
//!   The server's Hello is envelope sequence 0.
//! * **Schema.** A client whose hash differs gets the server's `Hello` and then a Close frame
//!   (1008). After the handshake every envelope in both directions carries the core's hash;
//!   the server closes (1008) on a client envelope that does not.
//! * **Sequence numbers** are per direction and increasing. TypeScript and Kotlin count from
//!   0, Swift from 1, so the server *does not validate* client sequence numbers; its own start
//!   at 0 and have no gaps.
//! * **Ignored, not fatal:** a repeated `Hello`, and core-to-host kinds sent by the client.
//!   Fatal (Close 1002): anything that does not parse.

use std::collections::HashSet;
use std::io::{self, Read};
use std::net::TcpStream;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use keel_runtime::log::{DEBUG, ERROR, FATAL, INFO, WARN};
use keel_runtime::Runtime;
use keel_wire::payload::{
    Call, CallTarget, Cancel, Event, Hello, Observe, PortReply, PortStatus, Release, StreamCredit,
    TimerFired,
};
use keel_wire::{Envelope, Kind, Reader, WireError, Writer};
use tungstenite::Message;
use tungstenite::error::{Error, ProtocolError};

use crate::bridge::{Bridge, ClientInfo};
use crate::conn::{Conn, Item};
use crate::server::{Shared, ServerConfig};
use crate::ws::{self, ReadHalf, close};
use crate::writer;

/// The `target` of the server's own log records.
const TARGET: &str = "keel::transport";

/// The keel version the server announces in its `Hello`.
pub const KEEL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Why a session must end: the Close frame's code and reason.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Violation {
    pub(crate) code: u16,
    pub(crate) reason: String,
}

impl Violation {
    fn protocol(reason: String) -> Violation {
        Violation {
            code: close::PROTOCOL_ERROR,
            reason,
        }
    }

    fn schema(ours: u64, theirs: u64) -> Violation {
        Violation {
            code: close::POLICY_VIOLATION,
            reason: format!("schema mismatch: core {ours:#018x}, client {theirs:#018x}"),
        }
    }
}

/// Decodes `payload` with `f` and requires that it is consumed exactly.
fn decode<'a, T>(
    payload: &'a [u8],
    what: &str,
    f: impl FnOnce(&mut Reader<'a>) -> Result<T, WireError>,
) -> Result<T, Violation> {
    let mut r = Reader::new(payload);
    f(&mut r)
        .and_then(|value| r.finish().map(|()| value))
        .map_err(|e| Violation::protocol(format!("malformed {what} payload: {e}")))
}

/// What the reader thread does with one client's envelopes.
pub(crate) struct Session {
    rt: Arc<Runtime>,
    bridge: Arc<Bridge>,
    conn: Arc<Conn>,
    release_on_disconnect: bool,
    busy_grace: Duration,
}

impl Session {
    pub(crate) fn new(
        rt: Arc<Runtime>,
        bridge: Arc<Bridge>,
        conn: Arc<Conn>,
        config: &ServerConfig,
    ) -> Session {
        Session {
            rt,
            bridge,
            conn,
            release_on_disconnect: config.release_on_disconnect,
            busy_grace: config.busy_grace,
        }
    }

    fn note(&self, level: u8, message: &str) {
        self.rt.log(level, TARGET, message);
    }

    /// The server's own `Hello`: its version, schema hash, platform and mode.
    fn send_hello(&self) {
        let config = self.rt.config();
        let hello = Hello {
            keel_version: KEEL_VERSION,
            schema_hash: self.rt.schema_hash(),
            platform: &config.platform,
            mode: &config.mode,
        };
        self.conn.send_with(Kind::Hello, 32, |w| hello.encode(w));
    }

    /// The first message of a connection. `Ok` means the client is attached and the loop
    /// continues with [`on_frame`](Session::on_frame); `Err` means the connection is refused
    /// (after the server's own Hello, so the client can say why).
    pub(crate) fn on_hello(&self, data: &[u8]) -> Result<(), Violation> {
        let env = Envelope::parse(data)
            .map_err(|e| Violation::protocol(format!("malformed envelope: {e}")))?;
        if env.kind != Kind::Hello {
            return Err(Violation::protocol(format!(
                "expected Hello as the first message, got {:?}",
                env.kind
            )));
        }
        let hello = decode(env.payload, "Hello", Hello::decode)?;
        // Answered whatever happens next: a client that is about to be refused reads the
        // reason from the server's schema hash.
        self.send_hello();
        let ours = self.rt.schema_hash();
        if hello.schema_hash != ours {
            self.note(
                WARN,
                &format!(
                    "refused a {} client: schema {:#018x}, core {ours:#018x}",
                    hello.platform, hello.schema_hash
                ),
            );
            return Err(Violation::schema(ours, hello.schema_hash));
        }
        let info = ClientInfo {
            keel_version: hello.keel_version.to_owned(),
            platform: hello.platform.to_owned(),
            mode: hello.mode.to_owned(),
        };
        self.conn.set_client(info.clone());
        if !self.bridge.claim(&self.conn, self.busy_grace) {
            self.note(
                INFO,
                &format!("refused a second client ({}): one is already attached", info.platform),
            );
            return Err(Violation {
                code: close::TRY_AGAIN_LATER,
                reason: "another client is already connected; keel dev serves one at a time"
                    .to_owned(),
            });
        }
        self.note(
            INFO,
            &format!(
                "client connected: platform={} mode={} keel={}",
                info.platform, info.mode, info.keel_version
            ),
        );
        Ok(())
    }

    /// One envelope from an attached client.
    pub(crate) fn on_frame(&self, data: &[u8]) -> Result<(), Violation> {
        let env = Envelope::parse(data)
            .map_err(|e| Violation::protocol(format!("malformed envelope: {e}")))?;
        let ours = self.rt.schema_hash();
        if env.schema != ours {
            return Err(Violation::schema(ours, env.schema));
        }
        let payload = env.payload;
        match env.kind {
            Kind::Call => self.on_call(payload)?,
            Kind::Cancel => {
                let cancel = decode(payload, "Cancel", Cancel::decode)?;
                self.rt.cancel(cancel.call_id);
                self.conn.end_call(cancel.call_id);
            }
            Kind::StreamCredit => {
                let credit = decode(payload, "StreamCredit", StreamCredit::decode)?;
                self.rt.stream_credit(credit.call_id, credit.credit);
            }
            Kind::Observe => {
                let observe = decode(payload, "Observe", Observe::decode)?;
                self.conn
                    .observe(observe.handle.0, observe.signal_id, observe.on);
                self.rt
                    .observe(observe.handle.0, observe.signal_id, observe.on);
            }
            Kind::Release => {
                let release = decode(payload, "Release", Release::decode)?;
                self.conn.release(release.handle.0);
                self.rt.release(release.handle.0);
            }
            Kind::PortReply => {
                let reply = decode(payload, "PortReply", PortReply::decode)?;
                self.conn.port_call_answered(reply.port_call_id);
                self.rt.port_reply(payload);
            }
            Kind::Event => {
                let event = decode(payload, "Event", Event::decode)?;
                self.rt.event(event.port_id, event.method_id, event.payload);
            }
            Kind::TimerFired => {
                let fired = decode(payload, "TimerFired", TimerFired::decode)?;
                self.rt.timer_fired(fired.timer_id);
            }
            Kind::Restore => {
                if let Err(e) = self.rt.restore(payload) {
                    self.note(ERROR, &format!("the client's Restore was refused: {e}"));
                }
            }
            Kind::Hello => self.note(DEBUG, "ignoring a repeated Hello"),
            Kind::Reply
            | Kind::ChangeSet
            | Kind::PortCall
            | Kind::StreamItem
            | Kind::Log
            | Kind::Snapshot => self.note(
                WARN,
                &format!("ignoring a {:?} message: only the core sends those", env.kind),
            ),
        }
        Ok(())
    }

    fn on_call(&self, payload: &[u8]) -> Result<(), Violation> {
        let call = decode(payload, "Call", Call::decode)?;
        let constructor = matches!(call.target, CallTarget::Constructor { .. });
        // Recorded before the runtime sees it: a sync method replies before `call` returns.
        if !self.conn.begin_call(call.call_id, constructor) {
            self.note(
                WARN,
                &format!("ignoring a call that reuses the open call id {}", call.call_id),
            );
            return Ok(());
        }
        if self.rt.call(payload) != 0 {
            // Refused without a reply (call id 0, a runtime shutting down): answer for it, or
            // the client would wait for ever.
            self.conn.end_call(call.call_id);
            self.conn.send_bad_request(
                call.call_id,
                "the core refused the call (the call id is 0 or the core is shutting down)",
            );
        }
        Ok(())
    }

    /// Gives back what the client held: cancels its calls, stops its observations, releases
    /// the objects its constructors made and fails the port calls it will never answer.
    /// Idempotent.
    ///
    /// Releasing is not something the protocol asks for (no client releases at disconnect and
    /// none reconnects to reuse a handle), but a dev core outlives many app launches and would
    /// otherwise keep every launch's stores, observers included, for ever.
    pub(crate) fn teardown(&self) {
        let had_client = self.conn.client().is_some();
        let left = self.conn.drain();
        for call_id in &left.calls {
            self.rt.cancel(*call_id);
        }
        let owned: HashSet<u64> = if self.release_on_disconnect {
            left.constructed.iter().copied().collect()
        } else {
            HashSet::new()
        };
        for (handle, signal) in &left.observed {
            if !owned.contains(handle) {
                self.rt.observe(*handle, *signal, false);
            }
        }
        for handle in &left.constructed {
            if self.release_on_disconnect {
                self.rt.release(*handle);
            }
        }
        for id in &left.port_calls {
            let mut w = Writer::with_capacity(5);
            PortReply {
                port_call_id: *id,
                status: PortStatus::Unavailable,
                body: &[],
            }
            .encode(&mut w);
            self.rt.port_reply(w.as_slice());
        }
        let attached = self.bridge.is_attached(self.conn.id);
        self.bridge.vacate(self.conn.id);
        if attached && had_client {
            self.note(
                INFO,
                &format!(
                    "client disconnected ({} calls cancelled, {} handles released)",
                    left.calls.len(),
                    if self.release_on_disconnect { left.constructed.len() } else { 0 }
                ),
            );
        }
    }
}

/// Runs one accepted socket to the end. Never panics.
pub(crate) fn run(shared: &Arc<Shared>, id: u64, tcp: TcpStream) {
    let outcome = catch_unwind(AssertUnwindSafe(|| serve(shared, id, tcp)));
    if let Err(panic) = outcome {
        let message = panic
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a non-string panic".to_owned());
        shared
            .rt
            .log(FATAL, TARGET, &format!("a connection thread panicked: {message}"));
    }
    shared.detach(id);
}

/// Everything that can fail with an `io::Error` before the session proper starts.
fn serve(shared: &Arc<Shared>, id: u64, tcp: TcpStream) {
    let config = &shared.config;
    let prepared = (|| -> io::Result<(TcpStream, TcpStream, TcpStream)> {
        tcp.set_nodelay(true)?;
        tcp.set_read_timeout(Some(config.handshake_timeout))?;
        tcp.set_write_timeout(Some(config.write_timeout))?;
        Ok((tcp.try_clone()?, tcp.try_clone()?, tcp.try_clone()?))
    })();
    let (control, abort, write_tcp) = match prepared {
        Ok(handles) => handles,
        Err(e) => {
            shared.rt.log(DEBUG, TARGET, &format!("could not set up a connection: {e}"));
            return;
        }
    };
    let (conn, queue) = Conn::new(id, shared.rt.schema_hash(), config.max_queued_bytes, Some(abort));
    let conn = Arc::new(conn);
    if !shared.attach(id, &conn) {
        // The server is shutting down.
        conn.abort();
        return;
    }
    let session = Session::new(shared.rt.clone(), shared.bridge.clone(), conn.clone(), config);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        session_loop(shared, &session, tcp, &control, write_tcp, queue);
    }));
    // A panic in the loop must still free the slot and stop the writer.
    session.teardown();
    conn.stop_writer();
    conn.abort();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// Upgrades the socket and runs the read loop. On return the caller tears the session down.
fn session_loop(
    shared: &Shared,
    session: &Session,
    tcp: TcpStream,
    control: &TcpStream,
    write_tcp: TcpStream,
    queue: Receiver<Item>,
) {
    let config = &shared.config;
    let conn = &session.conn;
    let ws_config = ws::config(config.max_message_bytes);

    let half = match ReadHalf::new(tcp) {
        Ok(half) => half,
        Err(e) => {
            session.note(DEBUG, &format!("could not clone the socket: {e}"));
            return;
        }
    };
    let mut socket = match tungstenite::accept_with_config(half, Some(ws_config)) {
        Ok(socket) => socket,
        Err(e) => {
            // Not a WebSocket client, or one that stalled: nothing to say to it.
            session.note(DEBUG, &format!("a connection did not complete the WebSocket upgrade: {e}"));
            return;
        }
    };
    let writer = match writer::spawn(conn.clone(), queue, write_tcp, ws_config, config.close_timeout) {
        Ok(handle) => handle,
        Err(e) => {
            session.note(ERROR, &format!("could not start a writer thread: {e}"));
            return;
        }
    };
    socket.get_mut().route_writes_to_queue(conn.raw_sender());

    let begin_close = |violation: Violation| {
        session.note(
            DEBUG,
            &format!("closing a connection ({}): {}", violation.code, violation.reason),
        );
        conn.close(violation.code, &violation.reason);
        // The peer's Close reply, or the end of the stream, is now all we wait for.
        let _ = control.set_read_timeout(Some(config.close_timeout));
    };

    let mut attached = false;
    let mut peer_closed = false;
    loop {
        match socket.read() {
            Ok(Message::Binary(data)) => {
                if conn.is_closing() {
                    continue;
                }
                if attached {
                    if let Err(violation) = session.on_frame(&data) {
                        begin_close(violation);
                    }
                } else {
                    match session.on_hello(&data) {
                        Ok(()) => {
                            attached = true;
                            let _ = control.set_read_timeout(None);
                        }
                        Err(violation) => begin_close(violation),
                    }
                }
            }
            Ok(Message::Text(_)) => {
                if !conn.is_closing() {
                    begin_close(Violation {
                        code: close::UNSUPPORTED_DATA,
                        reason: "Keel speaks binary envelopes; got a text message".to_owned(),
                    });
                }
            }
            Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
            Ok(Message::Close(_)) => {
                peer_closed = true;
                // Sends the echo (the read side hands it to the writer through the queue).
                let _ = socket.flush();
                break;
            }
            Err(error) => {
                if !conn.is_closing() {
                    if let Some(violation) = violation_for(&error, attached, config.handshake_timeout) {
                        begin_close(violation);
                    }
                }
                break;
            }
        }
    }

    // Slot first, then the slow part.
    session.teardown();
    conn.stop_writer();
    let _ = writer.join();
    if conn.close_queued() && !peer_closed {
        drain(control, config.close_timeout);
    }
}

/// Maps a read error to the Close frame it deserves, or `None` when the peer is simply gone.
fn violation_for(error: &Error, attached: bool, handshake_timeout: Duration) -> Option<Violation> {
    match error {
        Error::ConnectionClosed | Error::AlreadyClosed => None,
        Error::Protocol(ProtocolError::ResetWithoutClosingHandshake) => None,
        Error::Io(e)
            if !attached
                && matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) =>
        {
            Some(Violation {
                code: close::POLICY_VIOLATION,
                reason: format!("no Hello within {} ms", handshake_timeout.as_millis()),
            })
        }
        Error::Io(_) => None,
        Error::Capacity(e) => Some(Violation {
            code: close::MESSAGE_TOO_BIG,
            reason: e.to_string(),
        }),
        Error::Utf8 => Some(Violation {
            code: close::INVALID_PAYLOAD,
            reason: "a text message was not valid UTF-8".to_owned(),
        }),
        other => Some(Violation::protocol(other.to_string())),
    }
}

/// Reads and discards until the peer closes its side or `limit` passes, so that closing our
/// socket does not reset the connection while the peer is still reading our Close frame.
fn drain(tcp: &TcpStream, limit: Duration) {
    let deadline = Instant::now() + limit;
    let mut sink = [0_u8; 4096];
    let mut tcp = tcp;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if left.is_zero() || tcp.set_read_timeout(Some(left)).is_err() {
            return;
        }
        match tcp.read(&mut sink) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}
