# SDE — index

[DISCOVERED + verified by test runs, 2026-09-30]

Built and green: undra-meta, undra-wire, undra-macros, undra-signals, undra-runtime,
undra-bindgen, undra facade (+ e2e todo), TS/Kotlin/Swift runtime cores with adapters and
transports. Stubs: undra-ports, undra-query, undra-ffi, undra-transport, undra-cli,
playground, bench. Per-piece notes land here as `<slug>.md` when each piece merges.

- `playground.md` — the playground core, apps, contract scenarios (17 x 3), findings for the integrator (2026-09-30).
- `undra-cli.md` — the CLI: layout, shim/runner design, XCFramework and shell decisions, open items (2026-09-30).
- `rename-undra.md` — the rename to Undra: the script, false positives, frozen wire magic, goldens, what was verified, what the integrator owns (2026-09-30).
- `swift-error-channel.md` — ADR-032: generated Swift never traps (calls throw `E`, `CancellationError` or `UndraCallError`; commands report through `onError`), what was built, verification counts, deviations, what the integrator owns (2026-09-30).
- `wire-magic.md` — the envelope magic becomes `UNDR` (ADR-033): where the four bytes lived, the near-miss tests, the vector drift fixed and checked in CI, what the site and in-flight branches still owe (2026-09-30).
- `stress-bench.md` — the Rust half of the harsh-conditions benchmark: sustained scenarios, the `[stress]` budget table, the soak, the ffi allocation gate, numbers, deviations from the design (2026-09-30).
- `keepalive-test-clock.md` — two flaky transport tests fixed: `a_chatty_client_is_never_pinged` (the keepalive reads an injected clock; the test is a unit test with a manual clock) and the pre-upgrade byte fuzz (the connection limit it could trip, and the ephemeral ports it used up); root causes, why the test and not the server, what stays real-time (2026-09-30).
- `frame-coalesced-delivery.md` — ADR-031 in the three runtimes: merged, frame-aligned, bounded mirrors; `no_coalesce` through the schema; S18; the TS before/after numbers (2026-09-30).
- `stress-screen.md` — S1b: the playground's Timer-paced, Clock-corrected stress generator (`start`/`stop`, `generated`, `running`), the web stress screen and its drain-based stats, the extended `undra-stats` message, the landing page's "Push it" and trust-number refresh; numbers at 10k and 100k updates a second, findings (the smoke test broken since ADR-031, a stale roadmap item) (2026-09-30).
- `schema-json.md` — v1.x track C1 and C2: `undra_schema_json` carries the doc comments (docs in the binary unconditionally, with size numbers for the host dylib, Android `.so` and wasm; the hash unchanged and pinned across the library and the runner; `bindgen --docs` no longer builds the runner) and the Swift standard types (`HttpRequest`, `HttpError`, ...) are public, which deletes bindgen's declare-on-reference fallback; ADR-024 amendment, goldens, what the playground dropped (2026-10-01).
- `device-bench.md` — E1: `scripts/bench-device.sh` and the result/report tooling, the blueprint rows measured through the generated binding and the mirror on the iPhone 17 Pro simulator, an arm64 Android emulator and headless Chromium (three, two and two runs), ADR-031's Swift and Kotlin drain numbers, the `undra init` Android template on `ChoreographerFramePacer`; how to get a real-device row (one command), what was and was not exercised, findings (the generated binding is the handle call; the web in-thread row; Kotlin's list copy; TS has no `restore()`) (2026-10-01).
