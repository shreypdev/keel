# SDE: docs-v1x — the cookbook (H1) and the production-shaped sample (H2), 2026-10-01

Worktree `wt/docs-v1x`, from `main` `8fbb6ce`, merged with `main` `660e026` (testkit landed meanwhile; `main` is an ancestor of
HEAD). Sources of truth: SPEC §3/§8/§9/§11/§17, `docs/ERRORS.md`, `docs/DEV_LOOP.md`, `docs/TESTING.md`, ADR-037/039/047/048/049/053,
the playground core, `.10x/specs/2026-10-01-v1x-default-choice-design.md` Track H and `…-competitive-limitations.md`.
No code, ADR, SPEC or schema change in `crates/` or `runtimes/`; nothing under `.10x/status.md` or `handoff.md`.

## What was built

**H1, the cookbook: `examples/cookbook/` (package `cookbook`, a workspace member).** One module per recipe, each `#[undra::api]`
stores / `#[undra::query]` / `#[undra::mutation]` over the public API only, each with tests that run it against `TestRuntime` and
`undra::ports::fakes` (a `net::testing::App` helper: the runtime plus `fakes::install`, which is what `undra::testing::Harness` wraps).

| Module | Recipe | Tests |
|---|---|---|
| `net.rs` | where the server is, typed `NetError` (shared) | |
| `auth.rs` | `Auth` store, tokens in `SecureStore`, `authed` (a 401 refreshes once, a second request waits for the refresh in flight, a refused refresh signs out and clears tokens and the persisted cache), sign-out refused with `PendingWrites` | 7 |
| `paging.rs` | `Feed`: keyed `posts`, `DerivedList` `visible` by topic, `cursor` signal, `load_more` guarded and cancel-safe, `refresh` generations, dedupe; one test decodes the change-sets a platform receives (a page = one change-set of inserts for the list and the view) | 8 |
| `forms.rs` | `SignUp`: signal per field, `errors` and `valid` as `Computed`, pure `problems`, `submit` refuses typed (`Invalid`, `EmailTaken`, `Busy`) | 7 |
| `upload.rs` | `Uploads`: `Fs` read, parts as idempotent mutations, progress as one-row patches, `retry` resumes, server name from the `Rng` port; offline wait with one part queued | 5 |
| `offline.rs` | persisted query, `add_note` (idempotent), `create_note` (optimistic), `outbox`/`retry_stuck`/`discard_stuck`, build 2 of `Note` (`#[undra(default)]` + `ty` hook) and a mutation hook; **a write queued offline survives a killed process** (second runtime over the same fakes) and replays with the same `Idempotency-Key` | 8 |
| `realtime.rs` (feature `realtime`) | `Live`: WebSocket reconnect with `Backoff` on the `Timer` port, typed ends (401/403 stop, 1000 goodbye), accept-and-drop keeps backing off, SSE fallback every third failure resuming by last event id, `send`, `stop`, a keep-last-100 list | 9 |

35 tests by default; 44 with `realtime` (see Deviations). `cargo test -p cookbook`.

Bindings: `examples/cookbook/undra.toml` + `generated/` (schema hash `0x88127919dcf53b11`), checked with `undra bindgen -C examples/cookbook --check --docs`.
Doc comments on public items carry no Rust intra-doc links (they would leak into the Swift/Kotlin/TS docs: the playground's do).

**Platform lines, extracted and compiled.** `examples/cookbook/snippets/{swiftui,kotlin,ts}` hold the 15 snippets the pages show (5 recipes
x Swift/Kotlin/TypeScript) with `docs:begin`/`docs:end` markers; `snippets/check.sh [swift|kotlin|ts]` compiles them:

| Language | Command | Result |
|---|---|---|
| Swift | `swift build` in `snippets/swiftui` (Swift 6 language mode, over the generated package and `UndraRuntime`) | ok |
| Kotlin | `scripts/kotlinc.sh -jvm-target 11 -Werror -opt-in=dev.undra.runtime.UndraEmbeddingApi` over the runtime + generated Kotlin + `Snippets.kt` (Kotlin 2.4.20 here, 2.0.21 in CI) | ok |
| TypeScript | `tsc -p snippets/ts/tsconfig.json` (strict, `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`, `verbatimModuleSyntax`, `jsx: react-jsx`; tsc 5.9.3) | ok |

Negative control: `feed.loadMore()` changed to `loadMoar()` in each file fails each leg (Swift "no member", `tsc` TS2551, kotlinc exit 1). Kotlin
snippets are plain Kotlin over `StateFlow` (Compose needs its compiler plugin; a Compose line is `collectAsState()` on the same flow, said in the file header).

**`site/scripts/build-cookbook.mjs`** (in `build-all`, documented in `docs/SITE.md`): fills the code blocks of the cookbook pages from the
sources between `<!-- snippet:path#id lang -->` markers (regions `// docs:begin id` .. `// docs:end` in the Rust sources and the snippet files; an id
may repeat and the pieces join), so a page's code is code that compiled and ran; a missing file or id fails the build. `build-search-index.mjs` and
`build-llms.mjs` learned the `docs/<dir>/` pages (the docs order is `docs.json`'s; `docs/cookbook/index.html` is `docs/cookbook/`).

**The pages (10), `site/docs/cookbook/` + `site/docs/sample.html`**, one docs-nav group "Cookbook" (All recipes, Auth, Pagination, Forms, File upload,
Real-time, Offline-first, From Kotlin Multiplatform, From UniFFI, Sample app): each recipe is a lede, "The recipe" bullets, the Rust collapsed
(`reveal-code`), the Swift/Kotlin/TS collapsed in tabs, "What the tests prove", "Limits", the source link. Prose words (tables counted): auth 362,
pagination 351, forms 297, upload 325, real-time 372, offline 339, from-KMP 345, from-UniFFI 328, index 202, sample 322 (budget 400). Landing page
unchanged at 342 words. At 375 px (browser pane, all ten pages through iframes): `scrollWidth` 375 on every page; wide tables scroll inside `.table-wrap`
(the site's pattern). Screenshots: `…/scratchpad/cookbook-from-kmp-375.jpg` (tables) and `cookbook-auth-code-375.jpg` (the Rust, open).
The docs index got a card and a "where to start" bullet.

The migration pages define L1 side by side / L2 one feature / L3 whole domain. **The SPEC does not name the levels**: the names are the brief's and
the catalogue's UNI-B4 ("L1 adoption: pure functions, no stores"). Each step lists what it removes, by catalogue row id (KMP-1..4, 5, 9, 11, 13, 14, 15;
UNI-1, 3, 4, 5, 9, 12), "what maps to what", and what does not carry over (objects as parameters, E0064; callbacks, E0004; generics and newtypes,
E0002/E0007; shared UI; Rust as the daily language). **For the fact-check (fable):** every statement about KMP and UniFFI on those two pages is the
catalogue's, with its basis codes (RAW/DOC/DER) and its 1 October 2026 date; the pages say "as of October 2026" and link the catalogue and the two
comparison posts. UNI-1 and UNI-9 are DER (judgement), written as what the step removes, not as a quotation of UniFFI.

**H2, the sample: `examples/fieldbook/` (name kept).** Field notes with photos. `undra init fieldbook` generated the three shells and the
dev loop wiring; the to-do core and UIs were replaced.

* **Core `fieldbook-core`** (a workspace member, schema hash `0x7bfb0229a00c20ed`, bindings checked in CI): `auth.rs` (slim version of the auth recipe),
  `notes.rs` (`Notebook`: a keyed `notes` list persisted in `Kv` (durable before the UI shows it), `visible` = a `DerivedList` filtered by `Filter {query, tag}` and sorted
  pinned-first then newest, `tags` = a `Computed`, `pending` = how many writes wait (recomputed 250 ms after a write and 1 s after the connectivity port reports
  online); every change a queued idempotent `push_note`; photos in `Fs` + queued `push_photo`; `delete_remote_note`; `sync_all`; `outbox`; `Note` build 2 with
  `#[undra(default)]` + the `ty = "Note"` hook, and JSON in `Kv` that reads build 1's `text` through a serde alias), `presence.rs` (feature `presence`: members over a
  reconnecting WebSocket). 18 tests (22 with `presence`): local-first save, typed title errors, a full device keeps the note out of the list, view order and filter, edit,
  relaunch loads and skips unreadable notes and continues ids above them, **offline write -> kill -> relaunch -> replay with the same key**, photo in `Fs` and queued, delete 404,
  `sync_all`, the hook, a one-row patch for the list and the view (decoded wire), **a dev-reload snapshot/restore keeps notes, filter and ids** (what ADR-053's reload needs).
* **Web** (`web/`, Vite + React, the `undra()` plugin): the whole app with an in-page `DemoServer` behind the `Http` port (login with team code `fieldbook`, expiring access
  tokens, idempotent PUT, an offline switch that also tells the core); `npm test` = vitest, 13 tests in 3 files: the demo server (4), **the generated TypeScript bindings over
  the real wasm core** loaded in Node with memory adapters (6: sign-in, notes, view/tags, filter, photo, offline write -> waiting -> replay, sign-out refused then allowed), and
  `PreviewCore` on the testing kit (3, manual clock, scripted `FakeHttp`: token on the request, offline queue and replay with the same key, sign-out refusal). `npm run build` =
  `tsc --noEmit && vite build` (app + `stories.html`). The page was run in the browser pane: the stories page renders the three seeded notes (pinned first), the app signs in.
* **iOS** (SwiftUI) and **Android** (Compose): the same app over the generated bindings (sign-in, notes list, new note, search, tag chips, pin, delete, photo picker, the "N changes
  waiting" line), talking to `server/server.mjs` (a dependency-free Node backend with the same routes; `FIELDBOOK_SERVER` / 10.0.2.2). iOS also has two `#Preview`s over
  `UndraTestKit` (`PreviewCore`: three notes; no signal) with the `UndraTestKit` product wired into the Xcode project.
* README: what it demonstrates (table), how to run each platform, the tests, the previews, feature `presence`.

Build proofs (the shells compile; they were **not launched** on a simulator or an emulator):

| Platform | Command | Result |
|---|---|---|
| iOS | `xcodebuild -project examples/fieldbook/ios/Fieldbook.xcodeproj -scheme Fieldbook -destination 'generic/platform=iOS Simulator' build` (Run Script phase runs `undra build`), then again `-configuration Debug` after the previews were added | exit 0, `BUILD SUCCEEDED` |
| Android | `ANDROID_HOME=/opt/homebrew/share/android-commandlinetools ./gradlew :app:assembleDebug` (the `undraBuild` task builds both ABIs), before and after merging `main` | `BUILD SUCCESSFUL`, `app-debug.apk` |
| Web | `npm test`, `npm run build` (tsc + vite) | 13 passed; built |

The web core is 612 KB / 262 KB gzip (serde_json and the three modules); the CLI's "budget 120 KB (hello world)" note is informational.

**CI** (`ci.yml`, `site.yml` paths): the two crates join the wasm32 build list (and `cargo clippy/test/doc --workspace` covers them as members); the Rust job also runs
`undra bindgen --check --docs` for both, the TypeScript snippets, and Fieldbook's `npm ci && npm test && npm run build`; the `contracts` job (kotlinc) runs the Kotlin snippets; the macOS job
the Swift snippets and `xcodebuild` of Fieldbook; the Android job `assembleDebug` of Fieldbook. **These CI steps were written from the commands that ran here and have not run on GitHub.**

## Verification matrix (run once, at the end, after the merge of `main`)

| Check | Result |
|---|---|
| `cargo fmt --check` | exit 0 |
| `cargo clippy -p cookbook -p fieldbook-core --all-targets -- -D warnings`; `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test -p cookbook -p fieldbook-core` | 35 + 18 passed |
| `RUSTDOCFLAGS="-D warnings" cargo doc -p cookbook -p fieldbook-core --no-deps`; `cargo build --target wasm32-unknown-unknown` for both | clean |
| `undra bindgen -C examples/{cookbook,fieldbook} --check --docs` | up to date |
| `bash examples/cookbook/snippets/check.sh` | ok (swift kotlin ts) |
| `npm test` / `npm run build` in `examples/fieldbook/web` | 13 passed / built |
| `node site/scripts/build-all.mjs` twice | no diff |
| `check-links.mjs` | 37 pages OK (one pending note: `docs/realtime.html`) |
| `check-links.mjs --words` | landing 342 words (budget 350), unchanged |
| `sync-docs-nav.mjs --check`, `sync-chrome.mjs --check`, `decls.test.mjs` | clean, 10/10 |
| `git merge-base --is-ancestor main HEAD` | true |

## Deviations (and why)

1. **Real-time and presence compile against `wt/ports-v2`, not against this branch.** The WebSocket/Sse ports are not on `main`. `cookbook/Cargo.toml` has `realtime = []` and `fieldbook-core/Cargo.toml`
   `presence = []` (module switches only); the lines they were built and tested with are `realtime = ["undra/websocket", "undra/sse"]` and `presence = ["undra/websocket"]`. Proof: a detached worktree
   of `wt/ports-v2` at `22fbbe2` (`/Users/shrey/Desktop/src/.work/scratch-ports`, **removed at the end**) with both crates copied in, the members added and those two lines: `cargo test -p cookbook --features realtime
   -p fieldbook-core --features presence` = 44 + 22 passed; clippy `-D warnings`, `cargo doc -D warnings` and `cargo fmt --check` clean. Their pages: `docs/cookbook/realtime.html` links `../realtime.html` (ports-v2's page), listed in
   `site/data/pending.json`. **When ports lands:** flip the two feature lines, add `--features realtime` / `presence` to a CI step (and the `bindgen` of Fieldbook stays default), delete the `pending.json` entry (the checker says so), re-run `build-all`.
2. **No `Db` in Fieldbook**: same reason; notes are `Kv` JSON (one key per note), and the sample page says the `Db` port is the next step.
3. **The recipes' tests use a local `App` helper, not `undra::testing::Harness`.** The testkit landed on `main` after they were written; `Harness` is the same runtime + `fakes::install` (+ seeds), so the swap is mechanical. Fieldbook uses the kit where it is the point (web `PreviewCore` test and stories, SwiftUI `#Preview`s).
4. **No Compose `@Preview` in Fieldbook**: Android Studio's preview pane cannot load a native core (docs/TESTING.md), so it would play a `RecordedCore` recording, and none was made. The README says so.
5. **`FakeHttp` has no latency**, so "a second request meets a refresh in flight" is tested by holding the refresh flag in the test and releasing it, not by two overlapping requests.
6. **The migration hooks are tested directly**; an end-to-end old-build -> new-build restore needs two builds (the playground's S20-S22 do that). The pages say so.

## Limitations found while writing (for the integrator; none fixed here)

* `QueryClient` has no `clear()` and no observable pending count: sign-out deletes the persisted cache entries and keys cached data by user, and `Notebook.pending` is recomputed on a timer and on connectivity events. A `pending_mutations` signal and a `clear()` would delete both workarounds.
* A `Signal::set` with an equal value still sends a change-set (the paging test caught a "loading = false" repeated by a guard); recipes write only on change. Worth a line in `docs/` or a dedupe in `undra-signals`.
* Rust intra-doc links in `///` comments of public items appear verbatim in the generated Swift/Kotlin/TS docs (`[`Foo::bar`]`); the playground has the same. A `bindgen` pass that rewrites them to plain code spans would stop every app writing around it.
* The runtime pins the app while a spawned task holds a `Ctx` across `ctx.mutate(..).await` offline (ADR-034); Fieldbook's pushes upgrade a `WeakCtx` per push and so do cookbook calls, which is the documented pattern, but a queued write's future lives until the queue replays it.
* Fieldbook's iOS and Android shells were compiled, not launched; the web app was run.

## Open / for the next piece

* Flip the ports features and remove the pending entry (above); optionally move the recipes' tests to `undra::testing::Harness`.
* H4 can link the cookbook pages; `site/data/roadmap.json` and `.10x/handoff.md` are the integrator's (H1 and H2 shipped; H3 was `docs-reference`).
* A Fieldbook recording (`undra dev --record`) would give Android its Compose previews.
