# SDE: the site describes the typed error channel (wt/site-errors, 2026-10-01)

The `parity` piece gave Kotlin and TypeScript the closed error set Swift got in ADR-032 (amendment A and its addendum).
The Kotlin and TypeScript API pages still described the old behaviour. Source of truth for every sentence: `docs/ERRORS.md`,
`docs/DEV_LOOP.md`, SPEC 17, `runtimes/kotlin/undra-runtime/README.md`, the runtimes' own doc comments and the
bindgen `full` golden bindings (`TodoStore`, `TodoError`). No code, ADR, SPEC or schema change.

## What changed

* `site/docs/api-kotlin.html`, `api-typescript.html`: Errors (a call fails with its own `E`, the platform's cancellation
  or `UndraCallError`; the five-case table with the real member names; commands report through `onError`; the handler's
  thread and its one rule; a lost connection is logged, not reported), Loading the core (the `shared` placeholder, check
  `current`), Objects and functions (a command, a stream's errors), a `// a command` comment on the store samples.
  TypeScript also: `UndraSchemaMismatch` is `UndraSchemaMismatchError`; only an async method takes an `AbortSignal`.
* `site/docs/api-swift.html`: the same lost-connection paragraph, the reconnecting and rebuilt cases in the
  `.unavailable` row and the `current` hint, so the three pages agree (same sections, same order, same wording where the
  platform allows).
* `site/docs/cli.html`: one sentence on what a call and a command do while `undra dev` is reconnecting.
  `site/docs/getting-started.html`: "reload the app to reconnect" became "the app reconnects by itself" (ADR-051).
* `site/blog/reads-never-cross-the-boundary/index.html` (its own commit): the paragraph that said TypeScript rejects
  with the reply error and Kotlin throws `UndraReplyException`, and the Kotlin name of the schema error.
* Generated: `llms-full.txt`, `search-index.json`, `sitemap.xml` (`build-all`). `errors.html` is generated and was already
  up to date; none of its sentences contradict the rule (its two port-panic entries describe the Rust side).
  `dev-loop.html` does not exist: the dev loop is on `cli.html` and `getting-started.html`.

## Verification

* Every Kotlin and TypeScript sample that can compile is built from the page's own HTML (a generator extracts the code
  blocks, so the proof is of the published text) against the real runtime and the golden bindings: `kotlinc -jvm-target 11
  -Werror` (Kotlin 2.4.20) and `tsc --strict` (5.9.3, the runtime's own flag set). A harness also builds the real
  `TodoStore` call site, a command call without `try`, and an exhaustive `when` / `switch` over the five cases using every
  member the tables name. Negative controls (a wrong member name, the old `UndraSchemaMismatch`) fail as they should.
* `node site/scripts/build-all.mjs` (no diff on a second run), `check-links.mjs` (20 pages OK), `--words` (342, budget
  350, unchanged), `sync-chrome.mjs --check`; every changed page loaded in the browser pane with no console errors (Kotlin,
  TypeScript and the blog post in the dark theme, Swift in the light one), and the five docs pages checked at phone width
  with no horizontal scroll.

## For the integrator

* `site/data/roadmap.json` and `.10x/handoff.md` (the v1.x queue) still list "Android undra dev remote mode" and
  "Dev-client auto-reconnect" as upcoming; `.10x/status.md` has them merged (ADR-051, `a0d638f`).
* `site/blog/undra-vs-kotlin-multiplatform/index.html` (tooling weight) still says to reload the app to reconnect and
  that Android has no remote transport. Left alone: a dated post, and not about errors.
