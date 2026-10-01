#include "UndraApi.h"

#include <mutex>

#if defined(__ANDROID__) || defined(UNDRA_RN_DLOPEN)
#include <dlfcn.h>
#define UNDRA_RN_USE_DLOPEN 1
#endif

namespace undra::rn {

#if defined(UNDRA_RN_USE_DLOPEN)

namespace {

#ifndef UNDRA_RN_CORE_LIBRARY
#define UNDRA_RN_CORE_LIBRARY "libundra_core.so"
#endif

/// Resolves `name` in `lib` into `slot`; records the first missing symbol in `missing`.
template <typename F>
void resolve(void *lib, const char *name, F &slot, std::string &missing) {
  void *symbol = dlsym(lib, name);
  if (symbol == nullptr && missing.empty()) {
    missing = name;
  }
  slot = reinterpret_cast<F>(symbol);
}

Api g_api{};
std::string g_error;
bool g_loaded = false;
std::once_flag g_once;

void loadOnce() {
  // The library stays loaded for the life of the process: the core's threads and every callback
  // the host registered live in it, so it is never `dlclose`d.
  void *lib = dlopen(UNDRA_RN_CORE_LIBRARY, RTLD_NOW | RTLD_LOCAL);
  if (lib == nullptr) {
    const char *why = dlerror();
    g_error = std::string("cannot load the Undra core ") + UNDRA_RN_CORE_LIBRARY + ": " +
        (why != nullptr ? why : "unknown error") +
        ". Build it with `undra build --platform rn` (or android) and package build/android/jniLibs "
        "with the app (docs/REACT_NATIVE.md).";
    return;
  }
  std::string missing;
  resolve(lib, "undra_abi_version", g_api.abi_version, missing);
  resolve(lib, "undra_schema_hash", g_api.schema_hash, missing);
  resolve(lib, "undra_schema_json", g_api.schema_json, missing);
  resolve(lib, "undra_init", g_api.init, missing);
  resolve(lib, "undra_shutdown", g_api.shutdown, missing);
  resolve(lib, "undra_call", g_api.call, missing);
  resolve(lib, "undra_call_sync", g_api.call_sync, missing);
  resolve(lib, "undra_cancel", g_api.cancel, missing);
  resolve(lib, "undra_stream_credit", g_api.stream_credit, missing);
  resolve(lib, "undra_observe", g_api.observe, missing);
  resolve(lib, "undra_release", g_api.release, missing);
  resolve(lib, "undra_port_register", g_api.port_register, missing);
  resolve(lib, "undra_port_reply", g_api.port_reply, missing);
  resolve(lib, "undra_event", g_api.event, missing);
  resolve(lib, "undra_timer_fired", g_api.timer_fired, missing);
  resolve(lib, "undra_snapshot", g_api.snapshot, missing);
  resolve(lib, "undra_restore", g_api.restore, missing);
  resolve(lib, "undra_stats_json", g_api.stats_json, missing);
  resolve(lib, "undra_buf_free", g_api.buf_free, missing);
  if (!missing.empty()) {
    g_error = std::string(UNDRA_RN_CORE_LIBRARY) + " is not an Undra core: it does not export " + missing;
    return;
  }
  g_loaded = true;
}

} // namespace

const Api *loadApi(std::string &error) {
  std::call_once(g_once, loadOnce);
  if (!g_loaded) {
    error = g_error;
    return nullptr;
  }
  return &g_api;
}

#else

const Api *loadApi(std::string & /*error*/) {
  static const Api linked{
      &undra_abi_version,
      &undra_schema_hash,
      &undra_schema_json,
      &undra_init,
      &undra_shutdown,
      &undra_call,
      &undra_call_sync,
      &undra_cancel,
      &undra_stream_credit,
      &undra_observe,
      &undra_release,
      &undra_port_register,
      &undra_port_reply,
      &undra_event,
      &undra_timer_fired,
      &undra_snapshot,
      &undra_restore,
      &undra_stats_json,
      &undra_buf_free,
  };
  return &linked;
}

#endif

} // namespace undra::rn
