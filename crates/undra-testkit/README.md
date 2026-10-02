# undra-testkit

Testing tools for Undra apps (`docs/TESTING.md`): the recording format, port record/replay, seeds for the deterministic fakes, and the harness the three platform testing kits (`UndraTestKit`, `dev.undra.testkit`, `@undra/testkit`) are checked against. It is re-exported as `undra::testing`.

```rust
use std::time::Duration;
use undra_testkit::{Harness, Recorder, Recording, Replayer};

// A runtime with every fake installed and a manual clock, started from a seed.
let h = Harness::from_seed_json(r#"{"now_ms": 0, "kv": {"theme": "dark"}}"#).unwrap();
assert_eq!(h.fakes().kv.value("theme"), Some(b"dark".to_vec()));
h.advance(Duration::from_secs(1));

// A recording is versioned JSON; the writer is canonical, so equal sessions are equal bytes.
let recorder = Recorder::with_clock(0xabcd, "test", || 0);
let recording: Recording = recorder.finish();
let again = Recording::from_json(&recording.to_json()).unwrap();
assert_eq!(again, recording);

// A replayer answers a core's port calls from the recording, in order.
let replayer = Replayer::new(&recording);
assert!(replayer.finish().is_ok());
```
