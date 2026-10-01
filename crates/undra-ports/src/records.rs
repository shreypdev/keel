//! The records, enums and errors the standard ports exchange (SPEC 8).
//!
//! Every platform runtime (Swift, Kotlin, TypeScript) hand-writes codecs for these types, so
//! their wire layout is a contract: **field order, variant order and variant indices must not
//! change** without an ADR (CLAUDE.md R7, R11). `tests/encoding.rs` locks the exact bytes.
//!
//! | Type | Wire form |
//! |---|---|
//! | [`HttpMethod`] | `u16` index: `Get` 0, `Post` 1, `Put` 2, `Delete` 3, `Patch` 4, `Head` 5, `Options` 6 |
//! | [`Header`] | `name String, value String` |
//! | [`HttpRequest`] | `method, url String, headers Vec<Header>, body Option<Bytes>, timeout_ms Option<u32>` |
//! | [`HttpResponse`] | `status u16, headers Vec<Header>, body Bytes` |
//! | [`HttpError`] | `u16` index: `Network(String)` 0, `Timeout` 1, `Cancelled` 2, `InvalidUrl(String)` 3 |
//! | [`FsError`] | `u16` index: `NotFound` 0, `Denied` 1, `Io(String)` 2 |
//! | [`NetKind`] | `u16` index: `Wifi` 0, `Cellular` 1, `Wired` 2, `Unknown` 3, `None` 4 |
//! | [`AppState`] | `u16` index: `Active` 0, `Inactive` 1, `Background` 2 |

use core::fmt;

use undra_runtime::PortError;
use undra_wire::{Bytes, Decode};

/// The method of an `HttpRequest`.
// SPEC 8 names this type without listing its variants; the three platform runtimes number
// them in the order below, so this declaration order is the wire contract.
#[undra_macros::api]
#[undra(crate = "crate::root")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    /// `GET`.
    Get,
    /// `POST`.
    Post,
    /// `PUT`.
    Put,
    /// `DELETE`.
    Delete,
    /// `PATCH`.
    Patch,
    /// `HEAD`.
    Head,
    /// `OPTIONS`.
    Options,
}

impl HttpMethod {
    /// The method as written in an HTTP request line: `"GET"`, `"POST"`, ...
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Head => "HEAD",
            HttpMethod::Options => "OPTIONS",
        }
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One HTTP header. Repeated header names are repeated entries.
#[undra_macros::api]
#[undra(crate = "crate::root")]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Header {
    /// The header name, as the sender spelled it.
    pub name: String,
    /// The header value.
    pub value: String,
}

impl Header {
    /// Creates a header.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Header {
        Header {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// An HTTP request the core asks the platform to perform.
#[undra_macros::api]
#[undra(crate = "crate::root")]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HttpRequest {
    /// The HTTP method.
    pub method: HttpMethod,
    /// The absolute URL.
    pub url: String,
    /// Request headers, in order.
    pub headers: Vec<Header>,
    /// The request body, if any.
    pub body: Option<Bytes>,
    /// The timeout of the whole request in milliseconds; absent leaves it to the platform.
    pub timeout_ms: Option<u32>,
}

impl HttpRequest {
    /// A request with no headers, no body and no timeout.
    ///
    /// ```
    /// use undra_ports::{HttpMethod, HttpRequest};
    ///
    /// let request = HttpRequest::new(HttpMethod::Post, "https://example.com/todos")
    ///     .with_header("content-type", "application/json")
    ///     .with_body(br#"{"title":"milk"}"#.to_vec())
    ///     .with_timeout_ms(5_000);
    /// assert_eq!(request.method, HttpMethod::Post);
    /// assert_eq!(request.timeout_ms, Some(5_000));
    /// ```
    pub fn new(method: HttpMethod, url: impl Into<String>) -> HttpRequest {
        HttpRequest {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout_ms: None,
        }
    }

    /// A `GET` request for `url`.
    pub fn get(url: impl Into<String>) -> HttpRequest {
        HttpRequest::new(HttpMethod::Get, url)
    }

    /// A `POST` request for `url` with `body`.
    pub fn post(url: impl Into<String>, body: impl Into<Bytes>) -> HttpRequest {
        HttpRequest::new(HttpMethod::Post, url).with_body(body)
    }

    /// Appends a header.
    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> HttpRequest {
        self.headers.push(Header::new(name, value));
        self
    }

    /// Sets the body.
    #[must_use]
    pub fn with_body(mut self, body: impl Into<Bytes>) -> HttpRequest {
        self.body = Some(body.into());
        self
    }

    /// Sets the timeout in milliseconds.
    #[must_use]
    pub fn with_timeout_ms(mut self, timeout_ms: u32) -> HttpRequest {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    /// The value of the first header called `name`, compared case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        find_header(&self.headers, name)
    }
}

/// An HTTP response. Any status, including 4xx and 5xx, is a response; only failures before a
/// response exists are an `HttpError`.
#[undra_macros::api]
#[undra(crate = "crate::root")]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HttpResponse {
    /// The HTTP status code.
    pub status: u16,
    /// Response headers, in the order the platform reports them.
    pub headers: Vec<Header>,
    /// The response body.
    pub body: Bytes,
}

impl HttpResponse {
    /// A response with `status`, no headers and `body`.
    ///
    /// ```
    /// use undra_ports::HttpResponse;
    ///
    /// let response = HttpResponse::new(200, b"ok".to_vec()).with_header("Content-Type", "text/plain");
    /// assert!(response.is_success());
    /// assert_eq!(response.header("content-type"), Some("text/plain"));
    /// ```
    pub fn new(status: u16, body: impl Into<Bytes>) -> HttpResponse {
        HttpResponse {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    /// Appends a header.
    #[must_use]
    pub fn with_header(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> HttpResponse {
        self.headers.push(Header::new(name, value));
        self
    }

    /// Whether the status is 2xx.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The value of the first header called `name`, compared case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        find_header(&self.headers, name)
    }
}

fn find_header<'a>(headers: &'a [Header], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

/// Why an HTTP request failed before a response existed.
#[undra_macros::error]
#[undra(crate = "crate::root")]
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum HttpError {
    /// The connection failed (DNS, refused, reset, TLS, ...); the text is the platform's.
    #[error("network error: {0}")]
    Network(String),
    /// The request took longer than its timeout.
    #[error("the request timed out")]
    Timeout,
    /// The request was cancelled before it finished.
    #[error("the request was cancelled")]
    Cancelled,
    /// The URL (or a header) could not be used; the text names it.
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
}

/// Why a file operation failed.
#[undra_macros::error]
#[undra(crate = "crate::root")]
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum FsError {
    /// The file or directory does not exist.
    #[error("not found")]
    NotFound,
    /// Access is not allowed: a permission error, or a path that leaves the file root.
    #[error("access denied")]
    Denied,
    /// Any other I/O failure; the text is the platform's.
    #[error("I/O error: {0}")]
    Io(String),
}

/// The text of an unavailable port as a typed error: names the port and carries the code and the
/// docs link of E0062, like the runtime message of a method that has no error channel to put it in
/// (generated code, `undra-macros`).
fn no_adapter(port: &str) -> String {
    format!(
        "the {port} port has no adapter registered (E0062: register one, see https://shreypdev.github.io/undra/docs/errors.html#E0062)"
    )
}

/// A port that cannot answer is an ordinary outcome (SPEC 6.3), not a bug: a platform that does
/// not register `Http` answers "unavailable". `HttpProxy::request` therefore returns the outcome
/// as an error instead of panicking (which would trap a wasm core):
///
/// | `PortError` | `HttpError` |
/// |---|---|
/// | `Unavailable` | `Network("the Http port has no adapter registered (E0062: ..)")` |
/// | `Cancelled` | `Cancelled` |
/// | `Decode(e)` | `Network("malformed port reply: <e>")` |
/// | `Failed(bytes)` | the decoded `HttpError`, else `Network("the Http port reported an error that does not decode")` |
impl From<PortError> for HttpError {
    fn from(error: PortError) -> Self {
        if let PortError::Failed(bytes) = &error {
            return match HttpError::decode_exact(bytes) {
                Ok(typed) => typed,
                Err(_) => HttpError::Network(
                    "the Http port reported an error that does not decode".to_owned(),
                ),
            };
        }
        match error {
            PortError::Unavailable => HttpError::Network(no_adapter("Http")),
            PortError::Cancelled => HttpError::Cancelled,
            PortError::Decode(why) => HttpError::Network(format!("malformed port reply: {why}")),
            other => HttpError::Network(format!("the Http port call failed: {other}")),
        }
    }
}

/// A port that cannot answer is an ordinary outcome (SPEC 6.3), not a bug: a platform without a
/// file system (or a web page without the File System Access API) answers "unavailable".
/// `FsProxy`'s methods return the outcome as an error instead of panicking:
///
/// | `PortError` | `FsError` |
/// |---|---|
/// | `Unavailable` | `Io("the Fs port has no adapter registered (E0062: ..)")` |
/// | `Cancelled` | `Io("the Fs call was cancelled")` |
/// | `Decode(e)` | `Io("malformed port reply: <e>")` |
/// | `Failed(bytes)` | the decoded `FsError`, else `Io("the Fs port reported an error that does not decode")` |
impl From<PortError> for FsError {
    fn from(error: PortError) -> Self {
        if let PortError::Failed(bytes) = &error {
            return match FsError::decode_exact(bytes) {
                Ok(typed) => typed,
                Err(_) => {
                    FsError::Io("the Fs port reported an error that does not decode".to_owned())
                }
            };
        }
        match error {
            PortError::Unavailable => FsError::Io(no_adapter("Fs")),
            PortError::Cancelled => FsError::Io("the Fs call was cancelled".to_owned()),
            PortError::Decode(why) => FsError::Io(format!("malformed port reply: {why}")),
            other => FsError::Io(format!("the Fs port call failed: {other}")),
        }
    }
}

/// What kind of network the device is on.
#[undra_macros::api]
#[undra(crate = "crate::root")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NetKind {
    /// Wi-Fi.
    Wifi,
    /// Mobile data.
    Cellular,
    /// Ethernet or another wired link.
    Wired,
    /// Connected, but the kind is not known.
    Unknown,
    /// No network.
    None,
}

/// Where the app is in its lifecycle.
// SPEC 8 lists the type as `Active, Inactive, Background` (its trait comment lists
// `Active | Background | Inactive`); the platform runtimes and this crate follow the type
// definition.
#[undra_macros::api]
#[undra(crate = "crate::root")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AppState {
    /// In the foreground and receiving input.
    Active,
    /// In the foreground but not receiving input (an interruption is showing).
    Inactive,
    /// Not visible.
    Background,
}

#[cfg(test)]
mod tests {
    use super::*;
    use undra_wire::{Decode, Encode};

    #[test]
    fn method_names_and_display() {
        let all = [
            (HttpMethod::Get, "GET"),
            (HttpMethod::Post, "POST"),
            (HttpMethod::Put, "PUT"),
            (HttpMethod::Delete, "DELETE"),
            (HttpMethod::Patch, "PATCH"),
            (HttpMethod::Head, "HEAD"),
            (HttpMethod::Options, "OPTIONS"),
        ];
        for (method, name) in all {
            assert_eq!(method.as_str(), name);
            assert_eq!(method.to_string(), name);
        }
    }

    #[test]
    fn request_builders_compose() {
        let request = HttpRequest::post("https://x.test/a", vec![1, 2])
            .with_header("Accept", "*/*")
            .with_header("accept", "text/plain")
            .with_timeout_ms(30);
        assert_eq!(request.method, HttpMethod::Post);
        assert_eq!(request.body, Some(Bytes(vec![1, 2])));
        assert_eq!(request.timeout_ms, Some(30));
        assert_eq!(request.headers.len(), 2);
        assert_eq!(request.header("ACCEPT"), Some("*/*"), "first match wins");
        assert_eq!(request.header("missing"), None);
        let get = HttpRequest::get("u");
        assert_eq!(
            (get.method, get.body, get.timeout_ms),
            (HttpMethod::Get, None, None)
        );
    }

    #[test]
    fn response_helpers() {
        assert!(HttpResponse::new(204, Vec::new()).is_success());
        assert!(!HttpResponse::new(404, Vec::new()).is_success());
        assert!(!HttpResponse::new(199, Vec::new()).is_success());
        assert!(!HttpResponse::new(300, Vec::new()).is_success());
        let response = HttpResponse::new(200, vec![]).with_header("ETag", "\"a\"");
        assert_eq!(response.header("etag"), Some("\"a\""));
    }

    #[test]
    fn errors_display_like_the_platform_runtimes() {
        assert_eq!(
            HttpError::Network("dns".into()).to_string(),
            "network error: dns"
        );
        assert_eq!(HttpError::Timeout.to_string(), "the request timed out");
        assert_eq!(
            HttpError::Cancelled.to_string(),
            "the request was cancelled"
        );
        assert_eq!(
            HttpError::InvalidUrl("x".into()).to_string(),
            "invalid URL: x"
        );
        assert_eq!(FsError::NotFound.to_string(), "not found");
        assert_eq!(FsError::Denied.to_string(), "access denied");
        assert_eq!(FsError::Io("disk".into()).to_string(), "I/O error: disk");
        let boxed: Box<dyn std::error::Error> = Box::new(HttpError::Timeout);
        assert_eq!(boxed.to_string(), "the request timed out");
    }

    #[test]
    fn port_errors_map_onto_the_typed_errors() {
        use undra_wire::WireError;

        let bad = WireError::InvalidTag {
            tag: 9,
            at: 3,
            ty: "HttpResponse",
        };
        assert_eq!(
            HttpError::from(PortError::Unavailable),
            HttpError::Network(
                "the Http port has no adapter registered (E0062: register one, see https://shreypdev.github.io/undra/docs/errors.html#E0062)".into()
            )
        );
        assert_eq!(HttpError::from(PortError::Cancelled), HttpError::Cancelled);
        assert_eq!(
            HttpError::from(PortError::Decode(bad)),
            HttpError::Network(format!("malformed port reply: {bad}"))
        );
        // `Failed` carries the encoded error: a typed one comes back as itself, garbage as text.
        assert_eq!(
            HttpError::from(PortError::Failed(HttpError::Timeout.encode_to_vec())),
            HttpError::Timeout
        );
        assert_eq!(
            HttpError::from(PortError::Failed(vec![0xff, 0xff])),
            HttpError::Network("the Http port reported an error that does not decode".into())
        );

        assert_eq!(
            FsError::from(PortError::Unavailable),
            FsError::Io(
                "the Fs port has no adapter registered (E0062: register one, see https://shreypdev.github.io/undra/docs/errors.html#E0062)".into()
            )
        );
        assert_eq!(
            FsError::from(PortError::Cancelled),
            FsError::Io("the Fs call was cancelled".into())
        );
        assert_eq!(
            FsError::from(PortError::Decode(bad)),
            FsError::Io(format!("malformed port reply: {bad}"))
        );
        assert_eq!(
            FsError::from(PortError::Failed(FsError::Denied.encode_to_vec())),
            FsError::Denied
        );
        assert_eq!(
            FsError::from(PortError::Failed(vec![9])),
            FsError::Io("the Fs port reported an error that does not decode".into())
        );
    }

    #[test]
    fn every_unit_enum_variant_round_trips() {
        for kind in [
            NetKind::Wifi,
            NetKind::Cellular,
            NetKind::Wired,
            NetKind::Unknown,
            NetKind::None,
        ] {
            assert_eq!(NetKind::decode_exact(&kind.encode_to_vec()), Ok(kind));
        }
        for state in [AppState::Active, AppState::Inactive, AppState::Background] {
            assert_eq!(AppState::decode_exact(&state.encode_to_vec()), Ok(state));
        }
    }

    #[test]
    fn unknown_variant_indices_are_wire_errors() {
        assert!(HttpMethod::decode_exact(&[7, 0]).is_err());
        assert!(NetKind::decode_exact(&[5, 0]).is_err());
        assert!(AppState::decode_exact(&[3, 0]).is_err());
        assert!(HttpError::decode_exact(&[4, 0]).is_err());
        assert!(FsError::decode_exact(&[3, 0]).is_err());
    }
}
