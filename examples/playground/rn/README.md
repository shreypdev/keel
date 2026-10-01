# The Undra playground, React Native

The playground's one Rust core (`../core`) under a bare React Native 0.87 app (TypeScript, Hermes,
New Architecture), through `@undra/react-native` (`runtimes/rn/@undra/react-native`, ADR-038). The
screens use the generated TypeScript bindings (`../generated/ts`) and `@undra/runtime/react`
unchanged, exactly as the web app does.

| Screen | Shows |
|---|---|
| Todos | the `Todos` store: a keyed list, a filter, two computed values, an async command with a typed error |
| 10k list | the `BigList` store: 10,000 keyed rows; every button is a one-operation keyed patch; Stream updates a visible row ten times a second |
| Bench | on-device self-checks of the boundary (RN01..RN10) and the measurements of ADR-038, run once at start and on demand |

Every result is also logged as an `UNDRA-RN ...` line (Xcode console / `xcrun simctl spawn <device>
log stream`, `adb logcat -s ReactNativeJS`).

## Build and run

```sh
# 1. The core for both phones and the UndraCore pod (from the repository root):
cargo build -p undra-cli
target/debug/undra build -C examples/playground --platform rn --release

# 2. JavaScript dependencies (links @undra/react-native and @undra/runtime from this checkout):
cd examples/playground/rn && npm install

# 3a. iOS (CocoaPods; the UndraCore pod comes from ../build/ios):
cd ios && pod install && cd ..
npx react-native run-ios            # or: xcodebuild -workspace ios/UndraPlayground.xcworkspace ...

# 3b. Android (the core's .so files come from ../build/android/jniLibs):
npx react-native run-android        # or: cd android && ./gradlew :app:assembleRelease
```

A debug core works too (`undra build --platform rn` without `--release`); it is much slower, which
the Bench screen shows. Metro (`npm start`) serves the TypeScript sources of the runtime, the module
and the bindings directly (`metro.config.js`), so editing any of them needs no build step.

`docs/REACT_NATIVE.md` is the guide: install, autolinking, the build, the dev loop, the limits.
