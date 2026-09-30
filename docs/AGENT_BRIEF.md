# Brief for implementation agents

You are one engineer on the Undra team. You own exactly the piece named in your task and
nothing else. The binding process — worktrees, briefs, review, merge, cleanup — is
`docs/AGENT_WORKFLOW.md`; read it first, then `CLAUDE.md`, then the SPEC sections your
task names.

Ground rules, condensed:

* Work ONLY in the worktree path your task names (`../.work/<slug>`, branch `wt/<slug>`,
  made with `scripts/wt.sh new <slug>`). Never merge, rebase onto, or push `main`.
* `source scripts/env.sh` first; run suites directly (no slice runners — this is native
  macOS; `docs/ONBOARDING.md` has every command).
* The spec is binding; if you believe it is wrong, implement it as written and list the
  concern in your report — EXCEPT where a shipped consumer (the three platform runtimes,
  the bindgen goldens) already disagrees: then the consumer wins and your report says so.
* Quality bar: R1–R12 of CLAUDE.md. Tests beside code + `tests/`, proptest where inputs
  are open-ended, docs on every `pub` item, `cargo clippy -p <crate> --all-targets -- -D
  warnings`, `cargo fmt`, wasm32 build for core crates, no `TODO`/`unimplemented!()`, no
  new dependencies beyond the workspace list unless the task allows it.
* Small `type(scope): summary` commits; `git status` clean at the end.

## When you finish

Report, in this order, in under 60 lines:

1. Branch and worktree path.
2. What you built (files, public API summary).
3. Test summary (counts, what they cover, which checks ran green).
4. Deviations from the spec or the task, and why.
5. Open questions or concerns for the reviewer.
