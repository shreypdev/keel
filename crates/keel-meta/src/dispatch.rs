//! Runtime-agnostic dispatch types (SPEC §5.6).
//!
//! `keel-meta` sits below `keel-runtime` in the dependency graph, so it cannot
//! name the runtime's `Runtime`, `Handle` or `DispatchResult` types. The
//! dispatch function pointer stored in [`ObjectMeta`](crate::ObjectMeta) and
//! [`FunctionMeta`](crate::FunctionMeta) is therefore expressed in terms of
//! two erased types:
//!
//! * the runtime is passed as `&dyn Any`; the macro-generated dispatcher
//!   downcasts it to `keel_runtime::Runtime` (and answers
//!   [`DispatchOutcome`] "unknown" if the downcast fails);
//! * the result is a [`DispatchOutcome`], an opaque `Box<dyn Any + Send>`.
//!   The generated dispatcher boxes a `keel_runtime::DispatchResult` into it
//!   and `keel-runtime` downcasts it back with [`DispatchOutcome::downcast`].
//!
//! The contract between the two sides is owned by `keel-runtime`; this crate
//! only transports the values.

use core::any::Any;
use core::fmt;

/// The signature of a macro-generated dispatcher.
///
/// The first argument is the runtime, erased as `&dyn Any` (see the module
/// docs). The returned [`DispatchOutcome`] wraps the runtime's own result type.
pub type DispatchFn = fn(&dyn Any, DispatchCall<'_>) -> DispatchOutcome;

/// One call routed to a dispatcher (SPEC §5.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DispatchCall<'a> {
    /// The method (or constructor, or function) being invoked.
    pub method_id: u32,
    /// The caller-chosen call id (never `0`).
    pub call_id: u32,
    /// The raw `u64` handle of the receiver; `0` for constructors and free
    /// functions.
    pub handle: u64,
    /// The encoded arguments, in parameter declaration order.
    pub args: &'a [u8],
}

/// An opaque dispatch result.
///
/// It wraps a `Box<dyn Any + Send>` so that `keel-meta` needs no runtime types.
/// `keel-runtime` builds one from its `DispatchResult` in the generated
/// dispatcher and downcasts it back on the other side:
///
/// ```
/// use keel_meta::DispatchOutcome;
///
/// #[derive(Debug, PartialEq)]
/// struct RuntimeResult(Vec<u8>);
///
/// let outcome = DispatchOutcome::new(RuntimeResult(vec![1, 2, 3]));
/// assert!(outcome.is::<RuntimeResult>());
/// assert_eq!(
///     outcome.downcast::<RuntimeResult>().unwrap(),
///     RuntimeResult(vec![1, 2, 3])
/// );
/// ```
pub struct DispatchOutcome(pub Box<dyn Any + Send>);

impl DispatchOutcome {
    /// Wraps a runtime result.
    #[must_use]
    pub fn new<T: Any + Send>(value: T) -> DispatchOutcome {
        DispatchOutcome(Box::new(value))
    }

    /// Whether the wrapped value is a `T`.
    #[must_use]
    pub fn is<T: Any>(&self) -> bool {
        self.0.is::<T>()
    }

    /// Borrows the wrapped value as a `T`, if it is one.
    #[must_use]
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref::<T>()
    }

    /// Unwraps the value as a `T`, or gives the outcome back unchanged if it
    /// holds something else.
    pub fn downcast<T: Any + Send>(self) -> Result<T, DispatchOutcome> {
        match self.0.downcast::<T>() {
            Ok(value) => Ok(*value),
            Err(other) => Err(DispatchOutcome(other)),
        }
    }
}

impl fmt::Debug for DispatchOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DispatchOutcome(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downcast_round_trip() {
        let outcome = DispatchOutcome::new(42_u32);
        assert!(outcome.is::<u32>());
        assert!(!outcome.is::<u64>());
        assert_eq!(outcome.downcast_ref::<u32>(), Some(&42));
        assert_eq!(outcome.downcast::<u32>().unwrap(), 42);
    }

    #[test]
    fn wrong_downcast_returns_the_outcome() {
        let outcome = DispatchOutcome::new(String::from("kept"));
        let outcome = outcome.downcast::<u32>().unwrap_err();
        assert_eq!(outcome.downcast::<String>().unwrap(), "kept");
    }

    #[test]
    fn debug_does_not_require_debug_payload() {
        struct Opaque;
        assert_eq!(
            format!("{:?}", DispatchOutcome::new(Opaque)),
            "DispatchOutcome(..)"
        );
    }

    #[test]
    fn dispatch_fn_is_a_plain_fn_pointer() {
        fn echo(rt: &dyn Any, call: DispatchCall<'_>) -> DispatchOutcome {
            let base = rt.downcast_ref::<u32>().copied().unwrap_or(0);
            DispatchOutcome::new(base + call.method_id + call.args.len() as u32)
        }
        let f: DispatchFn = echo;
        let out = f(
            &10_u32,
            DispatchCall {
                method_id: 5,
                call_id: 1,
                handle: 0,
                args: &[1, 2, 3],
            },
        );
        assert_eq!(out.downcast::<u32>().unwrap(), 18);
    }
}
