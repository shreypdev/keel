//! Objects and stores as the runtime sees them (SPEC 5.4, 16.2).
//!
//! * [`KeelObject`] and [`StoreObject`] are the traits `#[keel::api]` and `#[keel::store]`
//!   implement on user types; both carry compile-time constants and static functions, so
//!   they are not object safe.
//! * [`KeelObjectDyn`] and [`AnyObject`] are their object-safe counterparts. The object table
//!   holds `Arc<dyn AnyObject>`. Generated code never builds one: it calls
//!   [`Runtime::insert_object`](crate::Runtime::insert_object) with its `Arc<T>`, and the runtime
//!   wraps it. [`plain`] and [`store`] do the same for code that talks to an
//!   [`ObjectTable`](crate::object_table::ObjectTable) directly.
//! * [`StoreRestorer`] is submitted through `inventory` by `#[keel::store]` (one per store
//!   type). It lets the runtime treat any `Arc<dyn Any>` of that type as a store (find its
//!   [`StoreCell`] without generics) and rebuild stores from a snapshot knowing only their
//!   type id.

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
    /// `r` is positioned at the store's snapshot **body**: `signal_count u32` followed by
    /// `signal_count x { signal_id u32, len u32, value }`. The `handle` and `type_id` that
    /// precede the body in a snapshot record have already been consumed by the runtime.
    /// Computed signals are absent and must be recomputed. The reader is checked for trailing
    /// bytes afterwards.
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

/// What the object table stores: a shared `dyn Any` with the object-safe [`KeelObjectDyn`]
/// accessors.
///
/// The runtime builds these; implement it by hand only for runtime-internal object kinds.
pub trait AnyObject: Any + Send + Sync + KeelObjectDyn {
    /// The object as a shared `dyn Any`, for downcasting to its concrete type.
    fn shared(&self) -> Arc<dyn Any + Send + Sync>;
}

impl dyn AnyObject {
    /// Returns the object as an `Arc<T>` if that is its concrete type.
    pub fn downcast<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.shared().downcast::<T>().ok()
    }
}

/// Finds the [`StoreCell`] of a store held as `dyn Any`; `None` if it is not that store.
pub type CellFn = fn(&(dyn Any + Send + Sync)) -> Option<&Arc<StoreCell>>;

/// The runtime's [`AnyObject`]: a shared `dyn Any` plus what the runtime needs to know about it.
struct Erased {
    any: Arc<dyn Any + Send + Sync>,
    type_id: u32,
    name: &'static str,
    cell: Option<CellFn>,
}

impl KeelObjectDyn for Erased {
    fn keel_type_id(&self) -> u32 {
        self.type_id
    }

    fn keel_type_name(&self) -> &'static str {
        self.name
    }

    fn as_store(&self) -> Option<&Arc<StoreCell>> {
        (self.cell?)(&*self.any)
    }
}

impl AnyObject for Erased {
    fn shared(&self) -> Arc<dyn Any + Send + Sync> {
        self.any.clone()
    }
}

/// Wraps an erased object of a known type for the object table.
pub(crate) fn erased(
    any: Arc<dyn Any + Send + Sync>,
    type_id: u32,
    name: &'static str,
    cell: Option<CellFn>,
) -> Arc<dyn AnyObject> {
    Arc::new(Erased {
        any,
        type_id,
        name,
        cell,
    })
}

/// Wraps a plain object for the object table.
pub fn plain<T: KeelObject>(obj: Arc<T>) -> Arc<dyn AnyObject> {
    erased(obj, T::TYPE_ID, T::NAME, None)
}

/// Wraps a store for the object table; the runtime will see its cell.
pub fn store<T: StoreObject>(obj: Arc<T>) -> Arc<dyn AnyObject> {
    fn cell_of<T: StoreObject>(any: &(dyn Any + Send + Sync)) -> Option<&Arc<StoreCell>> {
        any.downcast_ref::<T>().map(T::cell)
    }
    erased(obj, T::TYPE_ID, T::NAME, Some(cell_of::<T>))
}

/// Rebuilds one store type from a snapshot (SPEC 5.9). `#[keel::store]` submits one per type;
/// the runtime also uses `cell` to recognise stores at insert time, so every store type must
/// have one.
///
/// ```ignore
/// keel_meta::inventory::submit! {
///     keel_runtime::StoreRestorer {
///         type_id: <Todos as KeelObject>::TYPE_ID,
///         restore: |ctx, handle, r| {
///             let store = <Todos as StoreObject>::restore(ctx, r)?;
///             store.cell().set_handle(handle);
///             Ok(Arc::new(store))
///         },
///         cell: |any| any.downcast_ref::<Todos>().map(<Todos as StoreObject>::cell),
///     }
/// }
/// ```
pub struct StoreRestorer {
    /// The store's type id.
    pub type_id: u32,
    /// Builds the store from its snapshot body (see [`StoreObject::restore`]); `handle` is the
    /// handle the snapshot re-issues, which the store's cell must be told.
    pub restore: RestoreFn,
    /// Finds the store's cell inside a `dyn Any` holding an instance of this type.
    pub cell: CellFn,
}

/// The signature of [`StoreRestorer::restore`]: context, the re-issued raw handle, the store
/// body.
pub type RestoreFn = fn(Ctx, u64, &mut Reader<'_>) -> Result<Arc<dyn Any + Send + Sync>, WireError>;

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

    #[test]
    fn a_cell_accessor_makes_an_erased_object_a_store() {
        struct Gadget {
            cell: Arc<StoreCell>,
        }
        impl KeelObject for Gadget {
            const TYPE_ID: u32 = 12;
            const NAME: &'static str = "Gadget";
        }
        impl StoreObject for Gadget {
            fn cell(&self) -> &Arc<StoreCell> {
                &self.cell
            }
            fn restore(_: Ctx, _: &mut Reader<'_>) -> Result<Self, WireError> {
                Ok(Gadget {
                    cell: StoreCell::new(12),
                })
            }
        }
        let gadget = Arc::new(Gadget {
            cell: StoreCell::new(12),
        });
        let as_store = store(gadget.clone());
        assert!(Arc::ptr_eq(as_store.as_store().unwrap(), gadget.cell()));
        assert!(Arc::ptr_eq(
            &as_store.downcast::<Gadget>().unwrap(),
            &gadget
        ));
        // The same value wrapped without a cell accessor is not a store.
        let as_plain = plain(gadget);
        assert!(as_plain.as_store().is_none());
    }
}
