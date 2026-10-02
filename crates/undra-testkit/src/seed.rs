//! [`Seed`]: the starting state of the deterministic fakes, as one JSON document.
//!
//! The same document seeds `undra::ports::fakes` here and the fakes of the Swift, Kotlin and
//! TypeScript testing kits (`PreviewCore.load(seed:)`), so a preview, a unit test and a Rust test
//! start from the same world.
//!
//! ```json
//! {
//!   "version": 1,
//!   "now_ms": 1700000000000,
//!   "rng_seed": 42,
//!   "kv": { "todos/1": "buy milk", "blob": { "hex": "00ff" } },
//!   "secure_store": { "token": "t-123" },
//!   "fs": { "notes/a.txt": "hello" },
//!   "http": [
//!     { "url": "https://api.test/todos", "method": "get", "status": 200,
//!       "headers": [["content-type", "application/json"]], "body": "[]" },
//!     { "url_prefix": "https://api.test/slow", "error": "timeout" }
//!   ],
//!   "connectivity": { "online": true, "kind": "wifi" },
//!   "lifecycle": "active"
//! }
//! ```
//!
//! A string value is its UTF-8 bytes; `{"hex": ".."}` is raw bytes. Every key is optional. HTTP
//! rules are tried in order and the first match answers, as in `FakeHttp`; a rule with an `error`
//! (`"timeout"`, `"cancelled"`, `{"network": ".."}`, `{"invalid_url": ".."}`) fails instead of
//! answering.

use core::fmt;

use serde_json::Value;
use undra_ports::fakes::{Fakes, Matcher};
use undra_ports::{AppState, Header, HttpError, HttpMethod, HttpResponse, NetKind};

use crate::hex;

/// The `version` of the seed document this crate reads.
pub const SEED_VERSION: u64 = 1;

/// One scripted HTTP rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRule {
    /// Exact URL, if the rule names one.
    pub url: Option<String>,
    /// URL prefix, if the rule names one.
    pub url_prefix: Option<String>,
    /// Method, if the rule names one.
    pub method: Option<HttpMethod>,
    /// What the rule answers.
    pub reply: Result<HttpResponse, HttpError>,
}

/// The starting state of the fakes. Every field is optional: `None` and empty leave the fake as
/// [`Fakes::new`] made it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Seed {
    /// The wall clock, milliseconds since the Unix epoch.
    pub now_ms: Option<i64>,
    /// The seed of the random generator.
    pub rng_seed: Option<u64>,
    /// `Kv` entries.
    pub kv: Vec<(String, Vec<u8>)>,
    /// `SecureStore` entries.
    pub secure_store: Vec<(String, Vec<u8>)>,
    /// Files of the `Fs`.
    pub fs: Vec<(String, Vec<u8>)>,
    /// `Http` rules, in order.
    pub http: Vec<HttpRule>,
    /// The connectivity the app starts in.
    pub connectivity: Option<(bool, NetKind)>,
    /// The lifecycle state the app starts in.
    pub lifecycle: Option<AppState>,
}

/// Why a seed could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SeedError {
    /// The text is not JSON.
    Json(String),
    /// `version` is present and not [`SEED_VERSION`].
    UnsupportedVersion(u64),
    /// A value is missing or malformed; `path` says where (`http[1].status`).
    Invalid {
        /// Where in the document.
        path: String,
        /// What is wrong.
        problem: String,
    },
}

impl fmt::Display for SeedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SeedError::Json(e) => write!(f, "the seed is not valid JSON: {e}"),
            SeedError::UnsupportedVersion(v) => {
                write!(
                    f,
                    "seed version {v} is not supported (this reader knows {SEED_VERSION})"
                )
            }
            SeedError::Invalid { path, problem } => write!(f, "seed {path}: {problem}"),
        }
    }
}

impl std::error::Error for SeedError {}

fn invalid(path: &str, problem: &str) -> SeedError {
    SeedError::Invalid {
        path: path.to_owned(),
        problem: problem.to_owned(),
    }
}

/// A string is its UTF-8 bytes, `{"hex": ".."}` is raw bytes.
fn bytes_of(path: &str, v: &Value) -> Result<Vec<u8>, SeedError> {
    match v {
        Value::String(s) => Ok(s.as_bytes().to_vec()),
        Value::Object(o) => o
            .get("hex")
            .and_then(Value::as_str)
            .and_then(hex::decode)
            .ok_or_else(|| invalid(path, "an object value must be {\"hex\": \"..\"}")),
        _ => Err(invalid(path, "must be a string or {\"hex\": \"..\"}")),
    }
}

fn entries(doc: &Value, key: &str) -> Result<Vec<(String, Vec<u8>)>, SeedError> {
    let Some(v) = doc.get(key) else {
        return Ok(Vec::new());
    };
    let map = v
        .as_object()
        .ok_or_else(|| invalid(key, "must be an object"))?;
    map.iter()
        .map(|(k, v)| Ok((k.clone(), bytes_of(&format!("{key}.{k}"), v)?)))
        .collect()
}

const METHODS: [(&str, HttpMethod); 7] = [
    ("get", HttpMethod::Get),
    ("post", HttpMethod::Post),
    ("put", HttpMethod::Put),
    ("delete", HttpMethod::Delete),
    ("patch", HttpMethod::Patch),
    ("head", HttpMethod::Head),
    ("options", HttpMethod::Options),
];

const KINDS: [(&str, NetKind); 5] = [
    ("wifi", NetKind::Wifi),
    ("cellular", NetKind::Cellular),
    ("wired", NetKind::Wired),
    ("unknown", NetKind::Unknown),
    ("none", NetKind::None),
];

const STATES: [(&str, AppState); 3] = [
    ("active", AppState::Active),
    ("inactive", AppState::Inactive),
    ("background", AppState::Background),
];

fn named<T: Copy>(table: &[(&str, T)], path: &str, v: &Value) -> Result<T, SeedError> {
    let name = v
        .as_str()
        .ok_or_else(|| invalid(path, "must be a string"))?;
    table
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, t)| *t)
        .ok_or_else(|| {
            let names: Vec<&str> = table.iter().map(|(n, _)| *n).collect();
            invalid(path, &format!("is not one of {}", names.join(", ")))
        })
}

fn http_error(path: &str, v: &Value) -> Result<HttpError, SeedError> {
    match v {
        Value::String(s) if s == "timeout" => Ok(HttpError::Timeout),
        Value::String(s) if s == "cancelled" => Ok(HttpError::Cancelled),
        Value::Object(o) => {
            if let Some(m) = o.get("network").and_then(Value::as_str) {
                Ok(HttpError::Network(m.to_owned()))
            } else if let Some(u) = o.get("invalid_url").and_then(Value::as_str) {
                Ok(HttpError::InvalidUrl(u.to_owned()))
            } else {
                Err(invalid(
                    path,
                    "an object error is {\"network\": ..} or {\"invalid_url\": ..}",
                ))
            }
        }
        _ => Err(invalid(
            path,
            "must be \"timeout\", \"cancelled\" or an object",
        )),
    }
}

fn http_rule(i: usize, v: &Value) -> Result<HttpRule, SeedError> {
    let at = |field: &str| format!("http[{i}].{field}");
    let text = |field: &str| -> Result<Option<String>, SeedError> {
        match v.get(field) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(invalid(&at(field), "must be a string")),
        }
    };
    let method = v
        .get("method")
        .map(|m| named(&METHODS, &at("method"), m))
        .transpose()?;
    let reply = if let Some(e) = v.get("error") {
        Err(http_error(&at("error"), e)?)
    } else {
        let status = match v.get("status") {
            None => 200,
            Some(s) => s
                .as_u64()
                .and_then(|s| u16::try_from(s).ok())
                .ok_or_else(|| invalid(&at("status"), "must be a status code"))?,
        };
        let body = v
            .get("body")
            .map(|b| bytes_of(&at("body"), b))
            .transpose()?
            .unwrap_or_default();
        let mut response = HttpResponse::new(status, body);
        if let Some(headers) = v.get("headers") {
            let list = headers
                .as_array()
                .ok_or_else(|| invalid(&at("headers"), "must be a list of [name, value]"))?;
            for h in list {
                let pair = h.as_array().filter(|p| p.len() == 2);
                let (Some(name), Some(value)) = (
                    pair.and_then(|p| p[0].as_str()),
                    pair.and_then(|p| p[1].as_str()),
                ) else {
                    return Err(invalid(&at("headers"), "must be a list of [name, value]"));
                };
                response.headers.push(Header::new(name, value));
            }
        }
        Ok(response)
    };
    Ok(HttpRule {
        url: text("url")?,
        url_prefix: text("url_prefix")?,
        method,
        reply,
    })
}

impl Seed {
    /// Reads a seed document.
    ///
    /// # Errors
    ///
    /// [`SeedError`] naming the path of the first malformed value.
    pub fn from_json(text: &str) -> Result<Seed, SeedError> {
        let doc: Value = serde_json::from_str(text).map_err(|e| SeedError::Json(e.to_string()))?;
        if !doc.is_object() {
            return Err(invalid("$", "must be an object"));
        }
        if let Some(v) = doc.get("version") {
            let version = v.as_u64().unwrap_or(0);
            if version != SEED_VERSION {
                return Err(SeedError::UnsupportedVersion(version));
            }
        }
        let now_ms = match doc.get("now_ms") {
            None => None,
            Some(v) => Some(
                v.as_i64()
                    .ok_or_else(|| invalid("now_ms", "must be an integer"))?,
            ),
        };
        let rng_seed = match doc.get("rng_seed") {
            None => None,
            Some(Value::Number(n)) => Some(
                n.as_u64()
                    .ok_or_else(|| invalid("rng_seed", "must be a non-negative integer"))?,
            ),
            Some(Value::String(s)) => Some(
                s.strip_prefix("0x")
                    .and_then(|h| u64::from_str_radix(h, 16).ok())
                    .ok_or_else(|| invalid("rng_seed", "a string seed must be \"0x..\""))?,
            ),
            Some(_) => {
                return Err(invalid(
                    "rng_seed",
                    "must be an integer or a \"0x..\" string",
                ));
            }
        };
        let http = match doc.get("http") {
            None => Vec::new(),
            Some(v) => v
                .as_array()
                .ok_or_else(|| invalid("http", "must be a list"))?
                .iter()
                .enumerate()
                .map(|(i, r)| http_rule(i, r))
                .collect::<Result<_, _>>()?,
        };
        let connectivity = match doc.get("connectivity") {
            None => None,
            Some(c) => {
                let online = c
                    .get("online")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| invalid("connectivity.online", "must be true or false"))?;
                let kind = named(
                    &KINDS,
                    "connectivity.kind",
                    c.get("kind").unwrap_or(&Value::Null),
                )?;
                Some((online, kind))
            }
        };
        let lifecycle = doc
            .get("lifecycle")
            .map(|l| named(&STATES, "lifecycle", l))
            .transpose()?;
        Ok(Seed {
            now_ms,
            rng_seed,
            kv: entries(&doc, "kv")?,
            secure_store: entries(&doc, "secure_store")?,
            fs: entries(&doc, "fs")?,
            http,
            connectivity,
            lifecycle,
        })
    }

    /// Puts the seed into `fakes`.
    ///
    /// # Errors
    ///
    /// [`SeedError::Invalid`] for an `fs` path the fake refuses (an empty path, `..`, a file in
    /// the way); the fakes seeded before it keep their state.
    pub fn apply(&self, fakes: &Fakes) -> Result<(), SeedError> {
        if let Some(now) = self.now_ms {
            fakes.clock.set_now_ms(now);
        }
        if let Some(seed) = self.rng_seed {
            fakes.rng.reseed(seed);
        }
        for (key, value) in &self.kv {
            fakes.kv.insert(key.clone(), value.clone());
        }
        for (key, value) in &self.secure_store {
            fakes.secure_store.insert(key.clone(), value.clone());
        }
        for (path, contents) in &self.fs {
            fakes
                .fs
                .seed(path, contents.clone())
                .map_err(|e| invalid(&format!("fs.{path}"), &e.to_string()))?;
        }
        for rule in &self.http {
            let mut matcher = Matcher::any();
            if let Some(url) = &rule.url {
                matcher = matcher.and(Matcher::url(url.clone()));
            }
            if let Some(prefix) = &rule.url_prefix {
                matcher = matcher.and(Matcher::url_prefix(prefix.clone()));
            }
            if let Some(method) = rule.method {
                matcher = matcher.and(Matcher::method(method));
            }
            match &rule.reply {
                Ok(response) => fakes.http.respond(matcher, response.clone()),
                Err(error) => fakes.http.fail(matcher, error.clone()),
            };
        }
        if let Some((online, kind)) = self.connectivity {
            fakes.connectivity.set(online, kind);
        }
        if let Some(state) = self.lifecycle {
            fakes.lifecycle.set(state);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"{
      "version": 1, "now_ms": 5000, "rng_seed": "0x2a",
      "kv": {"a": "x", "b": {"hex": "00ff"}},
      "secure_store": {"t": "tok"},
      "fs": {"d/f.txt": "hi"},
      "http": [
        {"url": "https://x.test/a", "method": "get", "status": 201, "headers": [["h","v"]], "body": "ok"},
        {"url_prefix": "https://x.test/", "error": {"network": "down"}}
      ],
      "connectivity": {"online": false, "kind": "none"},
      "lifecycle": "background"
    }"#;

    #[test]
    fn a_full_seed_reads_and_applies() {
        let seed = Seed::from_json(FULL).unwrap();
        assert_eq!(seed.now_ms, Some(5000));
        assert_eq!(seed.rng_seed, Some(42));
        assert_eq!(seed.kv[1], ("b".to_owned(), vec![0, 255]));
        let fakes = Fakes::new();
        seed.apply(&fakes).unwrap();
        assert_eq!(undra_ports::Clock::now_ms(&*fakes.clock), 5000);
        assert_eq!(fakes.kv.value("a"), Some(b"x".to_vec()));
        assert_eq!(fakes.secure_store.value("t"), Some(b"tok".to_vec()));
        assert_eq!(fakes.fs.contents("d/f.txt"), Some(b"hi".to_vec()));
        assert_eq!(fakes.connectivity.current(), (false, NetKind::None));
        assert_eq!(fakes.lifecycle.current(), AppState::Background);
        assert_eq!(
            fakes.rng.next_u64(),
            undra_ports::fakes::SeededRng::new(42).next_u64()
        );
    }

    #[test]
    fn http_rules_answer_in_order_and_errors_fail() {
        use undra_ports::{Http, HttpRequest};
        let fakes = Fakes::new();
        Seed::from_json(FULL).unwrap().apply(&fakes).unwrap();
        let t = undra_runtime::testing::TestRuntime::new();
        let ok = t
            .run_until(fakes.http.request(HttpRequest::get("https://x.test/a")))
            .unwrap();
        assert_eq!((ok.status, ok.body.0.as_slice()), (201, &b"ok"[..]));
        assert_eq!(ok.headers, [Header::new("h", "v")]);
        let other = t.run_until(fakes.http.request(HttpRequest::get("https://x.test/b")));
        assert_eq!(other, Err(HttpError::Network("down".to_owned())));
    }

    #[test]
    fn an_empty_document_changes_nothing_and_bad_ones_name_the_path() {
        let fakes = Fakes::new();
        Seed::from_json("{}").unwrap().apply(&fakes).unwrap();
        assert!(fakes.kv.is_empty());
        let path = |text: &str| match Seed::from_json(text).unwrap_err() {
            SeedError::Invalid { path, .. } => path,
            other => panic!("{other}"),
        };
        assert_eq!(path(r#"{"kv": {"a": 1}}"#), "kv.a");
        assert_eq!(path(r#"{"http": [{"status": "x"}]}"#), "http[0].status");
        assert_eq!(path(r#"{"http": [{"method": "fetch"}]}"#), "http[0].method");
        assert_eq!(
            path(r#"{"connectivity": {"online": true, "kind": "5g"}}"#),
            "connectivity.kind"
        );
        assert_eq!(path(r#"{"lifecycle": 3}"#), "lifecycle");
        assert_eq!(path("[]"), "$");
        assert_eq!(
            Seed::from_json(r#"{"version": 2}"#).unwrap_err(),
            SeedError::UnsupportedVersion(2)
        );
        assert!(matches!(Seed::from_json("{"), Err(SeedError::Json(_))));
        assert!(
            Seed::from_json(r#"{"fs": {"../x": "y"}}"#)
                .unwrap()
                .apply(&fakes)
                .unwrap_err()
                .to_string()
                .contains("fs.../x")
        );
    }
}
