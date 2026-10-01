// The pure C++ TurboModule `UndraNative` (ADR-038, decision 1).
//
// Its one method, `install(namespace)`, loads the core of that namespace (ADR-044) and puts
// `globalThis.__undraNative[namespace]` into the calling runtime; everything else goes through that
// object's JSI host functions. One module serves every core of the app, one binding per core. The class name is also
// the header name and lives in `facebook::react`, because React Native's Android autolinking
// instantiates `cxxModuleHeaderName` there (`react-native.config.js`); iOS reaches it through
// `UndraModuleProvider` (`ios/UndraModuleProvider.mm`, `codegenConfig.ios.modules`).
#pragma once

#include <ReactCommon/TurboModule.h>
#include <jsi/jsi.h>

#include <map>
#include <memory>
#include <string>

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
  /// The JS runtime is being torn down (a reload): the cores this runtime started are shut down.
  ~UndraTurboModule() override;

 private:
  static jsi::Value install(jsi::Runtime &rt, TurboModule &module, const jsi::Value *args, size_t count);

  /// One binding per core this runtime installed, by namespace. Touched on the JS thread only
  /// (`install`), and by the destructor once nothing calls the module any more.
  std::map<std::string, std::shared_ptr<undra::rn::Binding>> bindings_;
};

} // namespace facebook::react
