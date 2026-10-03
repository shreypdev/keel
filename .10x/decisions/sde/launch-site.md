# SDE — launch-site: the site for launch (wt/launch-site, 2026-10-03)

The founder's eight asks for launch day. Design reasoning: `.10x/specs/2026-10-03-launch-site-design.md`. No product code
changed (crates, runtimes); the playground's head, theme and its `main.tsx` theme line are the only example code touched.

## What changed, per ask

1. **No "new" row.** The hero's eyebrow and the "New" strip are gone, with their CSS (`.hero .eyebrow`, `.whatsnew`,
   `.install-note`). The hero says what Undra is in one line and how to start.
2. **Install.** Homebrew, the installer, cargo, in that order, on the landing page (both blocks), getting started, the README
   and `docs/SITE.md`; the npm CLI and the "until it is tagged" note are gone everywhere outside the release machinery
   (`docs/RELEASING.md`, `docs/ONBOARDING.md`'s packaging row, `packaging/npm`, `release.yml`: untouched). Getting started
   opens with the shortest path, as run (below), and states two things the old text did not: an API change stops an app
   built from old bindings, and on the web the Vite plugin reloads the page on a core change (`UNDRA_SKIP_BUILD=1` keeps the
   page on the `undra dev` core and its state). Node 20+ (what `undra doctor` checks), not 22+.
3. **Roadmap.** One chip scale in `base.css` used by both pages; Shipped in nine groups of one-line items; no Now; Next,
   Later and Exploring from the record only, each item with its `source` in `roadmap.json`. `build-roadmap.mjs` renders
   groups, the jump links and the landing teaser (`"teaser": true`).
4. **Blog.** `undra-1-0` (851 words), `a-week-of-outside-use` (897), `the-javascript-runtime-at-16-kb` (897), each with a
   `claims.md` (34, 38 and 32 rows). Feed, sitemap and index regenerated; they use the shared `og.png` as the other posts do.
5. **Landing.** Eight sections (hero, live, numbers, how, receipts, why Undra, still open, start), 275 words. The diagram's
   screens are buttons; the core call and the change-set cost are measured slots in the diagram (`row-<id>` slots, new in
   `build-numbers.mjs`). Harsh-conditions cards behind a disclosure. Fixed on the way: `hero.js`'s first frame could step time
   backwards (rAF stamped before `performance.now()`), giving a write origin of -1 for a frame; now clamped.
6. **Playground theme.** `examples/playground/web/src/index.css` wears the site's tokens and fonts, light and dark; the site's
   stored theme carries over.
7. **Icons.** The playground's (and the stories page's) favicon is Undra's mark. `grep -ri keel` over `site/`,
   `examples/*/web`, `runtimes/ts/devtools` finds nothing.
8. **Docs pass.** Fixed: the npm CLI and the release note (getting started); Node 22+ → 20+; the live loop's web behaviour;
   iOS/Android needing a checkout until the runtimes are published; "the seventeen scenarios" and "the four adversarial
   reviews" (architecture, now a trust slot and every feature's reviews); the upload recipe saying the queue never reaches
   BGTaskScheduler/WorkManager (background runs replay it, ADR-046); the docs index and getting started now link Bazel,
   your network stack, reviewing generated code, workers and crash recovery, generics, and `opt_level` on the production
   page. No other page restyled or reorganised.

## Verified by running

* **Getting started, from an empty directory** (`/private/tmp/…/scratchpad/gs`), with `cargo build -p undra-cli` and
  `target/debug` on `PATH`: `undra --version` → `undra 0.1.0 (unknown)`. `undra init myapp` → `Created …/myapp (79 files,
  bindings for schema hash 0xce5731fc471dc5b8)` and the next steps; its `core/Cargo.toml` pins `undra` at git tag `v0.1.0`,
  which does not exist (`git ls-remote --tags origin` is empty), so the build ran with `undra init myapp --undra-path
  <worktree>` (78 files). `undra doctor` → 31 ok, 1 warning (ANDROID_NDK_HOME), exit 0. `undra dev` → built the host core,
  banner `ws://127.0.0.1:7443`, devtools URL, per-platform lines. `cd web && npm install --offline && npm run dev` →
  `==> Building the core for web (release)`, `web wasm 263.8 KB (gzip 112.8 KB)`, `Local: http://localhost:5173/`; the page
  (Chromium) showed "0 left", an added item showed "1 left". With `?undra=ws://127.0.0.1:7443` the page used the served core
  ("Dev server" bar). A body-only edit of `core/src/lib.rs`: `Restarted … state kept (1 store, 1 KiB, restored in 140 µs)`,
  the client reconnected, then the Vite plugin rebuilt the wasm and full-reloaded the page, which started fresh ("0 left").
  With `UNDRA_SKIP_BUILD=1 npm run dev` the state was kept across the edit (the item added under the old code stayed, a new
  one used the new code; console `Reloaded, state kept`). Editing an error's message changed the schema hash; the server said
  so and refused the web client built from the old bindings, as documented. Every server I started was stopped.
* `node site/scripts/build-all.mjs` (nothing left stale), `check-links.mjs` on `site/` (54 pages) and on the staged site (55),
  `--words` 275/350, `node --test site/scripts/decls.test.mjs` (10/10), `undra bindgen --check --docs` of the playground,
  cookbook and fieldbook (up to date), the playground's `npm run build` (tsc + vite) and `vitest run` (136/136).
* The diagram's tap, driven in Chromium (Playwright from the playground's `node_modules`, no download): role `button`, the
  call label and the call cost lit as the write crosses, the change-set cost lit while it is built, every badge "2 left" after
  the apply; with reduced motion a tap shows the applied frame. Screenshots before/after at 375 and 1280, light and dark, no
  horizontal scroll on any page; the landing page without JavaScript reads top to bottom.

## Product findings (not fixed here: product code)

* **Release-mode projects cannot build on iOS or Android yet.** `undra bindgen` (`crates/undra-cli/src/bindgen.rs:211-216`)
  points a released project's Swift package at `https://github.com/shreypdev/undra-swift`, which does not exist, and
  `undra init` (`commands/init.rs:278-281`) depends on `dev.undra:runtime` from Maven Central, which is not published.
  Neither is in `docs/RELEASING.md` or `release.yml`. The docs say so and send iOS/Android to `--undra-path`.
* **The web template loses state on an `undra dev` reload.** The `undra()` Vite plugin (`runtimes/ts/@undra/runtime/src/vite.ts`,
  `configureServer`) watches `core/src` and full-reloads the page even when the page runs on the remote core, so "the app comes
  back with its state" holds for iOS and Android but not for the web shell under `vite dev`. A fix: do not reload a page that
  is attached to `undra dev` (or let the template set `skip` when `VITE_UNDRA_DEV_URL` is set). Documented as is.
* `undra upgrade`'s C0014 help (`commands/upgrade.rs:242`, its golden, and `docs/errors.html`) still says `npm update -g @undra/cli`.
* The devtools page's favicon (`runtimes/ts/devtools/static/index.html`) is a plain orange square, not Undra's mark.
* The published `why-undra-is-the-default-choice` post (2 Oct) says nothing is on npm or Homebrew yet; true on its date, left as is.

## Left

* The npm CLI wrapper is still built and published by the release pipeline (`packaging/npm`, `release.yml`, `RELEASING.md`):
  the founder removes it separately if wanted.
* `og.png` was not re-rendered (it needs network for the fonts); its line says "SwiftUI, Compose and React".
* Not run: the cookbook's snippet compile (`examples/cookbook/snippets/check.sh`); the cookbook pages' code is generated from
  the sources and `build-cookbook` reports them up to date.
