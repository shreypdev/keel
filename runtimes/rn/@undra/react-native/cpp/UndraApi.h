// The C ABI of docs/SPEC.md section 6 as one struct per core, resolved once per namespace.
//
// A core exports one function, `<namespace>_undra_api()`, returning its `UndraApi` table (C ABI
// version 2, ADR-044). Every way of reaching that function lives in one shim file per platform:
//   * `UndraApiLinked.cpp` (iOS, and the macOS host tests): the core is linked into the app, and the
//     module, which is built once for every core of the app, finds it by its namespace through the
//     Objective-C class `UndraCoreTable_<namespace>` that the core's pod compiles (`+api` returns the
//     table; `undra build --platform rn` writes it);
//   * `UndraApiAndroid.cpp` (Android, and every build with `UNDRA_RN_DLOPEN`): the core is the APK's
//     `lib<namespace>.so`, opened with `dlopen`, its table found with `dlsym("<namespace>_undra_api")`.
// A shim only finds the table (`findTable`); `loadApi` (UndraApi.cpp, shared) caches one `Api` per
// namespace and hands each new table to `copyTable`, which checks it (its `abi_version` first, then
// its size, its namespace and every entry) and copies its pointers. Nothing else in the module names
// a core: `Host` and the JSI binding call through `Api`.
#pragma once

#include <cstdint>
#include <string>

#include "undra.h"

namespace undra::rn {

/// The C ABI version this module speaks: the `abi_version` of every table it accepts.
inline constexpr uint32_t kAbiVersion = UNDRA_ABI_VERSION;
static_assert(kAbiVersion == 2, "this module is written against the UndraApi table of C ABI version 2");

/// The longest namespace a core may have (ADR-044, decision 1).
inline constexpr std::size_t kMaxNamespace = 32;

/// One core's operations (SPEC 6), copied out of its table, with the table's constants.
struct Api {
  /// The table's `abi_version`; `loadApi` refuses a table whose value is not `kAbiVersion`.
  uint32_t abi_version = 0;
  /// The table's `schema_hash` (it needs no running core).
  uint64_t schema_hash = 0;
  /// The core's namespace (`[core] namespace` of its undra.toml): the table's `name_space`.
  std::string name_space;
  UndraBuf (*schema_json)(void) = nullptr;
  uint32_t (*init)(const uint8_t *, uint32_t, undra_reply_cb, undra_changeset_cb, undra_stream_cb, void *) = nullptr;
  void (*shutdown)(void) = nullptr;
  uint32_t (*call)(const uint8_t *, uint32_t) = nullptr;
  UndraBuf (*call_sync)(const uint8_t *, uint32_t) = nullptr;
  void (*cancel)(uint32_t) = nullptr;
  void (*stream_credit)(uint32_t, uint32_t) = nullptr;
  void (*observe)(uint64_t, uint32_t, uint8_t) = nullptr;
  void (*release)(uint64_t) = nullptr;
  void (*port_register)(uint32_t, undra_port_cb, void *) = nullptr;
  void (*port_reply)(const uint8_t *, uint32_t) = nullptr;
  void (*event)(uint32_t, uint32_t, const uint8_t *, uint32_t) = nullptr;
  void (*timer_fired)(uint32_t) = nullptr;
  UndraBuf (*snapshot)(void) = nullptr;
  uint32_t (*restore)(const uint8_t *, uint32_t) = nullptr;
  UndraBuf (*stats_json)(void) = nullptr;
  void (*buf_free)(UndraBuf) = nullptr;
};

/// The core named `name_space`, resolved the first time it is asked for and then kept for the life
/// of the process (a core is never unloaded: its threads and every callback it was given live in
/// it). Thread-safe. `nullptr` with `error` set when the name is not a namespace, when no such core
/// is in the app, or when its table is refused (another ABI version, another namespace, too short,
/// an entry missing); a failure is not cached, so a later call tries again.
const Api *loadApi(const std::string &name_space, std::string &error);

/// The platform's half (UndraApiLinked.cpp or UndraApiAndroid.cpp): the table of the core
/// `name_space` (already a valid namespace), with what it came from in `where` (the class or the
/// library, for messages). `nullptr` with `error` set when the app has no such core. Called once
/// per namespace at a time, under `loadApi`'s lock.
const void *findTable(const std::string &name_space, std::string &where, std::string &error);

/// Whether `name_space` can name a core: a C identifier (`[A-Za-z_][A-Za-z0-9_]*`) of at most
/// `kMaxNamespace` characters (ADR-044, decision 1). It becomes part of a class or library name.
bool validNamespace(const std::string &name_space) noexcept;

/// Checks the table `table` that `where` (the class or the library it came from) returned for the
/// core `name_space`, and copies it into `out`. Reads `abi_version` first and nothing else of a
/// table of another version; then requires `size >= sizeof(UndraApi)`, `name_space` equal to the
/// one asked for, and every entry. `false` with `error` set when it refuses the table.
bool copyTable(const void *table, const std::string &name_space, const std::string &where, Api &out, std::string &error);

} // namespace undra::rn
