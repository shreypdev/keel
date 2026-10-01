//! [`Server`]: the listener, the connection registry and shutdown.

use core::fmt;
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use undra_runtime::log::{INFO, WARN};
use undra_runtime::{InitError, Runtime};
use parking_lot::{Condvar, Mutex};

use crate::bridge::Bridge;
use crate::conn::Conn;
use crate::error::ServeError;
use crate::origin::OriginPolicy;
use crate::resume::{self, Resume};
use crate::session;
use crate::ws::close;

const TARGET: &str = "undra::transport";

/// Limits and timeouts of a [`Server`]. Build one with struct-update syntax:
/// `ServerConfig { close_timeout: Duration::from_millis(200), ..ServerConfig::default() }`.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// How long a new connection has to complete the WebSocket upgrade and send its `Hello`.
    /// Default 10 s (the clients wait 10 s for the server's).
    pub handshake_timeout: Duration,
    /// How long to wait for the peer's answer to a Close frame before dropping the socket.
    /// Default 2 s.
    pub close_timeout: Duration,
    /// How long a write may block on a client that has stopped reading before the connection
    /// is dropped. Default 30 s.
    pub write_timeout: Duration,
    /// How long a second client waits for the first one's teardown before it is refused.
    /// A browser reload or an app relaunch reconnects within milliseconds of the old socket
    /// closing, and must not lose that race. Default 1 s.
    pub busy_grace: Duration,
    /// The largest message accepted from a client, in bytes. Default 64 MiB (what the Kotlin
    /// and TypeScript runtimes accept from the core).
    pub max_message_bytes: usize,
    /// How many bytes of outbound messages may be queued for a client that is slower than the
    /// core before the connection is dropped. Default 64 MiB.
    pub max_queued_bytes: usize,
    /// How many sockets (upgrading, attached or closing) are served at once; further ones are
    /// dropped on accept. A socket counts until its connection thread has finished tearing it
    /// down, which can be a moment after the peer sees it close, so a burst of connections that
    /// all fail can briefly use up slots a well-formed client would otherwise find. Default 16.
    pub max_connections: usize,
    /// Release the objects a client's constructors made when it disconnects (its observations
    /// and open calls are always ended). Default `true`.
    pub release_on_disconnect: bool,
    /// How long a client that announced a session token (`?undra_session=` in its URL) may be
    /// gone before the objects its constructors made are released; a client that comes back with
    /// the same token within the time finds them again (ADR-034). They are released earlier when
    /// another client attaches. `Duration::ZERO` (the default) keeps nothing: objects are
    /// released at disconnect, as for a client that sends no token. `undra dev` sets ten minutes.
    pub resume_grace: Duration,
    /// How long a client may be silent before it is sent a WebSocket Ping, and (three times
    /// as long) before it is dropped as dead. Every client answers pings without any code of
    /// its own, so this only catches connections that died without a FIN: a phone that left the
    /// Wi-Fi, a sleeping laptop. Without it such a connection would hold the one client slot
    /// until the OS gave up on it, and the relaunched app would be refused. Default 5 s;
    /// `Duration::ZERO` switches it off.
    pub ping_interval: Duration,
    /// Which web pages may connect. Default: pages on this machine or a private network (see
    /// [`OriginPolicy`]); native clients always may.
    pub origin_policy: OriginPolicy,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            handshake_timeout: Duration::from_secs(10),
            close_timeout: Duration::from_secs(2),
            write_timeout: Duration::from_secs(30),
            busy_grace: Duration::from_secs(1),
            max_message_bytes: 64 << 20,
            max_queued_bytes: 64 << 20,
            max_connections: 16,
            release_on_disconnect: true,
            resume_grace: Duration::ZERO,
            ping_interval: Duration::from_secs(5),
            origin_policy: OriginPolicy::default(),
        }
    }
}

struct Entry {
    /// A handle on the socket, so it can be shut down even before the session exists.
    tcp: TcpStream,
    conn: Option<Arc<Conn>>,
}

#[derive(Default)]
struct Registry {
    conns: HashMap<u64, Entry>,
    threads: Vec<JoinHandle<()>>,
}

/// State shared by the accept thread and every connection thread.
pub(crate) struct Shared {
    pub(crate) rt: Arc<Runtime>,
    pub(crate) bridge: Arc<Bridge>,
    pub(crate) config: ServerConfig,
    pub(crate) resume: Arc<Resume>,
    stopping: AtomicBool,
    next_id: AtomicU64,
    registry: Mutex<Registry>,
    drained: Condvar,
}

impl Shared {
    /// Registers the session of connection `id`. `false` once the server is stopping: the
    /// session must not start.
    pub(crate) fn attach(&self, id: u64, conn: &Arc<Conn>) -> bool {
        let mut registry = self.registry.lock();
        if self.stopping.load(Ordering::Acquire) {
            return false;
        }
        match registry.conns.get_mut(&id) {
            Some(entry) => {
                entry.conn = Some(conn.clone());
                true
            }
            None => false,
        }
    }

    /// Forgets connection `id`.
    pub(crate) fn detach(&self, id: u64) {
        let mut registry = self.registry.lock();
        registry.conns.remove(&id);
        if registry.conns.is_empty() {
            self.drained.notify_all();
        }
    }

    /// Starts a thread for a freshly accepted socket, unless the server is full.
    fn spawn_connection(self: &Arc<Self>, tcp: TcpStream) {
        let mut registry = self.registry.lock();
        registry.threads.retain(|handle| !handle.is_finished());
        if registry.conns.len() >= self.config.max_connections {
            self.rt.log(
                WARN,
                TARGET,
                &format!("dropping a connection: {} are already open", registry.conns.len()),
            );
            return;
        }
        let control = match tcp.try_clone() {
            Ok(control) => control,
            Err(e) => {
                self.rt.log(WARN, TARGET, &format!("dropping a connection: {e}"));
                return;
            }
        };
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        registry.conns.insert(
            id,
            Entry {
                tcp: control,
                conn: None,
            },
        );
        let shared = self.clone();
        let spawned = thread::Builder::new()
            .name(format!("undra-transport-conn-{id}"))
            .spawn(move || session::run(&shared, id, tcp));
        match spawned {
            Ok(handle) => registry.threads.push(handle),
            Err(e) => {
                registry.conns.remove(&id);
                self.rt
                    .log(WARN, TARGET, &format!("could not start a connection thread: {e}"));
            }
        }
    }

    fn accept_loop(self: &Arc<Self>, listener: &TcpListener) {
        for incoming in listener.incoming() {
            if self.stopping.load(Ordering::Acquire) {
                break;
            }
            match incoming {
                Ok(tcp) => self.spawn_connection(tcp),
                Err(e) => {
                    // Typically running out of file descriptors: back off instead of spinning.
                    self.rt.log(WARN, TARGET, &format!("accept failed: {e}"));
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }

    /// Asks every connection to end: a Close frame to established ones, a hard shutdown to
    /// those still upgrading.
    fn close_all(&self, hard: bool) {
        let registry = self.registry.lock();
        for entry in registry.conns.values() {
            match &entry.conn {
                Some(conn) if !hard => conn.close(close::GOING_AWAY, "the server is shutting down"),
                Some(conn) => conn.abort(),
                None => {
                    let _ = entry.tcp.shutdown(std::net::Shutdown::Both);
                }
            }
        }
    }

    /// Waits until no connection is left or `limit` passes. Returns whether none is left.
    fn wait_drained(&self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        let mut registry = self.registry.lock();
        while !registry.conns.is_empty() {
            if self.drained.wait_until(&mut registry, deadline).timed_out() {
                break;
            }
        }
        registry.conns.is_empty()
    }
}

/// A running WebSocket server that serves one [`Runtime`] to one remote client at a time.
///
/// # What it does
///
/// * **Accepts** WebSocket connections on a TCP listener, one thread each (host-side plumbing
///   is the one place threads are allowed; the core stays deterministic).
/// * **Handshakes**: the client's `Hello` is answered with the server's; a client built from
///   another schema is told (the server's `Hello` carries the core's hash) and closed with
///   code 1008. The client speaks first, as all three runtimes do.
/// * **Serves one client at a time.** A connection that arrives while another is attached
///   gets the server's `Hello` and is closed with 1013 (try again later), after a short grace
///   ([`ServerConfig::busy_grace`]) so that a reconnect right after the old socket closed does
///   not lose the race. The core is single-mutator and the mirror of a client is a whole
///   world; two clients on one runtime would see each other's handles as noise.
/// * **Pumps envelopes** in both directions: `Call`, `Cancel`, `StreamCredit`, `Observe`,
///   `Release`, `Event`, `TimerFired`, `PortReply` and `Restore` go into the runtime;
///   replies, change-sets, stream items, port calls and log records come out through the
///   [`Bridge`], each direction with its own sequence counter starting at 0.
/// * **Cleans up** when the client goes: its open calls are cancelled, its observations
///   stopped, the objects its constructors made released (unless
///   [`ServerConfig::release_on_disconnect`] is off) and the port calls it will never answer
///   completed as unavailable.
/// * **Survives** anything a client sends: a malformed message closes that connection with
///   1002 and nothing else.
///
/// # Queueing and threads
///
/// The runtime calls the [`Bridge`] from core threads and must not be made to wait. So every
/// outbound message is *enqueued* on the connection (a sequence number is assigned under the
/// same lock that pushes it) and a per-connection writer thread does the socket writes. A
/// client that stops reading is dropped once [`ServerConfig::max_queued_bytes`] are waiting.
/// Per connection there is one reader thread (which also runs the runtime's synchronous
/// methods, on the caller's-thread path the runtime defines) and one writer thread.
///
/// # Shutdown
///
/// [`shutdown`](Server::shutdown) (also on drop) stops accepting, closes the attached client
/// with 1001, waits up to [`ServerConfig::close_timeout`] and then joins every thread. It does
/// **not** shut the runtime down: the caller owns that.
pub struct Server {
    shared: Arc<Shared>,
    addr: SocketAddr,
    accept: Mutex<Option<JoinHandle<()>>>,
    reaper: Mutex<Option<JoinHandle<()>>>,
    shut: Mutex<bool>,
}

impl Server {
    /// Builds a [`Bridge`], lets `make_runtime` build the runtime with it as its host, and
    /// serves that runtime on `addr` (use port 0 to let the OS choose; see
    /// [`addr`](Server::addr)).
    ///
    /// `make_runtime` receives the bridge because a runtime's host is fixed at construction:
    /// `Runtime::new(config, bridge)` (or `Runtime::init`). It may also bind ports, insert
    /// objects and set the bridge's log sink before returning. Build the runtime with
    /// `mode: "dev"` if clients should receive development-mode records (SPEC 5.10).
    ///
    /// # Errors
    ///
    /// [`ServeError::Runtime`] if `make_runtime` fails; [`ServeError::Io`] if the address
    /// cannot be bound (the runtime it built is shut down again).
    pub fn start<A: ToSocketAddrs>(
        addr: A,
        config: ServerConfig,
        make_runtime: impl FnOnce(Arc<Bridge>) -> Result<Arc<Runtime>, InitError>,
    ) -> Result<Server, ServeError> {
        let bridge = Bridge::new();
        let runtime = make_runtime(bridge.clone())?;
        match Server::bind(addr, runtime.clone(), bridge, config) {
            Ok(server) => Ok(server),
            Err(e) => {
                runtime.shutdown();
                Err(ServeError::Io(e))
            }
        }
    }

    /// Serves `runtime` on `addr`. `runtime` **must have been built with `bridge` as its
    /// host** (`Runtime::new(config, bridge.clone())`): the bridge is how the runtime reaches
    /// the client, and nothing else can check that it is. [`start`](Server::start) does it
    /// correctly by construction.
    ///
    /// # Errors
    ///
    /// The `io::Error` of binding or of reading the bound address.
    pub fn bind<A: ToSocketAddrs>(
        addr: A,
        runtime: Arc<Runtime>,
        bridge: Arc<Bridge>,
        config: ServerConfig,
    ) -> io::Result<Server> {
        let listener = TcpListener::bind(addr)?;
        let addr = listener.local_addr()?;
        let resume = Resume::new(config.resume_grace);
        let shared = Arc::new(Shared {
            rt: runtime,
            bridge,
            config,
            resume,
            stopping: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            registry: Mutex::new(Registry::default()),
            drained: Condvar::new(),
        });
        let accept = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("undra-transport-accept".to_owned())
                .spawn(move || shared.accept_loop(&listener))?
        };
        let reaper = if shared.resume.enabled() {
            Some(shared.resume.spawn_reaper(shared.rt.clone())?)
        } else {
            None
        };
        shared
            .rt
            .log(INFO, TARGET, &format!("serving on ws://{addr}"));
        Ok(Server {
            shared,
            addr,
            accept: Mutex::new(Some(accept)),
            reaper: Mutex::new(reaper),
            shut: Mutex::new(false),
        })
    }

    /// The address the server listens on (the real port when 0 was asked for).
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The `ws://` URL a client connects to: the listen address, with the loopback address
    /// standing in for an unspecified one (`0.0.0.0`).
    pub fn url(&self) -> String {
        let ip = match self.addr.ip() {
            ip if ip.is_unspecified() => loopback_of(ip),
            ip => ip,
        };
        format!("ws://{}", SocketAddr::new(ip, self.addr.port()))
    }

    /// The runtime being served.
    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.shared.rt
    }

    /// The host the runtime was built with.
    pub fn bridge(&self) -> &Arc<Bridge> {
        &self.shared.bridge
    }

    /// Stops the server: no new connections, the attached client is closed with 1001 and every
    /// thread is joined. Idempotent (later calls return once the first has finished). The
    /// runtime keeps running; shut it down separately.
    pub fn shutdown(&self) {
        let mut done = self.shut.lock();
        if *done {
            return;
        }
        let shared = &self.shared;
        shared.stopping.store(true, Ordering::Release);

        // `accept` blocks; a connection to ourselves wakes it, and it sees the flag.
        let wake = SocketAddr::new(loopback_of(self.addr.ip()), self.addr.port());
        let woken = TcpStream::connect_timeout(&wake, Duration::from_secs(1)).is_ok();
        if let Some(handle) = self.accept.lock().take() {
            if woken {
                let _ = handle.join();
            }
        }

        shared.close_all(false);
        let grace = shared.config.close_timeout + Duration::from_secs(1);
        if !shared.wait_drained(grace) {
            shared.close_all(true);
            shared.wait_drained(Duration::from_secs(2));
        }
        let threads = std::mem::take(&mut shared.registry.lock().threads);
        for handle in threads {
            let _ = handle.join();
        }
        // Whatever a dropped client left for its return goes back to the runtime.
        if let Some(left) = shared.resume.stop() {
            resume::release_all(&shared.rt, &left.handles);
        }
        if let Some(handle) = self.reaper.lock().take() {
            let _ = handle.join();
        }
        *done = true;
    }
}

/// The loopback address of `ip`'s family, or `ip` itself when it is not unspecified.
fn loopback_of(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(v4) if v4.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(v6) if v6.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        other => other,
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl fmt::Debug for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Server").field("addr", &self.addr).finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unspecified_addresses_map_to_their_loopback() {
        assert_eq!(loopback_of(IpAddr::V4(Ipv4Addr::UNSPECIFIED)), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(loopback_of(IpAddr::V6(Ipv6Addr::UNSPECIFIED)), IpAddr::V6(Ipv6Addr::LOCALHOST));
        let lan = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5));
        assert_eq!(loopback_of(lan), lan);
    }

    #[test]
    fn defaults_are_the_documented_ones() {
        let c = ServerConfig::default();
        assert_eq!(c.max_message_bytes, 64 << 20);
        assert_eq!(c.max_connections, 16);
        assert!(c.release_on_disconnect);
        assert!(c.resume_grace.is_zero(), "resuming is opt in");
    }
}
