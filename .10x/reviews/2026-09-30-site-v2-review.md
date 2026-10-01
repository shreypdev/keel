# Site v2 — adversarial review

**Date:** 2026-09-30 · **Reviewer:** fable (design, SEO, a11y, honesty) · **Piece:** `wt/site-v2` at `e9b7a4c` (review fixes committed on top) · **Spec:** `.10x/specs/2026-09-30-launch-v2-design.md`, Section 2 and Amendment A

The site was built with `bash site/scripts/build-local.sh` (real wasm playground) and served from `_site/`;
the v1 baseline is `git archive e45e30f site`, served beside it. Screenshots were taken with headless
Chrome at 1440×900 (top) and 1440×7000 (full), dark and light, and are listed at the end.

## Verdict on design fidelity

It reads as "v1, cleaner, with a live demo". The token block of `site/assets/base.css` is v1's verbatim
(diffed against `e45e30f:site/assets/base.css`: the only differences are the removed `--font-display`
serif token and one added `--ok` alias); dark and light screenshots match v1's canvases, surfaces and the
single orange accent, with Geist replacing the serif italics and nothing italic anywhere. The arc is v1's
(hero with the diagram, thesis, numbers, how it works, features, receipts, principles, final CTA) plus the
three additions the amendment asked for (install block, live playground with counters, compared and
roadmap teasers). The generated code is closed by default behind a native `<details>` (keyboard-operable,
counted out of the prose budget); the numbers section is v1's card-and-meter style driven by
`site/data/bench.json`; every section is eyebrow + headline + at most two sentences + one artifact, and
the visible prose is 332 words against the 350 budget. The one structural liberty is the "Twelve rules"
section, which the amendment's list does not name but v1 had (L4 below, a founder call).

## Findings

| # | Sev | Area | Where | Finding | Status |
|---|---|---|---|---|---|
| H1 | High | Honesty | `site/index.html` (both install blocks), `site/docs/getting-started.html` §2 | The brew, npm and curl one-liners were shown with nothing saying they need a tagged release; today none of them works. | **Fixed**: an `.install-note` line under both install blocks ("brew, npm and curl ship with the v1.0.0 release; until it is tagged, use cargo") and one sentence on the getting-started channel table; `docs/SITE.md` says when to remove it. Word budget unchanged (332). |
| M1 | Medium | Honesty | `site/assets/home.js` (stats listener) | The counters trusted the playground's measured clock step. When the step comes back 0 (frozen or unmeasurable clock; reproduced with the real embed under headless virtual time: `timerResolutionUs: 0`, `applyP50Us: 0`) the page printed "< 1 µs", a bound no non-isolated browser can support. | **Fixed**: the step is floored at 100 µs (5 µs when `crossOriginIsolated`), which is the coarsest-known clamp of Chrome, Firefox and Safari; the counters now read "< 100 µs" for sub-step applies. `docs/SITE.md` Known limits updated. |
| M2 | Medium | Honesty | `site/data/roadmap.json` → `site/roadmap/index.html` (card + ItemList JSON-LD), `llms-full.txt`, search index | "Frame-coalesced delivery: Planned in ADR-031, not shipped" cited a decision record that does not exist (`.10x/adrs/` ends at ADR-028). | **Fixed**: reworded to "Not shipped, and a runtime-model change that needs an ADR first…"; generated files rebuilt. |
| M3 | Medium | Build/CI (rename) | `site/scripts/build-local.sh` (`cargo run -p keel-cli`, `--base=/keel/playground/`, `ln -s . _site/keel`), `site/scripts/lib.mjs:9` (`ORIGIN`), `site/scripts/check-links.mjs` (`/keel/` root-absolute rule), `site/404.html` (`/keel/…` links), every `github.com/shreypdev/keel` link under `site/`, `keel-theme` storage key, `keel:theme` event, `KeelSearch`, `docs/SITE.md` | Spec §6 excludes `site/` from the rename script on the assumption the site piece would rename its own files; it did not (only the install commands, blog slugs and code samples say Undra). `.github/workflows/site.yml` sits outside `site/` and will be renamed, so after the rename CI would build with `undra-cli` and base `/undra/playground/` while `site/` still points at `/keel/`, the old repository path and the old crate. | **Open** — integrator: run the rename over `site/` as well (the ordered rules apply cleanly; there is no `keel` in the IndexNow key), then `node site/scripts/build-all.mjs` (regenerates sitemap, feed, llms, search index from the new `ORIGIN`) and `bash site/scripts/build-local.sh`. `og.png` already carries the new name and mark. |
| L1 | Low | No-JS | `site/index.html` live section, `site/assets/home.css` | With JavaScript off the demo frame said "The demo loads as you scroll." forever. | **Fixed**: `<noscript>The live demo needs JavaScript.</noscript>`; the scroll hint is hidden when `html` lacks `.js`. |
| L2 | Low | postMessage | `examples/playground/web/src/embed-stats.ts`, `main.tsx`, `embed-stats.test.ts` | The playground posted stats with `"*"` (only in embed mode, only to a real parent). The landing's listener already checks `event.source === frame.contentWindow`, `event.origin === location.origin` and the numeric shape, so nothing was exploitable; the target origin was the one loose end. | **Fixed**: `startStatsPoster` takes a `targetOrigin` (default `"*"` for callers that embed cross-origin), `main.tsx` passes `location.origin`; test added (64/64 pass). |
| L3 | Low | Mark | `site/index.html` header/footer, `site/favicon.svg`, `site/og/index.html`, `site/assets/og.png` | Implementation item 8: the mark was still a "K" and the OG card said the old name. | **Fixed**: a "U" in the same stroke style (`M10.5 7v11a5.5 5.5 0 0 0 11 0V7`), synced to all 14 pages by `sync-chrome`; OG card re-rendered with the U mark, "Undra" and `shreypdev.github.io/undra` (121 KB, 1200×630). |
| L4 | Low | Design | `site/index.html` "Twelve rules, enforced." | Amendment A lists ten sections and says "Nothing else"; this eleventh is v1's principles section reduced to twelve chips (no prose, links to the architecture digest). | Open, founder call. Removal is one section deletion and drops 30 words. |
| L5 | Low | Design | `examples/playground/web/src/index.css` (`--accent: #2f6fed` / `#6b9bff`) | The standalone playground at `/playground/`, linked from the nav, keeps its own blue accent; only `embed=1` wears the site palette. The blue predates this branch (on `main`), so it is not a v2 leftover; there is no cyan or blue token in `site/`. | Open; recommend a follow-up that gives the playground's default theme the site's orange so the nav link does not change palette. |
| L6 | Low | Tooling | `examples/playground/web` | `npx tsc --noEmit` fails on `runtimes/ts/@keel/runtime/src/react.ts` (`react` types not resolvable from the playground's tsconfig); identical on the branch head without the review changes. `site.yml` runs `npx vite build` (no type check), so CI is unaffected. | Open, pre-existing; noted for the runtime owners. |

Low fixes made directly: 3 (L1, L2, L3). High/Medium fixed: H1, M1, M2. Open: M3 (rename-time checklist), L4, L5, L6.

## What was checked and passed

* **Palette and restraint.** No cyan or blue token in `site/**` (the only teal is v1's own light-theme
  `--s-ty` syntax colour, verbatim from `e45e30f`); one accent; the embedded playground wears the site palette
  in `embed=1`, dark and light.
* **Numbers match the sources.** Every `bench.json` row equals `bench/RESULTS.md` and `README.md`
  (49.8 ns labelled "C ABI, core side"; 228 ns; 6.3 µs; 2.3 µs; 71 µs; 85 KB; 831 KB; the budgets; the
  machine line). Trust cards: 3,800+ (2,109 / 897 / 454 / 328), 17 × 3 = 51/51, four reviews, ASan + Miri —
  all as in the README. No "per-frame coalescing" claim anywhere in `site/`; the roadmap says it is not
  shipped and every change-set is delivered on its own.
* **SEO.** 15 pages with unique `<title>` ≤ 60 and description ≤ 155, canonical, `og:*`, `twitter:*`; one
  JSON-LD block each, all parse: `WebSite` + `Organization` + `SoftwareSourceCode` + `SoftwareApplication`
  (landing), `TechArticle` + `BreadcrumbList` (docs), `ItemList` (roadmap), `Blog` (blog index). `404.html`
  is `noindex` and exempt. `sitemap.xml` (14 URLs, `lastmod` from each page's `dateModified`) and `feed.xml`
  (RSS 2.0 with `atom:link`, zero items until the blog piece lands) pass `xmllint`. `robots.txt` allows all
  and names the sitemap. `llms.txt` / `llms-full.txt` (119 KB) are sensible. IndexNow: key file at the site
  root with the key as content, JSON body `{host, key, keyLocation, urlList}` with
  `content-type: application/json; charset=utf-8`, `continue-on-error: true` after the deploy step.
* **Accessibility.** Contrast computed for every token pair used for text in both themes: all ≥ 4.5:1
  (lowest 4.68 light `--text-3` on `--surface-3`, 4.78 light string colour on code). Visible focus ring,
  skip link, `header`/`nav`/`main`/`footer` landmarks, tabs with `role=tablist/tab/tabpanel`, roving
  `tabindex`, Arrow/Home/End; `<details>` disclosures are native; theme toggle and menu are labelled;
  diagrams have `role=img` with title and description; `prefers-reduced-motion` pauses the hero (button says
  "Play animation"), skips the count-ups and reveals, and removes meter and grid transitions.
* **Performance and robustness.** Landing transfer 34 KB gzipped (HTML + 2 CSS + 3 JS + favicon) before the
  lazy demo; the landing's CSS is 39.9 KB raw (base 16.0 + home 23.9 — at the 40 KB line, mind it); no
  third-party JavaScript; fonts preconnected with `display=swap`; the iframe is `loading="lazy"` and mounted
  by an IntersectionObserver 200 px before view; no console errors in headless Chrome on the landing or the
  embedded playground; with scripts stripped every section, all four install channels, the final numbers
  and the footer render.
* **Build and CI.** `site.yml` is consistent with itself (`keel-cli`, `/keel/playground/`), runs the
  generated-file freshness check, the source link check, staging and the staged link check before upload.
  `sync-chrome.mjs` run twice: no diff. `build-local.sh` works from the staged state (`stage.sh` recreates
  `_site/`, `npm ci` uses the committed lockfile, `scripts/env.sh` is optional); the scripts have no
  dependencies. `check-links.mjs --words`: 332/350.

## Screenshots

Durable copies in `/Users/shrey/Desktop/src/.work/site-v2-review-shots/` (not committed):
`v1-dark-top.png`, `v1-light-top.png`, `v1-dark-full.png`, `v1-light-full.png` (baseline `e45e30f`);
`new-dark-top.png`, `new-light-top.png`, `new-dark-full.png`, `new-light-full.png` (branch head before
fixes); `after-dark-top.png`, `after-light-top.png`, `after-dark-full.png` (after fixes: U mark, install
note, "< 100 µs" counters); `new-reduced-top.png` (reduced motion), `new-mobile-top.png` (390 px),
`new-nojs-full.png` (scripts stripped), `pg-embed.png` (the embedded playground on its own), `docs-dark.png`,
`roadmap-light.png`, `blog-dark.png`, `404-dark.png`, `og-after.png`.
