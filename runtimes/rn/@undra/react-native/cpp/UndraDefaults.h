// The standard ports @undra/react-native answers natively: `Kv`, `SecureStore`, `Fs` and the
// `Connectivity` event source (ADR-038, amendment B).
//
// `Platform` is what a phone supplies (iOS: `ios/UndraPlatformApple.mm`, Android:
// `cpp/UndraPlatformAndroid.cpp`; the host test supplies its own): where the files live, the
// platform's secret store and its network monitor. `NativeDefaults` is one running host's use of
// it: a port callback hands each call to that port's serial worker thread, which does the I/O and
// answers with `undra_port_reply`; JavaScript is never involved (B2). `stop()` ends the event source
// and joins the workers, and `Host::shutdown` calls it before it releases the process's core slot, so
// nothing a stopped core asked for can reach the next one (B3).
#pragma once

#include <condition_variable>
#include <cstdint>
#include <deque>
#include <functional>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <thread>
#include <vector>

#include "UndraApi.h"
#include "UndraHost.h"
#include "UndraStores.h"

namespace undra::rn {

/// Ids of the standard ports this file answers (docs/SPEC.md section 8).
namespace ports {
inline constexpr uint32_t kKv = fnv1a32("port.Kv");
inline constexpr uint32_t kKvGet = fnv1a32("Kv.get");
inline constexpr uint32_t kKvSet = fnv1a32("Kv.set");
inline constexpr uint32_t kKvDelete = fnv1a32("Kv.delete");
inline constexpr uint32_t kKvList = fnv1a32("Kv.list");
inline constexpr uint32_t kSecureStore = fnv1a32("port.SecureStore");
inline constexpr uint32_t kSecureStoreGet = fnv1a32("SecureStore.get");
inline constexpr uint32_t kSecureStoreSet = fnv1a32("SecureStore.set");
inline constexpr uint32_t kSecureStoreDelete = fnv1a32("SecureStore.delete");
inline constexpr uint32_t kSecureStoreList = fnv1a32("SecureStore.list");
inline constexpr uint32_t kFs = fnv1a32("port.Fs");
inline constexpr uint32_t kFsRead = fnv1a32("Fs.read");
inline constexpr uint32_t kFsWrite = fnv1a32("Fs.write");
inline constexpr uint32_t kFsDelete = fnv1a32("Fs.delete");
inline constexpr uint32_t kFsList = fnv1a32("Fs.list");
inline constexpr uint32_t kConnectivity = fnv1a32("port.Connectivity");
inline constexpr uint32_t kConnectivityChanged = fnv1a32("Connectivity.changed");
} // namespace ports

/// `NetKind`, a unit enum: the wire index (docs/SPEC.md section 8).
enum class NetKind : uint16_t { Wifi = 0, Cellular = 1, Wired = 2, Unknown = 3, None = 4 };

/// The platform's store of secrets (the Keychain; the Keystore-sealed files on Android). Called
/// only from the `SecureStore` worker thread. A `false` answer, with `error` set, is a failure the
/// port reports as "unavailable"; a value that exists is never reported missing.
class SecretStore {
 public:
  virtual ~SecretStore() = default;
  virtual bool get(const std::string &key, std::optional<std::vector<uint8_t>> &value, std::string &error) = 0;
  virtual bool set(const std::string &key, const std::vector<uint8_t> &value, std::string &error) = 0;
  virtual bool remove(const std::string &key, std::string &error) = 0;
  virtual bool list(const std::string &prefix, std::vector<std::string> &keys, std::string &error) = 0;
  /// What it is, for `platformDefaults()` (a Keychain service, a directory).
  virtual std::string describe() const = 0;
};

/// A source of `Connectivity` reports (`NWPathMonitor`, `ConnectivityManager`).
class ConnectivitySource {
 public:
  /// Receives `(online, kind)` on the source's own thread: the current state first, then changes.
  using Report = std::function<void(bool online, NetKind kind)>;
  virtual ~ConnectivitySource() = default;
  /// Starts reporting. `false` when the platform refused (a missing permission, say).
  virtual bool start(Report report) = 0;
  /// Stops reporting: when it returns, `report` is not running and will not run again.
  virtual void stop() noexcept = 0;
};

/// What a platform gives the module. Created on the JS thread; its methods may be called from any.
class Platform {
 public:
  virtual ~Platform() = default;
  /// The `Kv` directory; empty when the platform could not find one (the port is then not native).
  virtual std::string kvDirectory() = 0;
  /// How a key names its `Kv` file on this platform (its native runtime's layout).
  virtual KvNaming kvNaming() = 0;
  /// The `Fs` root; empty when unknown.
  virtual std::string fsRoot() = 0;
  /// The secret store; null when the platform has none.
  virtual std::unique_ptr<SecretStore> makeSecretStore() = 0;
  /// The network monitor; null when the platform has none.
  virtual std::unique_ptr<ConnectivitySource> makeConnectivity() = 0;
  /// A worker thread starts or ends (Android attaches it to the VM). Must not throw.
  virtual void workerStarted() noexcept {}
  virtual void workerEnded() noexcept {}
};

/// The platform of a phone: `ios/UndraPlatformApple.mm` on Apple platforms, `UndraPlatformAndroid.cpp`
/// on Android (call it on the JS thread: Android resolves its Java class there). Null elsewhere, or when
/// the platform cannot be reached (`error` says why).
std::unique_ptr<Platform> makePlatform(std::string &error);

/// The ports of `ids` the platform can answer natively (`Kv`, `SecureStore`, `Fs`, `Connectivity`).
std::vector<uint32_t> nativePortsOf(Platform &platform);

/// One serial worker thread: jobs run in the order they were posted.
class Worker {
 public:
  Worker(Platform *platform, const char *name) : platform_(platform), name_(name) {}
  ~Worker() {
    stop();
  }
  Worker(const Worker &) = delete;
  Worker &operator=(const Worker &) = delete;
  /// Queues `job`, starting the thread on first use. `false` once stopped (or out of memory).
  bool post(std::function<void()> job) noexcept;
  /// Drops the jobs still queued, lets a running one finish, and joins the thread.
  void stop() noexcept;

 private:
  void run();
  Platform *platform_;
  const char *name_;
  std::mutex mutex_;
  std::condition_variable wake_;
  std::deque<std::function<void()>> jobs_;
  bool stopping_ = false;
  std::thread thread_;
};

/// One host's native default ports.
class NativeDefaults {
 public:
  /// Writes a log record of the host (level, message); called from the workers and the monitor.
  using Log = std::function<void(uint8_t level, const std::string &message)>;

  /// `ports` are the ports to answer natively (a subset of `nativePortsOf(platform)`).
  NativeDefaults(const Api &api, Platform &platform, const std::vector<uint32_t> &ports, Log log);
  ~NativeDefaults();
  NativeDefaults(const NativeDefaults &) = delete;
  NativeDefaults &operator=(const NativeDefaults &) = delete;

  /// Whether `portId` is one of the request/reply ports answered here (not `Connectivity`).
  bool answers(uint32_t portId) const noexcept;
  /// Whether the `Connectivity` source is to be started.
  bool reportsConnectivity() const noexcept { return connectivity_; }
  /// A port call from the core (any thread, possibly under the core lock): queues it on the port's
  /// worker and returns 1, or 2 (unavailable) when it cannot be queued. Never blocks, never throws.
  uint8_t post(uint32_t portId, uint32_t methodId, uint32_t portCallId, const uint8_t *args, uint32_t len) noexcept;
  /// Starts the `Connectivity` source after `undra_init` succeeded; reports go to the core with
  /// `undra_event`, the first one at once, identical consecutive ones once.
  void startConnectivity() noexcept;
  /// Stops the source, then the workers (see the file comment). Idempotent.
  void stop() noexcept;

 private:
  void answer(uint32_t portId, uint32_t methodId, uint32_t portCallId, const std::vector<uint8_t> &args) noexcept;
  std::vector<uint8_t> kv(KvStore &store, uint32_t portId, uint32_t methodId, uint32_t portCallId, const std::vector<uint8_t> &args);
  std::vector<uint8_t> secure(uint32_t methodId, uint32_t portCallId, const std::vector<uint8_t> &args);
  std::vector<uint8_t> fs(uint32_t methodId, uint32_t portCallId, const std::vector<uint8_t> &args);
  void report(bool online, NetKind kind) noexcept;

  const Api &api_;
  Log log_;
  bool kv_ = false;
  bool secureStore_ = false;
  bool fs_ = false;
  bool connectivity_ = false;
  std::unique_ptr<KvStore> kvStore_;
  std::unique_ptr<FsRoot> fsRoot_;
  std::unique_ptr<SecretStore> secrets_;
  std::unique_ptr<ConnectivitySource> monitor_;
  Worker kvWorker_;
  Worker secureWorker_;
  Worker fsWorker_;
  std::mutex reportMutex_;
  bool stopped_ = false;
  bool hasLast_ = false;
  bool lastOnline_ = false;
  NetKind lastKind_ = NetKind::Unknown;
};

} // namespace undra::rn
