# Handoff — launch v2 in progress (2026-09-30, late evening)

The product is **Undra** (renamed from the working name Keel today, ADR-030). `main` is
`shreypdev/undra`; the site is https://shreypdev.github.io/undra/. CI was fully green twice
before the rename merged; the post-rename and post-site runs are being watched.

## Landed today, after v1
1. ADR-029 — the loaded host cdylib keeps its schema and JNI exports (non-incremental shim,
   private target dir); the contract jobs went green on CI.
2. Test-harness truth: `contract-tests/run-all.sh` reports which runner failed and why; S04's
   delays are 150 ms apart; the fixed keel-query finding is a regression test; the C-ABI
   allocation claim is asserted per call.
3. ADR-030 — the rename, as an idempotent script (`scripts/rename-keel-to-undra.sh`, takes paths;
   `docs/AGENT_WORKFLOW.md` has the "bringing a branch across" recipe).
4. Site v2 with Amendment A (the founder's v1 palette and structure; 350-word budget enforced),
   four comparison posts with a 95-row claims ledger and a fable fact-check, roadmap page,
   SEO/JSON-LD/sitemap/RSS/llms.txt/IndexNow, live wasm demo with measured counters, error-codes page.
5. Docs truth pass from the fact-check.

## In flight (one worktree each; see status.md)
Every piece of the launch-v2 spec is merged (checkpoints 1–3 in status.md): ADR-029…033, the rename,
site v2 + blog, distribution (dry run green), the harsh-conditions harness, the Swift error channel,
frame-coalesced delivery (contracts 54/54), deterministic transport tests. No open worktrees: the bench follow-ups (ratio gates, same-VM base baseline in CI, contended
completions, drift gate) and S1b (the playground stress screen) are merged too. Each: adversarial review,
full local matrix, CI green, `state(<piece>)` commit, `scripts/wt.sh rm`.

## Next after those
* Device-phase numbers for Kotlin/Swift drains (ADR-031 measured TS only); the `undra init` Android template could adopt `ChoreographerFramePacer` like the playground.
* Founder steps (`docs/RELEASING.md`; the dry run is green): npm org `undra` + automation token →
  `NPM_TOKEN`; repository `shreypdev/homebrew-undra` with `Formula/` + fine-grained PAT →
  `HOMEBREW_TAP_TOKEN`; the `release` environment with a `v*` tag restriction and a `v*` tag ruleset;
  optionally `undra.rs` and a GitHub org; then `scripts/bump-version.sh 1.0.0` → tag `v1.0.0`.
* An AVD named `undra` on the dev machine (docs say `undra`; the machine still has `keel`).
* v1.x queue, unchanged: device-measured bench rows; Android `undra dev` remote mode; dev-client
  auto-reconnect; `WeakCtx`; macro diagnostic polish; Swift `Port*` public; full-JSON
  `undra_schema_json`; crates.io publishing under `undra-*` (ADR first).

## How to run everything
`source scripts/env.sh`, then the commands in status.md's matrix table and `docs/ONBOARDING.md`;
`bash contract-tests/run-all.sh` for the scenario grid.

## v1.1 / v1.2 program (2026-10-01)
Spec: `.10x/specs/2026-10-01-v1x-default-choice-design.md` (tracks A–H; Amendment A: the founder
approved all six v1.2 bets). Phase 1 in flight: `gaps` (audit + ADR drafts 034–037), `competitive`
(sourced limitations matrix), `dev-loop` (B1 Android remote mode, B2 auto-reconnect), `schema-json`
(C1, C2), `diagnostics` (D1), `device-bench` (E1), `react-native` (G1 with ADR-038),
`derived-lists` (E2 ADR-039). Distribution stays parked until after v2.
