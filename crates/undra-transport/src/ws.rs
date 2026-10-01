//! The seam with tungstenite.
//!
//! tungstenite gives one `WebSocket` per socket, and that object both reads and writes, so a
//! reader thread and a writer thread cannot share it without one blocking the other. Instead
//! each direction gets its own protocol state over the same socket:
//!
//! * the **read side** is a `WebSocket` over [`ReadHalf`]. It reads the socket and, because
//!   the protocol requires it to answer pings and echo a close, it also *writes*; but every
//!   byte it writes is complete frames and goes into the outbound queue as [`Item::Raw`], for
//!   the writer thread to put on the wire between messages;
//! * the **write side** is a `WebSocketContext` over [`WriteHalf`], owned by the writer
//!   thread ([`crate::writer`]), which writes every message this crate sends.
//!
//! Only public tungstenite API is used. Because all writes are serialised through the queue,
//! a frame is never interleaved with another.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::mpsc::Sender;

use tungstenite::protocol::WebSocketConfig;

use crate::conn::{Conn, Item};

/// WebSocket close codes (RFC 6455 section 7.4) the server sends.
pub mod close {
    /// The server is shutting down.
    pub const GOING_AWAY: u16 = 1001;
    /// A malformed envelope or payload, or a message that breaks the framing rules.
    pub const PROTOCOL_ERROR: u16 = 1002;
    /// A text message: Undra speaks binary envelopes only.
    pub const UNSUPPORTED_DATA: u16 = 1003;
    /// A text message that is not valid UTF-8.
    pub const INVALID_PAYLOAD: u16 = 1007;
    /// The peer's schema hash is not the core's, or it sent no Hello in time.
    pub const POLICY_VIOLATION: u16 = 1008;
    /// A message larger than the configured limit.
    pub const MESSAGE_TOO_BIG: u16 = 1009;
    /// Another client already holds the connection (Undra serves one at a time).
    pub const TRY_AGAIN_LATER: u16 = 1013;
}

/// Where the read side's own writes go.
enum Sink {
    /// During the HTTP upgrade, straight to the socket: the writer thread does not exist yet.
    Socket(TcpStream),
    /// Afterwards, into the outbound queue.
    Queue(Sender<Item>),
}

/// The stream the read-side `WebSocket` owns. See the [module documentation](self).
pub(crate) struct ReadHalf {
    tcp: TcpStream,
    sink: Sink,
    /// Told whenever bytes arrive, so silence can be told from a slow transfer.
    conn: Arc<Conn>,
}

impl ReadHalf {
    /// A read side that reads `tcp` and, until [`route_writes_to_queue`](Self::route_writes_to_queue),
    /// writes to it as well (the upgrade response).
    pub(crate) fn new(tcp: TcpStream, conn: Arc<Conn>) -> io::Result<ReadHalf> {
        let writer = tcp.try_clone()?;
        Ok(ReadHalf {
            tcp,
            sink: Sink::Socket(writer),
            conn,
        })
    }

    /// From now on the read side's writes (pong, close echo) are queued for the writer thread.
    pub(crate) fn route_writes_to_queue(&mut self, queue: Sender<Item>) {
        self.sink = Sink::Queue(queue);
    }
}

impl Read for ReadHalf {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.tcp.read(buf)?;
        if n > 0 {
            self.conn.touch();
        }
        Ok(n)
    }
}

impl Write for ReadHalf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match &mut self.sink {
            Sink::Socket(tcp) => tcp.write(buf),
            Sink::Queue(queue) => {
                // Whole frames only: tungstenite hands `write_out_buffer` the entire buffer
                // and this accepts all of it.
                queue
                    .send(Item::Raw(buf.to_vec()))
                    .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
                Ok(buf.len())
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.sink {
            Sink::Socket(tcp) => tcp.flush(),
            Sink::Queue(_) => Ok(()),
        }
    }
}

/// The stream the write-side `WebSocketContext` writes to. It is never read.
pub(crate) struct WriteHalf(pub(crate) TcpStream);

impl Read for WriteHalf {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
}

impl Write for WriteHalf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// The tungstenite settings for both sides. Frames and messages share one limit: browsers and
/// the JDK send a message as a single frame, so the frame limit must not be the smaller one.
pub(crate) fn config(max_message_bytes: usize) -> WebSocketConfig {
    WebSocketConfig {
        max_message_size: Some(max_message_bytes),
        max_frame_size: Some(max_message_bytes),
        ..WebSocketConfig::default()
    }
}
