# wasm boundary tests

End-to-end checks of the wasm ABI (SPEC 7) against the **real** `undra-ffi` module, built from
the fixture core in `../fixture` (the same code `tests/abi.rs` runs natively).

```sh
cargo build --manifest-path crates/undra-ffi/tests/fixture/Cargo.toml --target wasm32-unknown-unknown            # debug
cargo build --manifest-path crates/undra-ffi/tests/fixture/Cargo.toml --target wasm32-unknown-unknown --profile release-wasm   # size-optimised, panic=abort (SPEC 7)
node --test crates/undra-ffi/tests/wasm/raw.test.mjs                 # the ABI, hand-written host
# Against the TypeScript runtime, in "wasm-main" and in "wasm-worker" mode (the worker is a real
# worker_threads Worker running the built dist/worker.js). run.sh builds it fresh from source into a scratch
# directory every time; by hand, build it first (npm ci && npx tsc -p tsconfig.build.json in
# runtimes/ts/@undra/runtime) and point UNDRA_TS_DIST at its dist/index.js:
node --test crates/undra-ffi/tests/wasm/ts-runtime.test.mjs
```

`UNDRA_FFI_FIXTURE_WASM=/path/to/undra_fixture.wasm` picks another build (for example a release one
optimised with `wasm-opt`); `UNDRA_TS_DIST` points at the TypeScript runtime's `dist/index.js` (run.sh otherwise builds one; it
never uses a `dist/` that happens to be lying around, and `UNDRA_SKIP_TS=1` skips that leg explicitly).
`run.sh` does all of the above.
