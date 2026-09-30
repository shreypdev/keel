# wasm boundary tests

End-to-end checks of the wasm ABI (SPEC 7) against the **real** `keel-ffi` module, built from
the fixture core in `../fixture` (the same code `tests/abi.rs` runs natively).

```sh
cargo build --manifest-path crates/keel-ffi/tests/fixture/Cargo.toml --target wasm32-unknown-unknown            # debug
cargo build --manifest-path crates/keel-ffi/tests/fixture/Cargo.toml --target wasm32-unknown-unknown --profile release-wasm   # size-optimised, panic=abort (SPEC 7)
node --test crates/keel-ffi/tests/wasm/raw.test.mjs                 # the ABI, hand-written host
# Against the TypeScript runtime's WasmMainTransport (build it first: npm ci && npx tsc -p
# tsconfig.build.json in runtimes/ts/@keel/runtime):
node --test crates/keel-ffi/tests/wasm/ts-runtime.test.mjs
```

`KEEL_FFI_FIXTURE_WASM=/path/to/keel_core.wasm` picks another build (for example a release one
optimised with `wasm-opt`); `KEEL_TS_DIST` points at the TypeScript runtime's `dist/index.js`.
`run.sh` does all of the above.
