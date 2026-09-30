# keel-transport

The WebSocket **server** behind `keel dev`: it serves one Keel core (a `keel_runtime::Runtime`)
to a platform runtime running somewhere else, a simulator, a phone, a browser tab, over the
envelope of `docs/SPEC.md` section 3.2. The three platform runtimes (Swift, Kotlin, TypeScript)
each ship the client half, their `remote` transport; this crate is the server they were written
against.

```text
   your app (SwiftUI / Compose / React)                      your dev machine
  ┌───────────────────────────────────┐   WebSocket    ┌──────────────────────────────┐
  │ generated bindings + mirror        │  KEEL envelopes │  keel-transport::Server      │
  │ platform runtime (`remote`)        │◄──────────────►│    ├ reader thread ─► Runtime │
  │ adapters: Http, Kv, Timer, ...     │  Call / Reply   │    └ writer thread ◄─ Bridge  │
  └───────────────────────────────────┘  ChangeSet ...  └──────────────────────────────┘
```

The core runs here; the *platform's adapters run in the app*, so a port call travels from the
core to the client (`PortCall`) and its answer travels back (`PortReply`).

## Serving a core

```rust
use std::net::TcpStream;

use keel::wire::payload::{Call, CallTarget, Hello, Reply};
use keel::wire::{Decode, Encode, Envelope, Kind, Reader, Writer};
use keel_transport::{Server, ServerConfig};
use tungstenite::{Message, WebSocket};

#[keel::api]
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

fn send(ws: &mut WebSocket<TcpStream>, schema: u64, kind: Kind, seq: u32, payload: &[u8]) {
    let mut w = Writer::new();
    Envelope::write(&mut w, kind, seq, schema, payload);
    ws.send(Message::Binary(w.into_vec())).unwrap();
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The runtime's host is fixed when it is built, so the server hands you its `Bridge`.
    let server = Server::start("127.0.0.1:0", ServerConfig::default(), |host| {
        keel::runtime::Runtime::new(keel::runtime::RuntimeConfig::default(), host)
    })?;
    let schema = server.runtime().schema_hash();

    // A platform runtime's `remote` transport, by hand: connect, Hello, then a call.
    let (mut ws, _) = tungstenite::client(server.url().as_str(), TcpStream::connect(server.addr())?)?;
    let mut hello = Writer::new();
    Hello { keel_version: "0", schema_hash: schema, platform: "example", mode: "dev" }.encode(&mut hello);
    send(&mut ws, schema, Kind::Hello, 0, hello.as_slice());
    let Message::Binary(reply) = ws.read()? else { panic!("binary envelopes only") };
    assert_eq!(Envelope::parse(&reply)?.kind, Kind::Hello); // the server's Hello, schema hash included

    let mut args = Writer::new();
    (2_i32, 40_i32).encode(&mut args);
    let target = CallTarget::Function { method_id: keel::meta::ids::function_id("add") };
    let mut call = Writer::new();
    Call { target, call_id: 1, args: args.as_slice() }.encode(&mut call);
    send(&mut ws, schema, Kind::Call, 1, call.as_slice());

    // The core also sends log records ("client connected"): wait for the reply.
    let sum = loop {
        let Message::Binary(bytes) = ws.read()? else { panic!("binary envelopes only") };
        let envelope = Envelope::parse(&bytes)?;
        if envelope.kind == Kind::Reply {
            let reply = Reply::decode(&mut Reader::new(envelope.payload))?;
            break i32::decode_exact(reply.body)?;
        }
    };
    assert_eq!(sum, 42);

    server.shutdown();
    server.runtime().shutdown();
    Ok(())
}
```

`Server::start` builds the [`Bridge`] (the `Host` the runtime reports to), lets you build the
runtime with it, and listens. Port `0` picks a free port; [`Server::addr`] and [`Server::url`]
say which. To serve a runtime you built yourself, build it with a [`Bridge`] as its host and pass
both to [`Server::bind`].

## What the server does

| Concern | Behaviour |
|---|---|
| Handshake | The client sends `Hello` first; the server always answers with its own (sequence 0, the core's schema hash in the header). A client with another schema hash is closed with **1008** *after* that reply, which is where every client reports `KeelSchemaMismatch`. |
| Clients | **One at a time.** A second one gets the server's `Hello`, then **1013** (try again later), after a short grace ([`ServerConfig::busy_grace`]) so that a reload or relaunch that reconnects before the old socket is torn down still wins. |
| Sequence numbers | Per direction; the server's start at 0 and have no gaps. Client numbers are not validated (Swift counts from 1, the others from 0). |
| Malformed input | A message that does not parse, a text message, an envelope with another schema hash, an oversized message: the connection is closed (**1002**, **1003**, **1008**, **1009**) and nothing else is affected. |
| Web pages | Only pages on this machine or a private network may connect ([`OriginPolicy`]); native clients send no `Origin` and always may. Without this, any site open in the developer's browser could drive the core. |
| Disconnect | The client's open calls and streams are cancelled, its observations stopped, the objects its constructors made released ([`ServerConfig::release_on_disconnect`]), and port calls it will never answer are failed as unavailable. |
| Slow clients | Outbound messages are queued and written by a writer thread; the core is never made to wait for the network. A client that falls [`ServerConfig::max_queued_bytes`] behind is dropped. |
| Dev records | A client that said `mode = "dev"` in its `Hello` receives the core's development-mode `Log` records (SPEC 5.10) when the runtime was built with `mode: "dev"`. |
| Sync ports | A *synchronous* port cannot be served by a remote client (the core needs the answer before it returns, and the bridge never waits): it is unavailable. Bind Rust implementations of `Clock`, `Rng` and `Log` in a dev core. |

The `Server` does not shut the runtime down; `Server::shutdown` (also on drop) closes the client
with **1001**, waits up to [`ServerConfig::close_timeout`] and joins every thread.

## No authentication

`keel dev` is a development tool. Bind to loopback (the default for `keel dev`); if a device on
your network has to reach it, that network is trusted with the core's whole public API, including
every function it exposes.

## Features

`server` (default) is the crate. Without it, and on `wasm` targets, the crate is empty: it is
host-side plumbing (`std::net` and threads, no async runtime, `tungstenite` as the only network
dependency) with nothing to do in a browser.

## Tests

`tests/` drives the server over real sockets on `127.0.0.1:0` with a raw `tungstenite` client and
a hand-rolled RFC 6455 client, the way each platform's transport behaves: `handshake`, `calls`,
`observe`, `streams`, `ports`, `lifecycle`, `robustness` (malformed frames and a byte fuzz),
`protocol` (logs, events, restore, ordering under concurrent writers) and `origin`. Unit tests
live beside the code, including property tests that feed random bytes to the dispatch.
