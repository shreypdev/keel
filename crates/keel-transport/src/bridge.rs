//! [`Bridge`]: the [`Host`] that connects a runtime to whichever client is attached.

use std::sync::Arc;
use std::time::{Duration, Instant};

use keel_runtime::{Host, PortCallOutcome};
use parking_lot::{Condvar, Mutex};

use crate::conn::Conn;

/// A callback for log records: `(level, target, message)`, levels as in the Log port (0 trace
/// .. 5 fatal).
pub type LogSink = Arc<dyn Fn(u8, &str, &str) + Send + Sync>;

/// What a client said about itself in its `Hello` (SPEC 3.2, kind 12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientInfo {
    /// The Keel version of the client's runtime.
    pub keel_version: String,
    /// The client's platform: `"ios"`, `"android"`, `"web"`, `"node"`, `"jvm"`, ...
    pub platform: String,
    /// `"dev"` asks for the core's development-mode records (SPEC 5.10); anything else does not.
    pub mode: String,
}

impl ClientInfo {
    /// Whether the client asked for development-mode records.
    pub fn is_dev(&self) -> bool {
        self.mode == "dev"
    }
}

/// The [`Host`] of a served runtime.
///
/// A [`Runtime`](keel_runtime::Runtime) is given its host when it is built, before there is
/// anything to serve it to, so the host has to be able to exist without a client. The bridge
/// is that host: it forwards every reply, change-set, stream item, port call and log record to
/// the client that is currently attached, and drops them when there is none. Build it first,
/// build the runtime with it, then give both to [`Server::bind`](crate::Server::bind); or let
/// [`Server::start`](crate::Server::start) do all three.
///
/// # Threading
///
/// The runtime calls these methods from its own threads, often with the core lock held, and
/// forbids them to wait on anything slow. The bridge only ever *enqueues* onto the attached
/// connection's outbound queue (see [`Server`](crate::Server)); a separate writer thread does
/// the socket I/O.
///
/// # Ports
///
/// A port the runtime does not implement in Rust is implemented by the client. A port call
/// becomes a `PortCall` envelope and is answered asynchronously by the client's `PortReply`;
/// with no client attached it is `Unavailable`. **Synchronous ports cannot be served by a
/// remote client**: a sync call needs its answer before it returns and the bridge never
/// waits, so such a call is `Unavailable` (the runtime treats an asynchronous answer to a sync
/// call that way) and the late `PortReply` is discarded. Bind Rust implementations for `Clock`,
/// `Rng` and `Log` in a dev core, as `keel-ports` does by default.
pub struct Bridge {
    active: Mutex<Option<Arc<Conn>>>,
    vacated: Condvar,
    sink: Mutex<Option<LogSink>>,
}

impl Bridge {
    /// A bridge with no client and no log sink.
    pub fn new() -> Arc<Bridge> {
        Arc::new(Bridge {
            active: Mutex::new(None),
            vacated: Condvar::new(),
            sink: Mutex::new(None),
        })
    }

    /// Sends every log record the runtime emits (and the server's own notes about
    /// connections) to `sink`, in addition to the attached client. `keel dev` prints them.
    /// The sink runs on whichever thread logs, possibly with the core lock held: it must not
    /// call into the runtime.
    pub fn set_log_sink(&self, sink: impl Fn(u8, &str, &str) + Send + Sync + 'static) {
        *self.sink.lock() = Some(Arc::new(sink));
    }

    /// What the attached client said in its `Hello`, or `None` when nobody is attached.
    pub fn client(&self) -> Option<ClientInfo> {
        self.current().and_then(|conn| conn.client().cloned())
    }

    /// Whether a client is attached.
    pub fn is_connected(&self) -> bool {
        self.active.lock().is_some()
    }

    fn current(&self) -> Option<Arc<Conn>> {
        self.active.lock().clone()
    }

    /// Makes `conn` the attached client, waiting up to `grace` for the previous one to be
    /// torn down. `false` if the slot stayed taken.
    pub(crate) fn claim(&self, conn: &Arc<Conn>, grace: Duration) -> bool {
        let deadline = Instant::now() + grace;
        let mut active = self.active.lock();
        while active.is_some() {
            if self.vacated.wait_until(&mut active, deadline).timed_out() {
                break;
            }
        }
        if active.is_some() {
            return false;
        }
        *active = Some(conn.clone());
        true
    }

    /// Whether connection `id` is the attached one.
    pub(crate) fn is_attached(&self, id: u64) -> bool {
        self.active.lock().as_ref().is_some_and(|conn| conn.id == id)
    }

    /// Detaches connection `id` (a no-op if it is not the attached one) and wakes a client
    /// waiting in [`claim`](Bridge::claim).
    pub(crate) fn vacate(&self, id: u64) {
        let mut active = self.active.lock();
        if active.as_ref().is_some_and(|conn| conn.id == id) {
            *active = None;
            self.vacated.notify_all();
        }
    }
}

impl Host for Bridge {
    fn reply(&self, _call_id: u32, payload: &[u8]) {
        if let Some(conn) = self.current() {
            conn.on_reply(payload);
        }
    }

    fn change_set(&self, payload: &[u8]) {
        if let Some(conn) = self.current() {
            conn.on_change_set(payload);
        }
    }

    fn stream_item(&self, _call_id: u32, payload: &[u8]) {
        if let Some(conn) = self.current() {
            conn.on_stream_item(payload);
        }
    }

    fn port_call(
        &self,
        port_id: u32,
        method_id: u32,
        port_call_id: u32,
        args: &[u8],
    ) -> PortCallOutcome {
        match self.current() {
            Some(conn) => conn.on_port_call(port_id, method_id, port_call_id, args),
            None => PortCallOutcome::Unavailable,
        }
    }

    fn log(&self, level: u8, target: &str, message: &str) {
        let sink = self.sink.lock().clone();
        if let Some(sink) = sink {
            sink(level, target, message);
        }
        if let Some(conn) = self.current() {
            conn.on_log(level, target, message);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::Receiver;
    use std::thread;

    use keel_wire::{Envelope, Kind};

    use super::*;
    use crate::conn::Item;

    fn conn(id: u64) -> (Arc<Conn>, Receiver<Item>) {
        let (conn, rx) = Conn::new(id, 7, 1 << 20, None);
        (Arc::new(conn), rx)
    }

    #[test]
    fn with_no_client_everything_is_dropped_and_ports_are_unavailable() {
        let bridge = Bridge::new();
        bridge.reply(1, &[1, 0, 0, 0, 0]);
        bridge.change_set(&[0; 12]);
        assert_eq!(bridge.port_call(1, 2, 3, &[]), PortCallOutcome::Unavailable);
        assert!(!bridge.is_connected());
        assert_eq!(bridge.client(), None);
    }

    #[test]
    fn callbacks_reach_the_attached_client_and_the_sink() {
        let bridge = Bridge::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        bridge.set_log_sink(move |level, target, message| {
            record.lock().push(format!("{level} {target} {message}"));
        });
        let (c, rx) = conn(1);
        assert!(bridge.claim(&c, Duration::ZERO));
        bridge.change_set(&[9; 12]);
        bridge.log(3, "app", "careful");
        let kinds: Vec<Kind> = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|item| match item {
                Item::Frame(f) => Some(Envelope::parse(&f).unwrap().kind),
                _ => None,
            })
            .collect();
        assert_eq!(kinds, [Kind::ChangeSet, Kind::Log]);
        assert_eq!(*seen.lock(), ["3 app careful"]);
    }

    #[test]
    fn one_client_at_a_time_and_the_next_waits_for_the_slot() {
        let bridge = Bridge::new();
        let (first, _rx1) = conn(1);
        let (second, _rx2) = conn(2);
        assert!(bridge.claim(&first, Duration::ZERO));
        assert!(
            !bridge.claim(&second, Duration::from_millis(20)),
            "refused when the slot stays taken"
        );
        // Vacating an id that is not attached changes nothing.
        bridge.vacate(2);
        assert!(bridge.is_connected());

        let waiter = {
            let (bridge, second) = (bridge.clone(), second.clone());
            thread::spawn(move || bridge.claim(&second, Duration::from_secs(5)))
        };
        thread::sleep(Duration::from_millis(30));
        bridge.vacate(1);
        assert!(waiter.join().unwrap(), "the waiting client got the slot");
        assert!(bridge.is_connected());
    }
}
