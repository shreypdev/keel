# QA — launch-v2 (2026-09-30)

* Rename gate: `grep -rni keel` over the tree returns only `.10x/reviews/`, ADR-018…029,
  the rename script and ADR-030; every suite (Rust, TS, Kotlin, Swift, wasm, C, contracts
  ×3, playground apps on the three platforms) green before review.
* Stress budgets are tests (R9): each scenario in the design gets a row in
  `bench/budgets.toml`; a regression fails CI's Budgets job. The soak runs 10 s in CI and
  60 s locally with RSS growth ≤ 1 %.
* Site gates: Lighthouse ≥ 95 ×4 (checked in the built-in browser by the integrator), no
  broken internal links (`site/scripts/check-links.mjs`, run in `site.yml`), generated
  files fresh, every page has title/description/canonical/og/JSON-LD, `sync-chrome`
  produces no diff.
* Distribution gates: the release dry run builds all four targets; `install.sh` is run in
  CI against the dry-run artifacts (served from a local `python3 -m http.server`) and
  must verify checksums and produce an `undra` that prints its version; the npm meta
  package is smoke-installed from a local tarball (`npm pack` → `npm i -g ./…tgz` →
  `undra --version`).
* Blog: an adversarial fact-check review (fable) of every competitor claim, recorded in
  `.10x/reviews/2026-09-30-blog-factcheck.md`.
