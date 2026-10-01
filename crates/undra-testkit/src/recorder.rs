//! [`Recorder`]: writes a [`Recording`] from the payloads that cross the boundary.
//!
//! Three things feed it. `undra dev --record` taps the dev server's envelopes with
//! [`Recorder::record_envelope`]; a Rust test wraps its host with [`Recorder::host`] to record
//! what the runtime tells it; and a test that plays the platform records the answers it gives
//! with [`Recorder::record_envelope`] too. The time of an event is read from the recorder's
//! clock, so a recorder built on a manual clock writes the same bytes every run.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use parking_lot::Mutex;
use undra_ports::{Clock, Rng};
use undra_runtime::testing::{TestRuntime, call_payload};
use undra_runtime::{Host, PortCallOutcome};
use undra_wire::payload::{
    Call, CallTarget, Cancel, ChangeSet, Event as WireEvent, Observe, PortCall, PortReply,
    PortStatus, Release, Reply, StreamItem, TimerFired,
};
use undra_wire::{Bytes, Encode, Kind, Reader};

use crate::recording::{Entry, Event, EventKind, Recording, RecordingError, Target};

type Now = Box<dyn Fn() -> u64 + Send + Sync>;

/// Collects a [`Recording`]. Cheap to share: every method takes `&self`.
pub struct Recorder {
    now: Now,
    start: u64,
    state: Mutex<Recording>,
}

impl Recorder {
    /// A recorder whose times are real milliseconds since this call.
    pub fn new(schema_hash: u64, source: impl Into<String>) -> Recorder {
        let origin = Instant::now();
        Recorder::with_clock(schema_hash, source, move || {
            u64::try_from(origin.elapsed().as_millis()).unwrap_or(u64::MAX)
        })
    }

    /// A recorder whose times are `now()` minus its first reading: pass a fake clock's reading
    /// (`|| fakes.clock.monotonic_ns() / 1_000_000`) and the recording is the same every run.
    pub fn with_clock(
        schema_hash: u64,
        source: impl Into<String>,
        now: impl Fn() -> u64 + Send + Sync + 'static,
    ) -> Recorder {
        let start = now();
        Recorder {
            now: Box::new(now),
            start,
            state: Mutex::new(Recording::new(schema_hash, source)),
        }
    }

    /// Names the host platform in the recording (informational).
    #[must_use]
    pub fn with_platform(self, platform: impl Into<String>) -> Recorder {
        self.state.lock().platform = Some(platform.into());
        self
    }

    /// Names the host platform in the recording (informational); the dev server learns it from
    /// the client's `Hello`, after the recorder exists.
    pub fn set_platform(&self, platform: impl Into<String>) {
        self.state.lock().platform = Some(platform.into());
    }

    /// Appends `kind`, stamped with the time since the recorder started. Times never go
    /// backwards, even if the clock does.
    pub fn push(&self, kind: EventKind) {
        let t = (self.now)().saturating_sub(self.start);
        let mut state = self.state.lock();
        let t = state.events.last().map_or(t, |last| last.t.max(t));
        state.events.push(Event { t, kind });
    }

    /// How many events have been recorded.
    pub fn len(&self) -> usize {
        self.state.lock().events.len()
    }

    /// Whether nothing has been recorded.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A copy of the recording so far.
    pub fn finish(&self) -> Recording {
        self.state.lock().clone()
    }

    /// Records the envelope of `kind` whose payload is `payload` (SPEC 3.2). Returns `Ok(false)`
    /// for the kinds a recording leaves out (`Hello`, `Log`, `StreamCredit`, `Snapshot`,
    /// `Restore`).
    ///
    /// # Errors
    ///
    /// [`RecordingError::Payload`] if the payload does not decode; nothing is recorded then.
    pub fn record_envelope(&self, kind: Kind, payload: &[u8]) -> Result<bool, RecordingError> {
        let bad =
            |what: &str, e: undra_wire::WireError| RecordingError::Payload(format!("{what}: {e}"));
        let mut r = Reader::new(payload);
        let event = match kind {
            Kind::Call => {
                let call = Call::decode(&mut r).map_err(|e| bad("Call", e))?;
                EventKind::Call {
                    target: match call.target {
                        CallTarget::Function { method_id } => {
                            Target::Function { method: method_id }
                        }
                        CallTarget::Method { handle, method_id } => Target::Method {
                            handle: handle.0,
                            method: method_id,
                        },
                        CallTarget::Constructor { type_id, method_id } => Target::Constructor {
                            type_id,
                            method: method_id,
                        },
                        CallTarget::LazyPage {
                            handle,
                            offset,
                            limit,
                        } => Target::LazyPage {
                            handle: handle.0,
                            offset,
                            limit,
                        },
                    },
                    call: call.call_id,
                    args: call.args.to_vec(),
                }
            }
            Kind::Reply => {
                let reply = Reply::decode(&mut r).map_err(|e| bad("Reply", e))?;
                EventKind::Reply {
                    call: reply.call_id,
                    status: reply.status,
                    body: reply.body.to_vec(),
                }
            }
            Kind::ChangeSet => {
                let cs = ChangeSet::decode(&mut r).map_err(|e| bad("ChangeSet", e))?;
                EventKind::ChangeSet {
                    txn: cs.txn_id,
                    entries: cs
                        .entries
                        .into_iter()
                        .map(|e| Entry {
                            handle: e.handle.0,
                            signal: e.signal_id,
                            op: e.op,
                            value: e.value,
                        })
                        .collect(),
                }
            }
            Kind::StreamItem => {
                let item = StreamItem::decode(&mut r).map_err(|e| bad("StreamItem", e))?;
                EventKind::StreamItem {
                    call: item.call_id,
                    flag: item.flag,
                    body: item.body.to_vec(),
                }
            }
            Kind::PortCall => {
                let call = PortCall::decode(&mut r).map_err(|e| bad("PortCall", e))?;
                EventKind::PortCall {
                    port: call.port_id,
                    method: call.method_id,
                    call: call.port_call_id,
                    args: call.args.to_vec(),
                }
            }
            Kind::PortReply => {
                let reply = PortReply::decode(&mut r).map_err(|e| bad("PortReply", e))?;
                EventKind::PortReply {
                    call: reply.port_call_id,
                    status: reply.status,
                    body: reply.body.to_vec(),
                }
            }
            Kind::Event => {
                let event = WireEvent::decode(&mut r).map_err(|e| bad("Event", e))?;
                EventKind::PortEvent {
                    port: event.port_id,
                    method: event.method_id,
                    payload: event.payload.to_vec(),
                }
            }
            Kind::TimerFired => EventKind::TimerFired {
                timer: TimerFired::decode(&mut r)
                    .map_err(|e| bad("TimerFired", e))?
                    .timer_id,
            },
            Kind::Observe => {
                let o = Observe::decode(&mut r).map_err(|e| bad("Observe", e))?;
                EventKind::Observe {
                    handle: o.handle.0,
                    signal: o.signal_id,
                    on: o.on,
                }
            }
            Kind::Release => EventKind::Release {
                handle: Release::decode(&mut r)
                    .map_err(|e| bad("Release", e))?
                    .handle
                    .0,
            },
            Kind::Cancel => EventKind::Cancel {
                call: Cancel::decode(&mut r)
                    .map_err(|e| bad("Cancel", e))?
                    .call_id,
            },
            Kind::Hello | Kind::Log | Kind::StreamCredit | Kind::Snapshot | Kind::Restore => {
                return Ok(false);
            }
        };
        self.push(event);
        Ok(true)
    }

    /// Records a call and makes it on `t`: the way a test plays the host and records itself.
    /// Returns `Runtime::call`'s result (0 accepted).
    pub fn call(&self, t: &TestRuntime, target: CallTarget, call_id: u32, args: &[u8]) -> u32 {
        let payload = call_payload(target, call_id, args);
        let _ = self.record_envelope(Kind::Call, &payload);
        t.runtime().call(&payload)
    }

    /// Records and makes an `observe` on `t` (`signal` may be `u32::MAX` for every signal).
    pub fn observe(&self, t: &TestRuntime, handle: u64, signal: u32, on: bool) {
        self.push(EventKind::Observe { handle, signal, on });
        t.runtime().observe(handle, signal, on);
    }

    /// Wraps `inner` in a [`Host`] that records every reply, change-set, stream item and port
    /// call the runtime emits (and the answer of a port call that is answered inline), then
    /// forwards it. A port call answered later is recorded by whoever answers it:
    /// `recorder.record_envelope(Kind::PortReply, ..)`.
    pub fn host<H: Host>(self: &Arc<Self>, inner: Arc<H>) -> Arc<RecordingTap<H>> {
        Arc::new(RecordingTap {
            recorder: self.clone(),
            inner,
        })
    }
}

impl core::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Recorder")
            .field("events", &self.len())
            .finish_non_exhaustive()
    }
}

/// The first call id of the port calls a [`RecordingClock`] or [`RecordingRng`] writes, which
/// never crossed a host (the core's own bindings): above any id the runtime hands out.
const SYNTHETIC_CALL_IDS: u32 = 0x8000_0000;

fn record_native_call(
    rec: &Recorder,
    next: &AtomicU32,
    port: &str,
    method: &str,
    args: Vec<u8>,
    body: Vec<u8>,
) {
    let call = SYNTHETIC_CALL_IDS + next.fetch_add(1, Ordering::Relaxed);
    rec.push(EventKind::PortCall {
        port: undra_meta::ids::port_id(port),
        method: undra_meta::ids::port_method_id(port, method),
        call,
        args,
    });
    rec.push(EventKind::PortReply {
        call,
        status: PortStatus::Ok,
        body,
    });
}

/// A [`Clock`] that records every reading it gives, so a recording made where the core's `Clock`
/// is a native binding (the dev runner: a synchronous port cannot cross the socket) holds what
/// the core saw.
pub struct RecordingClock {
    rec: Arc<Recorder>,
    inner: Arc<dyn Clock>,
    next: AtomicU32,
}

impl RecordingClock {
    /// Wraps `inner`.
    pub fn new(rec: Arc<Recorder>, inner: Arc<dyn Clock>) -> RecordingClock {
        RecordingClock {
            rec,
            inner,
            next: AtomicU32::new(0),
        }
    }
}

impl Clock for RecordingClock {
    fn now_ms(&self) -> i64 {
        let now = self.inner.now_ms();
        record_native_call(
            &self.rec,
            &self.next,
            "Clock",
            "now_ms",
            Vec::new(),
            now.to_le_bytes().to_vec(),
        );
        now
    }

    fn monotonic_ns(&self) -> u64 {
        let now = self.inner.monotonic_ns();
        record_native_call(
            &self.rec,
            &self.next,
            "Clock",
            "monotonic_ns",
            Vec::new(),
            now.to_le_bytes().to_vec(),
        );
        now
    }
}

/// An [`Rng`] that records every fill it serves (see [`RecordingClock`]).
pub struct RecordingRng {
    rec: Arc<Recorder>,
    inner: Arc<dyn Rng>,
    next: AtomicU32,
}

impl RecordingRng {
    /// Wraps `inner`.
    pub fn new(rec: Arc<Recorder>, inner: Arc<dyn Rng>) -> RecordingRng {
        RecordingRng {
            rec,
            inner,
            next: AtomicU32::new(0),
        }
    }
}

impl Rng for RecordingRng {
    fn fill(&self, len: u32) -> Bytes {
        let bytes = self.inner.fill(len);
        record_native_call(
            &self.rec,
            &self.next,
            "Rng",
            "fill",
            len.to_le_bytes().to_vec(),
            bytes.encode_to_vec(),
        );
        bytes
    }
}

/// A [`Host`] that records what the runtime tells `inner`, from [`Recorder::host`].
pub struct RecordingTap<H: Host> {
    recorder: Arc<Recorder>,
    inner: Arc<H>,
}

impl<H: Host> RecordingTap<H> {
    /// The wrapped host.
    pub fn inner(&self) -> &Arc<H> {
        &self.inner
    }
}

impl<H: Host> Host for RecordingTap<H> {
    fn reply(&self, call_id: u32, payload: &[u8]) {
        let _ = self.recorder.record_envelope(Kind::Reply, payload);
        self.inner.reply(call_id, payload);
    }

    fn change_set(&self, payload: &[u8]) {
        let _ = self.recorder.record_envelope(Kind::ChangeSet, payload);
        self.inner.change_set(payload);
    }

    fn stream_item(&self, call_id: u32, payload: &[u8]) {
        let _ = self.recorder.record_envelope(Kind::StreamItem, payload);
        self.inner.stream_item(call_id, payload);
    }

    fn port_call(
        &self,
        port_id: u32,
        method_id: u32,
        port_call_id: u32,
        args: &[u8],
    ) -> PortCallOutcome {
        self.recorder.push(EventKind::PortCall {
            port: port_id,
            method: method_id,
            call: port_call_id,
            args: args.to_vec(),
        });
        let outcome = self.inner.port_call(port_id, method_id, port_call_id, args);
        match &outcome {
            PortCallOutcome::Sync(reply) => {
                let _ = self.recorder.record_envelope(Kind::PortReply, reply);
            }
            PortCallOutcome::Unavailable => self.recorder.push(EventKind::PortReply {
                call: port_call_id,
                status: PortStatus::Unavailable,
                body: Vec::new(),
            }),
            PortCallOutcome::Async => {}
        }
        outcome
    }

    fn log(&self, level: u8, target: &str, message: &str) {
        self.inner.log(level, target, message);
    }

    fn schedule(&self) {
        self.inner.schedule();
    }

    fn timer_set(&self, timer_id: u32, delay_ms: u64) -> bool {
        self.inner.timer_set(timer_id, delay_ms)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use undra_runtime::testing::{RecordingHost, call_payload, port_reply_ok};
    use undra_wire::Writer;

    use super::*;

    fn manual() -> (Arc<AtomicU64>, Recorder) {
        let now = Arc::new(AtomicU64::new(1_000));
        let read = now.clone();
        (
            now,
            Recorder::with_clock(7, "test", move || read.load(Ordering::SeqCst)),
        )
    }

    #[test]
    fn times_are_relative_to_the_first_reading_and_never_go_back() {
        let (now, rec) = manual();
        rec.push(EventKind::Cancel { call: 1 });
        now.store(1_250, Ordering::SeqCst);
        rec.push(EventKind::Cancel { call: 2 });
        now.store(900, Ordering::SeqCst);
        rec.push(EventKind::Cancel { call: 3 });
        let times: Vec<u64> = rec.finish().events.iter().map(|e| e.t).collect();
        assert_eq!(times, [0, 250, 250]);
        assert_eq!(rec.len(), 3);
        assert!(!rec.is_empty());
    }

    #[test]
    fn envelopes_decode_into_events_and_unrecorded_kinds_are_skipped() {
        let (_, rec) = manual();
        let call = call_payload(CallTarget::Function { method_id: 5 }, 9, &[1, 2]);
        assert_eq!(rec.record_envelope(Kind::Call, &call), Ok(true));
        assert_eq!(rec.record_envelope(Kind::Hello, &[]), Ok(false));
        assert_eq!(rec.record_envelope(Kind::Log, &[]), Ok(false));
        let pr = port_reply_ok(3, &[4]);
        assert_eq!(rec.record_envelope(Kind::PortReply, &pr), Ok(true));
        let kinds: Vec<EventKind> = rec.finish().events.into_iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            [
                EventKind::Call {
                    target: Target::Function { method: 5 },
                    call: 9,
                    args: vec![1, 2]
                },
                EventKind::PortReply {
                    call: 3,
                    status: PortStatus::Ok,
                    body: vec![4]
                }
            ]
        );
        assert!(matches!(
            rec.record_envelope(Kind::Reply, &[1, 2]),
            Err(RecordingError::Payload(_))
        ));
        assert_eq!(
            rec.len(),
            2,
            "a payload that does not decode records nothing"
        );
    }

    #[test]
    fn every_kind_decodes() {
        let (_, rec) = manual();
        let mut w = Writer::new();
        ChangeSet {
            txn_id: 4,
            entries: vec![undra_wire::payload::ChangeEntry {
                handle: undra_wire::Handle(0x1_0000_0001),
                signal_id: 2,
                op: undra_wire::payload::ChangeOp::Full,
                value: vec![1],
            }],
        }
        .encode(&mut w);
        assert_eq!(rec.record_envelope(Kind::ChangeSet, w.as_slice()), Ok(true));
        for (kind, bytes) in [
            (Kind::Cancel, 3_u32.to_le_bytes().to_vec()),
            (Kind::TimerFired, 5_u32.to_le_bytes().to_vec()),
            (Kind::Release, 9_u64.to_le_bytes().to_vec()),
        ] {
            assert_eq!(rec.record_envelope(kind, &bytes), Ok(true), "{kind:?}");
        }
        let mut obs = 9_u64.to_le_bytes().to_vec();
        obs.extend_from_slice(&u32::MAX.to_le_bytes());
        obs.push(1);
        assert_eq!(rec.record_envelope(Kind::Observe, &obs), Ok(true));
        assert_eq!(rec.len(), 5);
    }

    #[test]
    fn a_recording_clock_and_rng_record_what_the_core_read() {
        let (_, rec) = manual();
        let rec = Arc::new(rec);
        let clock = RecordingClock::new(
            rec.clone(),
            Arc::new(undra_ports::fakes::FakeClock::with_now_ms(77)),
        );
        let rng = RecordingRng::new(rec.clone(), Arc::new(undra_ports::fakes::SeededRng::new(1)));
        assert_eq!(clock.now_ms(), 77);
        assert_eq!(clock.monotonic_ns(), 0);
        assert_eq!(rng.fill(3).0.len(), 3);
        let events = rec.finish().events;
        assert_eq!(events.len(), 6);
        let replayer = crate::Replayer::new(&rec.finish());
        let clock_port = undra_meta::ids::port_id("Clock");
        let now = undra_meta::ids::port_method_id("Clock", "now_ms");
        assert!(matches!(
            replayer.answer(clock_port, now, 5, &[]),
            PortCallOutcome::Sync(_)
        ));
        rec.set_platform("web");
        assert_eq!(rec.finish().platform.as_deref(), Some("web"));
    }

    #[test]
    fn the_host_tap_records_what_the_runtime_tells_the_host_and_forwards_it() {
        let (_, rec) = manual();
        let rec = Arc::new(rec);
        let inner = Arc::new(RecordingHost::new());
        inner.script_port_ok(1, 2, vec![9]);
        let tap = rec.host(inner.clone());
        assert!(matches!(
            tap.port_call(1, 2, 11, &[7]),
            PortCallOutcome::Sync(_)
        ));
        assert_eq!(tap.port_call(5, 6, 12, &[]), PortCallOutcome::Unavailable);
        tap.change_set(&{
            let mut w = Writer::new();
            ChangeSet {
                txn_id: 1,
                entries: vec![],
            }
            .encode(&mut w);
            w.into_vec()
        });
        let events = rec.finish().events;
        assert_eq!(events.len(), 5);
        assert!(matches!(
            events[0].kind,
            EventKind::PortCall { call: 11, .. }
        ));
        assert!(matches!(
            events[1].kind,
            EventKind::PortReply {
                call: 11,
                status: PortStatus::Ok,
                ..
            }
        ));
        assert!(matches!(
            events[3].kind,
            EventKind::PortReply {
                call: 12,
                status: PortStatus::Unavailable,
                ..
            }
        ));
        assert_eq!(inner.port_calls().len(), 2, "forwarded");
        assert_eq!(inner.change_set_count(), 1);
    }
}
