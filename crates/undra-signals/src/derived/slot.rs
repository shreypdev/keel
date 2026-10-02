//! The derived slot of a [`StoreCell`](crate::StoreCell) (ADR-039 section 5): a computed that
//! ships keyed patches. A lazy list (ADR-043) is a slot of the same kind: evaluated by the core,
//! isolated like a computed when its pipeline panics, announced instead of sent.

use std::sync::Arc;

use undra_wire::Writer;

use super::node::DerivedNode;
use crate::lazy::LazyCell;

/// What a derived slot produced for one commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Emitted {
    /// The bytes are a keyed patch (`ChangeOp::KeyedPatch`).
    Patch,
    /// The bytes are the full value (`ChangeOp::Full`).
    Full,
    /// The view did not change since the host's copy: no entry.
    Nothing,
    /// A lazy list changed: the bytes are a `LazyInvalidated` (`ChangeOp::LazyInvalidated`, ADR-043).
    Invalidated,
}

/// A derived list as a store slot sees it.
pub(crate) trait DerivedSlot: Send + Sync {
    /// Writes what the view did since the last call: a patch of the pending derived ops, the full
    /// value when they overflowed, the view was rebuilt or the host has no copy, or nothing.
    /// `retain == false` (delivered only because it is `no_coalesce`, unobserved): always the full
    /// value, and no ops are kept.
    fn commit(&self, w: &mut Writer, retain: bool) -> Emitted;
    /// Writes the full value and keeps the derived ops from this instant (`observe(on)`).
    fn resync(&self, w: &mut Writer);
    /// Stops keeping derived ops (unobserved, or a delivery was abandoned); the index stays.
    fn forget(&self);
    /// Whether a snapshot carries the slot: a derived list is rebuilt on restore, a lazy list
    /// (ADR-043) is store state.
    fn persisted(&self) -> bool {
        false
    }
    /// The slot as a lazy list, when it is one.
    fn lazy(&self) -> Option<&LazyCell> {
        None
    }
}

/// A derived list attached to a store, with the key function of its rows.
pub(crate) struct Attached<U> {
    pub(crate) node: Arc<dyn DerivedNode<U>>,
    /// Hashes a row's key field: debug builds check at every full value that the view's keys are
    /// unique. Maintenance never looks at keys.
    pub(crate) key: fn(&U) -> u64,
}

impl<U: Send + Sync + 'static> DerivedSlot for Attached<U> {
    fn commit(&self, w: &mut Writer, retain: bool) -> Emitted {
        self.node.commit(w, retain, self.key)
    }

    fn resync(&self, w: &mut Writer) {
        self.node.resync(w, self.key);
    }

    fn forget(&self) {
        self.node.forget();
    }
}
