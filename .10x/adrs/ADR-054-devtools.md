# ADR-054: The devtools of `undra dev`: a page on the dev listener, a hub that observes everything, a ring of snapshots and time travel

Status: Accepted (2026-10-01, piece B4 of the v1.x plan, `wt/devtools`; the integrator accepted the four decisions of
`.10x/decisions/architect/devtools.md` and added the token requirement of decision 6). Touches `undra-transport` (a second
endpoint, a hub, three small session hooks), `undra-runtime` (one seam, `register_inspector`), `undra-query` (registers
it), `undra-cli` (`--devtools`, the embedded page, the runner template) and adds the page, `runtimes/ts/devtools`. It does
**not** touch the envelope (SPEC 3.2), any payload, the C ABI, the wasm ABI, the schema, generated code, the snapshot layout
or any platform runtime: an app client is not aware that a page exists. Constitution R11: decided here, before the
code (the design note is written first; this text is its binding form).

## Context

SPEC 5.10 gives a client that said `mode = "dev"` text `Log` records: `commit txn=T entries=N bytes=B`, a line per port call
and its duration. That is enough to read a terminal and not enough for a tool: no value, no argument, no reply, only for
signals *some client observed*, and only to the one app client. The developer's questions are "what is in my stores right
now", "what did that tap change", "what did the core ask the platform for and how long did it take", "what is cached",
and "put it back the way it was". ADR-053 made the second half cheap: `Runtime::snapshot` / `restore` and a server that
owns the core can carry state across a rebuild. The same two calls are a time machine.

## Decision

### 1. A second endpoint on the dev listener, not an envelope kind

The dev server's listener answers `GET /devtools` and `GET /devtools/<asset>` with the page, and upgrades
`/devtools/ws` to a *devtools connection*. The request line is peeked (left in the socket) before the WebSocket upgrade
decides which kind of connection it is, so an app client's path is untouched. A devtools connection is not an envelope
stream: each binary message is `[tag u8][body]` in the primitives of SPEC 3 (`undra-transport`
`devtools::proto`, documented; the page's `proto.ts` is tested against the same vectors, `runtimes/ts/devtools/test/vectors.json`,
which the Rust test writes and checks). Server to page: `Welcome` (protocol 1, versions, schema hash, `core_epoch`, the
ring's bounds and the whole schema as JSON), `Stores`, `ChangeSet` (the SPEC 3.5 payload, with a sequence, a time, whether
it is a commit or an initial state, and its cause: the id of the synchronous call that made it, `restore(step)` or none),
`Step`, `Evicted`, `Port` (start and end), `Stats` (JSON), `Queries` (JSON), `Traveled`, `App`. Page to server: `Restore {
request_id, step }` and `Resync`. Messages may repeat (a page that attaches is told the steps it may have): a page applies
them idempotently. The app slot of ADR-051 ("one client at a time") is not used: pages and the app coexist, up to four pages.
A new envelope kind would have meant a wire change in every codec and golden vector for something only the dev server
sends; the separate framing is a protocol of its own that only the dev server and its own page speak, and they ship together
(`Welcome.protocol` is checked anyway).

### 2. Observe-all is the server's, and the bridge splits what the runtime emits

While at least one page is attached the hub observes every store with `Runtime::observe`. The runtime has one observed flag
per signal and one host, so the *bridge* routes by what is happening on the thread: the pages get every change-set whole;
the app client gets only the entries **it** observed (the original bytes when nothing is cut, otherwise a re-encoding with
the same `txn_id`); the hub's own observation emits go to the pages as initial state and not to the app; the app's own
`Observe` replies go to the app only. Both sides share the runtime's flag, so the server records the app's `observe(off)` of a
store the hub holds without passing it to the runtime, and when the last page leaves it switches the observation off
(per store, all-or-nothing) and observes what the app asked for again: the app is sent the current values like any
`Observe`, so a write made in the instant between the two steps cannot be lost. The TypeScript runtime needs no `mode: "dev"`
change: the page decodes with `@undra/runtime/wire` and the schema the welcome carries. A cost, stated: a computed that
nothing shows is evaluated while a page is open.

### 3. The ring lives in the dev server, per core process, while a page is attached

A *step* is one `Runtime::snapshot` taken by the hub's worker after a burst of commits (coalesced over 10 ms, so a tap is one
step), tagged with the last change-set sequence and `txn_id` it covers. Bounds: 200 steps, 32 MiB in all and 4 MiB a step
(a state over the per-step bound is listed with its size, not kept, and cannot be restored); a snapshot equal to the newest
is not stored twice. The ring records only while a page is attached and is cleared when the last one leaves: nothing costs
anything, or changes anything for the app, until someone opens the page. It does not survive a reload (a rebuilt core is a new
process with a new hub and a new `core_epoch` in its welcome; the page marks the break and disables the old steps).
History is append-only.

### 4. Time travel is `Runtime::restore` of a ring snapshot, on the same runtime, through the app's own session

`Restore { step }` restores the step's snapshot (SPEC 5.9, all or nothing) in the hub's worker. The app client is restored
as a side effect: the restore emits change-sets for every observed signal, its mirrors converge, no client code is involved,
and the dev bar says `time travel: step N` through the dev notice of ADR-053. The restore is itself a commit labelled
`restore(step)` and a new step that records where it came from. Everything ADR-053 says about what a restore carries applies:
objects that are not stores and query handles go stale; in-flight calls on replaced stores are cancelled; stores built since the
step are dropped, and the answer says how many.

### 5. Port calls, the query cache and the counters

Calls to **platform-implemented** ports are recorded at the bridge (arguments, reply, status, and a latency on the server's
monotonic clock; Rust-bound ports never reach it); calls made while no app client is attached are counted, not listed
(a query's hydration retries ten times a second). The query cache is read through the one new runtime seam:
`Runtime::register_inspector(name, Arc<dyn Fn() -> String>)`, `Runtime::inspect(name)`, `Runtime::inspectors()`. An inspector
answers one JSON document, is called outside any lock, holds what it describes weakly (ADR-034) and is never read by the
core; `undra-query` registers `queries` the first time its client starts (entries with their key, status, observers, age, and
the wire encoding of data and error in hex, which the page decodes with the schema). The page derives the log of what
happened to each entry by comparing samples; nothing in the query hot path changed. Counters are `stats_json` once a
second plus the hub's own. **The mirror's drains, merges and backlog (SPEC 11.1) are counted by the app's runtime and never
reach the dev server**; the page shows the server-side counterparts (commits per second, commits merged per step, the app
connection's outbound backlog) and labels them as the server's.

### 6. Access: loopback by default and a token always (the integrator's requirement)

The endpoint exists only when `ServerConfig::devtools` is set, which only the dev runner `undra dev` generates sets; a production core has
no `Server` (`undra-transport` is not a dependency of `undra-ffi`, the wasm shell or any generated code), and `mode = "dev"`
of a runtime only adds log lines. `undra dev --devtools auto|on|off` (default `auto`) serves the page on a loopback `--addr`
only. **Every request carries `?token=`**, 128 bits from the operating system made per run of `undra dev` (not per runner: a
reload keeps the page's address valid), printed in the page's address and handed to the runner in its environment (not its
arguments, which `ps` lists). A missing or wrong token, an unknown path, devtools turned off and a method other than `GET`
are the *same* `404` (same status, headers and body), so the endpoint does not announce itself; the comparison reads every
byte. The `Origin` policy of the app socket applies to the devtools socket. The page's assets are a fixed table compiled into the
runner (no file is opened, so no path reaches anything outside it), served `no-store`, `nosniff`, `no-referrer`, with a
`Content-Security-Policy` of `script-src 'self'; style-src 'self'; connect-src 'self' ws: wss:`, and the index gets the token
substituted at serve time so the page can address its own files and socket.

### 7. The page is committed, built by a script, and checked by CI

`runtimes/ts/devtools` (TypeScript, strict, no framework; its only import is `@undra/runtime/wire`) builds with esbuild
(pinned) into `crates/undra-cli/assets/devtools/` (`index.html`, `app.js`, `app.css`), which is **committed** so that
`cargo build` needs no Node. `build.sh --check` rebuilds into a temporary directory and fails when it differs from what is
committed, and when the three files exceed 150 KB gzipped (they are about 17 KB); CI runs it with the page's typecheck and
tests. `undra dev` writes the assets beside the runner's sources and the runner `include_bytes!`s them.

## Alternatives considered

* **A `Devtools` envelope kind (and a "devtools attach" `Hello` mode).** Reuses the TypeScript `remote` transport as it
  is, and changes the envelope in four codecs and the golden vectors for a message only the dev server sends. The page needs
  a decoder of its own anyway (a generic one, driven by the schema), and a second endpoint keeps app clients out of it.
* **Observe-all in the TypeScript runtime** (`mode: "dev"` observing every store from the page's own `UndraCore`). The page
  would need the store handles (it cannot construct objects: that is the point), and the runtime's observed flag is the
  server's, so a page and an app client would still fight over it. The server is the only place that sees both.
* **Snapshot per commit, inside `Host::change_set`.** `Runtime::snapshot` is callable from a callback, but the callback runs
  inside the commit, under the store's delivery lock and usually the core lock: every commit would pay for a snapshot before the
  next could start, and a burst could not be coalesced. A worker thread takes it between commits.
* **Keeping the ring across a reload** (carried by `undra dev` next to the state). Restoring a snapshot taken by another build
  into a newer one is exactly what ADR-053 refuses across a schema change; a ring of old snapshots would put that choice in
  the developer's hand on every click. Left out: the history starts again.
* **A `QueryEvent` stream from `undra-query`.** Hooks in the fetch, retry, invalidate and gc paths for what the page can get by
  comparing two samples of the cache. Left out: one seam, no change to the query code's hot paths.
* **The assets read from a directory by the runner, or served by `undra dev` itself.** A file the runner opens is a path to
  get wrong and to traverse; a second HTTP server in the CLI means a second port and a page that is up while the core is not.
  Compiled into the runner, the page is where the core is.

## Consequences

* A developer opens one address and sees the core. Nothing changes for an app that never opens it, and an app that does is
  unchanged (it sees exactly what it observed), apart from the evaluation of unobserved computeds.
* The dev server now reads every store's values while a page is open, and keeps up to 32 MiB of snapshots. Both are bounded
  and both stop when the last page leaves.
* `undra-transport` grows a public module (`devtools::proto`, `DevtoolsConfig`, `Asset`) and `undra-runtime` one public seam;
  SPEC 5.10, 13 and 16.2 are updated; `docs/DEV_LOOP.md` gets a section.
* Time travel shares ADR-053's limits. It is not undo for the world: a request already sent, a port already answered and a
  timer already armed stay what they were.

## Amendments from the adversarial review (2026-10-02, `.10x/reviews/2026-10-02-devtools-review.md`)

None touches the wire, the ABI or a generated shape.

* **The socket's refusal is the page's `404`.** Section 6 said every refusal is the same `404`; the WebSocket upgrade at
  `/devtools/ws` answered with the WebSocket library's bare `404` instead. The token is now checked on the request line
  before anything is allocated and a request without it is answered by the code that answers every other refusal.
* **Pacing.** The worker's snapshots are at least 10 ms apart and at least nine times as far apart as the last one took
  (at most 2 s), and the query cache is sampled at most four times a second and nine times as far apart as the last sample
  took, with the cache's lock held only to copy its rows. Section 5's "sampled" and section 3's "coalesced over 10 ms" are
  those numbers for a small state.
* **Inspector panics (section 5's seam).** A panicking inspector is logged once (level 5, counted in `panics`) and skipped
  until a new one is registered under its name.
* **The app's notice** says how many stores a time travel dropped, as the page's answer does.
* **A worker that dies closes the pages** (they reconnect and get a new one); the hub stays active until the last has left,
  so the app client is never sent what it did not observe.

## Amendment (2026-10-02, ADR-059): time travel leaves live query handles alone

Decision 4's sentence "Everything ADR-053 says about what a restore carries applies: objects that are not stores and query
handles go stale" is amended by [ADR-059](ADR-059-transient-handles-across-restore.md) (the text above is left as written).
Objects that are not stores and are not re-creatable still go stale. A query handle does not: a restore into the runtime that
holds a live query handle leaves it exactly as it is, so a time travel sends no change-set for it, makes no refetch, keeps its
polling and any fetch in flight and the pages of an infinite query, **also for a handle created after the step** (a query
handle has no state in the snapshot for a restore to put back; dropping it would break a screen for nothing). The one handle
a time travel can still remove is one whose slot a store of the step needs (the slot was reused since): it is listed in
`RestoreReport::displaced` and added to the count of stores built since the step that the page's answer and the app's notice
report. The hub's store list leaves the snapshot's recreation records out, since they are not stores to list or observe
(observing one would build it), and the count of stores built since a step ignores them.
