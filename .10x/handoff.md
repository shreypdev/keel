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
ADR-019 amendment; Lows L2–L5/L8 open), docs-reference (H3), tooling (D2–D5), wasm-size (E5, ADR-052; `ts-runtime-size` owed), dev-reload (B3, ADR-053), rn-adapters (G1b), swift-fs, derived-lists (E2, ADR-039), abi-table (ADR-044; `ns-storage` owed), testkit (ADR-055), docs-v1x (cookbook + Fieldbook), devtools (B4, ADR-054: sonnet review `.10x/reviews/2026-10-02-devtools-review.md`, 3 Medium fixed — the one 404, a silent panicking inspector, cache sampling under its lock; Rust 2,789 · contracts 60/60). Main moves with each checkpoint. CI pins Rust 1.98.1
(1.99.0 broke it on 2026-10-01; the bump is a deliberate piece: four workflow pins, `rustup update`,
`TRYBUILD=overwrite` goldens, bench re-baseline).

**Merge queue (cross-merge main on the branch, full matrix, fast-forward).** Each branch lives in
`/Users/shrey/Desktop/src/.work/<name>` with its record in `.10x/decisions/sde/<name>.md` there. State on
2026-10-02 when the usage budget ran low (5-hour cap, then the weekly cap; agents stop mid-step and resume
with their context when told to):
3. `ports-v2` → record as **ports** (G2/G3): implemented; **opus review interrupted mid-way** at `adab13f`
   (main `e3573f7` merged; 31 conflicts resolved; the three ports on `dispatcher_by_use`; playground hash
   `0xb5b7b1dc29182a9d`). Done in the review: a Major Rust bug fixed (a cancelled `connect`/`open`/`begin`
   orphaned what the platform opened — `crates/undra-ports/src/owned.rs`, two tests), the web Live view after
   a crash restart, two cores on one database name (shared by design, tested), the web Db serves one tab per
   origin (documented). **Open, blocking B1:** the hello-world JS is 26,132 B gzipped vs the 26,000 gate; the
   planned fix (IIFE the codecs, move the three opt-in `PortIds` to `OptInPortIds` exported from the
   `realtime`/`db` subpaths, shrink `PortImpl.dispose` to dispose-on-close) is written in the report but not
   applied. **Uncommitted in the worktree:** the four platform sub-reviews' edits under `runtimes/{swift,
   kotlin,ts,rn}` (TS: `core.ts`, `port.ts`, `recovery.ts`, `recovery.test.ts`; `test/zz-scratch-review.test.ts`
   must not be committed). **Not done:** the full matrix (74/74 contracts expected), ADR-047/048 → Accepted,
   `.10x/reviews/2026-10-02-ports-review.md`. Resume the reviewer (agent context is intact) with: collect the
   sub-reviews, apply B1, matrix once, review doc, report.
Landed: devtools, persistence (ADR-037/049 Accepted; opus review `.10x/reviews/2026-10-02-persistence-review.md`: 3 High fixed — a wrong-typed migration hook spliced bytes, RN storage not on ADR-049, a dead web core after a trap during restart; hello wasm 116.8 KB, JS 25,984/26,000; Rust 2,891 · Swift 553 · Kotlin 652 · TS 1,264 · contracts 65/65; hash `0xfa536b9ac6f06149`). Still to land: testkit (review) then ports (review; it must cross persistence: `check.sh`, `run-all.sh`, `scenarios.md`, SPEC §8).

**Also owed:** the Rust 1.99.0 bump (ci.yml header says how; do it when no worktree is mid-build);
`ts-runtime-size` (16 KB target);  a custom port in the playground for the reference's Ports section; `undra bindgen --declarations`.

**Next:** wave 0 of `.10x/specs/2026-10-01-boundary-surface-plan.md` (`abi-table` ADR-044 — after
Track A and RN merge, it rewrites the FFI they touch; `ios-floor` ADR-045 — after parity;
`newtypes` ADR-042 — after parity and Track A), `persistence-v2` (ADR-037 + A6/A7 of ADR-049),
`dev-reload` (B3), `testkit` (F1, F2), derived lists (ADR-039), E4 binding call path, then waves
1–3 and the v1.2 bets (B4 devtools, G2/G3 ports, G4 Dart), H1–H4 as the APIs settle.
