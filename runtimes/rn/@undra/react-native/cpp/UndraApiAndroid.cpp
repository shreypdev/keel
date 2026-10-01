// The shim of a core opened at run time (Android, and every build with `UNDRA_RN_DLOPEN`, such as
// the host tests); see UndraApi.h.
//
// The core `<namespace>` is the APK's `lib<namespace>.so` (`undra build --platform rn` or android,
// packaged from build/android/jniLibs). It is opened with `dlopen` and its one export,
// `<namespace>_undra_api`, found with `dlsym`. When the app's Kotlin side has loaded the same library
// already (`System.loadLibrary`), `dlopen` of the same name returns that handle (ADR-044, risks).
// The library stays loaded for the life of the process: the core's threads and every callback it
// was given live in it, so it is never `dlclose`d. No JNI: `dlopen` does not run the library's
// `JNI_OnLoad`, and this module registers no natives.
#if !defined(__APPLE__) || defined(UNDRA_RN_DLOPEN)

#include <dlfcn.h>

#include "UndraApi.h"

/// Where the libraries are: empty on Android (the dynamic linker searches the app's native library
/// directory); a directory ending in `/` for the host tests.
#ifndef UNDRA_RN_LIBRARY_DIR
#define UNDRA_RN_LIBRARY_DIR ""
#endif

/// The file name suffix of a library: `.so` on Android and Linux, `.dylib` for the macOS host tests.
#ifndef UNDRA_RN_LIBRARY_SUFFIX
#define UNDRA_RN_LIBRARY_SUFFIX ".so"
#endif

namespace undra::rn {

const void *findTable(const std::string &name_space, std::string &where, std::string &error) {
  const std::string library = std::string(UNDRA_RN_LIBRARY_DIR) + "lib" + name_space + UNDRA_RN_LIBRARY_SUFFIX;
  where = library;
  void *lib = dlopen(library.c_str(), RTLD_NOW | RTLD_LOCAL);
  if (lib == nullptr) {
    const char *why = dlerror();
    error = "cannot load the Undra core `" + name_space + "`, " + library + ": " +
        (why != nullptr ? why : "unknown error") +
        ". Build it with `undra build --platform rn` (or android) and package build/android/jniLibs with "
        "the app (docs/REACT_NATIVE.md).";
    return nullptr;
  }
  const std::string symbol = name_space + "_undra_api";
  dlerror();
  void *entry = dlsym(lib, symbol.c_str());
  if (entry == nullptr) {
    error = library + " is not the Undra core `" + name_space + "`: it does not export " + symbol;
    dlclose(lib); // nothing of it was used
    return nullptr;
  }
  using ApiFunction = const void *(*)(void);
  return reinterpret_cast<ApiFunction>(entry)();
}

} // namespace undra::rn

#endif
