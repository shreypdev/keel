# Contract tests

The definition of "the platforms agree" (SPEC section 14): thirty-four scenarios (S34 is not assigned), run
against the **real playground core** (`examples/playground/core`) through the real boundary: S01 to S20, S23 to
S33 (the opt-in ports of ADR-047 and ADR-048, two cores, objects as parameters and returns, host callbacks, the
panic report and the background run of ADR-046, newtypes and generics, paged queries and lazy lists, polling) and
S35 (query handles across a restore, ADR-059) by each platform runtime, S21 and S22 (web worker mode and crash
recovery, ADR-049) by TypeScript only, 98 cells in all.

| Directory | Platform | Boundary | Run |
|---|---|---|---|
| `ts/` | TypeScript (vitest) | `@undra/runtime` over the real `playground_core.wasm` (wasm-main) | `ts/run.sh` |
| `kotlin/` | Kotlin (kotlinc + JVM) | `dev.undra.runtime` over JNI and the real `libplayground_core` | `kotlin/run.sh` |
| `swift/` | Swift (XCTest) | `UndraRuntime` over the C ABI table of the real core library | `swift/run.sh` |

```sh
contract-tests/run-all.sh            # all three (swift only on macOS), then the scenario grid
contract-tests/run-all.sh ts kotlin  # a subset
```

* `scenarios.md` is the shared manifest: each scenario, its wire-level steps and what is expected, and the
  harness (manual clock, in-memory `Http` server, `Kv`, `Log`, `Connectivity`) every runner implements.
* Every runner prints `SCENARIO S07 PASS|FAIL|SKIP <title>` lines; `check.sh <platform>` fails unless every
  scenario of that platform passes.
* S14, S15 and S35 also run against a second build of the core (build B, `UNDRA_PLAYGROUND_V2=1`; ADR-037,
  ADR-059): see "Two builds" in `scenarios.md`.
* Each runner builds the core it needs with the `undra` CLI (`undra build -C examples/playground --platform
  web|host`) and uses the bindings `undra bindgen` generated (`examples/playground/generated`), plus the runtime's
  own API for what bindings do not expose (raw signal updates, statistics, snapshots, schema checks).
* `NOTES.md` in each runner says where the scenario text had to be read for that platform, and lists the
  defects and gaps the run found in the merged crates and runtimes. A defect that is still open is kept as
  an expected-failure test that turns red when it is fixed (`ts/test/findings.test.ts`); a fixed one is an
  ordinary check in a scenario or in the runtime's own tests.
* `wire-vectors.json` is the shared byte-level vector table the three runtimes' codec tests read.

Environment: Node 22+, Rust with `wasm32-unknown-unknown`, `kotlinc` (set `UNDRA_KOTLIN_STDLIB` and
`UNDRA_KOTLINX_COROUTINES`, see `scripts/env.sh`), JDK 17, and full Xcode for the Swift runner.
