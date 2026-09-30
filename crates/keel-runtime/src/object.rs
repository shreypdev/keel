//! Objects and stores as the runtime sees them (SPEC 5.4, 16.2).
//!
//! * [`KeelObject`] and [`StoreObject`] are the traits `#[keel::api]` and `#[keel::store]`
//!   implement on user types; both carry compile-time constants and static functions, so
//!   they are not object safe.
//! * [`KeelObjectDyn`] and [`AnyObject`] are their object-safe counterparts. The object table
//!   holds `Arc<dyn AnyObject>`; [`plain`] and [`store`] wrap a typed `Arc<T>` into one. The
//!   wrapper is what lets the runtime encode a store's snapshot without generics
//!   ([`KeelObjectDyn::as_store`]) while [`dyn AnyObject::downcast`](AnyObject) still returns
//!   the caller's own `Arc<T>`.
//! * [`StoreRestorer`] is submitted through `inventory` by `#[keel::store]` (one per store
//!   type) so [`Runtime::restore`](crate::Runtime::restore) can rebuild stores from a
//!   snapshot knowing only their type id.

use std::any::Any;
use std::sync::Arc;

use keel_signals::StoreCell;
use keel_wire::{Reader, WireError};

use crate::ctx::Ctx;

/// A type that crosses the boundary by handle (implemented by `#[keel::api] impl Type`).
pub trait KeelObject: Send + Sync + 'static {
    /// `fnv1a32("<TypeName>")`.
    const TYPE_ID: u32;
    /// The Rust type name.
    const NAME: &'static str;
}

/// An object with signal fields that platforms mirror (implemented by `#[keel::store]`).
pub trait StoreObject: KeelObject {
    /// The store's cell in `keel-signals`; the runtime sets its handle on insert and asks it
    /// to encode observations and snapshots.
    fn cell(&self) -> &Arc<StoreCell>;

    /// Rebuilds the store from snapshot values.
    ///
    /// `r` is positioned at the store's snapshot body, exactly what
    /// [`StoreCell::encode_snapshot`] wrote: `signal_count u32` followed by
    /// `signal_count x { signal_id u32, len u32, value }`. Computed signals are absent and
    /// must be recomputed. The reader is checked for trailing bytes afterwards.
    fn restore(ctx: Ctx, r: &mut Reader<'_>) -> Result<Self, WireError>
    where
        Self: Sized;
}

/// The object-safe view of a [`KeelObject`].
pub trait KeelObjectDyn: Send + Sync + 'static {
    /// The object's type id (`KeelObject::TYPE_ID`).
    fn keel_type_id(&self) -> u32;
    /// The object's type name (`KeelObject::NAME`).
    fn keel_type_name(&self) -> &'static str;
    /// The store cell, if the object is a store.
    fn as_store(&self) -> Option<&Arc<StoreCell>> {
        None
    }
}

/// What the object table stores: an `Any` with the object-safe [`KeelObjectDyn`] accessors.
///
/// Implemented by the wrappers [`plain`] and [`store`] build; implement it by hand only for
/// runtime-internal object kinds.
pub trait AnyObject: Any + Send + Sync + KeelObjectDyn {
    /// `self` as `&dyn Any`, for downcasting to the wrapper.
    fn as_any(&self) -> &dyn Any;
}

impl dyn AnyObject {
    /// Returns the wrapped `Arc<T>` if this object was built from a `T` with [`plain`] or
    /// [`store`].
    pub fn downcast<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let any = self.as_any();
        if let Some(w) = any.downcast_ref::<PlainObject<T>>() {
            return Some(w.0.clone());
        }
        any.downcast_ref::<StoreBox<T>>().map(|w| w.0.clone())
    }
}

struct PlainObject<T>(Arc<T>);
struct StoreBox<T>(Arc<T>);

impl<T: KeelObject> KeelObjectDyn for PlainObject<T> {
    fn keel_type_id(&self) -> u32 {
        T::TYPE_ID
    }
    fn keel_type_name(&self) -> &'static str {
        T::NAME
    }
}

impl<T: KeelObject> AnyObject for PlainObject<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl<T: StoreObject> KeelObjectDyn for StoreBox<T> {
    fn keel_type_id(&self) -> u32 {
        T::TYPE_ID
    }
    fn keel_type_name(&self) -> &'static str {
        T::NAME
    }
    fn as_store(&self) -> Option<&Arc<StoreCell>> {
        Some(self.0.cell())
    }
}

impl<T: StoreObject> AnyObject for StoreBox<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Wraps a plain object for the object table.
pub fn plain<T: KeelObject>(obj: Arc<T>) -> Arc<dyn AnyObject> {
    Arc::new(PlainObject(obj))
}

/// Wraps a store for the object table; the runtime will see its cell.
pub fn store<T: StoreObject>(obj: Arc<T>) -> Arc<dyn AnyObject> {
    Arc::new(StoreBox(obj))
}

/// Rebuilds one store type from a snapshot (SPEC 5.9). `#[keel::store]` submits one per type:
///
/// ```ignore
/// keel_meta::inventory::submit! {
///     keel_runtime::StoreRestorer {
///         type_id: <Todos as KeelObject>::TYPE_ID,
///         restore: |ctx, r| Ok(keel_runtime::store(Arc::new(<Todos as StoreObject>::restore(ctx, r)?))),
///     }
/// }
/// ```
pub struct StoreRestorer {
    /// The store's type id.
    pub type_id: u32,
    /// Builds the store from its snapshot body (see [`StoreObject::restore`]).
    pub restore: RestoreFn,
}

/// The signature of [`StoreRestorer::restore`].
pub type RestoreFn = fn(Ctx, &mut Reader<'_>) -> Result<Arc<dyn AnyObject>, WireError>;

inventory::collect!(StoreRestorer);

#[cfg(test)]
mod tests {
    use super::*;

    struct Widget;
    impl KeelObject for Widget {
        const TYPE_ID: u32 = 11;
        const NAME: &'static str = "Widget";
    }

    #[test]
    fn plain_wrapper_exposes_identity_and_downcasts_to_the_callers_arc() {
        let widget = Arc::new(Widget);
        let obj = plain(widget.clone());
        assert_eq!(obj.keel_type_id(), 11);
        assert_eq!(obj.keel_type_name(), "Widget");
        assert!(obj.as_store().is_none());
        let back = obj.downcast::<Widget>().unwrap();
        assert!(Arc::ptr_eq(&back, &widget));
        assert!(obj.downcast::<String>().is_none());
    }
}
