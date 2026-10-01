// The C ABI of docs/SPEC.md section 6 as a table of function pointers.
//
// On iOS (and in the host tests) the core is linked into the app and the table holds the linked
// symbols; taking their addresses is also what keeps a dead-stripping linker from dropping them. On
// Android the core is the APK's `libundra_core.so`, opened with `dlopen` the first time the module
// starts, so the module needs no link-time knowledge of the app's core (ADR-038, decision 1).
#pragma once

#include <string>

#include "undra.h"

namespace undra::rn {

/// The 19 functions of `undra.h`, one pointer each.
struct Api {
  uint32_t (*abi_version)(void);
  uint64_t (*schema_hash)(void);
  UndraBuf (*schema_json)(void);
  uint32_t (*init)(const uint8_t *, uint32_t, undra_reply_cb, undra_changeset_cb, undra_stream_cb, void *);
  void (*shutdown)(void);
  uint32_t (*call)(const uint8_t *, uint32_t);
  UndraBuf (*call_sync)(const uint8_t *, uint32_t);
  void (*cancel)(uint32_t);
  void (*stream_credit)(uint32_t, uint32_t);
  void (*observe)(uint64_t, uint32_t, uint8_t);
  void (*release)(uint64_t);
  void (*port_register)(uint32_t, undra_port_cb, void *);
  void (*port_reply)(const uint8_t *, uint32_t);
  void (*event)(uint32_t, uint32_t, const uint8_t *, uint32_t);
  void (*timer_fired)(uint32_t);
  UndraBuf (*snapshot)(void);
  uint32_t (*restore)(const uint8_t *, uint32_t);
  UndraBuf (*stats_json)(void);
  void (*buf_free)(UndraBuf);
};

/// The core's table: the linked core, or the `dlopen`ed `libundra_core.so` on Android (or wherever
/// `UNDRA_RN_DLOPEN` is defined). Loaded once per process; `nullptr` with `error` set when the
/// library or one of its symbols cannot be found.
const Api *loadApi(std::string &error);

} // namespace undra::rn
