# Claims ledger: "Fifteen levers: the JavaScript runtime at 16 KB"

Post: `site/blog/the-javascript-runtime-at-16-kb/index.html`, published 2026-10-03. Piece: `launch-site`. Written against `main` `005790c`.
Same purpose and shape as `site/blog/why-undra-is-the-default-choice/claims.md`. Every number is a byte count gzipped at zlib level 9 as
`scripts/wasm-size.sh` measures it, unless the row says otherwise.

**Aliases.** `adr` = `.10x/adrs/ADR-057-js-runtime-16kb.md`. `rev` = `.10x/reviews/2026-10-02-ts-runtime-16k-review.md`. `rec` =
`bench/results/web-size.jsonl`. `stat` = `.10x/status.md`. **Checked by**: `A` the author opened the path.

| ID | Claim | Source | Checked by |
|---|---|---|---|
| J01 | What a hello-world page loads up front of `@undra/runtime` went from 22,100 to 15,774 bytes gzipped | `adr` lines 17-18 (22,100); `rec` line 2 (`web/hello-runtime-js` 15,774); `stat` checkpoint 33 | A |
| J02 | under a 16,000-byte budget | `rec` line 2 (`budget` 16000); `adr` line 90 | A |
| J03 | Nothing was removed: what left the first chunk loads with what needs it | `adr` "Alternatives considered" (rows 1 to 12 "none of it is removed from the package"); `stat` checkpoint 33 | A |
| J04 | ADR-052 asked for 16 KB up front | `adr` line 16 | A |
| J05 | An earlier piece reached 21.2 KB and concluded 16 KB would mean removing behaviour; five pieces then added 0.9 KB; the gate stood at 22,100 with no room | `adr` lines 16-18 | A |
| J06 | A script charges each byte of the minified chunk and each bit of its gzip stream to its declaration | `adr` lines 25-29 (`scripts/web-size-attribute.mjs`) | A |
| J07 | The shares: wire 3,702; mirror 2,867; the in-process wasm host 2,521; errors and sentences 2,038; call path 1,889; streams 945; the bundler's helper and chunk table 717 | `adr` lines 33-41 | A |
| J08 | A hello page shipped code it cannot run: streams, reconnection for a core that is not remote, 20 of 24 codecs | `adr` lines 70-74 | A |
| J09 | Where the bundler emits a module is decided by re-exports, not by use | `adr` lines 75-79 | A |
| J10 | The sentences of errors were about a tenth of the chunk | `adr` lines 80-82 | A |
| J11 | Each lever landed with its measured number; gzip is not additive | `adr` lines 100, 276-277 | A |
| J12 | The lever table (rows 1 to 15 and 4b, with "after" and "saved") | `adr` lines 279-295, the implementation note's table, copied as built | A |
| J13 | Row 15 changes what is counted, not what is shipped; with the helper, 16,285 against 16,600 | `adr` lines 248-251 (the helper beside the number) and 441-442 (D6: a second gate), line 360; `rec` line 3 (`web/hello-runtime-js-with-helper` 16,285, budget 16,600) | A |
| J14 | Row 13 is the largest single lever | `adr` line 292 of the table (−1,316) | A |
| J15 | A production page carries an error's class, kind and fields; its text is `T<code>` with a link; the development build keeps the sentences | `adr` row 13 (lines 117, 291) and line 433 (the text is `T<code>: <values> — https://…/errors.html#T<code>`); `site/docs/errors.html` (the `T` anchors) | A |
| J16 | What left: stream support, the framed transports' codecs, reconnection, the observe waiters, `stats` and background runs, the codecs a schema does not name | `adr` lines 105-115 and the implementation note (rows 3, 4, 5, 6, 7, 8, 11) | A |
| J17 | Keyed-patch merging stays: read-your-writes drains are synchronous | `adr` "Levers measured and not taken" ("Keyed-patch merging loaded on the first keyed entry") | A |
| J18 | The mirror's compaction stays: its backlog bound must hold offline | `adr` same table ("The mirror's compaction on first use") | A |
| J19 | Object identity stays: every app's first screen waits for it | `adr` same table ("Object identity on the first create()") | A |
| J20 | Connectivity and Lifecycle events, about 530 bytes; needs the core to say when it listens, an ABI decision with its own record; the next lever | `adr` same table, first row ("−530 … an ABI decision, its own ADR. This is the next lever.") | A |
| J21 | The review reproduced every number to the byte | `rev` lines 45-46 ("the implementer's numbers to the byte") | A |
| J22 | Four High defects | `rev` lines 12-17, 23-26 | A |
| J23 | H1: `reclaim` was internal, the published types dropped it, generated bindings failed `tsc` against the installed package; every suite aliased the sources; a test asks the compiler about every name the generator imports | `rev` line 23 | A |
| J24 | H2: `snapshot()`/`restore()` after `await import()`, which yields when cached; a call after `restore()` reached the core first; they run at the call again | `rev` line 24; `adr` "Review" first bullet | A |
| J25 | H3: the background drain at `pagehide` needed a chunk at the moment a page cannot fetch one; it calls the core directly | `rev` line 25 | A |
| J26 | H4: the production build turned a WebSocket close reason (reaches the server) and port errors (reach the core) into codes; data is a sentence in both builds | `rev` line 26 | A |
| J27 | The fixes cost 131 bytes, to 15,811 | `rev` lines 12-13 | A |
| J28 | Smaller fixes and a later merge left the record at 15,774 | `rev` lines 48-50; `adr` lines 358-360 | A |
| J29 | An awaited call is 4% slower on Node, 313-316 ns to 325-333; `callSync` did not move | `rev` lines 56-57; `adr` lines 353-354 | A |
| J30 | The development build's first chunk is 21,151 bytes and not gated; a development page is served module by module | `adr` lines 300-303, 400 | A |
| J31 | The page that uses every feature loads 39,922 bytes, less than the 42,385 before | `rec` line 4 (`web/all-features-runtime-js` 39,922); `adr` line 299 ("42,400, which was 42,385") | A |
| J32 | All three numbers are gated in CI; a change that grows the chunk fails and names the lever left | `bench/budgets.toml` lines 912-922; `rev` line 28 (L1: the message names ADR-057's levers left) | A |
