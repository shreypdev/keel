//! Auth: a session store, the tokens in `SecureStore`, one re-auth on a 401, and a logout that
//! clears what the user left behind.
//!
//! * [`Auth`] is the store a login screen observes: the [`Session`] and whether a call is running.
//! * The tokens live in the `SecureStore` port (the Keychain, the Android Keystore, WebCrypto),
//!   never in a signal: a signal is mirrored into every platform's memory, a secret should not be.
//! * [`authed`] is the one way the recipes talk to a protected endpoint. It adds the access token;
//!   on a `401` it refreshes once (a second request that meets the same `401` waits for that one
//!   refresh instead of starting another) and retries; when the refresh is refused it signs the
//!   user out. Queries and mutations call it, so an offline replay of a queued write uses the token
//!   that is current when the replay runs.
//! * [`Auth::sign_out`] forgets the tokens and the persisted cache, and refuses to run while
//!   writes wait in the offline queue (they would be sent as whoever signs in next).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use serde::Deserialize;
use undra::ports::{HttpRequest, HttpResponse, StorageError};
use undra::prelude::*;

use crate::net::{self, NetError};

const ACCESS: &str = "auth.access";
const REFRESH: &str = "auth.refresh";
/// The prefix of the query client's persisted cache entries (`undra.query.cache2.<id>.<hash>`).
const CACHE_PREFIX: &str = "undra.query.cache2.";

/// Who is signed in.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Session {
    /// Nobody: show the login screen.
    SignedOut,
    /// A user with a session the server accepted.
    SignedIn {
        /// The display name the server returned at sign-in.
        user: String,
    },
}

/// Why an auth call failed.
#[undra::error]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthError {
    /// The server refused the email or the password.
    #[error("wrong email or password")]
    BadCredentials,
    /// The session ended and refreshing it was refused: the user has to sign in again.
    #[error("the session ended, sign in again")]
    SessionExpired,
    /// Sign-out was refused: writes are still waiting to be sent, and would go out as the next user.
    #[error("{count} changes have not been sent yet")]
    PendingWrites {
        /// How many writes are waiting in the offline queue.
        count: u32,
    },
    /// The request failed.
    #[error("{0}")]
    Net(#[from] NetError),
    /// The secure store failed.
    #[error("secure storage: {0}")]
    Storage(#[from] StorageError),
    /// The core is shutting down.
    #[error("the core is shutting down")]
    Closed,
}

/// The tokens the server hands out at sign-in and at refresh.
#[derive(Deserialize)]
struct Tokens {
    access: String,
    refresh: String,
    #[serde(default)]
    user: String,
}

/// What [`authed`] and the store share in one runtime.
#[derive(Default)]
struct AuthState {
    /// The store's session signal, so a refresh that fails can sign the user out from anywhere.
    session: Mutex<Option<Signal<Session>>>,
    /// A refresh is running: the other requests that meet a 401 wait for it.
    refreshing: AtomicBool,
    /// Bumped whenever the tokens change; a request compares it to know if someone else refreshed.
    epoch: AtomicU64,
}

fn state(ctx: &Ctx) -> &AuthState {
    ctx.runtime().extension::<AuthState>()
}

/// Resets a flag when dropped, so a cancelled call cannot leave it set.
struct Flag<'a>(&'a AtomicBool);

impl Drop for Flag<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Sets a signal to `false` when dropped (the call ended, failed or was cancelled).
struct Busy(Signal<bool>);

impl Busy {
    fn start(signal: &Signal<bool>) -> Busy {
        signal.set(true);
        Busy(signal.clone())
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

async fn secret(ctx: &Ctx, key: &str) -> Result<Option<String>, StorageError> {
    let value = ctx.secure_store().get(key.to_owned()).await?;
    Ok(value.and_then(|bytes| String::from_utf8(bytes.0).ok()))
}

async fn save_tokens(ctx: &Ctx, tokens: &Tokens) -> Result<(), StorageError> {
    let store = ctx.secure_store();
    store
        .set(ACCESS.to_owned(), Bytes(tokens.access.clone().into_bytes()))
        .await?;
    store
        .set(
            REFRESH.to_owned(),
            Bytes(tokens.refresh.clone().into_bytes()),
        )
        .await?;
    state(ctx).epoch.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

/// Forgets the user: both tokens, the persisted cache, and the session the UI shows.
async fn forget(ctx: &Ctx) {
    let store = ctx.secure_store();
    // Best effort: a store that cannot delete is already unusable, and the session must still end.
    let _ = store.delete(ACCESS.to_owned()).await;
    let _ = store.delete(REFRESH.to_owned()).await;
    let kv = ctx.kv();
    if let Ok(keys) = kv.list(CACHE_PREFIX.to_owned()).await {
        for key in keys {
            let _ = kv.delete(key).await;
        }
    }
    let st = state(ctx);
    st.epoch.fetch_add(1, Ordering::SeqCst);
    let session = st
        .session
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if let Some(session) = session {
        session.set(Session::SignedOut);
    }
}

/// Sends `request` as the signed-in user: with the access token, and with one refresh and one retry
/// if the server answers `401`. A refresh the server refuses signs the user out and fails with
/// [`AuthError::SessionExpired`].
pub async fn authed(ctx: &Ctx, request: HttpRequest) -> Result<HttpResponse, AuthError> {
    for attempt in 0..2 {
        let seen = state(ctx).epoch.load(Ordering::SeqCst);
        let Some(token) = secret(ctx, ACCESS).await? else {
            return Err(AuthError::SessionExpired);
        };
        let signed = request
            .clone()
            .with_header("Authorization", format!("Bearer {token}"));
        let response = net::send(ctx, signed).await?;
        if response.status != 401 {
            return Ok(response);
        }
        if attempt == 0 {
            refresh(ctx, seen).await?;
        }
    }
    forget(ctx).await;
    Err(AuthError::SessionExpired)
}

/// Refreshes the tokens, unless somebody already did since `seen`; waits if somebody is doing it.
async fn refresh(ctx: &Ctx, seen: u64) -> Result<(), AuthError> {
    let st = state(ctx);
    if st.epoch.load(Ordering::SeqCst) != seen {
        return Ok(());
    }
    if st.refreshing.swap(true, Ordering::SeqCst) {
        // The core runs on one thread, so this flag is race free. Waiting on the Timer port keeps
        // it deterministic in a test: the fake clock decides when the poll runs.
        while st.refreshing.load(Ordering::SeqCst) {
            ctx.sleep(Duration::from_millis(20)).await;
        }
        return Ok(()); // the retry shows how the refresh went
    }
    let _running = Flag(&st.refreshing);
    let Some(refresh_token) = secret(ctx, REFRESH).await? else {
        forget(ctx).await;
        return Err(AuthError::SessionExpired);
    };
    let url = net::url(ctx, "/auth/refresh")?;
    let response = net::send(
        ctx,
        net::post_json(url, &serde_json::json!({ "refresh": refresh_token })),
    )
    .await?;
    match response.status {
        200..=299 => {
            let tokens: Tokens = net::json(&response.body.0)?;
            save_tokens(ctx, &tokens).await?;
            Ok(())
        }
        // The refresh token was refused: this session is over.
        401 | 403 => {
            forget(ctx).await;
            Err(AuthError::SessionExpired)
        }
        code => Err(NetError::Status { code }.into()),
    }
}

/// The signed-in user's profile. The key carries the user, so the next person to sign in on this
/// device never reads this one's cached entry.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct Profile {
    /// The display name.
    pub name: String,
    /// The email address.
    pub email: String,
}

/// `GET /me`, as `user`: a persisted query that goes through [`authed`], so it survives an expired
/// token without its caller knowing.
#[undra::query(key = "profile:{user}", stale = "60s", persist, retry = 0)]
pub async fn profile(ctx: &Ctx, user: String) -> Result<Profile, AuthError> {
    let _ = &user; // the key's part; the server knows who we are from the token
    let url = net::url(ctx, "/me")?;
    let body = net::ok(authed(ctx, HttpRequest::get(url)).await?)?;
    Ok(net::json(&body)?)
}

/// The login screen's store: the session and whether a call is running.
#[undra::store(restore = "Self::assemble")]
pub struct Auth {
    ctx: WeakCtx,
    session: Signal<Session>,
    busy: Signal<bool>,
}

#[undra::api(store)]
impl Auth {
    /// A store with nobody signed in. Call [`resume`](Auth::resume) at launch.
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(ctx, Signal::new(Session::SignedOut), Signal::new(false))
    }

    // Used by `new` and, through `restore = ".."`, to rebuild the store from a snapshot.
    fn assemble(ctx: Ctx, session: Signal<Session>, busy: Signal<bool>) -> Self {
        *state(&ctx)
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(session.clone());
        Self {
            ctx: ctx.downgrade(),
            session,
            busy,
        }
    }

    /// Picks up a session the last run left in the secure store: asks the server who the token
    /// belongs to (which refreshes it if it expired). No token, or a refused one, leaves the
    /// user signed out.
    pub async fn resume(&self) -> Result<(), AuthError> {
        let ctx = self.ctx.upgrade().map_err(|_| AuthError::Closed)?;
        let _busy = Busy::start(&self.busy);
        if secret(&ctx, ACCESS).await?.is_none() {
            return Ok(());
        }
        let url = net::url(&ctx, "/me")?;
        let response = match authed(&ctx, HttpRequest::get(url)).await {
            Ok(response) => response,
            Err(AuthError::SessionExpired) => return Ok(()),
            Err(error) => return Err(error),
        };
        let me: Profile = net::json(&net::ok(response)?)?;
        self.session.set(Session::SignedIn { user: me.name });
        Ok(())
    }

    /// Signs in with an email and a password: the tokens go to the secure store, the session to
    /// the UI. A wrong password is [`AuthError::BadCredentials`], not a network error.
    pub async fn sign_in(&self, email: String, password: String) -> Result<(), AuthError> {
        let ctx = self.ctx.upgrade().map_err(|_| AuthError::Closed)?;
        let _busy = Busy::start(&self.busy);
        let url = net::url(&ctx, "/auth/login")?;
        let body = serde_json::json!({ "email": email, "password": password });
        let response = net::send(&ctx, net::post_json(url, &body)).await?;
        let tokens: Tokens = match response.status {
            401 | 403 => return Err(AuthError::BadCredentials),
            _ => net::json(&net::ok(response)?)?,
        };
        save_tokens(&ctx, &tokens).await?;
        self.session.set(Session::SignedIn { user: tokens.user });
        Ok(())
    }

    /// Signs out: tells the server (best effort), forgets both tokens and the persisted cache and
    /// shows the login screen. Refused with [`AuthError::PendingWrites`] while writes wait in the
    /// offline queue.
    pub async fn sign_out(&self) -> Result<(), AuthError> {
        let ctx = self.ctx.upgrade().map_err(|_| AuthError::Closed)?;
        let pending = ctx.query().pending_mutations();
        if pending > 0 {
            return Err(AuthError::PendingWrites {
                count: u32::try_from(pending).unwrap_or(u32::MAX),
            });
        }
        let _busy = Busy::start(&self.busy);
        if let Ok(url) = net::url(&ctx, "/auth/logout") {
            // The server may be unreachable: signing out must not depend on it.
            let _ = authed(&ctx, HttpRequest::post(url, Vec::new())).await;
        }
        forget(&ctx).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use undra::ports::fakes::Matcher;
    use undra::query::QueryStatus;

    use super::*;
    use crate::net::testing::{App, BASE, json_response};

    /// A server that issues `access-1`, and accepts only the access token it issued last.
    struct Server {
        valid: Mutex<String>,
        refreshes: AtomicUsize,
        refresh_works: AtomicBool,
    }

    impl Server {
        fn serve(app: &App) -> Arc<Server> {
            let server = Arc::new(Server {
                valid: Mutex::new("access-1".into()),
                refreshes: AtomicUsize::new(0),
                refresh_works: AtomicBool::new(true),
            });
            let s = server.clone();
            app.fakes.http.respond_with(
                Matcher::post(format!("{BASE}/auth/login")),
                move |request| {
                    let body = request.body.as_ref().map(|b| b.0.clone()).unwrap_or_default();
                    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    Ok(if body["password"] == "hunter2" {
                        json_response(
                            200,
                            serde_json::json!({"access": "access-1", "refresh": "refresh-1", "user": "Ada"}),
                        )
                    } else {
                        HttpResponse::new(401, b"no".to_vec())
                    })
                },
            );
            let s2 = s.clone();
            app.fakes
                .http
                .respond_with(Matcher::post(format!("{BASE}/auth/refresh")), move |_| {
                    s2.refreshes.fetch_add(1, Ordering::SeqCst);
                    if s2.refresh_works.load(Ordering::SeqCst) {
                        *s2.valid.lock().unwrap() = "access-2".into();
                        Ok(json_response(
                            200,
                            serde_json::json!({"access": "access-2", "refresh": "refresh-2"}),
                        ))
                    } else {
                        Ok(HttpResponse::new(401, b"revoked".to_vec()))
                    }
                });
            let s3 = s.clone();
            app.fakes
                .http
                .respond_with(Matcher::get(format!("{BASE}/me")), move |request| {
                    let want = format!("Bearer {}", s3.valid.lock().unwrap());
                    Ok(if request.header("Authorization") == Some(want.as_str()) {
                        json_response(
                            200,
                            serde_json::json!({"name": "Ada", "email": "ada@example.com"}),
                        )
                    } else {
                        HttpResponse::new(401, b"expired".to_vec())
                    })
                });
            app.fakes.http.respond(
                Matcher::post(format!("{BASE}/auth/logout")),
                HttpResponse::new(204, Vec::new()),
            );
            s
        }

        /// The server expires the access token the app holds.
        fn expire_access(&self) {
            *self.valid.lock().unwrap() = "access-0".into();
        }
    }

    fn signed_in(app: &App) -> Auth {
        let auth = Auth::new(app.ctx());
        app.run(auth.sign_in("ada@example.com".into(), "hunter2".into()))
            .expect("signs in");
        auth
    }

    #[test]
    fn signing_in_keeps_the_tokens_in_the_secure_store_and_the_session_in_the_store() {
        let app = App::new();
        Server::serve(&app);
        let auth = signed_in(&app);
        assert_eq!(auth.session.get(), Session::SignedIn { user: "Ada".into() });
        assert!(!auth.busy.get());
        assert_eq!(
            app.fakes.secure_store.value(ACCESS),
            Some(b"access-1".to_vec())
        );
        assert_eq!(
            app.fakes.secure_store.value(REFRESH),
            Some(b"refresh-1".to_vec())
        );
        assert!(app.fakes.kv.is_empty(), "no secret reached plain storage");
    }

    #[test]
    fn a_wrong_password_is_a_typed_error_and_changes_nothing() {
        let app = App::new();
        Server::serve(&app);
        let auth = Auth::new(app.ctx());
        let result = app.run(auth.sign_in("ada@example.com".into(), "nope".into()));
        assert_eq!(result, Err(AuthError::BadCredentials));
        assert_eq!(auth.session.get(), Session::SignedOut);
        assert!(app.fakes.secure_store.is_empty());
    }

    #[test]
    fn a_401_refreshes_once_and_retries_without_the_caller_noticing() {
        let app = App::new();
        let server = Server::serve(&app);
        let _auth = signed_in(&app);
        server.expire_access();
        let handle = app
            .ctx()
            .query()
            .observe::<ProfileQuery>(("Ada".to_owned(),));
        app.t.run_pending();
        assert_eq!(handle.status().get(), QueryStatus::Success);
        assert_eq!(
            handle.data().get(),
            Some(Profile {
                name: "Ada".into(),
                email: "ada@example.com".into()
            })
        );
        assert_eq!(server.refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(
            app.fakes.secure_store.value(ACCESS),
            Some(b"access-2".to_vec())
        );
    }

    #[test]
    fn a_refused_refresh_signs_the_user_out_and_clears_everything() {
        let app = App::new();
        let server = Server::serve(&app);
        let auth = signed_in(&app);
        // A cached profile on disk, as a persisted query leaves it.
        app.fakes
            .kv
            .insert(format!("{CACHE_PREFIX}00000001.0000000000000002"), vec![1]);
        server.expire_access();
        server.refresh_works.store(false, Ordering::SeqCst);

        let ctx = app.ctx();
        let result = app.run(authed(&ctx, HttpRequest::get(format!("{BASE}/me"))));
        assert_eq!(result.unwrap_err(), AuthError::SessionExpired);
        assert_eq!(auth.session.get(), Session::SignedOut, "the screen follows");
        assert!(app.fakes.secure_store.is_empty(), "both tokens are gone");
        assert!(app.fakes.kv.is_empty(), "so is the persisted cache");
    }

    #[test]
    fn a_request_that_meets_a_refresh_in_progress_waits_for_it() {
        let app = App::new();
        let server = Server::serve(&app);
        let _auth = signed_in(&app);
        server.expire_access();
        // Another request is refreshing right now.
        state(&app.ctx()).refreshing.store(true, Ordering::SeqCst);
        let answer = Arc::new(Mutex::new(None));
        let slot = answer.clone();
        app.ctx().spawn(async move {
            let ctx = Ctx::current();
            let url = net::url(&ctx, "/me").unwrap();
            *slot.lock().unwrap() = Some(authed(&ctx, HttpRequest::get(url)).await);
        });
        app.advance(100);
        assert!(
            answer.lock().unwrap().is_none(),
            "it waits, it does not refresh a second time"
        );
        assert_eq!(server.refreshes.load(Ordering::SeqCst), 0);
        // The refresh that was running finishes.
        *server.valid.lock().unwrap() = "access-2".into();
        app.t.run_until(async {
            let ctx = Ctx::current();
            ctx.secure_store()
                .set(ACCESS.to_owned(), Bytes(b"access-2".to_vec()))
                .await
                .unwrap();
        });
        state(&app.ctx()).epoch.fetch_add(1, Ordering::SeqCst);
        state(&app.ctx()).refreshing.store(false, Ordering::SeqCst);
        app.advance(100);
        let response = answer
            .lock()
            .unwrap()
            .take()
            .expect("answered")
            .expect("ok");
        assert_eq!(response.status, 200);
        assert_eq!(server.refreshes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn resuming_picks_the_session_up_from_the_secure_store() {
        let app = App::new();
        let server = Server::serve(&app);
        app.fakes.secure_store.insert(ACCESS, b"access-0".to_vec());
        app.fakes
            .secure_store
            .insert(REFRESH, b"refresh-1".to_vec());
        let auth = Auth::new(app.ctx());
        assert_eq!(server.refreshes.load(Ordering::SeqCst), 0);
        app.run(auth.resume()).expect("resumes");
        // The stored access token was stale: it was refreshed on the way.
        assert_eq!(auth.session.get(), Session::SignedIn { user: "Ada".into() });
        assert_eq!(server.refreshes.load(Ordering::SeqCst), 1);
        // With nothing stored, resuming leaves the user signed out.
        let app = App::new();
        Server::serve(&app);
        let auth = Auth::new(app.ctx());
        app.run(auth.resume()).expect("nothing to resume");
        assert_eq!(auth.session.get(), Session::SignedOut);
    }

    #[test]
    fn signing_out_forgets_the_user() {
        let app = App::new();
        Server::serve(&app);
        let auth = signed_in(&app);
        app.run(auth.sign_out()).expect("signs out");
        assert_eq!(auth.session.get(), Session::SignedOut);
        assert!(app.fakes.secure_store.is_empty());
        assert_eq!(
            app.fakes.http.calls().last().map(|c| c.url.clone()),
            Some(format!("{BASE}/auth/logout"))
        );
    }
}
