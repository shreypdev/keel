//! The Undra playground core: the one Rust core every playground app and every contract test runs.
//!
//! It is written the way an application writes its core, with nothing but the public `undra` API
//! (constitution R10: we ship on it first), and it is deliberately broad, because it has three
//! jobs at once:
//!
//! 1. **A reference app.** The web, iOS and Android apps in `examples/playground/` are three UIs
//!    over this crate, through the Swift, Kotlin and TypeScript bindings `undra bindgen` generates
//!    from its schema. They show the to-do list, the counter, a 10,000-row list, and a cached
//!    server list with optimistic updates.
//! 2. **The contract tests' subject.** `contract-tests/` drives every public item from all three
//!    platform runtimes and checks they agree (SPEC section 14).
//! 3. **The benchmarks' target.** The blueprint's section 14 budget rows are measured against the
//!    `bench_*` methods of [`Bench`].
//!
//! | Module | What it shows |
//! |---|---|
//! | [`todos`](mod@todos) | a store with a keyed list, a filter and two computed values |
//! | [`counter`](mod@counter) | a store whose commands are transactions: one change-set for many signals |
//! | [`biglist`](mod@biglist) | 10,000 keyed rows; every one-row operation is a one-operation patch |
//! | [`remote`](mod@remote) | a query, mutations and optimistic commands over the `Http` port |
//! | [`lab`](mod@lab) | every wire type, sync and async calls, typed errors, panics, cancellation, streams |
//! | [`bench`](mod@bench) | the budget-row methods: a primitive call, 1 KB echo, 100 dirty signals, one insert |
//! | [`stress`](mod@stress) | high-frequency data: a Timer-paced generator (and bursts) of one-write transactions the platforms apply once per frame, and a `no_coalesce` signal they apply step by step |
//!
//! The core reads no clock and no random source and starts no thread (R12): identities come from
//! counters, time from the `Clock` port, delays from `Ctx::sleep` and the network from the
//! `Http` port, so every test can drive it with `undra::ports::fakes`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod bench;
pub mod biglist;
pub mod counter;
pub mod lab;
pub mod remote;
pub mod stress;
pub mod todos;

pub use bench::Bench;
pub use biglist::{BigList, Item, ListError};
pub use counter::{Counter, Parity};
pub use lab::{
    Composite, Figure, LabError, Primitives, Probe, ProbeCounters, add, add_later, area,
    echo_composite, echo_figure, echo_primitives, explode, explode_later, fail_later, greet,
    parse_count, ping, version,
};
pub use remote::{
    RemoteConfig, RemoteError, RemoteTodo, configure_remote, create_remote_todo, patch_remote_todo,
    post_remote_todo, remote_todos, set_remote_done,
};
pub use stress::{MAX_RATE, Stress, StressError, StressMode};
pub use todos::{Filter, Todo, TodoError, Todos};
