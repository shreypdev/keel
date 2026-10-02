# SDE: devtools (B4, v1.2), ADR-054

Worktree `wt/devtools`, 2026-10-01. Design: `.10x/decisions/architect/devtools.md`; binding text: ADR-054, SPEC 5.10 /
13 / 16.2, `docs/DEV_LOOP.md` ("The devtools page").

## What was built

**`undra-transport` (`src/devtools/`).** `proto` (the page's messages, `[tag u8][body]`, documented and public),
`ring` (the bounded snapshot history), `hub` (observe-all, routing, port records, the worker, time travel, counters),
`http` (`GET /devtools[/..]` from a fixed asset table; request line peeked, not consumed), `serve` (the upgrade at
`/devtools/ws` and the connection loop). Small hooks in existing files: `Bridge` (a hub slot; `change_set` and `port_call`
take the longer route only while a page is attached), `Conn` (`send_plain`, `on_change_set_observed`, `observed_signals`,
`queued_bytes`), `Tracker` (`covers`, `signals_of`), `Session` (the app's `Observe` goes through the hub-aware path, a sync
call labels what it commits, port replies and teardown tell the hub, the app slot's changes), `Server` (`ServerConfig::devtools`,
the hub's life, `suspend` stops it first) and `serve()` (the peek that routes `/devtools` paths). No envelope kind, payload,
ABI, schema or runtime change.

**`undra-runtime`**: `register_inspector` / `inspect` / `inspectors` (`InspectFn`), the one seam; `tests/inspectors.rs` (4).
**`undra-query`**: registers `queries` when its client starts; `inspect.rs` renders the cache as JSON (key, status,
observers, age, hex of data and error, capped at 256 KiB a value); `tests/inspector.rs` (2).

**`undra-cli`**: `--devtools auto|on|off`; `devtools.rs` (the embedded assets, `new_token` from `/dev/urandom` with a
`RandomState` fallback, `enabled`, `page_url`, `write_assets`); the runner template takes `--devtools` and the token from
`UNDRA_DEVTOOLS_TOKEN` (an argument would show in `ps`) and compiles the page in (`include_bytes!` of files `write_runner`
writes beside `main.rs`); the banner prints `devtools      http://127.0.0.1:PORT/devtools?token=...` (or why it is off).

**The page** (`runtimes/ts/devtools`, TypeScript strict, no framework, only `@undra/runtime/wire`): `schema` / `value`
(schema-driven decoding of every wire type, keyed patches applied by SPEC 3.8's rules), `proto`, `mirror`, `diff`, `format`,
`state` (no DOM), `net` (reconnect with 250 ms to 5 s backoff; gives up after four refusals before ever opening: a wrong
token), `ui/*` (stores with keyed tables and a flash on change, a scrubber, timeline with per-entry diffs and a Restore
button, ports with decoded arguments and replies, the query cache and its events, counters labelled as the server's), the
site's tokens in dark and light. 17 KB gzipped (budget 150), committed under `crates/undra-cli/assets/devtools/`;
`build.sh --check` is a CI step.

## Decisions taken while building (beyond the note)

* **Observed-set bookkeeping.** The runtime's observe flag is shared, so `Route` (a thread-local the bridge reads) says what a
  change-set delivered on this thread is: `Commit(cause)`, the hub's `Initial { only }`, the app's own `AppObserve`, or
  `Silent`. When the last page leaves the hub observes the app's signals again with `AppObserve`, not silently: a write in the
  instant between `observe(off)` and `observe(on)` would otherwise never reach the app (found by a test that raced it).
  The hub stays *active* until that is done, so the bridge keeps filtering during it.
* **Steps repeat.** A late page is sent the ring's steps, and the first step it also gets as a broadcast: the protocol says
  messages are idempotent; the page keys steps by number, and the test helper skips the ones it has.
* **Unattended port calls are counted, not listed**: the playground's query hydration asks `Kv` ten times a second while no
  app is attached, which flushed the port log in the first browser run.
* **A restore is one row**: it re-sends every observed value of every store (one change-set per store); the page merges
  consecutive ones of the same restore and shows only what changed.
* **Stores are announced before their values** (`observe_stores` broadcasts `Stores`, then observes the new ones), and the
  mirror holds entries for a handle it has not been told about (256 at most).
* **`Hub` holds the runtime and the bridge holds the hub** (and the runtime holds the bridge as its host): a cycle that
  `Server::stop` breaks (`bridge.set_hub(None)`); a server dropped without stopping is stopped by `Drop`.
* **Query log from samples, not hooks** (ADR-054 alternatives).
* **The token is URL material**, so the index gets it substituted at serve time and its assets are requested with it; no
  cookie (cookies are shared across ports of one host).

## Findings

* Restoring tears down the app's query handles (`0 watching` right after a time travel in the playground): the ADR-053
  caveat, now visible. A screen re-mounting its query handle fixes it; nothing new.
* `Runtime::snapshot` taken after a commit is exactly the state after it in the single-writer case (every write happens
  under the core lock), so a step is labelled with the last change-set sequence delivered before it was taken.
* Playground: 10,000-row `BigList` is 200 KB of snapshot, so 200 steps would be 40 MiB: the 32 MiB bound evicts at about
  160 steps. Snapshot 68 us (ADR-053's measurement); the page's first paint of a 10,000-row list shows 40 rows.
* Opening the page changes what the core does: every computed is evaluated, and every commit costs a snapshot (a worker, off
  the commit path). The cost is bounded (steps, bytes) and absent when no page is open.

## Verification

Rust: `undra-transport` 71 unit + 18 `tests/devtools.rs` (the token and the 404 matrix, origin policy, schema in the
welcome, every signal to the page and only the observed ones to the app, the app's `observe(off)` not blinding the page, the
observed set restored on leave, time travel and the app converging, a step gone, a store dropped, a state over the per-step
bound, port calls with arguments, reply and latency, unattended calls, a second page, ring bounds, a malformed message, a
suspend with a page attached, history cleared and numbering kept); `undra-runtime` 4; `undra-query` 2; `undra-cli` 3 real
`undra dev` tests on a copy of the playground (page and token matrix, a change driven through an app client seen by the page,
time travel back with the app converging and its notice; `--devtools off`; a rebuild with a page open). TypeScript:
71 vitest tests in `runtimes/ts/devtools` (protocol vectors written by Rust, value decoding of every type, patches, mirror,
diff, state, the reconnect, the DOM through jsdom). Browser pane: the playground web app against `undra dev`, screenshots in
the session scratchpad (`devtools-proof/`: dark, light, phone width, timeline and diff, restore with the app's dev bar saying
`time travel: step 3`, port call detail, query cache, counters).

## Open

* The mirror's drains/merges/backlog (SPEC 11.1) are not visible to the page: they would need a report from the app runtime.
* A page cannot yet preview a step before restoring it (the server could send the snapshot; the page already decodes values).
* The ring is lost at a reload; carrying it would need ADR-053's schema-hash rule applied per step.
