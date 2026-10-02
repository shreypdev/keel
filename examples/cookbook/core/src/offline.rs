//! Offline-first: queries that persist, writes that queue, an outbox the UI can show, and an
//! update that changes the data the last build left on the device.
//!
//! * [`notes`] is a persisted query: the list is on the screen from the first frame of the next
//!   launch, from the `Kv` port, and refreshed in the background.
//! * [`add_note`] is an idempotent mutation: offline, it goes into a persisted queue and is replayed
//!   in order when the network returns, even after the app was killed in between. The server sees the
//!   same `Idempotency-Key` on every replay.
//! * [`create_note`] is what a UI calls: the note shows at once (an optimistic placeholder) and
//!   gives way to the server's note when the write lands, or goes away if the server refuses it.
//! * [`outbox`] says what is waiting and what could not be sent (the dead letters).
//! * **Shipping an update.** This is build 2 of the app. Build 1 called the note's text `body` and
//!   had no `pinned`. A device that updates still holds build-1 notes in its cache and a build-1
//!   `add_note` in its queue. `pinned` is `#[undra(default)]`, so old notes read it as `false` with
//!   no code; the rename needs the two hooks below. Anything a hook refuses is never lost: a queued
//!   write becomes a dead letter that [`outbox`] lists.

use serde::Deserialize;
use undra::ports::HttpRequest;
use undra::prelude::*;

use crate::net::{self, NetError};

// docs:begin offline-update
/// A note. Build 1 had `{ id, body }`; build 2 renamed the text and added `pinned`.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Note {
    /// The server's identity of the note. A note that is only shown optimistically, and that the
    /// server has not answered for yet, has an identity counting down from `u32::MAX`.
    pub id: u32,
    /// What the note says.
    pub text: String,
    /// Whether it is pinned to the top. Added in build 2: build-1 data reads it as `false`.
    #[undra(default)]
    #[serde(default)]
    pub pinned: bool,
}
// docs:end

// docs:begin offline-queue
/// The notes of `list`: `GET /lists/{list}/notes`. Persisted, so the next launch shows them before
/// the network answers.
#[undra::query(key = "notes:{list}", stale = "30s", persist, retry = 1)]
pub async fn notes(ctx: &Ctx, list: String) -> Result<Vec<Note>, NetError> {
    let url = net::url(ctx, &format!("/lists/{list}/notes"))?;
    net::json(&net::ok(net::send(ctx, HttpRequest::get(url)).await?)?)
}
// docs:end

// docs:begin offline-queue
/// Adds a note to `list`: `POST /lists/{list}/notes`. Idempotent: the key stays the same across
/// retries and offline replays. `pinned` is build 2's; a queued build-1 call has none (`None`).
#[undra::mutation(key = "notes:{list}", idempotent)]
pub async fn add_note(
    ctx: &Ctx,
    list: String,
    text: String,
    pinned: Option<bool>,
) -> Result<Note, NetError> {
    let url = net::url(ctx, &format!("/lists/{list}/notes"))?;
    let body = serde_json::json!({ "text": text, "pinned": pinned.unwrap_or(false) });
    let mut request = net::post_json(url, &body);
    if let Some(key) = undra::query::idempotency_key() {
        request = request.with_header("Idempotency-Key", key.to_string());
    }
    net::json(&net::ok(net::send(ctx, request).await?)?)
}
// docs:end

// docs:begin offline-queue
/// Adds a note the way a UI wants it: it shows at once, the server is asked, and the placeholder is
/// taken back if the server refuses. Offline, the call keeps waiting and the placeholder stays
/// until the queue replays the write.
#[undra::api]
pub async fn create_note(
    ctx: &Ctx,
    list: String,
    text: String,
    pinned: bool,
) -> Result<Note, NetError> {
    let placeholder = Note {
        id: u32::MAX,
        text: text.clone(),
        pinned,
    };
    let cached = list.clone();
    ctx.mutate::<AddNoteMutation>((list, text, Some(pinned)))
        .optimistic(move |cache| {
            cache.update::<NotesQuery>((cached,), |notes| notes.push(placeholder));
        })
        .await
}
// docs:end

/// A write that could not be sent, and why.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stuck {
    /// The key its replays carry; it identifies the dead letter.
    pub id: Uuid,
    /// The mutation's name.
    pub mutation: String,
    /// Why it could not be migrated to this build.
    pub reason: String,
}

/// What is waiting to be sent.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outbox {
    /// Writes waiting in the offline queue.
    pub pending: u32,
    /// Writes an update could not carry over. They are kept until the user (or the app) settles them.
    pub stuck: Vec<Stuck>,
}

/// What is waiting to be sent. Call it when the screen appears and after a write.
#[undra::api]
pub fn outbox(ctx: &Ctx) -> Outbox {
    let query = ctx.query();
    Outbox {
        pending: u32::try_from(query.pending_mutations()).unwrap_or(u32::MAX),
        stuck: query
            .dead_letters()
            .into_iter()
            .map(|letter| Stuck {
                id: letter.idempotency_key,
                mutation: letter.mutation,
                reason: letter.reason,
            })
            .collect(),
    }
}

/// Tries a stuck write again (after an update that added a hook for it); `true` if it went back
/// into the queue.
#[undra::api]
pub fn retry_stuck(ctx: &Ctx, id: Uuid) -> bool {
    ctx.query().retry_dead_letter(id).is_ok()
}

/// Gives a stuck write up for good; `true` if there was one.
#[undra::api]
pub fn discard_stuck(ctx: &Ctx, id: Uuid) -> bool {
    ctx.query().discard_dead_letter(id)
}

// docs:begin offline-update
/// Build 1's notes (`{ id, body }`) become build 2's. Structural migration cannot do this one, a
/// rename is a removal and an addition to it, so the hook reads the old name.
#[undra::migrate(ty = "Note")]
fn note_from_build_1(old: &DynValue) -> Result<Note, MigrateError> {
    let id = old
        .field("id")
        .and_then(DynValue::as_i64)
        .and_then(|id| u32::try_from(id).ok())
        .ok_or_else(|| MigrateError::new("a note without an id"))?;
    let text = old
        .field("text")
        .or_else(|| old.field("body"))
        .and_then(DynValue::as_str)
        .ok_or_else(|| MigrateError::new("a note without text"))?;
    let pinned = old.field("pinned").and_then(DynValue::as_bool);
    Ok(Note {
        id,
        text: text.to_owned(),
        pinned: pinned.unwrap_or(false),
    })
}
// docs:end

// docs:begin offline-update
/// A queued build-1 `add_note(list, body)` becomes build 2's `add_note(list, text, pinned)`:
/// `pinned` is an `Option` and fills itself with `None`; the renamed parameter is the hook's job.
#[undra::migrate(mutation = "add_note")]
fn add_note_from_build_1(old: &DynRecord) -> Result<DynRecord, MigrateError> {
    let mut new = old.clone();
    new.rename("body", "text");
    Ok(new)
}
// docs:end

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use undra::ports::fakes::Matcher;
    use undra::ports::{HttpError, HttpMethod, HttpResponse, NetKind};
    use undra::query::QueryStatus;
    use undra::runtime::testing::TestRuntime;

    use super::*;
    use crate::net::testing::{App, BASE, json_response};

    fn note(id: u32, text: &str) -> Note {
        Note {
            id,
            text: text.into(),
            pinned: false,
        }
    }

    fn note_json(n: &Note) -> serde_json::Value {
        serde_json::json!({"id": n.id, "text": n.text, "pinned": n.pinned})
    }

    fn list_url() -> String {
        format!("{BASE}/lists/inbox/notes")
    }

    fn serve(app: &App, notes: &[Note]) {
        app.fakes.http.respond(
            Matcher::get(list_url()),
            json_response(
                200,
                serde_json::Value::Array(notes.iter().map(note_json).collect()),
            ),
        );
    }

    #[test]
    fn the_list_is_cached_and_persisted() {
        let app = App::new();
        serve(&app, &[note(1, "milk")]);
        let handle = app
            .ctx()
            .query()
            .observe::<NotesQuery>(("inbox".to_owned(),));
        app.t.run_pending();
        assert_eq!(handle.status().get(), QueryStatus::Success);
        assert_eq!(handle.data().get(), Some(vec![note(1, "milk")]));
        app.advance(500); // the entry is written to the Kv port a moment after the fetch
        assert!(
            app.fakes
                .kv
                .keys()
                .iter()
                .any(|k| k.starts_with("undra.query.cache2.")),
            "{:?}",
            app.fakes.kv.keys()
        );
    }

    #[test]
    fn a_note_created_offline_shows_at_once_and_is_sent_when_the_network_returns() {
        let app = App::new();
        serve(&app, &[note(1, "milk")]);
        let handle = app
            .ctx()
            .query()
            .observe::<NotesQuery>(("inbox".to_owned(),));
        app.t.run_pending();
        app.fakes.connectivity.go_offline();
        app.t.run_pending();
        app.fakes.http.fail(
            Matcher::post(list_url()),
            HttpError::Network("offline".into()),
        );

        let result = Arc::new(Mutex::new(None));
        let slot = result.clone();
        app.ctx().spawn(async move {
            *slot.lock().unwrap() =
                Some(create_note(&Ctx::current(), "inbox".into(), "eggs".into(), true).await);
        });
        app.t.run_pending();
        assert!(result.lock().unwrap().is_none(), "the caller keeps waiting");
        assert_eq!(
            handle.data().get().map(|l| l.len()),
            Some(2),
            "the placeholder shows"
        );
        assert_eq!(
            outbox(&app.ctx()),
            Outbox {
                pending: 1,
                stuck: vec![]
            }
        );

        app.fakes.http.reset();
        app.fakes.http.respond(
            Matcher::post(list_url()),
            json_response(
                201,
                note_json(&Note {
                    id: 2,
                    text: "eggs".into(),
                    pinned: true,
                }),
            ),
        );
        serve(
            &app,
            &[
                note(1, "milk"),
                Note {
                    id: 2,
                    text: "eggs".into(),
                    pinned: true,
                },
            ],
        );
        app.fakes.connectivity.go_online(NetKind::Wifi);
        app.t.run_pending();
        app.advance(2_000);
        assert_eq!(
            result.lock().unwrap().clone(),
            Some(Ok(Note {
                id: 2,
                text: "eggs".into(),
                pinned: true
            }))
        );
        assert_eq!(handle.data().get().map(|l| l.len()), Some(2));
        assert_eq!(outbox(&app.ctx()).pending, 0);
    }

    #[test]
    fn a_write_made_offline_survives_the_app_being_killed() {
        // Run 1: offline, the write is queued (and persisted), then the process ends.
        let first = App::new();
        first.fakes.connectivity.go_offline();
        first.t.run_pending();
        first.fakes.http.fail(
            Matcher::post(list_url()),
            HttpError::Network("offline".into()),
        );
        first.ctx().spawn(async {
            let _ = create_note(&Ctx::current(), "inbox".into(), "eggs".into(), false).await;
        });
        first.t.run_pending();
        let key = first.fakes.http.calls()[0]
            .header("Idempotency-Key")
            .unwrap()
            .to_owned();
        let fakes = first.fakes.clone();
        drop(first);

        // Run 2: a new runtime over the same storage. The platform says it is offline, then online.
        fakes.http.reset();
        let t = TestRuntime::new();
        fakes.install_test(&t);
        t.run_init_hooks();
        crate::net::configure_server(
            &t.ctx(),
            crate::net::ServerConfig {
                base_url: BASE.into(),
            },
        );
        fakes.connectivity.go_offline();
        t.run_pending();
        assert_eq!(outbox(&t.ctx()).pending, 1, "the queue came back from disk");
        fakes.http.respond(
            Matcher::post(list_url()),
            json_response(201, note_json(&note(7, "eggs"))),
        );
        fakes.connectivity.go_online(NetKind::Wifi);
        t.run_pending();
        fakes.advance(&t, Duration::from_secs(2));
        assert_eq!(outbox(&t.ctx()).pending, 0);
        let replay = fakes
            .http
            .calls()
            .into_iter()
            .find(|r| r.method == HttpMethod::Post)
            .unwrap();
        assert_eq!(
            replay.header("Idempotency-Key"),
            Some(key.as_str()),
            "the server can tell a repeat"
        );
    }

    #[test]
    fn a_refused_write_takes_its_placeholder_back() {
        let app = App::new();
        serve(&app, &[note(1, "milk")]);
        let handle = app
            .ctx()
            .query()
            .observe::<NotesQuery>(("inbox".to_owned(),));
        app.t.run_pending();
        app.fakes.http.respond(
            Matcher::post(list_url()),
            HttpResponse::new(422, b"no".to_vec()),
        );
        let ctx = app.ctx();
        let result =
            app.run(async move { create_note(&ctx, "inbox".into(), "x".into(), false).await });
        assert_eq!(result, Err(NetError::Status { code: 422 }));
        assert_eq!(handle.data().get(), Some(vec![note(1, "milk")]));
    }

    #[test]
    fn build_1_notes_become_build_2_notes() {
        let v1 = DynValue::Record(
            DynRecord::new("Note")
                .with("id", DynValue::Int(7))
                .with("body", DynValue::String("milk".into())),
        );
        assert_eq!(note_from_build_1(&v1), Ok(note(7, "milk")));
        // Already build 2: the hook reads the new name too.
        let v2 = DynValue::Record(
            DynRecord::new("Note")
                .with("id", DynValue::Int(8))
                .with("text", DynValue::String("eggs".into()))
                .with("pinned", DynValue::Bool(true)),
        );
        assert_eq!(
            note_from_build_1(&v2),
            Ok(Note {
                id: 8,
                text: "eggs".into(),
                pinned: true
            })
        );
        // Something that is not a note is refused, and the caller keeps it (a dead letter, a dropped cache entry).
        let broken = DynValue::Record(DynRecord::new("Note").with("id", DynValue::Int(-1)));
        assert!(note_from_build_1(&broken).is_err());
        assert!(
            note_from_build_1(&DynValue::Record(
                DynRecord::new("Note").with("id", DynValue::Int(1))
            ))
            .is_err()
        );
    }

    #[test]
    fn a_queued_build_1_write_is_renamed_to_build_2s_parameters() {
        let queued = DynRecord::new("add_note")
            .with("list", DynValue::String("inbox".into()))
            .with("body", DynValue::String("milk".into()));
        let migrated = add_note_from_build_1(&queued).unwrap();
        assert_eq!(migrated.get("text"), Some(&DynValue::String("milk".into())));
        assert_eq!(migrated.get("body"), None);
        assert_eq!(
            migrated.get("list"),
            Some(&DynValue::String("inbox".into()))
        );
    }

    #[test]
    fn the_outbox_starts_empty_and_a_stuck_write_that_does_not_exist_is_not_found() {
        let app = App::new();
        assert_eq!(
            outbox(&app.ctx()),
            Outbox {
                pending: 0,
                stuck: vec![]
            }
        );
        assert!(!retry_stuck(&app.ctx(), Uuid([9; 16])));
        assert!(!discard_stuck(&app.ctx(), Uuid([9; 16])));
    }

    #[test]
    fn signing_out_is_refused_while_writes_wait_in_the_queue() {
        use crate::auth::{Auth, AuthError};
        let app = App::new();
        let auth = Auth::new(app.ctx());
        app.fakes.connectivity.go_offline();
        app.t.run_pending();
        app.fakes.http.fail(
            Matcher::post(list_url()),
            HttpError::Network("offline".into()),
        );
        app.ctx().spawn(async {
            let _ = create_note(&Ctx::current(), "inbox".into(), "eggs".into(), false).await;
        });
        app.t.run_pending();
        assert_eq!(
            app.run(auth.sign_out()),
            Err(AuthError::PendingWrites { count: 1 })
        );
    }
}
