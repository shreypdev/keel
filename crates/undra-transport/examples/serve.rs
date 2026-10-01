//! A development server to point a platform runtime's `remote` transport at:
//!
//! ```text
//! cargo run -p undra-transport --example serve            # 127.0.0.1, any free port
//! cargo run -p undra-transport --example serve -- 127.0.0.1:7878
//! ```
//!
//! It serves a tiny core (a `Counter` store and a free function `sum`) and prints one JSON line
//! on stdout with the URL, the schema hash and the ids a client needs; log records go to stderr.
//! It runs until stdin closes or it is interrupted.
#![forbid(unsafe_code)]

use std::io::Read;

use undra::meta::ids;
use undra::prelude::*;
use undra::runtime::{Runtime, RuntimeConfig};
use undra_transport::{DevtoolsConfig, Server, ServerConfig};

/// The platform's counter adapter, called from the core.
#[undra::port]
pub trait Echo {
    async fn echo(&self, x: i32) -> i32;
}

#[undra::store]
pub struct Counter {
    ctx: Ctx,
    count: Signal<i32>,
}

#[undra::api(store)]
impl Counter {
    pub fn new(ctx: Ctx, initial: i32) -> Self {
        Self {
            ctx,
            count: Signal::new(initial),
        }
    }

    pub fn add(&self, n: i32) -> i32 {
        self.count.update(|c| *c += n);
        self.count.get()
    }

    /// Asks the platform's `Echo` adapter.
    pub async fn ask(&self, x: i32) -> i32 {
        echo(&self.ctx).echo(x).await
    }
}

#[undra::api]
pub fn sum(a: i32, b: i32) -> i32 {
    a + b
}

fn main() {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:0".to_owned());
    // Keeps a dropped client's objects for ten minutes, so a client that reconnects (the platform runtimes do)
    // finds its counter again (ADR-051).
    // With `UNDRA_SERVE_DEVTOOLS=<token>` (16 or more letters and digits) the devtools socket is
    // served too, behind that token, with no page: the interop script drives it as a page would.
    let devtools = std::env::var("UNDRA_SERVE_DEVTOOLS")
        .ok()
        .map(|token| DevtoolsConfig::new(token, &[]));
    let config = ServerConfig {
        resume_grace: std::time::Duration::from_secs(600),
        devtools,
        ..ServerConfig::default()
    };
    let server = Server::start(addr, config, |host| {
        host.set_log_sink(|level, target, message| eprintln!("[{level}] {target}: {message}"));
        Runtime::new(
            RuntimeConfig {
                mode: "dev".to_owned(),
                log_level: 1,
                ..RuntimeConfig::default()
            },
            host,
        )
    })
    .expect("the server starts");

    println!(
        "{{\"url\":\"{}\",\"schema\":\"{:#018x}\",\"counter\":{},\"new\":{},\"add\":{},\"ask\":{},\"sum\":{},\"echoPort\":{},\"echoMethod\":{}}}",
        server.url(),
        server.runtime().schema_hash(),
        ids::type_id("Counter"),
        ids::method_id("Counter", "new"),
        ids::method_id("Counter", "add"),
        ids::method_id("Counter", "ask"),
        ids::function_id("sum"),
        ids::port_id("Echo"),
        ids::port_method_id("Echo", "echo"),
    );

    // Serve until stdin is closed (or the process is interrupted).
    let mut sink = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut sink);
    server.shutdown();
    server.runtime().shutdown();
}
