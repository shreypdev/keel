// The pure C++ TurboModule `UndraNative` (ADR-038, decision 1).
//
// Its one method, `install()`, loads the core and puts `globalThis.__undraNative` into the calling
// runtime; everything else goes through that object's JSI host functions. The class name is also
// the header name and lives in `facebook::react`, because React Native's Android autolinking
// instantiates `cxxModuleHeaderName` there (`react-native.config.js`); iOS reaches it through
// `UndraModuleProvider` (`ios/UndraModuleProvider.mm`, `codegenConfig.ios.modules`).
#pragma once

#include <ReactCommon/TurboModule.h>
#include <jsi/jsi.h>

#include <memory>

namespace undra::rn {
class Binding;
}

namespace facebook::react {

/// The `UndraNative` TurboModule.
class UndraTurboModule : public TurboModule {
 public:
  /// The name JavaScript asks `TurboModuleRegistry` for.
  static constexpr const char *kModuleName = "UndraNative";

  explicit UndraTurboModule(std::shared_ptr<CallInvoker> jsInvoker);
  /// The JS runtime is being torn down (a reload): the core this runtime started is shut down.
  ~UndraTurboModule() override;

 private:
  static jsi::Value install(jsi::Runtime &rt, TurboModule &module, const jsi::Value *args, size_t count);

  std::shared_ptr<undra::rn::Binding> binding_;
};

} // namespace facebook::react
