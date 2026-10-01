#![forbid(unsafe_code)]
#![deny(missing_docs)]
// The README (with its compiled example) documents the server, which exists only on native
// targets with the `server` feature.
#![cfg_attr(all(feature = "server", not(target_family = "wasm")), doc = include_str!("../README.md"))]
//!
//! # Threads, and why they are allowed here
//!
//! The constitution (R12) keeps threads out of the deterministic *core*; they live in
//! `undra-runtime` and in host-side plumbing. This crate is host-side plumbing: it runs in the
//! process of an `undra dev` core, next to the runtime, and does blocking socket I/O. It uses
//! `std::net` and threads (one accept thread, and a reader and a writer thread per connection)
//! rather than an async runtime, which CLAUDE.md bans from core crates. Nothing here is
//! reachable from core code: the only way in is through the `Bridge` the runtime is
//! built with, and the bridge only enqueues.
//!
//! # The wire contract with the client runtimes
//!
//! The Swift, Kotlin and TypeScript runtimes ship a `remote` transport and are tested against
//! fake servers. This crate is the real one, and the shipped clients define its behaviour
//! wherever `docs/SPEC.md` is silent or differs. What they require:
//!
//! * One envelope (SPEC 3.2) per WebSocket **binary** message. A text message is a protocol
//!   error in both directions.
//! * The **client speaks first**: it sends `Hello` when the socket opens, then waits for the
//!   server's `Hello`. The server always answers, even when it is about to refuse the client,
//!   because a schema mismatch is reported *from the server's Hello*. That envelope is the
//!   server's sequence 0 and carries the core's schema hash in its header, as does everything
//!   after it. Clients drop a connection whose envelopes carry another hash, and so does the
//!   server.
//! * Sequence numbers are per direction and increasing. The server's start at 0 and have no
//!   gaps. Clients differ (TypeScript and Kotlin start at 0, Swift at 1), so the server does
//!   not validate them. (SPEC 3.2 does not say where a direction starts.)
//! * Port calls are the *server's* requests: in `undra dev` the core runs here and the app is
//!   the platform, so adapters live in the client. `PortCall` goes out, `PortReply` comes back;
//!   a client answers `unavailable` for a port it does not implement.
//! * The client's `Hello` `mode` decides whether it receives development-mode records
//!   (SPEC 5.10): `"dev"` gets them, anything else does not. Kotlin and Swift always send
//!   `"dev"`; TypeScript sends it when its `devtools` option is on.
//! * The clients **reconnect by themselves** (ADR-051): a closed socket fails what is in flight
//!   and the client connects again with backoff, then observes its stores again. A connection
//!   is a *session* only if its URL says so (`?undra_session=<token>`): the server ends a
//!   connection's calls and observations when it closes, and releases its objects too, unless
//!   [`ServerConfig::resume_grace`] keeps them for a client that comes back with the same token
//!   (`&undra_resume=1`). A client that asks to resume objects the server no longer holds (the
//!   core was restarted) is answered with [`close::SESSION_LOST`]; it has to load a new core.
//!
//! # Which thread runs a call
//!
//! The reader thread of the attached connection calls into the runtime (SPEC 5.1: a sync call
//! runs on the caller's thread holding the core lock). A synchronous method therefore blocks
//! that connection's *inbound* messages while it runs (they wait in the socket); outbound
//! messages are never blocked by it, because they only enqueue.
//!
//! # Deviations from the spec, and decisions it leaves open
//!
//! * **The entry point is `Server::start`, not `serve(runtime, addr)`.** A `Runtime`'s host is
//!   fixed when it is built, so the host that carries messages to a client has to exist first;
//!   `Server::start` builds the `Bridge`, hands it to a closure that builds the runtime, and
//!   listens. `Server::bind` takes a runtime and bridge you paired yourself.
//! * **SPEC 3.2 leaves the Hello order and the sequence origin open.** The clients settle both:
//!   see above. SPEC 3.4 lists "schema mismatch" as a status 5 reason; in practice a mismatch is
//!   found at the `Hello` (every client checks it there) and later mismatching headers end the
//!   connection (1008).
//! * **SPEC 16.2 imagines a server with one core per connection**; this one serves one core to
//!   one client at a time, as `undra dev` needs (one device against the developer's core).
//! * **Dependencies** (SPEC 13 lists `undra-runtime` and `tungstenite`): `undra-wire` (the
//!   envelope, not re-implemented here) and `parking_lot` (as in `undra-runtime`) as well.
//!
//! # Features
//!
//! `server` (default) is the whole crate. Without it, and on `wasm` targets, the crate is
//! empty: the server is host-side and has nothing to do in a browser.

// The server is host-side plumbing for native processes: without the `server` feature, and on
// wasm targets, the crate is empty.
macro_rules! native_server {
    ($($item:item)*) => {
        $(#[cfg(all(feature = "server", not(target_family = "wasm")))] $item)*
    };
}

native_server! {
    mod bridge;
    mod conn;
    mod error;
    mod notice;
    mod origin;
    mod resume;
    mod server;
    mod session;
    mod tracker;
    mod writer;
    mod ws;

    pub use bridge::{Bridge, ClientInfo, LogSink};
    pub use error::ServeError;
    pub use notice::{AttachNotices, NOTICE_TARGET};
    pub use origin::OriginPolicy;
    pub use resume::KeptSession;
    pub use server::{Server, ServerConfig, Suspended};
    pub use session::UNDRA_VERSION;
    pub use ws::close;
}
