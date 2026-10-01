# v1.1 / v1.2 — "the default choice" (design)

**Date:** 2026-10-01 · **Status:** plan approved in direction by the founder ("go"); the v1.2 bets in §6
await his explicit yes · **Owner:** 10x-team integrator

## 0. Goal and what "no limitations" means

The founder's bar: *no engineer declines Undra because of something it cannot do*; best-in-class
developer experience; rock solid; every quality-of-life gap closed; distribution deferred until
after v2. Operationally, "no limitations" is a checklist with three columns — **a thing a team
needs**, **what the alternatives offer**, **what Undra offers** — and the goal is that no row reads
"Undra: missing" by v1.2. Two documents produce that checklist (§1): a gap audit of our own code
and a sourced catalogue of the alternatives' limitations. The plan below is written from what we
know today and is amended when those land.

Definition of done for the whole program: every row closed or explicitly scheduled with a
version; every new runtime-model change behind an ADR and an adversarial review; contracts
extended for every new boundary behaviour; budgets for every new hot path; CI green; the
"why Undra is the default choice" post published with a limitations matrix a competitor's
maintainer would accept.

## 1. Discovery (first, in parallel)

* **Gap audit** (`.10x/specs/2026-10-01-v1x-gaps.md`, opus): from the code — type coverage of
  the boundary (every Rust type a domain model needs: numerics incl. `i128`/`f32`, tuples,
  nested `Option`/`Vec`/`Map`, recursive types, newtypes, `Duration`, decimals), the port
  adapter matrix per platform (iOS, Android, JVM desktop, web, Node, macOS), platform parity
  of hooks and counters (`onError`, `stats()`, drain listener), what `undra dev` preserves
  across a core rebuild, dev-client reconnect behaviour, `Effect` and lazy-list ergonomics,
  persisted-state behaviour across schema changes, the stream error-item ambiguity, off-runtime
  writes in release builds, a panicking computed's blast radius, `WeakCtx`. Output: a gap table
  with severity and the piece that closes it, plus draft ADRs for the runtime-model items.
* **Competitive limitations** (`.10x/specs/2026-10-01-competitive-limitations.md`, opus): Kotlin
  Multiplatform, UniFFI, Crux, React Native, Flutter, Compose Multiplatform, and "write it three
  times" — each limitation with a primary source and a date, mapped to Undra: solved / planned
  (which piece) / missing (proposal) — and, honestly, where each still beats us. This is the
  evidence base for the blog post and for the checklist.

## 2. The upgrade map

### Track A — Rock solid: runtime lifecycle and error semantics (v1.1)
A1 **`WeakCtx`** (ADR-034): long-lived tasks (timers, streams, event subscribers) hold a weak
reference to the runtime so a dropped or shut-down runtime is released and the task ends with
a typed outcome. A2 **Off-runtime writes are a typed error, never a silent drop** (ADR-035): a
write from a thread the runtime does not own fails loudly in release as in debug. A3
**Per-signal isolation of a panicking computed** (ADR-019 follow-up): one poisoned computed
does not take its store down. A4 **Stream error items are typed** (ADR-036, a wire change made
while nothing is published): flag 2 carries `E` when the stream has one and a typed
`UndraCallError` otherwise, never a bare string. A5 **Persisted state across schema changes**
(ADR-037): versioned snapshots, query caches and offline queues with migration hooks, so an
app update never silently discards a user's queued work.

### Track B — The dev loop (v1.1, devtools v1.2)
B1 **Android `undra dev` remote mode** (iOS and web have it). B2 **Dev-client auto-reconnect**
on all three platforms with backoff, state resync via re-observe, and a visible status. B3
**State-preserving core reload**: `undra dev` snapshots before a rebuild and restores after, so
editing Rust never loses the screen you were on. B4 **Devtools** (v1.2): a page served by the
dev server — store state viewer, change-set timeline with time-travel (restore), port-call and
query-cache logs, perf counters (drains, merges, backlog) — on the devtools protocol SPEC 5.10.

### Track C — Schema and codegen (v1.1)
C1 **Full-JSON `undra_schema_json`** so the dlopen path keeps docs. C2 **Swift `Port*` types
public** (drops the bindgen fallback; ADR-024 amendment). C3 **Type-coverage audit → support or
a teaching error** for every reasonable boundary type. C4 **Platform parity audit → fixes**:
hooks, counters, adapters matrix, with contract scenarios for anything that differs. C5
**Desktop as first-class targets**: Compose Desktop (JVM) and macOS (SwiftUI) samples built in
CI over the existing runtimes.

### Track D — Diagnostics and quality of life (v1.1)
D1 **Macro diagnostic polish** (query-in-impl, split-impl follow-ons, NF1/NF2 wording) and an
**E-code audit**: every code has what/why/fix/docs anchor and a compile-fail test; the
error-codes page regenerates. D2 **`undra upgrade`** (bump the dependency, regenerate, print
migration notes). D3 **Build-system integration for production builds**: a Gradle plugin task,
an Xcode build phase emitted by `undra init`, a Vite plugin — so `undra build` is never a manual
step. D4 **`undra init` emits CI workflows** for the three apps. D5 **`undra doctor`** depth:
every prerequisite diagnosed with the fix command.

### Track E — Performance (v1.1 rows, v1.2 lists)
E1 **Device-measured benchmark rows**: tooling (`undra bench --device`) that runs the
playground's bench hooks on an attached iPhone/Android and on Chromium, writes JSON, and fills
`bench/RESULTS.md`; simulator/emulator/Chromium rows now, real-device rows the moment hardware
is attached; Kotlin and Swift drain measurements; the `undra init` Android template uses
`ChoreographerFramePacer`. E2 **Derived keyed lists** (v1.2, ADR): incremental filter/sort/map
producing keyed patches, so `visible: Computed<Vec<Todo>>` over 10,000 rows costs O(change).
E3 **Lazy-list ergonomics**: generated paging APIs idiomatic on each platform.

### Track F — Testing kit for app developers (v1.1)
F1 **Preview/fake cores**: SwiftUI previews, Compose previews and Storybook run against an
in-memory core with the Rust fakes, or against recorded change-sets. F2 **Port record/replay**
fixtures: record a real session's port traffic, replay it deterministically in tests.

### Track G — Reach (v1.2, founder's yes required)
G1 **React Native runtime** (`@undra/react-native`: a TurboModule over the C ABI, the existing
TS mirror on top) — teams that keep RN for UI can still put Undra under it. G2 **WebSocket
port** (bidirectional backpressured streams) and SSE. G3 **`Db` port** (SQL over SQLite on
iOS/Android/JVM, wa-sqlite on web) for structured local storage with the same deterministic
fake story. G4 **Flutter/Dart bindgen** — v1.3 candidate, decided after G1.

### Track H — Docs and the narrative (v1.1 docs, v1.2 post)
H1 **Cookbook**: auth, pagination and infinite lists, forms and validation, file upload,
real-time, offline-first, migration from KMP / UniFFI / an existing app (adoption levels L1–L3).
H2 **A production-shaped sample** beyond the playground. H3 **API reference** for the Rust side
(rustdoc on the site) and per-platform generated surfaces. H4 **The post**: "Why Undra is the
default choice" — the limitations matrix from §1 with what we solved and how, fact-checked
adversarially before publication.

## 3. Sequencing and ownership

Phase 1 (now): §1 discovery (two opus analysts) in parallel with the unambiguous, founder-listed
pieces: `dev-loop` (B1+B2), `schema-json` (C1+C2), `diagnostics` (D1), `device-bench` (E1).
Phase 2: Track A implementation from the ADRs; C3/C4 fixes from the gap audit; B3; D2–D5; F.
Phase 3 (v1.2): B4, E2, G1–G3, H4. H1–H3 run alongside phases 2–3 as APIs settle.

| Piece | Worktree | Implementer | Reviewer | Needs first |
|---|---|---|---|---|
| gap audit + ADR drafts 034–037 | `wt/gaps` | opus | integrator | — |
| competitive limitations | `wt/competitive` | opus | fable (fact-check at publication) | — |
| dev-loop (B1, B2) | `wt/dev-loop` | sonnet | opus | — |
| schema-json (C1, C2) | `wt/schema-json` | sonnet | opus | — |
| diagnostics (D1) | `wt/diagnostics` | sonnet | opus | — |
| device-bench (E1) | `wt/device-bench` | sonnet | opus | — |
| runtime lifecycle (A1–A4) | `wt/runtime-lifecycle` | opus | opus | ADR-034/035/036 accepted |
| persistence migrations (A5) | `wt/persistence-v2` | opus | opus | ADR-037 |
| type coverage + parity (C3, C4, C5) | `wt/parity` | sonnet | opus | gap audit |
| state-preserving reload (B3) | `wt/dev-reload` | sonnet | opus | dev-loop |
| QoL tooling (D2–D5) | `wt/tooling` | sonnet | fable | — |
| testing kit (F1, F2) | `wt/testkit` | sonnet | opus | parity |
| devtools (B4) | `wt/devtools` | opus | opus | dev-reload |
| derived keyed lists (E2) | `wt/derived-lists` | opus | opus | ADR |
| React Native runtime (G1) | `wt/react-native` | opus | opus | founder yes |
| WebSocket + Db ports (G2, G3) | `wt/ports-v2` | sonnet | opus | founder yes; ADR |
| docs: cookbook, sample, reference (H1–H3) | `wt/docs-v1x` | sonnet | fable | phase 2 APIs |
| the post (H4) | `wt/default-choice-post` | sonnet | fable | §1 + phases 2–3 |

Merge discipline is unchanged: review → full local matrix → merge → CI green → `state(<piece>)`.

## 4. Quality bar per piece

Every piece: unit tests; a contract scenario when boundary behaviour changes (the grid grows
past 18 × 3); a budget row when a hot path is touched; docs on every `pub` item; the generated
code passes native review (R3); an adversarial review with findings fixed before merge.

## 5. Out of scope

Distribution (brew/npm/crates.io/Maven) — deferred by the founder until after v2. Shared UI of
any kind. Hosted services.

## 6. Decisions the founder makes (the v1.2 bets)

Each is a product bet with real agent cost; the plan proceeds without them and adds them on a yes:
1. **React Native runtime** (G1) — the biggest market expansion; moderate complexity (JSI over the C ABI).
2. **Devtools / inspector** (B4) — the biggest DevX differentiator; the protocol exists.
3. **`Db` port with SQLite adapters** (G3) — structured local storage; the most work of the three.
4. **WebSocket port** (G2) — real-time apps; small.
5. **Derived keyed lists** (E2) — O(change) filtered/sorted views over large lists; medium.
6. **Flutter/Dart bindgen** (G4) — after G1 proves the pattern.

## Amendment A — the founder approved all six v1.2 bets (2026-10-01)

"All." Sequencing, chosen for dependencies and machine load (every piece builds Rust; more than
seven concurrent pieces makes timing-sensitive suites lie):

| Bet | Starts | Depends on | Owner |
|---|---|---|---|
| G1 React Native runtime | now (`wt/react-native`) | nothing structural — ADR-038 first, then code | opus design + implement, opus review |
| E2 derived keyed lists | ADR now (`wt/derived-lists`), code after Track A | ADR-039; Track A lands (same crates) | opus design, opus implement, opus review |
| G2 WebSocket port | after the first wave merges | ADR-040 (a port with two backpressured streams) | sonnet, opus review |
| B4 devtools | after `dev-loop` and B3 merge | the dev server, state-preserving reload | opus, opus review |
| G3 `Db` port (SQLite) | after ADR-037 and G2 | persistence model (ADR-037); the port pattern of G2 | opus design, sonnet implement, opus review |
| G4 Flutter/Dart bindgen | after G1 proves the fourth-host pattern | G1 | opus design, sonnet implement, opus review |

## Amendment B — findings from the competitive catalogue (2026-10-01)

`.10x/specs/2026-10-01-competitive-limitations.md` (68 sourced limitations, a 36-row matrix,
18 ranked missing items) adds pieces the first draft did not have. They are scheduled now:

| New piece | Track | What | ADR | Starts |
|---|---|---|---|---|
| **Android platform adapters** | C4 → its own piece | real Kv, SecureStore, Fs, Http, Connectivity and Lifecycle adapters in `android-adapters`, installed by default, instrumented tests on the emulator, the playground stops faking them | none (adapters) | now, `wt/android-adapters` |
| Boundary surface | C3 escalated | objects as parameters and returns (E0064), host callback interfaces / listeners (E0004), newtypes (E0007), limited generics (E0002) | ADR-040 objects, ADR-041 callbacks, ADR-042 newtypes and generics | ADRs now (`wt/boundary-adrs`), code in phase 2 |
| Data layer completion | A5/E3 re-scoped | interval polling (SPEC §9 promises it), paged and infinite queries with lazy lists, queued offline mutations keep their optimistic state and invalidations across restarts | ADR-043 paged queries and lazy lists; A5's ADR-037 covers the queue | ADR now, code after ADR-037 |
| Production operations | new Track I | crash-symbol files (dSYM, wasm source maps, Android symbols) from `undra build`, a documented debugging path into Rust on each platform, OS background execution to drain the offline queue (BGTaskScheduler / WorkManager through the Lifecycle port) | ADR-046 | phase 2 |
| Multiple cores per app | compatibility | per-library symbol namespacing so two Undra libraries can coexist in one process | ADR-044 (decided together with ADR-038) | ADR now |
| iOS floor | compatibility | an iOS 15/16 mode for generated stores (`ObservableObject` where `Observation` is unavailable) | ADR-045 | ADR now |

Renumbering: the WebSocket port becomes ADR-047, the `Db` port ADR-048. The post must not reuse
the blueprint claims the catalogue found unbacked (time-travel devtools, a worker core by default,
lazy collections, newtypes, optimistic state surviving restarts, Telemetry/Push ports, `undra adopt`)
until the code backs them.

## Amendment C — decisions from the gap audit (2026-10-01)

`.10x/specs/2026-10-01-v1x-gaps.md`: 71 gaps (7 block adoption, 40 hurt, 24 polish); ADR-034…037
drafted and needed. Integrator decisions:

1. **ADR-034, 035, 036, 037 are accepted in direction**; they flip to Accepted with their
   implementation. ADR-036 (typed stream errors) and ADR-037 (persisted-state migrations, which
   changes the `Snapshot` payload) ship as **one wire revision** — the last before publication;
   the earlier architect note ("A4 is the last wire change") is superseded by this amendment.
2. **New pieces**: A6 web-core crash recovery (restart from the last snapshot, typed outcome to the
   app); A7 storage ports gain an error channel and worker-mode sync ports work (ADR-049, amending
   ADR-024/025); PO-4 (`wasm-worker` traps on the first Clock/Rng/Log call) is fixed as a bug in
   the parity piece if it needs no ADR, else under ADR-049.
3. **C4 is split**: C4a Android adapters (in flight); C4b the Kotlin and TypeScript failure model
   to ADR-032's standard (commands never throw into UI callbacks, `onError`, a closed set of
   error types, wire errors under one base) as a dated ADR-032 amendment; C4c snapshot/restore
   parity for TypeScript.
4. **A3** (a panicking computed is isolated per signal) is recorded as a dated ADR-019 amendment
   before code, inside the Track A piece.
5. **C3** is covered by ADR-040/042 (boundary-adrs piece); decimals and `uuid`/`chrono` types join
   ADR-042's scope; the recursive-record compile failure is a bug fixed in the parity piece.
6. Port cancellation (reopens the 19-function ABI) is deferred to v1.2 as its own ADR.
7. RX-1/RX-2 (a one-row edit in 10,000 rows ships 192,647 B through a derived `Computed<Vec<T>>`
   vs 39 B as a keyed patch) is the measured baseline for ADR-039.

Ownership: `runtime-lifecycle` (ADR-034/035/036 + ADR-019 amendment; opus), then
`persistence-v2` (ADR-037 + A6; opus); `parity` (C4b, C4c, PO-4, TY recursive bug; sonnet, opus review).
