// The standard ports @undra/react-native answers natively: `Kv`, `SecureStore`, `Fs`, `Db` (ADR-048) and
// the `Connectivity` event source (ADR-038, amendment B).
//
// `Platform` is what a phone supplies (iOS: `ios/UndraPlatformApple.mm`, Android:
// `cpp/UndraPlatformAndroid.cpp`; the host test supplies its own): where the files live, the
// platform's secret store, its SQLite and its network monitor. `NativeDefaults` is one running host's
// use of it: a port callback hands each call to that port's serial worker thread (`Db`: to the thread of
// the database it names, `UndraDb.h`), which does the I/O and answers with `undra_port_reply`;
// JavaScript is never involved (B2). `stop()` ends the event source and joins the workers, and
// `Host::shutdown` calls it before it releases the process's core slot, so nothing a stopped core asked
// for can reach the next one (B3).
#pragma once

#include <condition_variable>
#include <cstddef>
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
#include "UndraDb.h"
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
  /// The platform's SQLite for the `Db` port (ADR-048); null when it has none (the port is then not
  /// native). Cheap: no file is touched until a database is opened.
  virtual std::unique_ptr<DbBackend> makeDbBackend() {
    return nullptr;
  }
  /// A worker thread named `name` starts, on that thread (Android attaches it to the VM under that name, which
  /// would otherwise replace the thread's own). Must not throw.
  virtual void workerStarted(const char * /*name*/) noexcept {}
  /// The worker thread ends, on that thread (Android detaches it). Must not throw.
  virtual void workerEnded() noexcept {}
};

/// Whether `name_space` may name a core's stores: a lowercase letter, then lowercase letters, digits and `_`, at most 32
/// bytes, the rule of `undra.toml` (SPEC 13). `loadApi` only needs a C identifier (a class or library name), but the default
/// stores put the namespace in a directory, a Keychain service, a Keystore alias and a file name, on file systems that
/// are case-insensitive (Apple's), so `makePlatform` refuses anything else before it makes a path.
inline bool validStorageNamespace(const std::string &name_space) noexcept {
  if (name_space.empty() || name_space.size() > 32) return false;
  for (std::size_t i = 0; i < name_space.size(); ++i) {
    const char c = name_space[i];
    const bool lower = c >= 'a' && c <= 'z';
    const bool digit = c >= '0' && c <= '9';
    if (!(lower || (i > 0 && (digit || c == '_')))) return false;
  }
  return true;
}

/// What `makePlatform` says when `name_space` is not one (`validStorageNamespace`).
inline std::string invalidStorageNamespace(const std::string &name_space) {
  return "the Undra core namespace `" + (name_space.size() > 40 ? name_space.substr(0, 40) + "..." : name_space) +
      "` cannot name the default stores: lowercase letters, digits and `_`, starting with a letter, at most 32 characters (undra.toml [core] namespace)";
}

/// The platform of a phone: `ios/UndraPlatformApple.mm` on Apple platforms, `UndraPlatformAndroid.cpp`
/// on Android (call it on the JS thread: Android resolves its Java class there). Null elsewhere, or when
/// the platform cannot be reached (`error` says why), or when `name_space` is not a core namespace
/// (`validStorageNamespace`).
///
/// `name_space` is the namespace of the core the platform serves (the table's `name_space`): every default
/// store is per namespace (ADR-044 amendment A), so two cores of one app never share one. Kv, Fs and the
/// databases live under `.../Undra/<name_space>/...` on Apple platforms (the casing the Swift runtime has always
/// used on disk) and `.../undra/<name_space>/...` on Android (the latter's databases are
/// `undra-<name_space>-<name>.sqlite`), the Keychain service and the Keystore alias are
/// `<name_space>.dev.undra.securestore`: the Swift runtime's and `android-adapters`' layouts, with the
/// namespace in the same place.
std::unique_ptr<Platform> makePlatform(const std::string &name_space, std::string &error);

/// The ports of `ids` the platform can answer natively (`Kv`, `SecureStore`, `Fs`, `Connectivity`, `Db`).
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
  NativeDefaults(const Api &api, std::shared_ptr<Platform> platform, const std::vector<uint32_t> &ports, Log log);
  ~NativeDefaults();
  NativeDefaults(const NativeDefaults &) = delete;
  NativeDefaults &operator=(const NativeDefaults &) = delete;

  /// Whether `portId` is one of the request/reply ports answered here (not `Connectivity`).
  bool answers(uint32_t portId) const noexcept;
  /// The `Db` port's binding options (tests shorten the busy timeout). Before the first `Db` call.
  void setDbOptions(DbOptions options) noexcept {
    dbOptions_ = options;
  }
  /// Whether the `Connectivity` source is to be started.
  bool reportsConnectivity() const noexcept { return connectivity_; }
  /// A port call from the core (any thread, possibly under the core lock): queues it on the port's
  /// worker and returns 1, or 2 (unavailable) when it cannot be queued. `Db` may also answer at once
  /// into `out` and return 0 (an id that names nothing, a refused argument). Never blocks, never throws.
  uint8_t post(uint32_t portId, uint32_t methodId, uint32_t portCallId, const uint8_t *args, uint32_t len, UndraBuf *out = nullptr) noexcept;
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
  /// Writes a log record, dropping it if even that fails (out of memory).
  void logQuietly(uint8_t level, const char *message) noexcept;

  const Api &api_;
  /// Kept alive while anything made from it (stores, the monitor, the workers) may run.
  std::shared_ptr<Platform> platform_;
  Log log_;
  bool kv_ = false;
  bool secureStore_ = false;
  bool fs_ = false;
  bool connectivity_ = false;
  bool db_ = false;
  std::unique_ptr<KvStore> kvStore_;
  std::unique_ptr<FsRoot> fsRoot_;
  std::unique_ptr<SecretStore> secrets_;
  std::unique_ptr<ConnectivitySource> monitor_;
  /// The `Db` binding, made on the first `Db` call (it starts one thread per open database).
  std::shared_ptr<DbBackend> dbBackend_;
  DbOptions dbOptions_;
  std::mutex dbMutex_;
  std::shared_ptr<DbPort> dbPort_;
  bool dbStopped_ = false;
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
