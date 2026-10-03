# The site for launch: design (2026-10-03)

**Piece:** `launch-site` (`wt/launch-site`). **Brief:** the founder's eight asks for launch day (the landing page, install,
the roadmap, three posts, the playground's theme and icon, a docs pass). **Record:** `.10x/decisions/sde/launch-site.md`.
**Binding taste:** the v1 orange palette of `site/assets/base.css`, restraint, collapsed code, no cyan or blue, no
text-heavy pages, a 350-word landing page, plain words, every number from a record.

## What the landing page is for

A visitor must know in ten seconds what Undra is for and why it is the best at it. Undra's claim is an architecture
(reads never cross the boundary, a write crosses once, one change-set patches every mirror) with measured costs, so the
page leads with the architecture and the measurements and lets everything else be one click away. The page demonstrates
instead of claiming: the diagram answers a tap, the playground runs the real core, the numbers link to their records.

## The sections that remain, and what each is for

| # | Section | Its job |
|---|---|---|
| 1 | **Hero** | What Undra is in one line ("Your app's logic, state and data in one Rust core, under SwiftUI, Compose, React and React Native"), how to start (Get started, and the install block: Homebrew, the installer, cargo), and the idea itself as the diagram. |
| 2 | **Live** | Proof that the core is real: the playground's 10,000-row list in an iframe, its apply times measured in the visitor's browser, "Push it". |
| 3 | **Numbers** | The boundary's cost, measured, each card linked to its record; the harsh-conditions cards behind a disclosure, so the page leads with the eight that answer "is the boundary cheap". |
| 4 | **How it works** | Describe, generate, write once; the code of four platforms collapsed behind one button. |
| 5 | **Receipts** | Why the claims can be checked: the test counts, the contract grid, the adversarial reviews, the reference app's proof. |
| 6 | **Why Undra** | Who it is for (two or more platforms that must agree) and how it compares, as three links to the comparison posts and one to the limitations matrix; the arguments live in the posts. |
| 7 | **What is still open** | Three items from the roadmap, in the roadmap's own chips, generated from `roadmap.json` so the two pages cannot disagree. |
| 8 | **Start** | The install block again, the two commands after it, and the docs and playground. |

## What was cut, and why

* **The eyebrow "v1 shipped · v1.x on main" and the "New" row** (React Native, devtools, derived lists, …): release-notes
  thinking. To a first-time visitor everything is new, and the roadmap lists what is in 1.0 by group.
* **The thesis line** ("Write the domain once. Keep every screen exactly as native…"): it repeated the hero's one line.
* **The features grid** ("Everything under the pixels", eight tiles): a list of capabilities that the roadmap's Shipped
  groups and the docs index carry better; on the landing page it competed with the diagram for the same ten seconds.
* **The twelve rules**: the constitution is the architecture page's (`docs/architecture.html#constitution`); on the landing
  page it was twelve links to one anchor.
* **"Reads never cross the boundary" as a comparison card**: it is not a comparison; the diagram is that post's summary
  and the launch post links it.
* **The npm install tab and the release note** (founder's ask 2): see Install below.

The page went from eleven sections to eight and from 347 to 275 words of counted prose.

## The diagram that demonstrates

The animated diagram was already the idea. Two additions, no new prose:

* **Each screen is a button.** A tap (or Enter) sends the next write from that screen: the visitor chooses the origin and
  watches one write cross, one commit, one change-set and four mirrors update. Under reduced motion a tap shows the moment
  every screen has the change, without motion. The buttons exist only once `hero.js` runs; without JavaScript the SVG stays
  a labelled image (`role="img"`), and the hint ("tap a screen") is hidden.
* **The measured cost is in the core.** One line in the core's box: the core call and a 100-signal change-set, each lit as
  the write crosses and as the change-set is built. Both numbers are `<!--measured:row-…-->` slots that
  `build-numbers.mjs` fills from `site/data/bench.json` (new: `row-<id>` slots for any card), so the diagram cannot drift
  from the cards. They link to the numbers section.

## Install

A native CLI's standard channels, easiest first: Homebrew (the easiest on a Mac, where iOS work happens), the shell
installer, cargo. The npm wrapper of the CLI is not advertised anywhere; the release pipeline still builds it (`packaging/npm`,
`release.yml`), which is the founder's to remove. The runtime libraries an app depends on still come from each platform's
registry (Swift Package Manager, Maven, npm for `@undra/runtime` and `@undra/react-native`).

## The roadmap: one scale, and the record

* **One chip scale**, defined once in `base.css` and used by both pages: Shipped solid accent, Next accent outline, Later
  neutral outline, Exploring dashed neutral, Not planned a faint neutral. No green checks; a shipped item's marker is an
  accent dot. Cards follow their chip's edge (Next accent, Exploring dashed).
* **Shipped is scannable**: nine titled groups (core and bindings, platforms, data, ports, dev loop and tooling, testing,
  size and speed, production, docs and samples) of one-line items, instead of forty cards of paragraphs.
* **No "Now"**: nothing is in flight, so the column is gone rather than empty.
* **Next, Later, Exploring come only from the record**: status checkpoint 34's open list, the v1.x and launch-v2 specs,
  the reviews' follow-ups. Each item carries its `source` in `roadmap.json` (not rendered). Plans are worded as plans;
  ideas sit under Exploring. Nothing is invented.

## The playground

It wears the site's tokens (background, surface, text, border, accent, light and dark), Geist and Geist Mono loaded without
blocking paint, and Undra's mark as its tab icon, so moving from the site into `/playground/` reads as one product. The theme
a visitor chose on the site carries over (same origin, `undra-theme` in `localStorage`) unless the URL forces one.

## The posts

Three posts in the format of the five: the launch post (what it is, who it is for, what is in 1.0, five minutes, what it is
not), the honest account of a week of outside use (status checkpoint 34, the team unnamed), and the JavaScript runtime at
16 KB (ADR-057 and its review). Each has a `claims.md` beside it; each number the launch post shows that a record supplies
is a generated slot. A newcomer would also be served by "How the testkit runs a core without a device"; it is the next post
to write, not one of these three.

## Checks

`node site/scripts/build-all.mjs` (nothing stale), `check-links.mjs` on the source and on the staged `_site/`, `--words`
(275 of 350), the playground's type-check, build and tests, screenshots at 375 and 1280 px in light and dark, with and
without JavaScript, and the diagram's tap driven in Chromium with and without reduced motion.
