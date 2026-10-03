# SDE: the Swift SSE adapter reads chunks, not bytes (wt/sse-chunks, 2026-10-02)

User feedback U1 measured the iOS SSE ceiling 2.4x below what it should be. `URLSessionSseStream` read the response body one byte at a
time (`URLSession.AsyncBytes`, `reader.iterator.next()` per byte, `parser.push(CollectionOfOne(byte))`). The Kotlin adapter reads
chunks into a buffer and the TypeScript one reads chunks from a `ReadableStream`. ADR-047 has a dated amendment; no public shape
changed (`URLSessionSseAdapter.init(session:)`, `SseAdapter`, `SseStream` are as they were), so no new ADR.

## What changed

| File | Change |
|---|---|
| `Adapters/URLSessionSseAdapter.swift` | `URLSessionSseStream` is a `URLSessionDataDelegate` (the task's own delegate, so the session passed in is used as before). `didReceive data` feeds each chunk to `SseParser` once and queues the events; `didReceive response` checks the head (the refusals are the old ones, moved to `URLSessionSseAdapter.refusal(of:)`); `didCompleteWithError` is the end (`ended` / `network`). `SseTaskControl` (suspend, resume, cancel; `URLSessionTask` conforms, the tests record) is the seam for the tests. Cancelling `open` cancels the request. |
| `Ports/SsePort.swift` | `SseParser.push(_:into:)` (internal): appends to the caller's array, so the events a chunk completed before a line that is not UTF-8 are delivered, then the error, as the byte reader did. The public `push` is unchanged. |
| `Ports/PulledInbox.swift`, `Adapters/PlatformDefaults.swift`, `README.md` | comments and the README line that said `URLSession.bytes(for:)`. |
| `Tests/.../SseChunkTests.swift`, `SseThroughputTests.swift` | below. |

## The design, and the three things measured on the way

* **Delegate, not `AsyncBytes` with a bulk path.** The brief allowed a proposal if it measured the same; it does not. Iterating
  `AsyncBytes` is cheap (counting bytes alone: 1,467 MB/s on 4 KB events, about 0.7 ns a byte; gathering them into 8 KiB buffers 488
  MB/s). The old loop around it cost about 19 ns a byte (51 MB/s). `AsyncBytes` offers no way to take what is already buffered, so the
  only bulk unit that keeps an event's latency is a line: a prototype feeding the parser once per line over `AsyncBytes` (inside the
  module, so the parser is specialised) reached 77,000 to 98,000 events/s on 512 B events (the delegate: 96,000 to 104,000) and 35,000
  to 43,000 events/s = 143 to 177 MB/s on 4,096 B events (the delegate: 84,000 to 99,000 = 344 to 408 MB/s). The delegate is 2.3x the
  best an `AsyncBytes` design measured on large events, and it is the Kotlin adapter's shape. The prototype was thrown away.
* **Backpressure keeps decision 3 and the Kotlin rule, with the same names** (`room`, `waiting`; `SseStreamReader` in
  `SseAdapters.kt`): the task is suspended when `waiting` (events queued, not yet taken by the binding's pump) reaches `room`
  (`initialReadAhead`, 16: the binding's window before the first pull) and resumed when a pull takes `waiting` below it. The pump
  already pulls only while the binding's buffer has room, so the queue is what "the binding's buffer has room for" shows through; the
  stream does not need the Kotlin `setRoom` call (no change to `PulledInbox`). Read-ahead is `room` plus one chunk plus the chunks in
  flight when the task was suspended.
* **The suspend must come before the parse (a first version suspended after it and failed the flood test).** With the suspend after the
  parse, `testAnSseFloodStallsTheServerWhileTheCoreDoesNotPull` failed: the server wrote 41,451 of 100,000 events and 15,867 more in the
  next half second. A trace showed chunks of 429 KB, 3.7 MB, 1 MB and 3 MB arriving after the suspend: URLSession keeps reading the
  socket while the delegate callback runs (the callback's parse took longer than the socket's), and hands over what it read in one
  piece. A profile of the process showed the NSURLSession work queue entirely in CFNetwork's `conCatData` / `dispatch_data_create_concat`.
  A standalone delegate that suspends in its first callback stops at 64 KB and 1,599 events written (suspend/resume/suspend and
  suspend-then-resume from another queue behave the same), so the semantics are right and only the order was wrong. `receive` now
  suspends first, parses, queues, and resumes unless `waiting >= room`; a `parsing` count keeps a pull from resuming the task in the
  middle of a parse. After the change the stalled flood stops at 43 KB.
* **A floor no adapter can pass: small events.** A delegate that does nothing at all receives 100,000 events of 49 bytes
  (`/sse/flood?size=32`) in 9 s (11,000 events/s) with 13 chunks, and 20,000 of 64 bytes at 32,000/s; 20,000 of 512 bytes at 115,000/s
  and 20,000 of 1 KB at 227,000/s. The profile shows `conCatData` joining the small reads of a flood into one growing buffer. A
  curl against the same server takes 86 ms for the 100,000. So the 64 B rows do not move and are not a claim; real events of a few
  hundred bytes at a rate a network allows are not in this regime.

## Measurements

Apple M5 Pro (Mac17,9, 18 cores, 48 GiB), macOS 26.5, Xcode's Swift 6.3.3, Node 24.21.0 for `contract-tests/servers/realtime-server.mjs`;
the machine was shared (load average 7 during the runs), so each figure is the best of three passes, runs interleaved before/after.

Debug, the `BENCH swift sse/...` lines of `swift test` (CI records them; `SseThroughputTests`, the adapter's stream read directly, and
the binding with `next(max: 16)`):

| Line | before | after (three runs) |
|---|---|---|
| `sse/adapter_4096B` | 967 events/s, 4.0 MB/s | 4,125 / 4,028 / 4,063 events/s, 16.6 to 17.0 MB/s |
| `sse/adapter_512B` | 6,501 events/s, 3.4 MB/s | 27,873 / 27,638 / 29,238 events/s, 14.6 to 15.5 MB/s |
| `sse/binding_512B` | 6,469 events/s | 28,598 / 25,967 / 28,465 events/s |
| `sse/adapter_64B` | 12,231 events/s | 97,911 / 106,684 / 123,311 events/s (URLSession-bound; varies with how the reads fall) |

Release (a throwaway SwiftPM executable depending on the package by path: `URLSessionSseAdapter().open` then `for try await` over
`stream.events` on `/sse/flood`, 20,000 events of 64 and 512 B and 5,000 of 4,096 B, best of three; the package's tests do not
compile in release, `ObjectsCallbacksTests` uses a debug-only hook, so this is not a repository target):

| Event size | before | after | |
|---|---|---|---|
| 4,096 B | 12,605 and 12,664 events/s (51.8, 52.1 MB/s) | 83,722 / 98,174 / 99,176 events/s (344, 404, 408 MB/s) | 7.8x |
| 512 B | 45,749 and 48,232 events/s (24.2, 25.5 MB/s) | 101,225 / 104,416 / 96,234 events/s (53.6, 55.3, 51.0 MB/s) | 2.1x |
| 64 B | 39,391 and 43,188 events/s | 36,403 / 43,790 / 45,760 events/s | unchanged |

The debug figures are the parser's (about 100 ns a byte unoptimised) as much as the adapter's, which is why release shows the larger
step on large events. The user's 2.4x is inside the range.

## Tests (R4)

* `SseChunkBoundaryTests`: the awkward body (byte order mark, CR LF, lone LF and CR, characters of 2, 3 and 4 bytes, retry, ids that
  persist, an empty `data`) as one chunk equals the hand-written events; cut at every byte, one byte per chunk, and 300 seeded
  three-way cuts parse the same; a resumed stream carries `Last-Event-ID`; bytes that are not UTF-8 end the stream after the events
  before them and cancel the task (and a chunk that arrives after is ignored); a character cut short by its line end is not UTF-8
  wherever the chunks end; ends come after the queued events; a waiting pull is answered by a chunk and by the end.
* `SseChunkBackpressureTests` (a recording `SseTaskControl`, `room: 4`): suspended with ten events waiting; still suspended at four
  waiting, resumed once at three; three chunks in flight make one suspend and one resume; a chunk that leaves room resumes at once; a
  pulling core leaves the task running (a suspend and a resume per chunk, nothing left suspended); `close` of a suspended stream is
  cancel then resume and wakes a waiting pull.
* `SseChunkWireTests` (a loopback server that writes exactly the bytes a test sends and the test waits for the delegate to have each
  before it sends the next, so no sleep sets a chunk boundary): every byte of the awkward body as its own chunk of a real task; a
  suspended real task resumes after a pull and delivers what was sent while it was suspended, in order; closing a suspended stream
  makes the server see the client leave; cancelling `open` cancels the request (`network("cancelled")`, the server sees it leave); a
  body that ends is `ended` after its events.
* No test depends on machine speed: counts and order; waits are conditions (`eventually`, 5 s) or `HangDetector`s (30 s) that end a
  wait so a regression fails by assertion instead of hanging the suite, and each asserts it did not fire. The throughput test prints
  and asserts only that every event arrived in order.
* Each of these was shown to fail against a mutant of the adapter: suspend after the parse (17 failures), never suspend (12), no
  resume on a pull (4 unit failures and the wire test), the events before a bad line dropped (1), close without the resume (1), no
  cancellation handler in `open` (3, after the hang detector fires). (An earlier hang detector that closed the stream masked the
  fault; each now asserts `fired` is false.)
* Green: `swift test` 891 tests (871 before) three times; the Swift contract column (`contract-tests/run-all.sh swift`): 33 of 33,
  S21 and S22 n/a, S23 and S24 pass; `testAnSseFloodStallsTheServerWhileTheCoreDoesNotPull`, `testSseResumeRetryAndEnd`,
  `URLSessionSseAdapterTests` and `SseBindingTests` unchanged.

## Findings and what was not verified

* CFNetwork hands the answer's head to the delegate only once the first byte of the body arrived (with `Connection: close`, with a
  length and chunked alike: a delegate probe against a server that wrote only the head saw nothing until the body's first byte). A
  server that sends its head and then waits therefore leaves `open` pending until its first event; the shared Node server writes a
  comment first (`/sse/hang`), and the scripted server of `SseChunkWireTests` does the same. This is the platform's, not new (the
  same data task sits under `AsyncBytes`).
* The delegate queue of the `session` passed to `init(session:)` must be serial (what URLSession makes of the queue it creates); the
  parser is behind a lock, so a concurrent queue would corrupt order, not memory. It is in the type's doc comment.
* Not verified: an iOS simulator or device run (the suites here are macOS; `URLSessionTask.delegate` is iOS 15 / macOS 12, the
  package's floor, and the suspend semantics are the same API on both); other OS versions than macOS 26.5 (CI's macOS 15 runner is the
  first); a real network (cellular, TLS: the figures are loopback); `AsyncBytes`' own behaviour on the same server was measured only
  for throughput, not for the kernel-buffer stall of the old flood test.
* The parser itself was then the limit on large events in release (about 400 MB/s: a byte at a time with `[UInt8].append`). The
  review added the run-based path (below).

## After the review (2026-10-02, `.10x/reviews/2026-10-02-sse-chunks-review.md`)

* A cancelled pull waited for a chunk or a close (the request stayed open); it now ends the stream with `Network("cancelled")` and
  cancels the request, as an `AsyncBytes` read did.
* The session's delegate: challenges (server trust at the session or the task level, HTTP authentication) and metrics reach it, over
  TLS (`SseSessionDelegateTests`); `bytes(for:)` had never asked a session-level challenge handler. The parse runs on the session's
  delegate queue (documented on the type).
* `URLSessionTask` counts suspends and a `resume` of a running task cancels the next `suspend` (measured), so the strict alternation
  the stream keeps is now a seeded test.
* `SseParser` splits lines on bytes (a combining mark after `:`, the space or NUL was joined to it as a `Character`, unlike Kotlin,
  TypeScript and the standard) and appends runs between line ends: release, the parser alone, 4,096 B events 310 to 2,246 MB/s; the
  adapter end to end 57,000-67,000 to 95,000-117,000 events/s (a release harness outside the repository, a machine at load average 21,
  so the figures are lower than the table above and are a before/after pair, not a ceiling).
* The `BENCH swift sse/...` lines end with the build they ran in (`debug build` under `swift test`).
