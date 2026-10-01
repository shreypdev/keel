// Registers the pure C++ TurboModule `UndraNative` on iOS: React Native's generated
// `RCTModuleProviders` maps the module name to this class through the package's
// `codegenConfig.ios.modules` (ADR-038, decision 1).
#import <Foundation/Foundation.h>
#import <ReactCommon/RCTTurboModule.h>

#include "UndraTurboModule.h"

@interface UndraModuleProvider : NSObject <RCTModuleProvider>
@end

@implementation UndraModuleProvider

- (std::shared_ptr<facebook::react::TurboModule>)getTurboModule:(const facebook::react::ObjCTurboModule::InitParams &)params
{
  return std::make_shared<facebook::react::UndraTurboModule>(params.jsInvoker);
}

@end
