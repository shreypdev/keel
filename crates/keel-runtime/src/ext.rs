//! Extension points for crates layered on the runtime (`keel-ports`, `keel-query`).
//!
//! * [`Runtime::extension`](crate::Runtime::extension) is a per-runtime type map: a crate
//!   stores its own state (the query cache, say) on the runtime and gets `&T` back with the
//!   lifetime of the runtime, from any thread, without unsafe code.
//! * [`InitHook`]s run when a runtime is created ([`Runtime::init`](crate::Runtime::init),
//!   [`Runtime::new`](crate::Runtime::new)); `keel-query` submits one that hydrates its cache
//!   from the `Kv` port. A hook receives the [`Ctx`] with the core lock held and may spawn
//!   tasks (hydration is async).

use core::any::{Any, TypeId};
use std::sync::OnceLock;

use crate::ctx::Ctx;

/// Something to run once for every new runtime. Submit with `inventory::submit!`.
///
/// ```ignore
/// inventory::submit! {
///     keel_runtime::InitHook { name: "keel-query.hydrate", run: |ctx| { ctx.spawn(hydrate(ctx.clone())); } }
/// }
/// ```
pub struct InitHook {
    /// Shown in the log if the hook panics.
    pub name: &'static str,
    /// The hook. Runs under the core lock; must not block.
    pub run: fn(&Ctx),
}

inventory::collect!(InitHook);

struct Node {
    type_id: TypeId,
    value: Box<dyn Any + Send + Sync>,
    next: OnceLock<Box<Node>>,
}

/// An append-only, lock-free list of values keyed by their type. Nodes are never removed, so
/// references stay valid for as long as the list does.
#[derive(Default)]
pub(crate) struct Extensions {
    head: OnceLock<Box<Node>>,
}

impl Extensions {
    /// Returns the `T` stored here, creating it with `init` if this is the first request.
    ///
    /// If two threads race to create the same `T`, one value wins and the other is dropped
    /// (`init` may run more than once, but only one result is ever observable).
    pub(crate) fn get_or_init<T: Send + Sync + 'static>(&self, init: impl FnOnce() -> T) -> &T {
        let wanted = TypeId::of::<T>();
        if let Some(found) = self.find::<T>() {
            return found;
        }
        let mut candidate = Some(Box::new(Node {
            type_id: wanted,
            value: Box::new(init()),
            next: OnceLock::new(),
        }));
        let mut slot = &self.head;
        loop {
            match slot.get() {
                Some(node) => {
                    if node.type_id == wanted {
                        if let Some(value) = node.value.downcast_ref::<T>() {
                            return value;
                        }
                    }
                    slot = &node.next;
                }
                None => {
                    if let Some(node) = candidate.take() {
                        if let Err(rejected) = slot.set(node) {
                            // Another thread appended here first; look again from this slot.
                            candidate = Some(rejected);
                        }
                    }
                }
            }
        }
    }

    fn find<T: Send + Sync + 'static>(&self) -> Option<&T> {
        let wanted = TypeId::of::<T>();
        let mut slot = &self.head;
        while let Some(node) = slot.get() {
            if node.type_id == wanted {
                if let Some(value) = node.value.downcast_ref::<T>() {
                    return Some(value);
                }
            }
            slot = &node.next;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn values_are_created_once_per_type() {
        let ext = Extensions::default();
        let calls = AtomicUsize::new(0);
        let a: &u32 = ext.get_or_init(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            7
        });
        let b: &u32 = ext.get_or_init(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            8
        });
        assert_eq!((*a, *b), (7, 7));
        assert!(std::ptr::eq(a, b));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn different_types_coexist() {
        let ext = Extensions::default();
        let n: &u32 = ext.get_or_init(|| 1);
        let s: &String = ext.get_or_init(|| "two".to_owned());
        let v: &Vec<u8> = ext.get_or_init(|| vec![3]);
        assert_eq!((*n, s.as_str(), v.as_slice()), (1, "two", &[3][..]));
        assert_eq!(*ext.get_or_init(|| 99_u32), 1);
    }

    #[test]
    fn concurrent_initialisation_yields_one_shared_value() {
        let ext = Arc::new(Extensions::default());
        let handles: Vec<_> = (0..8_u32)
            .map(|i: u32| {
                let ext = ext.clone();
                std::thread::spawn(move || {
                    let v: &AtomicUsize = ext.get_or_init(|| AtomicUsize::new(0));
                    v.fetch_add(1, Ordering::SeqCst);
                    let other: &u64 = ext.get_or_init(move || u64::from(i));
                    (v as *const AtomicUsize as usize, *other)
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(
            results.iter().all(|r| r.0 == results[0].0),
            "one AtomicUsize"
        );
        assert!(results.iter().all(|r| r.1 == results[0].1), "one u64");
        let v: &AtomicUsize = ext.get_or_init(|| AtomicUsize::new(100));
        assert_eq!(v.load(Ordering::SeqCst), 8);
    }
}
