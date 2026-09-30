# keel-ports

The ten **standard ports** of Keel (`docs/SPEC.md` section 8), the records they exchange, and
deterministic **fakes** for every one of them.

A port is a trait the core calls and the platform implements: `URLSession` / OkHttp / `fetch` for
`Http`, the Keychain for `SecureStore`, `setTimeout` for `Timer`, and so on. Core code never
touches the system directly (no wall clock, no `rand`, no threads: constitution R12), so the
same core runs against the real platform in production and against the fakes in tests.

| Port | Kind | Methods |
|---|---|---|
| `Clock` | sync | `now_ms`, `monotonic_ns` |
| `Rng` | sync | `fill` |
| `Log` | sync | `log` |
| `Http` | async | `request` |
| `Kv` | async | `get`, `set`, `delete`, `list` |
| `SecureStore` | async | `get`, `set`, `delete`, `list` |
| `Fs` | async | `read`, `write`, `delete`, `list` |
| `Timer` | sync | `set` (fire and forget; the platform answers with `TimerFired`) |
| `Connectivity` | event | `changed` (host to core) |
| `Lifecycle` | event | `changed` (host to core) |

Records and errors: `HttpRequest`, `HttpResponse`, `Header`, `HttpMethod`, `HttpError`,
`FsError`, `NetKind`, `AppState`.

## The wire contract

The three platform runtimes (Swift, Kotlin, TypeScript) hand-write codecs for these types and
hard-code the port and method ids. **Field order, variant order and indices, method names and
declaration order are frozen**; changing any of them is a major version and an ADR
(constitution R7, R11). `tests/ids.rs`, `tests/encoding.rs` and `tests/schema.rs` lock the ids,
the exact bytes and the schema hash.

* `port_id = fnv1a32("port.<Trait>")`, `method_id = fnv1a32("<Trait>.<method>")` (SPEC 1.1).
* Records are their fields in order; enums and errors are a `u16` variant index plus fields.
* `HttpMethod`: Get 0, Post 1, Put 2, Delete 3, Patch 4, Head 5, Options 6 (SPEC 8 does not list
  them; the platform runtimes and this crate agree on this order).

## Using a port from the core

Every request/reply port trait gets a typed accessor (`keel_ports::http(ctx)`, `kv`, `clock`,
...). It returns the Rust binding if one is installed (a fake, a built-in), and otherwise a proxy
that crosses to the platform. The two event ports (`Connectivity`, `Lifecycle`) are subscribed to
instead: `on_connectivity_changed(ctx, f)`, `on_lifecycle_changed(ctx, f)`. See the crate
documentation for a compiled example.

## Testing with the fakes

`keel_ports::fakes::install` binds a fake for every port into a `TestRuntime`:

```rust
use std::time::Duration;
use keel_ports::{Clock, Http, HttpRequest, HttpResponse, Kv, fakes};
use keel_runtime::testing::TestRuntime;

// A test runtime with every port bound to a fake.
let t = TestRuntime::new();
let fakes = fakes::install(&t);

// Script the world.
fakes.http.respond("https://api.test/todos", HttpResponse::new(200, b"[]".to_vec()));
fakes.clock.set_now_ms(1_700_000_000_000);

// Core code reaches ports through the context, never through the system.
let ctx = t.ctx();
let request = HttpRequest::get("https://api.test/todos");
let response = t.run_until(keel_ports::http(&ctx).request(request)).unwrap();
assert_eq!(response.status, 200);
assert_eq!(keel_ports::clock(&ctx).now_ms(), 1_700_000_000_000);

// Persist through the Kv port and inspect the fake directly.
t.run_until(keel_ports::kv(&ctx).set("todos".into(), response.body.clone()));
assert_eq!(fakes.kv.value("todos"), Some(b"[]".to_vec()));

// Time only moves when told to: this advances the clock, fires its timers into the
// runtime and completes due sleeps.
fakes.advance(&t, Duration::from_secs(30));
assert_eq!(keel_ports::clock(&ctx).now_ms(), 1_700_000_030_000);
assert_eq!(fakes.http.call_count(), 1);
```

| Fake | Port(s) | What it does |
|---|---|---|
| `FakeHttp` | `Http` | replies chosen by `Matcher`, in sequence or by closure; records every request |
| `MemKv`, `MemSecureStore` | `Kv`, `SecureStore` | ordered in-memory maps; record every operation |
| `MemFs` | `Fs` | in-memory tree with the platform adapters' error semantics (`..` is `Denied`) |
| `FakeClock` | `Clock` + `Timer` | settable time; `advance(d)` fires due timers into the runtime |
| `SeededRng` | `Rng` | xorshift64\*: the same seed gives the same bytes |
| `CaptureLog` | `Log` | keeps every record |
| `ScriptedConnectivity`, `ScriptedLifecycle` | `Connectivity`, `Lifecycle` | push scripted events into a runtime |

All fakes are `Send + Sync`.

## Note on the schema

`#[keel::port]`, `#[keel::api]` and `#[keel::error]` register everything in this crate with the
schema, so any core that links `keel-ports` describes the ten ports and their records to
`keel-bindgen` and includes them in its schema hash.
