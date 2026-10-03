# pr-gate — the merge gate as GitHub enforces it (2026-10-02)

**What:** one roll-up job, `all-green` ("All green"), at the end of `ci.yml`, `bench.yml`, `two-cores.yml` and
`site.yml`: `needs` every other job of the workflow, `if: always()`, and fails unless each result is `success`.
Branch protection on `main` requires those four checks; nothing else need be required, because each one covers its
workflow's jobs, and new jobs are added to its `needs` (a job left out is a job the gate does not see — the
reviewer checks the list when a workflow changes).

**Why a roll-up and not every job:** required checks are named in the repository settings; naming thirty jobs there
means editing settings whenever a job is renamed, and a renamed job silently stops being required. One name per
workflow, stable, is what the settings hold.

**Why `if: always()`:** a job that does not run reports nothing, and a required check that never reports blocks the
pull request forever. With `always()` the roll-up runs after failures and cancellations and says so. The deploy
job of Site is `main`-only and is left out of its roll-up: a skipped deploy on a pull request is correct.

**Why Site has no path filter on pull requests any more:** a path-filtered required workflow that does not run leaves
its check "expected" forever (GitHub's documented trap). The build-and-check job takes minutes; it now runs on
every pull request, and the push trigger keeps its paths so `main` deploys only when the site's inputs changed.

**Why no more `wt/**` push triggers:** a pull request runs the workflows on its head; a second run per push to the
branch doubled the runner load for nothing. `scripts/wt.sh pr <slug>` opens a draft so CI runs from the first push.

**`scripts/wt.sh merge`:** the pull-request path is the default (open or find the PR, ready it, wait for the checks,
`wt-ci-check.sh pr-verdict` on `gh pr checks --json`, `gh pr merge --merge --delete-branch`, verify `origin/main`
contains the head, clean up). `--ff` keeps the fast-forward path for repositories without protection and for
`scripts/wt-cleanup.test.sh`, whose scratch repositories have no GitHub.

**Not done here:** no twin workflow for Site (not needed once it always runs on pull requests); no auto-merge.
