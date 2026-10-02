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
//! | [`platform`](mod@platform) | the standard ports seen from the core: one function per `Kv`, `SecureStore`, `Fs` and `Http` method, and a store of the `Connectivity` and `Lifecycle` reports, for proving a platform's adapters |
//! | [`remote`](mod@remote) | a query, mutations and optimistic commands over the `Http` port |
//! | [`ledger`](mod@ledger) | newtypes (`AccountId`, `Cents`, `Price`), named generic instantiations (`EntrySlice`, `LoadableEntries`), exact `Decimal` money and the leaf types of other crates (`uuid`, `chrono`, `rust_decimal`, `bytes`): ADR-042 |
//! | [`paging`](mod@paging) | a `Lazy<Item>` list the host pages through (a 10,000-row owned list and a view of a derived one), an infinite `feed` query over the big list and a polled `ticker` query: ADR-043 |
//! | [`lab`](mod@lab) | every wire type, sync and async calls, typed errors, panics, cancellation, streams |
//! | [`bench`](mod@bench) | the budget-row methods: a primitive call, 1 KB echo, 100 dirty signals, one insert |
//! | [`stress`](mod@stress) | high-frequency data: a Timer-paced generator (and bursts) of one-write transactions the platforms apply once per frame, and a `no_coalesce` signal they apply step by step |
//! | [`workshop`](mod@workshop) | objects as parameters and returns (a store that hands out child stores and takes stores as arguments) and a host callback interface the app implements (ADR-040, ADR-041) |
//! | [`updates`](mod@updates) | shipping an update (ADR-037): stores and queued mutations a second build changes, the persistence status, and an app sync port (ADR-049) |
//! | [`live`](mod@live) | real-time through the opt-in `WebSocket` and `Sse` ports: an echo, a connection read on demand (the core's credit), an event-stream reader that resumes |
//! | [`notes`](mod@notes) | a keyed list kept in SQLite through the opt-in `Db` port: migrations, bound statements, transactions, typed errors |
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
pub mod ledger;
pub mod live;
pub mod notes;
pub mod paging;
pub mod platform;
pub mod remote;
pub mod stress;
pub mod todos;
pub mod updates;
pub mod workshop;

pub use bench::Bench;
pub use biglist::{BigList, Item, ListError};
pub use counter::{Counter, Parity};
pub use lab::{
    Composite, Figure, LabError, Primitives, Probe, ProbeCounters, add, add_later, area,
    echo_composite, echo_figure, echo_primitives, explode, explode_later, fail_later, greet,
    parse_count, ping, version,
};
pub use ledger::{
    Account, AccountId, Cents, Entry, EntrySlice, Ledger, LedgerError, Loadable, LoadableEntries,
    Price, Receipt, Slice, balances, deposit, echo_account, echo_cents, echo_decimal, echo_price,
    echo_receipt, loadable_statement, open_account, sample_receipt, statement,
};
pub use live::{Live, SseFollow, sse_follow, ws_echo};
pub use notes::{DbCells, Note, Notes, db_cells, db_migrate, db_run};
pub use paging::{
    LIBRARY_LEN, Library, PAGE, TickError, feed, set_ticker_failing, ticker, ticker_fetches,
    touch_feed,
};
pub use remote::{
    RemoteConfig, RemoteError, RemoteTodo, configure_remote, create_remote_todo, patch_remote_todo,
    post_remote_todo, remote_todos, set_remote_done,
};
pub use stress::{MAX_RATE, Stress, StressError, StressMode};
pub use todos::{Filter, Todo, TodoError, Todos};
pub use updates::{
    Legacy, Locale, Profile, StorageStatus, localized_greeting, save_note, storage_status, tag_note,
};
pub use workshop::{ReportError, Reporter, Shelf, Watch, Workshop, WorkshopError};
