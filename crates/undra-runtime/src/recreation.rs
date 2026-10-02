//! Objects that are built again after a restore (ADR-059): what a snapshot keeps of a query
//! handle, what a restore does with it, and what builds the handle's object when the host first
//! uses it.
//!
//! The model, in four sentences. A snapshot keeps, for each object the host holds whose
//! [`recreation`](crate::UndraObjectDyn::recreation) is `Some`, one **recreation record** beside
//! the stores' records (the record shape of a store, with one field under
//! [`RECREATION_FIELD`](undra_wire::payload::RECREATION_FIELD)). A restore re-issues the handle of
//! each record this build can honour as a **dormant** entry (the record and the
//! [`Reviver`](crate::Reviver) that claims its type), builds nothing and calls no port. The object
//! is built, in the same entry, the first time the host uses the handle (`observe`, a call on it,
//! or passing it as an object parameter): the same ports and rules as the constructor call the
//! host made the first time. A live re-creatable object is not state, so a restore into the
//! runtime that holds it leaves it exactly as it is.
//!
//! **Link by use.** Everything here is reached through [`Hooks`], a table of function pointers
//! that the first [`Runtime::add_reviver`] installs, the way the first store with a `Lazy` field
//! installs the page dispatcher (ADR-052): a core that adds no reviver (one without queries) links
//! none of it, and handles a recreation record as a store type it does not have (ADR-037
//! decision 7: left out, a WARN, `RestoreReport::dropped`), which is also what a runtime older
//! than ADR-059 does with one. The hello-world wasm gate (120,000 bytes gzipped) is why.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use undra_wire::payload::{RECREATION_FIELD, Snapshot, StoreSnapshot};
use undra_wire::{Handle, Writer};

use crate::config::{RefusedHandle, RestoreReport};
use crate::guard;
use crate::log::{DEBUG, ERROR, WARN};
use crate::object::{AnyObject, Dormant, DormantObject, Reviver};
use crate::object_table::{BadHandle, BadHandleReason, Cleared, InsertAtError};
use crate::runtime::{Runtime, object_address};

/// What a restore re-issues: the handle and the dormant object behind it.
pub(crate) type Reissue = Vec<(Handle, Arc<dyn AnyObject>)>;

/// The encoded records of a snapshot, one chunk each.
type Chunks = Vec<Vec<u8>>;

/// What a restore built or kept: the handle and its object.
type Placed = [(Handle, Arc<dyn AnyObject>)];

/// The signature of [`Hooks::snapshot`]: the chunks so far, the types the records add (id and
/// fingerprint) and the store types already in the table.
type SnapshotFn = fn(&Runtime, &mut Chunks, &mut Vec<(u32, u64)>, &[u32]);

/// The signature of [`Hooks::clear`].
type ClearFn = fn(&Runtime, &Placed, &mut HashMap<u64, usize>, &mut RestoreReport) -> Vec<Cleared>;

/// The signature of [`Hooks::revive_object`].
type ReviveObjectFn =
    fn(&Runtime, Handle, Arc<dyn AnyObject>) -> Result<Arc<dyn AnyObject>, String>;

/// A runtime's revivers, and the table of the recreation code ([`Runtime::add_reviver`] creates it).
pub(crate) struct Recreation {
    /// The code; only this struct names it, so a core that never builds one does not link it.
    pub(crate) hooks: &'static Hooks,
    /// What builds objects again, one per name (appended only: an index stays valid).
    revivers: Mutex<Vec<Reviver>>,
    /// The reviver (its index) and fingerprint of each type id a reviver claimed (the fingerprint
    /// reads the schema and is the same for the life of the runtime, so it is computed once).
    claimed: Mutex<Vec<(u32, usize, u64)>>,
    /// Re-issued handles whose object could not be built when first used (`stats_json`'s
    /// `revive_failed`).
    pub(crate) revive_failed: AtomicU64,
}

impl Recreation {
    fn new() -> Recreation {
        Recreation {
            hooks: &HOOKS,
            revivers: Mutex::new(Vec::new()),
            claimed: Mutex::new(Vec::new()),
            revive_failed: AtomicU64::new(0),
        }
    }
}

/// The recreation code as function pointers (see the module documentation).
pub(crate) struct Hooks {
    /// `snapshot`: appends the recreation records and the types they add to the type table.
    pub(crate) snapshot: SnapshotFn,
    /// `restore`, phase 1: checks one record (builds nothing); an accepted one is queued as a
    /// dormant object, a refused one is reported.
    pub(crate) plan:
        fn(&Runtime, &Snapshot, &StoreSnapshot, &[u8], &mut RestoreReport, &mut Reissue),
    /// `restore`, phase 2: empties the table except the live re-creatable objects no store of the
    /// snapshot needs the slot of.
    pub(crate) clear: ClearFn,
    /// `restore`, phase 2: places the dormant objects.
    pub(crate) place: fn(&Runtime, Reissue, &mut RestoreReport),
    /// `observe`: `object` itself, or the live object built in place of the dormant `object`.
    pub(crate) revive_object: ReviveObjectFn,
    /// `Runtime::object` and `param`: builds the object of `handle` if it is dormant.
    pub(crate) revive_handle: fn(&Runtime, Handle) -> Result<(), BadHandle>,
    /// Whether `object` is a dormant entry.
    pub(crate) is_dormant: fn(&Arc<dyn AnyObject>) -> bool,
    /// How many dormant entries the table holds (`stats_json`).
    pub(crate) dormant_count: fn(&Runtime) -> usize,
}

static HOOKS: Hooks = Hooks {
    snapshot,
    plan,
    clear,
    place,
    revive_object,
    revive_handle,
    is_dormant,
    dormant_count,
};

impl Runtime {
    /// Adds what builds the re-creatable objects of a layered crate again after a restore
    /// (ADR-059), the way [`add_stats_section`](Runtime::add_stats_section) adds a section: at run
    /// time, so a core that never adds one links none of the recreation code. The runtime keeps
    /// one reviver per [`name`](Reviver::name); a second of the same name is ignored.
    ///
    /// Add it before the first restore: a recreation record whose type no reviver claims is
    /// refused ([`RestoreReport::refused`]). `undra-query` adds its own where it adds its stats
    /// section, which its `HYDRATE` init hook reaches when the runtime starts.
    ///
    /// ```
    /// use undra_runtime::testing::TestRuntime;
    /// use undra_runtime::Reviver;
    ///
    /// // A reviver that claims no type: its records would all be refused.
    /// let reviver = Reviver {
    ///     name: "example",
    ///     object_name: "Example",
    ///     fingerprint: |_, _| None,
    ///     check: |_, _, _| Err("never".to_owned()),
    ///     revive: |_, _, _| Err("never".to_owned()),
    /// };
    /// let t = TestRuntime::new();
    /// t.runtime().add_reviver(reviver);
    /// t.runtime().add_reviver(reviver); // the same name: ignored
    /// ```
    pub fn add_reviver(&self, reviver: Reviver) {
        let recreation = self.recreation.get_or_init(Recreation::new);
        let mut revivers = recreation.revivers.lock();
        if !revivers.iter().any(|known| known.name == reviver.name) {
            revivers.push(reviver);
        }
    }

    /// The reviver that builds objects of `type_id`, and the current fingerprint of their records.
    fn reviver_of(&self, type_id: u32) -> Option<(Reviver, u64)> {
        let recreation = self.recreation.get()?;
        if let Some(&(_, at, fingerprint)) = recreation
            .claimed
            .lock()
            .iter()
            .find(|(id, ..)| *id == type_id)
        {
            return recreation
                .revivers
                .lock()
                .get(at)
                .map(|r| (*r, fingerprint));
        }
        // The revivers' own code runs with no lock of ours held.
        let revivers: Vec<Reviver> = recreation.revivers.lock().clone();
        for (at, reviver) in revivers.iter().enumerate() {
            if let Some(fingerprint) = (reviver.fingerprint)(self, type_id) {
                recreation.claimed.lock().push((type_id, at, fingerprint));
                return Some((*reviver, fingerprint));
            }
        }
        None
    }

    /// The recreation record of `object` (`handle`), under the panic guard: a panic is reported
    /// and the object is treated as one that cannot be built again.
    fn recreation_of(
        &self,
        verb: &str,
        handle: Handle,
        object: &Arc<dyn AnyObject>,
    ) -> Option<Vec<u8>> {
        match guard::guarded(|| object.recreation()) {
            Ok(record) => record,
            Err(report) => {
                self.note_panic(verb, &self.store_operation(verb, handle), handle, &report);
                None
            }
        }
    }
}

/// [`Hooks::snapshot`]: one recreation record for every object the host holds a reference to whose
/// [`recreation`](crate::UndraObjectDyn::recreation) is `Some` and whose type a reviver claims
/// (dormant entries included: their record is carried on), in the record shape of a store with one
/// field. The type is added to the type table once, with the reviver's fingerprint.
fn snapshot(rt: &Runtime, chunks: &mut Chunks, types: &mut Vec<(u32, u64)>, store_types: &[u32]) {
    for (handle, object) in rt.objects().held() {
        let Some(record) = rt.recreation_of("snapshot", handle, &object) else {
            continue;
        };
        let type_id = object.undra_type_id();
        let Some((_, fingerprint)) = rt.reviver_of(type_id) else {
            continue;
        };
        let mut chunk = Writer::with_capacity(24 + record.len());
        StoreSnapshot {
            handle,
            type_id,
            signals: vec![(RECREATION_FIELD, record)],
        }
        .encode(&mut chunk);
        chunks.push(chunk.into_vec());
        if !store_types.contains(&type_id) && !types.iter().any(|(known, _)| *known == type_id) {
            types.push((type_id, fingerprint));
        }
    }
}

/// [`Hooks::plan`]: a record is **checked, not built**: the reviver that claims its type is found,
/// its current fingerprint compared with the one the snapshot was written with, and `check` run
/// under the panic guard. A record that fails any of the three is refused (its handle stays stale,
/// a WARN says why) and never fails the restore.
fn plan(
    rt: &Runtime,
    snapshot: &Snapshot,
    s: &StoreSnapshot,
    record: &[u8],
    report: &mut RestoreReport,
    reissue: &mut Reissue,
) {
    let type_id = s.type_id;
    let reason = match rt.reviver_of(type_id) {
        None => "this build has nothing that builds it again".to_owned(),
        Some((_, current)) if snapshot.fingerprint(type_id) != Some(current) => {
            "the types it was made from changed".to_owned()
        }
        Some((reviver, _)) => match guard::guarded(|| (reviver.check)(rt, type_id, record)) {
            Ok(Ok(())) => {
                reissue.push((
                    s.handle,
                    Arc::new(DormantObject(Arc::new(Dormant {
                        type_id,
                        record: record.to_vec(),
                        reviver,
                    }))),
                ));
                return;
            }
            Ok(Err(reason)) => reason,
            Err(panic) => {
                let operation = format!("restore {}", reviver.object_name);
                rt.log_panic("a reviver's check panicked", &operation, &panic);
                format!("checking its record panicked: {}", panic.message)
            }
        },
    };
    rt.log(
        WARN,
        "undra::persist",
        &format!(
            "restore: {:?} ({type_id:#010x}) is not re-issued: {reason}",
            s.handle
        ),
    );
    report.refused.push(RefusedHandle {
        handle: s.handle.0,
        type_id,
        reason,
    });
}

/// [`Hooks::clear`]: empties the table except the live re-creatable objects (dormant ones
/// included): their entry, references, observation, observer in the cache, polling timer, fetch
/// in flight and pages are not touched, and what runs on them is not cancelled (they are in
/// `before` as themselves). The one exception keeps ADR-023's all-or-nothing for stores: when a
/// store of the snapshot needs the slot (the same index under another generation) the store wins,
/// the object there is removed like any other and listed in `displaced`.
fn clear(
    rt: &Runtime,
    built: &Placed,
    before: &mut HashMap<u64, usize>,
    report: &mut RestoreReport,
) -> Vec<Cleared> {
    let needed: HashSet<u32> = built.iter().map(|(handle, _)| handle.index()).collect();
    let mut keep: HashSet<u64> = HashSet::new();
    for (handle, object) in rt.objects().held() {
        if rt.recreation_of("restore", handle, &object).is_none() {
            continue;
        }
        if needed.contains(&handle.index()) {
            report.displaced.push(handle.0);
            continue;
        }
        keep.insert(handle.0);
        before.insert(handle.0, object_address(&object));
        report.reissued += 1;
    }
    rt.objects()
        .clear_keeping(&|handle| keep.contains(&handle.0))
}

/// [`Hooks::place`]: each accepted record whose handle is **not live** is placed at its handle as
/// a dormant entry (one host reference). A record whose handle is live is not needed (the object
/// is there); one whose slot is taken by another kept handle is skipped (DEBUG): the slot was
/// reused, so the handle the record names was released. The generation counter is raised over the
/// records' generations as over the stores' (`insert_at` does it).
fn place(rt: &Runtime, reissue: Reissue, report: &mut RestoreReport) {
    for (handle, object) in reissue {
        match rt.objects().insert_at(handle, object) {
            Ok(()) => report.reissued += 1,
            Err(InsertAtError::Occupied) if rt.objects().type_of(handle).is_ok() => {}
            Err(InsertAtError::Occupied) => rt.log(
                DEBUG,
                "undra::persist",
                &format!(
                    "restore: {handle:?} is not re-issued: its slot was reused since the snapshot, so the handle was released"
                ),
            ),
            Err(e) => rt.log(
                ERROR,
                "undra::runtime",
                &format!("restore: could not place {handle:?}: {e}"),
            ),
        }
    }
}

/// [`Hooks::revive_object`].
fn revive_object(
    rt: &Runtime,
    handle: Handle,
    object: Arc<dyn AnyObject>,
) -> Result<Arc<dyn AnyObject>, String> {
    match object.downcast::<Dormant>() {
        Some(dormant) => revive(rt, handle, &dormant),
        None => Ok(object),
    }
}

/// [`Hooks::revive_handle`]: `Ok` when the handle is not dormant (anymore) or its object was built;
/// `Err` (a stale handle) when it could not be.
fn revive_handle(rt: &Runtime, handle: Handle) -> Result<(), BadHandle> {
    // As `snapshot` does: the lock is taken here when the caller (a task, a test) does not hold it.
    let _core = rt.enter_core().ok();
    let Ok(object) = rt.objects().get_dyn(handle) else {
        return Ok(());
    };
    match revive_object(rt, handle, object) {
        Ok(_) => Ok(()),
        Err(_) => Err(BadHandle {
            handle,
            reason: BadHandleReason::Stale,
        }),
    }
}

/// Builds the object of the dormant entry `handle` from its record and puts it in the same entry:
/// the handle, its references and its slot stay. On failure or a panic the entry is released, the
/// handle is stale for good, an ERROR names the handle, the reviver and the reason, and the
/// failure is counted (`stats_json`'s `revive_failed`).
fn revive(
    rt: &Runtime,
    handle: Handle,
    dormant: &Arc<Dormant>,
) -> Result<Arc<dyn AnyObject>, String> {
    let built = guard::guarded(|| (dormant.reviver.revive)(rt, dormant.type_id, &dormant.record));
    let reason = match built {
        Ok(Ok(live)) => match rt.objects().replace_object(handle, live.clone()) {
            Ok(()) => return Ok(live),
            Err(e) => e.to_string(),
        },
        Ok(Err(reason)) => reason,
        Err(panic) => {
            let operation = format!("revive {}", dormant.reviver.object_name);
            rt.log_panic("a reviver panicked", &operation, &panic);
            format!("it panicked: {}", panic.message)
        }
    };
    rt.release_issued(handle);
    if let Some(recreation) = rt.recreation.get() {
        recreation.revive_failed.fetch_add(1, Ordering::Relaxed);
    }
    let reason = format!(
        "{handle:?} could not be built again after a restore ({}): {reason}; the handle is stale",
        dormant.reviver.name
    );
    rt.log(ERROR, "undra::runtime", &reason);
    Err(reason)
}

/// [`Hooks::is_dormant`].
fn is_dormant(object: &Arc<dyn AnyObject>) -> bool {
    object.downcast::<Dormant>().is_some()
}

/// [`Hooks::dormant_count`].
fn dormant_count(rt: &Runtime) -> usize {
    rt.objects().count_where(&|object| is_dormant(object))
}
