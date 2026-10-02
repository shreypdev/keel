//! The notebook: field notes with photos, kept on the device first and sent to the team's server when
//! there is a network.
//!
//! * **Local first.** A write is durable (`Kv` port) before the UI shows it, and the screen never waits
//!   for the network. The notes live in one keyed list, so a new note is a one-row patch for the three
//!   UIs however many notes there are.
//! * **Derived.** `visible` is the list the screen draws: filtered by the search text and the tag,
//!   pinned notes first, newest first (a [`DerivedList`], ADR-039, kept from the list's recorded
//!   operations). `tags` is the set of tags in use, for the chips.
//! * **Offline.** Every change is also a [`push_note`] mutation: idempotent, so offline it waits in the
//!   persisted queue and is replayed when the network returns, even after the app was killed.
//!   `pending` is how many are waiting.
//! * **Photos.** A photo is written to the `Fs` port and uploaded by a queued mutation, like a note.
//! * **Updates.** Build 1 called the text `text` and had no tags, pins or photos. Build 2's `Note` adds
//!   them with `#[undra(default)]`, and `note_from_build_1` reads the old name from a snapshot, a
//!   cached list or a queued write. The notes the app stores itself (`Kv`) are JSON, which reads the old
//!   name through a serde alias: two kinds of data survive an update, and each has its own rule.

use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};
use undra::ports::{FsError, HttpMethod, StorageError};
use undra::prelude::*;
use undra::runtime::Subscription;

use crate::auth::{AuthError, authed};
use crate::net;

/// The longest title, in characters.
pub const MAX_TITLE: u32 = 120;
/// Where a note is kept in the `Kv` port: `fieldbook.note.<id, zero padded>`.
const KEY_PREFIX: &str = "fieldbook.note.";

// docs:begin fieldbook-note
/// One field note. Build 2's shape: build 1 had `text` where this has `body`, and no tag, pin or photos.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    /// Identity of the note; the list is updated by key, so the UI diffs by it.
    pub id: u32,
    /// The headline.
    pub title: String,
    /// What was seen.
    pub body: String,
    /// A label the notes are grouped by (lower case; empty for none). Added in build 2.
    #[undra(default)]
    pub tag: String,
    /// Whether the note is pinned to the top. Added in build 2.
    #[undra(default)]
    pub pinned: bool,
    /// When it was written.
    pub created: Timestamp,
    /// Photos, as paths of the `Fs` port; `read_photo` returns the bytes. Added in build 2.
    #[undra(default)]
    pub photos: Vec<String>,
}
// docs:end

/// What the notebook shows: the search text and the tag.
#[undra::api]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    /// Notes whose title or body contains this (any case); empty for all.
    pub query: String,
    /// Notes with exactly this tag; empty for all.
    pub tag: String,
}

/// Why a notebook call failed.
#[undra::error]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoteError {
    /// The title is empty once spaces are trimmed.
    #[error("the title cannot be empty")]
    EmptyTitle,
    /// The title is longer than the maximum (120 characters).
    #[error("the title is longer than {max} characters")]
    TitleTooLong {
        /// The limit.
        max: u32,
    },
    /// There is no note with this id.
    #[error("no such note")]
    NotFound,
    /// The `Kv` port failed: the note was not saved.
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
    /// The `Fs` port failed: the photo was not saved.
    #[error("file: {0}")]
    File(#[from] FsError),
    /// The core is shutting down.
    #[error("the core is shutting down")]
    Closed,
}

/// A note as the app stores it and as the server takes it: JSON, which can grow fields.
#[derive(Serialize, Deserialize)]
struct Stored {
    id: u32,
    title: String,
    /// Build 1 wrote this as `text`.
    #[serde(alias = "text")]
    body: String,
    #[serde(default)]
    tag: String,
    #[serde(default)]
    pinned: bool,
    created_ms: i64,
    #[serde(default)]
    photos: Vec<String>,
}

impl From<&Note> for Stored {
    fn from(note: &Note) -> Stored {
        Stored {
            id: note.id,
            title: note.title.clone(),
            body: note.body.clone(),
            tag: note.tag.clone(),
            pinned: note.pinned,
            created_ms: note.created.0,
            photos: note.photos.clone(),
        }
    }
}

impl From<Stored> for Note {
    fn from(stored: Stored) -> Note {
        Note {
            id: stored.id,
            title: stored.title,
            body: stored.body,
            tag: stored.tag,
            pinned: stored.pinned,
            created: Timestamp(stored.created_ms),
            photos: stored.photos,
        }
    }
}

fn key_of(id: u32) -> String {
    format!("{KEY_PREFIX}{id:08}")
}

// docs:begin fieldbook-push
/// Sends a note to the team's server: `PUT /notes/{id}`. Idempotent, so offline it waits in the
/// persisted queue and is replayed, with the same `Idempotency-Key`, when the network returns.
#[undra::mutation(key = "notes", idempotent)]
pub async fn push_note(ctx: &Ctx, note: Note) -> Result<(), AuthError> {
    let url = net::url(ctx, &format!("/notes/{}", note.id))?;
    let body = serde_json::to_value(Stored::from(&note)).unwrap_or_default();
    let mut request = net::json_request(HttpMethod::Put, url, &body);
    if let Some(key) = undra::query::idempotency_key() {
        request = request.with_header("Idempotency-Key", key.to_string());
    }
    net::ok(authed(ctx, request).await?)?;
    Ok(())
}
// docs:end

/// Uploads one photo of a note: `PUT /notes/{id}/photos/{index}`. Queued like a note.
#[undra::mutation(key = "photos", idempotent)]
pub async fn push_photo(ctx: &Ctx, note: u32, index: u32, data: Bytes) -> Result<(), AuthError> {
    let url = net::url(ctx, &format!("/notes/{note}/photos/{index}"))?;
    let mut request = undra::ports::HttpRequest::new(HttpMethod::Put, url).with_body(data.0);
    if let Some(key) = undra::query::idempotency_key() {
        request = request.with_header("Idempotency-Key", key.to_string());
    }
    net::ok(authed(ctx, request).await?)?;
    Ok(())
}

/// Deletes a note on the server: `DELETE /notes/{id}`. Queued like a note.
#[undra::mutation(key = "notes", idempotent)]
pub async fn delete_remote_note(ctx: &Ctx, id: u32) -> Result<(), AuthError> {
    let url = net::url(ctx, &format!("/notes/{id}"))?;
    let mut request = undra::ports::HttpRequest::new(HttpMethod::Delete, url);
    if let Some(key) = undra::query::idempotency_key() {
        request = request.with_header("Idempotency-Key", key.to_string());
    }
    // A note the server never saw is already gone.
    let response = authed(ctx, request).await?;
    if response.status != 404 {
        net::ok(response)?;
    }
    Ok(())
}

/// The bytes of a photo of a note (`Note::photos` holds the paths): what the UI draws.
#[undra::api]
pub async fn read_photo(ctx: &Ctx, path: String) -> Result<Bytes, FsError> {
    ctx.fs().read(path).await
}

/// Records how many writes are waiting, if it changed (every write is a change-set to the platforms).
fn refresh_pending(ctx: &Ctx, pending: &Signal<u32>) {
    let waiting = u32::try_from(ctx.query().pending_mutations()).unwrap_or(u32::MAX);
    if pending.get() != waiting {
        pending.set(waiting);
    }
}

/// Counts the waiting writes again after `delay`, from a task that holds the runtime weakly (ADR-034).
fn refresh_pending_later(ctx: &Ctx, pending: &Signal<u32>, delay: Duration) {
    let (weak, pending) = (ctx.downgrade(), pending.clone());
    ctx.spawn(async move {
        if weak.sleep(delay).await.is_ok() {
            if let Ok(ctx) = weak.upgrade() {
                refresh_pending(&ctx, &pending);
            }
        }
    });
}

/// The notebook's store: what the screens observe and call.
#[undra::store(restore = "Self::assemble")]
pub struct Notebook {
    ctx: WeakCtx,
    next: AtomicU32,
    /// Counts the waiting writes again when the network comes back; dropped with the store.
    _network: Subscription,
    #[undra(key = "id")]
    notes: Signal<Vec<Note>>,
    filter: Signal<Filter>,
    /// How many writes (notes, photos, deletions) are waiting in the offline queue.
    pending: Signal<u32>,
    #[undra(key = "id")]
    visible: DerivedList<Note>,
    tags: Computed<Vec<String>>,
}

#[undra::api(store)]
impl Notebook {
    /// An empty notebook. Call `load` at launch to read what the device holds.
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(
            ctx,
            Signal::new(vec![]),
            Signal::new(Filter::default()),
            Signal::new(0),
        )
    }

    // Used by `new` and, through `restore = ".."`, to rebuild the store from a snapshot (which is how
    // `undra dev` keeps the state across a rebuild).
    fn assemble(
        ctx: Ctx,
        notes: Signal<Vec<Note>>,
        filter: Signal<Filter>,
        pending: Signal<u32>,
    ) -> Self {
        // docs:begin fieldbook-derived
        let visible = notes
            .derive()
            .filter_with(&filter, |filter, note: &Note| matches(filter, note))
            // Pinned first, then the newest: a sort key, kept from the list's operations.
            .sort_by_key(|note| {
                (
                    std::cmp::Reverse(note.pinned),
                    std::cmp::Reverse(note.created),
                )
            })
            .build();
        let tags = Computed::new(&notes, |notes| {
            let mut tags: Vec<String> = notes
                .iter()
                .filter(|note| !note.tag.is_empty())
                .map(|note| note.tag.clone())
                .collect();
            tags.sort();
            tags.dedup();
            tags
        });
        // docs:end
        let next = notes.with(|list| list.iter().map(|note| note.id).max().unwrap_or(0));
        let weak = ctx.downgrade();
        let counter = pending.clone();
        let network = undra::ports::on_connectivity_changed(&ctx, move |ctx, online, _kind| {
            if online {
                // The queue replays when the network returns; count what is left once it had a moment.
                refresh_pending_later(ctx, &counter, Duration::from_secs(1));
            }
        });
        Self {
            ctx: weak,
            next: AtomicU32::new(next + 1),
            _network: network,
            notes,
            filter,
            pending,
            visible,
            tags,
        }
    }

    /// Reads the notes the device holds. Call it once at launch; a note that cannot be read is
    /// skipped and logged, never the whole notebook. Returns how many notes are there.
    pub async fn load(&self) -> Result<u32, NoteError> {
        let ctx = self.ctx.upgrade().map_err(|_| NoteError::Closed)?;
        let kv = ctx.kv();
        let mut notes = Vec::new();
        let mut highest = 0;
        for key in kv.list(KEY_PREFIX.to_owned()).await? {
            // An id is taken even when its note cannot be read, so a new note never overwrites it.
            let id = key
                .strip_prefix(KEY_PREFIX)
                .and_then(|id| id.parse::<u32>().ok());
            highest = highest.max(id.unwrap_or(0));
            let Some(bytes) = kv.get(key.clone()).await? else {
                continue;
            };
            match serde_json::from_slice::<Stored>(&bytes.0) {
                Ok(stored) => notes.push(Note::from(stored)),
                Err(error) => {
                    ctx.log()
                        .log(3, "fieldbook".to_owned(), format!("{key} skipped: {error}"))
                }
            }
        }
        notes.sort_by_key(|note| note.id);
        let count = u32::try_from(notes.len()).unwrap_or(u32::MAX);
        self.next.fetch_max(highest + 1, Ordering::Relaxed);
        // One raw write at launch: the list crosses whole once, then every change is a patch.
        self.notes.set(notes);
        refresh_pending(&ctx, &self.pending);
        Ok(count)
    }

    // docs:begin fieldbook-add
    /// Adds a note. It is saved on the device before this returns and sent to the server in the
    /// background: offline, the write waits in the queue.
    pub async fn add(&self, title: String, body: String, tag: String) -> Result<Note, NoteError> {
        let ctx = self.ctx.upgrade().map_err(|_| NoteError::Closed)?;
        let title = check_title(&title)?;
        let note = Note {
            id: self.next.fetch_add(1, Ordering::Relaxed),
            title,
            body,
            tag: tag.trim().to_lowercase(),
            pinned: false,
            created: Timestamp(ctx.clock().now_ms()),
            photos: vec![],
        };
        save(&ctx, &note).await?; // durable first: if the device cannot keep it, the UI does not show it
        self.notes.push(note.clone());
        self.push(&ctx, note.clone());
        Ok(note)
    }
    // docs:end

    /// Changes a note's title, text and tag.
    pub async fn edit(
        &self,
        id: u32,
        title: String,
        body: String,
        tag: String,
    ) -> Result<Note, NoteError> {
        let title = check_title(&title)?;
        self.change(id, |note| {
            note.title = title;
            note.body = body;
            note.tag = tag.trim().to_lowercase();
        })
        .await
    }

    /// Pins the note to the top of the list, or unpins it.
    pub async fn toggle_pin(&self, id: u32) -> Result<(), NoteError> {
        self.change(id, |note| note.pinned = !note.pinned).await?;
        Ok(())
    }

    /// Deletes a note on the device now and on the server when there is a network.
    pub async fn remove(&self, id: u32) -> Result<(), NoteError> {
        let ctx = self.ctx.upgrade().map_err(|_| NoteError::Closed)?;
        let at = self.position(id).ok_or(NoteError::NotFound)?;
        ctx.kv().delete(key_of(id)).await?;
        self.notes.remove(at);
        let pending = self.pending.clone();
        let weak = ctx.downgrade();
        ctx.spawn(async move {
            let Ok(ctx) = weak.upgrade() else { return };
            let _ = ctx.mutate::<DeleteRemoteNoteMutation>((id,)).await;
            refresh_pending(&ctx, &pending);
        });
        refresh_pending_later(&ctx, &self.pending, Duration::from_millis(250));
        Ok(())
    }

    /// Adds a photo to a note: the bytes are written to the `Fs` port and queued for upload. Returns
    /// the path, which `read_photo` reads back.
    pub async fn attach_photo(&self, id: u32, photo: Bytes) -> Result<String, NoteError> {
        let ctx = self.ctx.upgrade().map_err(|_| NoteError::Closed)?;
        let note = self.get(id).ok_or(NoteError::NotFound)?;
        let index = u32::try_from(note.photos.len()).unwrap_or(u32::MAX);
        let path = format!("photos/{id}/{index}.jpg");
        ctx.fs().write(path.clone(), photo.clone()).await?;
        let saved = path.clone();
        self.change(id, move |note| note.photos.push(saved)).await?;
        let pending = self.pending.clone();
        let weak = ctx.downgrade();
        ctx.spawn(async move {
            let Ok(ctx) = weak.upgrade() else { return };
            let _ = ctx.mutate::<PushPhotoMutation>((id, index, photo)).await;
            refresh_pending(&ctx, &pending);
        });
        Ok(path)
    }

    /// Sends every note again (the server takes a note it has as a no-op): the "sync now" button, and
    /// what to call after signing in on a device that has notes from before. Returns how many.
    pub fn sync_all(&self) -> u32 {
        let Ok(ctx) = self.ctx.upgrade() else {
            return 0;
        };
        let notes = self.notes.get();
        let count = u32::try_from(notes.len()).unwrap_or(u32::MAX);
        for note in notes {
            self.push(&ctx, note);
        }
        count
    }

    /// Shows the notes whose title or text contains `query` (any case); empty for all.
    pub fn set_query(&self, query: String) {
        self.filter.update(|filter| filter.query = query);
    }

    /// Shows the notes with this tag; empty for all.
    pub fn set_tag(&self, tag: String) {
        self.filter.update(|filter| filter.tag = tag);
    }

    fn get(&self, id: u32) -> Option<Note> {
        self.notes
            .with(|notes| notes.iter().find(|note| note.id == id).cloned())
    }

    fn position(&self, id: u32) -> Option<usize> {
        self.notes
            .with(|notes| notes.iter().position(|note| note.id == id))
    }

    /// Applies `edit` to a note: saved first, then one `Update` patch, then queued for the server.
    async fn change(&self, id: u32, edit: impl FnOnce(&mut Note)) -> Result<Note, NoteError> {
        let ctx = self.ctx.upgrade().map_err(|_| NoteError::Closed)?;
        let at = self.position(id).ok_or(NoteError::NotFound)?;
        let mut note = self.notes.with(|notes| notes[at].clone());
        edit(&mut note);
        save(&ctx, &note).await?;
        let changed = note.clone();
        self.notes.update_at(at, move |slot| *slot = changed);
        self.push(&ctx, note.clone());
        Ok(note)
    }

    /// Queues the note for the server, from a task: the caller does not wait for the network.
    fn push(&self, ctx: &Ctx, note: Note) {
        let pending = self.pending.clone();
        let weak = ctx.downgrade();
        ctx.spawn(async move {
            let Ok(ctx) = weak.upgrade() else { return };
            // Online this is one request. Offline it goes into the persisted queue and this future
            // keeps waiting; after a restart the queue replays by itself and nothing here is needed.
            let _ = ctx.mutate::<PushNoteMutation>((note,)).await;
            refresh_pending(&ctx, &pending);
        });
        refresh_pending_later(ctx, &self.pending, Duration::from_millis(250));
    }
}

fn check_title(title: &str) -> Result<String, NoteError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(NoteError::EmptyTitle);
    }
    if title.chars().count() > MAX_TITLE as usize {
        return Err(NoteError::TitleTooLong { max: MAX_TITLE });
    }
    Ok(title.to_owned())
}

fn matches(filter: &Filter, note: &Note) -> bool {
    let query = filter.query.trim().to_lowercase();
    (filter.tag.is_empty() || note.tag == filter.tag)
        && (query.is_empty()
            || note.title.to_lowercase().contains(&query)
            || note.body.to_lowercase().contains(&query))
}

async fn save(ctx: &Ctx, note: &Note) -> Result<(), StorageError> {
    let bytes = serde_json::to_vec(&Stored::from(note)).unwrap_or_default();
    ctx.kv().set(key_of(note.id), Bytes(bytes)).await
}

// docs:begin fieldbook-hook
/// Build 1's notes (`{ id, title, text, created }`) become build 2's. A rename is not something
/// structural migration can do (a removal and an addition), so the hook reads the old name. It runs
/// wherever a build-1 note turns up: a snapshot of the store, a cached list, a queued `push_note`.
#[undra::migrate(ty = "Note")]
fn note_from_build_1(old: &DynValue) -> Result<Note, MigrateError> {
    let id = old
        .field("id")
        .and_then(DynValue::as_i64)
        .and_then(|id| u32::try_from(id).ok())
        .ok_or_else(|| MigrateError::new("a note without an id"))?;
    let title = old
        .field("title")
        .and_then(DynValue::as_str)
        .ok_or_else(|| MigrateError::new("a note without a title"))?;
    let body = old
        .field("body")
        .or_else(|| old.field("text"))
        .and_then(DynValue::as_str)
        .unwrap_or_default();
    let created = match old.field("created") {
        Some(DynValue::Timestamp(ms)) => *ms,
        _ => return Err(MigrateError::new("a note without a creation time")),
    };
    Ok(Note {
        id,
        title: title.to_owned(),
        body: body.to_owned(),
        tag: old
            .field("tag")
            .and_then(DynValue::as_str)
            .unwrap_or_default()
            .to_lowercase(),
        pinned: old
            .field("pinned")
            .and_then(DynValue::as_bool)
            .unwrap_or(false),
        created: Timestamp(created),
        photos: Vec::new(),
    })
}
// docs:end

/// What the offline queue holds: how many writes are waiting, and the ones an update could not carry
/// over (dead letters), with the reason.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outbox {
    /// Writes waiting in the offline queue.
    pub pending: u32,
    /// Writes an update could not carry over, as `"<mutation>: <reason>"`.
    pub stuck: Vec<String>,
}

/// What is waiting to be sent, now (`Notebook::pending` is the observable count).
#[undra::api]
pub fn outbox(ctx: &Ctx) -> Outbox {
    let query = ctx.query();
    Outbox {
        pending: u32::try_from(query.pending_mutations()).unwrap_or(u32::MAX),
        stuck: query
            .dead_letters()
            .into_iter()
            .map(|d| format!("{}: {}", d.mutation, d.reason))
            .collect(),
    }
}

/// A shorthand the tests use.
#[cfg(test)]
impl Notebook {
    fn titles(&self) -> Vec<String> {
        self.visible
            .with(|v| v.iter().map(|n| n.title.clone()).collect())
    }
}

#[cfg(test)]
mod tests {
    use undra::meta::ids;
    use undra::ports::fakes::Matcher;
    use undra::ports::{HttpError, HttpResponse, NetKind};
    use undra::runtime::testing::TestRuntime;
    use undra::signals::ALL_SIGNALS;
    use undra::wire::payload::{CallTarget, ChangeOp, ChangeSet, ReplyStatus};
    use undra::wire::{Decode, Encode, KeyedPatch, PatchOp, Reader};

    use super::*;
    use crate::net::testing::{App, BASE};

    /// What a signed-in member has in the secure store.
    fn signed_in(app: &App) {
        app.fakes
            .secure_store
            .insert("fieldbook.access", b"access-1".to_vec());
        app.fakes
            .secure_store
            .insert("fieldbook.refresh", b"refresh-1".to_vec());
    }

    /// A signed-in member and a server that accepts every note and photo.
    fn accept_everything(app: &App) {
        signed_in(app);
        app.fakes.http.respond(
            Matcher::custom(|r| r.url.starts_with("https://fieldbook.test/notes")),
            HttpResponse::new(204, Vec::new()),
        );
    }

    fn add(app: &App, book: &Notebook, title: &str, tag: &str) -> Note {
        app.run(book.add(title.into(), "body".into(), tag.into()))
            .unwrap()
    }

    #[test]
    fn a_note_is_saved_on_the_device_before_it_shows() {
        let app = App::new();
        accept_everything(&app);
        let book = Notebook::new(app.ctx());
        let note = add(&app, &book, "  Gate 3 hinge  ", "Repairs");
        assert_eq!(
            (note.title.as_str(), note.tag.as_str()),
            ("Gate 3 hinge", "repairs")
        );
        assert_eq!(book.titles(), ["Gate 3 hinge"]);
        let stored = app
            .fakes
            .kv
            .value(&key_of(note.id))
            .expect("saved in the Kv port");
        let stored: serde_json::Value = serde_json::from_slice(&stored).unwrap();
        assert_eq!(stored["title"], "Gate 3 hinge");
    }

    #[test]
    fn a_title_is_checked_with_typed_errors_and_nothing_changes() {
        let app = App::new();
        let book = Notebook::new(app.ctx());
        assert_eq!(
            app.run(book.add("  ".into(), String::new(), String::new())),
            Err(NoteError::EmptyTitle)
        );
        let long = "x".repeat(MAX_TITLE as usize + 1);
        assert_eq!(
            app.run(book.add(long, String::new(), String::new())),
            Err(NoteError::TitleTooLong { max: MAX_TITLE })
        );
        assert!(book.notes.with(Vec::is_empty));
        assert!(app.fakes.kv.is_empty());
    }

    #[test]
    fn a_full_device_keeps_the_note_out_of_the_list() {
        let app = App::new();
        app.fakes
            .kv
            .fail(undra::ports::fakes::FailOn::Set, StorageError::Full);
        let book = Notebook::new(app.ctx());
        assert_eq!(
            app.run(book.add("a".into(), String::new(), String::new())),
            Err(NoteError::Storage(StorageError::Full))
        );
        assert!(
            book.notes.with(Vec::is_empty),
            "what the UI shows is what is saved"
        );
    }

    #[test]
    fn the_view_is_pinned_first_then_newest_and_follows_the_filter() {
        let app = App::new();
        accept_everything(&app);
        let book = Notebook::new(app.ctx());
        for (n, (title, tag)) in [
            ("first", "birds"),
            ("second", "repairs"),
            ("third", "birds"),
        ]
        .into_iter()
        .enumerate()
        {
            app.fakes.clock.set_now_ms(1_000 * (n as i64 + 1));
            add(&app, &book, title, tag);
        }
        assert_eq!(book.titles(), ["third", "second", "first"], "newest first");
        app.run(book.toggle_pin(1)).unwrap();
        assert_eq!(book.titles(), ["first", "third", "second"], "pinned first");
        assert_eq!(book.tags.get(), ["birds", "repairs"]);

        book.set_tag("birds".into());
        assert_eq!(book.titles(), ["first", "third"]);
        book.set_query("THI".into());
        assert_eq!(
            book.titles(),
            ["third"],
            "the query reads titles and bodies, any case"
        );
        book.set_tag(String::new());
        book.set_query(String::new());
        app.run(book.remove(2)).unwrap();
        assert_eq!(book.titles(), ["first", "third"]);
        assert_eq!(book.tags.get(), ["birds"], "the tag set follows");
    }

    #[test]
    fn editing_changes_one_row_and_a_missing_note_is_a_typed_error() {
        let app = App::new();
        accept_everything(&app);
        let book = Notebook::new(app.ctx());
        let note = add(&app, &book, "old", "");
        let edited = app
            .run(book.edit(note.id, "new".into(), "text".into(), "Tag".into()))
            .unwrap();
        assert_eq!(
            (
                edited.title.as_str(),
                edited.body.as_str(),
                edited.tag.as_str()
            ),
            ("new", "text", "tag")
        );
        assert_eq!(
            app.run(book.edit(99, "x".into(), String::new(), String::new())),
            Err(NoteError::NotFound)
        );
        assert_eq!(app.run(book.toggle_pin(99)), Err(NoteError::NotFound));
        assert_eq!(app.run(book.remove(99)), Err(NoteError::NotFound));
    }

    #[test]
    fn a_relaunch_loads_what_the_device_holds_and_skips_what_it_cannot_read() {
        let first = App::new();
        accept_everything(&first);
        let book = Notebook::new(first.ctx());
        add(&first, &book, "one", "a");
        add(&first, &book, "two", "b");
        // A note an older build wrote: JSON with the old field name and no tag, pin or photos.
        first.fakes.kv.insert(
            key_of(7),
            br#"{"id":7,"title":"from build 1","text":"old words","created_ms":5}"#.to_vec(),
        );
        first.fakes.kv.insert(key_of(8), b"not json".to_vec());
        let fakes = first.fakes.clone();
        drop((book, first));

        let second = App::relaunch(&fakes);
        let book = Notebook::new(second.ctx());
        assert_eq!(second.run(book.load()), Ok(3));
        assert_eq!(book.notes.with(Vec::len), 3);
        let old = book.get(7).unwrap();
        assert_eq!(
            (old.body.as_str(), old.tag.as_str(), old.pinned),
            ("old words", "", false)
        );
        assert!(
            fakes.log.messages().iter().any(|m| m.contains("skipped")),
            "{:?}",
            fakes.log.messages()
        );
        // Ids continue above the largest one the device holds.
        accept_everything(&second);
        assert_eq!(add(&second, &book, "three", "").id, 9);
    }

    #[test]
    fn a_note_written_offline_is_sent_when_the_network_returns_and_after_a_kill() {
        let app = App::new();
        signed_in(&app);
        app.fakes.connectivity.go_offline();
        app.t.run_pending();
        app.fakes
            .http
            .fail(Matcher::any(), HttpError::Network("offline".into()));
        let book = Notebook::new(app.ctx());
        let note = add(&app, &book, "no signal at the gate", "");
        app.advance(500);
        assert_eq!(
            book.pending.get(),
            1,
            "the screen can say: 1 change waiting"
        );
        assert_eq!(
            book.titles(),
            ["no signal at the gate"],
            "and the note is there"
        );

        // The app is killed. The queue and the note are on the device.
        let fakes = app.fakes.clone();
        let key = fakes.http.calls()[0]
            .header("Idempotency-Key")
            .unwrap()
            .to_owned();
        drop((book, app));
        fakes.http.reset();

        let relaunched = App::relaunch(&fakes);
        fakes.connectivity.go_offline();
        relaunched.t.run_pending();
        let book = Notebook::new(relaunched.ctx());
        relaunched.run(book.load()).unwrap();
        assert_eq!((book.pending.get(), book.notes.with(Vec::len)), (1, 1));

        accept_everything(&relaunched);
        fakes.connectivity.go_online(NetKind::Wifi);
        relaunched.t.run_pending();
        relaunched.advance(2_000);
        assert_eq!(
            book.pending.get(),
            0,
            "the connectivity report made the notebook count again"
        );
        let sent = fakes
            .http
            .calls()
            .into_iter()
            .find(|r| r.method == HttpMethod::Put)
            .unwrap();
        assert_eq!(sent.url, format!("{BASE}/notes/{}", note.id));
        assert_eq!(
            sent.header("Idempotency-Key"),
            Some(key.as_str()),
            "the server can tell a repeat"
        );
    }

    #[test]
    fn a_photo_is_kept_in_the_fs_port_and_queued_for_upload() {
        let app = App::new();
        accept_everything(&app);
        let book = Notebook::new(app.ctx());
        let note = add(&app, &book, "owl pellet", "birds");
        let path = app
            .run(book.attach_photo(note.id, Bytes(vec![1, 2, 3])))
            .unwrap();
        assert_eq!(path, format!("photos/{}/0.jpg", note.id));
        assert_eq!(app.fakes.fs.contents(&path), Some(vec![1, 2, 3]));
        assert_eq!(
            book.get(note.id).unwrap().photos,
            std::slice::from_ref(&path)
        );
        let ctx = app.ctx();
        assert_eq!(
            app.run(read_photo(&ctx, path.clone())),
            Ok(Bytes(vec![1, 2, 3]))
        );
        app.advance(500);
        let put = app
            .fakes
            .http
            .calls()
            .into_iter()
            .find(|r| r.url.ends_with("/photos/0"))
            .expect("the upload");
        assert_eq!(put.body.as_ref().map(|b| b.0.clone()), Some(vec![1, 2, 3]));
        assert_eq!(
            app.run(book.attach_photo(99, Bytes(vec![]))),
            Err(NoteError::NotFound)
        );
    }

    #[test]
    fn a_removed_note_is_deleted_on_the_server_and_a_404_counts_as_gone() {
        let app = App::new();
        app.fakes.http.respond(
            Matcher::method(HttpMethod::Delete),
            HttpResponse::new(404, Vec::new()),
        );
        accept_everything(&app);
        let book = Notebook::new(app.ctx());
        let note = add(&app, &book, "temporary", "");
        app.run(book.remove(note.id)).unwrap();
        app.advance(500);
        assert!(app.fakes.kv.is_empty());
        assert_eq!(book.pending.get(), 0);
        assert!(
            app.fakes
                .http
                .calls()
                .iter()
                .any(|r| r.method == HttpMethod::Delete)
        );
    }

    #[test]
    fn sync_all_sends_every_note_again() {
        let app = App::new();
        accept_everything(&app);
        let book = Notebook::new(app.ctx());
        add(&app, &book, "a", "");
        add(&app, &book, "b", "");
        app.advance(500); // the two pushes of the adds
        let before = app.fakes.http.call_count();
        assert_eq!(book.sync_all(), 2);
        app.advance(500);
        assert_eq!(app.fakes.http.call_count(), before + 2);
    }

    #[test]
    fn build_1_notes_become_build_2_notes() {
        let v1 = DynValue::Record(
            DynRecord::new("Note")
                .with("id", DynValue::Int(4))
                .with("title", DynValue::String("heron".into()))
                .with("text", DynValue::String("by the weir".into()))
                .with("created", DynValue::Timestamp(1_700_000_000_000)),
        );
        assert_eq!(
            note_from_build_1(&v1),
            Ok(Note {
                id: 4,
                title: "heron".into(),
                body: "by the weir".into(),
                tag: String::new(),
                pinned: false,
                created: Timestamp(1_700_000_000_000),
                photos: vec![],
            })
        );
        // What a hook cannot make a note of is refused (a dead letter, a dropped cache entry), not guessed.
        let broken = DynValue::Record(DynRecord::new("Note").with("id", DynValue::Int(4)));
        assert!(note_from_build_1(&broken).is_err());
    }

    // ----- what the platforms receive, and what a dev-loop reload keeps -----------------------

    const NOTES: u32 = 0;
    const FILTER: u32 = 1;
    const VISIBLE: u32 = 3;

    fn construct(t: &TestRuntime) -> Handle {
        let reply = t.call_sync(
            CallTarget::Constructor {
                type_id: ids::type_id("Notebook"),
                method_id: ids::method_id("Notebook", "new"),
            },
            1,
            &[],
        );
        assert_eq!(reply.status, ReplyStatus::Ok);
        Handle::decode_exact(&reply.body).unwrap()
    }

    fn call(t: &TestRuntime, store: Handle, id: u32, method: &str, args: &[u8]) -> ReplyStatus {
        let target = CallTarget::Method {
            handle: store,
            method_id: ids::method_id("Notebook", method),
        };
        assert_eq!(t.call(target, id, args), 0);
        t.run_pending();
        t.take_replies().remove(0).status
    }

    fn ops(cs: &ChangeSet, signal: u32) -> Vec<PatchOp<Note>> {
        let entry = cs
            .entries
            .iter()
            .find(|e| e.signal_id == signal)
            .expect("an entry");
        assert_eq!(entry.op, ChangeOp::KeyedPatch, "{cs:?}");
        let mut reader = Reader::new(&entry.value);
        let patch = KeyedPatch::<Note>::decode(&mut reader).unwrap();
        reader.finish().unwrap();
        patch.ops
    }

    fn args(title: &str, body: &str, tag: &str) -> Vec<u8> {
        [
            title.to_owned().encode_to_vec(),
            body.to_owned().encode_to_vec(),
            tag.to_owned().encode_to_vec(),
        ]
        .concat()
    }

    #[test]
    fn a_new_note_is_a_one_row_patch_for_the_list_and_the_view() {
        let app = App::new();
        accept_everything(&app);
        let store = construct(&app.t);
        app.t.take_change_sets();
        app.t.runtime().observe(store.0, ALL_SIGNALS, true);
        app.t.host().take_decoded_change_sets();
        assert_eq!(
            call(&app.t, store, 2, "add", &args("owl", "", "")),
            ReplyStatus::Ok
        );
        let sets = app.t.host().take_decoded_change_sets();
        let first = &sets[0];
        let inserted = ops(first, NOTES);
        assert!(
            matches!(inserted.as_slice(), [PatchOp::Insert { index: 0, item }] if item.title == "owl")
        );
        assert_eq!(
            ops(first, VISIBLE).len(),
            1,
            "the view is patched, not sent"
        );
        assert!(first.entries.iter().all(|e| e.signal_id != FILTER));
    }

    #[test]
    fn a_dev_reload_keeps_the_notes_the_filter_and_the_ids() {
        // `undra dev` carries the state across a rebuild with a snapshot and a restore: the stores
        // are rebuilt by `assemble`, which has to rebuild the view, the tag set and the id counter.
        let app = App::new();
        accept_everything(&app);
        let store = construct(&app.t);
        for title in ["a", "b", "c"] {
            assert_eq!(
                call(&app.t, store, 2, "add", &args(title, "", "x")),
                ReplyStatus::Ok
            );
        }
        call(
            &app.t,
            store,
            3,
            "set_query",
            &"b".to_owned().encode_to_vec(),
        );
        let snapshot = app.t.runtime().snapshot();
        app.t
            .runtime()
            .restore(&snapshot)
            .expect("the snapshot restores");

        app.t.take_change_sets();
        app.t.runtime().observe(store.0, ALL_SIGNALS, true);
        let initial = &app.t.host().take_decoded_change_sets()[0];
        let view = initial
            .entries
            .iter()
            .find(|e| e.signal_id == VISIBLE)
            .unwrap();
        assert_eq!(view.op, ChangeOp::Full);
        let titles: Vec<String> = Vec::<Note>::decode_exact(&view.value)
            .unwrap()
            .into_iter()
            .map(|n| n.title)
            .collect();
        assert_eq!(
            titles,
            ["b"],
            "the filter survived and the view was rebuilt from it"
        );
        // And the next note gets a fresh id, not one a note already has.
        assert_eq!(
            call(&app.t, store, 4, "add", &args("d", "", "")),
            ReplyStatus::Ok
        );
        let stored = app
            .fakes
            .kv
            .keys()
            .into_iter()
            .filter(|k| k.starts_with(KEY_PREFIX))
            .count();
        assert_eq!(stored, 4, "a, b, c and d are four different notes");
    }
}
