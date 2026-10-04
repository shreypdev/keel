# Launch v2 - win the argument (design)

**Date:** 2026-09-30 · **Status:** approved by the founder ("you know the best, so you decide"; the product name **Undra** was the founder's pick from the shortlist) · **Owner:** 10x-team integrator

v1 shipped under the working name Keel (see `.10x/status.md`). v2 of the *launch* (not
of the framework) renames the product to **Undra**, and turns the site and the
distribution into the vehicle that wins the argument with platform engineers: premium,
fast, honest, developer-obsessed, installable in one line, with comparison posts that
survive expert scrutiny, a roadmap that shows momentum, and stress numbers that end the
"but does it hold up" question.

## 0. North star and success criteria

North star: *every app team writes its domain once, in Rust, and every platform gets the
code its own engineers would have written.*

Success for this batch, all verifiable:

1. The repository, every crate, every generated shape, every runtime package and every
   document say **Undra**; the old name survives only in immutable history
   (`.10x/reviews/`, ADR-018…029) and in the rename script. All suites green.
2. `brew install shreypdev/undra/undra`, `npm i -g @undra/cli`, `curl … | sh` and
   `cargo install --git …` each produce a working `undra` (proved by the release
   workflow's dry run and the runbook's smoke steps).
3. The site scores ≥ 95 on Lighthouse performance / accessibility / best-practices / SEO,
   has zero italic display type, and every page has a unique title, description, canonical,
   Open Graph card and JSON-LD.
4. Four comparison/architecture posts with every competitor claim fact-checked in an
   adversarial review, each with a "when the other tool is the better choice" section.
5. A roadmap page with shipped / now / next / later, sourced from `.10x/handoff.md`.
6. A "harsh conditions" benchmark suite with CI-gated budgets, its numbers on the landing
   page, and a live in-browser stress demo running the real wasm core.
7. CI fully green on `main` (the schema-strip fix landed).

## 1. Identity (ADR-030)

Facts found 2026-09-30: "keel" is a crowded name in developer tooling: keel.sh
(Kubernetes), keel.so (owns the npm package `keel`), the npm user `keel` (so `@keel/*`
was never claimable), another active Rust project "keel" (crates.io `keel-cli`,
`keel-macros`, npm `@getkeel`, `@keel-dev`), and `keel.dev` is someone else's site. After a
search of ~100 candidates across npm (scope and unscoped), crates.io, GitHub, Homebrew
and domains, the founder chose **Undra** ("under" made into a word; Swedish *undra*, "to
wonder"): free as an npm scope and package, free on crates.io, no software collision.

Decisions:

* **Product name Undra.** CLI `undra`; Rust facade crate `undra` (`use undra::prelude::*`,
  `#[undra::store]`); crates `undra-*`; C ABI symbols `undra_*` and header `undra.h`;
  Swift module `UndraRuntime`; Kotlin package `dev.undra.runtime` (JNI `Java_dev_undra_…`);
  TypeScript `@undra/runtime` (adapters as subpaths `@undra/runtime/react` …); env vars
  `UNDRA_*`; shim library `undra_core`; playground bundle ids `dev.undra.playground`.
* **npm scope `@undra`**: `@undra/runtime`, `@undra/cli` (meta package, `bin: undra`) +
  `@undra/cli-{darwin-arm64,darwin-x64,linux-x64,linux-arm64}` (optionalDependencies, as
  esbuild). The unscoped `undra` name is reserved with a stub that points to `@undra/cli`.
* **crates.io**: `undra`, `undra-cli`, `undra-*` are free; publishing is v1.1 (needs its
  own ADR for the crates' public API review) - this batch only ensures the names are ours
  by naming the workspace packages `undra-*` now.
* **GitHub**: repository `shreypdev/keel` → `shreypdev/undra` (GitHub redirects the old
  URLs); Homebrew tap `shreypdev/homebrew-undra`, formula `undra`
  (`brew install shreypdev/undra/undra`). An organisation (`undra-dev` if free) is a
  later founder step; the tap and the repo can move under it without breaking anything.
* **Site**: `https://shreypdev.github.io/undra/` (a custom domain - `undra.rs` was free
  at the time of writing - is a founder purchase and a roadmap item).
* **curl installer** at `https://shreypdev.github.io/undra/install.sh`: detects OS/arch,
  downloads the release asset, verifies sha256 against `checksums.txt`, installs to
  `~/.undra/bin`, prints the PATH line. Never `sudo`, fail closed.
* **cargo**: `cargo install --git https://github.com/shreypdev/undra undra-cli`.

## 2. Site v2

### 2.1 Typography - the founder's first complaint

* **Geist** (sans, weights 400/500/600) for everything textual, **Geist Mono** (400/500)
  for code, eyebrow labels, numbers and table data. Both via Google Fonts (already wired),
  `display=swap`, preconnect. **Instrument Serif is removed. No italic display or headline
  type anywhere.** Prose `<em>` may stay italic in blog body text only.
* Scale: h1 `clamp(2.5rem, 6vw, 4.5rem)` / 600 / letter-spacing −0.03em / line-height 1.05;
  h2 2rem / 600 / −0.02em; h3 1.25rem / 600; body 1.0625rem / 1.6; eyebrow mono 0.75rem
  uppercase tracking 0.08em; numbers `font-variant-numeric: tabular-nums`.

### 2.2 Design system - "premium, modern, developer-obsessed"

Reference the qualities of Linear, Vercel, Bun, Zed and Biome: restraint, one accent,
thin borders, mono labels, code as a first-class element, calm motion.

* Dark default (unchanged), light via the existing toggle; both palettes polished.
* Dark: canvas `#0a0a0b`, surface `#111114`, elevated `#17171b`, border
  `rgba(255,255,255,.08)` (hover `.14`), text `#f4f4f5` / secondary `#a1a1aa` / muted
  `#71717a`. Light: canvas `#fafafa`, surface `#ffffff`, border `rgba(0,0,0,.08)`, text
  `#18181b` / `#52525b`. **One accent** in the teal–cyan family (dark `#22d3ee`, light
  `#0e7490`); keep the current accent if it already is in that family. No multi-hue
  gradients; at most one subtle radial glow behind the hero.
* Surfaces: 1px borders, 12px radius, a 3%-opacity dot grid on the canvas, glass sticky nav
  (`backdrop-filter: blur(12px)`, bottom border). Bento grid for features. Code blocks
  with a header bar (language / filename) and a copy button. A terminal block with a typed
  animation of real commands. Tabs are pills, keyboard-operable (arrow keys, roving
  tabindex).
* Motion: 150–250 ms ease-out; reveal on scroll is opacity + 8px translate only; the hero
  diagram animates continuously but calmly; everything honours `prefers-reduced-motion`.
* Accessibility: AA contrast, visible focus rings, skip link, landmarks, alt text, tabs and
  toggles with ARIA. Performance: no third-party JS, CSS ≤ 40 KB, landing transfer
  ≤ 300 KB before the lazy demo, images lazy, iframe mounted only when scrolled into view.

### 2.3 Information architecture

```
/                       landing
/docs/…                 existing 10 pages (search, prev/next, "Edit on GitHub", copy buttons)
/blog/                  index (cards, RSS link) - generated from the posts by a script
/blog/<slug>/           4 posts (Section 3)
/roadmap/               shipped · now · next · later
/playground/            the real web playground, built in CI (Section 2.6)
/install.sh             curl installer (owned by the distribution piece)
/feed.xml /sitemap.xml /robots.txt /llms.txt /llms-full.txt /404.html
```

### 2.4 Landing page sections, in order

1. **Hero**: eyebrow "Rust core · native UI"; H1 "One Rust core. Native everywhere."; one
   sentence thesis; the **install block** (tabs `brew` · `npm` · `curl` · `cargo`, each a
   one-liner with a copy button; brew first): `brew install shreypdev/undra/undra`,
   `npm install -g @undra/cli`, `curl -fsSL https://shreypdev.github.io/undra/install.sh | sh`,
   `cargo install --git https://github.com/shreypdev/undra undra-cli`; CTAs "Get started"
   and "See it live"; the animated diagram (existing `hero.js`, refined to the new palette).
2. **60 seconds to a native app**: terminal block typing `undra init myapp` → `undra dev`
   → the three shells, with the real output shapes.
3. **See it live**: the playground iframe (`playground/?screen=list&stream=1&embed=1`)
   with live counters rendered *outside* the iframe from `postMessage` stats: change-sets
   applied per second and p50/p99 apply time, labelled "measured in your browser". A
   "Push it" control switches to the stress screen when it exists (`screen=stress`).
4. **Numbers**: the bench table from `site/data/bench.json` (host rows + the harsh-
   conditions rows, each with its budget and a link to `bench/RESULTS.md`).
5. **How it works**: three-column generated code (Swift / Kotlin / TypeScript) tabs, the
   "reads never cross" explanation, one diagram.
6. **Features** bento: stores, queries/mutations, offline queue, ports, contracts, dev loop,
   diagnostics, snapshot/restore.
7. **Trust**: tests across five languages, 17 contract scenarios × 3 platforms, four
   adversarial reviews, ASan/Miri - every number linked to its source.
8. **Compared**: teaser cards for the four posts.
9. **Roadmap** teaser: three "now" items and a link.
10. **Final CTA** + footer (GitHub, docs, blog, roadmap, RSS, license).

### 2.5 SEO - indexed fast, found for the right phrases

* Brand phrase everywhere: **"Undra, the Rust core for native apps"**. Target long-tail:
  *share business logic between iOS and Android*, *Kotlin Multiplatform alternative*,
  *UniFFI alternative*, *Rust iOS Android web framework*, *SwiftUI Compose React shared
  logic*, *native UI shared core*. "Undra" has no software competitor for the term.
* Every page: unique `<title>` (≤ 60 chars), description (≤ 155), canonical, `og:*` and
  `twitter:card=summary_large_image`, `og:image` → `assets/og.png` (1200×630, rendered by
  `site/scripts/render-og.sh` with headless Chrome from `site/og/index.html`, committed).
* JSON-LD: `WebSite` + `Organization` (landing), `SoftwareSourceCode` +
  `SoftwareApplication` (landing), `TechArticle` + `BreadcrumbList` (docs), `BlogPosting`
  (posts), `ItemList` (roadmap).
* `sitemap.xml` with `lastmod`, `robots.txt`, RSS `feed.xml`, `llms.txt` and
  `llms-full.txt` (the docs concatenated - agents discovering Undra should get the whole
  thing in one fetch).
* IndexNow: `site/<key>.txt` + a post-deploy step in `site.yml` that submits the sitemap
  URLs to `api.indexnow.org` (Bing, Yandex, DuckDuckGo pick it up; Google is via Search
  Console, a founder action documented in `docs/SITE.md`).
* GitHub repository topics (integrator): `rust`, `ios`, `android`, `swiftui`,
  `jetpack-compose`, `react`, `cross-platform`, `state-management`, `wasm`, `ffi`,
  `kotlin-multiplatform-alternative`.

### 2.6 Build and tooling (static site, zero runtime dependencies)

* `site/scripts/build-search-index.mjs` (Node, no deps) → `site/search-index.json`
  (committed; `site.yml` regenerates and fails if it differs). `site/assets/search.js`
  implements ⌘K search over it.
* `site/scripts/build-blog-index.mjs`: reads every `site/blog/*/index.html`, extracts
  title / description / date / reading time, and writes the cards into
  `site/blog/index.html`, `site/feed.xml` and the blog entries of `site/sitemap.xml`.
  Handles zero posts.
* `site/scripts/build-llms.mjs`: `llms.txt` (index) and `llms-full.txt` (docs as text).
* `site/scripts/sync-chrome.mjs`: copies the `<header>` and `<footer>` blocks from
  `site/index.html` into every other page, so the chrome is edited once.
* `site/scripts/check-links.mjs`: every internal link resolves; every page has title,
  description, canonical, `og:image`, JSON-LD. Run in `site.yml`.
* `site/scripts/build-local.sh`: stages `_site/` = `site/` + the playground build, for
  local preview (`python3 -m http.server` or the built-in browser).
* `site.yml`: install the wasm32 target and `binaryen`; `cargo run -p undra-cli -- build
  --platform web -C examples/playground`; `npm ci && npx vite build
  --base=/undra/playground/` in `examples/playground/web`; stage into `_site/playground/`;
  run the search-index freshness check and the link checker; upload `_site`; after deploy,
  IndexNow ping.
* Playground web app gains URL parameters: `screen=` (todos|counter|list|remote|stress),
  `stream=1` (start the 10/s stream), `embed=1` (compact chrome, no tab bar) and posts
  `{ type: "undra-stats", changeSetsPerSec, applyP50Us, applyP99Us }` to `window.parent`
  every 500 ms in embed mode. The measurement wraps the runtime's apply of one change-set
  with `performance.now()`; label it exactly as what it is.

### 2.7 Roadmap page content (source of truth is `.10x/handoff.md`; keep in sync)

* **Shipped - v1.0 (2026-09-30)**: records, enums, typed errors, objects, stores
  (signals, computeds, keyed lists), 10 ports with platform adapters and Rust fakes,
  queries and mutations (staleness, dedup, retry, optimistic rollback, offline queue,
  persistence), streams with backpressure, cancellation, snapshot/restore, schema-hash
  gate, `undra dev` live core, teaching diagnostics, 17 contract scenarios × 3 platforms,
  adversarial reviews of every core crate.
* **Now - v1.x (in flight)**: distribution (brew, npm, curl, cargo-from-git); harsh-
  conditions benchmark suite with CI budgets; device-measured benchmark rows (iPhone,
  Android, Chromium); schema loader that survives dead-stripping; Android `undra dev`
  remote mode; dev-client auto-reconnect; `WeakCtx`; macro diagnostic polish; Swift
  `Port*` types public; full-JSON `undra_schema_json`.
* **Next - v1.1 / v1.2**: crates.io publishing; frame-coalesced delivery if the stress
  suite shows it is needed (ADR first); Maven Central for the Kotlin runtime; Windows CLI;
  `undra upgrade`; migration guides (from KMP, from UniFFI); a custom domain.
* **Later - horizon**: desktop targets over the same C ABI (macOS, Windows, Linux); a
  first-party inspector (time-travel over change-sets - the wire makes it natural); a sync
  engine as a separate package; Compose Multiplatform interop.
* Explicitly not planned: shared UI of any kind, hosted services.

## 3. Blog

Slugs (fixed; the site piece links to them): `undra-vs-kotlin-multiplatform`,
`undra-vs-uniffi-and-crux`, `why-undra-keeps-your-ui-native`,
`reads-never-cross-the-boundary`. URL `/blog/<slug>/` (`site/blog/<slug>/index.html`).

Rules: every claim about another tool is stated in a way the other tool's maintainers would
accept as accurate on the day of writing, with the date; each comparison post has a
"When X is the better choice" section; numbers about Undra link to `bench/RESULTS.md` or
the test counts in the README; no benchmarks *of competitors* we did not run. Author line:
"Undra team". Reading time computed from the word count.

Post skeleton (the contract between the content and the styling pieces):

```html
<!doctype html><html lang="en"><head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>… | Undra blog</title><meta name="description" content="…">
<link rel="canonical" href="https://shreypdev.github.io/undra/blog/<slug>/">
<!-- og:*, twitter:*, BlogPosting JSON-LD -->
<link rel="stylesheet" href="../../assets/base.css"><link rel="stylesheet" href="../../assets/blog.css">
</head><body data-page="blog">
<!-- <header> … </header> copied verbatim from site/docs/index.html at time of writing -->
<main class="post"><article>
  <header class="post-header">
    <p class="eyebrow">Comparison</p><h1>…</h1><p class="lede">…</p>
    <p class="post-meta"><time datetime="2026-09-30">30 Sep 2026</time> · 9 min read · Undra team</p>
  </header>
  <nav class="toc" aria-label="On this page"><ol><li><a href="#…">…</a></li></ol></nav>
  <section id="…"><h2>…</h2>…</section>
  <aside class="callout tip|note|warn"><p>…</p></aside>
  <table class="compare"><thead>…</thead><tbody>…</tbody></table>
  <pre><code class="language-swift">…</code></pre>
</article></main>
<!-- <footer> … </footer> copied verbatim -->
<script src="../../assets/site.js" defer></script></body></html>
```

## 4. Distribution pipeline

* `.github/workflows/release.yml`: on tag `v*` and on `workflow_dispatch` with input
  `publish` (default `false` = dry run: build everything, publish nothing). Matrix builds
  the `undra` binary for `aarch64-apple-darwin`, `x86_64-apple-darwin`,
  `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` (release, `strip`), packs
  `undra-v<ver>-<target>.tar.gz`, produces `checksums.txt` (sha256), verifies that the tag
  equals the workspace version and the npm versions, creates the GitHub Release with
  generated notes, publishes the npm packages (`npm publish --provenance --access public`,
  needs `NPM_TOKEN` and `id-token: write`), and updates the formula in the tap repository
  (`HOMEBREW_TAP_TOKEN`, fine-grained, contents:write on `shreypdev/homebrew-undra`).
  Windows is out of scope (roadmap).
* `packaging/npm/`: the meta package and the four platform packages, generated from one
  template by `packaging/npm/build.mjs` (no deps). The meta `bin/undra.js` resolves the
  platform package and `execFileSync`s the binary; a clear error names the unsupported
  platform and points at the curl installer.
* `packaging/homebrew/undra.rb`: the formula with per-platform `url`/`sha256`, `test do`
  runs `undra --version`.
* `site/install.sh`: POSIX sh; `set -eu`; detects `uname -sm`; uses the GitHub API for
  the latest tag (or `UNDRA_VERSION`); downloads with `curl -fsSL`; verifies sha256 with
  `shasum -a 256` or `sha256sum`; installs to `$UNDRA_HOME/bin` (default `~/.undra/bin`);
  prints the PATH line; no `sudo`; no `eval` of remote content.
* `undra --version` prints `undra <semver> (<short sha>)`; the sha comes from the
  `UNDRA_BUILD_SHA` env at build time, `unknown` otherwise.
* `undra init` writes `undra = { git = "https://github.com/shreypdev/undra", tag =
  "v<cli version>" }` by default; a `--undra-path <dir>` flag and running from inside the
  checkout keep today's `path` behaviour.
* `scripts/bump-version.sh <semver>` edits the workspace version, the TS runtime and the
  npm packaging in one go; `docs/RELEASING.md` is the runbook (bump → PR → tag → watch →
  smoke each channel). Founder prerequisites are listed there: create the npm org `undra`
  and an automation token (`NPM_TOKEN`), create `shreypdev/homebrew-undra` and a
  fine-grained PAT (`HOMEBREW_TAP_TOKEN`), then `git tag v1.0.0 && git push origin v1.0.0`.

## 5. Harsh-conditions benchmark ("does it survive very high-frequency data?")

Design first (an investigation by the strongest model), then implementation, then the
numbers on the site. Questions the design must answer from the code, not from memory:

1. What happens today at 100 k transactions/s: is every transaction a change-set delivered
   to the platform, or is delivery coalesced anywhere (ADR-018/019/020/023, the three
   platform runtimes' apply path)? If the platform thread would be flooded, the fix is a
   runtime-model change and needs **ADR-031 (frame-coalesced delivery)** before code.
2. Scenarios (each with a metric and a budget row in `bench/budgets.toml`): firehose
   transactions (single signal, N txn/s); keyed-list churn (10 k rows, 100 k random
   ops/s); fan-out (100 k observed signals, 1 % dirty per txn); stream backpressure
   (1 M items/s producer, slow consumer, bounded memory); concurrent completions (8 port
   threads completing while the main thread applies); a 60 s soak (10 s in CI) with RSS
   growth ≤ 1 %.
3. Metrics: sustained throughput, p50/p99/p999 apply latency, change-set bytes per txn,
   allocations per op where the existing harness counts them, memory steady state.
4. Platform side: a `stress` screen in the playground (web first; iOS/Android follow in
   the device phase) showing generated updates/s, applied change-sets/s, p50/p99 apply
   time, dropped frames - the same numbers the landing page's live section shows.
5. Output: `bench/benches/stress.rs`, `bench/src/bin/soak.rs`, budget rows,
   `bench/RESULTS.md` "Harsh conditions" section, `site/data/bench.json` rows.

## 6. The rename (Keel → Undra)

Product-wide and mechanical: an idempotent script `scripts/rename-keel-to-undra.sh`
(path moves deepest-first, then ordered content rules: `@keel/` → `@undra/`,
`dev.keel` → `dev.undra`, `dev/keel/` → `dev/undra/`, `Java_dev_keel_` →
`Java_dev_undra_`, `KeelRuntime` → `UndraRuntime`, then `Keel` → `Undra`, `KEEL` →
`UNDRA`, `keel` → `undra`), run once on the rename branch and re-run on any other branch
after it merges `main` (take your own side for conflicts in files you own, run the script,
re-test). Excluded: `site/` (the site piece renames its own files as it rewrites them),
`.10x/reviews/` and ADR-018…029 (immutable history; ADR-030 states the 1:1 mapping),
binaries. Golden files are regenerated through the golden mechanism and must equal the
script's output. Every suite runs green before the branch is reviewed.

## 7. Ownership, sequencing, review

| Piece | Worktree | Implementer | Reviewer | Owns |
|---|---|---|---|---|
| schema-strip (in flight) | `wt/schema-strip` | opus | integrator | cli schema loader, ci.yml contracts job |
| rename | `wt/rename` | sonnet | opus | everything in §6 except `site/` |
| dist | on top of the rename | sonnet | fable (security) | `release.yml`, `packaging/`, `site/install.sh`, `scripts/bump-version.sh`, `docs/RELEASING.md`, README install section, `undra --version`, `undra init` pinning |
| site-v2 | `wt/site-v2` | sonnet | fable (design, SEO, a11y, honesty) | `site/**` except `site/blog/<slug>/` and `site/install.sh`; `site.yml`; playground web URL params + stats |
| blog | `wt/blog` | sonnet | fable (fact-check) | `site/blog/<slug>/index.html` ×4 only |
| stress | `wt/stress` | opus (design) → sonnet (impl) | opus | `bench/**`, playground stress screen, ADR-031 if needed |

Merge order: schema-strip → rename (then `gh repo rename undra`) → dist → site-v2 → blog
→ stress. Each merge is followed by the full local matrix (`scripts/env.sh`; the commands
in `.10x/status.md`) and CI green before the next. Every piece gets an adversarial review
before merge; findings High/Medium are fixed and re-verified, Low are recorded.

## 8. Out of scope for this batch

crates.io publishing (v1.1), Windows CLI, Maven Central, a custom domain, per-post OG
images, Google Search Console verification (founder action, documented), moving the
repository under a GitHub organisation.

## Amendment A - founder feedback on the first redesign (2026-09-30, evening)

Overrides §2.2 (palette) and §2.4 (sections). The founder reviewed the v2 landing and
said: the colour and theme of the v1 site (commit `e45e30f`) must stay - "the blue change
is not looking good at all"; the new home page "has too much text and is very hard to
digest, the old one was better"; keep the live playground; the "Describe once. Generate
native. Write once per change." code should be minimised or shown on demand; the v1
numbers section ("The boundary is cheap. Measured, not promised.") is preferred over the
new table.

* **Palette = v1 tokens, verbatim** (`git show e45e30f:site/assets/base.css`): dark canvas
  `#0a0a0a` / surfaces `#111111`–`#1d1d1d` / text `#ededea`, accent **`#ff6a2a`**
  (`--accent-hi #ff9a6b`, soft/line/glow at 0.11/0.42/0.17 alpha); light canvas `#f7f6f2`,
  accent `#c2410c`. No cyan, no blue. Typography (Geist, no italics) and component
  refinements stay.
* **Landing = v1's arc plus three additions**: hero (v1 headline + diagram + the install
  block) → one-line thesis → live playground with counters → numbers in v1's presentation,
  driven by `site/data/bench.json` → "Describe once / Generate native / Write once per
  change" as a compact strip with the generated code collapsed by default behind an
  accessible disclosure → features as v1's compact grid → trust (v1) → compared (four
  one-line cards) → roadmap teaser (three lines) → final CTA. Nothing else.
* **Density rules**: one idea per section (eyebrow, headline, ≤ 2 sentences, one artifact);
  no paragraphs on the landing page; visible prose outside code, numbers and footer
  ≤ 350 words, enforced by a script in `site/scripts/`.
* **Acceptance**: side-by-side screenshots against `e45e30f` (dark and light, 1440×900)
  must read as "v1, cleaner, with a live demo".
