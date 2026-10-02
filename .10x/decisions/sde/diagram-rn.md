# SDE: React Native in the landing diagram (2026-10-02)

Branch `wt/diagram-rn`, from `main` `a309e9f`. The landing page's "Architecture · live" card drew three UIs
(SwiftUI, Compose, React); React Native had shipped (ADR-038) and was named in the hero but absent from the picture.
It is now the fourth frame, tag `useSignal`: React Native runs the same TypeScript mirror and hooks under the JSI
host (`docs/REACT_NATIVE.md`), so the tag is the React one and no API was invented. Constraints from `docs/SITE.md`
held: the v1 palette and Geist, no new colours, chips or labels, the landing prose budget (the diagram's text is not
counted; 347 words before and after).

## What exists, and where the "three" was assumed

Both SVGs are hand-authored in `site/index.html` (nothing under `site/scripts/` emits them), animated by
`site/assets/hero.js`. The script assumed three in four places: the node loop (`i < 3`), the beam loop (`w0..w2`,
`c0..c2`), the origin (`loop % 3`) and, silently, that a write beam is straight (the boundary-crossing time was a
linear fraction of its y range). It now counts `[data-node]` groups in the SVG it rigs, derives the origin from that
count, and finds the crossing along the path (a 20-step bisection on `getPointAtLength`), so a bent beam rings at the
right moment and place. The 8 s loop is unchanged; one full rotation through the UIs is 32 s instead of 24 s.

## Layout

| | Before | After |
|---|---|---|
| Wide frames | 3 x 320 wide at x = 80 / 440 / 800 (gap 40) | 4 x 248 wide at x = 80 / 344 / 608 / 872 (gap 16), edges on the core block's 80..1120 |
| Frame insides | text at +22, mirror +14 (292 wide) | text at +18, mirror +10 (228 wide), rows, separators and highlight inset in proportion; type sizes untouched (title 19, tag 12.5, rows 14) |
| Beams | write at frame x+100, change-set at x+220 (120 apart) | write at centre-16, change-set at centre+16 (32 apart): 188 / 220, 452 / 484, 716 / 748, 980 / 1012 |
| Band chips | between beams: 20..166, 312..528, 692..868 | 20..166, 228..444, 524..676 (centred in the gaps between a change-set beam and the next write beam; 8 px clear of the beams at the tightest, the middle chip) |
| Phone | 3 frames 104 x 138 in a row (viewBox 360 x 474) | 2 x 2 grid, 132 x 138 (viewBox 360 x 628), core and band moved down 154 |
| Phone beams | straight down from each frame | bottom row straight down; the top row leaves the inner sides of its frames (a 12 px rounded elbow) and runs down a 80 px middle channel: 152 / 164 and 196 / 208, nothing crosses a frame |
| Phone chips | numbered dots at 18 / 120 / 240 | 24 / 127 / 233, in the gaps between beams |

Two details the narrower frame forced.

* **The call chips lose `store.`.** In a 248 px frame the Swift chip `store.add(title: "milk")` (180 px of mono text)
  ran under the right-aligned "2 left" badge (the badge starts 183 px in). Dropping the receiver (`add(title: "milk")`,
  `add("milk")`) leaves 12 px between chip and badge in the Swift frame (the only one that shows a chip in the static
  state). All four chips follow the same rule; the phone layout still says `store.add()`. The alternatives were hiding
  the badge while a chip is up (breaks the static first paint, which shows both) or moving the badge (a new element).
* **The third band chip is 152 wide, not 176**: its text needs 140; the extra 36 px was visible slack and would have
  collided with nothing, but it is now centred on the gap.

Title and tag: "React Native" is 114 px at 19 px, "useSignal" 67 px; they sit 30 px apart in a 248 px frame, so the
tag stays on the title's line (no drop to a second line was needed).

## Phone layout: two were built

* **2 x 2 (kept).** Every label survives (13 px titles, `store.add()`, `local reads`, the three-bar mirrors), "React
  Native" fits on one line, the picture reads as the wide one folded. Cost: 154 units taller (about 135 px on a
  phone), the middle channel, and beams that are not all the same shape.
* **1 x 4 (rejected, mock kept in the scratchpad).** Four 80 px frames: "React Native" wraps to two lines, titles drop
  to 12 px, `local reads` and `store.add()` have to become `reads` and `add()`, "Compose" touches its badge, the bars
  are slivers. Shorter, but it gives up copy and legibility for height.

At 768 px the narrow SVG (max-width 520) is 907 px tall; it is one scroll on a tablet and kept.

## The architecture page's figure

`site/docs/architecture.html` has a sibling, a static "layers on each platform" figure (three stacks above the core).
Four equal columns would put its text at about 9 px in the page's 600 px column (it renders at 0.88 already), and React
Native does share two of those layers, so it is drawn as the two hand-written boxes "React" and "React Native" above
one shared stack (generated npm package, `@undra/runtime`), the transport box reading "wasm, worker, WebSocket or JSI".
Swift and Compose stack widths went from 204 to 176 (their longest label, 152 px, still has 12 px a side). A caption
sentence and one sentence of prose say how the core reaches React Native; the page's own claims ("three generators,
three runtimes") stay true, since React Native adds neither. `dateModified` bumped; `build-all` regenerated the search
index, `llms-full.txt` and the sitemap lastmod.

## Verified

Headless Chrome through CDP (a driver in the scratchpad: emulated colour scheme and reduced motion, `stage.seek`),
plus the browser pane for the live checks. Geometry checked by script at 1280, 1000, 768, 375 and 320 px: no text
outside its frame, no title/tag, title/badge, call/badge or idle/badge overlap, no frame overlap, no beam through a
frame or crossing another (change-set beams share their bus by design), no band chip within 4 px of a beam, no
horizontal page scroll; the same screenshots in dark and light at 1280, 1024, 768 and 375 (wide and narrow
layouts), the no-JS first paint (four frames, `is-static`), and `prefers-reduced-motion` (four frames, "Play
animation", Play starts the loop).

The sequence, observed live for 34 s at 1280 px and by seeking at 375 px, identical in both layouts: 0 s SwiftUI
writes (call chip, frame outline, pulse down `w0`), crossing, commit, the change-set leaves on `c0..c3`, all four
badges turn "1 left" to "2 left" and all four mirrors take the new row; 8 s Compose (`w1`); 16 s React (`w2`); 24 s
React Native (`w3`); 32 s SwiftUI again. Pause freezes the frame and the label turns "Play"; Play resumes. Switching
from the wide to the narrow layout mid-loop rebuilds the rig and keeps animating, with no console error.

`node site/scripts/build-all.mjs` leaves no diff on a second run; `sync-chrome.mjs --check` and `check-links.mjs`
pass; `check-links.mjs --words` reports 347 of 350.

## For the next frame

Add a `g[data-node="N"]` to both SVGs, a `w<N>` and a `c<N>` beam with their dots, and re-space the frames; nothing in
`hero.js` has to change. The wide layout is out of room for a fifth at these type sizes (the frames would be about 195 px);
that would be a second row or a smaller type scale, a decision for whoever needs it.
