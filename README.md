# Undra

**One Rust core. Native everywhere.**

[![CI](https://github.com/shreypdev/undra/actions/workflows/ci.yml/badge.svg)](https://github.com/shreypdev/undra/actions/workflows/ci.yml)
[![Benchmarks](https://github.com/shreypdev/undra/actions/workflows/bench.yml/badge.svg)](https://github.com/shreypdev/undra/actions/workflows/bench.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

**[Docs & site → shreypdev.github.io/undra](https://shreypdev.github.io/undra/)**

Undra owns everything **under the pixels** of your iOS, Android and web apps — domain
logic, reactive state, the data layer, persistence, and the dev loop — while the UI stays
100% native: SwiftUI, Jetpack Compose, React and React Native, written by hand, the way
platform engineers want to write them.

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
patches, not O(list) copies, and a filtered or sorted view of a list costs what changed. The
numbers below are measured by the benchmark suite in [`bench/`](bench/RESULTS.md) on an
Apple-Silicon host. The core operations are gated in CI against host budgets (a regression fails
the build), and so is the web size; the Android size is measured with `undra build` and recorded,
not gated. The "Budget" column holds the design's per-device targets. Rows from the iPhone
simulator, the Android emulator and headless Chromium are in
[`bench/RESULTS.md`](bench/RESULTS.md#device-numbers-ios-android-web); none is from a physical
phone, so no device target is claimed as met:

| Operation | Measured | Budget |
|---|---|---|
| Synchronous core call (C ABI, core side) | **49.8 ns** | ≤ 60 ns |
| 1 KB record round trip | **228 ns** | ≤ 3 µs |
| One insert into an observed 10,000-row list | **6.3 µs** | ≤ 20 µs |
| Filtered view of a 10,000-row list, one row changed (158 bytes on the wire, was 353 KB) | **392 ns** | ≤ 1 µs |
| Change-set for 100 dirty signals | **2.3 µs** | ≤ 100 µs |
| Cold start restoring 100 KB of state | **85 µs** | ≤ 3 ms |
| Web core: Undra's runtime and a hello-world core, one wasm module | **<!--measured:web-size-->116.7 KB<!--/measured-->** gzipped | ≤ 120 KB |
| Android core (`.so`, arm64-v8a, release, hello world) | **<!--measured:android-size-->978.6 KB<!--/measured-->** | ≤ 1.2 MB |

The web size is measured, not typed: [`scripts/wasm-size.sh`](scripts/wasm-size.sh) builds the
`undra init` template for the web the way an app does (`wasm-opt -Oz`, gzip level 9), and CI fails
a change that takes it over 120 KB or more than 5% over its record
([`bench/results/web-size.jsonl`](bench/results/web-size.jsonl), [ADR-052](.10x/adrs/ADR-052-web-bundle-size.md)).
The JavaScript runtime the page loads up front with it is gated the same way:
<!--measured:web-runtime-js-->22.1 KB<!--/measured--> gzipped against a 22.1 KB budget (the
blueprint's 8 KB predates the transports, reconnect, coalescing, worker mode and the typed error
channel; the transports and the default ports load when an app asks for them, and are not in it).
A 16 KB target is still open: it is not reachable without removing behaviour. The Android size is
a measurement of the same kind ([`bench/results/android-size.jsonl`](bench/results/android-size.jsonl)).
Sustained-load results (a firehose, keyed churn, fan-out, a 60-second soak) are under
[Harsh conditions](bench/RESULTS.md#harsh-conditions).

## Why you can trust it

* **<!--trust:tests-total-->7,253<!--/trust--> tests across the platforms** — Rust
  <!--trust:tests-rust-->3,536<!--/trust--> · TypeScript <!--trust:tests-typescript-->1,856<!--/trust--> ·
  Kotlin <!--trust:tests-kotlin-->881<!--/trust--> · Swift <!--trust:tests-swift-->870<!--/trust--> ·
  React Native <!--trust:tests-react-native-->110<!--/trust--> — the counts at the last merge, after the full matrix
  ran (the ledger is [`.10x/status.md`](.10x/status.md)).
* **<!--trust:scenarios-->33<!--/trust--> wire-level contract scenarios, run on every platform**
  (<!--trust:cells-->95<!--/trust-->/<!--trust:cells-->95<!--/trust--> cells pass; two scenarios are about the web
  host and run on TypeScript only): sync/async calls, typed errors, cancellation, stream backpressure, keyed patches,
  optimistic rollback, offline queue replay, snapshot/restore, schema-mismatch rejection, panic containment, a
  coalesced 1,000-transaction burst applied in one drain, a derived keyed list whose 60,000 recorded operations replay
  to the core's views, typed storage failures, WebSocket, SSE and Db, two cores in one process, objects and host
  callbacks, panic reports, background runs, newtypes, paged queries and polling. See
  [`contract-tests/`](contract-tests/scenarios.md).
* **Adversarial reviews**: the four core crates (signals, runtime, macros, ffi) first, every High/Medium finding
  fixed and independently re-verified, the unsafe boundary under ASan and Miri; then every feature of the v1.x
  program before it merged. The reports live in [`.10x/reviews/`](.10x/reviews/).
* **The reference app is real**: [`examples/playground`](examples/playground) runs one
  Rust core on Chrome, an iPhone simulator and an Android emulator, with proof screenshots
  committed; its React Native app runs on the same simulator and emulator.

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
    #[undra(key = "id")]
    visible: DerivedList<Todo>,
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
    let store: Todos           // try Todos() after UndraPlaygroundCore.load()
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
// React and React Native — hooks from @undra/runtime/react (vue / svelte / solid adapters ship too).
const store = useUndra(Todos);
const todos = useSignal(store?.visible);
```

Under the hood, `store.add("milk")` crosses the boundary once; the resulting change-set
updates `todos`, `visible` and `remaining` on every observer in a single main-thread
apply. Data fetching, caching, optimistic mutations with precise rollback, offline queues
and persistence are built in (`#[undra::query]` / `#[undra::mutation]`).

## Install

Four ways to get the same `undra` binary (macOS and Linux, x86_64 and arm64):

```bash
brew install shreypdev/undra/undra                                   # Homebrew
npm install -g @undra/cli                                            # npm (Node 20+)
curl -fsSL https://shreypdev.github.io/undra/install.sh | sh         # checks the sha256, installs to ~/.undra/bin, no sudo
cargo install --git https://github.com/shreypdev/undra undra-cli     # from source (Rust 1.85+)
```

Homebrew, npm and the installer script download the prebuilt binary of a GitHub Release, so
they work from the first tagged release (`v1.0.0`) on; the `cargo` line builds the default
branch today. Not supported yet: Windows, and Alpine (musl) for the prebuilt binaries.
`undra --version` prints `undra <version> (<commit>)` (`unknown` for a build from source). Maintainers: [docs/RELEASING.md](docs/RELEASING.md).

## Quick start

```bash
undra init myapp                          # core + SwiftUI + Compose + React shells
cd myapp
undra dev                                 # live core over WebSocket, rebuild on save
```

A new project depends on the Undra crates at the git tag of the `undra` that created it
(`undra = { git = "https://github.com/shreypdev/undra", tag = "v<version>" }`), which
exists from the first tagged release on. Before that, or to work on Undra itself, use a
checkout: its crates and runtimes are then used by path.

```bash
git clone https://github.com/shreypdev/undra.git && cd undra
cargo install --path crates/undra-cli
undra init myapp --dir .. --undra-path .  # the project next to the checkout, using it
```

Then open `web/` (`npm install && npm run dev`), `ios/` (Xcode) or `android/` (Gradle) —
each shell is a plain native project wired to your core, and each builds the core itself (a Gradle
task, an Xcode build phase, a Vite plugin: there is no manual `undra build`). `undra build --platform
ios,android,web` is the explicit form: an XCFramework, 16 KB-aligned `.so`s and a `wasm-opt`'d module.
`undra doctor` tells you exactly what your machine is missing, with the command that fixes it
(`undra doctor --fix` prints them all). `undra init` also writes a CI workflow, and `undra upgrade`
moves a project to a newer Undra in one step.

Prefer to explore first? The [playground](examples/playground/README.md) is the same
thing, fully built: todos, a counter, a 10,000-row keyed list, and remote
queries/mutations with an offline switch — on all three platforms.

## What's in it

The v1 core: records, enums, typed errors, **stores** (signals, computeds, keyed lists), **queries and
mutations** (staleness, dedup, retry with jitter, optimistic updates with surgical rollback, offline queue,
persistence), ten **ports** (Http, Kv, SecureStore, Fs, Clock, Rng, Log, Timer, Connectivity, Lifecycle — with
platform default adapters and deterministic Rust fakes), streams with backpressure, cancellation,
snapshot/restore, a schema-hash compatibility gate, `undra dev` with a live remote core, and teaching
diagnostics for every macro mistake (`error[undra::E0007]: …` with what/why/fix/docs).

Added since, each with its page:

* **React Native** — the same bindings and TypeScript mirror over a TurboModule on the C ABI, with the ten
  default adapters: [docs/REACT_NATIVE.md](docs/REACT_NATIVE.md).
* **Devtools with time travel** — a page served by `undra dev`: live stores, a change-set timeline you can scrub,
  port and query logs, behind a per-run token; state is kept across a rebuild: [docs/DEV_LOOP.md](docs/DEV_LOOP.md).
* **Derived lists** — a filtered or sorted view of a keyed list costs what changed (158 bytes, not 353 KB, for one
  edited row in 10,000): [docs](https://shreypdev.github.io/undra/docs/concepts.html#derived-lists).
* **WebSocket, SSE and Db ports** — opt-in real-time streams and SQL over SQLite with deterministic fakes on every
  platform: [real time](https://shreypdev.github.io/undra/docs/realtime.html),
  [database](https://shreypdev.github.io/undra/docs/db.html).
* **Migrations** — a changed schema still restores what the last build stored, and storage ports fail with typed
  errors: [Shipping an update](https://shreypdev.github.io/undra/docs/updates.html).
* **A testing kit** — previews that run your real core with scripted ports and a manual clock, and recorded sessions
  that replay in tests on Swift, Kotlin, TypeScript and Rust: [docs/TESTING.md](docs/TESTING.md).
* **iOS 15 and 16** — a lower deployment target generates `ObservableObject` stores; proven by compilation and a
  runtime probe on iOS 26.5, not yet on an iOS 15 or 16 runtime: [docs/IOS_15_16.md](docs/IOS_15_16.md).
* Also: objects and host callbacks across the boundary, newtypes, generics and `Decimal`, paged and lazy lists,
  polling, panic reports with symbolication, background runs, several cores in one app, and a
  [cookbook](https://shreypdev.github.io/undra/docs/cookbook/) with a sample app.

## What is not done

Open, with the work done around it (the same list as the [roadmap](https://shreypdev.github.io/undra/roadmap/)):

* Benchmark rows from physical phones; today's device rows are a simulator, an emulator and headless Chromium.
* The JavaScript runtime at 16 KB (it sits at its 22.1 KB gate).
* Generic functions and objects across the boundary (a generic record or enum crosses as one named type per
  instantiation).
* Query handles across an `undra dev` reload (stores survive; query handles need a decision record).
* The `undra-compose` and `android-adapters` tests in CI (they pass locally and on the emulator).

Waiting on a release, an account or a decision: the `v1.0.0` tag and its channels (brew, npm, curl; the
maintainer creates the npm organisation, the Homebrew tap and the tag), crates.io and Maven Central, a Windows
CLI, Flutter and Dart, and a custom domain. Undra is one team's work with no production users yet, and nothing is
published to a registry until the first tagged release.

Not planned, by design: shared UI of any kind and hosted services; a sync engine, if it comes, is a separate
package (see [`docs/blueprint.html`](docs/blueprint.html)).

## Repository map

| Path | What |
|---|---|
| `crates/` | the 13 Rust crates: schema (`undra-meta`), wire codec, macros, signals, runtime, ports, query, testkit, ffi (the only `unsafe`), transport, bindgen, cli, facade |
| `runtimes/` | the Swift, Kotlin, TypeScript and React Native runtime packages the generated code sits on |
| `examples/` | `playground` (the reference app: one core, three platforms and React Native, proof screenshots), `cookbook`, `fieldbook` (a sample app), `two-cores`, `ios15-sample` |
| `contract-tests/` | the <!--trust:scenarios-->33<!--/trust--> scenarios + a runner per platform |
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
