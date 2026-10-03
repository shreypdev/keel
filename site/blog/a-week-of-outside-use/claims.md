# Claims ledger: "A week of outside use: seven findings, five fixes"

Post: `site/blog/a-week-of-outside-use/index.html`, published 2026-10-03. Piece: `launch-site`. Written against `main` `005790c`, whose
record of this week is `.10x/status.md` checkpoint 34. Same purpose and shape as `site/blog/why-undra-is-the-default-choice/claims.md`.
The team is not named, on purpose, and neither is anyone in it.

**Aliases.** `stat` = `.10x/status.md`. `adr-NNN` = `.10x/adrs/ADR-NNN-*.md`. `rev:x` = `.10x/reviews/2026-10-02-x-review.md`.
`sde:x` = `.10x/decisions/sde/x.md`. **Checked by**: `A` the author opened the path; `J` the author's judgement, worded as such.

| ID | Claim | Source | Checked by |
|---|---|---|---|
| W01 | A team tried Undra for a week and sent seven findings | `stat` line 576 ("A team that tried Undra for a week sent seven findings (U1–U7)") | A |
| W02 | in a proof of concept inside its own repository | adr-061 line 17 (the feedback: "The POC had to be excluded from Bazel") | A |
| W03 | Five are fixed and merged, each through a pull request with a decision record and an adversarial review | `stat` lines 576-589 (each "landed through a pull request with its adversarial review closed"; the table's Review column); records: adr-047 amendment (U1), adr-061 (U3), adr-052 amendment (U4), adr-060 (U5), adr-062 (U7) | A |
| W04 | Two were not taken on | `stat` line 591 | A |
| W05 | The table of seven findings and outcomes | `stat` lines 583-591 (U1 sse-chunks, U2 not taken, U3 bazel, U4 android-size, U5 okhttp-adapters, U6 not taken, U7 generated-weight) | A |
| W06 | The report measured the iOS SSE ceiling 2.4x below what it should be | adr-047 lines 170-171; `sde:sse-chunks` line 3 | A |
| W07 | The Swift adapter pulled `URLSession.AsyncBytes` one byte at a time and handed the parser one byte per call | adr-047 lines 171-172 (`CollectionOfOne(byte)`); `sde:sse-chunks` lines 3-4 | A |
| W08 | The Kotlin and TypeScript adapters already read chunks | adr-047 line 172 | A |
| W09 | The adapter is a data task's delegate and the parser gets whole chunks | adr-047 lines 174-176 | A |
| W10 | Backpressure by the Kotlin adapter's rule: the task is suspended while events wait, URLSession stops reading, TCP pushes back | adr-047 lines 180-183 | A |
| W11 | Release, 4 KB events: 12,600 to 84,000-99,000 a second (7.8x); 512 B events 2.1x | adr-047 lines 196-197; `sde:sse-chunks` lines 68-69 | A |
| W12 | The parser alone went from 310 to 2,246 MB/s on 4 KB events once it worked on bytes | adr-047 lines 219-221; `rev:sse-chunks` line 71 | A |
| W13 | Two High defects: a cancelled waiter hung and held the request open; a background URLSession crashed the app, now refused with a typed error | `rev:sse-chunks` lines 42-43; adr-047 lines 214-218 (`Refused(status: nil)`); `stat` line 584 | A |
| W14 | Undra's build was a command line Xcode and Gradle call; a Bazel repository had nothing to put in its graph; lint flagged the generated Kotlin | adr-061 lines 17-27 | A |
| W15 | `bazel/` adds `undra_core`, `undra_bindings` and a library rule per language, running the CLI as a pinned, hermetic, offline tool | `stat` line 586; adr-061 lines 1-10 | A |
| W16 | `examples/bazel` tests a core from Kotlin on the JVM, TypeScript on Node and, on macOS, Swift | `stat` line 586 ("Kotlin over JNI, TypeScript on wasm, Swift in process"); `README.md` lines 190-192 | A |
| W17 | ktlint from 90 findings to none | `sde:bazel` lines 37-38; `stat` line 586 | A |
| W18 | Android under Bazel is declared and not verified | `stat` line 593; `sde:bazel` "Not verified" | A |
| W19 | Nothing gated a native core's size | adr-052 lines 657-659 ("This ADR gated the web only; nothing gated an Android or an iOS core") | A |
| W20 | Release builds use a profile at `opt-level = "s"` keeping the four call-path crates at full speed | adr-052 "The profile" (the `release-mobile` block and its list: `undra-wire`, `undra-signals`, `undra-runtime`, `undra-ffi` at 3) | A |
| W21 | Three gates; 898,176 bytes for the hello world's arm64-v8a against 1.2 MB | `stat` line 587; `bench/results/native-size.jsonl` line 1 | A |
| W22 | `opt_level` in `undra.toml` chooses smaller or faster | adr-052 "The knob" | A |
| W23 | The reported core lands at about 1.42 MB with nothing changed and about 1.18 MB at `"z"` | adr-052 "Where the user's number lands" | A |
| W24 | `"z"`: a synchronous call 1.33x slower on the iPhone simulator, 1.58x on the Android emulator | adr-052 "The knob", speed table (sync call: 1.58x emulator, 1.33x simulator) | A |
| W25 | The default Android adapter used `HttpURLConnection`; the team's refresh, interceptors, tracing and pinning live in its own `OkHttpClient`; a core request carried no token, appeared in no trace, was not pinned | adr-060 lines 15-24 | A |
| W26 | `okhttp-adapters` puts Http, WebSocket and SSE over the app's client with one call | adr-060 lines 55, 64 (`installWithOkHttp`); `stat` line 585 | A |
| W27 | On iOS the app's `URLSession` serves all three | adr-060 line 7 (`URLSessionWebSocketAdapter(session:)`) and its table (`HttpAdapter(session:)`, `URLSessionSseAdapter(session:)`) | A |
| W28 | The review's High: in the old install order a queued offline request could leave through the default adapter without the token and the pin | `rev:okhttp-adapters` line 32; `stat` line 585 | A |
| W29 | Two features had produced about 1,400 lines of bindings | adr-062 line 12 (the feedback) | A |
| W30 | One feature of the playground is 130 to 250 generated lines per platform | adr-062 lines 31-32 | A |
| W31 | We kept the code and moved review to the schema | adr-062 lines 21-33 (decision) | A |
| W32 | Generated files are marked for GitHub to collapse; `undra schema diff` prints one line per change, breaking or additive | adr-062 lines 66-69 (`linguist-generated=true`); `site/docs/cli.html` `#undra-schema` | A |
| W33 | Twelve changes on a test fixture: bindings 506 lines, schema 60, the diff 12 | adr-062 lines 179-181 (after the review, the fixture grew to thirteen: 581, 77, 13, adr-062 lines 264-265) | A |
| W34 | Xcode 27: the Swift test target was reported not to build; our machines and CI run Xcode 26; it needs the crash log | `stat` line 591 | A |
| W35 | Undra has no GraphQL layer; an Apollo app keeps Apollo on the platform side | `stat` line 591 | A |
| W36 | Whether Undra should have more is a question, not a plan | `site/data/roadmap.json`, Exploring ("A GraphQL story") | A |
| W37 | Each finding taken on was a place where our tests were green and a real app was not | judgement | J |
| W38 | Left open by the reviews: OkHttp pinning end to end, Android under Bazel, a byte-reproducible build | `stat` line 593 | A |
