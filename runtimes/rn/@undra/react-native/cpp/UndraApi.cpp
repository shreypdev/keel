// What both shims do with a core's table once they have it (see UndraApi.h): check it, copy it.
#include "UndraApi.h"

#include <cstring>
#include <map>
#include <memory>
#include <mutex>
#include <utility>

namespace undra::rn {

bool validNamespace(const std::string &name_space) noexcept {
  if (name_space.empty() || name_space.size() > kMaxNamespace) {
    return false;
  }
  for (std::size_t i = 0; i < name_space.size(); ++i) {
    const char c = name_space[i];
    const bool letter = (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c == '_';
    const bool digit = c >= '0' && c <= '9';
    if (!(letter || (digit && i > 0))) {
      return false;
    }
  }
  return true;
}

namespace {

/// Copies one entry of the table; records the first missing one in `missing`.
template <typename F>
void entry(F from, F &slot, const char *name, const char *&missing) {
  if (from == nullptr && missing == nullptr) {
    missing = name;
  }
  slot = from;
}

} // namespace

bool copyTable(const void *table, const std::string &name_space, const std::string &where, Api &out, std::string &error) {
  if (table == nullptr) {
    error = where + " returned no table for the Undra core `" + name_space + "`";
    return false;
  }
  // The version first, and nothing else of a table of another version: only `abi_version` is
  // where every version puts it.
  uint32_t abi = 0;
  std::memcpy(&abi, table, sizeof(abi));
  if (abi != kAbiVersion) {
    error = "the Undra core `" + name_space + "` (" + where + ") speaks C ABI " + std::to_string(abi) +
        ", this module speaks " + std::to_string(kAbiVersion) +
        ": rebuild the core and the app with the same Undra version";
    return false;
  }
  const auto *api = static_cast<const UndraApi *>(table);
  if (api->size < sizeof(UndraApi)) {
    error = "the Undra core `" + name_space + "` (" + where + ") has a table of " + std::to_string(api->size) +
        " bytes, shorter than the " + std::to_string(sizeof(UndraApi)) + " of C ABI " + std::to_string(kAbiVersion) +
        ": rebuild the core and the app with the same Undra version";
    return false;
  }
  const char *declared = api->name_space;
  if (declared == nullptr || name_space != declared) {
    error = std::string(where) + " is the Undra core `" + (declared != nullptr ? declared : "") +
        "`, not `" + name_space + "`: a core's namespace names its class and its library (undra.toml, [core] namespace)";
    return false;
  }
  Api copied;
  copied.abi_version = api->abi_version;
  copied.schema_hash = api->schema_hash;
  copied.name_space = name_space;
  const char *missing = nullptr;
  entry(api->schema_json, copied.schema_json, "schema_json", missing);
  entry(api->init, copied.init, "init", missing);
  entry(api->shutdown, copied.shutdown, "shutdown", missing);
  entry(api->call, copied.call, "call", missing);
  entry(api->call_sync, copied.call_sync, "call_sync", missing);
  entry(api->cancel, copied.cancel, "cancel", missing);
  entry(api->stream_credit, copied.stream_credit, "stream_credit", missing);
  entry(api->observe, copied.observe, "observe", missing);
  entry(api->release, copied.release, "release", missing);
  entry(api->port_register, copied.port_register, "port_register", missing);
  entry(api->port_reply, copied.port_reply, "port_reply", missing);
  entry(api->event, copied.event, "event", missing);
  entry(api->timer_fired, copied.timer_fired, "timer_fired", missing);
  entry(api->snapshot, copied.snapshot, "snapshot", missing);
  entry(api->restore, copied.restore, "restore", missing);
  entry(api->stats_json, copied.stats_json, "stats_json", missing);
  entry(api->buf_free, copied.buf_free, "buf_free", missing);
  if (missing != nullptr) {
    error = "the table of the Undra core `" + name_space + "` (" + where + ") has no `" + missing + "` entry";
    return false;
  }
  out = std::move(copied);
  return true;
}

const Api *loadApi(const std::string &name_space, std::string &error) {
  if (!validNamespace(name_space)) {
    error = "`" + name_space + "` is not an Undra core namespace (a C identifier of at most " +
        std::to_string(kMaxNamespace) + " characters, undra.toml [core] namespace)";
    return nullptr;
  }
  // Never destroyed: a `Host` and every `ArrayBuffer` that owns an `UndraBuf` hold an `Api &`, and
  // JavaScript may release one while the process exits.
  static std::mutex *mutex = new std::mutex;
  static auto *cores = new std::map<std::string, std::unique_ptr<const Api>>;
  std::lock_guard<std::mutex> lock(*mutex);
  auto found = cores->find(name_space);
  if (found != cores->end()) {
    return found->second.get();
  }
  std::string where;
  const void *table = findTable(name_space, where, error);
  if (table == nullptr && !error.empty()) {
    return nullptr;
  }
  auto api = std::make_unique<Api>();
  if (!copyTable(table, name_space, where, *api, error)) {
    return nullptr;
  }
  const Api *kept = api.get();
  cores->emplace(name_space, std::move(api));
  return kept;
}

} // namespace undra::rn
