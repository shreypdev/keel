//! The devtools connection: the upgrade at `/devtools/ws` and its read loop.
//!
//! The same machinery as an app connection (the outbound queue and writer thread of
//! [`Conn`], the read side of [`crate::ws`]), without the envelope: the messages are those of
//! [`proto`](super::proto).

use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use tungstenite::Message;
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http::StatusCode;
use undra_runtime::log::{DEBUG, INFO, WARN};

use super::http::{self, RequestLine};
use super::proto::ClientMsg;
use super::{query_param, token_matches};
use crate::conn::Conn;
use crate::server::Shared;
use crate::session::{drain, printable};
use crate::ws::{self, ReadHalf, close};
use crate::writer;

const TARGET: &str = "undra::devtools";

/// The largest message a page may send: its messages are a few bytes.
const MAX_CLIENT_MESSAGE: usize = 64 * 1024;

/// Serves one connection that asked for a `/devtools` path: the page (`GET`) or the socket.
pub(crate) fn run(
    shared: &Arc<Shared>,
    id: u64,
    tcp: TcpStream,
    line: RequestLine,
    control: TcpStream,
    abort: TcpStream,
    write_tcp: TcpStream,
) {
    let config = &shared.config;
    let devtools = config.devtools.as_ref().filter(|c| c.token_is_valid());
    if line.path != "/devtools/ws" {
        http::serve_get(tcp, &line, devtools, config.handshake_timeout);
        return;
    }
    let Some(hub) = shared.hub.clone() else {
        // Not enabled: the upgrade is refused as if the path did not exist.
        http::serve_get(tcp, &line, None, config.handshake_timeout);
        return;
    };
    let (conn, queue) = Conn::new(id, shared.rt.schema_hash(), config.max_queued_bytes, Some(abort));
    let conn = Arc::new(conn);
    if !shared.attach(id, &conn) {
        conn.abort();
        return;
    }
    let ws_config = ws::config(MAX_CLIENT_MESSAGE);
    let half = match ReadHalf::new(tcp, conn.clone()) {
        Ok(half) => half,
        Err(e) => {
            shared.rt.log(DEBUG, TARGET, &format!("could not clone the socket: {e}"));
            return;
        }
    };
    let token = devtools.map(|c| c.token.clone()).unwrap_or_default();
    let check = |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
        let token_ok = query_param(request.uri().query().unwrap_or(""), "token")
            .is_some_and(|given| token_matches(&token, given));
        let origin_ok = match request.headers().get("Origin").map(|v| v.to_str()) {
            None => config.origin_policy.allows(None),
            Some(Ok(origin)) => config.origin_policy.allows(Some(origin)),
            Some(Err(_)) => false,
        };
        if token_ok && origin_ok && request.uri().path() == "/devtools/ws" {
            return Ok(response);
        }
        if token_ok && !origin_ok {
            // Someone who holds the token from a page that is not allowed: worth a line.
            let origin = request
                .headers()
                .get("Origin")
                .and_then(|v| v.to_str().ok())
                .map_or_else(|| "(none)".to_owned(), printable);
            shared.rt.log(WARN, TARGET, &format!("refused a devtools socket from origin {origin}: see OriginPolicy"));
        }
        // The same answer as for any path that does not exist.
        let mut refusal = ErrorResponse::new(Some("Not Found".to_owned()));
        *refusal.status_mut() = StatusCode::NOT_FOUND;
        Err(refusal)
    };
    let mut socket = match tungstenite::accept_hdr_with_config(half, check, Some(ws_config)) {
        Ok(socket) => socket,
        Err(e) => {
            shared.rt.log(DEBUG, TARGET, &format!("a devtools connection did not complete the upgrade: {e}"));
            conn.stop_writer();
            conn.abort();
            return;
        }
    };
    let timing = writer::Timing {
        linger: config.close_timeout,
        ping_interval: config.ping_interval,
    };
    let writer = match writer::spawn(conn.clone(), queue, write_tcp, ws_config, timing) {
        Ok(handle) => handle,
        Err(e) => {
            shared.rt.log(WARN, TARGET, &format!("could not start a writer thread: {e}"));
            conn.abort();
            return;
        }
    };
    socket.get_mut().route_writes_to_queue(conn.raw_sender());
    let _ = control.set_read_timeout(None);

    let mut peer_closed = false;
    if hub.attach(&conn) {
        loop {
            match socket.read() {
                Ok(Message::Binary(data)) => {
                    if conn.is_closing() {
                        continue;
                    }
                    match ClientMsg::decode(&data) {
                        Ok(msg) => hub.on_client(id, msg),
                        Err(e) => {
                            shared.rt.log(WARN, TARGET, &format!("closing a devtools page: {e}"));
                            conn.close(close::PROTOCOL_ERROR, "malformed devtools message");
                        }
                    }
                }
                Ok(Message::Text(_)) => {
                    if !conn.is_closing() {
                        conn.close(close::UNSUPPORTED_DATA, "the devtools protocol is binary");
                    }
                }
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                Ok(Message::Close(_)) => {
                    peer_closed = true;
                    let _ = socket.flush();
                    break;
                }
                Err(_) => break,
            }
        }
    } else {
        shared.rt.log(INFO, TARGET, "refused a devtools page: too many are open, or the server is stopping");
        conn.close(close::TRY_AGAIN_LATER, "too many devtools pages are open");
    }
    hub.detach(id);
    let _left = conn.drain();
    conn.stop_writer();
    let _ = writer.join();
    if conn.close_queued() && !peer_closed {
        drain(&control, Duration::from_millis(500).min(config.close_timeout));
    }
}
