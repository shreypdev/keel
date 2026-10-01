#include "UndraTurboModule.h"

#include <string>

#include "UndraApi.h"
#include "UndraJsi.h"

namespace facebook::react {

UndraTurboModule::UndraTurboModule(std::shared_ptr<CallInvoker> jsInvoker)
    : TurboModule(kModuleName, std::move(jsInvoker)) {
  methodMap_["install"] = MethodMetadata{1, &UndraTurboModule::install};
}

UndraTurboModule::~UndraTurboModule() {
  for (auto &[name_space, binding] : bindings_) {
    binding->detach();
  }
}

jsi::Value UndraTurboModule::install(jsi::Runtime &rt, TurboModule &module, const jsi::Value *args, size_t count) {
  auto &self = static_cast<UndraTurboModule &>(module);
  if (count < 1 || !args[0].isString()) {
    throw jsi::JSError(rt, "@undra/react-native: install(namespace) needs the core's namespace, a string");
  }
  const std::string name_space = args[0].getString(rt).utf8(rt);
  std::shared_ptr<undra::rn::Binding> &binding = self.bindings_[name_space];
  if (!binding) {
    std::string error;
    const undra::rn::Api *api = undra::rn::loadApi(name_space, error);
    if (api == nullptr) {
      self.bindings_.erase(name_space);
      throw jsi::JSError(rt, "@undra/react-native: " + error);
    }
    binding = std::make_shared<undra::rn::Binding>(*api, self.jsInvoker_);
  }
  binding->install(rt);
  return jsi::Value(true);
}

} // namespace facebook::react
