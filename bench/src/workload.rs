//! What a benchmarked operation looks like to the harness.

/// One benchmarked operation, already set up.
///
/// [`run`](Bench::run) is the operation being timed. Operations that change the state they run
/// against (an insert into a list that must keep its size) also implement
/// [`reset`](Bench::reset); the harness calls it outside the timed region before every
/// iteration.
pub trait Bench {
    /// Runs the operation once. This is what is timed.
    fn run(&mut self);

    /// Restores the state `run` needs, outside the timed region. Only called when
    /// [`needs_reset`](Bench::needs_reset) is `true`.
    fn reset(&mut self) {}

    /// Whether the harness must call [`reset`](Bench::reset) before every `run`. Such an
    /// operation is timed one iteration at a time, which costs two clock reads per iteration.
    fn needs_reset(&self) -> bool {
        false
    }
}

/// An operation with no state to restore: any closure.
struct Plain<F>(F);

impl<F: FnMut()> Bench for Plain<F> {
    fn run(&mut self) {
        (self.0)();
    }
}

/// An operation whose state must be restored before each run.
struct Resetting<F, R> {
    run: F,
    reset: R,
}

impl<F: FnMut(), R: FnMut()> Bench for Resetting<F, R> {
    fn run(&mut self) {
        (self.run)();
    }

    fn reset(&mut self) {
        (self.reset)();
    }

    fn needs_reset(&self) -> bool {
        true
    }
}

/// Wraps a closure as a [`Bench`] with nothing to reset.
pub fn plain(run: impl FnMut() + 'static) -> Box<dyn Bench> {
    Box::new(Plain(run))
}

/// Wraps two closures as a [`Bench`]: `run` is timed, `reset` runs untimed before each `run`.
pub fn with_reset(run: impl FnMut() + 'static, reset: impl FnMut() + 'static) -> Box<dyn Bench> {
    Box::new(Resetting { run, reset })
}

/// A named operation that can be built on demand.
///
/// Building is lazy and repeatable (`build` returns a fresh, independent operation) so a harness
/// holds one runtime at a time, and a failed measurement can start again from scratch.
pub struct Workload {
    /// Stable name, `group/operation[/variant]`, for example `wire/u32/roundtrip`. It is the
    /// criterion benchmark id and the key in `budgets.toml`.
    pub name: String,
    build: Box<dyn Fn() -> Box<dyn Bench>>,
}

impl Workload {
    /// Declares a workload; `build` does the setup and returns the operation to time.
    pub fn new(name: impl Into<String>, build: impl Fn() -> Box<dyn Bench> + 'static) -> Workload {
        Workload {
            name: name.into(),
            build: Box::new(build),
        }
    }

    /// Builds a fresh, set-up instance of the operation.
    pub fn build(&self) -> Box<dyn Bench> {
        (self.build)()
    }

    /// The part of the name before the first `/`: `wire`, `dispatch`, `signals`, `snapshot`.
    pub fn group(&self) -> &str {
        self.name.split('/').next().unwrap_or(&self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_is_the_first_path_segment() {
        let w = Workload::new("wire/u32/roundtrip", || plain(|| {}));
        assert_eq!(w.group(), "wire");
        let w = Workload::new("solo", || plain(|| {}));
        assert_eq!(w.group(), "solo");
    }

    #[test]
    fn plain_has_nothing_to_reset_and_resetting_does() {
        use std::cell::Cell;
        use std::rc::Rc;
        let runs = Rc::new(Cell::new(0));
        let resets = Rc::new(Cell::new(0));
        let mut a = plain({
            let runs = runs.clone();
            move || runs.set(runs.get() + 1)
        });
        assert!(!a.needs_reset());
        a.run();
        assert_eq!(runs.get(), 1);
        let mut b = with_reset(
            {
                let runs = runs.clone();
                move || runs.set(runs.get() + 1)
            },
            {
                let resets = resets.clone();
                move || resets.set(resets.get() + 1)
            },
        );
        assert!(b.needs_reset());
        b.reset();
        b.run();
        assert_eq!((runs.get(), resets.get()), (2, 1));
    }

    #[test]
    fn build_yields_independent_instances() {
        use std::cell::Cell;
        use std::rc::Rc;
        let built = Rc::new(Cell::new(0));
        let w = Workload::new("x/y", {
            let built = built.clone();
            move || {
                built.set(built.get() + 1);
                plain(|| {})
            }
        });
        let _a = w.build();
        let _b = w.build();
        assert_eq!(built.get(), 2);
    }
}
