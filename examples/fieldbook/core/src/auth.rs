//! Sign-in: the session the UI shows, the tokens in `SecureStore`, and [`authed`], the one way
//! the rest of the core talks to a protected endpoint.
//!
//! Same recipe as the cookbook's auth page (`examples/cookbook`), slimmed: the token is added to every
//! request, a `401` refreshes it once and retries, and a refresh the server refuses signs the user out.
//! Queued writes call [`authed`] when they replay, so they go out with the token that is current then.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use serde::Deserialize;
use undra::ports::{HttpMethod, HttpRequest, HttpResponse, StorageError};
use undra::prelude::*;

use crate::net::{self, NetError};

const ACCESS: &str = "fieldbook.access";
const REFRESH: &str = "fieldbook.refresh";

/// Who is signed in.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Session {
    /// Nobody: show the sign-in screen.
    SignedOut,
    /// A member of the field team.
    SignedIn {
        /// The name the server knows the member by.
        user: String,
    },
}

/// Why an auth call failed.
#[undra::error]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthError {
    /// The server refused the name or the team code.
    #[error("wrong name or team code")]
    BadCredentials,
    /// The session ended and refreshing it was refused: sign in again.
    #[error("the session ended, sign in again")]
    SessionExpired,
    /// Sign-out was refused: notes are still waiting to be sent, and would go out as the next person.
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

#[derive(Deserialize)]
struct Tokens {
    access: String,
    refresh: String,
    #[serde(default)]
    user: String,
}

#[derive(Default)]
struct AuthState {
    /// The store's signal, so a refused refresh can sign the user out from anywhere.
    session: Mutex<Option<Signal<Session>>>,
    /// Bumped whenever the tokens change.
    epoch: AtomicU64,
}

fn state(ctx: &Ctx) -> &AuthState {
    ctx.runtime().extension::<AuthState>()
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

/// Forgets the member: both tokens and the session the UI shows. The notes stay: they are the
/// member's work, on the device, and signing in again sends what is still waiting.
async fn forget(ctx: &Ctx) {
    let store = ctx.secure_store();
    let _ = store.delete(ACCESS.to_owned()).await;
    let _ = store.delete(REFRESH.to_owned()).await;
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

/// Sends `request` as the signed-in member: with the access token, and with one refresh and one
/// retry if the server answers `401`.
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
        // Somebody else may have refreshed while this request was in flight: then just retry.
        if attempt == 0 && state(ctx).epoch.load(Ordering::SeqCst) == seen {
            refresh(ctx).await?;
        }
    }
    forget(ctx).await;
    Err(AuthError::SessionExpired)
}

async fn refresh(ctx: &Ctx) -> Result<(), AuthError> {
    let Some(refresh_token) = secret(ctx, REFRESH).await? else {
        forget(ctx).await;
        return Err(AuthError::SessionExpired);
    };
    let url = net::url(ctx, "/auth/refresh")?;
    let body = serde_json::json!({ "refresh": refresh_token });
    let response = net::send(ctx, net::json_request(HttpMethod::Post, url, &body)).await?;
    match response.status {
        200..=299 => {
            let tokens: Tokens = net::json(&response.body.0)?;
            save_tokens(ctx, &tokens).await?;
            Ok(())
        }
        401 | 403 => {
            forget(ctx).await;
            Err(AuthError::SessionExpired)
        }
        code => Err(NetError::Status { code }.into()),
    }
}

/// The sign-in screen's store.
#[undra::store(restore = "Self::assemble")]
pub struct Auth {
    ctx: WeakCtx,
    session: Signal<Session>,
    busy: Signal<bool>,
}

#[undra::api(store)]
impl Auth {
    /// A store with nobody signed in. Call `resume` at launch.
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(ctx, Signal::new(Session::SignedOut), Signal::new(false))
    }

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

    /// Picks up the session the last run left in the secure store (refreshing it if it expired).
    /// No token, or a refused one, leaves the member signed out.
    pub async fn resume(&self) -> Result<(), AuthError> {
        let ctx = self.ctx.upgrade().map_err(|_| AuthError::Closed)?;
        if secret(&ctx, ACCESS).await?.is_none() {
            return Ok(());
        }
        let url = net::url(&ctx, "/me")?;
        let response = match authed(&ctx, HttpRequest::get(url)).await {
            Ok(response) => response,
            Err(AuthError::SessionExpired) => return Ok(()),
            Err(error) => return Err(error),
        };
        #[derive(Deserialize)]
        struct Me {
            name: String,
        }
        let me: Me = net::json(&net::ok(response)?)?;
        self.session.set(Session::SignedIn { user: me.name });
        Ok(())
    }

    /// Signs in with a name and the team's code. A refusal is `BadCredentials`, not a
    /// network error.
    pub async fn sign_in(&self, name: String, code: String) -> Result<(), AuthError> {
        let ctx = self.ctx.upgrade().map_err(|_| AuthError::Closed)?;
        self.busy.set(true);
        let result = async {
            let url = net::url(&ctx, "/auth/login")?;
            let body = serde_json::json!({ "name": name, "code": code });
            let response = net::send(&ctx, net::json_request(HttpMethod::Post, url, &body)).await?;
            let tokens: Tokens = match response.status {
                401 | 403 => return Err(AuthError::BadCredentials),
                _ => net::json(&net::ok(response)?)?,
            };
            save_tokens(&ctx, &tokens).await?;
            self.session.set(Session::SignedIn { user: tokens.user });
            Ok(())
        }
        .await;
        self.busy.set(false);
        result
    }

    /// Signs out. Refused with `PendingWrites` while notes wait in the offline queue.
    pub async fn sign_out(&self) -> Result<(), AuthError> {
        let ctx = self.ctx.upgrade().map_err(|_| AuthError::Closed)?;
        let pending = ctx.query().pending_mutations();
        if pending > 0 {
            return Err(AuthError::PendingWrites {
                count: u32::try_from(pending).unwrap_or(u32::MAX),
            });
        }
        forget(&ctx).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use undra::ports::fakes::Matcher;

    use super::*;
    use crate::net::testing::{App, BASE, json_response};

    /// A server that issues `access-1` and accepts only the access token it issued last.
    pub(crate) fn serve(app: &App) -> std::sync::Arc<Mutex<String>> {
        let valid = std::sync::Arc::new(Mutex::new("access-1".to_owned()));
        app.fakes.http.respond(
            Matcher::post(format!("{BASE}/auth/login")),
            json_response(
                200,
                serde_json::json!({"access": "access-1", "refresh": "refresh-1", "user": "Ada"}),
            ),
        );
        let on_refresh = valid.clone();
        app.fakes
            .http
            .respond_with(Matcher::post(format!("{BASE}/auth/refresh")), move |_| {
                *on_refresh.lock().unwrap() = "access-2".into();
                Ok(json_response(
                    200,
                    serde_json::json!({"access": "access-2", "refresh": "refresh-2"}),
                ))
            });
        let on_me = valid.clone();
        app.fakes
            .http
            .respond_with(Matcher::get(format!("{BASE}/me")), move |request| {
                let want = format!("Bearer {}", on_me.lock().unwrap());
                Ok(if request.header("Authorization") == Some(want.as_str()) {
                    json_response(200, serde_json::json!({"name": "Ada"}))
                } else {
                    HttpResponse::new(401, b"expired".to_vec())
                })
            });
        valid
    }

    #[test]
    fn signing_in_keeps_the_tokens_in_the_secure_store() {
        let app = App::new();
        serve(&app);
        let auth = Auth::new(app.ctx());
        app.run(auth.sign_in("Ada".into(), "team-7".into()))
            .unwrap();
        assert_eq!(auth.session.get(), Session::SignedIn { user: "Ada".into() });
        assert_eq!(
            app.fakes.secure_store.value(ACCESS),
            Some(b"access-1".to_vec())
        );
        assert!(app.fakes.kv.is_empty(), "no secret reached plain storage");
    }

    #[test]
    fn a_refused_code_is_a_typed_error() {
        let app = App::new();
        app.fakes
            .http
            .respond(Matcher::any(), HttpResponse::new(401, b"no".to_vec()));
        let auth = Auth::new(app.ctx());
        assert_eq!(
            app.run(auth.sign_in("Ada".into(), "wrong".into())),
            Err(AuthError::BadCredentials)
        );
        assert_eq!(auth.session.get(), Session::SignedOut);
    }

    #[test]
    fn an_expired_token_is_refreshed_once_and_the_request_retried() {
        let app = App::new();
        let valid = serve(&app);
        let auth = Auth::new(app.ctx());
        app.run(auth.sign_in("Ada".into(), "team-7".into()))
            .unwrap();
        *valid.lock().unwrap() = "access-0".into(); // the server expires what the app holds
        let ctx = app.ctx();
        let response = app
            .run(authed(&ctx, HttpRequest::get(format!("{BASE}/me"))))
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(
            app.fakes.secure_store.value(ACCESS),
            Some(b"access-2".to_vec())
        );
    }

    #[test]
    fn a_session_left_by_the_last_run_resumes_and_a_dead_one_signs_out() {
        let app = App::new();
        serve(&app);
        app.fakes.secure_store.insert(ACCESS, b"access-1".to_vec());
        let auth = Auth::new(app.ctx());
        app.run(auth.resume()).unwrap();
        assert_eq!(auth.session.get(), Session::SignedIn { user: "Ada".into() });

        // Both tokens dead: the refresh is refused and everything is forgotten.
        let app = App::new();
        app.fakes.secure_store.insert(ACCESS, b"old".to_vec());
        app.fakes.secure_store.insert(REFRESH, b"old".to_vec());
        app.fakes
            .http
            .respond(Matcher::any(), HttpResponse::new(401, b"no".to_vec()));
        let auth = Auth::new(app.ctx());
        app.run(auth.resume()).unwrap();
        assert_eq!(auth.session.get(), Session::SignedOut);
        assert!(app.fakes.secure_store.is_empty());
    }

    #[test]
    fn signing_out_forgets_the_tokens() {
        let app = App::new();
        serve(&app);
        let auth = Auth::new(app.ctx());
        app.run(auth.sign_in("Ada".into(), "team-7".into()))
            .unwrap();
        app.run(auth.sign_out()).unwrap();
        assert_eq!(auth.session.get(), Session::SignedOut);
        assert!(app.fakes.secure_store.is_empty());
    }
}
