//! The standard ports seen from the core: functions that call `Kv`, `SecureStore`, `Fs` and
//! `Http`, and a store that shows the `Connectivity` and `Lifecycle` reports the core received.
//!
//! An app never needs these to use the ports (the query layer persists through `Kv` and follows
//! `Connectivity` on its own); they exist so that a platform can prove its adapters **through the
//! core**, the way the core itself reaches them: the React Native app's on-device checks
//! (`examples/playground/rn/src/checks.ts`, ADR-038 amendment B) call them, and any playground app
//! can. Each function is one port call:
//!
//! * [`kv_put`], [`kv_get`], [`kv_remove`], [`kv_keys`]: the four methods of `Kv`;
//! * [`secret_put`], [`secret_get`], [`secret_remove`], [`secret_keys`]: the same on `SecureStore`;
//! * [`file_write`], [`file_read`], [`file_delete`], [`file_list`]: `Fs`, with its typed
//!   [`FsError`] (a path outside the adapter's root is `Denied`);
//! * [`http_get`]: one `GET` through `Http`, with its typed [`HttpError`] (no network is `Network`).
//!
//! `Kv` and `SecureStore` answer failures with their typed [`StorageError`] (ADR-049): a full
//! store is `Full`, no adapter is `Unavailable`, never a missing value.
//!
//! [`Device`] shows the last `Connectivity` and `Lifecycle` report and how many of each the core
//! received **since it started**: an init hook subscribes when the runtime is created, so the
//! first reports, which a platform sends as soon as the core is up, are counted even though no
//! store exists yet. Like the rest of the playground core it reads no clock and starts no thread
//! (R12), and its tests drive it with `undra::ports::fakes`.

use std::sync::{Mutex, MutexGuard, PoisonError};

use undra::ports::{
    AppState, FsError, HttpError, HttpRequest, HttpResponse, NetKind, StorageError,
};
use undra::prelude::*;
use undra::runtime::{InitHook, Subscription, inventory};

// ----- Kv ---------------------------------------------------------------------------------------

/// Stores `value` under `key` in the `Kv` port.
#[undra::api]
pub async fn kv_put(ctx: &Ctx, key: String, value: Bytes) -> Result<(), StorageError> {
    ctx.kv().set(key, value).await
}

/// The value the `Kv` port has under `key`, if any.
#[undra::api]
pub async fn kv_get(ctx: &Ctx, key: String) -> Result<Option<Bytes>, StorageError> {
    ctx.kv().get(key).await
}

/// Removes `key` from the `Kv` port; a missing key is not an error.
#[undra::api]
pub async fn kv_remove(ctx: &Ctx, key: String) -> Result<(), StorageError> {
    ctx.kv().delete(key).await
}

/// The keys of the `Kv` port that start with `prefix`, in ascending order.
#[undra::api]
pub async fn kv_keys(ctx: &Ctx, prefix: String) -> Result<Vec<String>, StorageError> {
    ctx.kv().list(prefix).await
}

// ----- SecureStore ------------------------------------------------------------------------------

/// Stores `value` under `key` in the `SecureStore` port (the Keychain, the Android Keystore).
#[undra::api]
pub async fn secret_put(ctx: &Ctx, key: String, value: Bytes) -> Result<(), StorageError> {
    ctx.secure_store().set(key, value).await
}

/// The value the `SecureStore` port has under `key`, if any.
#[undra::api]
pub async fn secret_get(ctx: &Ctx, key: String) -> Result<Option<Bytes>, StorageError> {
    ctx.secure_store().get(key).await
}

/// Removes `key` from the `SecureStore` port; a missing key is not an error.
#[undra::api]
pub async fn secret_remove(ctx: &Ctx, key: String) -> Result<(), StorageError> {
    ctx.secure_store().delete(key).await
}

/// The keys of the `SecureStore` port that start with `prefix`, in ascending order.
#[undra::api]
pub async fn secret_keys(ctx: &Ctx, prefix: String) -> Result<Vec<String>, StorageError> {
    ctx.secure_store().list(prefix).await
}

// ----- Fs ---------------------------------------------------------------------------------------

/// Writes `data` to the file `path` of the `Fs` port, creating its directories.
#[undra::api]
pub async fn file_write(ctx: &Ctx, path: String, data: Bytes) -> Result<(), FsError> {
    ctx.fs().write(path, data).await
}

/// The contents of the file `path` of the `Fs` port.
#[undra::api]
pub async fn file_read(ctx: &Ctx, path: String) -> Result<Bytes, FsError> {
    ctx.fs().read(path).await
}

/// Deletes the file or directory `path` of the `Fs` port.
#[undra::api]
pub async fn file_delete(ctx: &Ctx, path: String) -> Result<(), FsError> {
    ctx.fs().delete(path).await
}

/// The names in the directory `dir` of the `Fs` port, sorted.
#[undra::api]
pub async fn file_list(ctx: &Ctx, dir: String) -> Result<Vec<String>, FsError> {
    ctx.fs().list(dir).await
}

// ----- Http -------------------------------------------------------------------------------------

/// `GET url` through the `Http` port, with an optional timeout in milliseconds. Any status is a
/// response; only a failure to get one is an [`HttpError`].
#[undra::api]
pub async fn http_get(
    ctx: &Ctx,
    url: String,
    timeout_ms: Option<u32>,
) -> Result<HttpResponse, HttpError> {
    let mut request = HttpRequest::get(url);
    if let Some(timeout) = timeout_ms {
        request = request.with_timeout_ms(timeout);
    }
    ctx.http().request(request).await
}

// ----- Connectivity and Lifecycle ---------------------------------------------------------------

/// What the core has been told since it started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Reports {
    online: bool,
    net_kind: NetKind,
    app_state: AppState,
    connectivity: u32,
    lifecycle: u32,
}

impl Default for Reports {
    /// Before the first report the core assumes what the query layer assumes: online, active.
    fn default() -> Reports {
        Reports {
            online: true,
            net_kind: NetKind::Unknown,
            app_state: AppState::Active,
            connectivity: 0,
            lifecycle: 0,
        }
    }
}

/// The runtime's record of the reports, kept from the moment the runtime exists.
#[derive(Default)]
struct Seen {
    reports: Mutex<Reports>,
}

fn reports(ctx: &Ctx) -> MutexGuard<'_, Reports> {
    ctx.runtime()
        .extension::<Seen>()
        .reports
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Subscribes the runtime's record to both event ports. The subscriptions live as long as the
/// runtime (they use the `Ctx` they are given, never a captured one: ADR-034).
fn watch(ctx: &Ctx) {
    undra::ports::on_connectivity_changed(ctx, |ctx, online, kind| {
        let mut seen = reports(ctx);
        seen.online = online;
        seen.net_kind = kind;
        seen.connectivity = seen.connectivity.saturating_add(1);
    })
    .detach();
    undra::ports::on_lifecycle_changed(ctx, |ctx, state| {
        let mut seen = reports(ctx);
        seen.app_state = state;
        seen.lifecycle = seen.lifecycle.saturating_add(1);
    })
    .detach();
}

inventory::submit! {
    InitHook { name: "playground.platform", run: watch }
}

/// The `Connectivity` and `Lifecycle` reports as the core received them: the last of each and how
/// many there were since the core started. Every report moves the signals in one transaction, and
/// every signal is `no_coalesce`: a platform's mirror applies each report, even several that arrive
/// in one frame (or while the app is in the background, where React Native on Android pauses the
/// timers that drain it), so a UI or a check sees `background` even when `active` follows at once.
#[undra::store(restore = "Self::assemble")]
pub struct Device {
    /// Keeps the store's own subscriptions; dropped with the store.
    _subscriptions: Vec<Subscription>,
    /// Whether the last `Connectivity` report said online (`true` before any report).
    #[undra(no_coalesce)]
    online: Signal<bool>,
    /// The kind of network of the last report (`Unknown` before any report).
    #[undra(no_coalesce)]
    net_kind: Signal<NetKind>,
    /// The last `Lifecycle` report (`Active` before any report).
    #[undra(no_coalesce)]
    app_state: Signal<AppState>,
    /// How many `Connectivity` reports the core received since it started.
    #[undra(no_coalesce)]
    connectivity_reports: Signal<u32>,
    /// How many `Lifecycle` reports the core received since it started.
    #[undra(no_coalesce)]
    lifecycle_reports: Signal<u32>,
}

#[undra::api(store)]
impl Device {
    /// The store, showing every report received so far and following the next ones.
    pub fn new(ctx: Ctx) -> Self {
        let start = Reports::default();
        Self::assemble(
            ctx,
            Signal::new(start.online),
            Signal::new(start.net_kind),
            Signal::new(start.app_state),
            Signal::new(start.connectivity),
            Signal::new(start.lifecycle),
        )
    }

    // Used by `new` and, through `restore = ".."`, to rebuild the store from a snapshot: either way
    // the signals then show what the runtime has seen, which a snapshot cannot know.
    fn assemble(
        ctx: Ctx,
        online: Signal<bool>,
        net_kind: Signal<NetKind>,
        app_state: Signal<AppState>,
        connectivity_reports: Signal<u32>,
        lifecycle_reports: Signal<u32>,
    ) -> Self {
        let signals = DeviceSignals {
            online,
            net_kind,
            app_state,
            connectivity_reports,
            lifecycle_reports,
        };
        signals.show(*reports(&ctx));
        // Subscribed after the init hook's, so the record is already updated when these run.
        let on_net = signals.clone();
        let on_life = signals.clone();
        let subscriptions = vec![
            undra::ports::on_connectivity_changed(&ctx, move |ctx, _, _| {
                on_net.show(*reports(ctx));
            }),
            undra::ports::on_lifecycle_changed(&ctx, move |ctx, _| {
                on_life.show(*reports(ctx));
            }),
        ];
        let DeviceSignals {
            online,
            net_kind,
            app_state,
            connectivity_reports,
            lifecycle_reports,
        } = signals;
        Self {
            _subscriptions: subscriptions,
            online,
            net_kind,
            app_state,
            connectivity_reports,
            lifecycle_reports,
        }
    }
}

/// The store's signals, shared with its subscriptions.
#[derive(Clone)]
struct DeviceSignals {
    online: Signal<bool>,
    net_kind: Signal<NetKind>,
    app_state: Signal<AppState>,
    connectivity_reports: Signal<u32>,
    lifecycle_reports: Signal<u32>,
}

impl DeviceSignals {
    /// Shows `reports`, in one transaction.
    fn show(&self, reports: Reports) {
        txn(|| {
            self.online.set(reports.online);
            self.net_kind.set(reports.net_kind);
            self.app_state.set(reports.app_state);
            self.connectivity_reports.set(reports.connectivity);
            self.lifecycle_reports.set(reports.lifecycle);
        });
    }
}

#[cfg(test)]
mod tests {
    use undra::ports::fakes::{self, Fakes, Matcher};
    use undra::runtime::testing::TestRuntime;

    use super::*;

    /// A runtime like a started app: every port faked and the init hooks run.
    fn app() -> (TestRuntime, Fakes) {
        let t = TestRuntime::new();
        let fakes = fakes::install(&t);
        t.run_init_hooks();
        (t, fakes)
    }

    #[test]
    fn kv_functions_reach_the_kv_port() {
        let (t, fakes) = app();
        let ctx = t.ctx();
        t.run_until(kv_put(&ctx, "a.1".into(), Bytes(vec![1, 2])))
            .unwrap();
        t.run_until(kv_put(&ctx, "a.2".into(), Bytes(vec![3])))
            .unwrap();
        t.run_until(kv_put(&ctx, "b".into(), Bytes(vec![4])))
            .unwrap();
        assert_eq!(fakes.kv.value("a.1"), Some(vec![1, 2]));
        assert_eq!(
            t.run_until(kv_get(&ctx, "a.1".into())),
            Ok(Some(Bytes(vec![1, 2])))
        );
        assert_eq!(
            t.run_until(kv_keys(&ctx, "a.".into())).unwrap(),
            ["a.1", "a.2"]
        );
        t.run_until(kv_remove(&ctx, "a.1".into())).unwrap();
        assert_eq!(t.run_until(kv_get(&ctx, "a.1".into())), Ok(None));
        assert!(
            fakes.secure_store.is_empty(),
            "Kv never reaches SecureStore"
        );
    }

    #[test]
    fn secret_functions_reach_the_secure_store() {
        let (t, fakes) = app();
        let ctx = t.ctx();
        t.run_until(secret_put(&ctx, "token".into(), Bytes(b"s3cret".to_vec())))
            .unwrap();
        assert_eq!(fakes.secure_store.value("token"), Some(b"s3cret".to_vec()));
        assert!(fakes.kv.is_empty(), "SecureStore never reaches Kv");
        assert_eq!(
            t.run_until(secret_get(&ctx, "token".into())),
            Ok(Some(Bytes(b"s3cret".to_vec())))
        );
        assert_eq!(
            t.run_until(secret_keys(&ctx, String::new())).unwrap(),
            ["token"]
        );
        t.run_until(secret_remove(&ctx, "token".into())).unwrap();
        assert_eq!(t.run_until(secret_get(&ctx, "token".into())), Ok(None));
    }

    #[test]
    fn a_storage_failure_is_a_typed_answer() {
        let (t, fakes) = app();
        let ctx = t.ctx();
        fakes.kv.fail(fakes::FailOn::Set, StorageError::Full);
        assert_eq!(
            t.run_until(kv_put(&ctx, "k".into(), Bytes(vec![1]))),
            Err(StorageError::Full)
        );
    }

    #[test]
    fn file_functions_reach_the_fs_port_with_typed_errors() {
        let (t, fakes) = app();
        let ctx = t.ctx();
        t.run_until(file_write(
            &ctx,
            "notes/a.txt".into(),
            Bytes(b"hi".to_vec()),
        ))
        .unwrap();
        assert_eq!(fakes.fs.contents("notes/a.txt"), Some(b"hi".to_vec()));
        assert_eq!(
            t.run_until(file_read(&ctx, "notes/a.txt".into())).unwrap(),
            Bytes(b"hi".to_vec())
        );
        assert_eq!(
            t.run_until(file_list(&ctx, "notes".into())).unwrap(),
            ["a.txt"]
        );
        t.run_until(file_delete(&ctx, "notes/a.txt".into()))
            .unwrap();
        assert_eq!(
            t.run_until(file_read(&ctx, "notes/a.txt".into())),
            Err(FsError::NotFound)
        );
    }

    #[test]
    fn http_get_sends_one_get_and_passes_errors_through() {
        let (t, fakes) = app();
        let ctx = t.ctx();
        fakes.http.respond(
            Matcher::get("http://127.0.0.1:1/ok"),
            HttpResponse::new(200, b"pong".to_vec()),
        );
        fakes.http.fail(
            Matcher::url("http://127.0.0.1:1/down"),
            HttpError::Network("connection refused".into()),
        );
        let response = t
            .run_until(http_get(&ctx, "http://127.0.0.1:1/ok".into(), Some(500)))
            .unwrap();
        assert_eq!(
            (response.status, response.body),
            (200, Bytes(b"pong".to_vec()))
        );
        assert_eq!(fakes.http.last_call().unwrap().timeout_ms, Some(500));
        assert_eq!(
            t.run_until(http_get(&ctx, "http://127.0.0.1:1/down".into(), None)),
            Err(HttpError::Network("connection refused".into()))
        );
    }

    #[test]
    fn reports_before_the_store_exists_are_counted() {
        let (t, fakes) = app();
        fakes.connectivity.go_offline();
        fakes.lifecycle.set(AppState::Background);
        t.run_pending();
        let _scope = t.ctx().enter();
        let device = Device::new(t.ctx());
        assert!(!device.online.get());
        assert_eq!(device.net_kind.get(), NetKind::None);
        assert_eq!(device.app_state.get(), AppState::Background);
        assert_eq!(device.connectivity_reports.get(), 1);
        assert_eq!(device.lifecycle_reports.get(), 1);
    }

    #[test]
    fn the_store_follows_later_reports() {
        let (t, fakes) = app();
        let device = {
            let _scope = t.ctx().enter();
            Device::new(t.ctx())
        };
        assert!(device.online.get(), "online before any report");
        assert_eq!(device.connectivity_reports.get(), 0);
        fakes.connectivity.go_offline();
        fakes.connectivity.go_online(NetKind::Cellular);
        fakes.lifecycle.set(AppState::Background);
        fakes.lifecycle.set(AppState::Active);
        t.run_pending();
        assert!(device.online.get());
        assert_eq!(device.net_kind.get(), NetKind::Cellular);
        assert_eq!(device.app_state.get(), AppState::Active);
        assert_eq!(device.connectivity_reports.get(), 2);
        assert_eq!(device.lifecycle_reports.get(), 2);
        drop(device);
        fakes.lifecycle.set(AppState::Background);
        t.run_pending();
        let _scope = t.ctx().enter();
        assert_eq!(
            Device::new(t.ctx()).lifecycle_reports.get(),
            3,
            "the runtime kept counting after the store went away"
        );
    }
}
