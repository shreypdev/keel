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

/// What a keepalive check found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// The peer spoke recently enough.
    Quiet,
    /// The peer has been silent for a whole interval: send it a Ping.
    Ping,
    /// The peer has ignored three intervals' worth of Pings: drop it.
    Dead,
}

/// The keepalive schedule: one check per interval, each judging how long the peer has been
/// silent. It reads no clock itself; the caller says what time it is (the connection's
/// [`Clock`](crate::conn::Clock)), which is what lets a test place every tick exactly.
#[derive(Debug)]
struct Keepalive {
    interval: Duration,
    /// On the connection's clock.
    next_check: Duration,
}

impl Keepalive {
    /// A schedule whose first check is one `interval` after `now`; a zero interval is off.
    fn new(interval: Duration, now: Duration) -> Keepalive {
        Keepalive {
            interval,
            next_check: now + interval,
        }
    }

    fn enabled(&self) -> bool {
        !self.interval.is_zero()
    }

    /// How long until the next check is due.
    fn due_in(&self, now: Duration) -> Duration {
        self.next_check.saturating_sub(now)
    }

    /// Runs the check if one is due at `now`, and schedules the next. The silence is judged at
    /// `now`, so the whole check sees one reading of the clock.
    fn check(&mut self, conn: &Conn, now: Duration) -> Option<Verdict> {
        if !self.enabled() || now < self.next_check {
            return None;
        }
        self.next_check = now + self.interval;
        let silent = conn.silent_at(now);
        Some(if silent >= self.interval * 3 {
            Verdict::Dead
        } else if silent >= self.interval {
            Verdict::Ping
        } else {
            Verdict::Quiet
        })
    }
}

/// Starts the writer thread of `conn`.
pub(crate) fn spawn(
    conn: Arc<Conn>,
    queue: Receiver<Item>,
    tcp: TcpStream,
    config: WebSocketConfig,
    timing: Timing,
) -> io::Result<JoinHandle<()>> {
    // Scheduled here rather than on the new thread, so the first check is one interval after the
    // call whatever the thread scheduler does.
    let keepalive = Keepalive::new(timing.ping_interval, conn.now());
    thread::Builder::new()
        .name(format!("undra-transport-writer-{}", conn.id))
        .spawn(move || {
            // Nothing escapes as a panic (R6): a writer that dies takes its connection with it
            // rather than leaving the reader blocked on a socket nobody writes.
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                run(&conn, &queue, tcp, config, timing.linger, keepalive);
            }));
            if outcome.is_err() {
                conn.abort();
            }
        })
}

fn run(
    conn: &Conn,
    queue: &Receiver<Item>,
    tcp: TcpStream,
    config: WebSocketConfig,
    linger: Duration,
    mut keepalive: Keepalive,
) {
    let mut ws = WebSocketContext::new(Role::Server, Some(config));
    let mut out = WriteHalf(tcp);
    let mut close_sent: Option<Instant> = None;
    // Set when the socket is unusable, the peer ignored our Close or went silent: shut both
    // halves so the reader thread, blocked on the same socket, wakes up.
    let mut force = false;

    'run: loop {
        // What the next wait is bounded by: the linger after our Close, else the next
        // keepalive check.
        let bound = match close_sent {
            Some(at) => Some((at + linger).saturating_duration_since(Instant::now())),
            None if keepalive.enabled() => Some(keepalive.due_in(conn.now())),
            None => None,
        };
        let first = match bound {
            None => match queue.recv() {
                Ok(item) => Some(item),
                Err(_) => break,
            },
            Some(wait) => match queue.recv_timeout(wait) {
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

        if close_sent.is_none() {
            match keepalive.check(conn, conn.now()) {
                Some(Verdict::Dead) => {
                    force = true;
                    break;
                }
                Some(Verdict::Ping) => {
                    if ws.write(&mut out, Message::Ping(Vec::new())).is_err() {
                        force = true;
                        break;
                    }
                }
                Some(Verdict::Quiet) | None => {}
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

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::net::TcpListener;
    use std::sync::mpsc::Receiver;

    use crate::conn::ManualClock;
    use crate::ws::ReadHalf;

    use super::*;

    const INTERVAL: Duration = Duration::from_millis(150);
    const STEP: Duration = Duration::from_millis(50);

    /// A connection on a clock that only moves when the test says so, with its real read side
    /// over a loopback socket and the keepalive schedule the writer thread would run. The test
    /// plays the peer and the writer's clock tick, so nothing depends on how fast a thread runs.
    struct Rig {
        time: ManualClock,
        conn: Arc<Conn>,
        keepalive: Keepalive,
        reader: ReadHalf,
        peer: TcpStream,
        _queue: Receiver<Item>,
    }

    impl Rig {
        fn new(interval: Duration) -> Rig {
            let time = ManualClock::default();
            let (conn, queue) = Conn::with_clock(1, 1, 1 << 20, None, time.clock());
            let conn = Arc::new(conn);
            let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind a loopback port");
            let peer = TcpStream::connect(listener.local_addr().expect("its address")).expect("connect");
            let (served, _) = listener.accept().expect("accept");
            // Only a broken rig ever waits this long.
            served
                .set_read_timeout(Some(Duration::from_secs(10)))
                .expect("a read timeout");
            Rig {
                keepalive: Keepalive::new(interval, conn.now()),
                reader: ReadHalf::new(served, conn.clone()).expect("a read side"),
                time,
                conn,
                peer,
                _queue: queue,
            }
        }

        /// `by` passes; if `speak`, the peer sends two bytes and the read side takes them in (that
        /// is what marks the peer alive); then the writer's keepalive check runs.
        fn tick(&mut self, by: Duration, speak: bool) -> Option<Verdict> {
            self.time.advance(by);
            if speak {
                self.peer.write_all(&[0x82, 0x00]).expect("the peer sends");
                self.reader.read_exact(&mut [0_u8; 2]).expect("the read side receives");
            }
            self.keepalive.check(&self.conn, self.conn.now())
        }
    }

    #[test]
    fn a_chatty_client_is_never_pinged() {
        // The client sends every 50 ms, three times as often as the 150 ms interval, for 600 ms.
        let mut rig = Rig::new(INTERVAL);
        let verdicts: Vec<Option<Verdict>> = (0..12).map(|_| rig.tick(STEP, true)).collect();
        // A check falls due at 150, 300, 450 and 600 ms and finds the client alive every time:
        // never a Ping, never a drop.
        let quiet = Some(Verdict::Quiet);
        assert_eq!(
            verdicts,
            [None, None, quiet, None, None, quiet, None, None, quiet, None, None, quiet]
        );
    }

    #[test]
    fn a_client_that_goes_silent_is_pinged_and_then_dropped() {
        let mut rig = Rig::new(INTERVAL);
        let verdicts: Vec<Option<Verdict>> = (0..9).map(|_| rig.tick(STEP, false)).collect();
        // Silent for one interval: Ping, and again each interval; silent for three: dropped.
        let ping = Some(Verdict::Ping);
        assert_eq!(
            verdicts,
            [None, None, ping, None, None, ping, None, None, Some(Verdict::Dead)]
        );
    }

    #[test]
    fn a_client_that_speaks_after_a_ping_is_spared_the_drop() {
        let mut rig = Rig::new(INTERVAL);
        assert_eq!(rig.tick(INTERVAL, false), Some(Verdict::Ping));
        assert_eq!(rig.tick(STEP, true), None);
        // 100 ms since it spoke when the next check falls due: not even a second Ping.
        assert_eq!(rig.tick(Duration::from_millis(100), false), Some(Verdict::Quiet));
    }

    #[test]
    fn a_zero_interval_switches_keepalive_off() {
        let mut rig = Rig::new(Duration::ZERO);
        assert!(!rig.keepalive.enabled());
        for _ in 0..100 {
            assert_eq!(rig.tick(Duration::from_secs(3600), false), None);
        }
    }

    #[test]
    fn the_wait_for_the_next_check_is_what_remains_of_the_interval() {
        let rig = Rig::new(INTERVAL);
        assert_eq!(rig.keepalive.due_in(rig.conn.now()), INTERVAL);
        rig.time.advance(STEP);
        assert_eq!(rig.keepalive.due_in(rig.conn.now()), Duration::from_millis(100));
        rig.time.advance(Duration::from_secs(1)); // overdue: due now, not in the past
        assert_eq!(rig.keepalive.due_in(rig.conn.now()), Duration::ZERO);
    }
}
