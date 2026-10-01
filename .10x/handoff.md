# Handoff — the v1.1 / v1.2 program is landing (2026-10-01)

The product is **Undra** (renamed from the working name Keel on 2026-09-30, ADR-030). `main` is
`shreypdev/undra`; the site is https://shreypdev.github.io/undra/.

## Where things stand (2026-10-01, evening)
Launch v2 is complete and live (status.md checkpoints 1–4). The v1.1/v1.2 program (the section
below) is landing piece by piece: checkpoints 5–6 are merged, the queue is listed there. The
founder-only release steps are unchanged (`docs/RELEASING.md`; the dry run is green): npm org
`undra` + automation token → `NPM_TOKEN`; repository `shreypdev/homebrew-undra` with `Formula/`
+ fine-grained PAT → `HOMEBREW_TAP_TOKEN`; the `release` environment with a `v*` restriction and
a `v*` tag ruleset; then `scripts/bump-version.sh 1.0.0` → tag `v1.0.0`. Distribution is parked
until after v2 by the founder's decision.

## How to run everything
`source scripts/env.sh`, then the commands in status.md's matrix table and `docs/ONBOARDING.md`;
`bash contract-tests/run-all.sh` for the scenario grid.

## v1.1 / v1.2 program (2026-10-01)
Spec: `.10x/specs/2026-10-01-v1x-default-choice-design.md` (tracks A–H; Amendments A–D: all six v1.2
bets approved; gap-audit and boundary ADRs 034–051 accepted in direction). Distribution stays parked
until after v2.

**Merged (checkpoints 5–6 in status.md):** gaps + competitive + ADR-039 (design), schema-json (C1, C2),
device-bench (E1; the honest web size is 135 KB gzipped, over budget — E5), diagnostics (D1), dev-loop
(B1, B2, ADR-051), parity (C3, C4: the Kotlin/TS error channel), react-native (G1, ADR-038; G1b default
adapters, E4 Hermes cost and a site page are open), android-adapters (M1 Maven publishing open), runtime-lifecycle (Track A: ADR-034/035/036 + the
ADR-019 amendment; Lows L2–L5/L8 open), docs-reference (H3), tooling (D2–D5). Main `aa66ce9`. CI pins Rust 1.98.1
(1.99.0 broke it on 2026-10-01; the bump is a deliberate piece: four workflow pins, `rustup update`,
`TRYBUILD=overwrite` goldens, bench re-baseline).

**Merge queue (cross-merge main on the branch, full matrix, fast-forward).** Every branch below lives in
`/Users/shrey/Desktop/src/.work/<name>` with its record in `.10x/decisions/sde/<name>.md` there:
1. `wasm-size` (E5, ADR-052 Accepted; opus review `.10x/reviews/2026-10-01-wasm-size-review.md`: merge after
   fixes, all on the branch at `66954d1`, main is an ancestor). **One integrator decision is owed (D1):** after
   merging main the JS runtime measures 24,841 B gzipped against the 24 KB budget set in ADR-052 decision 2.
   Decision: restate the JS budget to **26 KB** in `bench/budgets.toml` + ADR-052 (dated note: parity's error
   channel added ~900 lines; `ts-runtime-size` targets 16 KB), then `scripts/wasm-size.sh --record`,
   `node site/scripts/build-all.mjs` (README/site move to 102.7 KB), commit, fast-forward, `wt.sh rm`.
3. `dev-reload` (B3; ADR-053 Accepted with four decisions; implementing).
4. `rn-adapters` (G1b; ADR-038 Amendment B accepted: hybrid with a C++ Kv/Fs core; implementing; adds a
   `platform` module to the playground core — regenerate bindings at the cross).
5. `abi-table` (ADR-044; implementing; migrates the RN host to the table).
6. `persistence-v2` (ADR-037 + ADR-049 A6/A7; implementing).
7. `derived-lists` (ADR-039; implementing; S19, nine bench rows, the playground's `Todos` on recorded ops).
Each still needs its adversarial review (opus) before merging, except 1 and 2 which have one.

**Also owed:** the Rust 1.99.0 bump (ci.yml header says how; do it when no worktree is mid-build);
`ts-runtime-size` (16 KB target); the Swift Fs adapter misses SPEC §8's symlink rule (rn-adapters' record has
the case); a custom port in the playground for the reference's Ports section; `undra bindgen --declarations`.

**Next:** wave 0 of `.10x/specs/2026-10-01-boundary-surface-plan.md` (`abi-table` ADR-044 — after
Track A and RN merge, it rewrites the FFI they touch; `ios-floor` ADR-045 — after parity;
`newtypes` ADR-042 — after parity and Track A), `persistence-v2` (ADR-037 + A6/A7 of ADR-049),
`dev-reload` (B3), `testkit` (F1, F2), derived lists (ADR-039), E4 binding call path, then waves
1–3 and the v1.2 bets (B4 devtools, G2/G3 ports, G4 Dart), H1–H4 as the APIs settle.
