// The C ABI of docs/SPEC.md section 6 as one struct, resolved once per process.
//
// Every reference to an `undra_*` symbol lives in one shim file per platform:
//   * `UndraApiLinked.cpp` (iOS, and the host tests): the core is linked into the app; the struct
//     holds the linked symbols (taking their addresses also keeps a dead-stripping linker from
//     dropping them);
//   * `UndraApiAndroid.cpp`: the core is the APK's `libundra_core.so`, opened with `dlopen` and
//     resolved with `dlsym` the first time the module starts.
// Nothing else in the module names a symbol of the core: it calls through `Api`.
//
// Transitional (ADR-038, decisions 1, 11 and 13; ADR-044): when ADR-044's `abi-table` piece lands, a
// core exports one `<namespace>_undra_api()` returning a table of the same operations (C ABI v2).
// The migration is these shim files alone: `loadApi(namespace)` calls that function (iOS: the core's
// header; Android: `dlsym` on `lib<namespace>.so`), checks `abi_version` and copies the table's
// pointers into the same `Api`; `Host` and the JSI binding do not change.
#pragma once

#include <string>

#include "undra.h"

namespace undra::rn {

/// The C ABI version this module speaks.
inline constexpr uint32_t kAbiVersion = 1;

/// The core's operations (SPEC 6), with the two constants read once when the shim resolved them.
struct Api {
  /// `undra_abi_version()`, read at load; `loadApi` refuses a core whose value is not `kAbiVersion`.
  uint32_t abi_version = 0;
  /// `undra_schema_hash()`, read at load (it needs no running core).
  uint64_t schema_hash = 0;
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

/// The core of this process, resolved once (see the file comment). `nullptr` with `error` set when
/// the library or one of its symbols cannot be found, or when it speaks another ABI version.
const Api *loadApi(std::string &error);

} // namespace undra::rn
