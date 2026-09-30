//! The panic guard (SPEC 5.6, constitution R6).
//!
//! Every place where the runtime runs code it does not own (a dispatcher, a task poll, an
//! event subscriber, a store's `restore`) goes through [`guarded`], which turns an unwinding
//! panic into a [`PanicReport`] carrying the message and a backtrace.
//!
//! A panic's backtrace only exists at the moment of the panic, so the first guard installs
//! (once per process) a chained panic hook. While a guard is active on the panicking thread
//! the hook records the report in a thread-local instead of printing it; otherwise it defers
//! to the previously installed hook, so panics outside the runtime behave as before.
//!
//! On wasm (`panic = "abort"`) nothing can catch a panic; the hook instead logs it at level 5
//! through the host before the trap, as SPEC 7 requires.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::panic::{self, AssertUnwindSafe, PanicHookInfo};
use std::sync::Once;

/// A caught panic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PanicReport {
    /// The panic message.
    pub message: String,
    /// The panic location followed by the backtrace, or `"unavailable"` on wasm.
    pub backtrace: String,
}

/// The payload of a panic that crossed a thread boundary (`spawn_blocking`) and is re-raised
/// with `resume_unwind`; keeps the original report intact.
pub(crate) struct CarriedPanic(pub PanicReport);

/// The per-thread state of the guard: how many guards are active and the last panic recorded
/// while one was. One thread-local holds both, so entering and leaving a guard (the hot path of
/// every dispatched call) each cost one thread-local access.
struct GuardState {
    depth: Cell<u32>,
    last: RefCell<Option<PanicReport>>,
}

thread_local! {
    static STATE: GuardState = const {
        GuardState {
            depth: Cell::new(0),
            last: RefCell::new(None),
        }
    };
}

fn payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(carried) = payload.downcast_ref::<CarriedPanic>() {
        carried.0.message.clone()
    } else {
        "panic with a non-string payload".to_owned()
    }
}

#[cfg(not(target_family = "wasm"))]
fn capture_backtrace() -> String {
    std::backtrace::Backtrace::force_capture().to_string()
}

#[cfg(target_family = "wasm")]
fn capture_backtrace() -> String {
    "unavailable".to_owned()
}

fn hook(previous: &(dyn Fn(&PanicHookInfo<'_>) + Send + Sync), info: &PanicHookInfo<'_>) {
    let guarded = STATE.try_with(|state| state.depth.get()).unwrap_or(0) > 0;
    if cfg!(target_family = "wasm") {
        let message = payload_message(info.payload());
        crate::runtime::log_fatal_current("undra::panic", &message);
        previous(info);
        return;
    }
    if !guarded {
        previous(info);
        return;
    }
    let message = payload_message(info.payload());
    let location = info
        .location()
        .map(|l| format!("panicked at {}:{}:{}\n", l.file(), l.line(), l.column()))
        .unwrap_or_default();
    let report = PanicReport {
        message,
        backtrace: format!("{location}{}", capture_backtrace()),
    };
    let _ = STATE.try_with(|state| *state.last.borrow_mut() = Some(report));
}

/// Installs the chained panic hook (once per process).
pub(crate) fn install_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| hook(&*previous, info)));
    });
}

/// Builds the report for a caught panic payload.
fn report_from(payload: Box<dyn Any + Send>) -> PanicReport {
    if let Some(carried) = payload.downcast_ref::<CarriedPanic>() {
        return carried.0.clone();
    }
    let message = payload_message(&*payload);
    let recorded = STATE
        .try_with(|state| state.last.borrow_mut().take())
        .ok()
        .flatten();
    // The hook's recording is used only if its message matches the payload's, so a stale
    // recording (a panic that user code caught itself) can never be attributed to a later
    // panic that bypassed the hook (`resume_unwind`).
    match recorded {
        Some(report) if report.message == message => report,
        _ => PanicReport {
            message,
            backtrace: capture_backtrace(),
        },
    }
}

/// Runs `f`, converting a panic into a [`PanicReport`].
pub(crate) fn guarded<R>(f: impl FnOnce() -> R) -> Result<R, PanicReport> {
    install_hook();
    STATE.with(|state| {
        if state.depth.get() == 0 {
            // A recording left over from a panic that user code caught itself must not be
            // attributed to this guard's panic.
            *state.last.borrow_mut() = None;
        }
        state.depth.set(state.depth.get() + 1);
    });
    let result = panic::catch_unwind(AssertUnwindSafe(f));
    STATE.with(|state| state.depth.set(state.depth.get().saturating_sub(1)));
    result.map_err(report_from)
}

/// Drops `value` under the guard, so a panicking `Drop` cannot unwind into the runtime.
pub(crate) fn drop_guarded<T>(value: T) -> Result<(), PanicReport> {
    guarded(move || drop(value))
}

/// Encodes a report as the body of a status 2 reply: `String message, String backtrace`.
pub(crate) fn encode_panic_body(report: &PanicReport) -> Vec<u8> {
    let mut w = undra_wire::Writer::with_capacity(8 + report.message.len() + report.backtrace.len());
    w.write_str(&report.message);
    w.write_str(&report.backtrace);
    w.into_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_passes_through() {
        assert_eq!(guarded(|| 5), Ok(5));
    }

    #[test]
    fn str_and_string_panics_are_reported() {
        let report = guarded(|| -> () { panic!("static message") }).unwrap_err();
        assert_eq!(report.message, "static message");
        assert!(
            report.backtrace.contains("panicked at"),
            "{}",
            report.backtrace
        );
        let n = 7;
        let report = guarded(|| -> () { panic!("formatted {n}") }).unwrap_err();
        assert_eq!(report.message, "formatted 7");
    }

    #[test]
    fn non_string_payload_is_described() {
        let report = guarded(|| -> () { panic::panic_any(42_u8) }).unwrap_err();
        assert_eq!(report.message, "panic with a non-string payload");
    }

    #[test]
    fn nested_guards_report_the_innermost_panic_once() {
        let outer = guarded(|| {
            let inner = guarded(|| -> () { panic!("inner") }).unwrap_err();
            assert_eq!(inner.message, "inner");
            "survived"
        });
        assert_eq!(outer, Ok("survived"));
        // The stale report of the inner panic does not leak into a later, unrelated one.
        let later = guarded(|| -> () { panic!("later") }).unwrap_err();
        assert_eq!(later.message, "later");
    }

    #[test]
    fn carried_panics_keep_their_original_report() {
        let original = PanicReport {
            message: "from another thread".into(),
            backtrace: "frames".into(),
        };
        let carried = original.clone();
        let report =
            guarded(move || -> () { panic::resume_unwind(Box::new(CarriedPanic(carried))) })
                .unwrap_err();
        assert_eq!(report, original);
    }

    #[test]
    fn panicking_drop_is_contained() {
        struct Bomb;
        impl Drop for Bomb {
            fn drop(&mut self) {
                panic!("drop bomb");
            }
        }
        assert_eq!(drop_guarded(Bomb).unwrap_err().message, "drop bomb");
    }

    #[test]
    fn panic_body_encodes_two_strings() {
        let body = encode_panic_body(&PanicReport {
            message: "m".into(),
            backtrace: "bt".into(),
        });
        assert_eq!(body, [1, 0, 0, 0, b'm', 2, 0, 0, 0, b'b', b't']);
    }
}
