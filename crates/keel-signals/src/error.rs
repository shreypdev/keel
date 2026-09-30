//! Errors returned when wiring signals into a store.

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
    }
}
