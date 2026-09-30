//! The writer thread: the only thing that writes to a connection's socket.
//!
//! It drains the outbound queue in order. Envelopes become binary messages; raw frames from
//! the read side go out between them; a Close item sends a Close frame and starts a linger
//! timer, after which the socket is shut down whether or not the peer answered; Stop ends it.
//! Consecutive messages are batched into one `write` and flushed when the queue runs dry.
//!
//! It also keeps the connection honest: a peer that has been silent for a while is pinged, and
//! one that stays silent (the phone left the Wi-Fi, the laptop lid closed, the tab was killed
//! without a FIN) is dropped, so that a dead client cannot hold the one client slot.

use std::io::{self, Write};
use std::net::{Shutdown, TcpStream};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::{CloseFrame, Message, Role, WebSocketConfig, WebSocketContext};

use crate::conn::{Conn, Item};
use crate::ws::WriteHalf;

/// The writer's clocks.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timing {
    /// How long to wait for the peer after sending a Close frame.
    pub(crate) linger: Duration,
    /// After this much silence from the peer send a Ping; after three times as much drop it.
    /// Zero switches keepalive off.
    pub(crate) ping_interval: Duration,
}

/// Starts the writer thread of `conn`.
pub(crate) fn spawn(
    conn: Arc<Conn>,
    queue: Receiver<Item>,
    tcp: TcpStream,
    config: WebSocketConfig,
    timing: Timing,
) -> io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name(format!("undra-transport-writer-{}", conn.id))
        .spawn(move || {
            // Nothing escapes as a panic (R6): a writer that dies takes its connection with it
            // rather than leaving the reader blocked on a socket nobody writes.
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                run(&conn, &queue, tcp, config, timing);
            }));
            if outcome.is_err() {
                conn.abort();
            }
        })
}

fn run(conn: &Conn, queue: &Receiver<Item>, tcp: TcpStream, config: WebSocketConfig, timing: Timing) {
    let mut ws = WebSocketContext::new(Role::Server, Some(config));
    let mut out = WriteHalf(tcp);
    let mut close_sent: Option<Instant> = None;
    // Set when the socket is unusable, the peer ignored our Close or went silent: shut both
    // halves so the reader thread, blocked on the same socket, wakes up.
    let mut force = false;
    let keepalive = !timing.ping_interval.is_zero();
    let mut next_check = Instant::now() + timing.ping_interval;

    'run: loop {
        // What the next wait is bounded by: the linger after our Close, else the next
        // keepalive check.
        let bound = match close_sent {
            Some(at) => Some(at + timing.linger),
            None if keepalive => Some(next_check),
            None => None,
        };
        let first = match bound {
            None => match queue.recv() {
                Ok(item) => Some(item),
                Err(_) => break,
            },
            Some(deadline) => match queue.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(item) => Some(item),
                Err(RecvTimeoutError::Timeout) if close_sent.is_some() => {
                    force = true;
                    break;
                }
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            },
        };
        let mut next = first;
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

        if keepalive && close_sent.is_none() && Instant::now() >= next_check {
            next_check = Instant::now() + timing.ping_interval;
            let silent = conn.silent_for();
            if silent >= timing.ping_interval * 3 {
                force = true;
                break;
            }
            if silent >= timing.ping_interval && ws.write(&mut out, Message::Ping(Vec::new())).is_err() {
                force = true;
                break;
            }
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
    let _ = out
        .0
        .shutdown(if force { Shutdown::Both } else { Shutdown::Write });
}
