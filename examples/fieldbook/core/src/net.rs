//! Where the server is and what a failed request is. The app says where once, at start-up
//! ([`configure_server`]); every module builds its URLs with `url` and sends through the `Http` port,
//! which is `URLSession`, OkHttp or `fetch` in the app and `FakeHttp` in a test.

use std::sync::{Mutex, PoisonError};

use serde::Deserialize;
use undra::ports::{HttpError, HttpRequest, HttpResponse};
use undra::prelude::*;

/// Where the server is: what an app supplies once, at start-up.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfig {
    /// The server's address without a trailing slash, such as `https://fieldbook.example.com`.
    pub base_url: String,
}

/// Why a request to the server failed.
#[undra::error]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetError {
    /// `configure_server` was not called.
    #[error("the server is not configured")]
    NotConfigured,
    /// The `Http` port failed: no network, a timeout, a bad URL or a cancelled request.
    #[error("network: {0}")]
    Http(#[from] HttpError),
    /// The server answered with a status that is not a success.
    #[error("the server answered {code}")]
    Status {
        /// The HTTP status code.
        code: u16,
    },
    /// The server's answer is not the JSON this core expects.
    #[error("the response is not what was expected: {0}")]
    BadBody(String),
}

#[derive(Default)]
struct Server {
    base_url: Mutex<Option<String>>,
}

/// Tells the core where the server is. Call it once at start-up.
#[undra::api]
pub fn configure_server(ctx: &Ctx, config: ServerConfig) {
    let base = config.base_url.trim_end_matches('/').to_owned();
    *ctx.runtime()
        .extension::<Server>()
        .base_url
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(base);
}

/// The URL of `path` on the configured server.
pub(crate) fn url(ctx: &Ctx, path: &str) -> Result<String, NetError> {
    let base = ctx
        .runtime()
        .extension::<Server>()
        .base_url
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    base.map(|base| format!("{base}{path}"))
        .ok_or(NetError::NotConfigured)
}

/// Sends `request`; any status is an answer.
pub(crate) async fn send(ctx: &Ctx, request: HttpRequest) -> Result<HttpResponse, NetError> {
    Ok(ctx.http().request(request).await?)
}

/// The body of a successful answer, or the status as a typed error.
pub(crate) fn ok(response: HttpResponse) -> Result<Vec<u8>, NetError> {
    if response.is_success() {
        Ok(response.body.0)
    } else {
        Err(NetError::Status {
            code: response.status,
        })
    }
}

/// Parses the JSON body of an answer.
pub(crate) fn json<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, NetError> {
    serde_json::from_slice(body).map_err(|e| NetError::BadBody(e.to_string()))
}

/// A JSON request with `body`.
pub(crate) fn json_request(
    method: undra::ports::HttpMethod,
    url: String,
    body: &serde_json::Value,
) -> HttpRequest {
    HttpRequest::new(method, url)
        .with_body(body.to_string().into_bytes())
        .with_header("Content-Type", "application/json")
}

/// The runtime and the scripted server every module's tests share.
#[cfg(test)]
pub(crate) mod testing {
    use std::future::Future;
    use std::time::Duration;

    use undra::ports::fakes::{self, Fakes};
    use undra::runtime::testing::TestRuntime;

    use super::*;

    /// The address the tests' server answers on.
    pub(crate) const BASE: &str = "https://fieldbook.test";

    /// A runtime like a started app: every port faked, the server configured.
    pub(crate) struct App {
        pub(crate) t: TestRuntime,
        pub(crate) fakes: Fakes,
    }

    impl App {
        pub(crate) fn new() -> App {
            let t = TestRuntime::new();
            let fakes = fakes::install(&t);
            App::boot(t, fakes)
        }

        /// A second process over the same storage: what a relaunch is.
        pub(crate) fn relaunch(fakes: &Fakes) -> App {
            let t = TestRuntime::new();
            fakes.install_test(&t);
            App::boot(t, fakes.clone())
        }

        fn boot(t: TestRuntime, fakes: Fakes) -> App {
            t.run_init_hooks();
            configure_server(
                &t.ctx(),
                ServerConfig {
                    base_url: BASE.into(),
                },
            );
            t.run_pending();
            App { t, fakes }
        }

        pub(crate) fn ctx(&self) -> Ctx {
            self.t.ctx()
        }

        pub(crate) fn run<F: Future>(&self, future: F) -> F::Output {
            self.t.run_until(future)
        }

        pub(crate) fn advance(&self, ms: u64) {
            self.fakes.advance(&self.t, Duration::from_millis(ms));
        }
    }

    /// A JSON answer.
    pub(crate) fn json_response(status: u16, value: serde_json::Value) -> HttpResponse {
        HttpResponse::new(status, value.to_string().into_bytes())
    }
}
