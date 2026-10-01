//! The pipeline of a derived list: its stages fused into one per-row function, and the parameters
//! the stages read.
//!
//! Every chain of `filter`, `filter_with`, `map`, `sort_by_key` and `sort_by_key_with` normalises
//! to "one composed per-row function `&T -> Option<(K, U)>`, then at most one stable sort by `K`"
//! (ADR-039 section 1): a filter after a sort is the same as one before it, and a map after a sort
//! does not move a row. The builder composes the stages in the order they were written into one
//! [`EvalFn`].
//!
//! The function takes the row as a `Cow`: a drain replaying a recorded op owns the item the op
//! carries and hands it over (`Cow::Owned`), so a pipeline without a `map` emits that very item
//! instead of cloning it a second time; a walk over the list borrows (`Cow::Borrowed`) and clones
//! only the rows it emits.

use std::any::Any;
use std::borrow::Cow;
use std::sync::{Arc, Weak};

use undra_wire::{Encode, Writer};

use crate::deps::OwnedDep;
use crate::graph::Reactive;
use crate::value::SignalValue;

/// The parameter values one drain evaluates with: one snapshot per parameter, in the order the
/// stages declared them.
#[derive(Clone, Default)]
pub(crate) struct Params(pub(crate) Vec<Arc<dyn Any + Send + Sync>>);

impl Params {
    /// The value of parameter `slot`, which the builder declared with type `P`.
    pub(crate) fn get<P: 'static>(&self, slot: usize) -> &P {
        self.0[slot]
            .downcast_ref::<P>()
            .expect("undra-signals: a derived list's parameter has the type its stage declared")
    }
}

/// The fused pipeline: a row in, its sort key (`()` without a sort stage) and its output out, or
/// `None` when a filter drops it.
pub(crate) type EvalFn<T, U, K> =
    dyn for<'a> Fn(&Params, Cow<'a, T>) -> Option<(K, Cow<'a, U>)> + Send + Sync;

/// Boxes a stage, fixing its higher-ranked signature.
pub(crate) fn eval_fn<T, U, K, F>(f: F) -> Box<EvalFn<T, U, K>>
where
    T: Clone,
    U: Clone,
    F: for<'a> Fn(&Params, Cow<'a, T>) -> Option<(K, Cow<'a, U>)> + Send + Sync + 'static,
{
    Box::new(f)
}

/// The identity pipeline: every row passes unchanged, no sort key.
pub(crate) fn identity<T: Clone + 'static>() -> Box<EvalFn<T, T, ()>> {
    eval_fn(|_, row: Cow<'_, T>| Some(((), row)))
}

/// One parameter of a pipeline, type-erased: a `Signal` or `Computed` (or derived list) the list
/// subscribes to and snapshots at every drain.
pub(crate) trait ParamSlot: Send + Sync {
    /// The parameter's current value.
    fn snapshot(&self) -> Arc<dyn Any + Send + Sync>;
    /// Whether two snapshots are the same value: the same allocation, or equal encoded bytes.
    fn same(&self, a: &Arc<dyn Any + Send + Sync>, b: &Arc<dyn Any + Send + Sync>) -> bool;
    /// Makes `dependent` invalidated when the parameter changes.
    fn subscribe(&self, dependent: Weak<dyn Reactive>);
}

/// A parameter backed by an owned dependency handle.
pub(crate) struct Param<O>(pub(crate) O);

impl<O> ParamSlot for Param<O>
where
    O: OwnedDep,
    O::Value: SignalValue,
{
    fn snapshot(&self) -> Arc<dyn Any + Send + Sync> {
        self.0.snapshot()
    }

    fn same(&self, a: &Arc<dyn Any + Send + Sync>, b: &Arc<dyn Any + Send + Sync>) -> bool {
        if std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b)) {
            return true;
        }
        let (Some(a), Some(b)) = (a.downcast_ref::<O::Value>(), b.downcast_ref::<O::Value>())
        else {
            return false;
        };
        let (mut left, mut right) = (Writer::new(), Writer::new());
        a.encode(&mut left);
        b.encode(&mut right);
        left.as_slice() == right.as_slice()
    }

    fn subscribe(&self, dependent: Weak<dyn Reactive>) {
        self.0.subscribe(dependent);
    }
}
