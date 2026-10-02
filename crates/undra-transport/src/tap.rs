//! [`FrameTap`]: a read-only view of every envelope a server sends and receives, for
//! `undra dev --record` (ADR-055). The tap sees the payload of each envelope after the server has
//! parsed it (host to core) or built it (core to host); it cannot change or drop anything.

use core::fmt;
use std::sync::Arc;

use undra_wire::Kind;

/// Which way an envelope went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// From the attached client to the core.
    HostToCore,
    /// From the core to the attached client.
    CoreToHost,
}

type TapFn = dyn Fn(Direction, Kind, &[u8]) + Send + Sync;

/// A callback that sees every envelope of every connection: `(direction, kind, payload)`.
///
/// It runs on the thread that moved the envelope, with the connection's queue lock held for
/// core-to-host messages, so it must be quick and must never call back into the server or the
/// runtime. Set it with [`ServerConfig::tap`](crate::ServerConfig::tap).
#[derive(Clone)]
pub struct FrameTap(Arc<TapFn>);

impl FrameTap {
    /// A tap that calls `f` for every envelope.
    pub fn new(f: impl Fn(Direction, Kind, &[u8]) + Send + Sync + 'static) -> FrameTap {
        FrameTap(Arc::new(f))
    }

    pub(crate) fn see(&self, direction: Direction, kind: Kind, payload: &[u8]) {
        (self.0)(direction, kind, payload);
    }
}

impl fmt::Debug for FrameTap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FrameTap(..)")
    }
}
