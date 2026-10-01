//! [`Deps`]: the set of signals and computeds a [`Computed`] or [`Effect`](crate::Effect) reads.
//!
//! A dependency set is written as a reference to one signal or computed, or a tuple of up to six
//! of them:
//!
//! ```
//! use undra_signals::{Computed, Signal};
//!
//! let a = Signal::new(1);
//! let b = Signal::new(String::from("x"));
//! let double = Computed::new(&a, |a| a * 2);
//! let label = Computed::new((&a, &b, &double), |(a, b, d)| format!("{b}{a}/{d}"));
//! assert_eq!(label.get(), "x1/2");
//! ```
//!
//! The closure receives *references* to the current values (a tuple of them for a tuple), so
//! reading a large list costs no clone. Each reference points at an immutable snapshot that is
//! valid inside the closure; nothing is locked while the closure runs, so it may write signals,
//! including the ones it reads.

use std::sync::Weak;

use crate::computed::Computed;
use crate::graph::Reactive;
use crate::signal::Signal;
use crate::value::SignalValue;

/// One reference to a [`Signal`] or [`Computed`] in a dependency set.
///
/// Implemented for `&Signal<T>` and `&Computed<T>`; there is nothing to implement by hand.
pub trait Dep {
    /// The value type of the source.
    type Value: SignalValue;
    #[doc(hidden)]
    type Owned: OwnedDep<Value = Self::Value>;
    #[doc(hidden)]
    fn into_owned(self) -> Self::Owned;
}

/// An owning handle to one source (a cloned `Signal` or `Computed`).
#[doc(hidden)]
pub trait OwnedDep: Send + Sync + 'static {
    type Value: 'static;
    fn subscribe(&self, dependent: Weak<dyn Reactive>);
    fn with_value<R>(&self, f: impl FnOnce(&Self::Value) -> R) -> R;
}

impl<A: SignalValue> Dep for &Signal<A> {
    type Value = A;
    type Owned = Signal<A>;
    fn into_owned(self) -> Signal<A> {
        self.clone()
    }
}

impl<A: SignalValue> OwnedDep for Signal<A> {
    type Value = A;
    fn subscribe(&self, dependent: Weak<dyn Reactive>) {
        self.add_dependent(dependent);
    }
    fn with_value<R>(&self, f: impl FnOnce(&A) -> R) -> R {
        self.with(f)
    }
}

impl<A: SignalValue> Dep for &Computed<A> {
    type Value = A;
    type Owned = Computed<A>;
    fn into_owned(self) -> Computed<A> {
        self.clone()
    }
}

impl<A: SignalValue> OwnedDep for Computed<A> {
    type Value = A;
    fn subscribe(&self, dependent: Weak<dyn Reactive>) {
        self.add_dependent(dependent);
    }
    fn with_value<R>(&self, f: impl FnOnce(&A) -> R) -> R {
        self.with(f)
    }
}

/// Something that reads its dependencies and produces a `T`: the recompute step of a computed
/// or the body of an effect (`T = ()`), bundled with the owning handles it reads.
#[doc(hidden)]
pub trait Compute<T>: Send + Sync {
    /// Reads every dependency and runs the user closure.
    fn run(&self) -> T;
    /// Registers `dependent` to be invalidated when any dependency changes.
    fn subscribe(&self, dependent: Weak<dyn Reactive>);
}

/// A set of dependencies: one `&Signal<A>` or `&Computed<A>`, or a tuple of up to six of them.
///
/// `Values` is what the closure of [`Computed::new`] and [`Effect::new`](crate::Effect::new)
/// receives: a reference for a single dependency, a tuple of references for a tuple.
pub trait Deps {
    /// References to the current values of every dependency.
    type Values<'a>;

    /// Bundles owning handles to the dependencies with the closure that reads them.
    #[doc(hidden)]
    fn into_compute<T, F>(self, f: F) -> Box<dyn Compute<T>>
    where
        T: 'static,
        F: for<'a> Fn(Self::Values<'a>) -> T + Send + Sync + 'static;
}

struct Single<O, F> {
    dep: O,
    f: F,
}

impl<T, O, F> Compute<T> for Single<O, F>
where
    O: OwnedDep,
    F: Fn(&O::Value) -> T + Send + Sync,
{
    fn run(&self) -> T {
        self.dep.with_value(&self.f)
    }

    fn subscribe(&self, dependent: Weak<dyn Reactive>) {
        self.dep.subscribe(dependent);
    }
}

impl<E: Dep> Deps for E {
    type Values<'a> = &'a E::Value;

    fn into_compute<T, F>(self, f: F) -> Box<dyn Compute<T>>
    where
        T: 'static,
        F: for<'a> Fn(&'a E::Value) -> T + Send + Sync + 'static,
    {
        Box::new(Single {
            dep: self.into_owned(),
            f,
        })
    }
}

/// Reads `$dep.with_value(..)` for each listed owned dependency, innermost last, then calls
/// `$f` with the tuple of references. Nesting keeps every snapshot alive until `$f` returns.
macro_rules! nest_read {
    ($f:expr; ; $($vals:ident),*) => { ($f)(($($vals,)*)) };
    ($f:expr; $head:ident $(, $tail:ident)*; $($vals:ident),*) => {
        $head.with_value(|$head| nest_read!($f; $($tail),*; $($vals,)* $head))
    };
}

macro_rules! impl_deps_tuple {
    ($( $Tuple:ident => ($($E:ident $O:ident $idx:tt $v:ident),+) ),+ $(,)?) => {$(
        struct $Tuple<D, F> {
            deps: D,
            f: F,
        }

        impl<T, $($O,)+ F> Compute<T> for $Tuple<($($O,)+), F>
        where
            $($O: OwnedDep,)+
            F: for<'a> Fn(($(&'a $O::Value,)+)) -> T + Send + Sync,
        {
            fn run(&self) -> T {
                let ($($v,)+) = &self.deps;
                nest_read!(&self.f; $($v),+;)
            }

            fn subscribe(&self, dependent: Weak<dyn Reactive>) {
                $(self.deps.$idx.subscribe(dependent.clone());)+
            }
        }

        impl<$($E: Dep),+> Deps for ($($E,)+) {
            type Values<'a> = ($(&'a $E::Value,)+);

            fn into_compute<T, F>(self, f: F) -> Box<dyn Compute<T>>
            where
                T: 'static,
                F: for<'a> Fn(($(&'a $E::Value,)+)) -> T + Send + Sync + 'static,
            {
                Box::new($Tuple {
                    deps: ($(self.$idx.into_owned(),)+),
                    f,
                })
            }
        }
    )+};
}

impl_deps_tuple! {
    Tuple1 => (E0 O0 0 v0),
    Tuple2 => (E0 O0 0 v0, E1 O1 1 v1),
    Tuple3 => (E0 O0 0 v0, E1 O1 1 v1, E2 O2 2 v2),
    Tuple4 => (E0 O0 0 v0, E1 O1 1 v1, E2 O2 2 v2, E3 O3 3 v3),
    Tuple5 => (E0 O0 0 v0, E1 O1 1 v1, E2 O2 2 v2, E3 O3 3 v3, E4 O4 4 v4),
    Tuple6 => (E0 O0 0 v0, E1 O1 1 v1, E2 O2 2 v2, E3 O3 3 v3, E4 O4 4 v4, E5 O5 5 v5),
}
