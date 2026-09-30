//! The writer thread: the only thing that writes to a connection's socket.
//!
//! It drains the outbound queue in order. Envelopes become binary messages; raw frames from
//! the read side go out between them; a Close item sends a Close frame and starts a linger
//! timer, after which the socket is shut down whether or not the peer answered; Stop ends it.
//! Consecutive messages are batched into one `write` and flushed when the queue runs dry.

use std::io::{self, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::{CloseFrame, Message, Role, WebSocketConfig, WebSocketContext};

use crate::conn::{Conn, Item};
use crate::ws::WriteHalf;

/// Starts the writer thread of `conn`. `linger` bounds how long it waits for the peer after
/// sending a Close frame.
pub(crate) fn spawn(
    conn: Arc<Conn>,
    queue: Receiver<Item>,
    tcp: TcpStream,
    config: WebSocketConfig,
    linger: Duration,
) -> io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name(format!("keel-transport-writer-{}", conn.id))
        .spawn(move || run(&conn, &queue, tcp, config, linger))
}

fn run(conn: &Conn, queue: &Receiver<Item>, tcp: TcpStream, config: WebSocketConfig, linger: Duration) {
    let mut ws = WebSocketContext::new(Role::Server, Some(config));
    let mut out = WriteHalf(tcp);
    let mut close_sent: Option<Instant> = None;
    // Set when the socket is unusable or the peer ignored our Close: shut both halves so the
    // reader thread, blocked on the same socket, wakes up.
    let mut force = false;

    'run: loop {
        let first = match close_sent {
            None => match queue.recv() {
                Ok(item) => item,
                Err(_) => break,
            },
            Some(at) => match queue.recv_timeout(linger.saturating_sub(at.elapsed())) {
                Ok(item) => item,
                Err(RecvTimeoutError::Timeout) => {
                    force = true;
                    break;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            },
        };
        let mut next = Some(first);
        while let Some(item) = next {
            match item {
                Item::Frame(bytes) => {
                    conn.dequeued(bytes.len());
                    if close_sent.is_none() && ws.write(&mut out, Message::Binary(bytes)).is_err() {
                        force = true;
                        break 'run;
                    }
                }
                Item::Raw(bytes) => {
                    // Nothing follows our own Close frame, and what the read side wants to
                    // say (an echo of the peer's close) is then moot.
                    if close_sent.is_none()
                        && (ws.flush(&mut out).is_err()
                            || out.write_all(&bytes).is_err()
                            || out.flush().is_err())
                    {
                        force = true;
                        break 'run;
                    }
                }
                Item::Close { code, reason } => {
                    if close_sent.is_none() {
                        let frame = CloseFrame {
                            code: CloseCode::from(code),
                            reason: reason.into(),
                        };
                        if ws.close(&mut out, Some(frame)).is_err() {
                            force = true;
                            break 'run;
                        }
                        close_sent = Some(Instant::now());
                    }
                }
                Item::Stop => break 'run,
            }
            next = queue.try_recv().ok();
        }
        if ws.flush(&mut out).is_err() {
            force = true;
            break;
        }
    }

    if !force {
        // Everything queued before Stop still goes out.
        let _ = ws.flush(&mut out);
    }
    // A FIN after our last frame, so the peer sees the Close frame and then end-of-stream. A
    // forced shutdown also wakes the reader thread.
    let _ = out.0.shutdown(if force { Shutdown::Both } else { Shutdown::Write });
}
