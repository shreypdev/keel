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
ADR-019 amendment; Lows L2–L5/L8 open), docs-reference (H3), tooling (D2–D5), wasm-size (E5, ADR-052; `ts-runtime-size` owed), dev-reload (B3, ADR-053), rn-adapters (G1b), swift-fs, derived-lists (E2, ADR-039), abi-table (ADR-044; `ns-storage` owed), devtools (B4, ADR-054: sonnet review `.10x/reviews/2026-10-02-devtools-review.md`, 3 Medium fixed — the one 404, a silent panicking inspector, cache sampling under its lock; Rust 2,789 · contracts 60/60). Main moves with each checkpoint. CI pins Rust 1.98.1
(1.99.0 broke it on 2026-10-01; the bump is a deliberate piece: four workflow pins, `rustup update`,
`TRYBUILD=overwrite` goldens, bench re-baseline).

**Merge queue (cross-merge main on the branch, full matrix, fast-forward).** Each branch lives in
`/Users/shrey/Desktop/src/.work/<name>` with its record in `.10x/decisions/sde/<name>.md` there. State on
2026-10-02 when the usage budget ran low (5-hour cap, then the weekly cap; agents stop mid-step and resume
with their context when told to):
2. `testkit` (F1/F2, ADR-055): **implemented and complete** at `ea5cc38` (18 commits, main `1b9b605` is an
   ancestor, matrix green: Rust 2,756, Swift 551, Kotlin 656, TS 1,133 + kit 26, contracts 60/60; `undra dev
   --record`, the three kits, `docs/TESTING.md`, a site page). Needs its review (sonnet is enough), then
   fast-forward. The `dev_reload` tests are load-sensitive on a busy machine (fail on main too under load 13+).
3. `ports-v2` → record as **ports** (G2/G3, ADR-047/048 written; implementing, 23 commits, merging main;
   the founder approved rusqlite/libsqlite3-sys dev-only, sqlite-jdbc test-only, wa-sqlite dev dep; the
   amalgamation is declined — RN Android uses JNI). Needs an opus review (new boundary surface), then land.
4. `persistence-v2` → record as **persistence** (ADR-037/049 Accepted; implemented, 77 commits; opus review
   started with the cross-merge of the ABI table — 506 files mid-merge when the budget ran out). The review
   brief: size gate first (the hello-world wasm sat 435 B under budget before the cross), then migrating
   restore integrity, the hand-written JSON writer, storage errors on every column, web recovery.
Order to land: 1, 2, 3, 4 (4 is the largest cross). Then the follow-ups below.

**Also owed:** the Rust 1.99.0 bump (ci.yml header says how; do it when no worktree is mid-build);
`ts-runtime-size` (16 KB target);  a custom port in the playground for the reference's Ports section; `undra bindgen --declarations`.

**Next:** wave 0 of `.10x/specs/2026-10-01-boundary-surface-plan.md` (`abi-table` ADR-044 — after
Track A and RN merge, it rewrites the FFI they touch; `ios-floor` ADR-045 — after parity;
`newtypes` ADR-042 — after parity and Track A), `persistence-v2` (ADR-037 + A6/A7 of ADR-049),
`dev-reload` (B3), `testkit` (F1, F2), derived lists (ADR-039), E4 binding call path, then waves
1–3 and the v1.2 bets (B4 devtools, G2/G3 ports, G4 Dart), H1–H4 as the APIs settle.
