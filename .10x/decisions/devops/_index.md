# DevOps — index

Local toolchain on Shrey's Mac (installed 2026-09-30): rustup stable 1.98.1 with
aarch64-apple-ios(-sim), aarch64/x86_64-linux-android, wasm32-unknown-unknown; JDK 17
(brew, keg-only), Kotlin 2.4.20, Gradle 9.8, binaryen 133; kotlinx-coroutines-core-jvm
1.6.4 at ../.tools/lib. Full Xcode.app present; xcode-select needs sudo so env.sh routes
via DEVELOPER_DIR. Missing until the playground step: Android commandlinetools + NDK r27
+ AVD, cargo-ndk. CI workflows: none yet (piece 7).
