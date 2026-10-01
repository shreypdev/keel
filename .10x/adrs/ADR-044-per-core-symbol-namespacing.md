# ADR-044: every core is a self-contained image with one namespaced entry point, so several cores can share a process

Status: **Proposed** (2026-10-01, `wt/boundary-adrs`; Amendment B "multiple cores per app", catalogue M-12
and finding 5; decided with ADR-038 in mind). Touches SPEC 6 (the C ABI is reached through one exported
function per core returning a function table; `undra_abi_version` becomes the table's version 2), 6.1 (the JNI
natives are registered on a class per core), 6.2, 7 (unchanged exports; one module instance per core), 11 and
17 (an `UndraCore` per core; generated code defaults to its own core), 13 (artefact names) and the CLI's
`undra.toml` (`[core] namespace`); `undra-ffi`, `undra-cli` (shim template, builds, schema loader, templates),
`undra-bindgen`, the three runtimes, and ADR-038's C++ module before it is built. **No wire change, no schema
change, no wasm ABI change. The native C ABI changes shape (same 19 operations, same signatures, reached
through a table) — a pre-publication ABI break, version 2.** Constitution R2 (the only new `unsafe` is in
`undra-ffi`), R7 (the ABI version and the schema hash are both checked at load) and R11.

## Context

Today a process can hold exactly one Undra core:

* The C ABI is 19 global symbols, `undra_init` … `undra_buf_free` (SPEC 6), exported with
  `#[unsafe(no_mangle)]` from `undra-ffi` itself (`crates/undra-ffi/src/native.rs:225-486`) and re-exported by
  the generated shim (`pub use undra_ffi::*;`, `crates/undra-cli/templates/shim/lib.rs:6`). `undra_init` is
  "idempotent per process" (SPEC 6); a second embedder gets `ALREADY_INITIALIZED`
  (`crates/undra-ffi/src/session.rs:134-141`).
* The JNI natives are static methods registered on one fixed class, `dev/undra/runtime/UndraNative`
  (`crates/undra-ffi/src/jni_shim.rs:38-40`, `:522`), and also exported as `Java_dev_undra_runtime_UndraNative_*`.
  The Kotlin runtime loads one library name (`undra_core`, `UndraNative.kt:57-65`, `:174-187`) and refuses a
  second in-process core (`InprocTransport.kt:95-99`, `:261-262`).
* The Swift runtime links the 18 `undra_*` functions it calls directly from its `UndraFFI` module
  (`runtimes/swift/UndraRuntime/Sources/UndraRuntime/Core/InprocTransport.swift:45-306`) and claims the process
  (`InprocTransport.active`, `:17-21`, `:88-97`).
* Every platform artefact has one fixed name: `UndraCore.xcframework` with `libundra_core.a`,
  `jniLibs/<abi>/libundra_core.so`, `undra_core.wasm` (`crates/undra-cli/src/builds/{ios,android,web}.rs`), so
  two cores cannot even sit in one APK.
* Generated code defaults to the process's first core (`ctx: UndraCore = .shared` in Swift,
  `UndraCore.shared` in Kotlin and TypeScript; the goldens under `crates/undra-bindgen/tests/golden/`).
* `undra.toml` has no namespace, library-name or JNI-class key (`crates/undra-cli/src/config.rs:253-408`).

The catalogue (`.10x/specs/2026-10-01-competitive-limitations.md`) records the same pain in KMP ("any state
passed by different modules through the same dependency won't be connected"; the fix is an umbrella, KMP-9)
and Flutter ("Packing multiple Flutter libraries into an application isn't supported", FL-5), finds UniFFI's
crate-namespaced symbols "appear to avoid it" (UNI-B6, unverified), rates it L, and says to decide it "before
anything is published" (finding 5). G1's React Native host (ADR-038, proposed) "calls the 19 functions of
`undra.h` exactly as the Swift host does" and states "one core per process" as a limit; it must not be built on
global symbols if this ADR changes them.

### What the linker does (probe, this ADR; throwaway crates in the session scratch directory, not committed)

Two tiny Rust cores, each depending on one shared crate that holds an `inventory` registry and a global
`static` (the shape of `undra-meta`/`undra-signals`/`undra-runtime`), each exporting one prefixed C function,
linked into one C program on macOS 26 (Xcode 26.6 `ld`, Rust 1.98.1, `aarch64-apple-darwin`; the iOS slice
`aarch64-apple-ios` was linked too):

| How the two cores are linked | Result |
|---|---|
| Two plain staticlibs (non-LTO), no `-force_load` | **links and silently merges them**: both cores see `core_b.item,core_a.item` in one registry and share one global counter (1, 2, 3). Two Undra cores would share one schema registry, one change sink, one global runtime |
| Two staticlibs with `-force_load` (what the app template does so debug registrations survive, ADR-029) | **2,179 duplicate symbols**, link fails (2,178 for the iOS slice) |
| Two fat-LTO staticlibs (the release profile), no `-force_load` | 2 duplicate symbols (`rust_eh_personality`, std's `EMPTY_PANIC`), link fails |
| Each staticlib **prelinked** (`ld -r`) into one object with only `_<ns>_undra_report` left global | links; registries and statics are **independent** (`core_a.item global=1`, `core_b.item global=1`, `core_a.item global=2`) |
| ... prelinked from the fat-LTO staticlib with `-u _<ns>_…` instead of `-all_load` | the same, and the two-core program is 712 KB — the size of two cdylibs (2 × 353 KB); with `-all_load` it is 1.3 MB |
| Two cdylibs (`.dylib`) | independent; each exports only its `#[no_mangle]` function (rustc's cdylib export list) |
| The prelinked program, built with `debug = "line-tables-only"`, after `dsymutil` | `atos` resolves the Rust frame: `a_undra_report (in t8) (lib.rs:3)`; the binary is the same size (DWARF stays in the object and the dSYM) |

So the global `undra_*` names are the visible half of the problem; the invisible half is that a Rust static
library exports **every** Rust symbol, and two of them in one binary either collide or, worse, merge.

## Decision

1. **A core has a namespace.** `undra.toml` `[core] namespace = "acme_pay"`: a C identifier of at most 32
   characters, default the core crate's package name in snake case (`playground_core`). It names every artefact
   and symbol below; two cores in one app must have different namespaces (the CLI refuses a namespace that
   another core in the same `undra.toml` workspace uses, and the runtimes refuse to load two cores with one
   namespace).
2. **One exported symbol per core: a function table.** `undra-ffi` stops exporting `undra_*`; its entry points
   become plain `extern "C" fn`s collected in a `static UNDRA_API: UndraApi`. The generated shim exports one
   function through a macro defined in `undra-ffi` (so the `unsafe` attribute stays in that crate, R2):

   ```rust
   undra_ffi::export_core!(acme_pay, jni_class = "dev/acme/pay/core/UndraCoreNative");
   // expands to: #[unsafe(no_mangle)] pub extern "C" fn acme_pay_undra_api() -> &'static UndraApi { .. }
   //            (+ JNI_OnLoad registering on `jni_class`, with the `jni` feature)
   ```

   ```c
   // undra.h (version 2): the table, the callbacks' typedefs and the host contract text; no functions.
   typedef struct UndraApi {
       uint32_t abi_version;   // 2
       uint32_t size;          // sizeof(UndraApi) as built; fields are only ever appended
       uint64_t schema_hash;
       const char *name_space; // "acme_pay", NUL-terminated
       uint32_t (*init)(const uint8_t *cfg, uint32_t len, undra_reply_cb, undra_changeset_cb, undra_stream_cb, void *user);
       void     (*shutdown)(void);
       uint32_t (*call)(const uint8_t *ptr, uint32_t len);
       UndraBuf (*call_sync)(const uint8_t *ptr, uint32_t len);
       /* … the other 15 entries of SPEC 6, same signatures, same host contract … */
   } UndraApi;
   // Each core exports: const UndraApi *<namespace>_undra_api(void);
   // (its own header declares the result as `const void *`, so the type stays owned by one module; decision 5)
   ```

   A host reads `abi_version` and `schema_hash` from the table before `init` (R7); the table is immutable
   static data. The cost is one indirect call per entry (about a nanosecond against the 49.8 ns synchronous
   call). The process-wide state the runtime keeps (`Runtime::global`, the port registry, the change sink, the
   `inventory` registries, the generation counter) stays exactly as it is, because decision 3 gives each core
   its own copy of it.
3. **Each core image is self-contained.** Nothing but the namespaced entry (and `JNI_OnLoad`) is global:
   * **Android, JVM desktop, macOS/Linux host:** a cdylib, as today; rustc already exports only the
     `#[no_mangle]` items. Renamed `lib<namespace>.so` / `.dylib`.
   * **iOS (and the macOS slice of C5):** `undra build` **prelinks** each slice's staticlib into one object
     whose only global is `_<namespace>_undra_api` (`ld -r -exported_symbol _<ns>_undra_api`), from the fat-LTO
     staticlib with `-u _<ns>_undra_api` in release and with `-all_load` in debug, then archives it as
     `lib<namespace>.a` in `<Namespace>Core.xcframework`. Because the entry symbol pulls the one object that
     holds every registration, **the app no longer needs `-force_load`** (the template, `undra adopt` and
     ADR-038's podspec drop it), and two cores in one binary neither collide nor merge.
   * **Web:** a wasm module is its own namespace already; the exports keep their names, each core is its own
     instance (the TypeScript runtime already supports several, `transport/wasm-main.ts:173-215`). Renamed
     `<namespace>.wasm`.
4. **JNI: a class per core.** `JNI_OnLoad` registers the 16 natives as statics of the core's own class
   (`<kotlin_package>.UndraCoreNative`, generated); the `Java_dev_undra_runtime_UndraNative_*` exports are
   dropped (every JVM that `System.loadLibrary`s calls `JNI_OnLoad`). The callbacks interface moves to
   `dev.undra.runtime.NativeCallbacks` (shared, no natives). The generated `internal object UndraCoreNative :
   NativeApi` (the runtime's existing interface, `InprocTransport.kt:16-34`) loads `lib<namespace>.so` and
   forwards to its `@JvmStatic external fun`s; R8 keep rules are generated with it.
5. **The runtimes hold one `UndraCore` per core; generated code uses its own.** Each generated module emits an
   entry point named after the namespace that loads its core with the right table (or library), expected hash
   and namespace, and every generated default parameter uses it:

   ```swift
   public enum UndraAcmePay {
       public static func load(_ options: LoadOptions = .init()) throws -> UndraCore   // .inproc(api: acme_pay_undra_api())
       public static var core: UndraCore { get }                                        // the loaded one, or the shut-down placeholder
   }
   public convenience init(ctx: UndraCore = UndraAcmePay.core) throws
   ```

   ```kotlin
   object UndraAcmePay { fun load(options: LoadOptions = LoadOptions()): UndraCore; val core: UndraCore }
   class Wallet(ctx: UndraCore = UndraAcmePay.core) : UndraObject(…)
   ```

   ```ts
   export const UndraAcmePay: { load(options: LoadOptions): Promise<UndraCore>; readonly core: UndraCore };
   static create(core: UndraCore = UndraAcmePay.core): Promise<Wallet>;
   ```

   `UndraCore.shared` remains (the first core loaded) for app code; generated code never reads it. The Swift
   in-process claim becomes per table (`InprocTransport.active` keyed by the table's address), Kotlin's
   `claimed` set already holds `NativeApi` instances. The C header for a core ships inside its XCFramework as
   module `<Namespace>CoreFFI` (`<namespace>_undra.h` declares `const void *<namespace>_undra_api(void);`; the
   generated module passes the pointer to `UndraCore`, whose `UndraFFI` module owns the `UndraApi` type), which
   the current single module, `UndraFFI`, could not allow (`crates/undra-cli/src/builds/ios.rs:4-10`).
6. **Two ways to have several crates, both supported.** Teams in one organisation should still prefer an
   **umbrella core** (one core crate that depends on the feature crates: one namespace, one runtime, one copy of
   std; `inventory` already merges registrations across crates and bindgen's E0050 catches name clashes).
   **Independent cores** (an SDK vendor's core inside an app that has its own) are what this ADR makes possible.
   The two cores share nothing: no handles (a handle of one is stale in the other, ADR-040), no types (each has
   its own generated `Todo`), no runtime threads; values pass between them through app code.
7. **The schema loader** (`crates/undra-cli/src/schema.rs:44-132`, the CLI's one `unsafe` site) `dlsym`s
   `<namespace>_undra_api` from `undra.toml` and reads the table instead of four symbols.
8. **ADR-038 (React Native) adopts the table before it is built.** Its decision 1 installs one host object per
   core (`install(namespace)` → `globalThis.__undraNative[namespace]`), calls through the `UndraApi*` obtained
   from `<namespace>_undra_api()` (iOS, the pod's header) or `dlopen("lib<namespace>.so")` + `dlsym` (Android),
   and keeps one inbox per core; its "one core per process" limit is lifted; its podspec drops `-force_load`.
   Nothing else in ADR-038 changes.

## Alternatives considered

* **Prefix all 19 symbols (`acme_pay_undra_call`, …).** Each runtime is compiled once and would have to
  resolve 19 names per core at run time (`dlsym`, which does not work for static iOS linking without
  exporting them from the app) or link per-core C shims; and it solves only the visible half — the Rust
  symbols still collide or merge in a static link.
* **One shared JNI library in the Kotlin runtime that `dlopen`s cores and calls through their tables.** Every
  host would then be a host of the C table (attractive), but it adds a second Rust image (its own std) to every
  Android app, a second `.so` whose version must match each core's, and an extra hop per call; the per-core
  class costs one generated object.
* **Shared natives on `UndraNative` that take a core pointer.** The last library to load would own the natives
  of every core: with two Undra versions, one core's code would call through another's table.
* **Dynamic frameworks on iOS instead of prelinking.** Also isolates (the probe's cdylib row), but embeds and
  signs a framework per core, costs dyld time at launch, and changes what `adopt` asks of an app. Prelinking
  keeps today's static integration and removes `-force_load`. A dynamic framework stays possible as an option.
* **Symbol renaming of a finished archive (`objcopy --prefix-symbols`).** Not available in Apple's toolchain,
  fragile with LTO, and renames symbols the app's dSYM must match.
* **Keep one core per process and document umbrellas** (KMP's answer). The SDK-vendor case is real and cannot
  be solved by an umbrella; this is the cheapest moment to change the ABI.

## Consequences

* Two (or more) Undra cores, including two Undra versions, can live in one app on iOS, Android, the JVM, macOS
  and the web. Catalogue row 30 moves to "yes".
* Artefact names change: `<Namespace>Core.xcframework` (`lib<namespace>.a`), `jniLibs/<abi>/lib<namespace>.so`,
  `<namespace>.wasm`. Templates, `adopt` output and the playground follow; the Xcode template drops
  `-force_load`.
* `undra.h` becomes version 2 (a table, no functions); the C test harnesses (`crates/undra-ffi/tests/swift/run.sh`,
  the contract suites) call through the table.
* Each extra core costs its own std and runtime image (about 0.35 MB for the probe's empty core; the real core
  is 831 KB per Android ABI and about 0.9 MB on iOS, `bench/RESULTS.md`) and its own `undra-core`, timer and
  blocking threads. The docs recommend an umbrella when one team owns both.

## Risks

* **The prelink step** depends on Apple's `ld -r` behaviour (exported-symbol lists with `-r`, the LTO object's
  bitcode sections, `__mod_init_func` preserved). The probe covers macOS and iOS device slices; the
  implementation adds a CI test that links two prelinked playground cores into one iOS simulator app and checks
  both schema hashes and both registries.
* **Symbolication across a prelinked object** works in the probe (`atos` with the dSYM); ADR-046 keeps a test.
* **`dlsym` on Android** of a library loaded `RTLD_LOCAL` by `System.loadLibrary` needs the handle from a second
  `dlopen` of the same name (returns the same handle); ADR-038's C++ code does exactly that.

## Implementation brief

1. `crates/undra-ffi`: `UndraApi` (`#[repr(C)]`, `abi_version = 2`, `size`, `schema_hash`, `name_space`, the 19
   function pointers in SPEC 6 order), `static UNDRA_API`, the entry points without `no_mangle`,
   `export_core!(ns, jni_class = ..)` (`macro_rules!`, every `unsafe` with its `// SAFETY:` comment), JNI
   registration on the class given to `export_core!`, no `Java_*` exports; ABI tests through the table
   (`abi.rs`'s callback rules, `sync_alloc.rs`).
2. `undra.h` (Swift runtime's `UndraFFI`, ADR-038's copy, and a test that keeps them byte-identical): the
   table, no functions; host contract text unchanged.
3. `crates/undra-cli`: `[core] namespace` (validation, default), the shim template calling `export_core!`,
   `builds/ios.rs` (prelink per slice, `lib<ns>.a`, `-headers` with `<ns>_undra.h` and the `<Namespace>CoreFFI`
   module map), `builds/android.rs` (`lib<ns>.so`), `builds/web.rs` (`<ns>.wasm`), `builds/host.rs`,
   `schema.rs` (the table), templates (`-force_load` removed; loaders call `Undra<Ns>.load`), `adopt` text;
   `tests/schema_retention.rs` checks the table symbol and `JNI_OnLoad` only.
4. `crates/undra-bindgen`: the per-namespace entry (`Undra<Ns>` in Swift, Kotlin, TS), defaults to it, the
   generated `UndraCoreNative` object and its R8 rules; `UndraIds.namespace`. Goldens updated.
5. Runtimes: Swift `UndraCore.load(.inproc(api:))` over `UnsafePointer<UndraApi>` (the transport calls through
   the table), per-table claim; Kotlin `InprocTransport(native = <generated object>)`, `NativeCallbacks`; TS
   unchanged except the generated entry.
6. A **two-core test app** in CI per platform (iOS simulator, Android emulator, JVM, Node): two copies of the
   playground core built under two namespaces (`playground_a`, `playground_b`), both loaded, a call and an
   observed change on each, independent stats, one shut down while the other keeps working. Contract scenario
   **S26 "two cores"** (provisional number) on the JVM, Swift and TS columns.
7. Bench: `boundary/call_sync/add` through the table (no regression beyond 2 ns on the host).
8. Docs: SPEC 6, 6.1, 6.2, 7, 11, 13, 17; `docs/ONBOARDING.md`; the cookbook's "one core or several" page
   (umbrella vs independent).

## Dependencies

None to start; the earlier the better, because every host built after it (ADR-038 first) must use the table.
ADR-046's symbol artefacts build on the prelinked iOS object and the renamed libraries.
