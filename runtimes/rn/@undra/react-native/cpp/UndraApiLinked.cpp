// The shim of a core linked into the app (iOS, and the macOS host tests); see UndraApi.h.
//
// The module is built once for every core of the app, so it cannot name a core's symbol. Each core's
// pod (`undra build --platform rn`: `<Bundle>.podspec`, `<Bundle>Table.m`) compiles one Objective-C
// class, `UndraCoreTable_<namespace>`, whose class method `+ (const void *)api` returns the core's
// table, and links with `-ObjC`, which keeps the class (and through it the core's one prelinked
// object) in the app. Looking the class up by name is how the module finds the core JavaScript asked
// for. This file is C++, so it talks to the Objective-C runtime through its C functions.
#if defined(__APPLE__) && !defined(UNDRA_RN_DLOPEN)

#include <objc/message.h>
#include <objc/runtime.h>

#include "UndraApi.h"

namespace undra::rn {

const void *findTable(const std::string &name_space, std::string &where, std::string &error) {
  const std::string className = "UndraCoreTable_" + name_space;
  where = "the class " + className;
  Class cls = objc_getClass(className.c_str());
  if (cls == nullptr) {
    error = "no Undra core `" + name_space + "` is linked into this app (there is no class " + className +
        "). Build it with `undra build --platform rn` and add its pod to the Podfile, " +
        "`pod '<Bundle>', :path => '<build>/ios'`, then `pod install` (docs/REACT_NATIVE.md)";
    return nullptr;
  }
  SEL api = sel_registerName("api");
  if (class_getClassMethod(cls, api) == nullptr) {
    error = "the class " + className + " of the Undra core `" + name_space +
        "` has no `+api` method: rebuild the core with `undra build --platform rn`";
    return nullptr;
  }
  // `objc_msgSend` must be called through a cast to the method's exact type (`+ (const void *)api`).
  using ApiMethod = const void *(*)(id, SEL);
  return reinterpret_cast<ApiMethod>(&objc_msgSend)(reinterpret_cast<id>(cls), api);
}

} // namespace undra::rn

#endif
