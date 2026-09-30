# Contract tests

The definition of "the platforms agree" (SPEC section 14): seventeen scenarios, run by each platform
runtime against the **real playground core** (`examples/playground/core`) through the real boundary.

| Directory | Platform | Boundary | Run |
|---|---|---|---|
| `ts/` | TypeScript (vitest) | `@keel/runtime` over the real `keel_core.wasm` (wasm-main) | `ts/run.sh` |
| `kotlin/` | Kotlin (kotlinc + JVM) | `dev.keel.runtime` over JNI and the real `libkeel_core` | `kotlin/run.sh` |
| `swift/` | Swift (XCTest) | `KeelRuntime` over the C ABI and the real core library | `swift/run.sh` |

```sh
contract-tests/run-all.sh            # all three (swift only on macOS), then the 17-scenario grid
contract-tests/run-all.sh ts kotlin  # a subset
```

* `scenarios.md` is the shared manifest: each scenario, its wire-level steps and what is expected, and the
  harness (manual clock, in-memory `Http` server, `Kv`, `Log`, `Connectivity`) every runner implements.
* Every runner prints `SCENARIO S07 PASS|FAIL|SKIP <title>` lines; `check.sh <platform>` fails unless all
  seventeen pass.
* Each runner builds the core it needs with the `keel` CLI (`keel build -C examples/playground --platform
  web|host`) and uses the bindings `keel bindgen` generated (`examples/playground/generated`), plus the runtime's
  own API for what bindings do not expose (raw signal updates, statistics, snapshots, schema checks).
* `NOTES.md` in each runner says where the scenario text had to be read for that platform, and lists the
  defects and gaps the run found in the merged crates and runtimes. The Swift and TypeScript runners keep
  each defect as an expected-failure test (`Findings.swift`, `test/findings.test.ts`) that turns red when the
  defect is fixed.
* `wire-vectors.json` is the shared byte-level vector table the three runtimes' codec tests read.

Environment: Node 22+, Rust with `wasm32-unknown-unknown`, `kotlinc` (set `KEEL_KOTLIN_STDLIB` and
`KEEL_KOTLINX_COROUTINES`, see `scripts/env.sh`), JDK 17, and full Xcode for the Swift runner.
