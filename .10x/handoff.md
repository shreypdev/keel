# Handoff — v1 complete

v1 shipped on branch `claude/keel-framework-takeover-66c4ea` (2026-09-30). The tree is
green across every gate; see status.md for the matrix and docs/HANDOFF.md §5 for the
definition it meets. `main` can fast-forward to this branch.

For whoever picks this up next:
1. Read status.md (the "Landed since takeover" ledger is the project history) and the
   four review files under .10x/reviews/ — every re-review verdict is recorded there.
2. ADRs 018–028 cover every decision made since the takeover.
3. The v1.x queue, in rough priority: device-measured bench rows (run the playground
   Bench hooks on an iPhone + Android device + Chromium and fill bench/RESULTS.md's
   device section); macros diagnostic polish (query-in-impl, split-impl follow-ons,
   NF1/NF2 wording); WeakCtx so long-lived tasks don't pin a dropped runtime; the L3
   release-build silent drop of off-runtime writes; keel_schema_json full-JSON variant so
   dlopen bindgen keeps docs; Swift runtime Port* types public (drops the bindgen
   fallback, ADR-024); Android remote (`keel dev`) mode; dev-client auto-reconnect;
   per-signal isolation of a panicking computed (ADR-019 note).
4. How to run everything locally: source scripts/env.sh, then the commands in
   status.md's table; contract-tests/run-all.sh for the scenario matrix;
   examples/playground/{web/npm run smoke, ios/smoke.sh, android/README.md} for apps.
