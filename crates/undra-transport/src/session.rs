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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use undra_runtime::log::{DEBUG, ERROR, FATAL, INFO, WARN};
use undra_runtime::Runtime;
use undra_wire::payload::{
    Call, CallTarget, Cancel, Event, Hello, Observe, PortReply, PortStatus, Release, StreamCredit,
    TimerFired,
};
use undra_wire::{Envelope, Kind, Reader, WireError, Writer};
use tungstenite::Message;
use tungstenite::error::{Error, ProtocolError};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http::StatusCode;

use crate::bridge::{Bridge, ClientInfo};
use crate::conn::{Begin, Conn, Item};
use crate::devtools;
use crate::devtools::hub::{Route, RouteGuard};
use crate::devtools::proto::Cause;
use crate::notice::{self, NOTICE_TARGET, Notices};
use crate::resume::{self, Resume, short_token};
use crate::server::{Shared, ServerConfig};
use crate::ws::{self, ReadHalf, close};
use crate::writer;

/// The `target` of the server's own log records.
const TARGET: &str = "undra::transport";

/// The undra version the server announces in its `Hello`.
pub const UNDRA_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Why a session must end: the Close frame's code and reason.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Violation {
    pub(crate) code: u16,
    pub(crate) reason: String,
    /// Already reported to the log by whoever produced it.
    pub(crate) noted: bool,
}

impl Violation {
    fn protocol(reason: String) -> Violation {
        Violation {
            code: close::PROTOCOL_ERROR,
            reason,
            noted: false,
        }
    }

    fn schema(ours: u64, theirs: u64) -> Violation {
        Violation {
            code: close::POLICY_VIOLATION,
            reason: format!("schema mismatch: core {ours:#018x}, client {theirs:#018x}"),
            noted: false,
        }
    }
}

/// Text a client sent, made safe to print in a terminal or a log: control characters (ANSI
/// escapes among them) replaced, length capped.
pub(crate) fn printable(text: &str) -> String {
    text.chars()
        .take(64)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
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

/// What the server shares with every session beyond the runtime: the flag that stops calls
/// from being run while the server is being suspended (ADR-053), the count of the calls that were
/// not run because of it, and what it tells a client that attaches.
#[derive(Clone)]
pub(crate) struct Hooks {
    /// Set by [`Server::suspend`](crate::Server::suspend): calls are no longer run.
    pub(crate) frozen: Arc<AtomicBool>,
    /// Calls a client sent while [`frozen`](Hooks::frozen) was set: not run, not answered (the
    /// client fails them as unavailable when the socket closes). `undra dev` says how many, so a
    /// write made during the reload does not vanish behind "state kept".
    pub(crate) dropped: Arc<AtomicUsize>,
    /// The dev notices of this server.
    pub(crate) notices: Arc<Notices>,
}

impl Default for Hooks {
    fn default() -> Self {
        Hooks {
            frozen: Arc::new(AtomicBool::new(false)),
            dropped: Arc::new(AtomicUsize::new(0)),
            notices: Arc::new(Notices::new(notice::AttachNotices::default())),
        }
    }
}

/// What the reader thread does with one client's envelopes.
pub(crate) struct Session {
    rt: Arc<Runtime>,
    bridge: Arc<Bridge>,
    conn: Arc<Conn>,
    resume: Arc<Resume>,
    release_on_disconnect: bool,
    busy_grace: Duration,
    hooks: Hooks,
}

impl Session {
    pub(crate) fn new(
        rt: Arc<Runtime>,
        bridge: Arc<Bridge>,
        conn: Arc<Conn>,
        resume: Arc<Resume>,
        config: &ServerConfig,
        hooks: Hooks,
    ) -> Session {
        Session {
            rt,
            bridge,
            conn,
            resume,
            release_on_disconnect: config.release_on_disconnect,
            busy_grace: config.busy_grace,
            hooks,
        }
    }

    /// Says the dev notice that is due to this client, if any (ADR-053).
    fn tell(&self, kind: notice::Kind) {
        let token = self.conn.session().map(|s| s.token.as_str());
        if let Some(text) = self.hooks.notices.for_client(kind, token) {
            self.conn.on_log(INFO, NOTICE_TARGET, &text);
        }
    }

    fn note(&self, level: u8, message: &str) {
        self.rt.log(level, TARGET, message);
    }

    /// A frame that arrived after the connection began to close, which is not processed: when the
    /// server is being suspended and it is a `Call`, it is one more call the reload did not run.
    fn ignored_while_closing(&self, data: &[u8]) {
        if self.hooks.frozen.load(Ordering::Acquire)
            && Envelope::parse(data).is_ok_and(|env| env.kind == Kind::Call)
        {
            self.hooks.dropped.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// The server's own `Hello`: its version, schema hash, platform and mode.
    fn send_hello(&self) {
        let config = self.rt.config();
        let hello = Hello {
            undra_version: UNDRA_VERSION,
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
                    printable(hello.platform),
                    hello.schema_hash
                ),
            );
            return Err(Violation {
                noted: true,
                ..Violation::schema(ours, hello.schema_hash)
            });
        }
        let info = ClientInfo {
            undra_version: printable(hello.undra_version),
            platform: printable(hello.platform),
            mode: printable(hello.mode),
        };
        self.conn.set_client(info.clone());
        // A client that is back on a new socket under its old token replaces its own stale one
        // at once; the claim below then waits for that socket's teardown (which retains its
        // objects first), not for the keepalive to give up on it.
        if let Some(request) = self.conn.session() {
            if self.bridge.evict_session(&request.token, self.conn.id) {
                self.note(
                    INFO,
                    &format!("session {}: replacing its previous connection", request.short()),
                );
            }
        }
        if !self.bridge.claim(&self.conn, self.busy_grace) {
            self.note(
                INFO,
                &format!("refused a second client ({}): one is already attached", info.platform),
            );
            return Err(Violation {
                code: close::TRY_AGAIN_LATER,
                reason: "another client is already connected; undra dev serves one at a time"
                    .to_owned(),
                noted: true,
            });
        }
        self.settle_session(&info)
    }

    /// Decides what the client finds: its own objects again (it resumed), or a core with none of
    /// its making (it is new), or neither (it asked for objects that are gone). The slot is
    /// ours by now.
    fn settle_session(&self, info: &ClientInfo) -> Result<(), Violation> {
        let request = self.conn.session().cloned();
        if let Some(request) = request.as_ref().filter(|r| r.resume) {
            return match self.resume.take(&request.token) {
                Some(kept) => {
                    self.conn.adopt(&kept.handles);
                    self.note(
                        INFO,
                        &format!(
                            "client reconnected: platform={} mode={} undra={} (session {}, away {:.1} s, {} object(s) kept)",
                            info.platform,
                            info.mode,
                            info.undra_version,
                            request.short(),
                            kept.away().as_secs_f32(),
                            kept.handles.len()
                        ),
                    );
                    self.tell(notice::Kind::Resumed);
                    self.app_attached();
                    Ok(())
                }
                None => {
                    self.note(
                        WARN,
                        &format!(
                            "a client ({}) asked to resume session {}, which this core does not hold (it was restarted, or the session expired): its objects are gone, so it is told to load a new core",
                            info.platform,
                            request.short()
                        ),
                    );
                    self.conn.mark_refused();
                    Err(Violation {
                        code: close::SESSION_LOST,
                        reason: format!(
                            "session lost: this core has no session {} (it was restarted or the session expired); load a new core",
                            request.short()
                        ),
                        noted: true,
                    })
                }
            };
        }
        // A new client: whatever the previous one left behind is not coming back.
        if let Some(gone) = self.resume.supersede() {
            resume::release_all(&self.rt, &gone.handles);
            self.note(
                INFO,
                &format!(
                    "released the {} object(s) of the previous client (session {}): a new client attached",
                    gone.handles.len(),
                    short_token(&gone.token)
                ),
            );
        }
        self.note(
            INFO,
            &format!(
                "client connected: platform={} mode={} undra={}",
                info.platform, info.mode, info.undra_version
            ),
        );
        self.tell(notice::Kind::Fresh);
        self.app_attached();
        Ok(())
    }

    /// Tells the devtools pages that an app client holds the slot.
    fn app_attached(&self) {
        if let Some(hub) = self.bridge.active_hub() {
            hub.app_changed();
        }
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
                self.observe(observe.handle.0, observe.signal_id, observe.on);
            }
            Kind::Release => {
                let release = decode(payload, "Release", Release::decode)?;
                self.conn.release(release.handle.0);
                self.rt.release(release.handle.0);
            }
            Kind::PortReply => {
                let reply = decode(payload, "PortReply", PortReply::decode)?;
                self.conn.port_call_answered(reply.port_call_id);
                if let Some(hub) = self.bridge.active_hub() {
                    hub.port_end(reply.port_call_id, reply.status.as_u8(), reply.body);
                }
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

    /// Starts or stops the runtime's observation on the client's behalf. While a devtools hub holds
    /// a store it keeps observing all of it: the client's `observe(off)` is only recorded (the
    /// bridge sends the client what it observed, ADR-054), and the values the runtime answers an
    /// `observe(on)` with are the client's alone.
    fn observe(&self, handle: u64, signal_id: u32, on: bool) {
        if !on && self.bridge.active_hub().is_some_and(|hub| hub.holds(handle)) {
            return;
        }
        let _route = on.then(|| RouteGuard::set(Route::AppObserve));
        self.rt.observe(handle, signal_id, on);
    }

    fn on_call(&self, payload: &[u8]) -> Result<(), Violation> {
        let call = decode(payload, "Call", Call::decode)?;
        let constructor = matches!(call.target, CallTarget::Constructor { .. });
        // Recorded before the runtime sees it: a sync method replies before `call` returns.
        match self
            .conn
            .begin_call(call.call_id, constructor, &self.hooks.frozen)
        {
            Begin::Started => {}
            Begin::Frozen => {
                // The server is being suspended (ADR-053): the core is about to be replaced, so
                // a call that starts now would run on state the snapshot may already have missed.
                // It is not answered; the client fails it as unavailable when the socket closes,
                // and it is counted, so that `undra dev` can say its write was lost.
                self.hooks.dropped.fetch_add(1, Ordering::AcqRel);
                self.note(
                    DEBUG,
                    &format!("not running call {}: the core is being reloaded", call.call_id),
                );
                return Ok(());
            }
            // The client fails it when the socket closes, a moment from now.
            Begin::Closing => return Ok(()),
            Begin::Duplicate => {
                self.note(
                    WARN,
                    &format!("ignoring a call that reuses the open call id {}", call.call_id),
                );
                return Ok(());
            }
        }
        // What this call commits is labelled with it in the devtools timeline (a synchronous
        // method commits on this thread; an asynchronous one commits later, on the core).
        let method_id = match call.target {
            CallTarget::Function { method_id }
            | CallTarget::Method { method_id, .. }
            | CallTarget::Constructor { method_id, .. } => method_id,
            CallTarget::LazyPage { .. } => 0,
        };
        let refused = {
            let _route = RouteGuard::set(Route::Commit(Cause::Call(method_id)));
            self.rt.call(payload) != 0
        };
        if refused {
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
    /// the objects its constructors made (or keeps them for its return, see
    /// [`resume`](crate::resume)) and fails the port calls it will never answer. Idempotent.
    ///
    /// Releasing is not something the protocol asks for (no client releases at disconnect), but
    /// a dev core outlives many app launches and would otherwise keep every launch's stores,
    /// observers included, for ever.
    pub(crate) fn teardown(&self) {
        let had_client = self.conn.client().is_some();
        let session = self.conn.session().cloned();
        let left = self.conn.drain();
        for call_id in &left.calls {
            self.rt.cancel(*call_id);
        }
        let attached = self.bridge.is_attached(self.conn.id);
        // Retained for a client that announced a session and made objects worth coming back to.
        let retain = attached
            && had_client
            && self.release_on_disconnect
            && self.resume.enabled()
            && !left.constructed.is_empty();
        let keep_for = if retain { session.as_ref() } else { None };
        let owned: HashSet<u64> = if self.release_on_disconnect && keep_for.is_none() {
            left.constructed.iter().copied().collect()
        } else {
            HashSet::new()
        };
        // Observations stop for everything that outlives the connection: the client observes
        // again when it comes back, and the core answers with the current values.
        let hub = self.bridge.active_hub();
        for (handle, signal) in &left.observed {
            // A store the devtools hub holds stays observed: the hub undoes it when the last page leaves.
            if !owned.contains(handle) && !hub.as_ref().is_some_and(|hub| hub.holds(*handle)) {
                self.rt.observe(*handle, *signal, false);
            }
        }
        if let Some(request) = keep_for {
            let replaced = self.resume.retain(&request.token, left.constructed.clone());
            resume::release_all(&self.rt, &replaced);
        } else if self.release_on_disconnect {
            resume::release_all(&self.rt, &left.constructed);
        }
        for id in &left.port_calls {
            let mut w = Writer::with_capacity(5);
            PortReply {
                port_call_id: *id,
                status: PortStatus::Unavailable,
                body: &[],
            }
            .encode(&mut w);
            if let Some(hub) = &hub {
                hub.port_end(*id, PortStatus::Unavailable.as_u8(), &[]);
            }
            self.rt.port_reply(w.as_slice());
        }
        // Vacated after the retention above: a client waiting in `claim` for this slot (the
        // same session, back on a new socket) must find the objects when it gets it.
        self.bridge.vacate(self.conn.id);
        if let Some(hub) = self.bridge.active_hub() {
            hub.app_changed();
        }
        if attached && had_client && !self.conn.is_refused() {
            let objects = if keep_for.is_some() {
                format!(
                    "{} object(s) kept for {} s so it can reconnect",
                    left.constructed.len(),
                    self.resume.grace().as_secs()
                )
            } else if self.release_on_disconnect {
                format!("{} handles released", left.constructed.len())
            } else {
                "0 handles released".to_owned()
            };
            self.note(
                INFO,
                &format!("client disconnected ({} calls cancelled, {objects})", left.calls.len()),
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
    // The first line of the request says whether this is the page of the devtools (or its socket)
    // or an app client. Peeking leaves it in the socket for the WebSocket upgrade.
    let deadline = Instant::now() + config.handshake_timeout;
    match devtools::http::peek_request_line(&tcp, deadline) {
        Ok(Some(line)) if devtools::http::is_devtools_path(&line.path) => {
            devtools::serve::run(shared, id, tcp, line, control, abort, write_tcp);
            return;
        }
        Ok(_) => {}
        Err(e) => {
            shared.rt.log(DEBUG, TARGET, &format!("a connection sent no request: {e}"));
            return;
        }
    }
    let (conn, queue) = Conn::new(id, shared.rt.schema_hash(), config.max_queued_bytes, Some(abort));
    let conn = Arc::new(conn);
    if !shared.attach(id, &conn) {
        // The server is shutting down.
        conn.abort();
        return;
    }
    let session = Session::new(
        shared.rt.clone(),
        shared.bridge.clone(),
        conn.clone(),
        shared.resume.clone(),
        config,
        shared.hooks.clone(),
    );
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
// The upgrade callback's error type (`http::Response`) is fixed by tungstenite.
#[allow(clippy::result_large_err)]
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

    let half = match ReadHalf::new(tcp, conn.clone()) {
        Ok(half) => half,
        Err(e) => {
            session.note(DEBUG, &format!("could not clone the socket: {e}"));
            return;
        }
    };
    let origin_check = |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
        // The session the client announces in the query of its URL (ADR-051).
        if let Some(request) = resume::parse_query(request.uri().query()) {
            conn.set_session(request);
        }
        let origin = request.headers().get("Origin").map(|value| value.to_str());
        let allowed = match origin {
            None => config.origin_policy.allows(None),
            Some(Ok(origin)) => config.origin_policy.allows(Some(origin)),
            Some(Err(_)) => false,
        };
        if allowed {
            return Ok(response);
        }
        let shown = origin.and_then(Result::ok).map_or_else(|| "(not text)".to_owned(), printable);
        session.note(WARN, &format!("refused a page from origin {shown}: see OriginPolicy"));
        let mut refusal = ErrorResponse::new(Some("origin not allowed".to_owned()));
        *refusal.status_mut() = StatusCode::FORBIDDEN;
        Err(refusal)
    };
    let mut socket = match tungstenite::accept_hdr_with_config(half, origin_check, Some(ws_config)) {
        Ok(socket) => socket,
        Err(e) => {
            // Not a WebSocket client, or one that stalled: nothing to say to it.
            session.note(DEBUG, &format!("a connection did not complete the WebSocket upgrade: {e}"));
            return;
        }
    };
    let timing = writer::Timing {
        linger: config.close_timeout,
        ping_interval: config.ping_interval,
    };
    let writer = match writer::spawn(conn.clone(), queue, write_tcp, ws_config, timing) {
        Ok(handle) => handle,
        Err(e) => {
            session.note(ERROR, &format!("could not start a writer thread: {e}"));
            return;
        }
    };
    socket.get_mut().route_writes_to_queue(conn.raw_sender());

    let begin_close = |violation: Violation| {
        if !violation.noted {
            session.note(
                WARN,
                &format!("closing the connection ({}): {}", violation.code, violation.reason),
            );
        }
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
                    session.ignored_while_closing(&data);
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
                        reason: "Undra speaks binary envelopes; got a text message".to_owned(),
                        noted: false,
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
                noted: false,
            })
        }
        Error::Io(_) => None,
        Error::Capacity(e) => Some(Violation {
            code: close::MESSAGE_TOO_BIG,
            reason: e.to_string(),
            noted: false,
        }),
        Error::Utf8 => Some(Violation {
            code: close::INVALID_PAYLOAD,
            reason: "a text message was not valid UTF-8".to_owned(),
            noted: false,
        }),
        other => Some(Violation::protocol(other.to_string())),
    }
}

/// Reads and discards until the peer closes its side or `limit` passes, so that closing our
/// socket does not reset the connection while the peer is still reading our Close frame.
pub(crate) fn drain(tcp: &TcpStream, limit: Duration) {
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

#[cfg(test)]
mod tests {
    use std::sync::mpsc::Receiver;

    use undra_runtime::RuntimeConfig;
    use undra_runtime::testing::call_payload;
    use undra_wire::payload::{Reply, ReplyStatus};
    use proptest::prelude::*;

    use super::*;

    /// A runtime with the bridge as its host, a connection attached to it, and its session.
    struct Rig {
        rt: Arc<Runtime>,
        session: Session,
        queue: parking_lot::Mutex<Receiver<Item>>,
    }

    impl Rig {
        fn new() -> Rig {
            let bridge = Bridge::new();
            let rt = Runtime::new(
                RuntimeConfig {
                    core_threads: 1,
                    log_level: 0,
                    ..RuntimeConfig::default()
                },
                bridge.clone(),
            )
            .expect("the runtime starts");
            let (conn, queue) = Conn::new(1, rt.schema_hash(), usize::MAX, None);
            let conn = Arc::new(conn);
            assert!(bridge.claim(&conn, Duration::ZERO));
            let session = Session::new(
                rt.clone(),
                bridge,
                conn,
                Resume::new(Duration::ZERO),
                &ServerConfig::default(),
                Hooks::default(),
            );
            Rig {
                rt,
                session,
                queue: parking_lot::Mutex::new(queue),
            }
        }

        fn frame(&self, kind: Kind, payload: &[u8]) -> Vec<u8> {
            let mut w = Writer::new();
            Envelope::write(&mut w, kind, 0, self.rt.schema_hash(), payload);
            w.into_vec()
        }

        /// The replies queued for the client so far.
        fn replies(&self) -> Vec<(u32, ReplyStatus)> {
            let mut out = Vec::new();
            while let Ok(item) = self.queue.lock().try_recv() {
                if let Item::Frame(bytes) = item {
                    let env = Envelope::parse(&bytes).expect("the server writes valid envelopes");
                    if env.kind == Kind::Reply {
                        let reply = Reply::decode(&mut Reader::new(env.payload)).unwrap();
                        out.push((reply.call_id, reply.status));
                    }
                }
            }
            out
        }
    }

    #[test]
    fn a_call_through_the_session_is_answered_and_tracked() {
        let rig = Rig::new();
        let payload = call_payload(CallTarget::Function { method_id: 0xdead }, 5, &[]);
        rig.session
            .on_frame(&rig.frame(Kind::Call, &payload))
            .expect("a well-formed call is not a violation");
        assert_eq!(rig.replies(), [(5, ReplyStatus::BadRequest)]);
        assert_eq!(rig.session.conn.drain().calls, Vec::<u32>::new(), "answered, so not open");
    }

    #[test]
    fn client_supplied_text_is_made_safe_to_print() {
        assert_eq!(printable("ios"), "ios");
        assert_eq!(printable("\u{1b}[2Jwipe\n"), "?[2Jwipe?");
        assert_eq!(printable(&"x".repeat(500)).len(), 64);
    }

    #[test]
    fn a_schema_mismatch_is_a_policy_violation_naming_both_hashes() {
        let rig = Rig::new();
        let mut w = Writer::new();
        Envelope::write(&mut w, Kind::Cancel, 0, 0x1234, &[1, 0, 0, 0]);
        let violation = rig.session.on_frame(w.as_slice()).unwrap_err();
        assert_eq!(violation.code, close::POLICY_VIOLATION);
        assert!(violation.reason.contains("0x0000000000001234"), "{}", violation.reason);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1500))]

        /// Random bytes are a violation or ignored; never a panic.
        #[test]
        fn random_bytes_never_panic_the_session(bytes in proptest::collection::vec(any::<u8>(), 0..300)) {
            let rig = rig();
            match rig.session.on_frame(&bytes) {
                Ok(()) => {}
                Err(v) => prop_assert!(v.code == close::PROTOCOL_ERROR || v.code == close::POLICY_VIOLATION),
            }
            let _ = rig.session.on_hello(&bytes);
        }

        /// A valid header with the right schema and a random kind and payload: the payload
        /// decoders and the runtime see arbitrary bytes and must survive them.
        #[test]
        fn valid_headers_with_random_payloads_never_panic(
            kind in 1_u8..=16,
            payload in proptest::collection::vec(any::<u8>(), 0..120),
        ) {
            let rig = rig();
            let kind = Kind::from_u8(kind).unwrap();
            match rig.session.on_frame(&rig.frame(kind, &payload)) {
                Ok(()) => {}
                Err(v) => prop_assert_eq!(v.code, close::PROTOCOL_ERROR),
            }
            // The core is still there afterwards.
            let probe = call_payload(CallTarget::Function { method_id: 1 }, 0xfff0, &[]);
            let _ = rig.rt.call_sync(&probe);
            prop_assert!(!rig.rt.is_shut_down());
        }
    }

    /// One shared rig per test binary keeps the property tests fast: they only need a live
    /// runtime, not a fresh one.
    fn rig() -> &'static Rig {
        static RIG: std::sync::OnceLock<Rig> = std::sync::OnceLock::new();
        RIG.get_or_init(Rig::new)
    }
}
