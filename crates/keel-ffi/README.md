# keel-ffi

The door between a Keel core and its host: the **C ABI** that Swift (and any C-speaking
host) calls, the **JNI shim** for Kotlin/Android, and the **wasm ABI** for the web. Everything
here is a thin shell over `keel_runtime::Runtime`; this is the only crate in the workspace that
contains `unsafe`.

You do not depend on `keel-ffi` to write a core; `keel-cli` links it into the library it builds.
Read this when you embed a core by hand or debug the boundary.

## Embedding the C ABI (about 30 lines of C)

```c
#include "keel.h"          /* runtimes/swift/KeelRuntime/Sources/KeelFFI/include/keel.h */
#include <stdlib.h>
#include <string.h>

static void on_reply(void *user, uint32_t call_id, const uint8_t *p, uint32_t n) {
    /* p is valid only during this call: copy, enqueue on your own thread, return.
       Never call keel_* from here (except keel_buf_free). */
}
static void on_changes(void *user, const uint8_t *p, uint32_t n) { /* ChangeSet payload */ }
static void on_stream(void *user, uint32_t call_id, const uint8_t *p, uint32_t n) { /* StreamItem */ }

static uint8_t on_port(void *user, uint32_t port, uint32_t method, uint32_t call,
                       const uint8_t *args, uint32_t n, KeelBuf *out) {
    /* Sync port: malloc a PortReply payload, cap = 0, return 0. Async: return 1, later
       keel_port_reply(). Unknown: return 2. */
    return 2;
}

int main(void) {
    /* RuntimeConfig: platform String, mode String, core_threads u8, blocking_threads u8, log u8 */
    const uint8_t cfg[] = {1,0,0,0,'c', 6,0,0,0,'i','n','p','r','o','c', 1, 0, 2};
    if (keel_abi_version() != 1) return 1;
    if (keel_init(cfg, sizeof cfg, on_reply, on_changes, on_stream, NULL) != 0) return 2;
    keel_port_register(0x575ff24a /* fnv1a32("port.Log") */, on_port, NULL);

    uint8_t call[/* target u8, handle u64, method u32, call_id u32, args */ 17] = {0};
    call[13] = 1;                                    /* call_id = 1 (0 is reserved) */
    KeelBuf r = keel_call_sync(call, sizeof call);   /* Reply payload, core-owned */
    /* r.ptr[0..4] = call_id, r.ptr[4] = status (5: no such method), then the body */
    keel_buf_free(r);
    keel_shutdown();
    return 0;
}
```

## Contracts worth knowing

* **Buffers.** A `KeelBuf` the core returns is yours until `keel_buf_free`. The single buffer
  a *host* allocates is the `out_reply` of a synchronous port callback: `malloc`ed, `cap = 0`;
  the core copies it and `free`s it.
* **Callbacks** run on the core thread, a blocking thread or your calling thread, possibly with
  the core lock held (SPEC 5.1). Copy the bytes and return; a same-thread call back into the core
  is refused (`E_REENTRANT`, status 5) rather than deadlocked.
* **Nothing unwinds out of a `keel_*` function.** A panic in the core answers status 2; a panic
  in the shim itself is contained and logged at level 5. This needs `panic = "unwind"` on native
  targets (the crate refuses to compile with `panic = "abort"`); only wasm aborts (SPEC 7: log at
  level 5, then trap).
* **`keel_init`** returns `0` or an `init_code`: repeating it with the same callbacks is a
  no-op, a different embedder gets `ALREADY_INITIALIZED`. `core_threads == 0` is treated as `1`
  (there is no native `keel_poll`). `keel_shutdown` joins the core threads and drops the port
  registrations; `keel_init` may follow. `keel_restore` returns a `restore_code`.
* **Logs** reach a native host as calls to its `Log` port (`port.Log` / `Log.log`), so register
  one to see the core's own records. `keel_port_register` also takes a port over from any default
  Rust binding.
* **Static linking.** The core's `#[keel::api]` registrations are static constructors in object
  files that nothing references. A release build (`lto = "fat"`, `codegen-units = 1`) is one
  object and links as is; a debug staticlib has many, so link it with `-force_load` (Apple) or
  `--whole-archive` (GNU) or the schema comes out empty.

## The three surfaces

| | Entry | Notes |
|---|---|---|
| C ABI | `keel_*` in `native` | Swift (module `KeelFFI`, `keel.h`), any C host, `keel-cli` (`dlopen` + `keel_schema_json`) |
| JNI | `JNI_OnLoad` registers `dev.keel.runtime.KeelNative` | Kotlin; callbacks are direct `ByteBuffer`s valid only during the call; callback threads attach as daemons |
| wasm | 20 exports + `_initialize`, 9 imports from module `"keel"` | `Clock`, `Rng`, `Log` are answered natively over `now_ms` / `random` / `log` unless the host answers the port first |

## Building

```sh
cargo build -p keel-ffi                          # libkeel_ffi.{dylib,so,a}: the C ABI
cargo build -p keel-ffi --features jni           # + JNI_OnLoad / RegisterNatives (Kotlin)
cargo build -p keel-ffi --target wasm32-unknown-unknown --profile release-wasm   # keel_ffi.wasm
```

The library's schema is whatever `#[keel::api]` items are linked into the same binary: a core
crate depends on `keel-ffi` and builds as a `cdylib` / `staticlib` (its `keel_*` exports come
along), and `keel-cli` extracts the schema with `keel_schema_json`. `tests/fixture` is exactly
such a core.

## Tests

| What | How |
|---|---|
| C ABI, in process, real callbacks | `cargo test -p keel-ffi` (`tests/abi.rs`) |
| wasm ABI, hand-written host | `tests/wasm/raw.test.mjs` |
| wasm ABI under the TypeScript runtime | `tests/wasm/ts-runtime.test.mjs` |
| JNI under the Kotlin runtime | `tests/jni/run.sh`, and the runtime's own `NativeSmokeTests` (`KEEL_NATIVE_LIB_DIR=target/debug KEEL_NATIVE_NAME=keel_ffi scripts/test-local.sh run`) |
| C ABI under the Swift runtime | `tests/swift/run.sh` |
| Crossing cost | `cargo bench -p keel-ffi --bench boundary` |
