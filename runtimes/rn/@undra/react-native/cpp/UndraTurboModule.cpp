#include "UndraTurboModule.h"

#include <string>

#include "UndraApi.h"
#include "UndraJsi.h"

namespace facebook::react {

UndraTurboModule::UndraTurboModule(std::shared_ptr<CallInvoker> jsInvoker)
    : TurboModule(kModuleName, std::move(jsInvoker)) {
  methodMap_["install"] = MethodMetadata{0, &UndraTurboModule::install};
}

UndraTurboModule::~UndraTurboModule() {
  if (binding_) {
    binding_->detach();
  }
}

jsi::Value UndraTurboModule::install(jsi::Runtime &rt, TurboModule &module, const jsi::Value * /*args*/, size_t /*count*/) {
  auto &self = static_cast<UndraTurboModule &>(module);
  if (!self.binding_) {
    std::string error;
    const undra::rn::Api *api = undra::rn::loadApi(error);
    if (api == nullptr) {
      throw jsi::JSError(rt, "@undra/react-native: " + error);
    }
    self.binding_ = std::make_shared<undra::rn::Binding>(*api, self.jsInvoker_);
  }
  self.binding_->install(rt);
  return jsi::Value(true);
}

} // namespace facebook::react
