#!/usr/bin/env node
// Renders the roadmap from site/data/roadmap.json:
//   * site/roadmap/index.html: the sections between <!-- roadmap:start --> and <!-- roadmap:end -->, the jump links
//     between <!-- roadmap-nav:start --> and <!-- roadmap-nav:end -->, the ItemList JSON-LD between
//     <!-- roadmap-ld:start --> and <!-- roadmap-ld:end -->, and the "Last updated" date;
//   * site/index.html: the items marked `"teaser": true`, between <!-- roadmap-teaser:start --> and
//     <!-- roadmap-teaser:end -->, so the landing page and the roadmap say the same thing in the same chips.
//
// A section has either `items` (cards) or `groups` (titled lists, for the long Shipped section). Every status
// chip is `.st .st-<section id>` (or `.st-no`), one scale defined once in assets/base.css.
//
//   node site/scripts/build-roadmap.mjs
import { join } from "node:path";
import { SITE, ORIGIN, read, writeIfChanged, replaceRegion, esc } from "./lib.mjs";

const data = JSON.parse(read(join(SITE, "data", "roadmap.json")));
const slug = (s) => s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
const CHIPS = new Set(["shipped", "next", "later", "exploring"]);
for (const sec of data.sections) {
  if (!CHIPS.has(sec.id)) throw new Error(`roadmap.json: section id ${sec.id} has no chip in assets/base.css (${[...CHIPS].join(", ")})`);
  if (!sec.items === !sec.groups) throw new Error(`roadmap.json: section ${sec.id} needs items or groups, not both`);
}
const itemsOf = (sec) => (sec.groups ? sec.groups.flatMap((g) => g.items) : sec.items);
const chip = (id, label) => `<span class="st st-${id}">${esc(label)}</span>`;

const sections = data.sections.map((sec) => {
  const side = `<div class="rm-side">${chip(sec.id, sec.status)}<h2 id="${sec.id}-h">${esc(sec.title)}</h2><p class="rm-when">${esc(sec.when)}</p><p>${esc(sec.intro)}</p></div>`;
  const body = sec.groups
    ? [
        `<div class="rm-groups">`,
        ...sec.groups.map((g) => [
          `<section class="rm-group" aria-labelledby="${sec.id}-group-${slug(g.title)}"><h3 id="${sec.id}-group-${slug(g.title)}">${esc(g.title)}</h3><ul>`,
          ...g.items.map((it) => `<li id="${sec.id}-${slug(it.title)}"><b>${esc(it.title)}</b><span>${esc(it.text)}</span></li>`),
          `</ul></section>`,
        ].join("\n")),
        `</div>`,
      ].join("\n")
    : [`<ul class="rm-items">`, ...sec.items.map((it) => `<li id="${sec.id}-${slug(it.title)}"><h3>${esc(it.title)}</h3><p>${esc(it.text)}</p></li>`), `</ul>`].join("\n");
  return [`<section class="rm rm-${sec.id}" id="${sec.id}" aria-labelledby="${sec.id}-h">`, side, body, `</section>`].join("\n");
});
const np = data.notPlanned;
sections.push(`<section class="rm" id="not-planned" aria-labelledby="not-planned-h">\n<div class="rm-side">${chip("no", "Not planned")}<h2 id="not-planned-h">${esc(np.title)}</h2></div>\n<p class="rm-no">${esc(np.text)}</p>\n</section>`);

const nav = [...data.sections.map((sec) => `<a href="#${sec.id}">${esc(sec.status)}</a>`), `<a href="#not-planned">Not planned</a>`].join("");

const list = [];
for (const sec of data.sections) for (const it of itemsOf(sec)) list.push({ "@type": "ListItem", position: list.length + 1, name: `${it.title} (${sec.status})`, description: it.text, url: `${ORIGIN}roadmap/#${sec.id}-${slug(it.title)}` });
const ld = { "@context": "https://schema.org", "@type": "ItemList", name: "Undra roadmap", description: "What is shipped, next, later and being explored for the Undra framework.", url: `${ORIGIN}roadmap/`, dateModified: data.updated, numberOfItems: list.length, itemListElement: list };

const file = join(SITE, "roadmap", "index.html");
let html = read(file);
html = replaceRegion(html, "roadmap", sections.join("\n"), "  ");
html = replaceRegion(html, "roadmap-nav", nav, "");
// The page's "Last updated" date is the data's `updated`, so the two cannot disagree.
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const [y, m, d] = data.updated.split("-").map(Number);
if (!y || !m || !d) throw new Error(`roadmap.json: updated must be YYYY-MM-DD, got ${data.updated}`);
if (!/<!--updated-->[^<]*<!--\/updated-->/.test(html)) throw new Error("roadmap/index.html has no <!--updated-->..<!--/updated--> slot");
html = html.replace(/<!--updated-->[^<]*<!--\/updated-->/, `<!--updated-->${d} ${MONTHS[m - 1]} ${y}<!--/updated-->`);
html = replaceRegion(html, "roadmap-ld", `<script type="application/ld+json">\n${JSON.stringify(ld)}\n</script>`, "");
const a = writeIfChanged(file, html);

// The landing page's "What is still open": the teaser items, in the roadmap's order, each linking to its card.
const teaser = data.sections.flatMap((sec) => itemsOf(sec).filter((it) => it.teaser).map((it) => ({ sec, it })));
if (teaser.length < 1 || teaser.length > 4) throw new Error(`roadmap.json: mark one to four items "teaser": true (found ${teaser.length})`);
if (teaser.some(({ sec }) => sec.id === "shipped")) throw new Error(`roadmap.json: a teaser item is something still open, not a shipped one`);
const teaserHtml = [`<ul class="now3 reveal">`, ...teaser.map(({ sec, it }) => `<li><a href="roadmap/#${sec.id}-${slug(it.title)}">${chip(sec.id, sec.status)}<span>${esc(it.title)}</span></a></li>`), `</ul>`].join("\n");
const indexPath = join(SITE, "index.html");
const b = writeIfChanged(indexPath, replaceRegion(read(indexPath), "roadmap-teaser", teaserHtml, "    "));
console.log(`build-roadmap: roadmap ${a ? "updated" : "up to date"}, landing teaser ${b ? "updated" : "up to date"}`);
