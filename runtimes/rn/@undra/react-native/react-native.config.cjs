// Autolinking of @undra/react-native (ADR-038, decision 1).
//
// iOS: `UndraReactNative.podspec`, found by the React Native CLI; the module is registered through
// `codegenConfig.ios.modules` in package.json (`UndraModuleProvider`).
// Android: a Java library (`android/build.gradle`, the default ports' Java half, ADR-038 amendment B,
// B8) and the C++ TurboModule: React Native's Gradle plugin adds `android/CMakeLists.txt` (target
// `undra_react_native`) to the app's `libappmodules.so`, whose autolinking instantiates
// `UndraTurboModule` (the header name), as it does for a pure C++ dependency.
module.exports = {
  dependency: {
    platforms: {
      android: {
        cxxModuleCMakeListsModuleName: "undra_react_native",
        cxxModuleCMakeListsPath: "CMakeLists.txt",
        cxxModuleHeaderName: "UndraTurboModule",
      },
    },
  },
};
