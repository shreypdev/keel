# QA — index

Suites and how to run them (all local):
- Rust: `cargo test --workspace` (proptest + byte-fuzz included). 1,079 green.
- TS: `cd runtimes/ts/@keel/runtime && npm test` (vitest). 830 green.
- Kotlin: `runtimes/kotlin/keel-runtime/scripts/test-local.sh` (kotlinc + stub JUnit
  annotations, reflection-free runner; env.sh sets the jars). 454 green, 2 JNI skips.
- Swift: `cd runtimes/swift/KeelRuntime && swift test` (needs Xcode; env.sh sets
  DEVELOPER_DIR). 313 green.
Contract scenarios (SPEC §14) not yet wired to the playground core — pending piece 7.
