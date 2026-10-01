# Undra

**One Rust core. Native everywhere.**

[![CI](https://github.com/shreypdev/undra/actions/workflows/ci.yml/badge.svg)](https://github.com/shreypdev/undra/actions/workflows/ci.yml)
[![Benchmarks](https://github.com/shreypdev/undra/actions/workflows/bench.yml/badge.svg)](https://github.com/shreypdev/undra/actions/workflows/bench.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

**[Docs & site → shreypdev.github.io/undra](https://shreypdev.github.io/undra/)**

Undra owns everything **under the pixels** of your iOS, Android and web apps — domain
logic, reactive state, the data layer, persistence, and the dev loop — while the UI stays
100% native: SwiftUI, Jetpack Compose and React, written by hand, the way platform
engineers want to write them.

It is the opposite of a cross-platform UI framework. Your screens never leave the
platform. Your *logic* stops being written three times.

```text
        SwiftUI            Compose             React
           │                  │                  │
   @Observable stores   StateFlow stores   hooks + signals     ← generated, idiomatic
           └──────────────────┼──────────────────┘
                      binary change-sets
                (one boundary crossing per transaction)
                              │
                     ╔════════╧════════╗
                     ║   Rust core     ║   your app: stores, queries,
                     ║  (one codebase) ║   mutations, persistence, offline
                     ╚═════════════════╝
```

## Why it's fast

Reads never cross the language boundary — each platform holds a mirror of your state,
updated by compact binary change-sets, once per transaction. Lists cross as O(change)
patches, not O(list) copies. The numbers below are measured by the benchmark suite in
[`bench/`](bench/RESULTS.md) on an Apple-Silicon host. The core operations are gated in CI
against host budgets (a regression fails the build); the sizes are reported by `undra build`,
and the per-device targets in the "Budget" column are the blueprint's goals, measured on
real hardware in the device phase (tracked in [`bench/RESULTS.md`](bench/RESULTS.md)):

| Operation | Measured | Budget |
|---|---|---|
| Synchronous core call (C ABI, core side) | **49.8 ns** | ≤ 60 ns |
| 1 KB record round trip | **228 ns** | ≤ 3 µs |
| One insert into an observed 10,000-row list | **6.3 µs** | ≤ 20 µs |
| Change-set for 100 dirty signals | **2.3 µs** | ≤ 100 µs |
| Cold start restoring 100 KB of state | **71 µs** | ≤ 3 ms |
| Web runtime + hello-world core | **85 KB** gzipped wasm | ≤ 120 KB |
| Android core (`.so`, per ABI, release) | **831 KB** | ≤ 1.2 MB |

## Why you can trust it

* **3,800+ tests across five languages** — Rust 2,110 · TypeScript 897 · Kotlin 454 ·
  Swift 328 · wasm/C-ABI acceptance suites — all green in one pass.
* **17 wire-level contract scenarios, run on all three platforms** (51/51): sync/async
  calls, typed errors, cancellation, stream backpressure, keyed patches, optimistic
  rollback, offline queue replay, snapshot/restore, schema-mismatch rejection, panic
  containment. See [`contract-tests/`](contract-tests/scenarios.md).
* **Four adversarial reviews** of the core crates (signals, runtime, macros, ffi), every
  High/Medium finding fixed and independently re-verified — the unsafe boundary under
  ASan and Miri. The full reports live in [`.10x/reviews/`](.10x/reviews/).
* **The reference app is real**: [`examples/playground`](examples/playground) runs one
  Rust core on Chrome, an iPhone simulator and an Android emulator, with proof
  screenshots committed.

## What it looks like

Write your domain once, in Rust:

```rust
use undra::prelude::*;

/// The to-do list: what every UI observes and calls.
#[undra::store]
pub struct Todos {
    #[undra(key = "id")]
    todos: Signal<Vec<Todo>>,
    filter: Signal<Filter>,
    visible: Computed<Vec<Todo>>,
    remaining: Computed<u32>,
}

#[undra::api(store)]
impl Todos {
    pub async fn add(&self, title: String) -> Result<Todo, TodoError> { /* … */ }
    pub fn set_filter(&self, filter: Filter) { /* … */ }
}
```

`undra bindgen` emits code a native reviewer would sign off on — no wrappers, no
reflection, no `Any`:

```swift
// SwiftUI — the store is @Observable; reads are local, instant.
struct TodoScreen: View {
    let store: Todos           // try Todos(ctx: .shared)
    var body: some View {
        List(store.visible) { TodoRow($0) }
        Text("\(store.remaining) left")
    }
}
```

```kotlin
// Compose — signals are StateFlows.
val todos by store.visible.collectAsState()
LazyColumn { items(todos, key = { it.id }) { TodoRow(it) } }
```

```tsx
// React — hooks from @undra/runtime/react (vue / svelte / solid adapters ship too).
const store = useUndra(Todos);
const todos = useSignal(store?.visible);
```

Under the hood, `store.add("milk")` crosses the boundary once; the resulting change-set
updates `todos`, `visible` and `remaining` on every observer in a single main-thread
apply. Data fetching, caching, optimistic mutations with precise rollback, offline queues
and persistence are built in (`#[undra::query]` / `#[undra::mutation]`).

## Quick start

```bash
git clone https://github.com/shreypdev/undra.git && cd undra
cargo install --path crates/undra-cli    # the `undra` command
undra init myapp                          # core + SwiftUI + Compose + React shells
cd myapp
undra dev                                 # live core over WebSocket, rebuild on save
```

Then open `web/` (`npm install && npm run dev`), `ios/` (Xcode) or `android/` (Gradle) —
each shell is a plain native project wired to your core. `undra build --platform
ios,android,web` packages an XCFramework, 16 KB-aligned `.so`s and a `wasm-opt`'d module.
`undra doctor` tells you exactly what your machine is missing.

Prefer to explore first? The [playground](examples/playground/README.md) is the same
thing, fully built: todos, a counter, a 10,000-row keyed list, and remote
queries/mutations with an offline switch — on all three platforms.

## What's in v1

Records, enums, typed errors, objects, **stores** (signals, computeds, keyed lists),
ports (Http, Kv, SecureStore, Fs, Clock, Rng, Log, Timer, Connectivity, Lifecycle — with
platform default adapters and deterministic Rust fakes), **queries and mutations**
(staleness, dedup, retry with jitter, optimistic updates with surgical rollback, offline
queue, persistence), streams with backpressure, cancellation, snapshot/restore, a
schema-hash compatibility gate, `undra dev` with a live remote core, and teaching
diagnostics for every macro mistake (`error[undra::E0007]: …` with what/why/fix/docs).

Not in v1 (by design, see [`docs/blueprint.html`](docs/blueprint.html)): a sync engine,
hosted services, shared UI of any kind.

## Repository map

| Path | What |
|---|---|
| `crates/` | the 12 Rust crates: schema (`undra-meta`), wire codec, macros, signals, runtime, ports, query, ffi (the only `unsafe`), transport, bindgen, cli, facade |
| `runtimes/` | the Swift, Kotlin and TypeScript runtime packages the generated code sits on |
| `examples/playground` | the reference app: one core, three platforms, proof screenshots |
| `contract-tests/` | the 17 scenarios + a runner per platform |
| `bench/` | criterion benches + the budget gate; `RESULTS.md` has the numbers |
| `docs/SPEC.md` | the binding specification (wire, ABI, runtime model, generated shapes) |
| `.10x/` | the project's decision record: ADRs, reviews, status ledger |

## For contributors

* **[docs/ONBOARDING.md](docs/ONBOARDING.md)** — machine setup and how to run every suite.
* **[docs/AGENT_WORKFLOW.md](docs/AGENT_WORKFLOW.md)** — the worktree-per-piece workflow
  (humans and AI agents alike): brief → implement → adversarial review → merge → clean up.
* **[CLAUDE.md](CLAUDE.md)** — the constitution: twelve non-negotiable rules (R1–R12)
  every change is held to.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
