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
adapters, E4 Hermes cost and a site page are open). Main `6fe1643`. CI pins Rust 1.98.1
(1.99.0 broke it on 2026-10-01; the bump is a deliberate piece: four workflow pins, `rustup update`,
`TRYBUILD=overwrite` goldens, bench re-baseline).

**Merge queue, in order (cross-merge main on the branch, full matrix, fast-forward):** `android-adapters` (`AndroidPlatformDefaults.install` + six adapters,
instrumented tests; hand-merge `UndraApp.kt` keeping dev-loop's start/load/retry skeleton and the
frame pacer, the adapters replacing the fakes; M1 Maven publishing is an open item) →
`runtime-lifecycle` (Track A; reviewed, merge after fixes: `.10x/reviews/2026-10-01-runtime-lifecycle-review.md`;
ADR-034 Amendment A on what a call pins is being written on the branch) → `wasm-size` (E5, ADR-052,
under review) → `tooling` (D2–D5, implementing). In parallel: `tooling` (D2–D5) and `wasm-size` (E5, ADR-052).

**Next:** wave 0 of `.10x/specs/2026-10-01-boundary-surface-plan.md` (`abi-table` ADR-044 — after
Track A and RN merge, it rewrites the FFI they touch; `ios-floor` ADR-045 — after parity;
`newtypes` ADR-042 — after parity and Track A), `persistence-v2` (ADR-037 + A6/A7 of ADR-049),
`dev-reload` (B3), `testkit` (F1, F2), derived lists (ADR-039), E4 binding call path, then waves
1–3 and the v1.2 bets (B4 devtools, G2/G3 ports, G4 Dart), H1–H4 as the APIs settle.
