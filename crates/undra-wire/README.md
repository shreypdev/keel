# undra-wire

The binary wire codec used by every Undra component (`docs/SPEC.md` section 3).

Little-endian, no alignment, no padding, no dependencies. Encoders write into a
`Writer`; decoders read from a `Reader<'a>` and never panic on malformed input, they
return a typed `WireError`.

```rust
use undra_wire::{Decode, Encode, Reader, Writer};

let mut w = Writer::new();
"hello".encode(&mut w);
Some(42_i32).encode(&mut w);
let bytes = w.into_vec();

let mut r = Reader::new(&bytes);
assert_eq!(String::decode(&mut r).unwrap(), "hello");
assert_eq!(Option::<i32>::decode(&mut r).unwrap(), Some(42));
r.finish().unwrap();
```

## What is in the crate

| Module / item | Purpose |
|---|---|
| `Writer`, `Reader` | Primitive reads and writes (`write_u32`, `read_str`, ...) |
| `Encode`, `Decode` | Traits implemented for primitives, `String`, `Bytes`, `Option`, `Vec`, maps, `Duration`, `Timestamp`, `Uuid`, `Result`, tuples, `Box`, `Arc`, `Handle` |
| `Envelope`, `Kind` | The 23 byte transport frame header and the 16 message kinds |
| `payload` | Typed payloads: `Call`, `Reply`, `ChangeSet`, `ChangeSetRef`, `PortCall`, `PortReply`, `Cancel`, `StreamCredit`, `StreamItem`, `Observe`, `Release`, `Event`, `Hello`, `Log`, `TimerFired`, `Snapshot` |
| `KeyedPatch` | Keyed list patches: encode, decode, `apply` and `diff` |

## Guarantees

* No `unsafe` (`#![forbid(unsafe_code)]`).
* Decoding never panics, never over-allocates from a hostile length prefix and never
  recurses without bound (see `MAX_DEPTH`).
* Maps encode with entries sorted by their encoded key bytes, so equal values always
  produce equal bytes.
* Zero-copy where it matters: `Reader::read_str` and `Reader::read_bytes` borrow the
  input, `Envelope::parse` and the call-like payloads borrow their bodies, and
  `ChangeSetRef` iterates entries without allocating.

## Tests

* Table-driven test over `contract-tests/wire-vectors.json` (both directions).
* Property tests: round trip for every `Encode` / `Decode` implementation and for
  keyed patch `diff` + `apply`.
* Byte-fuzz test: random and mutated bytes must never panic any decoder.
* Explicit malformed-input cases for every error variant.

```text
cargo test -p undra-wire
```
