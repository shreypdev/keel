# SDE: no-em-dash, no em-dash in any tracked file (wt/no-em-dash, 2026-10-03)

**The rule (the founder, 2026-10-03):** nothing public-facing contains an em-dash (U+2014): not the website, the README,
the docs, program output, code comments, nor the working records under `.10x/` (the site links to `.10x/status.md` and
the decision records). Commit messages and pull requests follow it too. A plain hyphen is fine (the founder's
follow-up, the same day); the en-dash (U+2013) is not part of the rule.

**What changed:** 870 em-dashes on 821 lines of 208 tracked files became 0: `.10x/` 671, `site/` 77 (most of them the
`og:image:alt` of every page and the generated pages), `docs/` 44, `runtimes/` 26, `README.md` 23, `crates/` 11,
`CLAUDE.md` 7, `scripts/` 6, `contract-tests/` 4, `.github/` 1. Typography only: no number, name, hash, link or
meaning moved.

**How each was replaced (writing, not search-and-replace):**

* an explanation, a list or a consequence: a colon ("one row changed: 158 bytes on the wire");
* an aside: commas or parentheses ("apps (domain logic, ..., the dev loop) while the UI stays 100% native");
* two sentences: a full stop, or a semicolon when they are closely tied;
* a label and its description: a colon on public pages and in the docs; in the records, headings, index lines and
  "label - description" rows take a spaced hyphen (fast and cannot change meaning, as the follow-up allowed);
* a table cell that means "nothing here": `none`, or `n/a` where "not applicable" is meant (the Android adapters'
  failure tables); the competitive matrix's "not assessed" cell reads `?`, its legend says so;
* a title "X - Undra": the site's brand-first titles use a colon ("Undra roadmap: ...", "Undra blog: ..."), so the
  landing page is "Undra: the Rust core for native apps", and so is every page's `og:image:alt`;
* where a colon would follow a colon, the sentence was restructured (the README's feature list leads with a bold label
  and a full stop; "the worktree-per-piece workflow for humans and AI agents alike (brief → ... → clean up)").

**Program output, the one behaviour change.** The production build of `@undra/runtime` separated a message from its link
with an em-dash. It now says `T0017: callSync, remote; see https://shreypdev.github.io/undra/docs/errors.html#T0017`,
`T0005; see <link>` with no values, and `wire: code=unexpected_eof at=12 needed=3; see <link>#wire-unexpected_eof`
(`messages.prod.ts`). The tests that match the text (`messages.test.ts`, `flavours.test.ts`, `dist-flavour.test.ts`,
`crates/undra-ffi/tests/wasm/ts-runtime.test.mjs`), the errors page generator (`site/scripts/build-errors.mjs`) and its
output, SPEC section 12, `docs/ERRORS.md`, `docs/DEV_LOOP.md`, the TypeScript API page and ADR-057 (one dated line) say
the new form; records that quote a production message quote the new form (the one byte count of the old text, "66
bytes" in `ts-runtime-16k.md`, now describes "`T0115` and the link" so it stays true). The class, `kind` and fields are
unchanged, so nothing a program branches on moves. The web size record moved by a few bytes and was re-recorded:
`web/hello-runtime-js` 15,774 -> 15,758 (gate 16,000), with the helper 16,285 -> 16,284 (16,600), all features
39,922 -> 39,919 (42,400). The wasm row keeps its committed record: the core is untouched (this machine measures the
same module before and after the change).

**The check:** `scripts/check-no-em-dash.sh` runs `git grep -I` for the character's UTF-8 bytes over the working-tree
content of the tracked files (binary files skipped, untracked files not counted) and fails listing `file:line`; the
character is written as `$'\xe2\x80\x94'`, so the script passes its own check. `scripts/check-no-em-dash.test.sh` proves
it on scratch repositories (fails on a tracked file with the character and names the line; passes with an en-dash, a
binary file holding the bytes, an untracked file; an error outside a repository). Both run as the first step of CI's
Rust (Linux) job, before the toolchain install, so a pull request with an em-dash fails in seconds; `scripts/ci-local.rb`
reads the step from the workflow. `CLAUDE.md` ("Engineering standards"), `docs/AGENT_BRIEF.md` and
`docs/AGENT_WORKFLOW.md` (section 2) state the rule.

**Left for the founder:** 232 en-dashes in 49 files, none of them an em-dash substitute: numeric and id ranges
("3.2–3.5 µs", "R1–R12", "S01–S30", most of the 176 in `.10x/`), names ("Theil–Sen"), and "–" as an empty-value
placeholder in UI (the landing page's live-demo counters, the playground's stress view, the devtools counters, two
table cells of `docs/reviewing-generated-code.html`). The check does not look at commit messages (history keeps its
em-dashes; new subjects follow the rule by review) or at HTML entities (`&mdash;` appears nowhere; the site's entity
decoder in `site/scripts/lib.mjs` spells U+2014 as an escape).
