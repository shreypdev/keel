# Architect: React Native is a fourth host of the C ABI (2026-10-01)

**Problem.** v1.2 bet G1: teams that keep React Native for UI must be able to put Undra under it.
Hermes has no WebAssembly, so `@undra/runtime`'s wasm modes cannot run; the remote mode is a dev loop.
The native core for both phones already exists (`undra build --platform ios|android`).

**Decision (ADR-038, proposed; implemented on `wt/react-native`).** A pure C++ TurboModule,
`UndraNative`, whose only method installs `globalThis.__undraNative`: JSI host functions over the C ABI.
One C++ implementation for iOS (linked XCFramework) and Android (`dlopen` of the APK's
`libundra_core.so`), every core symbol in one shim per platform (the ADR-044 migration point). The
JavaScript side is a `Transport` of the TypeScript runtime (`NativeTransport`, synchronous like
`wasm-main`); `loadNative` is `UndraCore.attach`, so the mirror, codecs, generated bindings and React
hooks are unchanged. Every callback copies once into one inbox (commit order across threads), handed
to JavaScript as one owned `ArrayBuffer` before a core-entering host function returns or through
`invokeAsync`. Clock, Rng and Log native; Timer the core's; async ports answered by JavaScript; JS sync
ports only on the JS thread (never block the core on the JS thread). Drains at a native vsync
(`CADisplayLink` / `AChoreographer`). Ports registered before `undra_init`.

**Rejected.** A wasm polyfill under Hermes (no WebAssembly, no threads); a JSON bridge per call
(serialisation on top of the wire, no `ArrayBuffer` in codegen); two platform modules (Obj-C++ and
Kotlin/JNI: two implementations of the callback discipline); Expo Modules (a dependency in every app,
two native implementations; a thin wrapper stays possible); `requestAnimationFrame` as the frame
(measured at 60 Hz today, but defined by React Native as `setTimeout(0)`); blocking the core for JS
sync ports (deadlock).

**Findings that changed the design while building it.** Hermes has no `TextDecoder` (installed by the
package, from a side-effect-only module so lazy imports cannot defer it) and its compiler rejects
`import.meta` (a Babel plugin ships with the package); a clean Xcode build refuses a `-force_load` path
inside `PODS_XCFRAMEWORKS_BUILD_DIR` (the pod picks the XCFramework slice per SDK instead); ADR-044
landed mid-piece: decisions 1, 11 and 13 are marked transitional and the symbol references were
confined to two shim files.

**Cost.** One package (`runtimes/rn/@undra/react-native`: TS, C++, a podspec, a CMake file), one
`undra build` target (`rn`), two TypeScript contract assertions made transport-agnostic, a React
Native app in the playground. No wire, ABI, schema or generated-code change; the TypeScript runtime
gains no code.

**Open for the integrator.** Whether `@undra/runtime` should take the Hermes per-call costs the
measurements expose (a 4 us `encodeCall` per sync call, a 13 to 16 ms mirror frame at 100,000
patches a second on the iOS simulator); a CI job for the RN app (macOS runner with CocoaPods, an
Android emulator); merging ADR-038 with ADR-044's `abi-table` piece (the shims, `install(namespace)`,
the pod without `-force_load`).
