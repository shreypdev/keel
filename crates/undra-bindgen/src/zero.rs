//! The search for a store signal's placeholder, the value a signal holds until the core's first
//! change-set arrives, shared by the three generators.
//!
//! A placeholder is built from the first variant of an enum and from every field of a record. A
//! recursive type needs more care: `enum Sum { Add(Box<Sum>, Box<Sum>), Zero }` has a finite value
//! only through `Zero`, wherever the schema lists it. The search therefore keeps the types it is
//! building on a path, refuses to build a type that is already on it, and lets an enum fall through
//! to its next variant when one needs a type on the path. An optional, an array and a map are empty
//! (`nil`, `[]`, `[:]` and their spellings), so recursion through them ends at once. For a schema
//! without recursion the first variant always succeeds, so the output is what a plain walk gives.

use std::collections::HashMap;

/// How many named types one search builds before it gives up on a type: a bound on the work a
/// schema of types that have no finite value can cause, far above what a real schema needs.
const ZERO_STEPS: usize = 10_000;

/// The state of one search for a placeholder value.
pub(crate) struct ZeroState {
    /// The types being built, outermost first.
    path: Vec<String>,
    /// The placeholder of every type built so far.
    built: HashMap<String, String>,
    /// How many more named types this search may build.
    steps: usize,
}

impl ZeroState {
    /// A search that has built nothing yet.
    pub(crate) fn new() -> ZeroState {
        ZeroState {
            path: Vec::new(),
            built: HashMap::new(),
            steps: ZERO_STEPS,
        }
    }

    /// The placeholder of the named type `name`, built by `build` (which recurses through this
    /// state), or `None` when every way to build it needs a type that is already being built
    /// further up (or the budget is spent). A placeholder that was built is remembered for the
    /// rest of the search: it is a finite value wherever it is used.
    pub(crate) fn named(
        &mut self,
        name: &str,
        build: impl FnOnce(&mut ZeroState) -> Option<String>,
    ) -> Option<String> {
        if let Some(built) = self.built.get(name) {
            return Some(built.clone());
        }
        if self.path.iter().any(|p| p == name) || self.steps == 0 {
            return None;
        }
        self.steps -= 1;
        self.path.push(name.to_owned());
        let built = build(self);
        self.path.pop();
        if let Some(built) = &built {
            self.built.insert(name.to_owned(), built.clone());
        }
        built
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_type_on_the_path_is_refused_and_a_success_is_remembered() {
        let mut state = ZeroState::new();
        let inner = state.named("A", |state| {
            // `A` is being built: asking for it again is refused, not a loop.
            assert_eq!(state.named("A", |_| Some("again".to_owned())), None);
            Some("A()".to_owned())
        });
        assert_eq!(inner.as_deref(), Some("A()"));
        // Remembered: the builder is not run again.
        assert_eq!(
            state.named("A", |_| panic!("built twice")).as_deref(),
            Some("A()")
        );
    }

    #[test]
    fn the_budget_bounds_the_search() {
        let mut state = ZeroState::new();
        state.steps = 1;
        assert_eq!(
            state.named("A", |state| state.named("B", |_| Some("B".to_owned()))),
            None
        );
    }
}
