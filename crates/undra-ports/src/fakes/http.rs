//! [`FakeHttp`]: scripted HTTP responses and a record of every request.

use core::fmt;
use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::{Http, HttpError, HttpMethod, HttpRequest, HttpResponse};

type Predicate = dyn Fn(&HttpRequest) -> bool + Send + Sync;
type Handler = dyn Fn(&HttpRequest) -> Result<HttpResponse, HttpError> + Send + Sync;

/// Decides whether a scripted reply applies to a request.
///
/// A `&str` or `String` converts to an exact-URL matcher and an [`HttpMethod`] to a
/// method matcher, so `http.respond("https://x.test/a", response)` reads naturally.
///
/// ```
/// use undra_ports::{HttpMethod, HttpRequest};
/// use undra_ports::fakes::Matcher;
///
/// let m = Matcher::url_prefix("https://api.test/").and(Matcher::method(HttpMethod::Post));
/// assert!(m.matches(&HttpRequest::post("https://api.test/todos", vec![])));
/// assert!(!m.matches(&HttpRequest::get("https://api.test/todos")));
/// ```
#[derive(Clone)]
pub struct Matcher(Arc<Predicate>);

impl Matcher {
    /// Matches every request.
    pub fn any() -> Matcher {
        Matcher::custom(|_| true)
    }

    /// Matches requests to exactly `url`.
    pub fn url(url: impl Into<String>) -> Matcher {
        let url = url.into();
        Matcher::custom(move |request| request.url == url)
    }

    /// Matches requests whose URL starts with `prefix`.
    pub fn url_prefix(prefix: impl Into<String>) -> Matcher {
        let prefix = prefix.into();
        Matcher::custom(move |request| request.url.starts_with(&prefix))
    }

    /// Matches requests whose URL contains `needle`.
    pub fn url_contains(needle: impl Into<String>) -> Matcher {
        let needle = needle.into();
        Matcher::custom(move |request| request.url.contains(&needle))
    }

    /// Matches requests with `method`.
    pub fn method(method: HttpMethod) -> Matcher {
        Matcher::custom(move |request| request.method == method)
    }

    /// Matches `GET` requests to exactly `url`.
    pub fn get(url: impl Into<String>) -> Matcher {
        Matcher::url(url).and(Matcher::method(HttpMethod::Get))
    }

    /// Matches `POST` requests to exactly `url`.
    pub fn post(url: impl Into<String>) -> Matcher {
        Matcher::url(url).and(Matcher::method(HttpMethod::Post))
    }

    /// Matches requests for which `predicate` returns `true`.
    pub fn custom(predicate: impl Fn(&HttpRequest) -> bool + Send + Sync + 'static) -> Matcher {
        Matcher(Arc::new(predicate))
    }

    /// Matches requests that both `self` and `other` match.
    #[must_use]
    pub fn and(self, other: Matcher) -> Matcher {
        Matcher::custom(move |request| self.matches(request) && other.matches(request))
    }

    /// Whether `request` matches.
    pub fn matches(&self, request: &HttpRequest) -> bool {
        (self.0)(request)
    }
}

impl fmt::Debug for Matcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Matcher(..)")
    }
}

impl From<&str> for Matcher {
    fn from(url: &str) -> Matcher {
        Matcher::url(url)
    }
}

impl From<String> for Matcher {
    fn from(url: String) -> Matcher {
        Matcher::url(url)
    }
}

impl From<HttpMethod> for Matcher {
    fn from(method: HttpMethod) -> Matcher {
        Matcher::method(method)
    }
}

type Reply = Result<HttpResponse, HttpError>;

enum Script {
    /// The same reply every time.
    Fixed(Reply),
    /// One reply per matching request, in order; exhausted rules stop matching.
    Sequence(VecDeque<Reply>),
    /// Computed from the request.
    Handler(Arc<Handler>),
}

struct Rule {
    matcher: Matcher,
    script: Script,
}

#[derive(Default)]
struct State {
    rules: Vec<Rule>,
    calls: Vec<HttpRequest>,
}

/// An [`Http`] port that answers from a script and remembers every request.
///
/// Rules are tried in the order they were added; the first one that matches wins. A request
/// nothing matches fails with [`HttpError::Network`] naming the request, and is still recorded.
///
/// ```
/// use undra_ports::{Http, HttpError, HttpRequest, HttpResponse};
/// use undra_ports::fakes::FakeHttp;
/// use undra_runtime::testing::TestRuntime;
///
/// let http = FakeHttp::new();
/// // A flaky endpoint: two timeouts, then success for ever after.
/// http.respond_sequence("https://x.test/a", [Err(HttpError::Timeout), Err(HttpError::Timeout)]);
/// http.respond("https://x.test/a", HttpResponse::new(200, b"ok".to_vec()));
///
/// let t = TestRuntime::new();
/// let get = || HttpRequest::get("https://x.test/a");
/// assert_eq!(t.run_until(http.request(get())), Err(HttpError::Timeout));
/// assert_eq!(t.run_until(http.request(get())), Err(HttpError::Timeout));
/// assert_eq!(t.run_until(http.request(get())).unwrap().status, 200);
/// assert_eq!(http.call_count(), 3);
/// ```
#[derive(Default)]
pub struct FakeHttp {
    state: Mutex<State>,
}

impl FakeHttp {
    /// A fake with no rules.
    pub fn new() -> FakeHttp {
        FakeHttp::default()
    }

    /// Answers every request that `matcher` matches with `response`.
    pub fn respond(&self, matcher: impl Into<Matcher>, response: HttpResponse) -> &FakeHttp {
        self.add(matcher.into(), Script::Fixed(Ok(response)))
    }

    /// Fails every request that `matcher` matches with `error`.
    pub fn fail(&self, matcher: impl Into<Matcher>, error: HttpError) -> &FakeHttp {
        self.add(matcher.into(), Script::Fixed(Err(error)))
    }

    /// Answers the requests that `matcher` matches with `replies`, one each, in order. Once the
    /// replies are used up the rule no longer matches, so later rules (or the unmatched-request
    /// failure) apply.
    pub fn respond_sequence(
        &self,
        matcher: impl Into<Matcher>,
        replies: impl IntoIterator<Item = Result<HttpResponse, HttpError>>,
    ) -> &FakeHttp {
        self.add(
            matcher.into(),
            Script::Sequence(replies.into_iter().collect()),
        )
    }

    /// Answers every request that `matcher` matches by calling `handler`. The handler runs
    /// with no lock held, so it may inspect this fake.
    pub fn respond_with(
        &self,
        matcher: impl Into<Matcher>,
        handler: impl Fn(&HttpRequest) -> Result<HttpResponse, HttpError> + Send + Sync + 'static,
    ) -> &FakeHttp {
        self.add(matcher.into(), Script::Handler(Arc::new(handler)))
    }

    fn add(&self, matcher: Matcher, script: Script) -> &FakeHttp {
        self.state.lock().rules.push(Rule { matcher, script });
        self
    }

    /// Every request received so far, oldest first (including unmatched ones).
    pub fn calls(&self) -> Vec<HttpRequest> {
        self.state.lock().calls.clone()
    }

    /// Removes and returns every request received so far.
    pub fn take_calls(&self) -> Vec<HttpRequest> {
        std::mem::take(&mut self.state.lock().calls)
    }

    /// How many requests have been received.
    pub fn call_count(&self) -> usize {
        self.state.lock().calls.len()
    }

    /// The most recent request.
    pub fn last_call(&self) -> Option<HttpRequest> {
        self.state.lock().calls.last().cloned()
    }

    /// Forgets every rule and every recorded request.
    pub fn reset(&self) {
        let mut state = self.state.lock();
        state.rules.clear();
        state.calls.clear();
    }

    /// Records `request` and finds its scripted reply.
    fn answer(&self, request: &HttpRequest) -> Reply {
        let handler = {
            let mut state = self.state.lock();
            state.calls.push(request.clone());
            let mut chosen = None;
            for rule in &mut state.rules {
                if !rule.matcher.matches(request) {
                    continue;
                }
                match &mut rule.script {
                    Script::Fixed(reply) => return reply.clone(),
                    Script::Sequence(replies) => {
                        if let Some(reply) = replies.pop_front() {
                            return reply;
                        }
                    }
                    Script::Handler(handler) => {
                        chosen = Some(Arc::clone(handler));
                        break;
                    }
                }
            }
            chosen
        };
        match handler {
            Some(handler) => handler(request),
            None => Err(HttpError::Network(format!(
                "FakeHttp: no scripted response for {} {}",
                request.method, request.url
            ))),
        }
    }
}

impl fmt::Debug for FakeHttp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state.lock();
        f.debug_struct("FakeHttp")
            .field("rules", &state.rules.len())
            .field("calls", &state.calls.len())
            .finish()
    }
}

#[undra_macros::port]
impl Http for FakeHttp {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        self.answer(&req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::testing::block_on;

    fn get(url: &str) -> HttpRequest {
        HttpRequest::get(url)
    }

    fn ok(status: u16) -> HttpResponse {
        HttpResponse::new(status, Vec::new())
    }

    #[test]
    fn first_matching_rule_wins() {
        let http = FakeHttp::new();
        http.respond("https://x.test/a", ok(201))
            .respond(Matcher::url_prefix("https://x.test/"), ok(202))
            .respond(Matcher::any(), ok(203));
        assert_eq!(block_on(http.request(get("https://x.test/a"))), Ok(ok(201)));
        assert_eq!(block_on(http.request(get("https://x.test/b"))), Ok(ok(202)));
        assert_eq!(block_on(http.request(get("https://other/"))), Ok(ok(203)));
    }

    #[test]
    fn unmatched_requests_fail_with_a_message_and_are_recorded() {
        let http = FakeHttp::new();
        let error = block_on(http.request(get("https://nowhere.test/"))).unwrap_err();
        assert_eq!(
            error,
            HttpError::Network(
                "FakeHttp: no scripted response for GET https://nowhere.test/".into()
            )
        );
        assert_eq!(http.call_count(), 1);
    }

    #[test]
    fn sequences_are_consumed_then_fall_through() {
        let http = FakeHttp::new();
        http.respond_sequence("u", [Ok(ok(500)), Err(HttpError::Timeout)]);
        http.respond("u", ok(200));
        let statuses: Vec<_> = (0..4)
            .map(|_| block_on(http.request(get("u"))).map(|r| r.status))
            .collect();
        assert_eq!(
            statuses,
            [Ok(500), Err(HttpError::Timeout), Ok(200), Ok(200)]
        );
    }

    #[test]
    fn handlers_see_the_request_and_may_inspect_the_fake() {
        let http = Arc::new(FakeHttp::new());
        let inner = http.clone();
        http.respond_with(Matcher::any(), move |request| {
            Ok(HttpResponse::new(
                200,
                format!("{} #{}", request.url, inner.call_count()).into_bytes(),
            ))
        });
        let response = block_on(http.request(get("u"))).unwrap();
        assert_eq!(response.body.0, b"u #1");
    }

    #[test]
    fn matchers_combine() {
        let post = HttpRequest::post("https://x.test/a", vec![1]);
        assert!(Matcher::post("https://x.test/a").matches(&post));
        assert!(!Matcher::get("https://x.test/a").matches(&post));
        assert!(Matcher::url_contains("x.test").matches(&post));
        assert!(!Matcher::url_contains("y.test").matches(&post));
        assert!(Matcher::from(HttpMethod::Post).matches(&post));
        assert!(Matcher::from(String::from("https://x.test/a")).matches(&post));
        assert!(Matcher::custom(|r| r.body.is_some()).matches(&post));
        assert_eq!(format!("{:?}", Matcher::any()), "Matcher(..)");
    }

    #[test]
    fn calls_are_recorded_taken_and_reset() {
        let http = FakeHttp::new();
        http.respond(Matcher::any(), ok(200));
        let a = get("a");
        let b = HttpRequest::post("b", vec![9]).with_header("k", "v");
        block_on(http.request(a.clone())).unwrap();
        block_on(http.request(b.clone())).unwrap();
        assert_eq!(http.calls(), [a.clone(), b.clone()]);
        assert_eq!(http.last_call(), Some(b));
        assert_eq!(http.take_calls().len(), 2);
        assert_eq!(http.call_count(), 0);
        assert_eq!(http.last_call(), None);
        block_on(http.request(a)).unwrap();
        http.reset();
        assert_eq!(http.call_count(), 0);
        assert!(
            block_on(http.request(get("a"))).is_err(),
            "rules were reset too"
        );
        assert!(format!("{http:?}").contains("calls: 1"));
    }
}
