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
