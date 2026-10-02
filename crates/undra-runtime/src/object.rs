//! Objects and stores as the runtime sees them (SPEC 5.4, 16.2).
//!
//! * [`UndraObject`] and [`StoreObject`] are the traits `#[undra::api]` and `#[undra::store]`
//!   implement on user types; both carry compile-time constants and static functions, so
//!   they are not object safe.
//! * [`UndraObjectDyn`] and [`AnyObject`] are their object-safe counterparts. The object table
//!   holds `Arc<dyn AnyObject>`. Generated code never builds one: it calls
//!   [`Runtime::insert_object`](crate::Runtime::insert_object) with its `Arc<T>`, and the runtime
//!   wraps it. [`plain`] and [`store`] do the same for code that talks to an
//!   [`ObjectTable`](crate::object_table::ObjectTable) directly.
//! * [`StoreRestorer`] is submitted through `inventory` by `#[undra::store]` (one per store
//!   type). It lets the runtime treat any `Arc<dyn Any>` of that type as a store (find its
//!   [`StoreCell`] without generics) and rebuild stores from a snapshot knowing only their
//!   type id.
//! * [`Reviver`] is what a layered crate adds to a runtime
//!   ([`Runtime::add_reviver`](crate::Runtime::add_reviver)) to build objects that are not stores
//!   again after a restore, from the record their [`recreation`](UndraObjectDyn::recreation) wrote
//!   (ADR-059: query handles).

use std::any::Any;
use std::sync::Arc;

use undra_signals::StoreCell;
use undra_wire::{Reader, WireError};

use crate::ctx::Ctx;

/// A type that crosses the boundary by handle (implemented by `#[undra::api] impl Type`).
pub trait UndraObject: Send + Sync + 'static {
    /// `fnv1a32("<TypeName>")`.
    const TYPE_ID: u32;
    /// The Rust type name.
    const NAME: &'static str;

    /// The signal cell, when the object is a `#[undra::store]` (`None` for any other). Generated
    /// by `#[undra::api(store)]`; the runtime uses it to hand a store the host did not construct
    /// to the host (ADR-040).
    #[doc(hidden)]
    fn __undra_store_cell(&self) -> Option<&Arc<StoreCell>> {
        None
    }

    /// Attaches a store's signals to its cell (idempotent; a no-op for any other object). Called
    /// before the runtime publishes an object a method returned.
    #[doc(hidden)]
    fn __undra_attach(&self) -> Result<(), undra_signals::SignalsError> {
        Ok(())
    }
}

/// An object with signal fields that platforms mirror (implemented by `#[undra::store]`).
pub trait StoreObject: UndraObject {
    /// The store's cell in `undra-signals`; the runtime sets its handle on insert and asks it
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

/// The object-safe view of an [`UndraObject`].
pub trait UndraObjectDyn: Send + Sync + 'static {
    /// The object's type id (`UndraObject::TYPE_ID`).
    fn undra_type_id(&self) -> u32;
    /// The object's type name (`UndraObject::NAME`).
    fn undra_type_name(&self) -> &'static str;
    /// The store cell, if the object is a store.
    fn as_store(&self) -> Option<&Arc<StoreCell>> {
        None
    }

    /// Whether the object is derived state that a snapshot does not carry **as a store**. `false`
    /// for everything `#[undra::store]` generates. `undra-query`'s query handles answer `true`:
    /// they are views of the query cache, which a restore does not rebuild, so a snapshot leaves
    /// them out of its stores (instead of the whole restore failing for lack of a
    /// `StoreRestorer`) and keeps what they are made of as a recreation record instead
    /// ([`recreation`](UndraObjectDyn::recreation), ADR-059).
    fn transient(&self) -> bool {
        false
    }

    /// What a snapshot keeps of an object that is not snapshotted as a store but can be built
    /// again from what it was made of (ADR-059): a query handle's parameters. The bytes are the
    /// business of the [`Reviver`] that claims the object's type id; a restore re-issues the
    /// object's handle from them and the object is built again when the host first uses the
    /// handle. `None` (the default) for everything else: its handle is stale after a restore into
    /// another runtime.
    ///
    /// Called without any lock of the runtime held (it may take the lock of whatever the object
    /// is a view of).
    fn recreation(&self) -> Option<Vec<u8>> {
        None
    }
}

/// What the object table stores: a shared `dyn Any` with the object-safe [`UndraObjectDyn`]
/// accessors.
///
/// The runtime builds these; implement it by hand only for runtime-internal object kinds.
pub trait AnyObject: Any + Send + Sync + UndraObjectDyn {
    /// The object as a shared `dyn Any`, for downcasting to its concrete type.
    fn shared(&self) -> Arc<dyn Any + Send + Sync>;

    /// Where the object lives: the address of the `Arc` the runtime holds (what the object table
    /// keys an object's single handle by, ADR-040). Equal addresses are the same object.
    fn address(&self) -> usize {
        Arc::as_ptr(&self.shared()).cast::<()>() as usize
    }
}

impl dyn AnyObject {
    /// Returns the object as an `Arc<T>` if that is its concrete type.
    pub fn downcast<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.shared().downcast::<T>().ok()
    }
}

/// Builds objects again from the recreation records a snapshot kept
/// ([`UndraObjectDyn::recreation`], ADR-059). A layered crate adds one to a runtime with
/// [`Runtime::add_reviver`](crate::Runtime::add_reviver) (the runtime keeps one per name), the way
/// it adds a stats section: the code behind it is linked only into a core that adds one.
///
/// A restore asks the reviver three things about each record before it re-issues the record's
/// handle, and refuses the record (the handle stays stale, the restore goes on) when the answer
/// is no: whether it builds objects of the record's type at all, whether the fingerprint of what
/// the record depends on is still the one the snapshot was written with, and whether the record
/// is well formed ([`check`](Reviver::check)). Nothing is built, and no port is called, until the
/// host first uses the handle ([`revive`](Reviver::revive)).
#[derive(Clone, Copy)]
pub struct Reviver {
    /// The reviver's identity, and what logs call it. A runtime keeps one reviver per name.
    pub name: &'static str,
    /// What a re-issued handle reports as its object's type name until it is built
    /// (`UndraObjectDyn::undra_type_name`), as `"QueryHandle"` does.
    pub object_name: &'static str,
    /// Whether this reviver builds objects of `type_id`, and if so the fingerprint of what their
    /// record depends on (for a query handle: the closure of the query's parameters). A snapshot
    /// stores it per type; a restore refuses the records of a type whose current fingerprint is
    /// another. Read from the schema only, and cached per type id by the runtime.
    pub fingerprint: fn(&crate::Runtime, u32) -> Option<u64>,
    /// Checks a record without building anything and without calling a port (a restore must be a
    /// function of the state and the snapshot, R12): `Err` refuses the record. A record that
    /// passes must not fail to build later because of its bytes.
    pub check: fn(&crate::Runtime, u32, &[u8]) -> Result<(), String>,
    /// Builds the object of a record that passed [`check`](Reviver::check). Called when the host
    /// first uses the handle, with the core lock held and the runtime current, as in a dispatched
    /// call: the same ports and the same rules as the constructor call the host made the first
    /// time. `Err` (or a panic) makes the handle stale for good.
    pub revive: ReviveFn,
}

/// The signature of [`Reviver::revive`].
type ReviveFn = fn(&crate::Runtime, u32, &[u8]) -> Result<Arc<dyn AnyObject>, String>;

impl std::fmt::Debug for Reviver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reviver")
            .field("name", &self.name)
            .field("object_name", &self.object_name)
            .finish_non_exhaustive()
    }
}

/// What a restore puts behind a re-issued handle until the host uses it: the record, and who
/// builds the object from it (ADR-059). The object table holds it like any object, so the handle
/// is live: it counts, routes by its type id, and is carried on by the next snapshot.
pub(crate) struct Dormant {
    pub(crate) type_id: u32,
    pub(crate) record: Vec<u8>,
    pub(crate) reviver: Reviver,
}

/// The [`AnyObject`] of a [`Dormant`] entry (it downcasts to `Dormant`, which is how a use of the
/// handle finds it).
pub(crate) struct DormantObject(pub(crate) Arc<Dormant>);

impl UndraObjectDyn for DormantObject {
    fn undra_type_id(&self) -> u32 {
        self.0.type_id
    }

    fn undra_type_name(&self) -> &'static str {
        self.0.reviver.object_name
    }

    fn transient(&self) -> bool {
        true
    }

    fn recreation(&self) -> Option<Vec<u8>> {
        Some(self.0.record.clone())
    }
}

impl AnyObject for DormantObject {
    fn shared(&self) -> Arc<dyn Any + Send + Sync> {
        self.0.clone()
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

impl UndraObjectDyn for Erased {
    fn undra_type_id(&self) -> u32 {
        self.type_id
    }

    fn undra_type_name(&self) -> &'static str {
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

/// Wraps an object (a store or not, as `T` says) for the object table: what `issue` uses for an
/// object a method returned (ADR-040).
pub(crate) fn any_object<T: UndraObject>(obj: Arc<T>) -> Arc<dyn AnyObject> {
    fn cell_of<T: UndraObject>(any: &(dyn Any + Send + Sync)) -> Option<&Arc<StoreCell>> {
        any.downcast_ref::<T>().and_then(T::__undra_store_cell)
    }
    erased(obj, T::TYPE_ID, T::NAME, Some(cell_of::<T>))
}

/// Wraps a plain object for the object table.
pub fn plain<T: UndraObject>(obj: Arc<T>) -> Arc<dyn AnyObject> {
    erased(obj, T::TYPE_ID, T::NAME, None)
}

/// Wraps a store for the object table; the runtime will see its cell.
pub fn store<T: StoreObject>(obj: Arc<T>) -> Arc<dyn AnyObject> {
    fn cell_of<T: StoreObject>(any: &(dyn Any + Send + Sync)) -> Option<&Arc<StoreCell>> {
        any.downcast_ref::<T>().map(T::cell)
    }
    erased(obj, T::TYPE_ID, T::NAME, Some(cell_of::<T>))
}

/// Rebuilds one store type from a snapshot (SPEC 5.9). `#[undra::store]` submits one per type;
/// the runtime also uses `cell` to recognise stores at insert time, so every store type must
/// have one.
///
/// ```ignore
/// undra_meta::inventory::submit! {
///     undra_runtime::StoreRestorer {
///         type_id: <Todos as UndraObject>::TYPE_ID,
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
    impl UndraObject for Widget {
        const TYPE_ID: u32 = 11;
        const NAME: &'static str = "Widget";
    }

    #[test]
    fn plain_wrapper_exposes_identity_and_downcasts_to_the_callers_arc() {
        let widget = Arc::new(Widget);
        let obj = plain(widget.clone());
        assert_eq!(obj.undra_type_id(), 11);
        assert_eq!(obj.undra_type_name(), "Widget");
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
        impl UndraObject for Gadget {
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
