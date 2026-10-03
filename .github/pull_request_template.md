<!-- One piece per pull request (docs/AGENT_WORKFLOW.md). The four "All green" checks must pass on the head. -->

**Piece:** `wt/<slug>` — one line on what it does.

**Design:** ADR-NNN (or the amendment), or "no public shape changes".

**Review:** `.10x/reviews/<date>-<slug>-review.md`, verdict and what was fixed.

**Proof:** tests added, benchmarks or size rows that moved, what was not verified and why.

**Public API:** none changed, or the output of `undra schema diff --against <base>` pasted here with every `breaking` line justified. Review the schema diff, not the generated bindings (collapsed by GitHub, proven by `undra bindgen --check`; ADR-062).
