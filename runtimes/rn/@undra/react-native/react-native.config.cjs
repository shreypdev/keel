// Autolinking of @undra/react-native (ADR-038, decision 1).
//
// iOS: `UndraReactNative.podspec`, found by the React Native CLI; the module is registered through
// `codegenConfig.ios.modules` in package.json (`UndraModuleProvider`).
// Android: a pure C++ dependency. There is no Gradle project: React Native's Gradle plugin runs this
// package's codegen for the app and adds `android/CMakeLists.txt` (target `undra_react_native`) to
// the app's `libappmodules.so`, whose autolinking instantiates `UndraTurboModule` (the header name).
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
