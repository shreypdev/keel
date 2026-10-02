//! ADR-037 end to end with real macro-generated stores: a snapshot an older build took restores
//! into this build with signals reordered, renamed types migrated by a `ty` hook, a signal added
//! with `#[undra(default)]`, an integer widened and a variant added in front; a signal whose type
//! changed is refused with `RestoreError::Incompatible`, unless a `#[undra::migrate]` hook converts
//! it; the audit's probe (`i32 -5` read as an `f32` NaN) cannot happen any more.

use std::sync::Arc;

use undra::meta::{Schema, TypeRef, ids};
use undra::prelude::*;
use undra::runtime::testing::TestRuntime;
use undra::runtime::{RestoreError, Runtime};
use undra::wire::payload::{CallTarget, ReplyStatus, Snapshot, SnapshotType, StoreSnapshot};
use undra::wire::{Decode, Encode, Reader, Writer};

// ----- this build ----------------------------------------------------------------------------

/// This build put `Team` in front of the old two.
#[undra::api]
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Tier {
    Team,
    #[default]
    Free,
    Pro,
}

/// The older build called `text` `body`: not structural, the `ty` hook below converts it.
#[undra::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub text: String,
    pub pinned: Option<bool>,
}

#[undra::store]
pub struct Profile {
    tier: Signal<Tier>,
    age: Signal<f32>,
    notes: Signal<Vec<Note>>,
    total: Signal<i64>,
    #[undra(default)]
    visits: Signal<u32>,
}

#[undra::api(store)]
impl Profile {
    #[allow(clippy::new_without_default)] // a constructor the platforms call
    pub fn new() -> Self {
        Profile {
            tier: Signal::new(Tier::Free),
            age: Signal::new(0.0),
            notes: Signal::new(Vec::new()),
            total: Signal::new(0),
            visits: Signal::new(1),
        }
    }
}

/// A store with no hook: its signal's type change refuses the restore.
#[undra::store]
pub struct Legacy {
    score: Signal<f32>,
}

#[undra::api(store)]
impl Legacy {
    #[allow(clippy::new_without_default)] // a constructor the platforms call
    pub fn new() -> Self {
        Legacy {
            score: Signal::new(0.0),
        }
    }
}

// ----- the hooks -----------------------------------------------------------------------------

/// The old `age` was an `i32`: a negative age is refused, a missing one is 18.
#[undra::migrate(store = "Profile", signal = "age")]
fn profile_age(old: Option<&DynValue>) -> Result<f32, MigrateError> {
    match old.and_then(DynValue::as_i64) {
        Some(age) if age >= 0 => Ok(age as f32),
        Some(age) => Err(MigrateError::new(format!("{age} is not an age"))),
        None => Ok(18.0),
    }
}

/// The old `Note { body }` is today's `Note { text }`.
#[undra::migrate(ty = "Note")]
fn note_from_body(old: &DynValue) -> Result<Note, MigrateError> {
    let text = old
        .field("body")
        .and_then(DynValue::as_str)
        .ok_or_else(|| MigrateError::new("a note without a body"))?;
    Ok(Note {
        text: text.to_owned(),
        pinned: None,
    })
}

// ----- the older build, described --------------------------------------------------------------

/// This build's schema, edited into the older one: `Profile { age: i32, tier: Tier, notes:
/// Vec<Note>, total: i32 }` (another order, no `visits`), `Tier { Free, Pro }`, `Note { body }`,
/// and `Legacy { score: i32 }`.
fn older(rt: &Runtime) -> Schema {
    let mut old = rt.schema().clone();
    let profile = old
        .objects
        .iter_mut()
        .find(|o| o.name == "Profile")
        .and_then(|o| o.store.as_mut())
        .unwrap();
    let signal = |name: &str, id: u32, ty: TypeRef| undra::meta::SignalDef {
        name: name.into(),
        signal_id: id,
        ty,
        computed: false,
        key: None,
        no_coalesce: false,
        default: false,
    };
    profile.signals = vec![
        signal("age", 0, TypeRef::I32),
        signal("tier", 1, TypeRef::named("Tier")),
        signal("notes", 2, TypeRef::vec(TypeRef::named("Note"))),
        signal("total", 3, TypeRef::I32),
    ];
    let tier = old.enums.iter_mut().find(|e| e.name == "Tier").unwrap();
    tier.variants.retain(|v| v.name != "Team");
    for (index, variant) in tier.variants.iter_mut().enumerate() {
        variant.index = u16::try_from(index).unwrap();
    }
    let note = old.records.iter_mut().find(|r| r.name == "Note").unwrap();
    note.fields = vec![undra::meta::FieldDef {
        name: "body".into(),
        ty: TypeRef::String,
        default: false,
        docs: String::new(),
    }];
    let legacy = old
        .objects
        .iter_mut()
        .find(|o| o.name == "Legacy")
        .and_then(|o| o.store.as_mut())
        .unwrap();
    legacy.signals = vec![signal("score", 0, TypeRef::I32)];
    old
}

/// One store of a snapshot: its handle, its type's name and its signals.
type OldStore<'a> = (Handle, &'a str, Vec<(u32, Vec<u8>)>);

/// A snapshot the older build would have taken of `stores`.
fn old_snapshot(rt: &Runtime, stores: Vec<OldStore<'_>>) -> Vec<u8> {
    let old = older(rt);
    let mut type_ids: Vec<u32> = Vec::new();
    for (_, name, _) in &stores {
        let id = ids::type_id(name);
        if !type_ids.contains(&id) {
            type_ids.push(id);
        }
    }
    let snapshot = Snapshot {
        generation_floor: 1,
        schema_hash: old.hash(),
        types: type_ids
            .iter()
            .map(|&type_id| SnapshotType {
                type_id,
                fingerprint: old.store_fingerprint(type_id).unwrap(),
            })
            .collect(),
        description: old.stores_closure(&type_ids).canonical_json(),
        stores: stores
            .into_iter()
            .map(|(handle, name, signals)| StoreSnapshot {
                handle,
                type_id: ids::type_id(name),
                signals,
            })
            .collect(),
    };
    let mut w = Writer::new();
    snapshot.encode(&mut w);
    w.into_vec()
}

fn construct(t: &TestRuntime, name: &str) -> Handle {
    let reply = t.call_sync(
        CallTarget::Constructor {
            type_id: ids::type_id(name),
            method_id: ids::method_id(name, "new"),
        },
        1,
        &[],
    );
    assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
    Handle(u64::decode_exact(&reply.body).unwrap())
}

fn enc<T: Encode>(value: &T) -> Vec<u8> {
    value.encode_to_vec()
}

fn profile(t: &TestRuntime, handle: Handle) -> Arc<Profile> {
    t.runtime().object::<Profile>(handle.0).unwrap()
}

// ----- the tests -------------------------------------------------------------------------------

#[test]
fn an_unchanged_store_takes_the_fast_path() {
    let t = TestRuntime::new();
    let h = construct(&t, "Profile");
    let snapshot = t.runtime().snapshot();
    let report = t.runtime().restore_with_report(&snapshot).unwrap();
    assert_eq!(report.restored, 1);
    assert!(report.migrated.is_empty() && !report.schema_changed);
    assert_eq!(profile(&t, h).visits.get(), 1);
}

#[test]
fn an_older_snapshot_restores_by_name_with_hooks_defaults_and_widenings() {
    let t = TestRuntime::new();
    let h = construct(&t, "Profile");
    // Old Tier::Pro was index 1; today it is index 2.
    let old_notes = vec![("milk".to_owned(),)];
    let bytes = old_snapshot(
        t.runtime(),
        vec![(
            h,
            "Profile",
            vec![
                (0, enc(&30_i32)),
                (1, enc(&1_u16)),
                (2, enc(&old_notes)),
                (3, enc(&-7_i32)),
            ],
        )],
    );
    let report = t.runtime().restore_with_report(&bytes).unwrap();
    assert_eq!(report.migrated, ["Profile"]);
    assert!(report.schema_changed);
    let p = profile(&t, h);
    assert_eq!(p.tier.get(), Tier::Pro, "by name, not by index");
    assert_eq!(p.age.get(), 30.0, "the signal hook");
    assert_eq!(
        p.notes.get(),
        [Note {
            text: "milk".into(),
            pinned: None
        }],
        "the ty hook, inside a list"
    );
    assert_eq!(p.total.get(), -7, "i32 widened to i64");
    assert_eq!(
        p.visits.get(),
        0,
        "added with #[undra(default)]: u32::default()"
    );
}

#[test]
fn a_hook_that_refuses_refuses_the_whole_restore_and_changes_nothing() {
    let t = TestRuntime::new();
    let h = construct(&t, "Profile");
    profile(&t, h).total.set(99);
    let bytes = old_snapshot(
        t.runtime(),
        vec![(
            h,
            "Profile",
            vec![
                (0, enc(&-5_i32)),
                (1, enc(&0_u16)),
                (2, enc(&Vec::<(String,)>::new())),
                (3, enc(&1_i32)),
            ],
        )],
    );
    match t.runtime().restore(&bytes) {
        Err(RestoreError::Incompatible {
            store,
            signal,
            reason,
            ..
        }) => {
            assert_eq!((store.as_str(), signal.as_str()), ("Profile", "age"));
            assert!(reason.contains("profile_age"), "{reason}");
            assert!(reason.contains("-5 is not an age"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(profile(&t, h).total.get(), 99, "all or nothing");
    assert!(
        t.host()
            .take_logs()
            .iter()
            .any(|l| l.level == undra::runtime::log::ERROR && l.message.contains("restore refused")),
        "the reason reaches the Log port"
    );
}

/// The audit's probe (ADR-037 context): `age: i32 = -5` restored into `age: f32` used to read
/// the bytes as an `f32` NaN and return `Ok`. Without a hook the restore is refused, typed.
#[test]
fn the_audit_probe_is_refused_with_the_typed_error() {
    let t = TestRuntime::new();
    let h = construct(&t, "Legacy");
    let bytes = old_snapshot(t.runtime(), vec![(h, "Legacy", vec![(0, enc(&-5_i32))])]);
    match t.runtime().restore(&bytes) {
        Err(RestoreError::Incompatible {
            store,
            signal,
            reason,
            type_id,
        }) => {
            assert_eq!(type_id, ids::type_id("Legacy"));
            assert_eq!((store.as_str(), signal.as_str()), ("Legacy", "score"));
            assert!(reason.contains("i32 cannot become f32"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_damaged_description_is_refused() {
    let t = TestRuntime::new();
    let h = construct(&t, "Legacy");
    let bytes = old_snapshot(t.runtime(), vec![(h, "Legacy", vec![(0, enc(&1_i32))])]);
    let mut snapshot = Snapshot::decode(&mut Reader::new(&bytes)).unwrap();
    snapshot.description = "{not json".into();
    let mut w = Writer::new();
    snapshot.encode(&mut w);
    assert!(matches!(
        t.runtime().restore(w.as_slice()),
        Err(RestoreError::Incompatible { reason, .. }) if reason.contains("does not parse")
    ));
}

#[test]
fn the_hooks_are_registered_and_their_targets_exist() {
    let t = TestRuntime::new();
    let hooks: Vec<&str> = undra::persist::migrations().map(|m| m.name).collect();
    assert!(
        hooks.iter().any(|n| n.ends_with("::profile_age")),
        "{hooks:?}"
    );
    assert!(
        hooks.iter().any(|n| n.ends_with("::note_from_body")),
        "{hooks:?}"
    );
    assert!(undra::persist::check_migrations(t.runtime().schema()).is_empty());
}

/// Whether a restore's failure is one a host reads as "bad snapshot" (5) or "incompatible" (7):
/// never a panic (2) and never "unavailable" (6) for a running core.
fn refused_typed(error: &RestoreError) -> bool {
    matches!(
        error,
        RestoreError::Decode(_)
            | RestoreError::UnknownStoreType { .. }
            | RestoreError::Store { .. }
            | RestoreError::BadHandle { .. }
            | RestoreError::GenerationFloor { .. }
            | RestoreError::Incompatible { .. }
    )
}

/// A damaged snapshot, truncated at every length and with every byte changed, on both paths (the
/// fast one and the migrating one): the restore never panics, a refusal is code 5 or 7, and a
/// refused restore leaves the core byte for byte as it was (ADR-023's all or nothing).
#[test]
fn a_damaged_snapshot_is_refused_typed_at_every_byte_and_changes_nothing() {
    let t = TestRuntime::new();
    let h = construct(&t, "Profile");
    let l = construct(&t, "Legacy");
    let p = profile(&t, h);
    p.total.set(41);
    p.notes.set(vec![Note {
        text: "é".into(),
        pinned: Some(true),
    }]);
    let current = t.runtime().snapshot();
    let older = old_snapshot(
        t.runtime(),
        vec![
            (
                h,
                "Profile",
                vec![
                    (0, enc(&30_i32)),
                    (1, enc(&1_u16)),
                    (2, enc(&vec![("milk".to_owned(),)])),
                    (3, enc(&-7_i32)),
                ],
            ),
            (l, "Legacy", vec![]),
        ],
    );
    // `Legacy` lost its only signal in that snapshot and has no default: drop it from the older one
    // so the original restores, and keep the migrating path for `Profile`.
    let older = {
        let mut s = Snapshot::decode(&mut Reader::new(&older)).unwrap();
        s.stores.retain(|st| st.type_id == ids::type_id("Profile"));
        s.types.retain(|ty| ty.type_id == ids::type_id("Profile"));
        let mut w = Writer::new();
        s.encode(&mut w);
        w.into_vec()
    };
    for original in [&current, &older] {
        t.runtime()
            .restore(original)
            .expect("the undamaged snapshot restores");
        let mut baseline = t.runtime().snapshot();
        let mut tried = 0_u32;
        let mut check = |damaged: &[u8], what: &str| {
            tried += 1;
            match t.runtime().restore(damaged) {
                Ok(()) => {
                    // A change that still decodes (a value byte, a higher floor): put the original
                    // back. The generation counter never goes down (ADR-022), so the baseline is
                    // taken again.
                    t.runtime().restore(original).unwrap();
                    baseline = t.runtime().snapshot();
                }
                Err(error) => {
                    assert!(refused_typed(&error), "{what}: {error:?}");
                    assert_eq!(
                        t.runtime().snapshot(),
                        baseline,
                        "{what}: a refused restore changed the core ({error})"
                    );
                }
            }
        };
        for cut in 0..original.len() {
            check(&original[..cut], &format!("cut at {cut}"));
        }
        for at in 0..original.len() {
            for change in [0xff_u8, 0x01, 0x80] {
                let mut damaged = original.clone();
                damaged[at] ^= change;
                check(&damaged, &format!("byte {at} ^ {change:#x}"));
            }
            let mut damaged = original.clone();
            damaged[at] = 0xff;
            check(&damaged, &format!("byte {at} = 0xff"));
        }
        assert!(tried > 100);
    }
}
