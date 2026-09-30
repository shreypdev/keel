# KeelRuntime (Swift)

The Swift platform runtime for [Keel](../../../docs/SPEC.md). This package currently contains the **wire layer** (docs/SPEC.md section 3): the byte-exact codecs, the transport envelope, every envelope payload, keyed list patches and the FNV-1a identifiers. The C ABI header (section 6) is declared in `KeelFFI`; the transport, mirror and adapters that sit on top come in later tasks.

Swift 6 language mode, strict concurrency, iOS 17 / macOS 14, no Objective-C, no third-party dependencies. The wire layer needs no Foundation except the one file that bridges `Date` and `UUID` (`Wire/Foundation+Keel.swift`).

## Layout

```
Package.swift
Sources/
  KeelFFI/                 C target: include/keel.h, include/module.modulemap, keel_stub.c
  KeelRuntime/Wire/
    WireError.swift        the typed error enum (section 3.9 plus duplicateKey / negativeDuration)
    KeelWriter.swift       little-endian appends over [UInt8]
    KeelReader.swift       bounds-checked cursor over safe storage; every failure is a WireError
    KeelCodec.swift        the KeelCodec protocol and its conformances (ints, floats, Bool, String,
                           Optional, Array, Dictionary)
    KeelTypes.swift        KeelBytes, KeelDuration, KeelTimestamp, KeelUUID, KeelHandle,
                           KeelResult, KeelUnit
    Foundation+Keel.swift  Date and Foundation.UUID bridges (the only Foundation import)
    Envelope.swift         the 23-byte envelope and the 16 message kinds
    Payloads.swift         Call, Reply, ChangeSet, PortCall, PortReply, Cancel, StreamCredit,
                           StreamItem, Observe, Release, Event, Hello, Log, TimerFired, Snapshot
    KeyedPatch.swift       PatchOp, decodePatch, encodePatch, applyPatch
    FNV.swift              fnv1a32 / fnv1a64
Tests/KeelRuntimeTests/    XCTest suites and Resources/wire-vectors.json
```

## Running the tests

On a Mac with Xcode 16 or newer (Swift 6.0):

```sh
cd runtimes/swift/KeelRuntime
swift test
```

The tests need neither the Rust core nor an XCFramework. They cover the shared contract vectors, round trips for every codec (empty, extremes, unicode), exact encodings of every payload, malformed input for every `WireError` case, and a byte-fuzz that asserts nothing but `WireError` is ever thrown.

`Tests/KeelRuntimeTests/Resources/wire-vectors.json` is a copy of `contract-tests/wire-vectors.json` (a SwiftPM package cannot read files outside its directory). After the contract changes, refresh it and commit the result:

```sh
runtimes/swift/scripts/sync-vectors.sh          # copy
runtimes/swift/scripts/sync-vectors.sh --check  # exit 1 if the copy is stale (for CI)
```

## Using the wire layer

```swift
import KeelRuntime

// Encode.
var w = KeelWriter()
w.writeU32(7)
w.writeString("Milk")
w.writeBool(false)
let bytes = w.finish()

// Decode. Every failure is a WireError; nothing traps on hostile bytes.
var r = KeelReader(bytes)
let id = try r.readU32()
let title = try r.readString()
let done = try r.readBool()
try r.finish()                                   // WireError.trailingBytes if anything is left

// Codecs compose: [String: [Int32?]] is a KeelCodec.
let map: [String: [Int32?]] = ["b": [1, nil], "a": []]
let encoded = map.keelEncoded()                  // map keys are sorted by their encoded bytes
let decoded = try [String: [Int32?]].keelDecoded(from: encoded)

// Envelope and payloads.
var args = KeelWriter()
args.writeI32(2)
args.writeI32(3)
let call = Call(target: .objectMethod(handle: handle, methodId: fnv1a32("Calculator.add")),
                callId: 9, args: args.finishSlice())
let frame = encodeEnvelope(kind: .call, seq: 1, schemaHash: schemaHash, payload: call.encode())

let (kind, seq, hash, payload) = try decodeEnvelope(frame)
try Envelope.requireSchemaHash(hash, expected: schemaHash)   // WireError.schemaMismatch

// Change-sets are walked without copying values.
try ChangeSet.forEachEntry(slice: payload) { handle, signalId, op, value in
    // `value` is a KeelReader restricted to this entry's bytes.
    let todos = try [Todo].keelDecode(&value)
}
```

### Naming

The Swift names mirror the Rust ones (`Writer::write_u32` is `KeelWriter.writeU32`):

| Rust | Swift |
|---|---|
| `write_u8` .. `write_f64`, `write_bool`, `write_str`, `write_bytes`, `write_len`, `into_vec` | `writeU8` .. `writeF64`, `writeBool`, `writeString`, `writeBytes`, `writeLen`, `finish` |
| `read_*`, `remaining`, `finish` | `readU8` .. `readF64`, `readBool`, `readString`, `readBytes`, `readLen`, `remaining`, `finish` |
| `Encode` / `Decode` | `KeelCodec` (`keelEncode(_:)` / `keelDecode(_:)`) |

### Byte fields are slices

Payload fields that hold a nested, already-encoded value (`Call.args`, `Reply.body`, `StreamItem.body`, `PortCall.args`, `Event.payload`, `ChangeEntry.value`, `SnapshotSignal.value`) are `ArraySlice<UInt8>` that share the input buffer, so decoding a message copies nothing. Build one with `KeelWriter.finishSlice()`, read one with `KeelReader(slice:)`. `KeelReader(_: [UInt8])` and `KeelReader(slice:)` are O(1); `KeelReader(copying:)` copies, and is the one to use for the `(ptr, len)` pair of a C callback, which is only valid until the callback returns.

### Behaviours worth knowing

* `readLen()` checks that a count does not exceed the bytes that remain, so a hostile `u32` cannot make a decoder allocate gigabytes. The consequence is that an array of zero-width elements (`[KeelUnit]`) with a non-zero count is rejected. No schema type produces one.
* `readString()` validates UTF-8 exactly (no overlong forms, no surrogates, nothing above U+10FFFF), and neither the reader nor the writer normalises: `"e\u{301}"` and `"\u{E9}"` are equal Swift strings but stay distinct on the wire.
* `WireError.unexpectedEOF(needed:at:)`: `needed` is the width of the read that failed, `at` its offset. Offsets are measured from the start of the buffer the reader was created over; sub-readers keep that origin.
* Map keys are sorted by their **encoded bytes** (unsigned, lexicographic), not by value: `"b"` sorts before `"aa"` because the length prefix comes first. The decoder rejects duplicate keys but accepts any order.
* `KeelDuration` decoding rejects negative values (`WireError.negativeDuration`); encoding writes the value as is and the core rejects it. Values beyond the `Int64` nanosecond range saturate.
* `PatchOp.move(from:to:)` removes the element at `from` and re-inserts it so that it ends up at index `to`. `applyPatch` validates every op against the simulated list length first, so a failing patch throws `PatchError` and leaves the list untouched.
* The decoders reserve at most 65,536 elements up front regardless of the claimed count.

## The FFI target and the core (not linked yet)

`Sources/KeelFFI` is a C target that declares the native ABI of docs/SPEC.md section 6 (`include/keel.h`) and exposes it to Swift as the `KeelFFI` module (`include/module.modulemap`). The Rust core is not linked into this package yet: it will be delivered as an XCFramework built from `crates/keel-ffi`.

Until then `keel_stub.c` provides a stand-in for every `keel_*` symbol so the package builds and tests on a Mac that does not have the core. It is compiled only when `KEEL_STUB_FFI` is defined, and `Package.swift` defines it through `cSettings` by default:

| Build | `KEEL_STUB_FFI` | Effect |
|---|---|---|
| `swift build` / `swift test` (default) | defined | the stub supplies the `keel_*` symbols; `keel_abi_version()` returns 0 and every call fails, so a runtime that reaches it fails loudly at attach time |
| `KEEL_LINK_CORE=1 swift build` | not defined | `keel_stub.c` compiles to an empty unit; the real core must be linked, otherwise the linker reports missing symbols |

Nothing in the wire layer calls into `KeelFFI`, so the wire tests behave identically in both modes.

Wiring the real core (a later task) means: build `libkeel_ffi.a` for the iOS device, iOS simulator and macOS slices; package it with `xcodebuild -create-xcframework -library ... -headers Sources/KeelFFI/include`; add a `.binaryTarget` for it as a dependency of `KeelFFI`; make `KEEL_LINK_CORE` the default in `Package.swift` (dropping the `KEEL_STUB_FFI` define); and import `KeelFFI` from the transport code. `include/keel.h` must stay in step with the exports of `crates/keel-ffi`; changing it is a boundary change and needs an ADR first (CLAUDE.md, R11).
