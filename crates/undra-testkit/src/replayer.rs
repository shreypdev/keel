//! [`Replayer`]: answers a core's port calls from a [`Recording`], in order.
//!
//! The recording's `port_call` events, per port, are the script. When the core calls port `P`
//! method `M`, the replayer looks at the next recorded call of `P`: if it is `M` with the same
//! arguments, the recorded reply is the answer. Anything else is a typed [`ReplayError`], kept
//! for [`finish`](Replayer::finish), and the core is answered `Unavailable`, which is what a
//! missing port looks like to it. A deviation consumes nothing, so one wrong call does not
//! shift every later answer.
//!
//! Time is not replayed: recorded `t` values are metadata. Answers are immediate and the run is
//! deterministic.
//!
//! ```
//! use std::sync::Arc;
//! use undra_ports::{Kv, Http};
//! use undra_runtime::testing::TestRuntime;
//! use undra_testkit::{Event, EventKind, Recording, Replayer};
//! use undra_wire::payload::PortStatus;
//!
//! let kv_port = <dyn Kv as undra_runtime::Port>::PORT_ID;
//! let get = undra_meta::ids::port_method_id("Kv", "get");
//! let mut recording = Recording::new(0, "hand");
//! recording.events.push(Event { t: 0, kind: EventKind::PortCall { port: kv_port, method: get, call: 1, args: vec![1, 0, 0, 0, b'k'] } });
//! recording.events.push(Event { t: 1, kind: EventKind::PortReply { call: 1, status: PortStatus::Ok, body: vec![0] } });
//!
//! let t = TestRuntime::new();
//! let replayer = Arc::new(Replayer::new(&recording));
//! replayer.install(t.host());
//! let answer = t.runtime().port_call_sync(kv_port, get, &[1, 0, 0, 0, b'k']);
//! assert_eq!(answer, Ok(vec![0]));
//! assert!(replayer.finish().is_ok());
//! ```

use core::fmt;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use parking_lot::Mutex;
use undra_runtime::PortCallOutcome;
use undra_runtime::testing::{PortCallRecord, RecordingHost, port_reply};
use undra_wire::payload::PortStatus;

use crate::names::standard_name;
use crate::recording::{EventKind, Recording};

/// How strictly a replayed call must match the recorded one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgsPolicy {
    /// The encoded arguments must be byte for byte the recorded ones (the default).
    Exact,
    /// Only the port and method must match: for calls whose arguments carry something that changes
    /// between runs (a generated id, a time).
    Ignore,
}

/// Where a replay left the recording.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayError {
    /// The core called a different method (or the same method with other arguments) than the
    /// recording's next call of this port.
    Mismatch {
        /// The port id.
        port: u32,
        /// What the core called, `"Http.request"` or the ids.
        called: String,
        /// What the recording expected next, in the same form.
        expected: String,
        /// Whether the methods were the same and only the arguments differed.
        args_differ: bool,
        /// How many calls of this port had been answered before.
        nth: usize,
    },
    /// The core called a port more often than the recording did.
    Exhausted {
        /// The port id.
        port: u32,
        /// What the core called.
        called: String,
        /// How many calls of this port the recording held.
        recorded: usize,
    },
    /// The replay ended with recorded calls the core never made.
    Unconsumed {
        /// The port id.
        port: u32,
        /// The next call that was never made.
        next: String,
        /// How many were left.
        remaining: usize,
    },
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplayError::Mismatch {
                called,
                expected,
                args_differ,
                nth,
                ..
            } => {
                if *args_differ {
                    write!(
                        f,
                        "replay: call {nth} of the port was {called} with other arguments than the recording's"
                    )
                } else {
                    write!(
                        f,
                        "replay: call {nth} of the port was {called}, the recording has {expected} next"
                    )
                }
            }
            ReplayError::Exhausted {
                called, recorded, ..
            } => write!(
                f,
                "replay: {called} was called after the recording's {recorded} call(s) of the port were used up"
            ),
            ReplayError::Unconsumed {
                next, remaining, ..
            } => write!(
                f,
                "replay: {remaining} recorded call(s) were never made, the next is {next}"
            ),
        }
    }
}

impl std::error::Error for ReplayError {}

struct Expected {
    method: u32,
    args: Vec<u8>,
    /// `None` when the recording holds no reply for the call: it is answered `Unavailable`.
    reply: Option<(PortStatus, Vec<u8>)>,
}

#[derive(Default)]
struct State {
    queues: HashMap<u32, VecDeque<Expected>>,
    recorded: HashMap<u32, usize>,
    answered: HashMap<u32, usize>,
    errors: Vec<ReplayError>,
}

/// Answers port calls from a recording. See the [module documentation](self).
pub struct Replayer {
    policy: ArgsPolicy,
    state: Mutex<State>,
}

fn label(port: u32, method: u32) -> String {
    standard_name(port, method).unwrap_or_else(|| format!("port {port} method {method}"))
}

impl Replayer {
    /// A replayer over the port calls of `recording`, with [`ArgsPolicy::Exact`]. A call with no
    /// recorded reply is answered `Unavailable`.
    pub fn new(recording: &Recording) -> Replayer {
        Replayer::with_policy(recording, ArgsPolicy::Exact)
    }

    /// Like [`new`](Replayer::new) with `policy`.
    pub fn with_policy(recording: &Recording, policy: ArgsPolicy) -> Replayer {
        let mut state = State::default();
        // Replies are matched to their call by the core's call id, the first one after the call.
        let mut open: HashMap<u32, (u32, usize)> = HashMap::new();
        for event in &recording.events {
            match &event.kind {
                EventKind::PortCall {
                    port,
                    method,
                    call,
                    args,
                } => {
                    let queue = state.queues.entry(*port).or_default();
                    open.insert(*call, (*port, queue.len()));
                    queue.push_back(Expected {
                        method: *method,
                        args: args.clone(),
                        reply: None,
                    });
                    *state.recorded.entry(*port).or_default() += 1;
                }
                EventKind::PortReply { call, status, body } => {
                    if let Some((port, index)) = open.remove(call) {
                        if let Some(expected) =
                            state.queues.get_mut(&port).and_then(|q| q.get_mut(index))
                        {
                            expected.reply = Some((*status, body.clone()));
                        }
                    }
                }
                _ => {}
            }
        }
        Replayer {
            policy,
            state: Mutex::new(state),
        }
    }

    /// Answers one call. The result is what the platform's port callback returns: a
    /// `PortReply` payload for `port_call_id`, or `Unavailable`.
    pub fn answer(
        &self,
        port: u32,
        method: u32,
        port_call_id: u32,
        args: &[u8],
    ) -> PortCallOutcome {
        let mut state = self.state.lock();
        let nth = state.answered.get(&port).copied().unwrap_or(0);
        let recorded = state.recorded.get(&port).copied().unwrap_or(0);
        let Some(next) = state.queues.get(&port).and_then(VecDeque::front) else {
            state.errors.push(ReplayError::Exhausted {
                port,
                called: label(port, method),
                recorded,
            });
            return PortCallOutcome::Unavailable;
        };
        let same_method = next.method == method;
        let same_args = self.policy == ArgsPolicy::Ignore || next.args == args;
        if !(same_method && same_args) {
            let error = ReplayError::Mismatch {
                port,
                called: label(port, method),
                expected: label(port, next.method),
                args_differ: same_method,
                nth,
            };
            state.errors.push(error);
            return PortCallOutcome::Unavailable;
        }
        let Some(expected) = state.queues.get_mut(&port).and_then(VecDeque::pop_front) else {
            return PortCallOutcome::Unavailable;
        };
        *state.answered.entry(port).or_default() += 1;
        match expected.reply {
            Some((PortStatus::Unavailable, _)) | None => PortCallOutcome::Unavailable,
            Some((status, body)) => PortCallOutcome::Sync(port_reply(port_call_id, status, &body)),
        }
    }

    /// Answers every port call `host` receives from this replayer, as the
    /// [`RecordingHost`]'s default script (so the host still records the calls).
    pub fn install(self: &Arc<Self>, host: &RecordingHost) {
        let replayer = self.clone();
        host.script_port_default(move |call: &PortCallRecord| {
            replayer.answer(call.port_id, call.method_id, call.port_call_id, &call.args)
        });
    }

    /// The deviations seen so far.
    pub fn errors(&self) -> Vec<ReplayError> {
        self.state.lock().errors.clone()
    }

    /// How many recorded calls are still waiting to be made.
    pub fn remaining(&self) -> usize {
        self.state.lock().queues.values().map(VecDeque::len).sum()
    }

    /// Ends the replay: `Ok` when every call matched and every recorded call was made.
    ///
    /// # Errors
    ///
    /// Every [`ReplayError`] of the run: deviations in the order they happened, then one
    /// [`Unconsumed`](ReplayError::Unconsumed) per port with calls left.
    pub fn finish(&self) -> Result<(), Vec<ReplayError>> {
        let state = self.state.lock();
        let mut errors = state.errors.clone();
        let mut ports: Vec<_> = state.queues.iter().filter(|(_, q)| !q.is_empty()).collect();
        ports.sort_by_key(|(port, _)| **port);
        for (port, queue) in ports {
            if let Some(next) = queue.front() {
                errors.push(ReplayError::Unconsumed {
                    port: *port,
                    next: label(*port, next.method),
                    remaining: queue.len(),
                });
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl fmt::Debug for Replayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Replayer")
            .field("remaining", &self.remaining())
            .field("errors", &self.errors().len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::Event;
    use undra_meta::ids::{port_id, port_method_id};

    fn recording() -> Recording {
        let (kv, get, set) = (
            port_id("Kv"),
            port_method_id("Kv", "get"),
            port_method_id("Kv", "set"),
        );
        let http = (port_id("Http"), port_method_id("Http", "request"));
        let mut r = Recording::new(1, "test");
        let mut t = 0;
        let mut push = |kind| {
            r.events.push(Event { t, kind });
            t += 1;
        };
        push(EventKind::PortCall {
            port: kv,
            method: get,
            call: 1,
            args: vec![1],
        });
        push(EventKind::PortCall {
            port: http.0,
            method: http.1,
            call: 2,
            args: vec![],
        });
        push(EventKind::PortReply {
            call: 1,
            status: PortStatus::Ok,
            body: vec![0],
        });
        push(EventKind::PortReply {
            call: 2,
            status: PortStatus::Error,
            body: vec![5],
        });
        push(EventKind::PortCall {
            port: kv,
            method: set,
            call: 3,
            args: vec![2],
        });
        push(EventKind::PortReply {
            call: 3,
            status: PortStatus::Unavailable,
            body: vec![],
        });
        r
    }

    fn reply_body(outcome: PortCallOutcome) -> Option<Vec<u8>> {
        match outcome {
            PortCallOutcome::Sync(bytes) => Some(bytes),
            _ => None,
        }
    }

    #[test]
    fn answers_in_order_per_port_whatever_the_interleaving() {
        let rp = Replayer::new(&recording());
        let (kv, get, set) = (
            port_id("Kv"),
            port_method_id("Kv", "get"),
            port_method_id("Kv", "set"),
        );
        // Http first although it was recorded second: ports are independent queues.
        let http = rp.answer(port_id("Http"), port_method_id("Http", "request"), 40, &[]);
        assert_eq!(
            reply_body(http),
            Some(port_reply(40, PortStatus::Error, &[5]))
        );
        let first = rp.answer(kv, get, 41, &[1]);
        assert_eq!(
            reply_body(first),
            Some(port_reply(41, PortStatus::Ok, &[0]))
        );
        assert_eq!(rp.answer(kv, set, 42, &[2]), PortCallOutcome::Unavailable);
        assert_eq!(rp.finish(), Ok(()));
    }

    #[test]
    fn a_wrong_method_or_args_is_a_typed_error_and_consumes_nothing() {
        let rp = Replayer::new(&recording());
        let (kv, get, set) = (
            port_id("Kv"),
            port_method_id("Kv", "get"),
            port_method_id("Kv", "set"),
        );
        assert_eq!(rp.answer(kv, set, 1, &[1]), PortCallOutcome::Unavailable);
        assert_eq!(rp.answer(kv, get, 2, &[9]), PortCallOutcome::Unavailable);
        assert!(
            reply_body(rp.answer(kv, get, 3, &[1])).is_some(),
            "still the next call"
        );
        let errors = rp.errors();
        assert_eq!(errors.len(), 2);
        assert!(matches!(
            &errors[0],
            ReplayError::Mismatch { called, expected, args_differ: false, nth: 0, .. }
                if called == "Kv.set" && expected == "Kv.get"
        ));
        assert!(matches!(
            &errors[1],
            ReplayError::Mismatch {
                args_differ: true,
                ..
            }
        ));
        assert!(errors[1].to_string().contains("other arguments"));
        assert!(errors[0].to_string().contains("Kv.get next"));
    }

    #[test]
    fn exhaustion_and_unconsumed_calls_are_reported_by_finish() {
        let rp = Replayer::new(&recording());
        let kv = port_id("Kv");
        assert_eq!(
            rp.answer(port_id("Fs"), 1, 1, &[]),
            PortCallOutcome::Unavailable
        );
        assert_eq!(rp.remaining(), 3);
        let errors = rp.finish().unwrap_err();
        assert!(
            matches!(&errors[0], ReplayError::Exhausted { called, recorded: 0, .. } if called.starts_with("port "))
        );
        assert!(errors.iter().any(
            |e| matches!(e, ReplayError::Unconsumed { port, remaining: 2, .. } if *port == kv)
        ));
        assert!(errors.iter().any(|e| e.to_string().contains("never made")));
    }

    #[test]
    fn ignoring_args_matches_on_the_method_alone() {
        let rp = Replayer::with_policy(&recording(), ArgsPolicy::Ignore);
        let (kv, get) = (port_id("Kv"), port_method_id("Kv", "get"));
        assert!(reply_body(rp.answer(kv, get, 1, &[0xee])).is_some());
        assert!(rp.errors().is_empty());
    }

    #[test]
    fn a_call_with_no_recorded_reply_is_unavailable() {
        let mut r = Recording::new(1, "test");
        r.events.push(Event {
            t: 0,
            kind: EventKind::PortCall {
                port: 3,
                method: 4,
                call: 1,
                args: vec![],
            },
        });
        let rp = Replayer::new(&r);
        assert_eq!(rp.answer(3, 4, 9, &[]), PortCallOutcome::Unavailable);
        assert_eq!(
            rp.finish(),
            Ok(()),
            "it was made, the answer was just empty"
        );
    }
}
