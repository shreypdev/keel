# sse-chunks (the Swift SSE adapter reads chunks; ADR-047 amendment) - adversarial review

**Date:** 2026-10-02 · **Reviewer:** adversarial (`docs/AGENT_WORKFLOW.md` section 3) · **Piece:** `wt/sse-chunks` at `53a315a`
(draft PR #5; contained `main` `fd7abb4`, `ef60acf` merged in at `76d8fe8`) · **Read:** `CLAUDE.md` (R4, R6, R9, R11), AGENT_WORKFLOW 4,
the ADR-047 amendment, `.10x/decisions/sde/sse-chunks.md`, ADR-060 (on `wt/okhttp-adapters`: apps give the adapter their session for
pinning), `NSURLSession.h` (iOS 26.5 SDK), the Kotlin `SseStreamReader` and the TypeScript `FetchStream`, and the diff.
**Method:** scratch SwiftPM probes depending on the package by path, built twice (this branch, and `main` for the before), against
throwaway Node servers (HTTPS on an `openssl` certificate made for the run, HTTP Basic, a chunked body dropped mid-stream, a flood); a
raw `URLSessionDataTask` probe for suspend/resume semantics; release harnesses for the parser and the adapter; a failing test before
each fix. **Fixes:** `8ff9e0d`, `5a97a0b`, `4d82c4c`, `e3e78f1`, `e6a1e81`, `8cf1acc`, `6b75bd9`, `f54cde7`.

## Verdict

**Merge after fixes; the fixes are on the branch.** The design holds: suspend-before-parse is right (the flood stall test passed 5 of
5), the suspend/resume bookkeeping is balanced on every path I could construct, `close` and the completion after it never surface a
cancellation as `Network`, and every continuation is taken under the lock before it is resumed. The delegate question, which could
have been a High, is the opposite: the task delegate *improves* pinning. Two Highs were real: a cancelled pull hung with the request
open, and a background session aborted the app. The parser had a cross-platform parity bug and was the release bottleneck; both fixed.

## The delegate and pinning answer

`URLSessionTask.delegate` is `API_AVAILABLE(macos(12.0), ios(15.0))` (the package's floor; the runtime builds for
`arm64-apple-ios15.0-simulator`, debug and release, without a warning). The header: "Methods not implemented on this delegate will
still be forwarded to the session delegate." The stream implements only `didReceive response`, `didReceive data` and
`didCompleteWithError`. Probe, same delegates, both adapters, HTTPS with a self-signed certificate and HTTP Basic:

| Session delegate implements | `main` (`bytes(for:)`) | this branch (task delegate) |
|---|---|---|
| session-level challenge only (the usual pinning) | **never called**; stream fails on the certificate | server trust reaches it; stream reads |
| task-level challenge only | server trust and Basic reach it | the same |
| neither | fails on the certificate (default handling) | the same |
| metrics | reach it | reach it |

So pinning in `urlSession(_:didReceive:completionHandler:)` was silently skipped by `main`'s SSE adapter, and works on this branch.
Locked by `SseSessionDelegateTests` (`5a97a0b`): a session-level pin decides the stream's trust and a pin that does not match refuses
it, a task-level delegate gets trust and Basic, both get metrics; a stream that answers challenges itself fails all three.

## Findings

| # | Sev | Finding | Status |
|---|---|---|---|
| H1 | High | **A cancelled pull hung and held the request open.** `park()` had no cancellation handler: cancelling the task iterating `stream.events` left it waiting for a chunk or `close`. `main` threw `Network("cancelled")` and cancelled the request (probe, both adapters). | **Fixed** `8ff9e0d`: the handler ends the stream the same way and cancels (and, if suspended, resumes) the task. Tests: recorded task, and a real task whose server sees the client leave; both fired the hang detector before. |
| H2 | High | **A background session aborted the app (R6).** `task.delegate = self` on a background session's task raises `NSGenericException` "Task delegate is not supported on background session task"; Swift cannot catch it. `main` streamed on one. | **Fixed** `f54cde7`: `open` refuses it, `Refused(status: nil)`, before a task exists (a background configuration is the one with an `identifier`). The test aborted the test process (signal 6) before. A behavior change for that configuration, stated in the amendment. |
| M1 | Medium | **Parser parity.** `SseParser` found `:`, the space and NUL on `Character`s, so a combining mark after one joined it into a grapheme: `data:\u{301}x` dispatched nothing, `event: \u{301}` kept the space, `id: 7\0\u{301}` was taken. Kotlin and TypeScript (UTF-16 code units) and the standard (code points) split there. Present on `main`; the piece's chunk path runs through it. | **Fixed** `e3e78f1`: lines handled as bytes. Test `testAColonSpaceOrNulBeforeACombiningMarkIsStillOne` (5 failures before). |
| M2 | Medium | **No test of the session delegate's reach** (attack 1, ADR-060's assumption). | **Fixed** `5a97a0b` (above). |
| M3 | Medium | **Suspends are counted, and an extra `resume` is not a no-op.** Raw probe: suspend x2 + resume x1 stays stalled; a `resume` of a running task then one `suspend` keeps reading (it pre-pays the suspend); `cancel` of a suspended task completes it. The stream's strict alternation was right but nothing locked it. | **Fixed** `4d82c4c`: a seeded run of chunks and pulls checks alternation, "suspended exactly when the room is full", order, and balance after `close`; a stream suspending every chunk fails it. |
| M4 | Medium | **Record honesty.** The `BENCH swift sse/...` lines did not say they are a debug build's; the record said the parser was the limit and "not in this piece". | **Fixed** `e6a1e81` (lines end `debug build`), `6b75bd9` (amendment and record). The release tables already said "throwaway harness". |
| L1 | Low | Chunks are parsed on the session's delegate queue: an app session on `.main` parses on the main thread (`main` parsed on the pump's task). | Documented on the type (`8cf1acc`); by design. |
| L2 | Low | Invalid UTF-8, pre-existing and unchanged by the piece: Kotlin and TypeScript drop the events of the chunk holding the bad byte (chunk-dependent) and fail at the byte; Swift keeps the events before the bad line and fails at its end, so a last line cut short with bad bytes is `Ended` on Swift, `Protocol` on the other two. All three: `Protocol` after zero or more earlier events. | Open; follow-up (a contract scenario for it). |
| L3 | Low | Cross-piece: ADR-060's `URLSessionWebSocketAdapter(session:)` (`wt/okhttp-adapters`, decision 8) sets `task.delegate` too, so a background session would abort there the same way. | Open; for the integrator / that piece. |

## Checked and found right

* **Backpressure.** The flood stall test (`RealtimeReviewTests`), 5 of 5. One chunk of many events: all queued, task stays suspended
  until a pull takes the queue below `room` (`testTheTaskIsSuspended…`, the seeded run). In-flight chunks after the suspend are queued,
  never re-suspended. `close` on a suspended task: `cancel` then `resume`, the waiter resumed once; the later
  `didCompleteWithError(NSURLErrorCancelled)` is ignored (`over`), so it never surfaces as `Network`.
* **Error mapping, `main` vs branch** (probes): non-2xx and 401 `Refused(status)`, wrong type `Protocol`, a chunked body whose socket is
  destroyed mid-stream `Ended` on both, cancelling `open` `Network("cancelled")`; `Last-Event-ID` resumption
  (`testSseResumeRetryAndEnd`, unchanged) passes. `URLSession.shared` works.
* **Delegate queue.** A concurrent `OperationQueue()` as the session's queue delivered 3 x 200,000 events with none out of order
  (URLSession serializes one task's callbacks); the "serial" requirement in the doc is Apple's and is kept.
* **Chunk boundaries.** BOM, CR LF, multi-byte characters and `id:` lines cut at every byte, one byte a chunk, seeded cuts: the
  author's tests, now also over the run path; `testRunsAndSingleBytesParseTheSame` (400 seeded bodies, both paths, same events,
  error and last id) catches a run that forgets the pending CR, which the boundary tests did not.

## Measurement

Debug, `swift test` on this machine (load average 7 to 28 during the review): `adapter_4096B` 2,400 to 5,000 events/s,
`adapter_512B` 22,000 to 35,000, varying with load, labelled `debug build`. Release, scratch harnesses outside the repository:
`SseParser` alone (64 KiB chunks) 4,096 B events 310 to 2,246 MB/s, 512 B 221 to 1,364, 64 B 87 to 425; the adapter end to end on
`/sse/flood`, interleaved before/after the parser change, best of three, load average 21: 4,096 B 57,000 to 67,000 then 95,000 to
117,000 events/s, 512 B 60,000 to 63,000 then 72,000 to 98,000. A before/after pair on a loaded machine, not a ceiling.

## Totals and what was not verified

`swift test` 900 tests (891 at `53a315a`, plus 9), 0 failures, three times after the merge of `ef60acf`; the Swift contract column
33 of 33 (S21, S22 n/a); the runtime for the iOS 15.0 simulator and macOS 12. **Not verified:** a run on an iOS simulator or device
(one was booted by another session and left alone), iOS 15 or 16 runtimes, macOS 15 (CI's runner is the first), a real network
(TLS here is loopback), HTTP/2 flow control under `suspend`.
