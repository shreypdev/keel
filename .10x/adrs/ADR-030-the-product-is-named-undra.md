# ADR-030: The product is named Undra — one identity across registries

**Status:** Accepted
**Date:** 2026-09-30
**Feature:** launch-v2
**Author:** 10x-Team (CTO + Architect); the name is the founder's decision

## Context

v1 was built under the working name Keel, which is a generated public shape everywhere:
the `keel` facade crate and `#[keel::store]`, the `keel_*` C ABI (SPEC §6), the Swift
module `KeelRuntime`, the Kotlin package `dev.keel.runtime` (and therefore the JNI symbol
names), the npm package `@keel/runtime`, the `keel` CLI and `KEEL_*` environment.

Checked on 2026-09-30, before anything was published: the npm user `keel` exists, so the
`@keel/*` scope can never be ours; `keel` (unscoped) belongs to keel.so; keel.sh is a
Kubernetes tool; another active Rust project called "keel" holds `keel-cli` and
`keel-macros` on crates.io and `@getkeel` / `@keel-dev` on npm; `keel.dev` is someone
else's live site. Shipping under a name we cannot own on the registries platform engineers
install from — and that collides with an active Rust project — would cost trust and search
placement from day one.

A search of about a hundred candidates (nautical and structural words, Latin/Greek/Spanish
words for keel, "under/beneath" coinages) against npm scope and package, crates.io,
GitHub, Homebrew, `.dev`/`.rs`/`.io` and a web collision check left six clean names. The
founder chose **Undra**.

## Decision

1. The product is **Undra**. Every identifier that carried the old name changes 1:1:
   crates `undra`, `undra-cli`, `undra-*`; macros `#[undra::…]`; C ABI `undra_*` and
   `undra.h`; Swift `UndraRuntime`; Kotlin `dev.undra.runtime` and `Java_dev_undra_…`;
   npm `@undra/runtime`, `@undra/cli`; CLI `undra`; env `UNDRA_*`; shim `undra_core`;
   playground ids `dev.undra.playground`; repository `shreypdev/undra`; site
   `https://shreypdev.github.io/undra/`.
2. The rename is performed by an idempotent script (`scripts/rename-keel-to-undra.sh`)
   so that every in-flight branch can be brought across it mechanically; golden files are
   regenerated, not edited. Immutable history is not rewritten: `.10x/reviews/` and
   ADR-018…029 keep the old identifiers, and their `keel-*` / `keel_*` names map 1:1 to
   `undra-*` / `undra_*`.
3. Registry identity: npm scope `@undra` (runtime, cli, platform packages) plus the
   unscoped `undra` reserved as a pointer to `@undra/cli`; crates.io names `undra-*`
   (publishing itself is v1.1 with its own ADR); Homebrew tap `shreypdev/homebrew-undra`,
   formula `undra`; artifacts on GitHub Releases; a checksum-verified curl installer on
   the site. A custom domain (`undra.rs` was free) and a GitHub organisation are founder
   steps that do not block anything.

## Alternatives Considered

| Alternative | Pros | Cons | Why Not |
|---|---|---|---|
| Keep Keel, publish under `@keel-native` / `keel-native-*` | No code rename | Still four other "keel"s in dev tooling, including an active Rust one; the brand is a disambiguation forever | The founder preferred a name we own outright |
| Keep Keel, ask the `keel` npm user for the scope | Keeps `@keel/*` | Unbounded wait, likely refused; solves npm only | Launch cannot depend on it |
| Other clean finalists (Umiak, Keelix, Keelin, Kelos, Subhull) | Equally available | Umiak's spelling; Keelix/Keelin keep the collision-prone root; Kelos means nothing; Subhull is descriptive, not catchy | Founder's pick after seeing the table |
| Rename later, after launch | Ship now | Every published artifact, URL and tutorial would break; the rename cost only grows | Now is the only cheap moment |

## Consequences

### Positive
* One name on every registry, URL and identifier, with no competitor for the term.
* The rename happens before anyone depends on the old name; the ABI/JNI/module changes
  are free today and would be a major version later.

### Negative
* A repository-wide change (hundreds of files, directory moves, regenerated goldens,
  the Xcode and Gradle projects, CI) that every in-flight branch must cross.
* Historical documents read with the old name; ADR-030 is the map.

### Risks
* Someone claims `@undra` or `undra` on npm, or the crate names, before the founder does
  — mitigated by making those the first two runbook steps.
* A missed identifier (a JNI symbol, a module map, an env var in CI) breaks one platform
  silently — mitigated by the rule that every suite runs green before review, and by the
  adversarial review's explicit checklist of ABI, JNI, Swift module and CI names.

## Dependencies
* Supersedes nothing; ADR-018…029 remain in force with the 1:1 name mapping.
* Constrains: the crates.io ADR (v1.1) uses the `undra-*` names; the release pipeline
  (launch-v2 §4) and the site (§2) use the URLs above.
