# Claims ledger: "Undra 1.0: one Rust core under native apps"

Post: `site/blog/undra-1-0/index.html`, published 2026-10-03. Piece: `launch-site`. Written against `main` `005790c` (`.10x/status.md`
checkpoint 34). Same purpose and shape as `site/blog/why-undra-is-the-default-choice/claims.md`: anyone can check every factual sentence
of the post quickly. Every number the post shows that a record can supply is a generated slot, not typed: `<!--measured:NAME-->` (filled by
`site/scripts/build-numbers.mjs` from `site/data/bench.json` and `bench/results/*.jsonl`) or `<!--trust:NAME-->` (filled by
`site/scripts/build-trust.mjs` from `site/data/tests.json` and `contract-tests/scenarios.md`), so the post moves with the records.

**Aliases.** `res` = `bench/RESULTS.md`. `adr-NNN` = `.10x/adrs/ADR-NNN-*.md`. `stat` = `.10x/status.md`. `rm` = `site/data/roadmap.json`.
`gs` = `site/docs/getting-started.html`. **Checked by**: `A` the author opened the path; `R` the author ran it (the run of 2026-10-03:
`cargo build -p undra-cli`, then `undra init myapp` and `npm run dev` in an empty directory, recorded in `.10x/decisions/sde/launch-site.md`);
`J` the author's judgement, worded as such.

| ID | Claim | Source | Checked by |
|---|---|---|---|
| L01 | Undra is a Rust framework for domain logic, reactive state, the data layer and persistence; the UI stays SwiftUI, Compose, React and React Native | `CLAUDE.md` line 3; `README.md` lines 1-30 | A |
| L02 | The macros describe the marked types as a schema; `undra bindgen` turns it into each platform's API | `CLAUDE.md` R1 (line 12); `site/docs/cli.html` `#undra-bindgen` | A |
| L03 | an `@Observable` class for SwiftUI, `StateFlow`s for Compose, hooks for React and React Native | `README.md` lines 22, 125, 136, 142-144 | A |
| L04 | A read is a property access and never calls into Rust; a write crosses once per transaction and one binary change-set patches every mirror | `CLAUDE.md` R5 (line 16); `site/blog/reads-never-cross-the-boundary/` | A |
| L05 | The home page's diagram can be tapped | `site/assets/hero.js` (`tap`), this piece | R |
| L06 | A synchronous call into the core costs 49.8 ns on the core side (slot `row-call-sync`) | `site/data/bench.json` row `call-sync` (line 8); `res` line 54 (`boundary/call_sync/add` 49.8 ns) | A |
| L07 | A change-set of 100 signals costs 2.3 µs (slot `row-changeset-100`) | `site/data/bench.json` row `changeset-100` (line 48); `res` line 39 (2.30 µs) | A |
| L08 | The reference machine | `res` line 20 (Apple M5 Pro, 18 cores) | A |
| L09 | For teams on two or more platforms that must agree on logic, state and data rules | the founder's brief for this piece; `.10x/specs/2026-10-01-v1x-default-choice-design.md` §0 | J |
| L10 | Written three times, those rules drift apart | judgement, the argument of `site/blog/why-undra-is-the-default-choice/` | J |
| L11 | It assumes a team that can carry Rust; the default-choice post weighs it | `site/blog/why-undra-is-the-default-choice/index.html` `#what-it-costs` | A |
| L12 | The core list: records, enums, typed errors, stores, signals, computeds, keyed and derived lists, objects and host callbacks, newtypes, decimals, declared generics | `rm` Shipped, "Core and bindings"; adr-039, adr-040, adr-041, adr-042, adr-058 | A |
| L13 | The data layer list: queries, mutations, staleness, retry, optimistic rollback, offline queue, persistence across schema changes, paged and lazy lists, polling | `rm` Shipped, "Data"; adr-037, adr-043, adr-049 | A |
| L14 | Ten platform services with adapters and deterministic fakes; WebSocket, SSE and SQLite; the app's own OkHttp, URLSession or fetch | `README.md` line 205; adr-047, adr-048, adr-060 | A |
| L15 | `undra dev` keeps state across a rebuild; devtools with a change-set timeline; `undra schema diff` marks changes breaking or additive | adr-053, adr-059, adr-054, adr-062; the run (`Restarted: … state kept`) | A, R |
| L16 | SwiftUI, Compose, React and React Native; built from Xcode, Gradle, Vite or Bazel | `README.md` lines 185-192; adr-038, adr-061 | A |
| L17 | iOS 15 and 16 get `ObservableObject` stores, proven by compilation and a probe on iOS 26.5, not on an iOS 15 or 16 device | `README.md` line 227; adr-045 | A |
| L18 | 35 contract scenarios, 101 of 101 cells (slots `scenarios`, `cells-of`) | `contract-tests/scenarios.md` (S01-S35, S21 and S22 TypeScript only: 33 × 3 + 2 = 101); `site/data/tests.json` line 12 | A |
| L19 | 7,414 tests (slot `tests-total`) | `site/data/tests.json` lines 5-11 (3,689 + 1,864 + 881 + 870 + 110) | A |
| L20 | An adversarial review before every feature merged | `stat` checkpoints 5-34 (one review per feature row; two small fix pieces had none, `site/blog/why-undra-is-the-default-choice/claims.md` MI03) | A |
| L21 | Hello-world web core 118.4 KB gzipped against 120 KB (slot `web-size`) | `bench/results/web-size.jsonl` line 1 (118,409 B, budget 120,000) | A |
| L22 | The JavaScript runtime up front 15.8 KB against 16 KB (slot `web-runtime-js`) | `bench/results/web-size.jsonl` line 2 (15,774 B, budget 16,000) | A |
| L23 | The Android library 898.2 KB per ABI against 1.2 MB (slot `android-size`) | `bench/results/native-size.jsonl` line 1 (898,176 B, budget 1,200,000) | A |
| L24 | The three commands, and that the installer and cargo work too | `docs/RELEASING.md` lines 8-11 | A, R (init and npm; brew needs the tag) |
| L25 | `undra init` writes a core with a to-do store, its bindings and an app per platform, downloading nothing | the run: `Created …/myapp (79 files, bindings for schema hash …)`; `gs` `#in-five-minutes` | R |
| L26 | `npm run dev` builds the core to WebAssembly and serves the app at `http://localhost:5173` | the run: `==> Building the core for web (release)`, `Local: http://localhost:5173/` | R |
| L27 | Edit `core/src/lib.rs` and save: the page is on the new core | the run: `undra: …/core/src/lib.rs changed, rebuilding the core`; `runtimes/ts/@undra/runtime/src/vite.ts` lines 18-20 (rebuild, full reload) | R |
| L28 | `undra dev` serves one core to every app at once | the run (banner: web, iOS, Android, JVM); `site/docs/cli.html` `#undra-dev` | R |
| L29 | The Swift and Kotlin runtimes are not published yet, so iOS and Android apps build against a checkout | `crates/undra-cli/src/bindgen.rs` lines 211-216 (a released project's Swift package depends on `github.com/shreypdev/undra-swift`, which does not exist on 2026-10-03: `gh repo view` says so); `crates/undra-cli/src/commands/init.rs` line 280 (`dev.undra:runtime` from Maven Central, not published: `.10x/handoff.md`, "M1 Maven publishing open"); `rm` Next | A |
| L30 | Not a UI toolkit; shared UI not planned | `rm` line 149-150 (Not planned) | A |
| L31 | A single-platform app does not need it | judgement | J |
| L32 | Every device number comes from the iPhone simulator, the Android emulator or headless Chromium | `res` line 849 ("Device numbers") and line 1025 ("Pending hardware"); `stat` checkpoint 34 ("no physical device has run any of it") | A |
| L33 | One team's work with no production users yet | `README.md` line 246 | A |
| L34 | The comparisons say where each tool is ahead | `site/blog/undra-vs-kotlin-multiplatform/`, `site/blog/undra-vs-uniffi-and-crux/` ("When Kotlin Multiplatform is the better choice", "When UniFFI is the better choice", "When Crux is the better choice") | A |

Not claimed, on purpose: any speed comparison with another tool; any date for the Swift package or Maven Central.
