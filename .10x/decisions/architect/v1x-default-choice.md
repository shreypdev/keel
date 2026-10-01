# Architect — v1.1 / v1.2 "the default choice" (2026-10-01)

* The boundary does not change shape for v1.1: new behaviour lands as typed values on the
  existing status/error channels (A2, A4), contracts grow, the C ABI stays at 19 functions.
* A4 is the last pre-publication wire change (stream error items); it lands with the same
  discipline as ADR-033.
* Persisted-state migrations (A5) version every persisted artefact (snapshot, query cache,
  offline queue) with the schema hash it was written under and a migration hook the app
  implements in Rust; nothing is discarded silently.
* Devtools reuse the devtools protocol (SPEC 5.10) and the dev server; time-travel is
  snapshot/restore over the change-set log.
* React Native is a fourth host of the same C ABI with the existing TypeScript mirror on top
  (JSI/TurboModule transport); no new wire.
* Every new port (WebSocket, Db) ships with a deterministic Rust fake and adapters per platform.
