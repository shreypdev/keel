# The Undra website

Static HTML, CSS and JS in `site/`, published to <https://shreypdev.github.io/undra/> by
`.github/workflows/site.yml`. No framework, no runtime dependencies, no third-party JavaScript. The scripts
that keep it consistent are plain Node (20+) with no packages.

The site's rules: the v1 palette (one orange accent, warm neutrals; the tokens in `base.css` are v1's, verbatim),
Geist and Geist Mono, no italic display type, AA contrast, at most 40 KB of CSS on any page, no third-party
JavaScript, every page with its own title, description, canonical, Open Graph card and JSON-LD, and **a landing
page for scanning: at most 350 words of visible prose** (enforced, see below). Code style follows `CLAUDE.md`. This
page is the operating manual.

## What is where

| Path | What it is |
|---|---|
| `site/index.html` | The landing page, and the **source of truth for the shared header, footer and head boilerplate**. |
| `site/docs/*.html` | The documentation pages (hand-written HTML), except `errors.html`, which is generated. Their sidebar and pager come from `site/data/docs.json`. |
| `site/roadmap/index.html` | The roadmap page; its sections are generated from `site/data/roadmap.json`. |
| `site/blog/index.html` | The blog index; its cards are generated from the posts. |
| `site/blog/<slug>/index.html` | One post each (the skeleton is styled by `blog.css`: `post-header`, `eyebrow`, `lede`, `post-meta`, `toc`, `callout tip|note|warn`, `table.compare`, `pre > code.language-x`). Written by hand. |
| `site/assets/` | `base.css` (tokens, type, header, code, tables, footer), `home.css`, `docs.css`, `blog.css`, `roadmap.css`; `site.js` (theme, menu, tabs, copy buttons, highlighter, reveals), `hero.js` (the animated diagram), `home.js` (the live demo), `search.js` (Cmd/Ctrl-K, loaded on first use); `og.png`. |
| `site/data/` | `bench.json` (landing numbers), `roadmap.json` (roadmap), `docs.json` (docs navigation), `pending.json` (pages linked before they exist). |
| `site/og/index.html` | The 1200x630 social card, rendered to `assets/og.png`. |
| `site/scripts/` | The tooling below. Not deployed. |
| `site/sitemap.xml`, `feed.xml`, `search-index.json`, `llms.txt`, `llms-full.txt` | **Generated.** Never edit by hand. |
| `site/<32 hex>.txt` | The IndexNow key file. |
| `site/install.sh` | The curl installer (owned by the distribution piece). |
| `_site/` | The staged site (git-ignored): `site/` plus the playground build. |

The theme is dark by default with a light toggle; `?theme=light` or `?theme=dark` in any URL forces one
(used by the live demo and handy for screenshots). The accent is v1's single orange (`#ff6a2a` dark, `#c2410c` light); type is Geist
and Geist Mono (Google Fonts, `display=swap`), and there is no italic display type anywhere. The embedded playground
wears the same palette in `embed=1` mode.

## Preview locally

Quick, without the live demo:

```sh
python3 -m http.server 8765 --directory site
```

Everything, including the real playground (the Rust core as wasm under the React app):

```sh
bash site/scripts/build-local.sh
python3 -m http.server 8765 --directory _site
# open http://localhost:8765/undra/
```

`build-local.sh` sources `scripts/env.sh`, regenerates the site's files, runs
`undra build --platform web -C examples/playground`, builds the web app with `vite build --base=/undra/playground/`,
stages `_site/` (and links `_site/undra -> .` so the playground's `/undra/...` asset URLs resolve), and runs the link
checker on the result. Without `binaryen` the build warns and keeps an unoptimised wasm, which is fine for a
preview.

## The scripts

All live in `site/scripts/` and run from the repository root.

| Script | What it does |
|---|---|
| `build-all.mjs` | Runs `build-numbers`, `build-roadmap`, `build-errors`, `sync-docs-nav`, `build-blog-index`, `build-search-index`, `build-llms`, in that order. CI runs it and fails if it changes anything under `site/`. |
| `build-numbers.mjs` | Renders the landing page's benchmark cards (big number, one-line label, budget in mono) from `data/bench.json` into `index.html` (between `<!-- numbers:start -->` and `<!-- numbers:end -->`), and shows or hides the demo's "Push it" button. **Not fetched at runtime.** |
| `build-errors.mjs` | Writes `docs/errors.html`, one anchored section per macro diagnostic (`#E0001` ...). The catalogue comes from `docs/SPEC.md` section 12, the short meanings from the doc table in `crates/*-macros/src/impl_/diag.rs`, and the real what/why/fix messages from the compile-fail goldens `crates/*-macros/tests/ui/*.stderr`. Codes no golden covers (they come from bindgen, rustc or the runtime) show the catalogue entry only. |
| `sync-docs-nav.mjs` | Writes the docs sidebar (column and mobile menu) and the previous/next pager into every docs page from `data/docs.json`. `--check` reports drift. |
| `build-roadmap.mjs` | Renders `roadmap/index.html`'s sections and its `ItemList` JSON-LD from `data/roadmap.json`. |
| `build-blog-index.mjs` | Reads every `blog/*/index.html` (title, description, `<time datetime>`, eyebrow, reading time from the word count) and writes the index cards, the `Blog` JSON-LD, `feed.xml` (RSS 2.0) and the whole `sitemap.xml`. With no posts it renders one "First posts are coming" card, which disappears when a post exists. It also keeps each post's "N min read" equal to the computed value. |
| `build-search-index.mjs` | Writes `search-index.json`: a page entry and one entry per heading (with its text) for the docs, the roadmap and every post. |
| `build-llms.mjs` | Writes `llms.txt` (an index) and `llms-full.txt` (the docs, roadmap and posts as Markdown, in one fetch). |
| `sync-chrome.mjs` | Copies the `<header>`, `<footer>` and the head boilerplate from `index.html` into every other page, rewriting relative URLs for the page's depth (posts are at depth 2) and marking the current section in the nav. `--check` writes nothing and exits 1 if a page is out of sync. |
| `check-links.mjs` | Fails on a broken internal link, asset or `#fragment`, on a missing or over-long title (60) or description (155), a canonical that is not the page's URL, a missing `og:*`/`twitter:*` tag, anything other than one `<h1>` and one valid JSON-LD block, duplicate titles or descriptions, and an out-of-sync chrome, and **a landing page with more than 350 words of visible prose** (`--words` prints the count, `--words --show` the words; header, footer, code, install blocks, the numbers grid, diagrams and what a closed `<details>` hides are not counted). `--root _site` checks the staged site. |
| `render-og.sh` | Renders `og/index.html` to `assets/og.png` with headless Chrome (needs network for the fonts). Commit the PNG. |
| `indexnow.mjs` | POSTs the sitemap's URLs to `api.indexnow.org` (`--dry-run` prints the body). |
| `stage.sh`, `build-local.sh` | Stage `_site/`; build everything for a local preview. |
| `lib.mjs`, `sitemap.mjs` | Shared helpers. |

## Editing the site

* **Shared header, footer, nav:** edit `site/index.html`, then `node site/scripts/sync-chrome.mjs`.
* **Numbers on the landing page:** edit `data/bench.json`, then `build-all`. A row is
  `{ id, operation, value, unit, budget, budgetUnit, gate, source }` with units `ns`, `µs`, `ms`, `KB`, `MB`;
  keep `operation` to one short line, it is the card's label. The `harsh` array takes rows of the same shape (the
  stress suite fills it); when it is non-empty the section grows a "Harsh conditions" group of cards in the same
  style. A new card adds no prose, so the word budget is not affected. `"stressScreen": true` shows the "Push it"
  button of the live demo, which loads the playground's stress screen
  (`playground/?screen=stress&embed=1&rate=10000&mode=firehose&autostart=1`) into the iframe and reveals the extra
  counters (generated per second, the share of received entries the mirror applied, dropped frames) from the
  optional fields of the `undra-stats` message (`examples/playground/web/src/embed-stats.ts`). Keep the button
  hidden (`false`) on a build whose playground has no `screen=stress`.
* **Roadmap:** edit `data/roadmap.json` (the source of truth for the wording is `.10x/handoff.md`; keep them in
  step), then `build-all`.
* **A docs page:** edit the HTML. Give it a unique `<title>` (at most 60 characters), a description (at most 155),
  the canonical URL, the `og:`/`twitter:` tags and one JSON-LD block (copy a neighbour), list it in
  `data/docs.json` (the sidebar, the pager, the search order and `llms-full.txt` all follow), then run
  `node site/scripts/build-all.mjs`.
* **The landing page's words:** it is for scanning. `node site/scripts/check-links.mjs --words --show` prints what
  counts against the 350-word budget; detail belongs in the docs, in a closed `<details>`, or in a tooltip.
* **`dateModified`:** every page states its last change in its JSON-LD (`dateModified`, or `datePublished` for a
  post) and the sitemap's `lastmod` is read from it. Bump it when you change the page; it is a hand-maintained
  date on purpose, so the generated files are deterministic in CI.
* **Package names** are written `@undra/runtime` and `@undra/cli` everywhere; the integrator replaces them in one
  global pass when the registry scope is final.
* **Pages that are linked before they exist** go in `data/pending.json`; the checker reports those links as notes
  instead of failures and tells you to remove the entry once the page exists.
* **The install block's release note** (`.install-note`, in both install blocks of `index.html`, and the matching
  sentence in `docs/getting-started.html`) says that brew, npm and curl need the first tagged release. Remove it
  when v1.0.0 is out and the three channels have been smoke-tested.

## The CI flow (`.github/workflows/site.yml`)

On a pull request that touches the site or the playground, and on `main`:

1. Install Rust with the wasm32 target, `binaryen` and Node.
2. `cargo run -p undra-cli -- build --platform web -C examples/playground` (the real core as wasm).
3. In `examples/playground/web`: `npm ci` and `npx vite build --base=/undra/playground/`.
4. `node site/scripts/build-all.mjs`, then fail if anything under `site/` changed (a committed generated file is stale).
5. `node site/scripts/check-links.mjs` on the source tree (this includes `sync-chrome --check`).
6. `bash site/scripts/stage.sh` to build `_site/`, then `check-links.mjs --root _site`.
7. On `main` only: upload `_site`, deploy to GitHub Pages, then submit the sitemap's URLs to IndexNow (a failure
   there never fails the deploy).

One-time repository setting: Settings, Pages, Source = "GitHub Actions".

## SEO, in one place

* The brand phrase is "Undra, the Rust core for native apps"; titles say "Undra framework" or "Undra (Rust)" where
  they can, so the name is always paired with what the product is (the name's own history is ADR-030).
* Every page has a canonical, Open Graph and Twitter tags, and one JSON-LD block: `WebSite`, `Organization`,
  `SoftwareSourceCode` and `SoftwareApplication` on the landing page; `TechArticle` and `BreadcrumbList` on docs
  pages; `ItemList` on the roadmap; `Blog` on the blog index; `BlogPosting` on posts.
* `robots.txt` allows everything and names the sitemap. `llms.txt` and `llms-full.txt` are for agents.
* The social card is `assets/og.png`, rendered from `og/index.html`; re-render it with `bash site/scripts/render-og.sh`
  when the headline changes.

## Founder actions (manual, one time)

These cannot be automated from the repository.

### Google Search Console

1. Open <https://search.google.com/search-console> and choose **Add property**, then **URL prefix**, and enter
   `https://shreypdev.github.io/undra/`.
2. Choose the **HTML tag** verification method and copy the `<meta name="google-site-verification" content="...">`
   tag it shows.
3. Paste that tag into the `<head>` of `site/index.html` (next to the canonical link, **outside** the
   `chrome:head` markers), commit, and let the Site workflow deploy it.
4. Back in Search Console, click **Verify**.
5. Under **Sitemaps**, submit `sitemap.xml` (the full URL is `https://shreypdev.github.io/undra/sitemap.xml`).
6. Optionally use **URL inspection** on the landing page and the blog index and click **Request indexing**.

### Bing

In Bing Webmaster Tools choose **Import from Google Search Console** and approve; the verified property and the
sitemap come across. IndexNow (already wired into the workflow) covers Bing, Yandex and DuckDuckGo after every deploy; the
key file is `site/<key>.txt` and it must stay in the repository, unchanged, for the pings to be accepted.

### Not automated, by design

Lighthouse scores are not gated in CI (the target is 95 or more for performance,
accessibility, best practices and SEO); run Lighthouse against the deployed URL after a release.

## Known limits

* The live demo's apply time (the duration of one mirror drain, which merges every change-set that arrived since the previous one) comes from `performance.now()`, which browsers round (about 0.1 ms in Chrome when
  the page is not cross-origin isolated, about 1 ms in Firefox and Safari). GitHub Pages cannot send the headers
  that lift this, so the landing page shows "under" the clock step for the smallest values and says so. The
  playground measures the step it sees and reports it with the stats; `home.js` never trusts a step below 100 µs
  (5 µs when the page is cross-origin isolated), because a frozen or unmeasurable clock reports 0 and the counter
  must not turn that into a sub-microsecond claim.
* Device-measured benchmark rows are not claimed anywhere on the site; they are a roadmap item.
