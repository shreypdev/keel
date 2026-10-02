# The schema's JSON written without serde (cold-restore regression) — adversarial review

**Date:** 2026-10-02 · **Reviewer:** adversarial (`docs/AGENT_WORKFLOW.md` section 3), did not write the code ·
**Piece:** `wt/cold-restore-regression`, the five commits `a309e9f..e061cf1` (worktree at `439f1ad`, whose merge commit
adds only `.10x/handoff.md`) · **Read:** `CLAUDE.md` (R1 to R12), the SDE record
`.10x/decisions/sde/cold-restore-regression.md`, `bench/RESULTS.md` Finding 8, `docs/SPEC.md` 2.2, 2.3 and 2.4, the whole
diff (`undra-meta`: `schema_json.rs`, `sort.rs`, `closure_json.rs`, `canonical.rs`; `undra-cli/tests/symbols.rs`),
`def.rs`, `type_ref.rs`, `closure.rs`, `canonical.rs` at `a309e9f` · **Host:** Apple M5 Pro, rustc 1.99.0, binaryen 133,
node 24.21, NDK 27.2; the machine was shared and heavily loaded all session (load average 18 to 106).

Nothing was fixed, committed or pushed. Scratch crates and a detached worktree of `a309e9f` lived under the session's
scratch directory and are removed or disposable; this file is the only change in the worktree.

## Verdict

The central claim holds, and it held under every attack I could build: **the new writer's bytes are the old bytes**.
On 820,400 generated schemas (about 80 GB of JSON compared), every Unicode scalar value in every string position, every
integer boundary, all 22 committed schema files, and the schemas of the four example cores collected from their real
registrations, `Schema::canonical_json`, `Schema::hash` and `Schema::to_json` of the branch equal those of `undra-meta`
built from `a309e9f`, and equal `serde_json` driven by std's stable sorts. The closure writer did not move either
(400,000 generated closures, 9.9 million closures derived from schemas, all identical), and a wasm32 build of the same
harness produces the same bytes as the host. No High finding.

One Medium, and it is not in the writer: **the test this piece loosened still fails on this branch** when an Android
device is online (it was, in this session). The web half was widened to half a percent; the Android half of the same
test has no margin at all, and on this branch the x86_64 library built with symbols is 16 bytes larger than the one
built without. It passes at `a309e9f`. The record's "`cargo test --workspace` ... 3,540 passed" is true only with no
device attached, and the integrator's matrix on this machine with the `undra` AVD up goes red.

Three Low findings (a compile-time guard that does not cover `TypeRef`, no committed benchmark for what was fixed, a
module doc left contradicting the test) and notes. The record's numbers reproduce where I could measure them; the
absolute cold-start times do not reproduce at this load, the ratios do.

## Findings

| # | Sev | Status | Where | Finding |
|---|---|---|---|---|
| M1 | Medium | CONFIRMED | `crates/undra-cli/tests/symbols.rs:1122-1138` (the Android half of `shipped_artefacts_do_not_grow_and_no_symbols_writes_none`) | With an Android device online the test fails on the branch: `android x86_64 grew: 2882576 bytes without symbols, 2882592 with`. Deterministic (two runs). At `a309e9f` the same test passes (x86_64 2,904,496 without, 2,904,464 with). The commit marked LOOSENED widened only the web half. Details and repro in attack 6. |
| L1 | Low | CONFIRMED (on a scratch copy) | `crates/undra-meta/src/closure_json.rs:164-181` (`type_ref`, the `_ => {}` arm), now reached from `schema_json.rs:147` | A `TypeRef` variant with a payload added later compiles after the two edits the compiler asks for and is written **without its payload**, with all 141 unit tests green: `set<u8>` and `set<string>` get the same schema hash and the exchange form cannot be read back. Before this piece the schema's `TypeRef` JSON came from `serde`'s derive and could not drift. The module doc's guard ("every definition is destructured in full") covers the `*Def` structs only. Repro in attack 1e. |
| L2 | Low | CONFIRMED | `bench/` (no row), the record's "The numbers after" table | Nothing committed measures what was fixed. `collect_schema`, the canonical JSON, `Schema::hash` and `Runtime::new` were timed by "a scratch test in `bench/tests/`, not committed", so the table cannot be re-measured from the tree, and no benchmark or budget row would notice the clone-and-serde path coming back: the two cold-start rows have budgets of 340 µs and 1 ms (3.5x and 9x above today), CI gates a commit against its parent at 1.5x, and the host baseline is not gated in CI. That is exactly how the 1.8x ramp this piece investigated got in. R9. |
| L3 | Low | CONFIRMED | `crates/undra-cli/src/builds/web.rs:13-16`; `crates/undra-cli/tests/symbols.rs:1101-1108` | The module doc still says the two web modules differ "by about 0.1% either way (measured: ... the playground -421)" while the test beside it now says 0.22% and 0.26%. The record names this and leaves it to the prod-ops owner; after the merge the tree contradicts itself. The new test comment also states a mechanism ("it folds about twenty functions fewer") that the record lists as open: what is measured is twenty functions *more*, not why. |
| N1 | Note | CONFIRMED | `schema_json.rs` tests (`arb_list`, `arb_schema`) | The generated schemas never hold a list longer than 19 items, and never more than 2 objects or 2 ports. A sort bug that needs 20 items passes the three `schema_json` tests (shown by mutation, attack 1d); it is caught by `sort.rs`'s own tests and one fixture in `canonical.rs`. The playground has 53 functions and 30 records. The generators do reach the cases the brief asked about: `TypeRef::Callback`, `Some("")`, duplicate variant indexes in lists of 14 to 19, `stale_ms: Some(0)` and `Some(u64::MAX)`. |
| N2 | Note | CONFIRMED | `closure_json::type_ref`, `schema_json::Writer` | Nesting is unbounded recursion, as it was with `serde`. It is not a regression: the new writer survives deeper types than the old path in every configuration measured (attack 4). Such a type cannot come from `Schema::from_json` (limit 128) or from a macro. |
| N3 | Note | CONFIRMED / THEORY | `schema_json.rs:57-77` (`capacity_hint`) | The hint overshoots by at most 2.53x (a schema of nothing but empty records: 96 bytes reserved for 38 written). THEORY: on wasm32 with overflow checks on, `160 * methods` overflows `usize` at 26.8 million methods; the release profiles have no overflow checks and such a schema's JSON would not fit in 4 GB anyway. |
| N4 | Note | CONFIRMED | the record, "The numbers after"; `bench/RESULTS.md` Finding 8 and its headline table | The headline rows (80.12 µs, 88.12 µs) are the best of three runs at load 2.4 to 2.8. At load 91 to 99 I measure 95.6 to 96.2 µs (1.41x the baseline) and 96.1 to 110.0 µs (1.20x to 1.37x): inside the 1.5x gate, with 6 µs of room, not 20. The parts in the table add up to more than the whole (`collect_schema` 22.5 + `Schema::hash` 45.3 = 67.8 µs against `Runtime::new` 61.1 µs; on main 110.4 against 103.8), so "twice 68 µs" for the double hash is an upper reading. |
| N5 | Note | CONFIRMED | the record's size figures | Sizes carry the checkout path (panic locations), as an earlier review found. Branch, same path as the author: hello web core 111,352 B gzipped (record 111,355), playground `--no-symbols` 1,018,255 / 408,397 (record 1,018,254 / 408,396). `a309e9f` in my longer scratch path: 116,286 (record 116,224). The difference the *symbols test* measures moves with the path too: 2,371 bytes (0.232%) for `a309e9f` in my path against the record's 2,233 (0.218%). |

## Attacks

### 1. Byte identity (R7)

**The harness.** A scratch crate depending on two copies of `undra-meta` by path: the branch (`new`) and the crate as
`git archive a309e9f` gives it (`old`, renamed `undra-meta-old`). A schema is generated with the branch's types, written
with `serde_json::to_string`, read by the old crate (the conversion is checked to be lossless by serializing the old
value again), and then seven things must agree:

* `new.canonical_json() == old.canonical_json()` (the old path: `without_docs`, `sort::by_name` / `by_index`, serde);
* `new.canonical_json()` equals an oracle written in the harness: a clone with every `docs` cleared by hand, the nine
  kinds of unordered list sorted with `slice::sort_by` / `sort_by_key`, `serde_json::to_string` of a `Canonical` struct
  of slices;
* `new.hash() == old.hash() == fnv1a64(oracle)`;
* `new.to_json() == old.to_json()` and `== serde_json::to_string(&schema)`;
* `Schema::from_json(new.to_json()) == schema`.

The generator is a seeded PRNG, not the repo's strategies. Names are drawn mostly from a pool of 20 keys chosen to
collide and to order differently under other comparisons (`""`, `"a"`, `"B"`, `"aa"`, `"ü"`, `"a\0"`, `"a\""`, `"\\"`,
DEL, U+FFFF, U+10000, `"é"`, `"e\u{301}"`, ...), otherwise from text with quotes, backslashes, every control character,
C1 controls, U+2028 / U+2029, U+FEFF, the surrogate neighbours U+D7FF / U+E000, U+10FFFF and random scalars. List
lengths are 0, short, 14 to 19, exactly 16, exactly 17, and up to 77; every sortable list is then left random, sorted,
reversed, sorted with two items swapped, or rotated by one (so both the "already in order" exit and the two sort paths
are hit with equal keys). Variant indexes repeat (`0..4`), are sequential, or any `u16`. Every optional key is set and
unset independently; `message` and `key` are `None`, `Some("")` or text; ids and milliseconds come from 0, the maxima,
powers of ten and their neighbours, powers of two.

| Run | Cases | Result |
|---|---|---|
| `fuzz`, seeds 1 to 5, 8 threads | 804,000 schemas (415,063 with the top-level records out of order; longest list 77), 79.5 GB of JSON | all identical |
| `orders`: every list length 0 to 40, keys from a pool of 1, 2 or 3, 400 schemas per length, all nine sorted lists | 16,400 schemas | all identical |
| `chars`: every Unicode scalar value, as `c`, `a{c}b` and `{c}{c}"`, in every string position of a schema (full schema for U+0000 to U+2FFF, around the surrogates, U+FF00 to U+100FF and every 251st; a one-record schema for the rest) and of a closure | 3,336,192 strings | all identical |
| `numbers`: 203 boundary values in every id and millisecond field; all 65,536 variant indexes in one enum, reversed | | all identical |
| wasm32: the same harness as a `cdylib` for `wasm32-unknown-unknown` with debug assertions and overflow checks on, run under node; a digest of everything the new crate wrote, compared with the host's for the same seeds | 10,000 schemas, 20,000 closures | no mismatch inside wasm; digests equal to the host's (`0xccee4c654014bde3`, `0x3bed96a9b8d15553`, `0x364519c8fa18d134`, `0x48f151a76a403a7f`) |

**1a. Does the harness have teeth?** Nine mutations applied to a scratch copy of the branch's crate, the harness run
against each. All nine were caught, each by the first or second sub-run: dropping `no_coalesce`; `>=` in `order_by`'s
insertion (unstable short lists: caught at `orders n=3`); the fast path letting 0x1F through; the merge taking the right
run on ties (caught at `orders n=17`); the exchange form writing empty docs; the exchange form sorting variants;
`interval_ms` written as `null`; the fast path not testing for a backslash; the "already sorted" test looking at the
first two items only (caught at `n=18`).

**1b. Serde attributes, by reading.** Every field of every `*Def` against the writer: key order is declaration order in
all thirteen structs; `transparent`, `coalesce`, `no_coalesce`, `default` (signal), `background`, `poll_in_background`
are left out when false; `interval_ms` and `infinite` when `None`; `stale_ms`, `message`, `key`, `store` are `null` when
`None` (no `skip_serializing_if` on those); a field's `default` is always written; `docs` only in the exchange form
and only when not empty; `PortKind` and `QueryKind` are the `snake_case` strings; `TypeRef` is adjacently tagged with
`of` a value, a two-element array, or a string. No difference found, which the runs above confirm.

**1c. Escaping and integers.** `serde_json` escapes `"`, `\`, and U+0000 to U+001F (`\b \t \n \f \r`, otherwise
`\u00xx` in lower-case hex) and nothing else: DEL, U+2028 and every non-ASCII character are written raw. The writer
does the same, proved over every scalar value (`chars`). The fast path tests bytes; a multi-byte character has no byte
below 0x80, so it cannot be split. `number` at 0, at every power of ten and its neighbours, and at `u64::MAX` (20
digits) equals `itoa`'s output.

**1d. The repo's own tests.** `cargo test -p undra-meta`: 141 unit tests and 3 integration suites pass. The record's
mutation claim reproduces exactly: writing a port's methods unsorted and dropping `no_coalesce` fails 11 tests, the
golden hash among them. Three more mutations against the repo's tests only: `Some("")` written as `null` is caught
(3 of 3 runs); a `u64` of 20 digits losing its first digit is caught (3 of 3); a sort that is wrong only for lists
longer than 19 passes all five `schema_json` tests 3 of 3 times and is caught by `sort::tests::order_by_*` and
`canonical::tests::equal_keys_keep_the_order_and_the_hash_they_had_before_the_schema_sort` (N1).

**1e. L1, the guard that is not there.** On a scratch copy of the branch's crate:

```rust
// type_ref.rs, after Callback(String):
    Set(Box<TypeRef>),
// closure_json.rs, the two places the compiler points at (kind_name is exhaustive):
        TypeRef::Set(_) => 27,          // and "set" appended to KINDS, now [&str; 28]
// validate.rs, the other one:
            TypeRef::Set(inner) => self.check(inner, allow, at),
```

It builds. `cargo test --lib`: `141 passed; 0 failed`. A function returning `Set<u8>`:

```
writer exchange: ...,"returns":{"kind":"set"},"is_async":false,...
serde  exchange: ...,"returns":{"kind":"set","of":{"kind":"u8"}},"is_async":false,...
canonical:       ...,"returns":{"kind":"set"},...
set<u8> and set<string> hash the same: true
from_json(to_json): Err("missing field `of` at line 1 column 154")
```

`type_ref` ends in `_ => {}`, and `arb_ty` lists the variants by hand, so neither the compiler nor the differential
tests notice. `bindgen` would fail loudly on the exchange form the first time it ran; the hash collision is silent. The
closure writer had this exposure before the piece (fingerprints); the piece extends it to the schema hash, which
`serde`'s derive used to keep right by construction. Suggested fix, not applied: name the eighteen unit variants in
that match instead of `_`, and make the generator's variant list fail to compile on a new one. The parallel branch
`wt/generics-fn-obj` adds a field to `MethodDef` and `FunctionDef`, not a `TypeRef` variant, so the record's merge note
(item 3) is right about it.

### 2. Real schemas

`Schema::from_json` by both crates, then the seven comparisons, plus every closure the schema derives:

* 19 full schema files (18 `crates/undra-bindgen/tests/golden/*/schema.json` and
  `crates/undra-cli/tests/fixtures/stores.schema.json`): hash, canonical and exchange forms identical, 238 derived
  closures identical. For example `stdlib` `0xfa67a4e57ec4340c` (24,783 B canonical), `full` `0x3b1bd1062551c158`.
* 3 canonical-form goldens (`crates/undra-cli/templates/core/schema.json`, `crates/undra-ports/tests/golden/schema.json`
  and `schema-opt-in.json`, which have no labels and are not `from_json` input): labelled, read, and written back in
  the canonical form by both crates, each reproduces its file byte for byte (12,520, 10,448 and 19,640 B).
* The four example cores, collected from their real registrations by a scratch binary linking each core and the
  branch's `undra-meta` (`collect_schema`), one dependency graph per core: playground `0xcaec1b9d8ea1f199` (73,373 B
  canonical, 110,543 B exchange), cookbook `0xd428f42565ed0361`, fieldbook `0x12d20845a8a3d7c7`, ios15-sample
  `0xa25c5f88af150795`. Each is the `schemaHash` committed in that example's generated `Ids.swift` / `Ids.kt` /
  `ids.ts`, and each schema gives the same three outputs under the old crate. (`0xfa536b9ac6f06149` in older records is
  the playground before four later merges; the current value is the one in `examples/playground/generated/`.)
* `cargo test -p undra-bindgen -p undra-ports`: every suite passes.

### 3. The closure writer

`string` and `number` are shared with `TypeClosure::canonical_json` and `StoresClosure::canonical_json`. Old against
new against `serde_json::to_string` of the closure, and the fingerprints:

* 200,000 generated type closures and 200,000 stores closures (330 MB): identical.
* 9,886,620 closures derived from the generated schemas (`store_closure`, `store_fingerprint`, `stores_closure`,
  `query_closure`, `mutation_closure`, `closure`, `closure_of_params`), and 448 from the real schemas: identical.
* Every scalar value as a closure's record, field and type name: identical.

No fingerprint moves; no snapshot will look like it needs migration because of this piece.

### 4. Panics and limits (R6)

* No input made the writer panic: the 820,400 schemas ran with debug assertions and overflow checks on, on the host and
  (10,000) on wasm32.
* Deep nesting, one `Option<Option<...<u8>>>` in a field, deepest depth that does not overflow the stack:

  | Stack | old canonical | new canonical | old exchange | new exchange | closure (old = new) |
  |---|---|---|---|---|---|
  | 8 MB (main thread) | about 87,000 | about 129,000 | about 104,000 | about 129,000 | about 129,000 |
  | 2 MB | about 21,700 | about 32,700 | about 25,900 | about 32,700 | about 32,700 |
  | 1 MB | about 11,000 | about 16,400 | about 13,200 | about 16,400 | about 16,400 |

  `serde` recursed too (and the old path cloned the type first, recursively); the writer's frame is smaller. N2.
* `capacity_hint`: 2,000,000 empty records write 82.9 MB and reserve 192 MB; 2,000,000 empty methods write 208.9 MB
  and reserve 320 MB. Proportional, never more than 2.53x. N3.

### 5. wasm32 and the MSRV

* `cargo build -p undra-meta --target wasm32-unknown-unknown`, debug and release: builds.
* The only casts in the new code are `(n % 10) as u8` on a `u64` and the pre-existing `c as u32 as usize`; nothing
  depends on `usize` being 64 bits, and the wasm32 run of attack 1 produced the host's bytes.
* MSRV 1.85: there is no 1.85 toolchain on this machine, so I used clippy's `incompatible_msrv` (it reads
  `rust-version`): `cargo clippy -p undra-meta --all-targets -- -D warnings -W clippy::incompatible_msrv` and the same
  for the wasm32 target with `-D`: clean. The lint is live: a `Vec::pop_if` (1.86) added to a scratch copy is refused
  with "current MSRV is `1.85.0` but this item is stable since `1.86.0`". `is_sorted_by_key` is 1.82.
* `cargo fmt --check -p undra-meta`: clean. `RUSTDOCFLAGS="-D warnings" cargo doc -p undra-meta --no-deps`: clean.

### 6. The loosened test

**The comment's numbers are true.** `cargo test -p undra-cli --test symbols shipped_artefacts -- --nocapture`, then
`wasm-opt --all-features --metrics` on the two `playground_core.wasm`:

| | with symbols | `--no-symbols` | difference |
|---|---|---|---|
| branch, raw bytes | 1,020,869 | 1,018,255 | 2,614 (0.257%); a quarter percent is 2,545 |
| branch, `gzip -9` | 409,163 | 408,397 | 766 (0.19%) |
| branch, functions | 3,072 | 3,052 | 20 |
| `a309e9f` (scratch worktree), raw bytes | 1,026,035 | 1,023,664 | 2,371 (0.232%) |
| `a309e9f`, functions | 3,098 | 3,079 | 19 |

So the function counts are exactly the comment's, the branch is 69 bytes over the old margin, and one function more
left over is about 0.04% (2,614 - 2,233 = 381 bytes on the record's figures). The piece touches no build step and no
symbol code (the diff is `undra-meta`, this test and records).

**Half a percent still catches what the web half exists to catch.** The module with its names kept
(`build/symbols/web/playground_core.debug.wasm`) is 1,407,675 bytes, 37.9% over the shipped one; the DWARF module is
6.4 MB. Names or line tables left in are 76 times and more than a thousand times the new margin. Neither margin
catches a stray `producers` or `sourceMappingURL` section (about a hundred bytes). What the wider margin gives up is a
real code-size cost of the names run between 2.5 KB and 5.1 KB raw; the measured value has drifted from -421 bytes
(when the margin was set) to +2,233 on main to +2,614 here, and the margin now lets it double again unseen. That is
the prod-ops owner's question, as the record says. L3 is the doc left behind.

**M1. The Android half fails.** The same test, lines 1122 to 1138, compares the two `.so` files with no margin
(`after <= before`). It runs only when `cargo-ndk`, both Android targets and an online device are present;
`adb devices` listed `emulator-5554` in this session.

```
$ cargo test -p undra-cli --test symbols shipped_artefacts -- --nocapture      # branch, twice
SIZE web wasm: before=1023346 after=1020869
SIZE web wasm gzip: before=410438 after=409163
SIZE android arm64-v8a: before=2721416 after=2721416
SIZE android x86_64: before=2882576 after=2882592
thread 'shipped_artefacts_do_not_grow_and_no_symbols_writes_none' panicked at crates/undra-cli/tests/symbols.rs:1090:9:
android x86_64 grew: 2882576 bytes without symbols, 2882592 with
test result: FAILED. 0 passed; 1 failed
```

```
$ ... the same command in a detached worktree of a309e9f
SIZE android arm64-v8a: before=2737992 after=2737976
SIZE android x86_64: before=2904496 after=2904464
SIZE ios app (linked, stripped): before=2538072 after=2538072
SIZE host dylib: before=2575680 after=2575624
test result: ok. 1 passed
```

`llvm-readelf -S` on the branch's two x86_64 libraries: every section is the same size except `.text`, 0x20470a with
symbols against 0x2046fa without (16 bytes), absorbed or not by `.relro_padding`. At `a309e9f` `.text` differs as well
(0x20970a against 0x20972a on x86_64, 0x1e2964 against 0x1e296c on arm64), in the direction the assertion allows. So
"the same code, so the same size" was never exactly true for Android: a build with debug info compiles to a few bytes
of different code, the sign is luck, and this piece's code change flipped it for one ABI. It is the web half's problem
again, in the half that has no margin.

With the Android half skipped (`ANDROID_HOME=/nonexistent`) the test passes on the branch: iOS app 2,521,544 both
ways, host dylib 2,559,120 without and 2,559,080 with, which are the record's figures.

Why Medium: `docs/AGENT_WORKFLOW.md` step 4 runs `cargo test --workspace` on this machine, and `CLAUDE.md` names the
`undra` AVD as part of the reference environment; with it running the matrix fails on a test this piece already
edited, with a reason and a comment that cover only the other half. CI does not run either half (no `wasm-opt`, no
device). The author or a fix round should either give the Android comparison a stated tolerance of its own (the
measured noise is 16 to 32 bytes of `.text`) or say why the strict form is to be kept, and the record's test tally
should say that the Android half did not run.

### 7. The record's claims

| Claim | What I measured | Verdict |
|---|---|---|
| The two cold-start rows are back inside the 1.5x gate (80.12 to 81.29 µs, 1.18x; 88.12 to 90.00 µs, 1.10x; load 2.4 to 2.8) | `UNDRA_BENCH_BASELINE=apple-m5-pro UNDRA_BENCH_FILTER=cold_start cargo test -p undra-bench --test budgets --release`, six runs at load 91 to 99: 95.62 to 96.21 µs (1.41x) and 96.12 to 110.04 µs (1.20x to 1.37x), `budgets ... ok` every time | Passes the gate. The absolute figures do not reproduce at this load (N4). |
| Main fails the gate (123.8 µs, 1.82x; 133.8 µs, 1.66x) | The same test in a worktree of `a309e9f`, six runs at load 88 to 106, three of them interleaved with the branch's: 128.17 to 161.00 µs (1.88x to 2.37x) and 150.00 to 166.12 µs (1.87x to 2.07x), `budgets ... FAILED` every time | Reproduced. Branch over main, same load: 0.60 to 0.75 on the first row (record: 0.65). |
| Canonical JSON 49.1 to 10.0 µs, `Schema::hash` 88.4 to 45.3 µs on a 40.6 KB schema; `fnv1a64` is three quarters of the hash now | The probe is not committed (L2). My own old-against-new timing in a scratch crate with the repo's release profile, best of 60 batches, load 50 to 90: playground schema (73.4 KB) canonical 120.7 to 23.9 µs, hash 176.6 to 90.6 µs, `fnv1a64` alone 68.1 µs (75%); `stdlib` golden (24.8 KB) canonical 33.5 to 7.2 µs, hash 61.6 to 30.8 µs | Consistent per KB: canonical 1.35 to 1.64 before, 0.29 to 0.33 after (record 1.21 and 0.25); hash 2.4 to 2.5 before, 1.2 after (record 2.2 and 1.1). |
| `Schema::to_json` is the same writer | Same timing: playground 72.5 to 65.3 µs, `stdlib` 23.6 to 21.7 µs | Not slower, docs with newlines included. |
| The hello web core 116,224 to 111,355 B gzipped | `bash scripts/wasm-size.sh`: branch 111,352 (gate ok; the JavaScript half measured too, 22,100 of 22,100); `a309e9f` in the scratch worktree 116,286 (its JavaScript half could not measure there, exit 2, as expected) | Reproduced within path noise: 4.9 KB (N5). |
| `serde`'s serializer and the derived `Serialize` impls are not in a shipped core | The hello core's function map (`hello_core.wasm.functions.txt`): branch 894 functions, 0 naming `serde_json::ser`, 0 naming `Serialize`; `a309e9f` 923 functions, 31 and 28 | True. |
| Playground web core `--no-symbols` 1,018,254 / 408,396; iOS app 2,521,544; host dylib 2,559,120 | 1,018,255 / 408,397; 2,521,544; 2,559,120 | Reproduced (one byte). |
| Mutation check: two mutations fail 11 tests | 11 fail, the same kind (attack 1d) | Reproduced. |
| Arithmetic: 22.7 to 40.6 KB is 1.79x; 9.2 + 5.6 + 3.1 = 17.9 KB; 2.3 to 2.6 µs per KB at every step; 61.1 µs is 41% under 103.8 | Recomputed from the table | Consistent, except that the parts exceed the whole (N4). |
| `cargo test --workspace` passes but for the two `compile_fail` tests | Not rerun in full. The suites I ran pass (`undra-meta`, `undra-bindgen`, `undra-ports`, `undra-bench --test budgets` filtered) except `undra-cli --test symbols` (M1) | True only with no Android device online. |

## Not checked

* The full `cargo test --workspace`, the full budgets test (unfiltered) and the criterion benches: the machine was at
  load 18 to 106 with other sessions building.
* The TypeScript, Kotlin and Swift runtime suites and the contract tests. Nothing they read changed (both JSON forms
  are the same bytes, shown above), but I did not run them, and neither did the author.
* A real 1.85 toolchain (not installed; the clippy lint stands in for it) and rustc 1.98.1, CI's pin: every number here
  is 1.99.0's.
* `undra bindgen --check` through the CLI against a built core. The route was covered a step lower: the collected
  schemas of the four example cores, their exchange form read back, and their hashes against the committed bindings.
* The record's bisect table and the 973-commit ramp (only its two ends were rebuilt), and the device-side cost of the
  double hash the record lists as open.
* Whether the Android size difference of M1 changes sign between clean builds of one commit (one build per commit
  here; the second branch run reused the first build).

## Re-verification (2026-10-02, after the fix round at 47b5f1a)

Same reviewer, own repros, the fix round `439f1ad..47b5f1a` (`dd90071`, `209c2ff`, `a2f75d7`, `47b5f1a`). Nothing
fixed or committed here. Load average 9 to 33 during this pass (it is given with each timing).

| # | Verdict | Evidence |
|---|---|---|
| M1 | **CLOSED** | `cargo test -p undra-cli --test symbols shipped_artefacts -- --nocapture` with `emulator-5554` listed by `adb devices`: `1 passed`, and every half compared. Web 1,020,843 with symbols against 1,018,229 without (2,614 bytes, 0.257%; 3,072 against 3,052 functions); Android arm64 2,721,448 both ways, x86_64 2,882,576 against 2,882,560 (16 larger with symbols, inside the 256); iOS app 2,521,544 both ways; host dylib 2,559,120 without, 2,559,080 with. The record now says the first round ran without a device. |
| L1 | **CLOSED** for what it reported (the schema hash and the exchange form) | The scratch-copy experiment again, on the crate at `47b5f1a`: adding `Set(Box<TypeRef>)` now stops at three places, `closure_json.rs:164` (the writer) among them. With the edit the new comment asks for (the payload written): `set<u8>` and `set<string>` hash differently, the exchange form equals `serde_json`'s and reads back. A silent wrong *schema hash* needs someone to put the new variant into the list of leaves under a comment that says not to. A silent wrong *fingerprint* is still reachable, outside the writer: R1 below. |
| L2 | **CLOSED** | In a scratch worktree of `47b5f1a` with `canonical_json` put back on the doc-stripped, sorted clone through `serde_json` (the body of `canonical_json_by_serde`), `UNDRA_BENCH_BASELINE=apple-m5-pro UNDRA_BENCH_FILTER=cold_start`, three runs interleaved with the branch at load 13 to 19: `snapshot/cold_start_schema_hash` 87.58 to 88.42 µs, **1.83x to 1.85x, `budgets ... FAILED` 3 of 3**; the branch 48.43 to 48.60 µs, 1.01x to 1.02x, `ok` 3 of 3. The record's "88 µs, 1.84x" is exact. The same ratio fails CI's gate against the parent commit. The hand-added baseline row is consistent with the file's writer (below). One overstatement in the row's comment: R2. |
| L3 | **CLOSED** | `builds/web.rs` now gives the 2026-10-02 measurement (0.22% to 0.26%, about twenty functions of 3,070) and points at the test; the test comment says "about twenty functions more" and that the reason is not known. Checked against today's artefacts: 0.257%, 3,072 and 3,052; the module with names is 1,407,649 bytes (37.9% over the shipped one, the comment's 38%), the DWARF module 6.4 MB. |
| N1 | Answer accurate | Left as is, with the reason: `equal_keyed_schema` in `canonical.rs` has 40 records, 20 enums of up to 22 variants, 18 objects and 17-item lists with repeated names, and the branch compares it with the oracle; `sort.rs`'s property tests run to 80 items. My mutation (a sort wrong only past 19 items) is caught by exactly those tests. |
| N4 | Answer accurate | The record and Finding 8 now say the figures are quiet-machine numbers and quote the review's. This pass, branch, filtered runs at load 13 to 19: 84.67 to 87.42 µs (1.24x to 1.28x) and 93.12 to 95.04 µs (1.16x to 1.18x). |
| N5 | Answer accurate | One sentence, true. |

**M1, the comment and the tolerance.** The measurements in the new comment are the ones I took (main: x86_64 32 and
arm64 16 bytes smaller with symbols; this branch: x86_64 16 larger, arm64 equal). One clause is not exact, and it
repeats a sentence of this report: "every other section has the same size". `llvm-readelf -S` on today's four
libraries, section by section: `.text` differs by 16 bytes on x86_64 and not at all on arm64; `.shstrtab` is 0x113
with symbols and 0x118 without on **both** ABIs (the stripped copy's name table is rewritten 5 bytes shorter); and
`.relro_padding` differs by 16 on x86_64, which occupies no file bytes. So the file-size difference is `.text` plus
alignment, as the comment concludes, but two other sections do differ. Note, CONFIRMED; my attack 6 text above has the
same flaw.

256 bytes still catches what that half exists to catch. The shipped copy is `llvm-strip --strip-debug
--strip-unneeded` of the library Cargo made; in the unstripped x86_64 library `.symtab` is 153,192 bytes, `.strtab`
537,007, and the smallest debug section (`.debug_abbrev`) 1,488, the others 54 KB to 6.2 MB. Losing either flag, or
any one section surviving, is at least 5.8 times the tolerance and normally thousands of times. What it no longer
catches is growth of 1 to 256 bytes, which is the class the measured noise is in (16 and 32 bytes); the pipeline adds
no small section of its own (no `.gnu_debuglink`). The tolerance is 8 times the largest difference seen.

**L2, the baseline row by hand.** `bench/src/baseline.rs` writes `[meta]` and the rows from `BTreeMap`s (name order)
with `p50_ns` at one decimal. A probe test in the scratch worktree: `Baseline::parse` of the committed file then
`to_text()` reproduces it byte for byte, before the fix round (55 bench rows, 12 meta keys) and after (56 and 13). So
the hand edit is exactly what the recorder would have written for that row, and a later recording diffs only where a
number moves; `record()` never writes the `added_cold_start_schema_hash` key, so `overlay` keeps it. Recording through
the tool instead (`UNDRA_BENCH_RECORD` with a filter) would have overwritten `rustc`, `date`, `git` and the load keys
with today's and so misdescribed the other 55 rows; by hand, with the note, is the more honest of the two. The value
is not padded: 47,869.5 ns recorded, 48.43 to 48.86 µs measured here on a busier machine. `budget_ns = 240000` is the
file's usual 5x. The workload's runtime is a zero-thread one, dropped (and so shut down) inside the setup closure;
nothing outlives the row.

### New findings

| # | Sev | Status | Where | Finding |
|---|---|---|---|---|
| R1 | Low, pre-existing (integrator; not this piece's diff) | CONFIRMED (scratch copy of `47b5f1a`) | `crates/undra-meta/src/closure.rs:496` (the collector's `_ => {}`), `crates/undra-meta/src/closure_json.rs:595-603` (the reader's `_ =>` arms) | The fix's comment says the writer "decides the schema hash and the fingerprints". The writer is guarded now; what feeds the fingerprints is not. With `Set(Box<TypeRef>)` added and every compiler-forced edit made correctly, 141 unit tests pass and: (a) the closure of a store holding `set<R>` has `"records":[]`, so its fingerprint does **not** move when `R.x` goes from `u8` to `string` (for `vec<R>` it does): a snapshot written with the old `R` would be taken as compatible; (b) `{"kind":"set","of":{"kind":"u8"}}` written by `TypeClosure::canonical_json` reads back through `TypeClosure::from_json` as `Stream(U8)`. Both are ADR-037 code this piece did not touch. The same remedy as L1 applies (name the leaves; index the reader by variant, not by "everything else"). |
| R2 | Note | CONFIRMED | `bench/common/workloads.rs`, the comment of `snapshot/cold_start_schema_hash` | "the canonical form going back through a clone of the schema **or** through `serde` ... fails the baseline gates here": the clone alone does not. With `schema_json::canonical(&self.canonicalized())` (the clone and its sorts kept, the new writer on it) the row reads 69.96 to 71.48 µs, 1.46x to 1.49x, `budgets ... ok` 3 of 3 at load 10. Clone and `serde` together fail, as L2 asked. |
| R3 | Note | CONFIRMED | `UNDRA_BENCH_BASELINE=apple-m5-pro cargo test -p undra-bench --test budgets --release`, unfiltered | At load 22 to 33 it passed 1 run of 3. The failures move between runs and are rows this piece cannot touch (`wire/bytes_1kb/roundtrip` 1.50x, `signals/changeset_100/decode` 1.72x, `stress/firehose/*` 1.57x to 1.88x, two ratios); in the worst run the two cold-start rows read 1.59x and 1.68x and the new row 66.67 µs, 1.39x. The third run passed every row (new row 48.86 µs, 1.02x; cold start 88.88 µs, 1.31x and 95.17 µs, 1.18x). Load, not the fix round: the baseline gate needs the quiet machine the record names. |
| R4 | Note | THEORY (from the per-KB timing) | the new row | The row is not normalised by schema size: it is 1.2 µs per KB of what the bench binary registers, so harness fixtures move it as they moved the cold-start rows, and about 20 KB more canonical schema (60 KB against today's 40.6) reaches the 1.5x host gate with no code change. It will need re-recording when the harness grows; that is the ramp of Finding 8 again, now visible in one row. |

### What was run in this pass

* `cargo test -p undra-meta`: 141 + 7 + 2 + 21 pass. `cargo clippy -p undra-meta -p undra-cli -p undra-bench
  --all-targets -- -D warnings`: clean. `cargo fmt --check`: clean. `undra-meta` for wasm32 and clippy's
  `incompatible_msrv`: clean.
* The differential harness against the crate at `47b5f1a`: 80,000 fuzzed schemas (990,327 derived closures, 7.9 GB),
  the 16,400 order schemas, 200,000 closures, every Unicode scalar value (3,336,192 strings), the integer boundaries,
  19 committed schema files and the playground's collected schema, the three canonical goldens: all identical to
  `a309e9f` and to `serde_json`. The digests of seeds 1 and 99 are the ones taken at `439f1ad`
  (`0xccee4c654014bde3`, `0x48f151a76a403a7f`): `dd90071` changed no byte the writer produces.
* The symbols size test with the emulator online (it reused the builds already made at this commit: 12 s), the
  budgets test filtered (6 branch runs, 6 mutated runs) and unfiltered (3 runs), the two scratch-copy experiments.

Not checked in this pass: the wasm32 run of the harness (only `type_ref`'s leaf arm changed, and the host digests are
unchanged), `scripts/wasm-size.sh`, the full workspace suite, the platform runtimes and the contract tests.
