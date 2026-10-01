# Architect: Swift error channel (2026-09-30)

**Problem.** Generated Swift routes every failure that is not the method's typed error, of a
typed-throws method or of a synchronous method without an error type, to `keelUnexpected` →
`fatalError`: core panics, refusals (closed object, `E_REENTRANT`, after shutdown), core cancellation
on restore, Swift task cancellation, remote disconnects. R6 broken on the lead platform; the contract
suite stepped around it (blog fact-check P1). Kotlin and TypeScript throw or reject in every case and
have no comparable trap (grepped).

**Decision (ADR-032, proposed).** Nothing generated traps. A call fails with its own `E`,
`CancellationError` (task cancelled), or a new `KeelCallError` (`cancelledByCore`, `panicked`,
`refused`, `unavailable`, `malformed`). Calls use untyped `throws` because a cancellable Swift call
does (`Task.sleep` and `Task.checkCancellation` are untyped). Synchronous `()` methods without an
error type ("commands") stay non-throwing and report to `KeelCore.report` → log + `LoadOptions.onError`
(TypeScript's hook), because SwiftUI calls them from closures that cannot throw and a refused command
changed nothing in the store. Port requirements keep `throws(E)`. The mapping is one runtime function;
`KeelCore.shared` without a core returns a shut-down placeholder instead of trapping.

**Rejected.** `throws(KeelCallError<E>)` (hides `CancellationError`, more source breaks, and Swift 6.3
still demands a catch-all), throwing commands (every `Button` and `Binding` gains `try?`), status 3 as
`CancellationError`, a debug-only trap default, returning defaults.

**Cost.** Source break in a few Swift call-site patterns, compiler-guided; nine Swift goldens and the
playground's generated Swift regenerate; Kotlin and TypeScript output unchanged. New contract steps
S05.6, S06.6, S15.9, S17.5-6 on all platforms that can express them.

**Open for the integrator.** Commands report vs throw; debug default for `onError`; ship decision 7
here or later; minor vs major version; names.

Brief: `.10x/specs/2026-09-30-swift-error-channel-impl.md`.
