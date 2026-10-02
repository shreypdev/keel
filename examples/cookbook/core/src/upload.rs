//! File upload: the file from `Fs`, the parts over `Http`, progress as a signal, and retries
//! through the offline queue.
//!
//! The `Http` port has no streaming request body, so a file goes up in parts: each part is an
//! idempotent `#[undra::mutation]`, which is what makes the rest work:
//!
//! * **Offline.** A part that fails because the device is offline is queued, and the upload waits
//!   (its `await` does not return) until the network is back and the queue replays it. Nothing in
//!   this module checks connectivity.
//! * **Restarts.** The queue is persisted, so a part that was waiting when the app was killed is
//!   sent when it starts again. The `Idempotency-Key` header is the same on every replay, so the
//!   server can tell a repeated part from a new one.
//! * **Progress.** `uploads` is a keyed list; every part that is acknowledged is a one-row
//!   `Update` patch (`sent` of `total` bytes), not the list.
//! * **Retry.** A part the server refuses fails the upload with the reason in its row.
//!   [`Uploads::retry`] reads the file again and continues from the first part the server has not
//!   acknowledged.
//!
//! One part is in the queue at a time, because the queue stores a part's bytes: the memory and
//! the disk it takes are one part's, not the file's.

use std::sync::atomic::{AtomicU32, Ordering};

use undra::ports::{FsError, HttpMethod, HttpRequest};
use undra::prelude::*;

use crate::net::{self, NetError};

/// The size of one part: 64 KiB.
const PART: usize = 64 * 1024;

/// Where an upload is.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UploadState {
    /// Parts are being sent (or waiting in the offline queue).
    Sending,
    /// Every part is acknowledged and the server has the whole file.
    Done,
    /// The upload stopped; `retry` continues it.
    Failed {
        /// Why, in the words of the error.
        reason: String,
    },
}

/// One upload, a row of the `uploads` signal of the `Uploads` store.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upload {
    /// Identity of the row; the list is updated by key.
    pub id: u32,
    /// The file's path in the app's storage, as given to `start`.
    pub path: String,
    /// The server's name for this upload, made once and kept for retries.
    pub remote: String,
    /// Bytes the server has acknowledged.
    pub sent: u32,
    /// The file's size in bytes.
    pub total: u32,
    /// Where it is.
    pub state: UploadState,
}

/// Why an upload failed.
#[undra::error]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UploadError {
    /// The file could not be read.
    #[error("file: {0}")]
    File(#[from] FsError),
    /// The server or the network refused a request.
    #[error("{0}")]
    Net(#[from] NetError),
    /// There is no upload with this id.
    #[error("no such upload")]
    Unknown,
    /// The file is larger than an upload can be (4 GiB).
    #[error("the file is too large")]
    TooLarge,
    /// The core is shutting down.
    #[error("the core is shutting down")]
    Closed,
}

// docs:begin upload-part
/// Sends part `index` of `total`: `PUT /uploads/{upload}/parts/{index}`. Idempotent, so while the
/// device is offline it waits in the persisted queue and is replayed when the network returns.
#[undra::mutation(key = "upload:{upload}", idempotent)]
pub async fn put_part(
    ctx: &Ctx,
    upload: String,
    index: u32,
    data: Bytes,
) -> Result<u32, UploadError> {
    let url = net::url(ctx, &format!("/uploads/{upload}/parts/{index}"))?;
    let mut request = HttpRequest::new(HttpMethod::Put, url).with_body(data.0.clone());
    if let Some(key) = undra::query::idempotency_key() {
        request = request.with_header("Idempotency-Key", key.to_string());
    }
    net::ok(net::send(ctx, request).await?)?;
    Ok(u32::try_from(data.0.len()).unwrap_or(u32::MAX))
}
// docs:end

/// Tells the server every part is there: `POST /uploads/{upload}/complete`.
#[undra::mutation(key = "upload:{upload}", idempotent)]
pub async fn complete_upload(ctx: &Ctx, upload: String, parts: u32) -> Result<(), UploadError> {
    let url = net::url(ctx, &format!("/uploads/{upload}/complete"))?;
    let body = serde_json::json!({ "parts": parts });
    let mut request = net::post_json(url, &body);
    if let Some(key) = undra::query::idempotency_key() {
        request = request.with_header("Idempotency-Key", key.to_string());
    }
    net::ok(net::send(ctx, request).await?)?;
    Ok(())
}

/// The uploads screen's store.
#[undra::store(restore = "Self::assemble")]
pub struct Uploads {
    ctx: WeakCtx,
    next: AtomicU32,
    #[undra(key = "id")]
    uploads: Signal<Vec<Upload>>,
}

#[undra::api(store)]
impl Uploads {
    /// No uploads.
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(ctx, Signal::new(vec![]))
    }

    // Used by `new` and, through `restore = ".."`, to rebuild the store from a snapshot.
    fn assemble(ctx: Ctx, uploads: Signal<Vec<Upload>>) -> Self {
        let next = uploads.with(|rows| rows.iter().map(|u| u.id).max().unwrap_or(0));
        Self {
            ctx: ctx.downgrade(),
            next: AtomicU32::new(next + 1),
            uploads,
        }
    }

    /// Uploads the file at `path` (a path of the `Fs` port: the app's own storage, where a photo
    /// picker's copy lives) and returns the row's id when the server has all of it. The row is
    /// there from the start, so the screen shows progress while this call is running.
    pub async fn start(&self, path: String) -> Result<u32, UploadError> {
        let ctx = self.ctx.upgrade().map_err(|_| UploadError::Closed)?;
        let data = ctx.fs().read(path.clone()).await?;
        let total = u32::try_from(data.0.len()).map_err(|_| UploadError::TooLarge)?;
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        // The server's name for the upload comes from the Rng port: random in an app, seeded in a test.
        let remote: String = ctx
            .rng()
            .fill(8)
            .0
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.uploads.push(Upload {
            id,
            path,
            remote,
            sent: 0,
            total,
            state: UploadState::Sending,
        });
        self.run(&ctx, id, data).await?;
        Ok(id)
    }

    /// Continues a failed upload from the first part the server has not acknowledged.
    pub async fn retry(&self, id: u32) -> Result<(), UploadError> {
        let ctx = self.ctx.upgrade().map_err(|_| UploadError::Closed)?;
        let row = self.row(id).ok_or(UploadError::Unknown)?;
        let data = ctx.fs().read(row.path).await?;
        self.set_state(id, |row| row.state = UploadState::Sending);
        self.run(&ctx, id, data).await
    }

    /// Sends the parts from the first one not yet acknowledged, then completes the upload.
    async fn run(&self, ctx: &Ctx, id: u32, data: Bytes) -> Result<(), UploadError> {
        let result = self.send_parts(ctx, id, &data.0).await;
        match &result {
            Ok(()) => self.set_state(id, |row| row.state = UploadState::Done),
            Err(error) => {
                let reason = error.to_string();
                self.set_state(id, |row| row.state = UploadState::Failed { reason });
            }
        }
        result
    }

    // docs:begin upload-loop
    async fn send_parts(&self, ctx: &Ctx, id: u32, data: &[u8]) -> Result<(), UploadError> {
        let row = self.row(id).ok_or(UploadError::Unknown)?;
        let first = row.sent as usize / PART;
        for (index, part) in data.chunks(PART).enumerate().skip(first) {
            // Waits here while the device is offline, and carries on when the queue replays it.
            let sent = ctx
                .mutate::<PutPartMutation>((
                    row.remote.clone(),
                    u32::try_from(index).unwrap_or(u32::MAX),
                    Bytes(part.to_vec()),
                ))
                .await?;
            // One row, one `Update` patch: this is the progress the screen draws.
            self.set_state(id, |row| row.sent = (index * PART) as u32 + sent);
        }
        let parts = u32::try_from(data.chunks(PART).count()).unwrap_or(u32::MAX);
        ctx.mutate::<CompleteUploadMutation>((row.remote, parts))
            .await?;
        Ok(())
    }
    // docs:end

    fn row(&self, id: u32) -> Option<Upload> {
        self.uploads
            .with(|rows| rows.iter().find(|row| row.id == id).cloned())
    }

    /// Changes one row, by key: a recorded `update_at`, so the platforms get one-row patches.
    fn set_state(&self, id: u32, change: impl FnOnce(&mut Upload)) {
        if let Some(at) = self
            .uploads
            .with(|rows| rows.iter().position(|row| row.id == id))
        {
            self.uploads.update_at(at, change);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use undra::ports::fakes::Matcher;
    use undra::ports::{HttpError, HttpResponse, NetKind};

    use super::*;
    use crate::net::testing::{App, BASE};

    /// A file of `len` bytes in the fake file system.
    fn seed_file(app: &App, path: &str, len: usize) {
        app.fakes
            .fs
            .seed(path, (0..len).map(|i| (i % 251) as u8).collect::<Vec<u8>>())
            .unwrap();
    }

    fn accept_everything(app: &App) {
        app.fakes.http.respond(
            Matcher::method(HttpMethod::Put),
            HttpResponse::new(204, Vec::new()),
        );
        app.fakes.http.respond(
            Matcher::custom(|r| r.method == HttpMethod::Post && r.url.ends_with("/complete")),
            HttpResponse::new(204, Vec::new()),
        );
    }

    /// Records every value of the first row's `sent` from now on.
    fn watch_sent(uploads: &Uploads) -> (Effect, Arc<Mutex<Vec<u32>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let effect = Effect::new(&uploads.uploads, move |rows| {
            if let Some(row) = rows.first() {
                sink.lock().unwrap().push(row.sent);
            }
        });
        (effect, seen)
    }

    #[test]
    fn a_file_goes_up_in_parts_and_progress_follows_each_one() {
        let app = App::new();
        accept_everything(&app);
        seed_file(&app, "photos/a.jpg", 150_000);
        let uploads = Uploads::new(app.ctx());
        let (_effect, seen) = watch_sent(&uploads);
        assert_eq!(app.run(uploads.start("photos/a.jpg".into())), Ok(1));

        let row = uploads.row(1).unwrap();
        assert_eq!(
            (row.sent, row.total, row.state),
            (150_000, 150_000, UploadState::Done)
        );
        // 0 when the row appears, then one value per acknowledged part (the last write, the state
        // going to `Done`, repeats the last value).
        let mut seen = seen.lock().unwrap().clone();
        seen.dedup();
        assert_eq!(seen, [0, 65_536, 131_072, 150_000]);

        let calls = app.fakes.http.calls();
        let parts: Vec<&HttpRequest> = calls
            .iter()
            .filter(|r| r.method == HttpMethod::Put)
            .collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(
            parts
                .iter()
                .map(|r| r.body.as_ref().unwrap().0.len())
                .collect::<Vec<_>>(),
            [65_536, 65_536, 18_928]
        );
        assert!(parts.iter().all(|r| r.header("Idempotency-Key").is_some()));
        assert!(parts[0].url.contains("/uploads/") && parts[0].url.ends_with("/parts/0"));
        assert!(calls.last().unwrap().url.ends_with("/complete"));
    }

    #[test]
    fn a_missing_file_is_a_typed_error_and_makes_no_row() {
        let app = App::new();
        let uploads = Uploads::new(app.ctx());
        assert_eq!(
            app.run(uploads.start("photos/none.jpg".into())),
            Err(UploadError::File(FsError::NotFound))
        );
        assert!(uploads.uploads.with(Vec::is_empty));
    }

    #[test]
    fn an_upload_made_offline_waits_in_the_queue_and_finishes_when_the_network_returns() {
        let app = App::new();
        seed_file(&app, "photos/a.jpg", 100_000);
        let uploads = Arc::new(Uploads::new(app.ctx()));
        app.fakes.connectivity.go_offline();
        app.t.run_pending();
        app.fakes.http.fail(
            Matcher::method(HttpMethod::Put),
            HttpError::Network("offline".into()),
        );

        let result = Arc::new(Mutex::new(None));
        let (slot, store) = (result.clone(), uploads.clone());
        app.ctx().spawn(async move {
            *slot.lock().unwrap() = Some(store.start("photos/a.jpg".into()).await);
        });
        app.t.run_pending();
        assert!(
            result.lock().unwrap().is_none(),
            "queued: the call keeps waiting"
        );
        let row = uploads.row(1).unwrap();
        assert_eq!((row.sent, row.state), (0, UploadState::Sending));
        assert_eq!(
            app.ctx().query().pending_mutations(),
            1,
            "one part, not the file"
        );

        // The network returns and the server accepts what is replayed.
        app.fakes.http.reset();
        accept_everything(&app);
        app.fakes.connectivity.go_online(NetKind::Wifi);
        app.t.run_pending();
        app.advance(2_000);
        assert_eq!(result.lock().unwrap().clone(), Some(Ok(1)));
        assert_eq!(uploads.row(1).unwrap().state, UploadState::Done);
        assert_eq!(app.ctx().query().pending_mutations(), 0);
    }

    #[test]
    fn a_refused_part_fails_the_upload_and_retry_continues_from_there() {
        let app = App::new();
        seed_file(&app, "photos/a.jpg", 150_000);
        // The first part is accepted, the second is refused, then the server recovers.
        app.fakes.http.respond_sequence(
            Matcher::method(HttpMethod::Put),
            [
                Ok(HttpResponse::new(204, Vec::new())),
                Ok(HttpResponse::new(500, b"boom".to_vec())),
            ],
        );
        let uploads = Uploads::new(app.ctx());
        let failed = app.run(uploads.start("photos/a.jpg".into()));
        assert_eq!(
            failed,
            Err(UploadError::Net(NetError::Status { code: 500 }))
        );
        let row = uploads.row(1).unwrap();
        assert_eq!(row.sent, 65_536, "the first part counts");
        assert_eq!(
            row.state,
            UploadState::Failed {
                reason: "the server answered 500".into()
            }
        );

        app.fakes.http.reset();
        accept_everything(&app);
        assert_eq!(app.run(uploads.retry(1)), Ok(()));
        let row = uploads.row(1).unwrap();
        assert_eq!((row.sent, row.state), (150_000, UploadState::Done));
        let resent: Vec<String> = app
            .fakes
            .http
            .calls()
            .iter()
            .filter(|r| r.method == HttpMethod::Put)
            .map(|r| r.url.rsplit('/').next().unwrap().to_owned())
            .collect();
        assert_eq!(resent, ["1", "2"], "part 0 is not sent again");
        assert_eq!(app.run(uploads.retry(9)), Err(UploadError::Unknown));
    }

    #[test]
    fn the_servers_name_for_an_upload_comes_from_the_rng_port() {
        let app = App::new();
        accept_everything(&app);
        seed_file(&app, "a", 10);
        seed_file(&app, "b", 10);
        let uploads = Uploads::new(app.ctx());
        app.run(uploads.start("a".into())).unwrap();
        app.run(uploads.start("b".into())).unwrap();
        let (a, b) = (
            uploads.row(1).unwrap().remote,
            uploads.row(2).unwrap().remote,
        );
        assert_eq!(a.len(), 16);
        assert_ne!(a, b);
        assert!(
            app.fakes.http.calls()[0]
                .url
                .starts_with(&format!("{BASE}/uploads/{a}/"))
        );
    }
}
