# undra-ffi

The door between an Undra core and its host: the **C ABI** that Swift (and any C-speaking
host) calls, the **JNI shim** for Kotlin/Android, and the **wasm ABI** for the web. Everything
here is a thin shell over `undra_runtime::Runtime`; this is the only crate in the workspace that
contains `unsafe`.

You do not depend on `undra-ffi` to write a core; `undra-cli` links it into the library it builds.
Read this when you embed a core by hand or debug the boundary.

## Exporting a core

A core library exports one symbol, named after its namespace (C ABI version 2, ADR-044). The crate
that links the core into a library (the shim `undra build` generates) says which:

```rust,ignore
// The shim crate's lib.rs (`ignore`: `acme_pay_core` is the app's crate, not one of this workspace)
undra_ffi::export_core!(acme_pay, jni_class = "com/acme/pay/UndraCoreNative");
extern crate acme_pay_core; // the core's #[undra::api] items
```

That emits `acme_pay_undra_api()`, which returns the core's `UndraApi` table, and on Android
`JNI_OnLoad`, which registers the natives on the bindings' own class. Nothing else is exported, so
two cores link into one app side by side. `undra-ffi` itself is an rlib and exports nothing.

## Embedding the C ABI (about 30 lines of C)

```c
#include "undra.h"          /* runtimes/swift/UndraRuntime/Sources/UndraFFI/include/undra.h */
#include <stdlib.h>
#include <string.h>

const void *acme_pay_undra_api(void);   /* the core's own acme_pay_undra.h declares it */

static void on_reply(void *user, uint32_t call_id, const uint8_t *p, uint32_t n) {
    /* p is valid only during this call: copy, enqueue on your own thread, return.
       Never call into the table from here (except buf_free). */
}
static void on_changes(void *user, const uint8_t *p, uint32_t n) { /* ChangeSet payload */ }
static void on_stream(void *user, uint32_t call_id, const uint8_t *p, uint32_t n) { /* StreamItem */ }

static uint8_t on_port(void *user, uint32_t port, uint32_t method, uint32_t call,
                       const uint8_t *args, uint32_t n, UndraBuf *out) {
    /* Sync port: malloc a PortReply payload, cap = 0, return 0. Async: return 1, later
       api->port_reply(). Unknown: return 2. */
    return 2;
}

int main(void) {
    const UndraApi *api = acme_pay_undra_api();
    if (api->abi_version != UNDRA_ABI_VERSION || api->size < sizeof *api) return 1;
    /* RuntimeConfig: platform String, mode String, core_threads u8, blocking_threads u8, log u8 */
    const uint8_t cfg[] = {1,0,0,0,'c', 6,0,0,0,'i','n','p','r','o','c', 1, 0, 2};
    if (api->init(cfg, sizeof cfg, on_reply, on_changes, on_stream, NULL) != 0) return 2;
    api->port_register(0x575ff24a /* fnv1a32("port.Log") */, on_port, NULL);

    uint8_t call[/* target u8, handle u64, method u32, call_id u32, args */ 17] = {0};
    call[13] = 1;                                    /* call_id = 1 (0 is reserved) */
    UndraBuf r = api->call_sync(call, sizeof call);  /* Reply payload, core-owned */
    /* r.ptr[0..4] = call_id, r.ptr[4] = status (5: no such method), then the body */
    api->buf_free(r);
    api->shutdown();
    return 0;
}
```

## Contracts worth knowing

* **Buffers.** An `UndraBuf` the core returns is yours until the table's `buf_free`. The single buffer
  a *host* allocates is the `out_reply` of a synchronous port callback: `malloc`ed, `len` set,
  `cap` reserved (0); the core copies it and always `free`s it.
* **Callbacks** run on the core thread, a blocking thread or your calling thread, possibly with
  the core lock held (SPEC 5.1), **concurrently**: they must be thread-safe, must not unwind and
  must copy the bytes and return. A call back into the core is refused (`E_REENTRANT`, status 5)
  rather than deadlocked, except `buf_free`, `port_reply`, `stream_credit`, `timer_fired`,
  `stats_json` and the read-only `schema_json` (the table's `abi_version` and `schema_hash` are
  data). The whole host contract is the header comment of `undra.h` (SPEC 6).
* **Lifetimes.** Your `user` pointers outlive `shutdown` returning; a port's outlives the
  `port_register` call that removes or replaces it, which **waits** for that port's running
  callbacks first (so you may free `user` when it returns, and must not call it from inside that
  callback). `shutdown` waits the same way.
* **Nothing unwinds out of an entry of the table.** A panic in the core answers status 2; a panic
  in the shim itself is contained and logged at level 5. This needs `panic = "unwind"` on native
  targets (the crate refuses to compile with `panic = "abort"`); only wasm aborts (SPEC 7: log at
  level 5, then trap).
* **`init`** returns `0` or an `init_code`: repeating it with the same callbacks is a
  no-op, a different embedder gets `ALREADY_INITIALIZED`. `core_threads == 0` is treated as `1`
  (there is no native `undra_poll`). `shutdown` joins the core threads and drops the port
  registrations (waiting for port callbacks still running); `init` may follow.
  `restore` returns a `restore_code`. Each core's table drives that core alone: two cores are two
  runtimes, with their own threads, handles and ports.
* **Logs** reach a native host as calls to its `Log` port (`port.Log` / `Log.log`), so register
  one to see the core's own records (fire and forget: port call id 0, any answer accepted, a
  `port_reply` for id 0 ignored). `port_register` also takes a port over from any default
  Rust binding.
* **Static linking.** The core's `#[undra::api]` registrations are static constructors in object
  files that nothing references. `undra build --platform ios` therefore prelinks each slice into
  one object (`ld -r`, every member kept, `-exported_symbol _<namespace>_undra_api`), so the app
  links `lib<namespace>.a` like any archive, without `-force_load`, and its only global symbol is
  the table's entry. Linking a raw debug staticlib by hand needs `--whole-archive` (GNU) or the
  schema comes out empty.

## The three surfaces

| | Entry | Notes |
|---|---|---|
| C ABI | `<namespace>_undra_api()` returns the `UndraApi` table (`export_core!`) | Swift (module `UndraFFI`, `undra.h`; the generated `<Bundle>FFI` target declares the symbol), any C host, `undra-cli` (`dlopen`, `dlsym` of the table, its `schema_json`) |
| JNI | `JNI_OnLoad` registers the natives on the class `export_core!` names (the bindings' `UndraCoreNative`) | Kotlin; callbacks go to `dev.undra.runtime.NativeCallbacks`, as direct `ByteBuffer`s valid only during the call; callback threads attach as daemons |
| wasm | 20 exports + `_initialize`, 9 imports from module `"undra"` | `Clock`, `Rng`, `Log` are answered natively over `now_ms` / `random` / `log` unless the host answers the port first |

## Building

`undra-ffi` is a library of the core's library, not a library of its own; `tests/fixture` is a core
that exports the ABI as any core does (namespace `undra_fixture`, JNI class
`dev.undra.fixture.UndraCoreNative`):

```sh
cargo build --manifest-path crates/undra-ffi/tests/fixture/Cargo.toml          # libundra_fixture.{dylib,so,a}, with JNI_OnLoad
cargo build --manifest-path crates/undra-ffi/tests/fixture/Cargo.toml \
  --target wasm32-unknown-unknown --profile release-wasm                       # undra_fixture.wasm
```

The library's schema is whatever `#[undra::api]` items are linked into the same binary, and
`undra-cli` extracts it through the table's `schema_json` (the whole schema, doc comments included;
its hash covers the canonical form without them).

## Tests

| What | How |
|---|---|
| C ABI, in process, through a table, real callbacks | `cargo test -p undra-ffi` (`tests/abi.rs`) |
| wasm ABI, hand-written host | `tests/wasm/raw.test.mjs` |
| wasm ABI under the TypeScript runtime | `tests/wasm/ts-runtime.test.mjs` |
| JNI under the Kotlin runtime | `tests/jni/run.sh`, and the runtime's own `NativeSmokeTests` (`UNDRA_NATIVE_LIB_DIR=crates/undra-ffi/tests/fixture/target/debug scripts/test-local.sh run`) |
| C ABI under the Swift runtime | `tests/swift/run.sh` |
| C ABI from a C host built against `undra.h` (`-Wall -Wextra -Werror`) | `tests/c/run.sh` |
| Arbitrary bytes into every payload entry | `arbitrary_bytes_never_break_the_boundary` in `tests/abi.rs` (proptest) |
| Crossing cost | `cargo bench -p undra-ffi --bench boundary` |
