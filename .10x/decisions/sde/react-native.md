# SDE — React Native runtime (wt/react-native, 2026-10-01)

Implements ADR-038 (React Native is a fourth host of the C ABI), v1.2 bet G1. No wire, C ABI, wasm
ABI, schema or generated-code change; nothing in `runtimes/ts/@undra/runtime` changed.

## What changed

**`runtimes/rn/@undra/react-native`** (new package):
* `cpp/UndraApi.h`, `cpp/UndraApiLinked.cpp` (iOS), `cpp/UndraApiAndroid.cpp` (`dlopen`): the C ABI as
  one struct, the only files that name a core symbol (ADR-044 migration point); `cpp/undra.h` (a copy
  a test keeps byte-identical to the Swift runtime's).
* `cpp/UndraHost.{h,cpp}`: the JSI-free host: `start` (ports registered before `undra_init`, native
  Clock/Rng/Log, JS async and sync routing), the inbox (`kind u8, len u32, payload` records, one mutex,
  wake through a posted function), `CallScope` (the JS thread inside a host function), `shutdown`, one
  running host per process, counters.
* `cpp/UndraJsi.{h,cpp}`: `Binding::install` (`__undraNative`'s host functions), the drain (outermost
  only, 1000 rounds, then a re-post), `CoreBuffer` (an `UndraBuf` as an `ArrayBuffer`, freed once),
  `JsAnswerer` (JS sync ports), reload handling (no JSI value held; a stale core stopped on `start`).
* `cpp/UndraTurboModule.{h,cpp}`, `ios/UndraModuleProvider.mm`: the `UndraNative` TurboModule.
* `cpp/UndraFrameSource.{h,cpp}` (AChoreographer, Android), `ios/UndraFrameSource.mm` (CADisplayLink).
* `src/`: `NativeTransport` (`transport.ts`), `loadNative` / `installNative` (`load.ts`),
  `nativeFrameScheduler` (`frame.ts`), `reactNativeAdapters` (`AppState` lifecycle), the
  `UndraNativeModule` type and `portPlan` (`native.ts`), the text codecs and their side-effect-only
  installer (`text-codec.ts`, `polyfills.ts`), the codegen spec (`specs/NativeUndra.ts`).
* `UndraReactNative.podspec`, `android/CMakeLists.txt`, `react-native.config.cjs` (a pure C++
  dependency), `codegenConfig` in `package.json`, `babel-plugin.cjs` (`import.meta` for Hermes).
* Tests: `test/*.test.ts` (36, a fake module), `test/contract/` (the contract scenarios through
  `NativeTransport` over `WasmNative`, a stand-in on the wasm core: 17 pass, S17 app-tested),
  `cpp/test/host_test.cpp` + `run.sh` (13 checks against the real core, per shim, ASan + UBSan).

**`crates/undra-cli`**: `undra build --platform rn` (`builds/rn.rs`): the iOS and Android builds and
`build/ios/UndraCore.podspec` (vendored XCFramework; `-force_load` of the SDK's slice through the pod
setting `UNDRA_CORE_SLICE`, because a clean Xcode build refuses a linker input under
`PODS_XCFRAMEWORKS_BUILD_DIR` that does not exist yet). Tests for the podspec and slice names.

**`contract-tests/ts`**: S03 accepts mode `native` next to `wasm-main`; S12 waits for the GET of a
stale observer and of `refetch()` instead of expecting it before the call resumes (the Swift and
Kotlin columns do the same). TS column 18/18.

**`examples/playground/rn`** (new): a bare React Native 0.87 app (TypeScript, Hermes, New
Architecture): Todos and 10k-list screens over the generated bindings and `@undra/runtime/react`, a
Bench screen with on-device checks RN01..RN10 and the ADR-038 measurements, `UNDRA-RN` log lines;
Metro serves the runtime, the module and the bindings from their sources; Podfile with the `UndraCore`
pod; Gradle `jniLibs` from `build/android`.

**Docs**: ADR-038 (decision, measurements, what is tested where, ADR-044 transitional notes),
`docs/REACT_NATIVE.md`, SPEC §0, §11, §11.1, §11.2 (new), §13, §17.4 (new).

## Proof

Release core (`undra build -C examples/playground --platform rn --release`), release apps:
* iPhone 17 Pro simulator (iOS 26.5): `UNDRA-RN CHECKS 10/10 passed`; Todos seeded through the core
  (`todos=3 visible=3 remaining=2 first.done=true`); 10k list insert and update applied in 7.6 and
  8.2 ms; Bench screen 10/10. Debug core: the same 10/10.
* Android emulator (arm64, API 35, a dedicated AVD `undra-rn`: the `undra` AVD was in use by another
  agent): `CHECKS 10/10`, a to-do typed through the UI, list operations, the stream switch updating
  rows, native vsync source confirmed. A debug build on Metro reloaded mid-benchmark: the core was
  shut down with the runtime and a fresh one started in the same process (`CHECKS 10/10` twice).
Numbers: ADR-038, Consequences.

## Deviations and notes

* `requestAnimationFrame` measured at 60 Hz on both (React Native's timers tick on the display); the
  native vsync source was kept as the contract (ADR-038 decision 6, alternatives).
* Ports are registered before `undra_init` (the Swift host registers after); a second Undra host of
  another language in the same process is unsupported until ADR-044.
* Hermes per-call costs are the TypeScript runtime's JavaScript: `encodeCall` alone is 4.2 us of a
  6 us `callSync` on the simulator; JSI plus the core is 0.23 us. A follow-up for `@undra/runtime`.
* Machine changes: CocoaPods 1.17.0 installed with Homebrew; Android SDK `platforms;android-37.0`,
  `build-tools;37.0.0`, `cmake;3.22.1` installed with `sdkmanager`; AVD `undra-rn` created (a copy of
  `undra`'s configuration).
