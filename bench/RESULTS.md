# Keel benchmark results

Host-measured numbers for every row of the blueprint's section 14 budget table that a host can
measure, the full criterion tables behind them, and what is still waiting for devices.

* Reproduce: `cargo bench -p keel-bench` (criterion, for humans) and
  `cargo bench -p keel-ffi --bench boundary` (the C ABI).
* The CI gate is different and cheaper: `cargo test -p keel-bench --test budgets --release`
  times the same operations with plain `Instant` and fails over `bench/budgets.toml` (see
  [The CI gate](#the-ci-gate)).
* Medians are criterion's; the bracket is its 95% confidence interval on the median.

## Machine

| | |
|---|---|
| CPU | Apple M5 Pro, 18 cores (Apple silicon, arm64) |
| OS | macOS 26.5 (25F71) |
| Rust | rustc 1.98.1 (48a229cea 2026-09-01), criterion 0.5 |
| Profile | `bench` inheriting `release`: opt-level 3, `lto = "fat"`, `codegen-units = 1`, `panic = "unwind"` |
| Runtime | no `keel-core` thread unless a row says so: the host drives the executor, as the wasm shell does |
| Load | **shared**: other builds ran on this machine during the measurements (load average from 5 to several hundred), so a thread-wake-dependent number can swing; two full runs and the gate's own timings agreed within about 5% for everything single-threaded |

## Section 14 rows, measured on the host

The blueprint's targets are per device (iOS A15-class, Android mid-range 2022, Chromium). An M5 Pro
core is substantially faster than an A15 core, so **"within target" here is necessary, not sufficient**,
and a miss here is a stronger miss on the device. "Host / target" is this host's median divided by the
iOS target.

| Row | Operation measured here | Median | iOS target | Host / target | Verdict |
|---|---|---|---|---|---|
| Handle method call, primitive args and return | `dispatch/call_sync/add` | 43.9 ns | ≤ 60 ns | 0.73x | within on the host, **open for the device** (Finding 2) |
| 1 KB record, round trip (codec) | `wire/record1k/roundtrip` | 228.0 ns | ≤ 3 µs | 0.08x | within |
| 1 KB record, round trip (through a call) | `dispatch/call_sync/echo_record1k` | 139.5 ns | ≤ 3 µs | 0.05x | within |
| Change-set, 100 dirty signals (core side: write, build, deliver) | `signals/changeset_100/runtime` | 2.30 µs | ≤ 100 µs | 0.02x | within |
| Keyed patch on 10,000 items, one insert | `signals/keyed_10k/insert` | 536.16 µs | ≤ 20 µs | 26.81x | **MISS** |
| Core cold start, 100 KB snapshot restore | `snapshot/cold_start_restore_100kb` | 70.87 µs | ≤ 3 ms | 0.02x | within |
| Core cold start, including the `keel-core` thread | `snapshot/cold_start_restore_100kb_core_thread` | 79.70 µs | ≤ 3 ms | 0.03x | within |
| Web crash recovery, 1 MB state (restore) | `snapshot/restore_1mb` | 270.31 µs | ≤ 100 ms | 0.003x | within |

Two more rows have a host proxy, which is a data point and not a verdict:

| Row | Host proxy | Value | Target | Note |
|---|---|---|---|---|
| Hello-world size added to the app | `keel-ffi` cdylib on `aarch64-apple-darwin`, release, LTO fat, stripped, no app code | 734 KB (316 KB gzipped) | ≤ 900 KB (iOS, arm64) | 82% of the budget before any app code; a macOS dylib carries Mach-O overhead an iOS static link does not. The `keel-ffi` test fixture core (runtime, JNI glue and a test core) is 978 KB stripped. A real measurement needs `aarch64-apple-ios` |
| Runtime memory at idle | resident-set growth of a test process after `Runtime::new` with a `keel-core` thread | about 0.45 MB for the first runtime, about 70 KB for each further one (no core thread) | ≤ 2 MB | `ps` RSS, page-granular, so indicative only; a heap counter needs `unsafe` (R2) |

Also measured, not a row: the same handle method call through the C ABI (`keel_call_sync`,
`boundary/call_sync/add`) is 49.8 ns (79.3 ns before ADR-028), so the ABI itself adds about 6 ns (the
`Runtime::global()` lookup and the `KeelBuf` hand-off). Without the one allocation that hand-off owes, the
same call is 31.5 ns (`dispatch/call_sync_with/add`).

## Findings

### 1. A keyed patch costs O(list), not O(change): the 10,000-row insert misses its row by ~27x

The blueprint row says lists must be "O(change), not O(list)". Measured, the cost is linear in the list and
independent of the change:

| Rows | One insert | One update | One move | Per row |
|---|---|---|---|---|
| 100 | 6.28 µs | | | 63 ns |
| 1,000 | 55.74 µs | | | 56 ns |
| 10,000 | 536.16 µs | 532.32 µs | 702.95 µs | 54 ns |

The 10,000-row update, which changes one row in place, costs as much as the insert. The cause is in the
design that `StoreCell`'s own docs describe ("each commit costs O(n) to compute the patch"): at every commit
`KeyedList::diff` (`crates/keel-signals/src/store.rs`) calls `KeyedPatch::diff` (`crates/keel-wire/src/patch.rs`)
over the old and the new list, and that diff hashes every key of both lists into two `HashMap`s and builds
position tables before it looks at what changed. Measured on its own, with a plain `u64` key and `PartialEq`
(`wire/keyed_patch_10k/diff`), it takes 456.79 µs: **85% of the 536.16 µs**. The rest is the generated key
function (a thread-local `Writer` encode plus an FNV hash per row), the encoded comparison of surviving rows,
and the baseline replay. The host side is cheap by comparison: decoding the one-op patch and replaying it on a
10,000-row list is 2.57 µs (`wire/keyed_patch_10k/apply`, including the `Vec::insert` shift).

Reaching 20 µs on 10,000 rows needs the list's mutations recorded as they happen (an op log on the keyed
signal) or per-row change stamps, instead of a diff; that changes a generated/public shape and the runtime
model, so it needs an ADR first (R11). A cheaper first step, cutting the constant and not the order, is to
keep the baseline's key vector between commits and skip the old-side hashing. The budgets file guards the
current O(list) cost at 5x, with a comment saying so; tighten it when the algorithm changes.

### 2. The handle method call: the allocator was 60% of it, ADR-028 took it out, the row is within target on the host

The row was the second miss: `Runtime::call_sync` of `add(i64, i64)` was 77.8 ns when first measured (1.30x the 60 ns
target) on a core faster than the iOS device's. A sampling profile of the call (macOS `sample`, ~4,900 samples of the
loop) put **about 60% of it in the allocator**: `malloc` and `free`, plus a `mach_absolute_time` read that this OS's
`libmalloc` does inside every allocation (the top symbol, 23% of the samples). The call allocated at least three times:
the dispatcher's encoded return value (`encode_to_vec`), its `Box<dyn Any>` outcome, and the `Reply` payload `Vec`.

ADR-028 removes all three. A generated dispatcher answers a synchronous method with `rt.sync_ok(&value, Encode::encode)`
(`sync_err` for a typed error): when `call_sync` has armed the thread's reply slot, that encodes the whole `Reply` payload
(`call_id`, status, value) into one reusable thread-local buffer and returns a zero-sized outcome (boxing a zero-sized
value does not allocate); `Runtime::call_sync_with` lends the buffer to a closure. `Runtime::call_sync` copies it into the
`Vec` it returns, which is the one allocation left and the one the C ABI owes (`keel_call_sync` hands that `Vec` over as
the `KeelBuf` the caller frees). Whatever the buffer cannot serve (`keel_call`, dispatch layers, a call nested in another,
a second runtime) takes the old allocating path, byte-identical on the wire. `crates/keel-ffi/tests/sync_alloc.rs` counts
allocations with a global allocator and holds the path to exactly 0 per `call_sync_with`, 1 per `call_sync` and 1 per
`keel_call_sync`. Four smaller costs on the same path went with it: SipHash on the `u32` dispatch ids, a linear scan of
the object's method list per call, an `Arc` reference taken twice per receiver lookup, and several thread-local accesses
in the panic guard.

The parent commit was built in a second worktree and measured back to back with the new build (criterion medians, best of
three interleaved rounds each; the machine was shared, load average 5 to 20, and the single-threaded rows agreed within
about 3% between rounds when it was not saturated):

| Benchmark | Before | After |
|---|---|---|
| `dispatch/call_sync/add` (returns a `Vec`) | 73.8 ns | 43.9 ns |
| `dispatch/call_sync_with/add` (new: no allocation at all) | n/a | 31.5 ns |
| `dispatch/call_sync/function` | 65.4 ns | 35.8 ns |
| `dispatch/call_sync/echo_record1k` | 268.4 ns | 139.5 ns |
| `boundary/call_sync/add` (C ABI, one `KeelBuf`) | 79.3 ns | 49.8 ns |
| `boundary/call_sync/unknown` (status 5: formats a reason, allocates by nature) | 106.8 ns | 107.3 ns |
| `boundary/call/add` (the async entry; the slot is not armed there) | 102.3 ns | 99.2 ns |

What is left of the 31.5 ns, from a sampling profile of `call_sync_with`: about a quarter is entering and leaving the core
lock (re-entrancy check, the lock, the current-runtime scope with its `Weak::upgrade`), about a sixth the two receiver
lookups (the object table's reader lock and an `Arc` reference, once to route and once for the dispatcher), a tenth the
panic guard, and the rest payload decode, the dispatcher and the reply encode. The cheapest next steps are resolving the
receiver once per call instead of twice, and a cheaper re-entrancy check (one thread-local read instead of two);
neither changes a contract.

**The row is not closed.** 43.9 ns against 60 ns is a pass on a core faster than an A15, which is necessary and not
sufficient: the row passes on the device only if an A15 core runs this path within 1.37x of this core's time (60 / 43.9),
and this note has no A15 to measure. The verdict belongs to the device phase. `keel_call` (the async entry) still
allocates its reply; it is not on the synchronous row's path.

### 3. `Runtime::new` + drop, without `shutdown()`, leaks the runtime and two threads

Found while building the cold-start benchmark: a `Runtime` created with `Runtime::new` and merely dropped is never
freed, and its `keel-core` thread and a second thread stay alive (100 create/drop cycles: +200 threads; after
about 1,500 cycles thread creation on this host degrades from ~100 µs to several ms). `shutdown()` before the
drop fixes it (+0 threads, runtime freed). The cause is a reference cycle through the `keel-query.hydrate`
init hook: it spawns a task that holds a `Ctx` and parks on the unavailable `Kv` port, so the executor owns a
task that owns the runtime. `TestRuntime` and `keel-transport`'s server both call `shutdown()`, so nothing in
the tree trips over it, and `Runtime::shutdown` documents that surviving `Ctx`s keep a runtime alive; but an
embedder that relies on drop leaks silently. The bench harness wraps its runtimes so they are shut down.

### 4. Expensive wire types

Everything scalar, string, option, enum and record round-trips in tens to a few hundred ns, and the 1 KB record in 228.0 ns.
The two that stand out are maps: a 100-entry `HashMap<String, u32>` is 6.46 µs and `HashMap<u32, u32>` is 3.26 µs round trip, dominated by
the encode (3.30 µs and 2.48 µs): maps encode their entries sorted by the encoded key bytes, so every encode encodes each key
into a scratch buffer and sorts. `Vec<u32>` of 1,000 is 1.12 µs (0.63 ns per element to decode): it goes element by element rather than as one copy. None is
near a section 14 row; they are the first places to look if a large-collection command ever shows up in a profile.

## Full tables

### Wire: encode, decode and round trip per type

`encode` writes into a reused buffer (the codec alone); `decode` reads a fixed byte string into an owned
value; `roundtrip` is what a call argument or return value pays: `encode_to_vec` (one allocation) and
`decode_exact`.

| Type | encode | decode | round trip |
|---|---|---|---|
| `bool` | 0.95 ns | 1.00 ns | 14.7 ns |
| `u8` | 0.95 ns | 0.95 ns | 15.5 ns |
| `u32` | 0.95 ns | 0.97 ns | 15.5 ns |
| `i64` | 1.16 ns | 0.72 ns | 15.6 ns |
| `f64` | 1.96 ns | 0.74 ns | 16.2 ns |
| `string_short` | 3.05 ns | 20.5 ns | 36.1 ns |
| `string_1kb` | 11.1 ns | 49.1 ns | 79.5 ns |
| `bytes_1kb` | 11.0 ns | 28.6 ns | 56.8 ns |
| `option_some` | 1.56 ns | 0.88 ns | 16.5 ns |
| `option_none` | 1.43 ns | 1.07 ns | 15.8 ns |
| `vec_u32_1k` | 258.2 ns | 630.1 ns | 1.12 µs |
| `map100_string_u32` | 3.30 µs | 2.81 µs | 6.46 µs |
| `map100_u32_u32` | 2.48 µs | 770.4 ns | 3.26 µs |
| `duration` | 1.17 ns | 1.04 ns | 16.9 ns |
| `timestamp` | 1.14 ns | 0.77 ns | 15.5 ns |
| `uuid` | 0.96 ns | 1.33 ns | 16.2 ns |
| `record5` | 4.50 ns | 22.3 ns | 96.7 ns |
| `record1k` | 13.6 ns | 68.7 ns | 228.0 ns |
| `enum_rect` | 1.84 ns | 3.02 ns | 56.3 ns |
| `enum_label` | 4.77 ns | 19.9 ns | 53.0 ns |
| `result_ok` | 1.99 ns | 2.71 ns | 16.9 ns |
| `result_err` | 3.43 ns | 23.5 ns | 68.5 ns |

### Wire: keyed patch (keel-wire)

The patch algorithm and its host-side replay on their own, with a cheap key and `PartialEq`: what is left of the signals number above once the generated key function and the encoded comparison are taken out.

| Benchmark | Median | 95% CI |
|---|---|---|
| `wire/keyed_patch_10k/apply` | 2.57 µs | 2.51 µs .. 2.63 µs |
| `wire/keyed_patch_10k/diff` | 456.79 µs | 450.62 µs .. 464.15 µs |
| `wire/keyed_patch_10k/roundtrip` | 136.7 ns | 134.4 ns .. 139.3 ns |

### Dispatch

`Runtime::call_sync` / `Runtime::call` with a prebuilt payload and a host that only counts: `keel_call_sync` without the C ABI. `call_sync` returns the reply as a `Vec` (the one allocation the C ABI owes the host); `call_sync_with` lends the reply buffer instead and allocates nothing (ADR-028). `call_async` includes building the `Call` payload (a host must) and running the executor (`run_pending`) on this thread; there is no thread hop.

| Benchmark | Median | 95% CI |
|---|---|---|
| `dispatch/call_sync/add` | 43.9 ns | 43.8 ns .. 44.0 ns |
| `dispatch/call_sync_with/add` | 31.5 ns | 31.4 ns .. 31.6 ns |
| `dispatch/call_sync/function` | 35.8 ns | 35.6 ns .. 36.0 ns |
| `dispatch/call_sync/echo_record1k` | 139.5 ns | 139.2 ns .. 139.8 ns |
| `dispatch/call_async/ready_add` | 220.6 ns | 217.6 ns .. 224.8 ns |

### Signals and stores

`cell` is the signals crate alone (100 `Signal<u32>` attached to a `StoreCell`, one transaction, a counting sink). `runtime` is the same 100 writes as one method call on a macro-generated store through the runtime. `decode` is a host validating and walking that change-set (borrowed). Keyed rows are one call through the runtime on an observed `Signal<Vec<Item>>` with `#[keel(key = "id")]`; insert runs against a list that is restored outside the timed region.

| Benchmark | Median | 95% CI |
|---|---|---|
| `signals/changeset_100/cell` | 760.9 ns | 749.6 ns .. 771.4 ns |
| `signals/changeset_100/runtime` | 2.30 µs | 2.27 µs .. 2.33 µs |
| `signals/changeset_100/decode` | 253.9 ns | 252.6 ns .. 255.1 ns |
| `signals/observe_100_initial` | 2.82 µs | 2.80 µs .. 2.86 µs |
| `signals/keyed_10k/insert` | 536.16 µs | 532.93 µs .. 539.09 µs |
| `signals/keyed_10k/update` | 532.32 µs | 529.86 µs .. 535.14 µs |
| `signals/keyed_10k/move` | 702.95 µs | 687.26 µs .. 721.70 µs |
| `signals/keyed_1k/insert` | 55.74 µs | 55.20 µs .. 56.34 µs |
| `signals/keyed_100/insert` | 6.28 µs | 6.25 µs .. 6.32 µs |
| `signals/computed/recompute_1` | 42.1 ns | 41.8 ns .. 42.4 ns |
| `signals/computed/recompute_chain_10` | 253.1 ns | 251.0 ns .. 256.3 ns |

### Snapshot and restore

100 KB is four stores of 250 rows of 100 bytes; 1 MB is forty. `cold_start` builds a runtime and restores; the runtime it made is shut down outside the timed region.

| Benchmark | Median | 95% CI |
|---|---|---|
| `snapshot/encode_100kb` | 13.79 µs | 13.73 µs .. 13.87 µs |
| `snapshot/restore_100kb` | 26.88 µs | 26.66 µs .. 27.15 µs |
| `snapshot/restore_1mb` | 270.31 µs | 268.02 µs .. 273.25 µs |
| `snapshot/cold_start_restore_100kb` | 70.87 µs | 70.12 µs .. 71.58 µs |
| `snapshot/cold_start_restore_100kb_core_thread` | 79.70 µs | 78.65 µs .. 80.96 µs |

### C ABI (`crates/keel-ffi/benches/boundary.rs`)

`call/ready_add` is the only row that includes a real `keel-core` thread hop (spawn, wake, poll, reply on the core thread) and is the one most sensitive to machine load.

| Benchmark | Median | 95% CI |
|---|---|---|
| `boundary/call_sync/add` | 49.8 ns | 49.6 ns .. 50.0 ns |
| `boundary/call_sync/unknown` | 107.3 ns | 106.7 ns .. 108.1 ns |
| `boundary/call/add` | 99.2 ns | 98.7 ns .. 100.1 ns |
| `boundary/call/ready_add` | 5.64 µs | 5.50 µs .. 5.80 µs |
| `boundary/write_observed` | 129.7 ns | 129.3 ns .. 130.2 ns |

### Query client (`bench/benches/query.rs`)

No row of its own in section 14; kept so regressions in the hot paths are visible.

| Benchmark | Median | 95% CI |
|---|---|---|
| `query/observe_cached_and_release` | 516.2 ns | 512.0 ns .. 521.1 ns |
| `query/refetch_published_to_100_observers` | 6.90 µs | 6.82 µs .. 6.98 µs |
| `query/platform_construct_and_release` | 1.19 µs | 1.18 µs .. 1.22 µs |
| `query/platform_refetch_call` | 586.1 ns | 585.0 ns .. 587.4 ns |

## The CI gate

`bench/budgets.toml` holds a host budget for each of the 46 operations the gate runs (the wire round trips,
dispatch, signals, snapshot). Each is about **5x** what this machine measures (with a 250 ns floor and two
significant figures), which is what makes a shared CI runner pass while an operation that became several
times slower fails. The budgets guard against **regressions on a host**; they are not the section 14 device
targets, and one of them (the keyed patch) sits above its device target today, see Findings (the handle
method call was the other until ADR-028; its budget now also guards the 43.9 ns it measures, not the allocator). The test takes the best p50 of up to three attempts, runs in `--release` only (a debug build
just smoke-runs every operation, so `cargo test --workspace` stays green and fast), and supports
`KEEL_BENCH_SCALE` for a slower runner. `.github/workflows/bench.yml` runs it on every PR and on main.

## Device numbers (iOS, Android, Web)

Land with the playground phase, measured on the devices the blueprint names (iPhone with an A15, a 2022
mid-range Android phone, Chromium) from `examples/playground`. Until then:

| Row | Waiting for |
|---|---|
| Handle method call, all three platforms | the real Swift/JNI/JS crossing on device: the host number above is the core half only |
| 1 KB record round trip, all three | the same, plus the platform runtime's own encode/decode (Swift, Kotlin, TypeScript) |
| Change-set with 100 dirty signals, applied on the main thread | the platform mirror applying the change-set (`@Observable`, Compose `State`, the TS store): this host measures the core side and a borrowed decode only |
| Keyed patch on 10,000 items, all three | the list mirror applying a patch, and the fix for Finding 1 |
| Core cold start with 100 KB snapshot restore | dlopen/app launch on iOS and Android, wasm compile and instantiate on web (the web row is "after wasm compile") |
| Hello-world size added to the app | release builds for `aarch64-apple-ios`, the Android ABIs and `wasm32-unknown-unknown` (none of these targets is installed here); the host proxy above is thin against 900 KB |
| Runtime memory at idle | a device memory profile (Instruments, Android Studio); the host proxy above is an RSS delta, and an exact heap counter needs a custom global allocator, which is `unsafe` and outside `keel-ffi` (R2) |
| Incremental core rebuild in `keel dev`, 20k-line core | a 20k-line core, which the playground does not yet have |
| Web crash recovery, 1 MB | the wasm build; the restore itself is measured above |
| Comparison with UniFFI and KMP baselines | the playground phase; the blueprint publishes these per release |
