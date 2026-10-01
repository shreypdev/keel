# ADR-033: The envelope magic is `UNDR`

Status: Accepted (implemented on `wt/magic`). Changes the wire: SPEC 3.2 (the four magic bytes of
the transport envelope), `undra-wire` (`MAGIC`, the bad-magic message), the TypeScript, Kotlin and
Swift runtime envelope codecs, the shared vector `envelope_call` in `contract-tests/wire-vectors.json`
and its two derived copies. No ABI change, no schema change (the schema hash does not depend on the
magic), no generated platform code change (bindgen goldens are byte-identical), no change to the
header layout (still 23 bytes). Constitution R7 and R11: the wire changes, so it is decided here
before the code, and it is a breaking change that is free only because nothing has been published.

## Context

Every transport envelope (SPEC 3.2: WebSocket and Worker transports, `undra dev`) starts with four
fixed magic bytes. They were `4B 45 45 4C`, the ASCII of `KEEL`, the working name of the product.
ADR-030 renamed the product to Undra and renamed every identifier 1:1. It left these four bytes
alone on purpose: they are wire format, and a wire change needs an ADR (R7, R11). The rename record
(`.10x/decisions/sde/rename-undra.md`) listed that as the one place the old name survived, and
recommended changing it "before publication or never".

The reason it is "before publication or never": after the first release, a core and its host
bindings built at different times must interoperate (R7), so changing the magic becomes a major
version with a compatibility story for every deployed frame. Today there is nothing deployed.
Nothing has been published to any registry, no app ships on the framework, and the only peers of
a core are the runtimes in this repository, which move together in one change. This is the last
moment at which the change costs four constants per runtime and one test vector.

The new magic must be four bytes: the header layout, `HEADER_LEN = 23`, every offset after the magic
(`version` at 4, `schema` at 6, `kind` at 14, `seq` at 15, `len` at 19) and every runtime's
fixed-size header read are defined around it, and `UNDRA` is five letters.

## Decision

1. The envelope magic is **`55 4E 44 52`**, the ASCII of `UNDR` (the first four letters of the name).
   SPEC 3.2 says so, and says it is a fixed tag.
2. Where the bytes live, all changed in this piece, none of them derived from another by anything
   but the spec:
   * Rust: `undra_wire::MAGIC` (`[0x55, 0x4E, 0x44, 0x52]`), used by `Envelope::write`,
     `write_with` and `parse`;
   * TypeScript: `MAGIC` in `wire/envelope.ts`, the same bytes read as a little-endian `u32`
     (`0x52444e55`);
   * Kotlin: `Envelope.MAGIC` and its little-endian `Int` (`0x52444E55`);
   * Swift: `Envelope.magic` and the little-endian `UInt32` the writer and the decoder compare
     (`0x5244_4E55`);
   * the shared contract vector `envelope_call` in `contract-tests/wire-vectors.json`
     (`554e4452` + the unchanged remaining 19 bytes), with the Swift copy synced and the Kotlin
     table regenerated from it.
3. The bad-magic error says what the right magic is, as hex, in every runtime: `554e4452` (it said
   `4b45454c`). The typed error itself (`WireError::BadMagic`, `WireException.BadMagic`,
   `WireError.badMagic`, `bad_magic`) is unchanged.
4. The tests that need a wrong magic use near misses of the new one: one byte off (`UNDQ`, `UNDX`,
   `VNDR`), a truncated one (`UND\0`), the right letters in lower case (`undr`), zeroes. A frame that
   is a strict prefix of the magic (`UN`) is still "short", not "bad magic" (`UnexpectedEof`).
5. The fixed seeds of the codec fuzz tests (Rust `fuzz.rs`, Kotlin and Swift `FuzzTests`), which were
   the old magic read as an integer, are the new magic read as an integer. They are arbitrary; the
   fuzz inputs differ and still pass. `SeededRng::DEFAULT_SEED` (a public deterministic default of
   the test fakes) is not touched: changing it would change every default-seeded sequence for no
   wire reason.

## Alternatives considered

* **Keep `KEEL` forever, as a fossil.** Costs nothing now and nothing ever breaks. Rejected: the
  magic is the first thing anyone sees in a hex dump of a socket, a packet capture, a bug report.
  A framework whose wire starts with the name of a product that does not exist would puzzle every
  reader and invite the question "is this a fork?", and the only price of fixing it is paid once,
  today, while it is a constant edit. After publication the price is a major version.
* **A longer magic (`UNDRA`, five bytes) or a versioned one (`UNDR` + a version byte).** Rejected.
  Five bytes shifts every offset after it and the header becomes 24 bytes: every runtime's header
  constants, SPEC 3.2, the vectors, the bench and the transport frame budgets change for no gain. A
  versioned magic duplicates the `version u16` that already follows the magic; the version field is
  where wire versioning lives (an unsupported version is already a typed error), so a second place
  would only create two answers to "which version is this".
* **No magic at all.** Rejected. The magic is the cheapest check that a peer is speaking this
  protocol at all (a browser tab pointed at the dev server's port, a stray client): a four-byte
  prefix check turns "garbage decoded as a header" into a typed `BadMagic` with the bytes that were
  found. Removing it moves every such failure to a misleading version or schema error and shortens
  the header to 19 bytes, which is again an offset change in every runtime for a loss of safety.
* **Make the magic a function of the product name or the schema hash.** Rejected: the magic's job
  is to be constant. A schema-derived tag is the schema hash, which the envelope already carries.

## Consequences

* **Every envelope produced before this change is rejected** by every runtime built after it, and
  the reverse, with the typed bad-magic error (`BadMagic` / `bad_magic`), before the version or the
  schema is looked at (the decoders check the magic first; the TypeScript and Kotlin suites test
  that ordering). There is no compatibility path and
  none is needed: nothing was published, and the peers are the in-repo runtimes, which change in
  one commit. A developer's running `undra dev` runner or a browser tab holding an old bundle
  fails fast with a clear error and is fixed by rebuilding.
* **The schema hash is unaffected.** It is computed from the schema (`undra-meta`), not from the
  envelope; the playground's hash is the same before and after. Hash-checked artifacts (generated
  bindings, goldens) do not change; the generated code embeds no magic. The goldens were not
  regenerated because none contains it.
* **Persistence is unaffected.** Snapshots and the Kv store do not use the envelope (`BadMagic`
  appears in restore tests only as an example decode error).
* **Documents that spell the old bytes** are updated: SPEC 3.2 and the "bringing a branch across the
  rename" note in `docs/AGENT_WORKFLOW.md`. The historical records (`.10x/reviews/`, ADR-018 to
  ADR-030 and the rename decision record) keep their text, as ADR-030 says: they describe what was
  true then. A branch cut before this change that has its own frames (a fixture, a vector) takes the
  new magic.
* `site/` is outside this piece (the site branch crosses the rename on its own): the envelope figure
  on `site/docs/architecture.html` still prints the old letters and needs `UNDR` when the site is
  brought across the rename.
* The sync of the Swift copy of `wire-vectors.json` happens here too (it had been missing the
  `map_key_order_by_encoded_bytes` vector, so `runtimes/swift/scripts/sync-vectors.sh --check` failed),
  and the check now runs in CI (Rust job), so a vector change that is not carried to the Swift copy
  fails the build. The Kotlin table already had its `gen-vectors.py --check` in `test-local.sh`.
* After the first publication this change would have been a major version of the wire (R7). The
  next one is: a change to these four bytes now needs a new ADR and a protocol version bump.

## Dependencies

* Follows ADR-030 (the rename) and closes the one exception it recorded. Amends SPEC 3.2 only in the
  tag; supersedes nothing.
