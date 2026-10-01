// The shim of a core linked into the app (iOS, and the host tests): the only file of the iOS build
// that names an `undra_*` symbol (see UndraApi.h, and ADR-044 for the migration to the table).
#if !defined(__ANDROID__) && !defined(UNDRA_RN_DLOPEN)

#include <mutex>

#include "UndraApi.h"

namespace undra::rn {

const Api *loadApi(std::string &error) {
  static Api api;
  static std::string failure;
  static std::once_flag once;
  std::call_once(once, [] {
    Api linked;
    linked.abi_version = undra_abi_version();
    linked.schema_hash = undra_schema_hash();
    linked.schema_json = &undra_schema_json;
    linked.init = &undra_init;
    linked.shutdown = &undra_shutdown;
    linked.call = &undra_call;
    linked.call_sync = &undra_call_sync;
    linked.cancel = &undra_cancel;
    linked.stream_credit = &undra_stream_credit;
    linked.observe = &undra_observe;
    linked.release = &undra_release;
    linked.port_register = &undra_port_register;
    linked.port_reply = &undra_port_reply;
    linked.event = &undra_event;
    linked.timer_fired = &undra_timer_fired;
    linked.snapshot = &undra_snapshot;
    linked.restore = &undra_restore;
    linked.stats_json = &undra_stats_json;
    linked.buf_free = &undra_buf_free;
    if (linked.abi_version != kAbiVersion) {
      failure = "the linked Undra core speaks C ABI " + std::to_string(linked.abi_version) + ", this module speaks " +
          std::to_string(kAbiVersion) + ": rebuild the core and the app with the same Undra version";
      return;
    }
    api = linked;
  });
  if (!failure.empty()) {
    error = failure;
    return nullptr;
  }
  return &api;
}

} // namespace undra::rn

#endif
