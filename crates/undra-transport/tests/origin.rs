//! Cross-site WebSocket hijacking: a web page must not be able to drive a dev core just
//! because the developer has it open in a browser.
#![cfg(feature = "server")]

mod common;

use std::io::Read;

use common::*;
use undra::wire::payload::ReplyStatus;
use undra_transport::{OriginPolicy, ServerConfig};

fn status_line(head: &str) -> &str {
    head.lines().next().unwrap_or_default()
}

#[test]
fn a_page_from_the_public_internet_is_refused_with_403() {
    let f = start();
    for origin in [
        "https://evil.example",
        "null",
        "http://localhost.evil.example",
    ] {
        let (mut ws, head) = RawWs::upgrade(f.server.addr(), &[("Origin", origin)]);
        assert!(status_line(&head).contains("403"), "{origin}: {head}");
        let mut rest = Vec::new();
        let _ = ws.tcp.read_to_end(&mut rest);
        assert!(String::from_utf8_lossy(&rest).contains("origin not allowed"));
        assert!(!f.bridge.is_connected(), "{origin} never became a session");
    }
    f.eventually("the refusal is noted", |f| {
        f.log_lines()
            .iter()
            .any(|l| l.contains("refused a page from origin https://evil.example"))
    });
    // The server is fine.
    let mut client = f.client();
    assert!(client.new_counter(1) > 0);
}

#[test]
fn pages_on_this_machine_or_a_private_network_and_native_clients_are_accepted() {
    let f = start();
    for origin in [
        "http://localhost:5173",
        "http://127.0.0.1:3000",
        "http://192.168.1.20:5173",
        "http://laptop.local",
    ] {
        let (_ws, head) = RawWs::upgrade(f.server.addr(), &[("Origin", origin)]);
        assert!(status_line(&head).contains("101"), "{origin}: {head}");
    }
    // No Origin header at all: what URLSession, the JDK and Node send.
    let (_ws, head) = RawWs::upgrade(f.server.addr(), &[]);
    assert!(status_line(&head).contains("101"), "{head}");
}

#[test]
fn extra_origins_can_be_allowed_and_the_check_can_be_switched_off() {
    let config = ServerConfig {
        origin_policy: OriginPolicy::LocalNetworkAnd(vec!["https://dev.example.com".into()]),
        ..quick()
    };
    let f = start_with(config, "dev");
    let (_ws, head) = RawWs::upgrade(f.server.addr(), &[("Origin", "https://dev.example.com")]);
    assert!(status_line(&head).contains("101"), "{head}");
    let (_ws2, head) = RawWs::upgrade(f.server.addr(), &[("Origin", "https://other.example.com")]);
    assert!(status_line(&head).contains("403"), "{head}");

    let open = start_with(
        ServerConfig {
            origin_policy: OriginPolicy::Any,
            ..quick()
        },
        "dev",
    );
    let (_ws3, head) = RawWs::upgrade(open.server.addr(), &[("Origin", "https://evil.example")]);
    assert!(status_line(&head).contains("101"), "{head}");
}

#[test]
fn an_accepted_page_completes_the_whole_handshake_and_calls() {
    // What a browser does: an ordinary session with an Origin header on the upgrade.
    let f = start();
    let mut page =
        TestClient::connect_with_origin(&f.url(), f.schema(), Some("http://localhost:5173"));
    page.handshake("web", "dev");
    let (status, body) = page.call(
        undra::wire::payload::CallTarget::Function { method_id: SUM },
        &[enc(&1_i32), enc(&2_i32)].concat(),
    );
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 3));
    assert_eq!(f.bridge.client().unwrap().platform, "web");
}
