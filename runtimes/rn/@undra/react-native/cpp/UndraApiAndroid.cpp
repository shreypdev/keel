// The shim of a core opened at run time (Android): the only file of the Android build that names an
// `undra_*` symbol (see UndraApi.h, and ADR-044 for the migration to the table). The library stays
// loaded for the life of the process: the core's threads and every callback it was given live in it,
// so it is never `dlclose`d. No JNI: `dlopen` does not run the library's `JNI_OnLoad`, and this module
// registers no natives.
#if defined(__ANDROID__) || defined(UNDRA_RN_DLOPEN)

#include <dlfcn.h>

#include <mutex>

#include "UndraApi.h"

#ifndef UNDRA_RN_CORE_LIBRARY
#define UNDRA_RN_CORE_LIBRARY "libundra_core.so"
#endif

namespace undra::rn {

namespace {

/// Resolves `name` in `lib` into `slot`; records the first missing symbol in `missing`.
template <typename F>
void resolve(void *lib, const char *name, F &slot, std::string &missing) {
  void *symbol = dlsym(lib, name);
  if (symbol == nullptr && missing.empty()) {
    missing = name;
  }
  slot = reinterpret_cast<F>(symbol);
}

} // namespace

const Api *loadApi(std::string &error) {
  static Api api;
  static std::string failure;
  static std::once_flag once;
  std::call_once(once, [] {
    void *lib = dlopen(UNDRA_RN_CORE_LIBRARY, RTLD_NOW | RTLD_LOCAL);
    if (lib == nullptr) {
      const char *why = dlerror();
      failure = std::string("cannot load the Undra core ") + UNDRA_RN_CORE_LIBRARY + ": " +
          (why != nullptr ? why : "unknown error") +
          ". Build it with `undra build --platform rn` (or android) and package build/android/jniLibs with "
          "the app (docs/REACT_NATIVE.md).";
      return;
    }
    Api loaded;
    uint32_t (*abi_version)(void) = nullptr;
    uint64_t (*schema_hash)(void) = nullptr;
    std::string missing;
    resolve(lib, "undra_abi_version", abi_version, missing);
    resolve(lib, "undra_schema_hash", schema_hash, missing);
    resolve(lib, "undra_schema_json", loaded.schema_json, missing);
    resolve(lib, "undra_init", loaded.init, missing);
    resolve(lib, "undra_shutdown", loaded.shutdown, missing);
    resolve(lib, "undra_call", loaded.call, missing);
    resolve(lib, "undra_call_sync", loaded.call_sync, missing);
    resolve(lib, "undra_cancel", loaded.cancel, missing);
    resolve(lib, "undra_stream_credit", loaded.stream_credit, missing);
    resolve(lib, "undra_observe", loaded.observe, missing);
    resolve(lib, "undra_release", loaded.release, missing);
    resolve(lib, "undra_port_register", loaded.port_register, missing);
    resolve(lib, "undra_port_reply", loaded.port_reply, missing);
    resolve(lib, "undra_event", loaded.event, missing);
    resolve(lib, "undra_timer_fired", loaded.timer_fired, missing);
    resolve(lib, "undra_snapshot", loaded.snapshot, missing);
    resolve(lib, "undra_restore", loaded.restore, missing);
    resolve(lib, "undra_stats_json", loaded.stats_json, missing);
    resolve(lib, "undra_buf_free", loaded.buf_free, missing);
    if (!missing.empty()) {
      failure = std::string(UNDRA_RN_CORE_LIBRARY) + " is not an Undra core: it does not export " + missing;
      return;
    }
    loaded.abi_version = abi_version();
    loaded.schema_hash = schema_hash();
    if (loaded.abi_version != kAbiVersion) {
      failure = std::string(UNDRA_RN_CORE_LIBRARY) + " speaks C ABI " + std::to_string(loaded.abi_version) +
          ", this module speaks " + std::to_string(kAbiVersion) + ": rebuild the core and the app with the same Undra version";
      return;
    }
    api = loaded;
  });
  if (!failure.empty()) {
    error = failure;
    return nullptr;
  }
  return &api;
}

} // namespace undra::rn

#endif
