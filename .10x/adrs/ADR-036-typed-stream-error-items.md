# ADR-036: a stream ends with its own `E` or with a typed failure, never with a bare string

Status: **Proposed** (2026-10-01, from the v1.x gap audit `.10x/specs/2026-10-01-v1x-gaps.md`, gaps SE-1…SE-3
and TY-14; Track A, piece A4). **Changes the wire** before publication, like ADR-033: SPEC 3.7 (the
`StreamItem` flags and bodies), `undra-wire` (`StreamFlag`, `StreamItem`), `undra-runtime` (who sends what),
`undra-macros` (`impl Stream<Item = Result<T, E>>`), the three platform runtimes' stream decoders, the shared
contract vectors and two contract scenarios. **No C ABI or wasm ABI change** (`undra_stream_cb` and the
`stream` import still pass a `StreamItem` payload), **no schema change** (decision 4 records the new Rust shape
as an existing schema shape), **no generated platform code change** (the generated stream methods already
throw `E` from a stream that has one). Constitution R7 and R11: the wire changes, so it is decided here before
the code, and it is free only because nothing has been published.

## Context

SPEC 3.7: `flag u8: 0 = item, 1 = end, 2 = error`; "flag 2: error E (or String if the stream has no error
type)". The code sends flag 2 from four places (`crates/undra-runtime/src/runtime.rs`):

| Producer | Body | Stream has an `E`? |
|---|---|---|
| an async `Result<impl Stream, E>` method whose opening failed (`undra-macros` `__UndraOpening`, `object.rs:869-873` → `drive_stream`, `:2372-2375`) | encoded `E` | yes |
| restore cancelling a call whose receiver it replaced (ADR-023 §3, `abort_call`, `:1424-1428`) | `String "cancelled: …"` | either |
| shutdown answering open streams (ADR-023 §4, same function) | `String "cancelled: the runtime shut down"` | either |
| a stream task that panicked (`task_panicked`, `:1758-1764`) | `String "the stream panicked: …"` | either |

So a stream *with* an error type can receive a `String`, and the body does not say which it is. The platforms
guess:

* **Swift** decodes `E`, falls back to the string on failure, and maps a `"cancelled: "` prefix to
  `UndraCallError.cancelledByCore`, everything else to `.panicked` (`Core/CallError.swift:120-146`, `:176-185`).
  Correct by string matching; its own doc comment notes that the two encodings can overlap ("`E` wins") and that
  "a distinct wire flag for core-ended streams is a v2 item (ADR-032, Risks)" — this ADR is that item, brought
  forward while it is still free.
* **Kotlin and TypeScript** decode `E` only (`fromReply` on a flag-2 failure; Kotlin `ConnectedCore.kt:444-447`,
  golden `Errors.kt:82-84`; TS `core.ts:745-748`). A core string's `u32` length is read as the `u16` variant tag:
  usually an invalid tag, so the consumer gets `WireException`/`WireError` — outside `UndraException`/`UndraError`
  — instead of "cancelled"; in principle a wrong variant. For a stream without `E` the consumer gets a raw
  "typed error" reply exception whose string body is never read.

Separately, a stream cannot end with its domain error **mid-flight**: `impl Stream<Item = Result<T, E>>` is
E0005 (a `Result` nested in a return type), and `__UndraOpening` only maps a failed *opening*
(`object.rs:837-887`). A subscription that loses its authorisation half-way has no typed way to say so.

## Decision

1. **Flag 2 carries only the stream's own `E`.** It is sent only by a method whose schema return type is
   `Result<Stream<T>, E>`, and only with an encoded `E`. A host that receives flag 2 for a stream without an
   error type treats it as malformed.
2. **New flag 3, `failed`: the call failed, in the reply-failure vocabulary.** Body:
   ```
   status   u8      2 = panicked, 3 = cancelled by the core, 5 = refused
   message  String  the panic message, the cancellation reason ("the runtime shut down",
                    "a restore replaced the receiver"), or the refusal reason
   detail   String  the backtrace for a panic; empty otherwise
   ```
   The statuses are SPEC 3.4's, so every platform maps a flag-3 item exactly as it maps a failed reply with that
   status (Swift: `UndraCallError.panicked` / `.cancelledByCore` / `.refused`; Kotlin and TS: the same
   `UndraReplyException`/`UndraReplyError` with that status and the §3.4 body, so the generated `fromReply`
   passes it through untouched). Restore and shutdown send `status 3`; a stream panic sends `status 2` with its
   backtrace. Flag 3 needs no credit, like flags 1 and 2, and ends the stream.
3. **SPEC 3.7 becomes:** `flag u8: 0 item, 1 end, 2 error (the stream's E), 3 failed (status u8, message String,
   detail String)`. Decoders reject other flags (`InvalidTag`).
4. **Typed errors mid-stream.** The macro accepts `impl Stream<Item = Result<T, E>>` (alone, or as the `Ok` of
   `Result<…, E>` with the *same* `E`; a different `E` is E0005 with a message naming both) and records it in the
   schema as `Result<Stream<T>, E>` — the shape bindgen and the platforms already handle, so the canonical form,
   the hash rules and the generated code do not change. The dispatcher maps an `Ok(t)` item to flag 0 and an
   `Err(e)` item to flag 2 with `e`, after which the stream ends (its future is dropped). `E` must be a
   `#[undra::error]` enum, as for every `Result`.
5. **Nothing else moves.** The `StreamCredit`/`Cancel` rules, the envelope (kind 8), the header, the ABIs and
   reply status 4 (`stream_opened`) are unchanged; `undra_cancel` of a stream still ends it silently on the host's
   request (the host asked).
6. **Version.** As with ADR-033, nothing is published and every peer is in this repository, so the envelope
   `version` stays 1 and the change lands in one piece across the core, the three runtimes and the vectors. After
   publication a change like this is a major version.

## Alternatives considered

* **A discriminator byte at the front of the flag-2 body** (`0` = `E`, `1` = string). Same information, one flag
  fewer, but it keeps two meanings under one flag and every decoder must remember to strip a byte that exists
  only for this case; a separate flag says "this is not your domain error" by itself.
* **Encode the cancellation as an `E` variant.** The core does not know the app's error enum, and a cancellation
  by the core is not a domain error (ADR-032's reasoning for `UndraCallError.cancelledByCore`).
* **End the stream with flag 1 on cancellation.** Reads as a clean end; ADR-023 §3 rejected it.
* **Reuse reply status codes as stream flags** (flag 4 = cancelled, 5 = refused, …). Spreads the reply
  vocabulary over a second numbering; one `failed` flag with the §3.4 status keeps one mapping per platform.
* **Leave the platforms to guess better** (Swift's prefix match everywhere). Strings are not a protocol; a
  translated or reworded message would silently change behaviour.

## Consequences

* Kotlin and TS consumers of a stream with `E` see "cancelled by the core" and "panicked" as such, not a wire
  decode error; consumers of a stream without `E` see the same typed failure instead of an unreadable body. Swift
  drops its prefix match.
* Apps can end a stream with a typed error mid-flight (`Err(AuthExpired)` from a subscription), which the
  generated code already throws as `E`.
* Every envelope or stream payload produced before the change with flag 2 + string is misread by the new
  decoders, and the reverse; there is no compatibility path, none is needed (ADR-033's argument). A running
  `undra dev` runner or a stale browser bundle fails fast and is fixed by rebuilding.
* The architect's earlier note that A4 is the last pre-publication wire change is amended: ADR-037 changes the
  opaque `Snapshot` payload too; both land in the same wire revision.

## Implementation brief

1. `crates/undra-wire/src/payload/mod.rs:72-80`: `StreamFlag::Failed = 3`; a `StreamFailure { status:
   ReplyStatus, message: String, detail: String }` body type with `Encode`/`Decode` (status limited to 2, 3, 5;
   others are `InvalidTag`); proptest round-trips and the byte-fuzz test cover it.
2. `crates/undra-runtime/src/runtime.rs`: `abort_call` (`:1424-1428`) and `task_panicked`'s stream arm
   (`:1758-1764`) send flag 3 (`Cancelled` + reason; `Panic` + message + backtrace from the `PanicReport`);
   `drive_stream`'s `Err(bytes)` stays flag 2. `string_body` is no longer used for stream items.
3. `crates/undra-macros/src/impl_/types.rs` and `impl_/object.rs`: accept `Stream<Item = Result<T, E>>` in the
   two return shapes; map to schema `Result(Stream(T), E)`; a `__UndraTry` stream adapter turns `Err(e)` into a
   final `Err(encoded e)` item; same-`E` check with an E0005 message; UI tests for both shapes and the mismatch.
4. Swift `runtimes/swift/UndraRuntime/Sources/UndraRuntime/Core/UndraCore.swift` (stream item dispatch,
   `:854-861`) and `Core/CallError.swift` (`mapped(streamFailure:)`): flag 3 → the `UndraCallError` for its
   status; flag 2 → domain decode only (a decode failure is `.malformed`); remove the prefix match.
5. Kotlin `ConnectedCore.kt:435-447` and the TS stream path (`core.ts:745-748`, `stream.ts`): flag 3 → a reply
   exception/error with that status and the §3.4 body (`message` + `detail` for status 2; empty for 3; `message`
   for 5) so generated `fromReply` passes it through; flag 2 → `ReplyStatus.ERROR` as today. The Kotlin and TS
   payload codecs (`Payloads.kt`, `payloads.ts`) and Swift `Payloads.swift` learn flag 3.
6. Vectors: `contract-tests/wire-vectors.json` gains `stream_item_error_typed` (flag 2 + an `E`) and
   `stream_item_failed_cancelled` / `stream_item_failed_panic` (flag 3); Swift copy synced
   (`runtimes/swift/scripts/sync-vectors.sh`), Kotlin table regenerated (`gen-vectors.py`).
7. Contract scenarios: S07 gains "a typed stream cancelled by a restore ends with *cancelled by the core* on all
   three runtimes, not with `E` or a decode error"; S17 gains "a panicking stream with an error type ends with
   *panicked*"; a new step for a mid-stream `Err(e)` ending as `E`.
8. SPEC 3.7, 5.1 (shutdown's stream item), 5.9 (restore's stream item), 4.1 (stream return shapes), §17.3 notes;
   ADR-023's two sentences about "flag 2 with a String body" point here.

## Dependencies

Amends ADR-023 §3 and §4 (the item they send) and SPEC 3.7. Lands in `wt/runtime-lifecycle` with ADR-034 and
ADR-035, and in the same wire revision as ADR-037's snapshot layout.
