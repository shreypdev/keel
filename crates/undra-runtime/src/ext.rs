//! Extension points for crates layered on the runtime (`undra-ports`, `undra-query`).
//!
//! * [`Runtime::extension`](crate::Runtime::extension) is a per-runtime type map: a crate
//!   stores its own state (the query cache, say) on the runtime and gets `&T` back with the
//!   lifetime of the runtime, from any thread, without unsafe code.
//! * [`InitHook`]s run when a runtime is created ([`Runtime::init`](crate::Runtime::init),
//!   [`Runtime::new`](crate::Runtime::new)); `undra-query` has one that hydrates its cache from
//!   the `Kv` port, submitted by every `#[undra::query]` and `#[undra::mutation]` (ADR-052). A
//!   hook receives the [`Ctx`] with the core lock held and may spawn tasks (hydration is async).

use core::any::{Any, TypeId};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;

use crate::ctx::Ctx;

/// What an inspector answers with: one JSON document describing a part of the runtime's state
/// (ADR-054). It runs on whatever thread asks (the dev server's devtools hub), never with the
/// core lock held by the caller, and must not call back into the runtime or block.
pub type InspectFn = Arc<dyn Fn() -> String + Send + Sync>;

/// One registered inspector.
struct Slot {
    name: &'static str,
    inspect: InspectFn,
    /// Set by the first panic: a broken inspector is not called again (each call would pay for a
    /// backtrace) until it is registered anew.
    broken: AtomicBool,
}

/// What asking an inspector came to.
pub(crate) enum Answer {
    /// No inspector has that name, or it panicked before and is skipped.
    None,
    /// Its document.
    Document(String),
    /// It panicked just now: the caller reports it, once.
    Panicked(crate::guard::PanicReport),
}

/// The inspectors registered with a runtime, by name. Dev tooling reads them; nothing in the
/// core does.
#[derive(Default)]
pub(crate) struct Inspectors {
    list: Mutex<Vec<Arc<Slot>>>,
}

impl Inspectors {
    /// Registers `inspect` under `name`, replacing an earlier one of the same name.
    pub(crate) fn register(&self, name: &'static str, inspect: InspectFn) {
        let slot = Arc::new(Slot {
            name,
            inspect,
            broken: AtomicBool::new(false),
        });
        let mut list = self.list.lock();
        match list.iter_mut().find(|s| s.name == name) {
            Some(existing) => *existing = slot,
            None => list.push(slot),
        }
    }

    /// The document `name` produces now. A panic is contained: it is returned to the caller to
    /// report, and the inspector is not asked again.
    pub(crate) fn inspect(&self, name: &str) -> Answer {
        let Some(slot) = self.list.lock().iter().find(|s| s.name == name).cloned() else {
            return Answer::None;
        };
        if slot.broken.load(Ordering::Acquire) {
            return Answer::None;
        }
        // Outside the lock: an inspector may take locks of its own, and registering from inside
        // one would otherwise deadlock.
        let f = slot.inspect.clone();
        match crate::guard::guarded(move || f()) {
            Ok(document) => Answer::Document(document),
            Err(report) => {
                slot.broken.store(true, Ordering::Release);
                Answer::Panicked(report)
            }
        }
    }

    /// The names registered, in registration order.
    pub(crate) fn names(&self) -> Vec<&'static str> {
        self.list.lock().iter().map(|s| s.name).collect()
    }
}

/// Something to run once for every new runtime. Submit with `inventory::submit!`.
///
/// The name identifies the hook: a hook submitted more than once under one name runs once per
/// runtime (the first submission linked). That lets the code that needs a hook submit it, rather
/// than the crate that implements it, so the hook and everything it reaches are linked only into
/// cores that use them (ADR-052: every `#[undra::query]` and `#[undra::mutation]` submits
/// `undra-query`'s hydration hook).
///
/// ```ignore
/// inventory::submit! {
///     undra_runtime::InitHook { name: "undra-query.hydrate", run: |ctx| { ctx.spawn(hydrate(ctx.clone())); } }
/// }
/// ```
pub struct InitHook {
    /// Shown in the log if the hook panics, and the hook's identity: one hook per name runs.
    pub name: &'static str,
    /// The hook. Runs under the core lock; must not block.
    pub run: fn(&Ctx),
}

inventory::collect!(InitHook);

/// A section a layered crate adds to [`Runtime::stats_json`](crate::Runtime::stats_json):
/// `"<name>": <json>` (`undra-query` reports its persistence counters as `"query"`). Added at run
/// time with [`Runtime::add_stats_section`](crate::Runtime::add_stats_section), when the crate
/// first keeps state on a runtime, so a core that never uses the crate does not link it (ADR-052).
#[derive(Clone, Copy)]
pub struct StatsSection {
    /// The key of the section in the stats document.
    pub name: &'static str,
    /// The section's JSON value, or `None` to leave it out (the crate has no state on this
    /// runtime yet). Called without the core lock; must not block or call into the runtime's
    /// host.
    pub json: fn(&crate::Runtime) -> Option<String>,
}

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
    /// The `T` stored here, if one was created.
    pub(crate) fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.find::<T>()
    }

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
