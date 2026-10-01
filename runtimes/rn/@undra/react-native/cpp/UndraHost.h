// The React Native host of the C ABI, without JSI (ADR-038, decisions 3, 4 and 7).
//
// `Host` owns one running core: it registers the port callbacks, calls `undra_init` with its three
// callbacks, and turns everything the core says into records in one inbox. It knows nothing about
// JavaScript, so its ownership and threading rules are tested against the real core on the host
// (`cpp/test/host_test.cpp`); `UndraJsi.cpp` puts JSI on top.
//
// The host contract (docs/SPEC.md section 6, `undra.h`, ADR-026) as this file honours it:
//  * every callback is `noexcept`, catches everything, never unwinds into the core (contract 3);
//  * a callback copies the bytes it was lent into the inbox (the one copy) and calls back into the
//    core never (contract 4): the wake function it may call only posts work to the JS thread;
//  * `user` (this object) stays valid until `undra_shutdown` has returned (contract 1): `shutdown()`
//    and the destructor call it before the object can go away;
//  * a synchronous port reply is a `malloc`ed block with `cap = 0`, which the core `free`s (6.3).
#pragma once

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <functional>
#include <memory>
#include <mutex>
#include <unordered_set>
#include <vector>

#include "UndraApi.h"

namespace undra::rn {

/// FNV-1a over a NUL-terminated string (docs/SPEC.md section 1.1): the id of a port or method.
constexpr uint32_t fnv1a32(const char *text) {
  uint32_t hash = 0x811c9dc5u;
  while (*text != '\0') {
    hash ^= static_cast<uint8_t>(*text++);
    hash *= 0x01000193u;
  }
  return hash;
}

/// The ids of the standard ports this host treats specially (docs/SPEC.md section 8).
namespace ports {
inline constexpr uint32_t kClock = fnv1a32("port.Clock");
inline constexpr uint32_t kClockNowMs = fnv1a32("Clock.now_ms");
inline constexpr uint32_t kClockMonotonicNs = fnv1a32("Clock.monotonic_ns");
inline constexpr uint32_t kRng = fnv1a32("port.Rng");
inline constexpr uint32_t kRngFill = fnv1a32("Rng.fill");
inline constexpr uint32_t kLog = fnv1a32("port.Log");
inline constexpr uint32_t kLogLog = fnv1a32("Log.log");
inline constexpr uint32_t kTimer = fnv1a32("port.Timer");
} // namespace ports

/// What an inbox record carries; the values are the envelope kinds of docs/SPEC.md section 3.2.
enum class RecordKind : uint8_t {
  /// A `Reply` payload (3.4).
  Reply = 2,
  /// A `ChangeSet` payload (3.5).
  ChangeSet = 3,
  /// A `PortCall` payload (3.6): `port_id u32, method_id u32, port_call_id u32, args`.
  PortCall = 4,
  /// A `StreamItem` payload (3.7).
  StreamItem = 8,
  /// A log record: `level u8, target String, message String` (the arguments of `Log.log`).
  Log = 13,
};

/// Bytes of a record header: `kind u8, len u32`.
inline constexpr std::size_t kRecordHeader = 5;

/// Start codes of `Host::start` beyond those of `undra_init` (0 ok, 1..5 its `init_code`s).
namespace start_code {
/// Another `Host` of this process is running (one core per process). One that is shutting down
/// on another thread is waited for (at most 5 s) instead.
inline constexpr uint32_t kBusy = 0x100;
/// The linked core speaks another C ABI version.
inline constexpr uint32_t kAbiMismatch = 0x101;
/// `start` was called on a host that is already running.
inline constexpr uint32_t kAlreadyStarted = 0x102;
} // namespace start_code

/// Answers a synchronous port method implemented in JavaScript. Called only on the JS thread, from
/// inside a host function (`CallScope`), while the core may hold its lock.
class SyncPortAnswerer {
 public:
  virtual ~SyncPortAnswerer() = default;
  /// Fills `reply` with a complete `PortReply` payload and returns 0, or returns 2 (unavailable).
  virtual uint8_t answer(
      uint32_t portId,
      uint32_t methodId,
      uint32_t portCallId,
      const uint8_t *args,
      uint32_t len,
      std::vector<uint8_t> &reply) noexcept = 0;
};

class Host;

/// Marks the calling thread (the JS thread) as being inside a host function of `host` for its
/// lifetime. Callbacks the core makes on this thread meanwhile do not wake the JS thread (the host
/// function drains the inbox before it returns) and may run a JavaScript sync port through
/// `answerer`. Scopes nest (a sync port's JavaScript may call a host function).
class CallScope {
 public:
  CallScope(Host &host, SyncPortAnswerer *answerer) noexcept;
  ~CallScope();
  CallScope(const CallScope &) = delete;
  CallScope &operator=(const CallScope &) = delete;

  /// The innermost scope of this thread, or null.
  static CallScope *current() noexcept;
  /// Whether this thread is running a JavaScript sync port of `host` right now (inside a callback,
  /// so possibly under the core lock): the JSI layer neither drains nor shuts down then.
  static bool inCallback(const Host &host) noexcept;

  /// The host this scope belongs to.
  Host &host;
  /// Who runs JavaScript sync ports on this thread; null when none can be run.
  SyncPortAnswerer *answerer;

 private:
  friend class Host;
  CallScope *previous_;
  bool inCallback_ = false;
};

/// A port the host registers: its id and the ids of its synchronous methods (from the schema).
struct PortSpec {
  uint32_t portId = 0;
  std::vector<uint32_t> syncMethods;
};

class NativeDefaults;
class Platform;

/// What `Host::start` is given besides the configuration and the schema's ports.
struct StartOptions {
  /// The standard ports the module answers natively (ADR-038 amendment B): ids of `Kv`,
  /// `SecureStore`, `Fs` and `Connectivity` that `platform` supports. The others stay JavaScript's.
  std::vector<uint32_t> nativePorts;
  /// The phone's platform; null for none (every port is then JavaScript's, as before).
  Platform *platform = nullptr;
};

/// Counters for `stats()` and the tests.
struct HostCounters {
  uint64_t records = 0;
  uint64_t bytes = 0;
  uint64_t wakes = 0;
  uint64_t dropped = 0;
  uint64_t nativePortCalls = 0;
  uint64_t jsSyncPortCalls = 0;
  uint64_t unavailableSyncPortCalls = 0;
};

/// One running core and its inbox (see the file comment).
class Host {
 public:
  /// `wake` is called, from any thread and outside every lock, when a record arrives from a thread
  /// other than the JS thread inside a host function and no drain is pending. It must only post
  /// work (React Native: `CallInvoker::invokeAsync`).
  Host(const Api &api, std::function<void()> wake);
  /// Shuts the core down if it is still running.
  ~Host();
  Host(const Host &) = delete;
  Host &operator=(const Host &) = delete;

  /// Registers `ports` (and the native Clock, Rng and Log, and the native defaults `options` asks
  /// for), then `undra_init`s the core with the encoded `RuntimeConfig` in `config`, then starts the
  /// native `Connectivity` source if asked. Returns 0, an `undra_init` code or a `start_code`.
  uint32_t start(const uint8_t *config, uint32_t len, const std::vector<PortSpec> &ports, const StartOptions &options = {});
  /// `undra_shutdown`, once: in-flight calls are answered by the core (and dropped here, nobody
  /// waits any more), then no callback runs again; then the native defaults stop (their event source
  /// and their worker threads, joined), and only then is the process's core slot released, so nothing
  /// this core asked for reaches the next one. Must not be called from a callback.
  void shutdown() noexcept;
  /// Whether `start` succeeded and `shutdown` has not run.
  bool running() const noexcept { return running_.load(std::memory_order_acquire); }

  /// Takes every record queued so far (`kind u8, len u32, payload` each), and clears the "drain
  /// pending" flag first, so a record that arrives after the take wakes the JS thread again.
  std::vector<uint8_t> takeInbox();
  /// Whether records are waiting.
  bool hasRecords() const;
  /// Asks for a drain on the JS thread (the wake function), unless one is pending.
  void requestDrain() noexcept;
  /// A snapshot of the counters.
  HostCounters counters() const;
  /// The C ABI this host calls.
  const Api &api() const noexcept { return api_; }

  /// The process's running host, if any (one core per process).
  static Host *runningHost() noexcept;

 private:
  friend class CallScope;
  friend struct HostTestAccess; // cpp/test/host_test.cpp drives the port callbacks directly

  static void replyTrampoline(void *user, uint32_t callId, const uint8_t *ptr, uint32_t len) noexcept;
  static void changeSetTrampoline(void *user, const uint8_t *ptr, uint32_t len) noexcept;
  static void streamTrampoline(void *user, uint32_t callId, const uint8_t *ptr, uint32_t len) noexcept;
  static uint8_t nativePortTrampoline(
      void *user,
      uint32_t portId,
      uint32_t methodId,
      uint32_t portCallId,
      const uint8_t *ptr,
      uint32_t len,
      UndraBuf *outReply) noexcept;
  static uint8_t defaultPortTrampoline(
      void *user,
      uint32_t portId,
      uint32_t methodId,
      uint32_t portCallId,
      const uint8_t *ptr,
      uint32_t len,
      UndraBuf *outReply) noexcept;
  static uint8_t jsPortTrampoline(
      void *user,
      uint32_t portId,
      uint32_t methodId,
      uint32_t portCallId,
      const uint8_t *ptr,
      uint32_t len,
      UndraBuf *outReply) noexcept;

  /// Appends one record: `kind`, then `head` and `body` as its payload. `false` when it was not
  /// queued (the host is not running, or memory ran out).
  bool append(RecordKind kind, const uint8_t *head, std::size_t headLen, const uint8_t *body, std::size_t bodyLen) noexcept;
  /// Appends a log record of this host's own (target `undra::react-native`).
  void log(uint8_t level, const char *message) noexcept;
  uint8_t answerNative(uint32_t portId, uint32_t methodId, uint32_t portCallId, const uint8_t *args, uint32_t len, UndraBuf *out) noexcept;
  uint8_t answerJs(uint32_t portId, uint32_t methodId, uint32_t portCallId, const uint8_t *args, uint32_t len, UndraBuf *out) noexcept;
  bool isSyncMethod(uint32_t portId, uint32_t methodId) const noexcept;
  static uint64_t key(uint32_t portId, uint32_t methodId) noexcept {
    return (static_cast<uint64_t>(portId) << 32) | methodId;
  }

  const Api &api_;
  std::function<void()> wake_;
  std::atomic<bool> running_{false};
  std::atomic<bool> started_{false};
  std::atomic<bool> drainPending_{false};

  mutable std::mutex inboxMutex_;
  std::vector<uint8_t> inbox_;
  HostCounters counters_;

  /// Written in `start` before the first registration, read-only afterwards.
  std::unordered_set<uint64_t> syncMethods_;
  std::vector<uint32_t> registered_;

  std::mutex warnedMutex_;
  std::unordered_set<uint32_t> warned_;

  /// The native default ports, when `start` was asked for any (written in `start` before the first
  /// registration, read-only afterwards).
  std::unique_ptr<NativeDefaults> defaults_;
};

/// Writes `value` little-endian into `out[0..4)`.
inline void putU32(uint8_t *out, uint32_t value) noexcept {
  out[0] = static_cast<uint8_t>(value);
  out[1] = static_cast<uint8_t>(value >> 8);
  out[2] = static_cast<uint8_t>(value >> 16);
  out[3] = static_cast<uint8_t>(value >> 24);
}

/// Reads a little-endian `u32` from `in[0..4)`.
inline uint32_t getU32(const uint8_t *in) noexcept {
  return static_cast<uint32_t>(in[0]) | (static_cast<uint32_t>(in[1]) << 8) |
      (static_cast<uint32_t>(in[2]) << 16) | (static_cast<uint32_t>(in[3]) << 24);
}

} // namespace undra::rn
