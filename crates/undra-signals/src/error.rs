//! Errors returned when wiring signals into a store, and when a write is refused.

use core::fmt;

/// Why a signal could not be attached to (or configured on) a [`StoreCell`](crate::StoreCell).
///
/// These are programming errors in the code that builds a store (normally macro-generated), so
/// they are typed values rather than panics: the runtime can turn them into a load-time
/// diagnostic instead of taking the process down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignalsError {
    /// The signal or computed is already attached to a store (this one or another). A signal
    /// belongs to at most one store slot for its whole life.
    AlreadyAttached,
    /// Signals must be attached in declaration order, so the next free `signal_id` is
    /// `expected` and `got` was passed instead.
    OutOfOrder {
        /// The signal id the next attach must use (the number of signals attached so far).
        expected: u32,
        /// The signal id that was passed.
        got: u32,
    },
    /// The store has no signal with this id.
    UnknownSignal {
        /// The id that was looked up.
        signal_id: u32,
    },
}

impl fmt::Display for SignalsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            SignalsError::AlreadyAttached => {
                f.write_str("the signal is already attached to a store")
            }
            SignalsError::OutOfOrder { expected, got } => write!(
                f,
                "signals must be attached in declaration order: expected signal id {expected}, got {got}"
            ),
            SignalsError::UnknownSignal { signal_id } => {
                write!(f, "the store has no signal with id {signal_id}")
            }
        }
    }
}

impl std::error::Error for SignalsError {}

/// Why a signal write was refused (ADR-035).
///
/// Returned by [`Signal::try_set`](crate::Signal::try_set) and
/// [`Signal::try_update`](crate::Signal::try_update); [`Signal::set`](crate::Signal::set) and the
/// other writers panic with the same teaching message (E0065) instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WriteError {
    /// The signal belongs to a store owned by runtime `owner` (`0`: to no published store) and
    /// the calling thread does not hold that runtime's core lock: a blocking-pool worker, a host or
    /// embedder thread, a thread inside no runtime, or the core of another runtime.
    OffCore {
        /// The id of the runtime that owns the store, or `0`.
        owner: u64,
    },
}

/// The E0065 teaching message (constitution R8: code, what, why, fix, docs link).
pub(crate) fn off_core_message(owner: u64) -> String {
    let what = if owner == 0 {
        "a signal with dependents was written from a thread that does not hold a runtime's core lock".to_owned()
    } else {
        format!(
            "a signal of a store owned by runtime {owner} was written from a thread that does not hold its core lock"
        )
    };
    format!(
        "error[undra::E0065]: {what}\n  \
         = note: signal writes belong on the core: the runtime delivers each change-set to its host in commit order under its core lock, so a write from any other thread (a blocking-pool worker, a host or embedder thread, another runtime's core) would be delivered unordered, to the wrong host, or not at all\n  \
         = help: return the value to the task or call that awaits it (`ctx.spawn`, a dispatched call, the result of `spawn_blocking`), or wrap the write in `ctx.with_core(|| ..)` on a host thread; `Signal::try_set` reports this as a `WriteError` instead of panicking\n  \
         = docs: https://shreypdev.github.io/undra/docs/errors.html#E0065"
    )
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            WriteError::OffCore { owner } => f.write_str(&off_core_message(owner)),
        }
    }
}

impl std::error::Error for WriteError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_descriptive() {
        assert_eq!(
            SignalsError::AlreadyAttached.to_string(),
            "the signal is already attached to a store"
        );
        assert_eq!(
            SignalsError::OutOfOrder {
                expected: 2,
                got: 5
            }
            .to_string(),
            "signals must be attached in declaration order: expected signal id 2, got 5"
        );
        assert_eq!(
            SignalsError::UnknownSignal { signal_id: 9 }.to_string(),
            "the store has no signal with id 9"
        );
    }

    #[test]
    fn is_a_std_error() {
        fn assert_error<E: std::error::Error + Send + Sync + 'static>() {}
        assert_error::<SignalsError>();
        assert_error::<WriteError>();
    }

    #[test]
    fn the_write_error_teaches_with_code_why_fix_and_docs() {
        let text = WriteError::OffCore { owner: 7 }.to_string();
        assert!(text.starts_with("error[undra::E0065]: a signal of a store owned by runtime 7"));
        assert!(
            text.contains("= note: signal writes belong on the core"),
            "{text}"
        );
        assert!(
            text.contains("= help: ") && text.contains("ctx.with_core"),
            "{text}"
        );
        assert!(
            text.ends_with("= docs: https://shreypdev.github.io/undra/docs/errors.html#E0065"),
            "{text}"
        );
        assert!(
            WriteError::OffCore { owner: 0 }
                .to_string()
                .contains("a runtime's core lock")
        );
    }
}
