# Architect — launch-v2 (2026-09-30)

**Rename.** Product-wide, generated-shape-wide (ADR-030): crates, macros, C ABI symbols,
JNI names, Swift module, Kotlin package, npm scope, env vars, CLI. Done by an idempotent
script so every in-flight branch crosses it by "merge main → keep your side → run the
script → re-test". Goldens regenerate; history stays.

**Site.** Static multi-page HTML with zero runtime dependencies; in-repo Node scripts for
the build-time concerns (search index, blog index, llms.txt, header/footer sync, OG
render, link check); `site.yml` stages `_site/` = `site/` + the CI-built web playground.
The live demo is the real wasm core inside an iframe; counters are measured by the
playground around the runtime's apply and posted to the parent — the landing page never
fakes a number.

**Distribution.** One `release.yml` produces immutable artifacts (tarballs + sha256 +
checksums.txt) that three consumers read: the Homebrew formula, the npm platform
packages, and the curl installer. Nothing is built twice; nothing is published without the
tag matching the workspace version.

**Stress benchmark.** Investigation-led: the design must establish from ADR-018/019/020/023
and the three platform runtimes whether delivery is coalesced anywhere today. If a
100 k txn/s producer means 100 k main-thread hops/s, frame-coalesced delivery is a
runtime-model change → ADR-031 before code (R11). The suite itself is additive.

**Failure modes considered.** Registry names lost before the founder claims them (runbook
step 1); a missed identifier in the rename breaking one platform (every suite green before
review; the reviewer's ABI/JNI/module/CI checklist); the demo iframe failing on a visitor's
browser (the section degrades to the static numbers with a note); a release workflow
publishing from a non-tag ref (guarded by `startsWith(github.ref, 'refs/tags/v')`).
