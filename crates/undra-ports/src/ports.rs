//! The ten standard ports (SPEC 8).
//!
//! Each trait is a `#[undra::port]`: the platform (Swift, Kotlin, TypeScript) implements it, and
//! the core calls it through the runtime's port table. Method declaration order and names are
//! part of the wire contract: a method id is `fnv1a32("<Trait>.<method>")` and a port id is
//! `fnv1a32("port.<Trait>")`, and every platform runtime hard-codes both.
//!
//! | Port | Kind | Methods |
//! |---|---|---|
//! | [`Clock`] | sync | `now_ms`, `monotonic_ns` |
//! | [`Rng`] | sync | `fill` |
//! | [`Log`] | sync | `log` |
//! | [`Http`] | async | `request` |
//! | [`Kv`] | async | `get`, `set`, `delete`, `list` |
//! | [`SecureStore`] | async | `get`, `set`, `delete`, `list` |
//! | [`Fs`] | async | `read`, `write`, `delete`, `list` |
//! | [`Timer`] | sync | `set` |
//! | [`Connectivity`] | event | `changed` |
//! | [`Lifecycle`] | event | `changed` |
//!
//! For each request/reply port the macro also generates a proxy (`ClockProxy`, ...), an accessor
//! function (`clock(&Ctx) -> Arc<dyn Clock>`, ...: the Rust binding if one is bound, else the
//! platform's) and a Rust-side dispatcher. For the two event ports it generates
//! `on_connectivity_changed(ctx, f)` / `encode_connectivity_changed_event(..)` and the
//! `Lifecycle` equivalents.

use undra_wire::Bytes;

use crate::records::{
    AppState, FsError, HttpError, HttpRequest, HttpResponse, NetKind, StorageError,
};

/// Wall-clock and monotonic time. The core asks this port instead of reading the system clock.
#[undra_macros::port(sync)]
#[undra(crate = "crate::root")]
pub trait Clock {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> i64;
    /// A monotonic counter in nanoseconds. Only differences between two readings mean anything.
    fn monotonic_ns(&self) -> u64;
}

/// A source of random bytes. Answer from a cryptographically secure generator.
#[undra_macros::port(sync)]
#[undra(crate = "crate::root")]
pub trait Rng {
    /// Returns `len` random bytes.
    fn fill(&self, len: u32) -> Bytes;
}

/// Where the core's log records go.
#[undra_macros::port(sync)]
#[undra(crate = "crate::root")]
pub trait Log {
    /// Records one line. `level` is 0 trace, 1 debug, 2 info, 3 warn, 4 error, 5 fatal.
    fn log(&self, level: u8, target: String, message: String);
}

/// Performs HTTP requests.
///
/// A platform that did not register the port answers "unavailable"; `request` reports that (and a
/// cancelled call, and a reply that does not decode) as an `HttpError`, through
/// `impl From<PortError> for HttpError`, instead of panicking.
#[undra_macros::port]
#[undra(crate = "crate::root")]
pub trait Http {
    /// Performs `req`. Any status is a response; only failures before a response exists are an
    /// `HttpError`.
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>;
}

/// A persistent key-value store of byte strings; every method can fail with a [`StorageError`].
// ADR-049: a quota that runs out, storage locked before the device's first unlock, bytes that
// cannot be read back, or no adapter at all (`Unavailable`, through `impl From<PortError> for
// StorageError`, instead of a panic that would trap a wasm core). Kept out of the doc comment:
// a core embeds its schema's docs (ADR-050), and every core has this port.
#[undra_macros::port]
#[undra(crate = "crate::root")]
pub trait Kv {
    /// The value stored under `key`, if any.
    async fn get(&self, key: String) -> Result<Option<Bytes>, StorageError>;
    /// Stores `value` under `key`, replacing any previous value.
    async fn set(&self, key: String, value: Bytes) -> Result<(), StorageError>;
    /// Removes `key`; a missing key is not an error.
    async fn delete(&self, key: String) -> Result<(), StorageError>;
    /// The keys that start with `prefix`, in ascending order.
    async fn list(&self, prefix: String) -> Result<Vec<String>, StorageError>;
}

/// A key-value store for secrets. Same methods as `Kv`, under its own port id, and the same
/// [`StorageError`] channel (ADR-049).
#[undra_macros::port]
#[undra(crate = "crate::root")]
pub trait SecureStore {
    /// The value stored under `key`, if any.
    async fn get(&self, key: String) -> Result<Option<Bytes>, StorageError>;
    /// Stores `value` under `key`, replacing any previous value.
    async fn set(&self, key: String, value: Bytes) -> Result<(), StorageError>;
    /// Removes `key`; a missing key is not an error.
    async fn delete(&self, key: String) -> Result<(), StorageError>;
    /// The keys that start with `prefix`, in ascending order.
    async fn list(&self, prefix: String) -> Result<Vec<String>, StorageError>;
}

/// A sandboxed file system. Paths are `/`-separated and relative to the platform's root.
///
/// A platform without a file system answers "unavailable"; every method reports that as an
/// `FsError::Unavailable` through `impl From<PortError> for FsError` instead of panicking.
#[undra_macros::port]
#[undra(crate = "crate::root")]
pub trait Fs {
    /// The contents of the file at `path`.
    async fn read(&self, path: String) -> Result<Bytes, FsError>;
    /// Writes `data` to `path`, creating missing directories and replacing an existing file.
    async fn write(&self, path: String, data: Bytes) -> Result<(), FsError>;
    /// Removes the file or directory at `path`.
    async fn delete(&self, path: String) -> Result<(), FsError>;
    /// The names of the entries directly inside `dir`, in ascending order.
    async fn list(&self, dir: String) -> Result<Vec<String>, FsError>;
}

/// Arms timers. Fire-and-forget: the platform later reports `TimerFired(timer_id)` to the
/// runtime, which completes the matching sleep. Timer ids are allocated by the runtime; a
/// platform never invents one.
#[undra_macros::port]
#[undra(crate = "crate::root")]
pub trait Timer {
    /// Arms timer `timer_id` to fire after `delay_ms` milliseconds.
    fn set(&self, timer_id: u32, delay_ms: u64);
}

/// Connectivity changes, pushed by the platform to the core.
#[undra_macros::port(event)]
#[undra(crate = "crate::root")]
pub trait Connectivity {
    /// The device went online or offline, on a network of kind `kind`.
    fn changed(&self, online: bool, kind: NetKind);
}

/// App lifecycle changes, pushed by the platform to the core.
#[undra_macros::port(event)]
#[undra(crate = "crate::root")]
pub trait Lifecycle {
    /// The app moved to `state`.
    fn changed(&self, state: AppState);
}
