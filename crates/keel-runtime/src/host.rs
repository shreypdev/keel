//! The [`Host`] trait: everything the runtime needs from whoever embeds it.

/// What a host answered to a port call (SPEC 3.6, 6.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortCallOutcome {
    /// The host answered synchronously; the bytes are a complete `PortReply` payload
    /// (`port_call_id u32, status u8, body`).
    Sync(Vec<u8>),
    /// The host will answer later through [`Runtime::port_reply`](crate::Runtime::port_reply).
    Async,
    /// The port is not available on this platform (or is not registered).
    Unavailable,
}

/// The embedder of a [`Runtime`](crate::Runtime): `keel-ffi`, the wasm shell, the transport
/// server, or a test double (see [`RecordingHost`](crate::testing::RecordingHost)).
///
/// # Threading
///
/// Callbacks are invoked from whichever thread completed the work: the `keel-core` thread, a
/// caller's thread (the synchronous path of `call`/`call_sync`/`observe`), or, for `log` and
/// `schedule`, any thread at all. **`reply`, `change_set`, `stream_item` and `port_call` are
/// commonly invoked while the core lock is held**, so an implementation must not call back
/// into the runtime synchronously from them (with the exception of
/// [`Runtime::port_reply`](crate::Runtime::port_reply), which never takes the core lock).
/// Hosts enqueue onto their own thread instead. A re-entrant call on the same thread is
/// detected and rejected with `E_REENTRANT` (see `docs/runtime-internals.md`).
pub trait Host: Send + Sync + 'static {
    /// A call finished: `payload` is a `Reply` (SPEC 3.4).
    fn reply(&self, call_id: u32, payload: &[u8]);

    /// A transaction committed: `payload` is a `ChangeSet` (SPEC 3.5).
    fn change_set(&self, payload: &[u8]);

    /// A stream produced something: `payload` is a `StreamItem` (SPEC 3.7).
    fn stream_item(&self, call_id: u32, payload: &[u8]);

    /// The core wants a platform-implemented port method executed (SPEC 3.6).
    fn port_call(
        &self,
        port_id: u32,
        method_id: u32,
        port_call_id: u32,
        args: &[u8],
    ) -> PortCallOutcome;

    /// A log record. `level` uses the Log port numbering: 0 trace, 1 debug, 2 info, 3 warn,
    /// 4 error, 5 fatal.
    fn log(&self, level: u8, target: &str, message: &str);

    /// wasm and manually driven runtimes: ask the host to call
    /// [`Runtime::poll`](crate::Runtime::poll) soon. Deduplicated per turn. Must not call back
    /// into the runtime synchronously.
    fn schedule(&self) {}

    /// Ask the host to call [`Runtime::timer_fired`](crate::Runtime::timer_fired) with
    /// `timer_id` after `delay_ms`. Return `true` if the host owns timers (wasm); `false`
    /// (the default) makes the runtime use its own timer thread.
    fn timer_set(&self, timer_id: u32, delay_ms: u64) -> bool {
        let _ = (timer_id, delay_ms);
        false
    }
}
