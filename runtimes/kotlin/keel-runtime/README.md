# Keel Kotlin runtime

The JVM/Android runtime for Keel (`docs/SPEC.md` §11). Dependencies: Kotlin stdlib and
kotlinx-coroutines, nothing else. Group `dev.keel`, package root `dev.keel.runtime`.

This first slice is the **wire layer** (`dev.keel.runtime.wire`): everything generated Kotlin code and the
transports need to speak the binary format of SPEC §3. Transports, the mirror and the core loader come in
later slices.

## Layout

```
keel-runtime/
  settings.gradle.kts        includes :runtime  (future: :android-adapters)
  build.gradle.kts           group / version, Kotlin plugin declared once
  gradle/libs.versions.toml  Kotlin 2.0.21, coroutines 1.6.4, JUnit 5.10.3
  gradlew, gradle/wrapper/   Gradle 8.14.3 wrapper
  runtime/                   the library
    src/main/kotlin/dev/keel/runtime/wire/
    src/test/kotlin/dev/keel/runtime/...
  scripts/
    test-local.sh            build + test without Gradle or JUnit
    gen-vectors.py           regenerates WireVectors.kt from contract-tests/wire-vectors.json
    local/junit-stub/        a stub @Test annotation, used only by test-local.sh
```

`:android-adapters` does not exist yet. It will hold everything that needs Android APIs: main-thread
delivery on `Dispatchers.Main`, the `Kv` / `SecureStore` / `Http` / `Connectivity` / `Lifecycle` port
adapters, and the `System.loadLibrary` glue. `:runtime` never depends on it.

## The wire layer

| Type | What it is |
|---|---|
| `KeelWriter` | Growable little-endian writer: `writeU8`..`writeU64`, signed variants, `writeF32/F64`, `writeBool`, `writeStr` (UTF-8, no temporary array), `writeBytes`, `writeRaw`, `writeLen`, `toByteArray()` |
| `KeelReader` | Cursor over a `ByteArray` window or a `ByteBuffer` (read in place, meant for JNI direct buffers). `readLen` bounds every count by the bytes left, `readStr` decodes strict UTF-8, `finish()` rejects trailing bytes |
| `WireException` | Sealed. One subclass per SPEC `WireError` variant, plus `DuplicateKey`, `NegativeDuration`, `PatchOutOfBounds` |
| `KeelCodec<T>`, `Codecs` | `bool`, `u8`..`u64`, `i8`..`i64`, `f32`, `f64`, `unit`, `string`, `bytes`, `duration`, `timestamp`, `uuid`, `handle`, and `option`, `vec`, `map`, `result` |
| `Handle`, `Timestamp`, `KeelResult` | Value types: object handle (index / generation), epoch milliseconds, `Ok` / `Err` |
| `Envelope` | The 23-byte transport frame, `Envelope.Kind` (16 kinds), `encode` / `decode` |
| `Payloads` | Typed `encode` / `decode` for every message body, including `ChangeSet.forEachEntry` |
| `KeyedPatch`, `PatchOp` | Decode / encode / apply keyed list patches |
| `Fnv` | `fnv1a32` / `fnv1a64` for type, method and schema ids |

```kotlin
val w = KeelWriter()
Codecs.vec(Codecs.string).encode(w, listOf("a", "b"))
val bytes = w.toByteArray()

val r = KeelReader(bytes)                  // or KeelReader(directByteBuffer) inside a JNI callback
val items = Codecs.vec(Codecs.string).decode(r)
r.finish()

// Host loop, core to host: no allocation per entry.
Payloads.ChangeSet.forEachEntry(changeSetBytes) { handle, signalId, op, value ->
    when (op) {
        Payloads.ChangeOp.FULL -> store.apply(handle, signalId, Codecs.vec(Todo).decode(value))
        Payloads.ChangeOp.PATCH -> store.patch(handle, signalId, KeyedPatch.decodePatch(value, Todo))
        Payloads.ChangeOp.INVALIDATED -> store.invalidate(handle, signalId)
    }
}
```

### Guarantees

* **Decoding only ever throws `WireException`.** Truncated, oversized, garbage or hostile bytes never
  produce `IndexOutOfBounds`, `NegativeArraySize` or an `OutOfMemoryError`: every length or count goes
  through `KeelReader.readLen`, which checks it against the bytes that remain before anything is allocated.
  A 5,000-iteration byte fuzz (random, structured and mutated inputs, plus every truncation and a bit flip
  at every byte of a valid corpus) enforces this, and also checks that array and direct-buffer readers agree.
* **Direct buffers are read in place.** `KeelReader(ByteBuffer)` does not copy or modify the buffer. It is only
  valid while the buffer is: decode inside the callback. `readBytes`, `readRaw` and the `Payloads` bodies are
  copies, so results outlive the buffer; strings are materialized.
* **Errors carry offsets.** `at` is relative to the start of the reader's window.
* **Canonical maps.** `Codecs.map` sorts entries by the encoded key bytes, so equal maps always encode to the
  same bytes. Note that for strings this orders by the little-endian length prefix first.

### Documented limitations

* A `Vec` (or `Map`) whose items encode to zero bytes (`Unit`, an empty record) cannot be decoded when the
  count exceeds the bytes left: `readLen` cannot tell a legitimate count from a hostile one.
* `Codecs.option` is `T?` for non-null `T`, so `Option<Option<T>>` does not type-check.
* `kotlin.time.Duration` keeps nanosecond precision only up to about 146 years, so larger durations lose their
  sub-millisecond part. Negative durations are rejected (the core's `Duration` is unsigned), as are
  `Duration.INFINITE` and anything above `Long.MAX_VALUE` nanoseconds.
* `Timestamp.toInstant` / `ofInstant` use `java.time`, which Android provides from API 26 or through
  core-library desugaring.
* Strings with unpaired surrogates cannot be encoded (`IllegalArgumentException`); they are never silently
  replaced.

## Building and testing

There are two ways to run the same tests.

### 1. Without Gradle (works in the sandbox and anywhere with a JDK)

```sh
runtimes/kotlin/keel-runtime/scripts/test-local.sh
```

It checks that `WireVectors.kt` is up to date, compiles `runtime/src/main` (explicit API mode, warnings are
errors) and `runtime/src/test` with `scripts/kotlinc.sh`, and runs every suite through `TestMain.kt`, a
reflection-free runner. It exits non-zero on any failure and prints each failing case with its stack frames.
Knobs: `KEEL_FUZZ_ITERATIONS=50000`, `KEEL_FUZZ_SEED=123`, `KEEL_WERROR=0`, `KEEL_KOTLINX_COROUTINES`,
`KEEL_KOTLIN_STDLIB`, `GRADLE_HOME`.

### 2. With Gradle and JUnit 5

```sh
cd runtimes/kotlin/keel-runtime
./gradlew :runtime:test
```

Each test class is a `Suite` (`runtime/src/test/kotlin/dev/keel/runtime/testing/Suite.kt`): its named cases run
through `runAll()`, and a single `@Test fun allCases()` delegates to it, so JUnit reports one result per class
with every failing case listed. Adding a class means extending `Suite`, adding `@Test fun allCases() =
assertPassed()`, and registering it in `TestMain.kt` (`test-local.sh` fails if a suite is not registered).

The Gradle build was written on a machine without network access to plugin repositories, so it has not been
executed end to end; the settings script, the version catalog and the root build script were confirmed to
parse. Expect at most small fixes on first run.

### Test data

`contract-tests/wire-vectors.json` is the cross-language source of truth. `scripts/gen-vectors.py` turns it into
`WireVectors.kt` (checked in, marked generated); `python3 scripts/gen-vectors.py --check` verifies it is fresh.
`WireVectorsTest` checks every vector in both directions over array, window and direct-buffer readers, and fails
on any vector `type` it does not know.
