//! `GET /devtools[/..]`: the page, served from the listener the dev server already has.
//!
//! The assets are a fixed table compiled into the program (no file is ever opened, so a path
//! cannot reach anything that is not in it). Every failure, a wrong or missing token, an unknown
//! path, devtools turned off, a method that is not `GET`, is the same `404`: the endpoint must not
//! say it exists to someone who does not hold the token.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::{Duration, Instant};

use super::{DevtoolsConfig, query_param, token_matches};

/// The text of `index.html` that stands for the token.
const PLACEHOLDER: &[u8] = b"__UNDRA_DEVTOOLS_TOKEN__";

/// What the first line of a request says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RequestLine {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) query: String,
}

/// Parses `GET /devtools/app.js?token=x HTTP/1.1`.
pub(crate) fn parse_request_line(line: &str) -> Option<RequestLine> {
    let mut parts = line.split(' ');
    let method = parts.next()?;
    let target = parts.next()?;
    if !parts.next()?.starts_with("HTTP/") || parts.next().is_some() {
        return None;
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Some(RequestLine {
        method: method.to_owned(),
        path: path.to_owned(),
        query: query.to_owned(),
    })
}

/// Whether `path` belongs to the devtools endpoint.
pub(crate) fn is_devtools_path(path: &str) -> bool {
    path == "/devtools" || path.starts_with("/devtools/")
}

/// Looks at the first line of the request on `tcp` **without consuming it** (so the WebSocket
/// upgrade that follows reads the whole request), waiting until it is complete or `deadline`.
/// `Ok(None)` when it is not a request line.
pub(crate) fn peek_request_line(tcp: &TcpStream, deadline: Instant) -> io::Result<Option<RequestLine>> {
    let mut buf = [0_u8; 2048];
    loop {
        let n = tcp.peek(&mut buf)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        if let Some(end) = buf[..n].windows(2).position(|w| w == b"\r\n") {
            return Ok(std::str::from_utf8(&buf[..end]).ok().and_then(parse_request_line));
        }
        if n == buf.len() {
            return Ok(None);
        }
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        // `peek` does not wait for more bytes than are already there.
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Reads up to the end of the request head, discarding it.
fn consume_head(tcp: &mut TcpStream, deadline: Instant) -> io::Result<()> {
    let mut head = Vec::with_capacity(512);
    let mut chunk = [0_u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        if head.len() > 16 * 1024 || Instant::now() >= deadline {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let n = tcp.read(&mut chunk)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        head.extend_from_slice(&chunk[..n]);
    }
    Ok(())
}

/// What to answer a request with.
pub(crate) struct Response {
    pub(crate) status: &'static str,
    pub(crate) content_type: &'static str,
    pub(crate) body: Vec<u8>,
}

impl Response {
    pub(crate) fn not_found() -> Response {
        Response {
            status: "404 Not Found",
            content_type: "text/plain; charset=utf-8",
            body: b"Not Found".to_vec(),
        }
    }
}

/// The response to `GET` of `line`: the asset, or the one `404`.
pub(crate) fn respond_to(line: &RequestLine, config: Option<&DevtoolsConfig>) -> Response {
    let Some(config) = config.filter(|c| c.token_is_valid()) else {
        return Response::not_found();
    };
    let token_ok = query_param(&line.query, "token").is_some_and(|t| token_matches(&config.token, t));
    if line.method != "GET" || !token_ok {
        return Response::not_found();
    }
    let name = match line.path.as_str() {
        "/devtools" | "/devtools/" => "index.html",
        path => path.strip_prefix("/devtools/").unwrap_or(""),
    };
    let Some(asset) = config.assets.iter().find(|a| a.path == name) else {
        return Response::not_found();
    };
    let body = if name == "index.html" {
        replace(asset.bytes, PLACEHOLDER, config.token.as_bytes())
    } else {
        asset.bytes.to_vec()
    };
    Response {
        status: "200 OK",
        content_type: asset.content_type,
        body,
    }
}

fn replace(text: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(text[i]);
            i += 1;
        }
    }
    out
}

/// The headers every response carries: nothing is cached, nothing is sniffed, no referrer leaks
/// the token, and the page may load only its own files and talk to its own server.
const COMMON_HEADERS: &str = "Cache-Control: no-store\r\n\
X-Content-Type-Options: nosniff\r\n\
Referrer-Policy: no-referrer\r\n\
Cross-Origin-Resource-Policy: same-origin\r\n\
Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self' ws: wss:; img-src 'self' data:; font-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'\r\n\
Connection: close\r\n";

/// Writes `response` and closes the connection politely (read what the client still sends, so
/// that closing does not reset the connection under a response the client is reading).
pub(crate) fn write_response(mut tcp: TcpStream, response: &Response) {
    let head = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{COMMON_HEADERS}\r\n",
        response.status,
        response.content_type,
        response.body.len()
    );
    let written = tcp
        .write_all(head.as_bytes())
        .and_then(|()| tcp.write_all(&response.body))
        .and_then(|()| tcp.flush());
    if written.is_ok() {
        let _ = tcp.shutdown(Shutdown::Write);
        let _ = tcp.set_read_timeout(Some(Duration::from_millis(200)));
        let mut sink = [0_u8; 512];
        while matches!(tcp.read(&mut sink), Ok(n) if n > 0) {}
    }
}

/// Answers the request on `tcp`, whose first line is `line`.
pub(crate) fn serve_get(mut tcp: TcpStream, line: &RequestLine, config: Option<&DevtoolsConfig>, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    if consume_head(&mut tcp, deadline).is_err() {
        return;
    }
    write_response(tcp, &respond_to(line, config));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devtools::Asset;

    static ASSETS: &[Asset] = &[
        Asset {
            path: "index.html",
            content_type: "text/html; charset=utf-8",
            bytes: b"<script src=\"app.js?token=__UNDRA_DEVTOOLS_TOKEN__\"></script>",
        },
        Asset {
            path: "app.js",
            content_type: "text/javascript; charset=utf-8",
            bytes: b"console.log(1)",
        },
    ];
    const TOKEN: &str = "0123456789abcdefXYZ";

    fn cfg() -> DevtoolsConfig {
        DevtoolsConfig::new(TOKEN, ASSETS)
    }

    fn get(path: &str, query: &str) -> RequestLine {
        RequestLine {
            method: "GET".into(),
            path: path.into(),
            query: query.into(),
        }
    }

    #[test]
    fn request_lines_are_parsed_and_junk_is_not() {
        assert_eq!(
            parse_request_line("GET /devtools/app.js?token=x HTTP/1.1"),
            Some(get("/devtools/app.js", "token=x"))
        );
        assert_eq!(parse_request_line("GET / HTTP/1.1"), Some(get("/", "")));
        assert_eq!(parse_request_line("GET /"), None);
        assert_eq!(parse_request_line("\u{1}\u{2}"), None);
        assert_eq!(parse_request_line("GET / FTP/1.1"), None);
        assert_eq!(parse_request_line("GET / HTTP/1.1 extra"), None);
    }

    #[test]
    fn only_the_devtools_paths_are_ours() {
        assert!(is_devtools_path("/devtools"));
        assert!(is_devtools_path("/devtools/"));
        assert!(is_devtools_path("/devtools/ws"));
        assert!(!is_devtools_path("/"));
        assert!(!is_devtools_path("/devtoolsx"));
        assert!(!is_devtools_path("/other/devtools"));
    }

    #[test]
    fn the_index_gets_the_token_and_assets_are_served_verbatim() {
        let q = format!("token={TOKEN}");
        let index = respond_to(&get("/devtools", &q), Some(&cfg()));
        assert_eq!(index.status, "200 OK");
        assert_eq!(index.content_type, "text/html; charset=utf-8");
        let text = String::from_utf8(index.body).unwrap();
        assert!(text.contains(&format!("app.js?token={TOKEN}")), "{text}");
        assert!(!text.contains("__UNDRA"));
        assert_eq!(respond_to(&get("/devtools/", &q), Some(&cfg())).status, "200 OK");
        let js = respond_to(&get("/devtools/app.js", &q), Some(&cfg()));
        assert_eq!(js.body, b"console.log(1)");
    }

    #[test]
    fn everything_else_is_the_same_404() {
        let good = format!("token={TOKEN}");
        let wrong = "token=0123456789abcdefXYX";
        let cases = [
            (get("/devtools", ""), Some(cfg())),
            (get("/devtools", wrong), Some(cfg())),
            (get("/devtools", "token="), Some(cfg())),
            (get("/devtools/nothing.js", &good), Some(cfg())),
            (get("/devtools/../secret", &good), Some(cfg())),
            (get("/devtools/%2e%2e/secret", &good), Some(cfg())),
            (
                RequestLine {
                    method: "POST".into(),
                    ..get("/devtools", &good)
                },
                Some(cfg()),
            ),
            (get("/devtools", &good), None),
            (get("/devtools", &good), Some(DevtoolsConfig::new("short", ASSETS))),
        ];
        let reference = Response::not_found();
        for (line, config) in cases {
            let got = respond_to(&line, config.as_ref());
            assert_eq!(
                (got.status, got.content_type, got.body),
                (reference.status, reference.content_type, reference.body.clone()),
                "{line:?}"
            );
        }
    }

    #[test]
    fn an_asset_table_that_is_empty_serves_nothing() {
        let config = DevtoolsConfig::new(TOKEN, &[]);
        let q = format!("token={TOKEN}");
        assert_eq!(respond_to(&get("/devtools", &q), Some(&config)).status, "404 Not Found");
    }
}
