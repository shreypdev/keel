//! [`ScriptedConnectivity`] and [`ScriptedLifecycle`]: the platform side of the two event ports.

use std::collections::VecDeque;
use std::sync::{Arc, Weak};

use parking_lot::Mutex;
use undra_runtime::{Port, Runtime};

use crate::{
    AppState, Connectivity, Lifecycle, NetKind, encode_connectivity_changed_event,
    encode_lifecycle_changed_event,
};

/// The shared machinery of a scripted event source: the current value, a queue of scripted
/// values, the history of what was emitted and the runtimes to deliver to.
struct Source<T: Copy> {
    state: Mutex<SourceState<T>>,
    /// Encodes a value and delivers it to one runtime.
    deliver: fn(&Runtime, T),
}

struct SourceState<T> {
    current: T,
    queue: VecDeque<T>,
    history: Vec<T>,
    runtimes: Vec<Weak<Runtime>>,
}

impl<T: Copy> Source<T> {
    fn new(initial: T, deliver: fn(&Runtime, T)) -> Source<T> {
        Source {
            state: Mutex::new(SourceState {
                current: initial,
                queue: VecDeque::new(),
                history: Vec::new(),
                runtimes: Vec::new(),
            }),
            deliver,
        }
    }

    fn attach(&self, rt: &Arc<Runtime>) {
        let mut state = self.state.lock();
        state.runtimes.retain(|weak| weak.strong_count() > 0);
        state.runtimes.push(Arc::downgrade(rt));
    }

    /// Delivers `value` to every attached runtime, with no lock held.
    fn emit(&self, value: T) {
        let runtimes: Vec<Arc<Runtime>> = self
            .state
            .lock()
            .runtimes
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        for rt in runtimes {
            (self.deliver)(&rt, value);
        }
    }

    /// Makes `value` current, records it and delivers it.
    fn set(&self, value: T) {
        {
            let mut state = self.state.lock();
            state.current = value;
            state.history.push(value);
        }
        self.emit(value);
    }

    fn script(&self, values: impl IntoIterator<Item = T>) {
        self.state.lock().queue.extend(values);
    }

    fn step(&self) -> bool {
        let next = self.state.lock().queue.pop_front();
        match next {
            Some(value) => {
                self.set(value);
                true
            }
            None => false,
        }
    }

    fn play(&self) -> usize {
        let mut count = 0;
        while self.step() {
            count += 1;
        }
        count
    }

    fn pending(&self) -> usize {
        self.state.lock().queue.len()
    }

    fn emit_current(&self) {
        let current = self.state.lock().current;
        self.emit(current);
    }

    fn current(&self) -> T {
        self.state.lock().current
    }

    fn history(&self) -> Vec<T> {
        self.state.lock().history.clone()
    }
}

fn deliver_connectivity(rt: &Runtime, (online, kind): (bool, NetKind)) {
    rt.event(
        <dyn Connectivity as Port>::PORT_ID,
        undra_meta::ids::port_method_id("Connectivity", "changed"),
        &encode_connectivity_changed_event(online, kind),
    );
}

fn deliver_lifecycle(rt: &Runtime, state: AppState) {
    rt.event(
        <dyn Lifecycle as Port>::PORT_ID,
        undra_meta::ids::port_method_id("Lifecycle", "changed"),
        &encode_lifecycle_changed_event(state),
    );
}

/// The platform side of [`Connectivity`]: emits connectivity events into attached runtimes,
/// immediately or from a script.
///
/// It starts out online on Wi-Fi. Nothing is emitted until you say so (real platforms report
/// their initial state right after start-up; call [`emit_current`](Self::emit_current) to do the
/// same). Events are delivered synchronously on the calling thread, exactly as
/// `Runtime::event` does for a real host, so drive it from test code, not from inside a task
/// the runtime is polling (the runtime refuses re-entrant calls).
///
/// ```
/// use std::sync::{Arc, Mutex};
/// use undra_ports::{NetKind, on_connectivity_changed};
/// use undra_ports::fakes::ScriptedConnectivity;
/// use undra_runtime::testing::TestRuntime;
///
/// let t = TestRuntime::new();
/// let net = ScriptedConnectivity::new();
/// net.attach(t.runtime());
///
/// let seen = Arc::new(Mutex::new(Vec::new()));
/// let sink = seen.clone();
/// let _subscription = on_connectivity_changed(&t.ctx(), move |_ctx, online, kind| {
///     sink.lock().unwrap().push((online, kind));
/// });
///
/// net.go_offline();
/// net.script([(true, NetKind::Cellular), (true, NetKind::Wifi)]);
/// assert_eq!(net.play(), 2);
/// assert_eq!(
///     *seen.lock().unwrap(),
///     [(false, NetKind::None), (true, NetKind::Cellular), (true, NetKind::Wifi)]
/// );
/// ```
pub struct ScriptedConnectivity {
    source: Source<(bool, NetKind)>,
}

impl ScriptedConnectivity {
    /// A source that is online on Wi-Fi and has emitted nothing.
    pub fn new() -> ScriptedConnectivity {
        ScriptedConnectivity {
            source: Source::new((true, NetKind::Wifi), deliver_connectivity),
        }
    }

    /// Delivers this source's events to `rt` from now on. Several runtimes may be attached; a
    /// dropped runtime is forgotten.
    pub fn attach(&self, rt: &Arc<Runtime>) {
        self.source.attach(rt);
    }

    /// Becomes `(online, kind)`, records it in the [history](Self::history) and emits it.
    pub fn set(&self, online: bool, kind: NetKind) {
        self.source.set((online, kind));
    }

    /// `set(false, NetKind::None)`.
    pub fn go_offline(&self) {
        self.set(false, NetKind::None);
    }

    /// `set(true, kind)`.
    pub fn go_online(&self, kind: NetKind) {
        self.set(true, kind);
    }

    /// Queues `(online, kind)` states to be emitted one at a time by [`step`](Self::step) or all
    /// at once by [`play`](Self::play).
    pub fn script(&self, states: impl IntoIterator<Item = (bool, NetKind)>) {
        self.source.script(states);
    }

    /// Emits the next queued state. Returns `false` if the queue was empty.
    pub fn step(&self) -> bool {
        self.source.step()
    }

    /// Emits every queued state and returns how many there were.
    pub fn play(&self) -> usize {
        self.source.play()
    }

    /// How many queued states have not been emitted yet.
    pub fn pending(&self) -> usize {
        self.source.pending()
    }

    /// Emits the current state again without recording it: the platform's initial report.
    pub fn emit_current(&self) {
        self.source.emit_current();
    }

    /// The state the device is in now.
    pub fn current(&self) -> (bool, NetKind) {
        self.source.current()
    }

    /// Every state emitted so far, oldest first.
    pub fn history(&self) -> Vec<(bool, NetKind)> {
        self.source.history()
    }
}

impl Default for ScriptedConnectivity {
    fn default() -> ScriptedConnectivity {
        ScriptedConnectivity::new()
    }
}

impl core::fmt::Debug for ScriptedConnectivity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ScriptedConnectivity")
            .field("current", &self.current())
            .field("pending", &self.pending())
            .finish_non_exhaustive()
    }
}

impl Connectivity for ScriptedConnectivity {
    fn changed(&self, online: bool, kind: NetKind) {
        self.set(online, kind);
    }
}

/// The platform side of [`Lifecycle`]: emits app lifecycle events into attached runtimes,
/// immediately or from a script. It starts out [`AppState::Active`] and emits nothing until
/// told to. See [`ScriptedConnectivity`] for the shared behaviour.
///
/// ```
/// use std::sync::{Arc, Mutex};
/// use undra_ports::{AppState, on_lifecycle_changed};
/// use undra_ports::fakes::ScriptedLifecycle;
/// use undra_runtime::testing::TestRuntime;
///
/// let t = TestRuntime::new();
/// let app = ScriptedLifecycle::new();
/// app.attach(t.runtime());
/// let seen = Arc::new(Mutex::new(Vec::new()));
/// let sink = seen.clone();
/// let _subscription = on_lifecycle_changed(&t.ctx(), move |_ctx, state| sink.lock().unwrap().push(state));
///
/// app.set(AppState::Background);
/// app.set(AppState::Active);
/// assert_eq!(*seen.lock().unwrap(), [AppState::Background, AppState::Active]);
/// ```
pub struct ScriptedLifecycle {
    source: Source<AppState>,
}

impl ScriptedLifecycle {
    /// A source that is active and has emitted nothing.
    pub fn new() -> ScriptedLifecycle {
        ScriptedLifecycle {
            source: Source::new(AppState::Active, deliver_lifecycle),
        }
    }

    /// Delivers this source's events to `rt` from now on.
    pub fn attach(&self, rt: &Arc<Runtime>) {
        self.source.attach(rt);
    }

    /// Becomes `state`, records it in the [history](Self::history) and emits it.
    pub fn set(&self, state: AppState) {
        self.source.set(state);
    }

    /// Queues states to be emitted one at a time by [`step`](Self::step) or all at once by
    /// [`play`](Self::play).
    pub fn script(&self, states: impl IntoIterator<Item = AppState>) {
        self.source.script(states);
    }

    /// Emits the next queued state. Returns `false` if the queue was empty.
    pub fn step(&self) -> bool {
        self.source.step()
    }

    /// Emits every queued state and returns how many there were.
    pub fn play(&self) -> usize {
        self.source.play()
    }

    /// How many queued states have not been emitted yet.
    pub fn pending(&self) -> usize {
        self.source.pending()
    }

    /// Emits the current state again without recording it: the platform's initial report.
    pub fn emit_current(&self) {
        self.source.emit_current();
    }

    /// The state the app is in now.
    pub fn current(&self) -> AppState {
        self.source.current()
    }

    /// Every state emitted so far, oldest first.
    pub fn history(&self) -> Vec<AppState> {
        self.source.history()
    }
}

impl Default for ScriptedLifecycle {
    fn default() -> ScriptedLifecycle {
        ScriptedLifecycle::new()
    }
}

impl core::fmt::Debug for ScriptedLifecycle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ScriptedLifecycle")
            .field("current", &self.current())
            .field("pending", &self.pending())
            .finish_non_exhaustive()
    }
}

impl Lifecycle for ScriptedLifecycle {
    fn changed(&self, state: AppState) {
        self.set(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use undra_runtime::testing::TestRuntime;

    fn record_connectivity(t: &TestRuntime) -> Arc<Mutex<Vec<(bool, NetKind)>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        crate::on_connectivity_changed(&t.ctx(), move |_ctx, online, kind| {
            sink.lock().push((online, kind))
        })
        .detach();
        seen
    }

    #[test]
    fn starts_online_on_wifi_and_silent() {
        let net = ScriptedConnectivity::new();
        assert_eq!(net.current(), (true, NetKind::Wifi));
        assert!(net.history().is_empty());
        assert_eq!(net.pending(), 0);
        assert!(!net.step());
        assert_eq!(net.play(), 0);
        let app = ScriptedLifecycle::new();
        assert_eq!(app.current(), AppState::Active);
        assert!(app.history().is_empty());
        assert!(!app.step());
        assert_eq!(app.play(), 0);
    }

    #[test]
    fn set_updates_current_history_and_delivers() {
        let t = TestRuntime::new();
        let seen = record_connectivity(&t);
        let net = ScriptedConnectivity::new();
        net.attach(t.runtime());
        net.go_offline();
        net.go_online(NetKind::Wired);
        assert_eq!(net.current(), (true, NetKind::Wired));
        assert_eq!(
            net.history(),
            [(false, NetKind::None), (true, NetKind::Wired)]
        );
        assert_eq!(*seen.lock(), net.history());
    }

    #[test]
    fn the_port_trait_method_is_the_same_as_set() {
        let t = TestRuntime::new();
        let seen = record_connectivity(&t);
        let net = ScriptedConnectivity::new();
        net.attach(t.runtime());
        Connectivity::changed(&net, false, NetKind::Unknown);
        assert_eq!(*seen.lock(), [(false, NetKind::Unknown)]);
    }

    #[test]
    fn scripts_step_and_play_in_order() {
        let t = TestRuntime::new();
        let seen = record_connectivity(&t);
        let net = ScriptedConnectivity::new();
        net.attach(t.runtime());
        net.script([
            (false, NetKind::None),
            (true, NetKind::Cellular),
            (true, NetKind::Wifi),
        ]);
        assert_eq!(net.pending(), 3);
        assert!(net.step());
        assert_eq!(*seen.lock(), [(false, NetKind::None)]);
        assert_eq!(net.pending(), 2);
        assert_eq!(net.play(), 2);
        assert_eq!(seen.lock().len(), 3);
        assert_eq!(net.current(), (true, NetKind::Wifi));
    }

    #[test]
    fn emit_current_does_not_touch_the_history() {
        let t = TestRuntime::new();
        let seen = record_connectivity(&t);
        let net = ScriptedConnectivity::new();
        net.attach(t.runtime());
        net.emit_current();
        assert_eq!(*seen.lock(), [(true, NetKind::Wifi)]);
        assert!(net.history().is_empty());
    }

    #[test]
    fn nothing_is_delivered_before_attach_and_dropped_runtimes_are_forgotten() {
        let net = ScriptedConnectivity::new();
        net.set(false, NetKind::None); // no runtime: recorded, delivered nowhere
        assert_eq!(net.history().len(), 1);
        let t1 = TestRuntime::new();
        let seen1 = record_connectivity(&t1);
        net.attach(t1.runtime());
        {
            let t2 = TestRuntime::new();
            net.attach(t2.runtime());
        } // t2 is dropped here
        let t3 = TestRuntime::new();
        let seen3 = record_connectivity(&t3);
        net.attach(t3.runtime());
        net.set(true, NetKind::Wifi);
        assert_eq!(*seen1.lock(), [(true, NetKind::Wifi)]);
        assert_eq!(*seen3.lock(), [(true, NetKind::Wifi)]);
    }

    #[test]
    fn lifecycle_delivers_states() {
        let t = TestRuntime::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        crate::on_lifecycle_changed(&t.ctx(), move |_ctx, state| sink.lock().push(state)).detach();
        let app = ScriptedLifecycle::new();
        app.attach(t.runtime());
        app.script([AppState::Inactive, AppState::Background]);
        app.set(AppState::Active);
        assert_eq!(app.play(), 2);
        Lifecycle::changed(&app, AppState::Active);
        app.emit_current();
        assert_eq!(
            *seen.lock(),
            [
                AppState::Active,
                AppState::Inactive,
                AppState::Background,
                AppState::Active,
                AppState::Active
            ]
        );
        assert_eq!(app.current(), AppState::Active);
        assert_eq!(app.history().len(), 4);
    }

    #[test]
    fn debug_shows_current_and_pending() {
        let net = ScriptedConnectivity::new();
        net.script([(true, NetKind::Wired)]);
        let text = format!("{net:?}");
        assert!(
            text.contains("pending: 1") && text.contains("Wifi"),
            "{text}"
        );
        assert!(format!("{:?}", ScriptedLifecycle::new()).contains("Active"));
    }
}
