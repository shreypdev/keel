//! One accepted connection as the rest of the crate sees it: an outbound queue with the
//! per-direction sequence counter, the tracker, and a way to close.
//!
//! # Queueing
//!
//! Runtime callbacks ([`Host::reply`](undra_runtime::Host::reply) and friends) arrive on core
//! threads, often with the core lock held, and must never wait for the network. So they only
//! *enqueue*: [`Conn::send_with`] builds the complete envelope (assigning the next sequence
//! number under the same lock that pushes it, which makes wire order equal sequence order) and
//! hands it to an unbounded channel. A dedicated writer thread ([`crate::writer`]) owns the
//! socket's write side and drains the channel.
//!
//! An unbounded queue would let a client that stops reading grow the process without limit, so
//! the bytes queued are counted; past `max_queued` the connection is aborted rather than
//! blocking the core or exhausting memory.

use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use undra_runtime::PortCallOutcome;
use undra_wire::payload::{ChangeSetBuilder, ChangeSetRef, Log, PortCall, Reply, ReplyStatus, StreamItem};
use undra_wire::{Envelope, Kind, Reader, Writer};
use parking_lot::Mutex;

use crate::bridge::ClientInfo;
use crate::resume::SessionRequest;
use crate::tracker::{Leftovers, Tracker};

/// The target of the runtime's development-mode records (SPEC 5.10).
const DEVTOOLS_TARGET: &str = "undra::devtools";

/// A source of monotonic time: how long since some fixed origin. The keepalive reads time only
/// through this, so that a test can hold it still and move it by exact steps.
pub(crate) type Clock = Arc<dyn Fn() -> Duration + Send + Sync>;

/// The real clock: time since this call.
pub(crate) fn real_clock() -> Clock {
    let origin = Instant::now();
    Arc::new(move || origin.elapsed())
}

/// A [`Clock`] a test moves by hand: it stands still until [`advance`](ManualClock::advance).
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct ManualClock(Arc<AtomicU64>);

#[cfg(test)]
impl ManualClock {
    /// A clock reading what this one reads, now and after every `advance`.
    pub(crate) fn clock(&self) -> Clock {
        let millis = self.0.clone();
        Arc::new(move || Duration::from_millis(millis.load(Ordering::SeqCst)))
    }

    /// Moves time forward by `by` (whole milliseconds).
    pub(crate) fn advance(&self, by: Duration) {
        let by = u64::try_from(by.as_millis()).expect("a step that fits a u64");
        self.0.fetch_add(by, Ordering::SeqCst);
    }
}

/// What the writer thread is asked to do, in queue order.
#[derive(Debug)]
pub(crate) enum Item {
    /// A complete envelope, to be written as one binary message.
    Frame(Vec<u8>),
    /// Complete WebSocket frames produced by the read side (the pong to a ping, the echo of a
    /// close). They are written between messages, never inside one.
    Raw(Vec<u8>),
    /// Send a Close frame, then wait for the peer.
    Close { code: u16, reason: String },
    /// The read side is done: flush, half-close and exit.
    Stop,
}

struct State {
    tx: Sender<Item>,
    next_seq: u32,
    /// No more frames are accepted (a close began, or the session was torn down).
    closing: bool,
    /// A Close item has been queued.
    close_queued: bool,
    tracker: Tracker,
}

/// One accepted connection. Shared between the reader thread, the writer thread and the
/// [`Bridge`](crate::Bridge) that the runtime calls into.
pub(crate) struct Conn {
    pub(crate) id: u64,
    schema: u64,
    max_queued: usize,
    queued: AtomicUsize,
    closing: AtomicBool,
    state: Mutex<State>,
    /// A handle on the socket used only to abort it.
    tcp: Option<TcpStream>,
    client: OnceLock<ClientInfo>,
    /// What the upgrade request said about the client's session (ADR-051).
    session: OnceLock<SessionRequest>,
    /// The server turned this client away after it attached (a session it cannot resume): its
    /// teardown is not worth a log line of its own.
    refused: AtomicBool,
    /// Where the keepalive reads the time.
    clock: Clock,
    /// Milliseconds on `clock` at which bytes last arrived from the peer; until then, when the
    /// connection was accepted.
    last_rx: AtomicU64,
}

/// What [`Conn::begin_call`] decided about a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Begin {
    /// Recorded: hand it to the runtime.
    Started,
    /// The server is being suspended for a reload (ADR-053): not run.
    Frozen,
    /// The connection is closing: not run.
    Closing,
    /// A call with this id is still open: not run.
    Duplicate,
}

impl Conn {
    /// A connection whose envelopes carry `schema`. Returns the receiving end of the outbound
    /// queue for the writer thread. `tcp` is only used by [`Conn::abort`].
    pub(crate) fn new(
        id: u64,
        schema: u64,
        max_queued: usize,
        tcp: Option<TcpStream>,
    ) -> (Conn, Receiver<Item>) {
        Conn::with_clock(id, schema, max_queued, tcp, real_clock())
    }

    /// [`Conn::new`] with the keepalive reading `clock` instead of the real one.
    pub(crate) fn with_clock(
        id: u64,
        schema: u64,
        max_queued: usize,
        tcp: Option<TcpStream>,
        clock: Clock,
    ) -> (Conn, Receiver<Item>) {
        let (tx, rx) = channel();
        let conn = Conn {
            id,
            schema,
            max_queued,
            queued: AtomicUsize::new(0),
            closing: AtomicBool::new(false),
            state: Mutex::new(State {
                tx,
                next_seq: 0,
                closing: false,
                close_queued: false,
                tracker: Tracker::default(),
            }),
            tcp,
            client: OnceLock::new(),
            session: OnceLock::new(),
            refused: AtomicBool::new(false),
            last_rx: AtomicU64::new(clock().as_millis().try_into().unwrap_or(u64::MAX)),
            clock,
        };
        (conn, rx)
    }

    /// A second sender into the outbound queue, for the read side's raw frames.
    pub(crate) fn raw_sender(&self) -> Sender<Item> {
        self.state.lock().tx.clone()
    }

    /// Records the client's Hello.
    pub(crate) fn set_client(&self, info: ClientInfo) {
        let _ = self.client.set(info);
    }

    /// The client's Hello, once it has been received.
    pub(crate) fn client(&self) -> Option<&ClientInfo> {
        self.client.get()
    }

    /// Records the session parameters of the upgrade request.
    pub(crate) fn set_session(&self, request: SessionRequest) {
        let _ = self.session.set(request);
    }

    /// The session the client announced in its URL, if it did.
    pub(crate) fn session(&self) -> Option<&SessionRequest> {
        self.session.get()
    }

    /// Notes that the server is turning this client away.
    pub(crate) fn mark_refused(&self) {
        self.refused.store(true, Ordering::Release);
    }

    /// Whether the server turned this client away.
    pub(crate) fn is_refused(&self) -> bool {
        self.refused.load(Ordering::Acquire)
    }

    /// Makes `handles` (objects of a session this client resumed) this connection's own: its
    /// disconnect retains or releases them like objects its constructors made.
    pub(crate) fn adopt(&self, handles: &[u64]) {
        self.state.lock().tracker.adopt(handles);
    }

    /// The keepalive's clock, now.
    pub(crate) fn now(&self) -> Duration {
        (self.clock)()
    }

    /// Bytes just arrived from the peer: it is alive.
    pub(crate) fn touch(&self) {
        let now = u64::try_from(self.now().as_millis()).unwrap_or(u64::MAX);
        self.last_rx.store(now, Ordering::Relaxed);
    }

    /// How long the peer had been silent (nothing at all, not even a pong) at `now`, a reading
    /// of [`Conn::now`] taken before this call. Judging every silence against one reading keeps
    /// the answer exact when a thread is descheduled between reading the clock and the
    /// timestamp.
    pub(crate) fn silent_at(&self, now: Duration) -> Duration {
        now.saturating_sub(Duration::from_millis(self.last_rx.load(Ordering::Relaxed)))
    }

    /// Whether frames are no longer accepted.
    pub(crate) fn is_closing(&self) -> bool {
        self.closing.load(Ordering::Acquire)
    }

    /// The writer wrote (or dropped) a frame of `n` bytes.
    pub(crate) fn dequeued(&self, n: usize) {
        self.queued.fetch_sub(n, Ordering::Relaxed);
    }

    // ----- sending ---------------------------------------------------------------------

    /// Queues one envelope of `kind` whose payload `payload` writes; `hint` sizes the buffer.
    /// Returns `false` (and drops the message) once the connection is closing.
    pub(crate) fn send_with(&self, kind: Kind, hint: usize, payload: impl FnOnce(&mut Writer)) -> bool {
        let mut state = self.state.lock();
        self.send_locked(&mut state, kind, hint, payload)
    }

    /// [`send_with`](Conn::send_with) for a payload that is already bytes.
    pub(crate) fn send(&self, kind: Kind, payload: &[u8]) -> bool {
        self.send_with(kind, payload.len(), |w| w.write_raw(payload))
    }

    fn send_locked(
        &self,
        state: &mut State,
        kind: Kind,
        hint: usize,
        payload: impl FnOnce(&mut Writer),
    ) -> bool {
        if state.closing {
            return false;
        }
        let seq = state.next_seq;
        state.next_seq = state.next_seq.wrapping_add(1);
        let mut w = Writer::with_capacity(Envelope::HEADER_LEN + hint);
        Envelope::write_with(&mut w, kind, seq, self.schema, payload);
        self.enqueue_locked(state, w.into_vec())
    }

    /// Queues one complete WebSocket message and applies the backlog bound.
    fn enqueue_locked(&self, state: &mut State, frame: Vec<u8>) -> bool {
        let len = frame.len();
        let before = self.queued.fetch_add(len, Ordering::Relaxed);
        if state.tx.send(Item::Frame(frame)).is_err() {
            // The writer is gone: the connection is over.
            state.closing = true;
            self.closing.store(true, Ordering::Release);
            return false;
        }
        // A single message larger than the cap is fine when nothing else waits (the cap is
        // about a client falling behind, not about how big one reply is).
        if before > 0 && before + len > self.max_queued {
            // The client is not keeping up. Waiting would block the core and buffering would
            // grow without bound, so the connection goes instead.
            state.closing = true;
            self.closing.store(true, Ordering::Release);
            self.abort();
            return false;
        }
        true
    }

    /// Queues one message that is not an envelope: a devtools connection's (ADR-054). Returns
    /// `false` (and drops it) once the connection is closing.
    pub(crate) fn send_plain(&self, message: Vec<u8>) -> bool {
        let mut state = self.state.lock();
        if state.closing {
            return false;
        }
        self.enqueue_locked(&mut state, message)
    }

    /// Bytes queued for the client that the writer has not written yet: the backlog.
    pub(crate) fn queued_bytes(&self) -> usize {
        self.queued.load(Ordering::Relaxed)
    }

    /// The signals of `handle` this client observes (`u32::MAX` stands for all of them).
    pub(crate) fn observed_signals(&self, handle: u64) -> Vec<u32> {
        self.state.lock().tracker.signals_of(handle)
    }

    /// Stops accepting frames and asks the writer to send a Close frame after everything
    /// queued so far. Idempotent.
    pub(crate) fn close(&self, code: u16, reason: &str) {
        let mut state = self.state.lock();
        state.closing = true;
        self.closing.store(true, Ordering::Release);
        if !state.close_queued {
            state.close_queued = true;
            let _ = state.tx.send(Item::Close {
                code,
                reason: fit_reason(reason),
            });
        }
    }

    /// Whether a Close frame has been queued (the server, not the peer, ended the session).
    pub(crate) fn close_queued(&self) -> bool {
        self.state.lock().close_queued
    }

    /// Tells the writer the read side is done.
    pub(crate) fn stop_writer(&self) {
        let _ = self.state.lock().tx.send(Item::Stop);
    }

    /// Shuts the socket down in both directions at once: unblocks a reader and a writer that
    /// are stuck on a peer that does not answer.
    pub(crate) fn abort(&self) {
        self.closing.store(true, Ordering::Release);
        if let Some(tcp) = &self.tcp {
            let _ = tcp.shutdown(Shutdown::Both);
        }
    }

    // ----- what the session tells the tracker -------------------------------------------

    /// Records a call before it is handed to the runtime (a sync method replies before
    /// `Runtime::call` returns), unless the server is being suspended (`frozen`), the connection
    /// is closing or the id is open already.
    ///
    /// `frozen` is read under this connection's lock, the lock [`open_plain_calls`] takes, and
    /// [`Server::suspend`](crate::Server::suspend) sets it before it counts the open calls: a call
    /// is therefore either recorded before that count (and waited for, so its reply goes out
    /// before the Close frame) or sees the flag and is not run. Without that, a call recorded
    /// between the count and the Close would run, land in the snapshot, and lose its reply.
    ///
    /// [`open_plain_calls`]: Conn::open_plain_calls
    pub(crate) fn begin_call(&self, call_id: u32, constructor: bool, frozen: &AtomicBool) -> Begin {
        let mut state = self.state.lock();
        if frozen.load(Ordering::Acquire) {
            Begin::Frozen
        } else if state.closing {
            Begin::Closing
        } else if state.tracker.begin_call(call_id, constructor) {
            Begin::Started
        } else {
            Begin::Duplicate
        }
    }

    /// Forgets a call that was cancelled or refused.
    pub(crate) fn end_call(&self, call_id: u32) {
        self.state.lock().tracker.end_call(call_id);
    }

    /// How many plain calls of this connection have not been answered yet (streams are not
    /// counted: they stay open for as long as the client wants them).
    pub(crate) fn open_plain_calls(&self) -> usize {
        self.state.lock().tracker.open_plain_calls()
    }

    /// Records an observation change.
    pub(crate) fn observe(&self, handle: u64, signal_id: u32, on: bool) {
        self.state.lock().tracker.observe(handle, signal_id, on);
    }

    /// Records a release.
    pub(crate) fn release(&self, handle: u64) {
        self.state.lock().tracker.release(handle);
    }

    /// The client answered a port call.
    pub(crate) fn port_call_answered(&self, id: u32) {
        self.state.lock().tracker.end_port_call(id);
    }

    /// Stops accepting frames and takes everything the tracker holds.
    pub(crate) fn drain(&self) -> Leftovers {
        let mut state = self.state.lock();
        state.closing = true;
        self.closing.store(true, Ordering::Release);
        state.tracker.drain()
    }

    // ----- what the runtime tells the connection (via the Bridge) -----------------------

    /// `Host::reply`.
    pub(crate) fn on_reply(&self, payload: &[u8]) {
        let mut state = self.state.lock();
        let mut r = Reader::new(payload);
        if let Ok(reply) = Reply::decode(&mut r) {
            state.tracker.on_reply(reply.call_id, reply.status, reply.body);
        }
        self.send_locked(&mut state, Kind::Reply, payload.len(), |w| {
            w.write_raw(payload);
        });
    }

    /// `Host::change_set`.
    pub(crate) fn on_change_set(&self, payload: &[u8]) {
        self.send(Kind::ChangeSet, payload);
    }

    /// `Host::change_set` while a devtools hub observes every store: the runtime's observed set
    /// is then larger than this client's, so only the entries it observed are sent (ADR-054). The
    /// bytes go out unchanged when it observed all of them, which is the usual case.
    pub(crate) fn on_change_set_observed(&self, payload: &[u8]) {
        let mut state = self.state.lock();
        // What to send: the bytes as they are, a re-encoding with only the observed entries, or
        // nothing.
        let kept: Option<Vec<u8>> = match ChangeSetRef::decode(&mut Reader::new(payload)) {
            // Not ours to judge: the runtime built it.
            Err(_) => None,
            Ok(set) => {
                let tracker = &state.tracker;
                if set.iter().all(|e| tracker.covers(e.handle.0, e.signal_id)) {
                    None
                } else {
                    let mut w = Writer::with_capacity(payload.len());
                    let mut builder = ChangeSetBuilder::new(&mut w, set.txn_id);
                    for e in set.iter().filter(|e| tracker.covers(e.handle.0, e.signal_id)) {
                        builder.push(e.handle, e.signal_id, e.op, e.value);
                    }
                    if builder.finish() == 0 {
                        return;
                    }
                    Some(w.into_vec())
                }
            }
        };
        let bytes = kept.as_deref().unwrap_or(payload);
        self.send_locked(&mut state, Kind::ChangeSet, bytes.len(), |w| w.write_raw(bytes));
    }

    /// `Host::stream_item`.
    pub(crate) fn on_stream_item(&self, payload: &[u8]) {
        let mut state = self.state.lock();
        let mut r = Reader::new(payload);
        if let Ok(item) = StreamItem::decode(&mut r) {
            state.tracker.on_stream_item(item.call_id, item.flag);
        }
        self.send_locked(&mut state, Kind::StreamItem, payload.len(), |w| {
            w.write_raw(payload);
        });
    }

    /// `Host::port_call`: the client owns the port, so the call goes out as a `PortCall`
    /// envelope and the answer comes back later (`Async`). A connection that is closing
    /// cannot answer, and neither can a port call the core makes while nobody is connected.
    pub(crate) fn on_port_call(
        &self,
        port_id: u32,
        method_id: u32,
        port_call_id: u32,
        args: &[u8],
    ) -> PortCallOutcome {
        let mut state = self.state.lock();
        if state.closing {
            return PortCallOutcome::Unavailable;
        }
        // Tracked before it is sent: teardown fails whatever is still tracked, so a call
        // whose frame never reaches the client cannot leave its future waiting forever.
        state.tracker.begin_port_call(port_call_id);
        let call = PortCall {
            port_id,
            method_id,
            port_call_id,
            args,
        };
        self.send_locked(&mut state, Kind::PortCall, 12 + args.len(), |w| {
            call.encode(w);
        });
        PortCallOutcome::Async
    }

    /// `Host::log`. Development-mode records are only for clients that said `mode = "dev"`
    /// in their Hello (SPEC 5.10).
    pub(crate) fn on_log(&self, level: u8, target: &str, message: &str) {
        if target == DEVTOOLS_TARGET && !self.client().is_some_and(ClientInfo::is_dev) {
            return;
        }
        let record = Log {
            level,
            target,
            message,
        };
        self.send_with(Kind::Log, 9 + target.len() + message.len(), |w| {
            record.encode(w);
        });
    }

    /// A `Reply` the server itself produces for a call the runtime refused.
    pub(crate) fn send_bad_request(&self, call_id: u32, reason: &str) {
        let mut body = Writer::with_capacity(4 + reason.len());
        body.write_str(reason);
        let reply = Reply {
            call_id,
            status: ReplyStatus::BadRequest,
            body: body.as_slice(),
        };
        self.send_with(Kind::Reply, 5 + body.len(), |w| reply.encode(w));
    }
}

/// Close reasons share a control frame with the two code bytes: 123 bytes at most (RFC 6455
/// section 5.5), cut at a character boundary.
fn fit_reason(reason: &str) -> String {
    const MAX: usize = 123;
    if reason.len() <= MAX {
        return reason.to_owned();
    }
    let mut end = MAX;
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    reason[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::TryRecvError;

    use undra_wire::Envelope;

    use super::*;

    /// A change-set of `txn` with one `Full` entry per `(handle, signal)`.
    fn change_set(txn: u64, entries: &[(u64, u32)]) -> Vec<u8> {
        let mut w = Writer::new();
        let mut b = undra_wire::payload::ChangeSetBuilder::new(&mut w, txn);
        for (handle, signal) in entries {
            b.push(
                undra_wire::Handle(*handle),
                *signal,
                undra_wire::payload::ChangeOp::Full,
                &signal.to_le_bytes(),
            );
        }
        b.finish();
        w.into_vec()
    }

    fn sent_change_sets(rx: &Receiver<Item>) -> Vec<Vec<u8>> {
        frames(rx)
            .iter()
            .map(|f| Envelope::parse(f).unwrap())
            .filter(|e| e.kind == Kind::ChangeSet)
            .map(|e| e.payload.to_vec())
            .collect()
    }

    #[test]
    fn a_client_is_sent_only_the_entries_it_observed() {
        let (conn, rx) = Conn::new(1, 7, 1 << 20, None);
        conn.observe(10, 0, true);
        conn.observe(20, u32::MAX, true);

        // Everything observed: the bytes go out as they are.
        let all = change_set(5, &[(10, 0), (20, 4)]);
        conn.on_change_set_observed(&all);
        // A mixed one: re-encoded with the same transaction id and the entries it observed.
        conn.on_change_set_observed(&change_set(6, &[(10, 0), (10, 1), (20, 2), (30, 0)]));
        // Nothing observed: nothing is sent.
        conn.on_change_set_observed(&change_set(7, &[(10, 1), (30, 0)]));
        // A payload the runtime built wrongly is not ours to judge: it goes out untouched.
        conn.on_change_set_observed(&[1, 2, 3]);

        let sent = sent_change_sets(&rx);
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[0], all, "the original bytes");
        let mixed = undra_wire::payload::ChangeSet::decode(&mut Reader::new(&sent[1])).unwrap();
        assert_eq!(mixed.txn_id, 6);
        let kept: Vec<(u64, u32)> = mixed.entries.iter().map(|e| (e.handle.0, e.signal_id)).collect();
        assert_eq!(kept, [(10, 0), (20, 2)]);
        assert_eq!(sent[2], [1, 2, 3]);
    }

    #[test]
    fn messages_that_are_not_envelopes_are_queued_in_order_and_counted() {
        let (conn, rx) = Conn::new(1, 7, 1 << 20, None);
        assert!(conn.send_plain(vec![1, 2, 3]));
        assert!(conn.send_plain(vec![4]));
        assert_eq!(conn.queued_bytes(), 4);
        assert_eq!(frames(&rx), [vec![1, 2, 3], vec![4]]);
        conn.dequeued(4);
        conn.close(1000, "done");
        assert!(!conn.send_plain(vec![9]), "nothing is accepted once the connection is closing");
    }

    fn frames(rx: &Receiver<Item>) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(Item::Frame(f)) => out.push(f),
                Ok(_) => {}
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return out,
            }
        }
    }

    #[test]
    fn sequence_numbers_are_gapless_and_follow_queue_order() {
        let (conn, rx) = Conn::new(1, 0xfeed, 1 << 20, None);
        for i in 0..5_u8 {
            assert!(conn.send(Kind::Log, &[i]));
        }
        let seen: Vec<(u32, u64, u8)> = frames(&rx)
            .iter()
            .map(|f| {
                let e = Envelope::parse(f).unwrap();
                (e.seq, e.schema, e.payload[0])
            })
            .collect();
        assert_eq!(
            seen,
            [
                (0, 0xfeed, 0),
                (1, 0xfeed, 1),
                (2, 0xfeed, 2),
                (3, 0xfeed, 3),
                (4, 0xfeed, 4)
            ]
        );
    }

    #[test]
    fn concurrent_senders_never_reorder_seq_against_the_queue() {
        let (conn, rx) = Conn::new(1, 1, usize::MAX, None);
        let conn = std::sync::Arc::new(conn);
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let conn = conn.clone();
                std::thread::spawn(move || {
                    for _ in 0..500 {
                        conn.send(Kind::Log, &[0]);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let seqs: Vec<u32> = frames(&rx)
            .iter()
            .map(|f| Envelope::parse(f).unwrap().seq)
            .collect();
        assert_eq!(seqs, (0..2000).collect::<Vec<_>>());
    }

    #[test]
    fn nothing_is_accepted_after_close_and_close_is_queued_once() {
        let (conn, rx) = Conn::new(1, 1, 1 << 20, None);
        assert!(conn.send(Kind::Log, &[1]));
        conn.close(1008, "first");
        conn.close(1002, "second");
        assert!(!conn.send(Kind::Log, &[2]));
        assert!(conn.is_closing());
        let items: Vec<Item> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(matches!(items[0], Item::Frame(_)));
        assert!(matches!(&items[1], Item::Close { code: 1008, reason } if reason == "first"));
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn a_client_that_falls_too_far_behind_is_dropped_not_waited_for() {
        let (conn, rx) = Conn::new(1, 1, 100, None);
        let payload = vec![0_u8; 60];
        assert!(conn.send(Kind::ChangeSet, &payload), "the first message fits");
        assert!(
            !conn.send(Kind::ChangeSet, &payload),
            "the second finds the first still queued and passes the cap"
        );
        assert!(conn.is_closing());
        assert!(!conn.send(Kind::Log, &[]));
        drop(rx);
    }

    #[test]
    fn one_message_bigger_than_the_cap_is_allowed_when_nothing_else_waits() {
        let (conn, rx) = Conn::new(1, 1, 100, None);
        assert!(conn.send(Kind::ChangeSet, &vec![0_u8; 500]));
        assert!(!conn.is_closing());
        // Once it is written, the queue is empty again.
        let frame = frames(&rx).remove(0);
        conn.dequeued(frame.len());
        assert!(conn.send(Kind::Log, &[]));
    }

    #[test]
    fn dequeuing_frees_room() {
        let (conn, rx) = Conn::new(1, 1, 100, None);
        let payload = vec![0_u8; 40];
        assert!(conn.send(Kind::ChangeSet, &payload)); // 63 bytes queued
        let frame = frames(&rx).remove(0);
        conn.dequeued(frame.len());
        assert!(conn.send(Kind::ChangeSet, &payload));
        assert!(!conn.is_closing());
    }

    #[test]
    fn a_dead_writer_closes_the_connection() {
        let (conn, rx) = Conn::new(1, 1, 1 << 20, None);
        drop(rx);
        assert!(!conn.send(Kind::Log, &[]));
        assert!(conn.is_closing());
    }

    #[test]
    fn port_calls_are_unavailable_once_closing_and_tracked_until_answered() {
        let (conn, rx) = Conn::new(1, 1, 1 << 20, None);
        assert_eq!(conn.on_port_call(1, 2, 7, &[9]), PortCallOutcome::Async);
        let frame = frames(&rx).remove(0);
        let env = Envelope::parse(&frame).unwrap();
        assert_eq!(env.kind, Kind::PortCall);
        assert_eq!(env.payload, [1, 0, 0, 0, 2, 0, 0, 0, 7, 0, 0, 0, 9]);
        assert_eq!(conn.drain().port_calls, [7]);
        assert_eq!(conn.on_port_call(1, 2, 8, &[]), PortCallOutcome::Unavailable);
        assert!(conn.drain().port_calls.is_empty());
    }

    #[test]
    fn devtools_records_only_reach_dev_clients() {
        let info = |mode: &str| ClientInfo {
            undra_version: "1".into(),
            platform: "test".into(),
            mode: mode.into(),
        };
        for (mode, delivered) in [("dev", true), ("prod", false)] {
            let (conn, rx) = Conn::new(1, 1, 1 << 20, None);
            conn.set_client(info(mode));
            conn.on_log(1, "undra::devtools", "commit");
            conn.on_log(1, "app", "hello");
            let targets: Vec<String> = frames(&rx)
                .iter()
                .map(|f| {
                    let e = Envelope::parse(f).unwrap();
                    Log::decode(&mut Reader::new(e.payload)).unwrap().target.to_owned()
                })
                .collect();
            let expected: &[&str] = if delivered {
                &["undra::devtools", "app"]
            } else {
                &["app"]
            };
            assert_eq!(targets, expected, "mode {mode}");
        }
    }

    #[test]
    fn silence_is_measured_from_the_last_bytes_received() {
        let time = ManualClock::default();
        time.advance(Duration::from_millis(7)); // the connection is accepted mid-run
        let (conn, _rx) = Conn::with_clock(1, 1, 1 << 20, None, time.clock());
        assert_eq!(conn.silent_at(conn.now()), Duration::ZERO);
        time.advance(Duration::from_millis(40));
        assert_eq!(conn.silent_at(conn.now()), Duration::from_millis(40));
        conn.touch();
        assert_eq!(conn.silent_at(conn.now()), Duration::ZERO);
        time.advance(Duration::from_millis(25));
        assert_eq!(conn.silent_at(conn.now()), Duration::from_millis(25));
        // A reading taken before the touch cannot make the peer look silent for longer.
        assert_eq!(conn.silent_at(Duration::from_millis(1)), Duration::ZERO);
    }

    #[test]
    fn a_close_reason_fits_a_control_frame_at_a_char_boundary() {
        assert_eq!(fit_reason("short"), "short");
        let long = "\u{e9}".repeat(100); // 200 bytes
        let cut = fit_reason(&long);
        assert!(cut.len() <= 123);
        assert!(cut.chars().all(|c| c == '\u{e9}'));
    }
}
