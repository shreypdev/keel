//! The panic guard every boundary entry runs under (constitution R6).
//!
//! `keel-runtime` already contains panics from user code (dispatchers, tasks, port
//! callbacks) and turns them into status 2 replies. This guard is the second line: a bug in
//! this crate, or in anything a shim calls, must not unwind across an `extern "C"` /
//! JNI frame either. On `wasm32` panics abort by construction (SPEC 7: the panic hook logs at
//! level 5 and the module traps), so the guard is a plain call there.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

use keel_runtime::Runtime;
use keel_runtime::log::FATAL;

/// The log target of records produced by this crate.
pub(crate) const TARGET: &str = "keel::ffi";

/// The text of a panic payload.
pub(crate) fn describe(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic with a non-string payload".to_owned()
    }
}

/// Runs `f` for the boundary entry `entry`. If it panics the panic is contained, logged at level
/// 5 through the runtime (when one is running) and `on_panic(message)` is the result.
pub(crate) fn guarded<R>(
    entry: &'static str,
    on_panic: impl FnOnce(&str) -> R,
    f: impl FnOnce() -> R,
) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => {
            let message = describe(&*payload);
            drop(payload);
            report(entry, &message);
            on_panic(&message)
        }
    }
}

/// Logs a contained panic; never panics itself (a failing log must not undo the containment).
fn report(entry: &str, message: &str) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(rt) = Runtime::global() {
            rt.log(FATAL, TARGET, &format!("{entry} panicked: {message}"));
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_passes_through() {
        assert_eq!(guarded("t", |_| 0, || 7), 7);
    }

    #[test]
    fn panics_become_the_fallback_with_the_message() {
        let got = guarded("t", |m| m.to_owned(), || -> String { panic!("boom {}", 1) });
        assert_eq!(got, "boom 1");
        let got = guarded("t", |m| m.to_owned(), || -> String { panic!("plain") });
        assert_eq!(got, "plain");
        let got = guarded(
            "t",
            |m| m.to_owned(),
            || -> String { std::panic::panic_any(3_u8) },
        );
        assert_eq!(got, "panic with a non-string payload");
    }
}
