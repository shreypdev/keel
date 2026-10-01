# Architect: devtools (B4, v1.2) design note

2026-10-01, `wt/devtools`. Binding text: ADR-054 (written with the code), SPEC 5.10. Read: SPEC 5.9, 5.10, 11,
ADR-051, ADR-053, `undra-transport`, the runner template.

## What 5.10 already carries, and why it is not enough

A client whose `Hello` says `mode = "dev"` gets `Log` records with the target `undra::devtools`: `commit txn=T
entries=N bytes=B`, `port call <port>.<method> id=I args=N bytes`, `port call id=I completed in N ns` (and the dev
notices of ADR-053). They are text, carry no values, no arguments and no replies, exist only for *observed* signals, and
go to the one app client. The page needs values, a diff, history, a restore and the query cache.

## Decisions

1. **A second endpoint, no new envelope kind.** The dev server's listener also answers `GET /devtools[/..]` with the
   page (assets compiled into the runner, which `undra dev` generates) and upgrades `/devtools/ws` into a *devtools
   connection*: its own binary messages `[tag u8][body]` (documents as JSON text, change-sets and records in the wire's
   primitives), never an envelope. The app slot ("one client at a time", ADR-051) is untouched, so the page and the
   app coexist; Swift, Kotlin and TypeScript runtimes, the wire, both ABIs and the schema do not change. ADR-054.
2. **Observe-all is the server's job.** While at least one devtools page is attached the hub observes every store
   through `Runtime::observe`, and the bridge routes: devtools get every change-set whole, the app client gets only the
   entries it observed (a filtered re-encode, the original bytes when nothing is cut), and the hub's own initial
   change-sets go to devtools only. The runtime's observed flag becomes the union app + hub, so neither can switch the
   other's off (the server swallows the app's `observe(off)` for a store the hub holds, and re-states the app's set when
   the hub leaves). The TS runtime needs no `mode: "dev"` change: the page decodes with `@undra/runtime/wire` and the
   schema (sent in `Welcome`). Cost: computeds nobody shows are evaluated while a page is open; documented.
3. **The ring lives in the dev server, per core process, recording only while a page is attached.** A *step* is one
   `Runtime::snapshot` taken by the hub's worker after a burst of commits (coalesced over ~10 ms, so one click is one
   step), tagged with the last change-set sequence it covers. Bounds: 200 steps, 32 MiB in all, 4 MiB per snapshot (a
   bigger state is listed but not restorable); an identical snapshot is not stored twice. It does not survive a reload
   (new runner, new hub; the page draws a divider and disables the old steps). History is append-only.
4. **Time travel = `Runtime::restore` of a ring snapshot, on the same runtime, through the app's own session.** The
   app client is restored as a side effect: restore emits change-sets for every observed signal and its mirrors converge
   (no client code). The restore is itself a timeline entry (`cause = restore(step)`) and a new step; the app's dev bar
   says `time travel: step N` through the existing dev notice (`undra::dev`), and the reply tells the page which stores
   were dropped (a store built after the step has no snapshot: its handle goes stale, as in ADR-053).

Smaller choices: the port log is taken at the bridge (calls to platform-implemented ports, with arguments, reply,
status and a host-side monotonic latency); the query cache view is a new `Runtime::register_inspector` seam (the cache
lives in a runtime extension the transport cannot name), sampled by the worker and turned into events by diffing;
counters are `stats_json` at 1 Hz plus the hub's own. The mirror's drains/merges/backlog (SPEC 11.1) are client-side
numbers the dev server never sees; the page shows their server-side counterparts (commits per second, commits merged
per step, outbound backlog) and says which is which. A change-set is labelled with the sync call that caused it
(thread-local around `Runtime::call`), else `async`/`restore`.

Security: the endpoint exists only when `ServerConfig::devtools` is set, which only the dev runner does; a production
core has no `Server` (`undra-transport` is not a dependency of `undra-ffi`, the wasm shell or generated code), and
`mode = "dev"` of a runtime only adds log lines. `undra dev --devtools auto|on|off` (default auto) serves it on a
loopback address only; a LAN `--addr` needs `on`. Same `OriginPolicy` as the app socket; assets are a fixed table (no
filesystem), `no-store`, CSP.

Assets: `runtimes/ts/devtools/` (TypeScript, esbuild, no framework) builds to `crates/undra-cli/assets/devtools/`
(committed; `build.sh`, `build.sh --check` in CI; <= 150 KB gzipped, a test).
