# CTO — launch-v2 (2026-09-30)

**Problem.** v1 is complete and green, but the working name "Keel" cannot be owned on
npm or crates.io and collides with an active Rust project; nobody can install the CLI
without a Rust toolchain; the site's typography reads "agency" rather than
"engineering"; there is no comparison material for the KMP/UniFFI/Flutter conversations
that decide adoption; and the performance story stops at microbenchmarks.

**Decisions.**

1. Rename the product to **Undra** before anything is published (ADR-030). The founder
   chose it from a shortlist of six names that were clean on npm (scope and package),
   crates.io, GitHub, Homebrew and domains. The rename is a script so in-flight work
   crosses it mechanically.
2. Publish under names we own: npm `@undra/*`, GitHub `shreypdev/undra`, Homebrew tap
   `shreypdev/homebrew-undra`, GitHub Releases as the artifact source, a curl installer
   on the site; crates.io in v1.1.
3. Build vs buy for the site: keep hand-written static HTML/CSS/JS with zero runtime
   dependencies and in-repo Node scripts (search index, chrome sync, OG render, blog
   index). A static-site generator costs more in lock-in than it saves at 20 pages.
4. The stress benchmark is a product feature: its numbers go on the landing page and its
   budgets gate CI (R9). If it shows that delivery floods the platform thread, the fix
   (frame-coalesced delivery) gets an ADR and a v1.1 line — that is what the suite is for.
5. Comparison posts are written to be read by the other projects' maintainers: four
   posts, fact-checked adversarially, each with an honest "when to pick the other tool".
6. Founder-only actions (npm org, tap repository, tokens, the `v1.0.0` tag, Search
   Console, a domain, a GitHub organisation) are listed in `docs/RELEASING.md` and
   `docs/SITE.md`; everything else is automated.

**Success metric.** Section 0 of `.10x/specs/2026-09-30-launch-v2-design.md`.
