#!/usr/bin/env node
// Renders site/roadmap/index.html from site/data/roadmap.json: the section cards between
// <!-- roadmap:start --> and <!-- roadmap:end -->, and the ItemList JSON-LD between
// <!-- roadmap-ld:start --> and <!-- roadmap-ld:end -->.
//
//   node site/scripts/build-roadmap.mjs
import { join } from "node:path";
import { SITE, ORIGIN, read, writeIfChanged, replaceRegion, esc } from "./lib.mjs";

const data = JSON.parse(read(join(SITE, "data", "roadmap.json")));
const slug = (s) => s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");

const sections = data.sections.map((sec) => {
  const items = sec.items.map((it) => `<li id="${sec.id}-${slug(it.title)}"><h3>${esc(it.title)}</h3><p>${esc(it.text)}</p></li>`).join("\n");
  return [
    `<section class="rm" id="${sec.id}" aria-labelledby="${sec.id}-h">`,
    `<div class="rm-side"><span class="st st-${sec.id}">${esc(sec.status)}</span><h2 id="${sec.id}-h">${esc(sec.title)}</h2><p class="rm-when">${esc(sec.when)}</p><p>${esc(sec.intro)}</p></div>`,
    `<ul class="rm-items">`,
    items,
    `</ul>`,
    `</section>`,
  ].join("\n");
});
const np = data.notPlanned;
sections.push(`<section class="rm" id="not-planned" aria-labelledby="not-planned-h">\n<div class="rm-side"><span class="st st-no">Not planned</span><h2 id="not-planned-h">${esc(np.title)}</h2></div>\n<p class="rm-no">${esc(np.text)}</p>\n</section>`);

const list = [];
for (const sec of data.sections) for (const it of sec.items) list.push({ "@type": "ListItem", position: list.length + 1, name: `${it.title} (${sec.status})`, description: it.text, url: `${ORIGIN}roadmap/#${sec.id}-${slug(it.title)}` });
const ld = { "@context": "https://schema.org", "@type": "ItemList", name: "Undra roadmap", description: "What is shipped, in flight, next and later for the Undra framework.", url: `${ORIGIN}roadmap/`, dateModified: data.updated, numberOfItems: list.length, itemListElement: list };

const file = join(SITE, "roadmap", "index.html");
let html = read(file);
html = replaceRegion(html, "roadmap", sections.join("\n"), "  ");
html = replaceRegion(html, "roadmap-ld", `<script type="application/ld+json">\n${JSON.stringify(ld)}\n</script>`, "");
console.log(writeIfChanged(file, html) ? "build-roadmap: updated site/roadmap/index.html" : "build-roadmap: up to date");
