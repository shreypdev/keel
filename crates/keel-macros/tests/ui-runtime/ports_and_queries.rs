//! Ports (request/reply, sync, event, and a Rust fake) and queries with mutations.
#![forbid(unsafe_code)]

use std::sync::Mutex;

use keel::prelude::*;

#[keel::error]
#[derive(Clone, PartialEq)]
pub enum HttpError {
    #[error("network error: {0}")]
    Network(String),
    #[error("request timed out")]
    Timeout,
}

/// A port that cannot answer is reported as the method's error.
impl From<keel::runtime::PortError> for HttpError {
    fn from(error: keel::runtime::PortError) -> Self {
        HttpError::Network(error.to_string())
    }
}

#[keel::api]
#[derive(Clone, Debug, PartialEq)]
pub struct HttpRequest {
    pub url: String,
    pub timeout_ms: Option<u32>,
}

#[keel::api]
#[derive(Clone, Debug, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Bytes,
}

#[keel::api]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NetKind {
    Wifi,
    Cellular,
    None,
}

#[keel::port]
pub trait Http {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>;
}

#[keel::port(sync)]
pub trait Clock {
    fn now_ms(&self) -> i64;
    fn monotonic_ns(&self) -> u64;
}

#[keel::port(event)]
pub trait Connectivity {
    fn changed(&self, online: bool, kind: NetKind);
}

pub struct FakeHttp {
    calls: Mutex<Vec<String>>,
}

#[keel::port]
impl Http for FakeHttp {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        self.calls.lock().unwrap().push(req.url);
        Ok(HttpResponse {
            status: 200,
            body: Bytes(Vec::new()),
        })
    }
}

pub fn use_ports(ctx: &Ctx) {
    let _http = http(ctx);
    let _clock = clock(ctx);
    let _subscription = on_connectivity_changed(ctx, |online, kind| {
        let _ = (online, kind);
    });
    let _payload = encode_connectivity_changed_event(true, NetKind::Wifi);
    let _ = HttpProxy::new(ctx.clone());
}

#[keel::query(key = "todos:{page}", stale = "30s", persist, retry = 3)]
pub async fn todos(ctx: &Ctx, page: u32) -> Result<Vec<String>, HttpError> {
    let response = http(ctx)
        .request(HttpRequest {
            url: format!("/todos?page={page}"),
            timeout_ms: None,
        })
        .await?;
    Ok(vec![response.status.to_string()])
}

#[keel::mutation(idempotent)]
pub async fn add_todo(ctx: &Ctx, title: String) -> Result<String, HttpError> {
    let _ = ctx;
    Ok(title)
}

fn main() {}
