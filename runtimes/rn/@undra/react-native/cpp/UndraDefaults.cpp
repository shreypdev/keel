#include "UndraDefaults.h"

#include <pthread.h>

#include <new>
#include <utility>

namespace undra::rn {

static_assert(ports::kKv == 0x5389110du, "Kv port id (docs/SPEC.md section 8)");
static_assert(ports::kKvGet == 0xf050bb1au, "Kv.get method id");
static_assert(ports::kSecureStore == 0xc01f5beau, "SecureStore port id");
static_assert(ports::kFs == 0x4ea34cabu, "Fs port id");
static_assert(ports::kFsRead == 0x01fdbe44u, "Fs.read method id");
static_assert(ports::kConnectivity == 0x1feff6ffu, "Connectivity port id");
static_assert(ports::kConnectivityChanged == 0xb4f2a010u, "Connectivity.changed method id");

namespace {

/// A `PortReply` payload (docs/SPEC.md section 3.6): `port_call_id u32, status u8, body`.
std::vector<uint8_t> portReply(uint32_t portCallId, uint8_t status, const std::vector<uint8_t> &body = {}) {
  WireWriter w;
  w.out.reserve(5 + body.size());
  w.u32(portCallId).u8(status);
  w.out.insert(w.out.end(), body.begin(), body.end());
  return std::move(w.out);
}

constexpr uint8_t kOk = 0;
constexpr uint8_t kTypedError = 1;
constexpr uint8_t kUnavailable = 2;

/// The encoded `FsError` of a failure.
std::vector<uint8_t> fsError(const FsFailure &failure) {
  WireWriter w;
  w.u16(static_cast<uint16_t>(failure.kind));
  if (failure.kind == FsErrorKind::Io) w.str(failure.message);
  return std::move(w.out);
}

std::vector<uint8_t> optionBytes(const std::optional<std::vector<uint8_t>> &value) {
  WireWriter w;
  if (value) {
    w.u8(1).bytes(*value);
  } else {
    w.u8(0);
  }
  return std::move(w.out);
}

const char *portName(uint32_t portId) {
  if (portId == ports::kKv) return "Kv";
  if (portId == ports::kSecureStore) return "SecureStore";
  if (portId == ports::kFs) return "Fs";
  return "?";
}

} // namespace

std::vector<uint32_t> nativePortsOf(Platform &platform) {
  std::vector<uint32_t> out;
  if (!platform.kvDirectory().empty()) out.push_back(ports::kKv);
  // A secret store is only offered when the platform has one; asking is cheap (no I/O).
  if (platform.makeSecretStore() != nullptr) out.push_back(ports::kSecureStore);
  if (!platform.fsRoot().empty()) out.push_back(ports::kFs);
  if (platform.makeConnectivity() != nullptr) out.push_back(ports::kConnectivity);
  return out;
}

// ----- Worker -----------------------------------------------------------------------------------

bool Worker::post(std::function<void()> job) noexcept {
  try {
    std::lock_guard<std::mutex> lock(mutex_);
    if (stopping_) return false;
    if (!thread_.joinable()) {
      thread_ = std::thread([this] { run(); });
    }
    jobs_.push_back(std::move(job));
  } catch (...) {
    return false; // out of memory, or no thread: the caller answers "unavailable"
  }
  wake_.notify_one();
  return true;
}

void Worker::run() {
  // Named, so a stack dump or a profiler shows which port it serves.
#if defined(__APPLE__)
  pthread_setname_np(name_);
#else
  pthread_setname_np(pthread_self(), name_);
#endif
  if (platform_ != nullptr) platform_->workerStarted();
  while (true) {
    std::function<void()> job;
    {
      std::unique_lock<std::mutex> lock(mutex_);
      wake_.wait(lock, [this] { return stopping_ || !jobs_.empty(); });
      if (stopping_) break;
      job = std::move(jobs_.front());
      jobs_.pop_front();
    }
    try {
      job();
    } catch (...) {
      // A job catches its own failures; nothing may end the thread early.
    }
  }
  if (platform_ != nullptr) platform_->workerEnded();
}

void Worker::stop() noexcept {
  std::thread thread;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    stopping_ = true;
    jobs_.clear();
    thread = std::move(thread_);
  }
  wake_.notify_all();
  if (thread.joinable() && thread.get_id() != std::this_thread::get_id()) {
    thread.join();
  } else if (thread.joinable()) {
    thread.detach(); // never happens: a job does not stop its own worker
  }
}

// ----- NativeDefaults ---------------------------------------------------------------------------

NativeDefaults::NativeDefaults(const Api &api, Platform &platform, const std::vector<uint32_t> &ports, Log log)
    : api_(api),
      log_(std::move(log)),
      kvWorker_(&platform, "undra-kv"),
      secureWorker_(&platform, "undra-securestore"),
      fsWorker_(&platform, "undra-fs") {
  for (uint32_t id : ports) {
    if (id == ports::kKv) {
      const std::string dir = platform.kvDirectory();
      if (!dir.empty()) {
        kvStore_ = std::make_unique<KvStore>(dir, platform.kvNaming());
        kv_ = true;
      }
    } else if (id == ports::kSecureStore) {
      secrets_ = platform.makeSecretStore();
      secureStore_ = secrets_ != nullptr;
    } else if (id == ports::kFs) {
      const std::string root = platform.fsRoot();
      if (!root.empty()) {
        fsRoot_ = std::make_unique<FsRoot>(root);
        fs_ = true;
      }
    } else if (id == ports::kConnectivity) {
      monitor_ = platform.makeConnectivity();
      connectivity_ = monitor_ != nullptr;
    }
  }
}

NativeDefaults::~NativeDefaults() {
  stop();
}

bool NativeDefaults::answers(uint32_t portId) const noexcept {
  return (portId == ports::kKv && kv_) || (portId == ports::kSecureStore && secureStore_) || (portId == ports::kFs && fs_);
}

uint8_t NativeDefaults::post(uint32_t portId, uint32_t methodId, uint32_t portCallId, const uint8_t *args, uint32_t len) noexcept {
  Worker *worker = portId == ports::kKv ? &kvWorker_ : portId == ports::kSecureStore ? &secureWorker_ : portId == ports::kFs ? &fsWorker_ : nullptr;
  if (worker == nullptr || !answers(portId)) return kUnavailable;
  try {
    // A copy: the core lends `args` for the duration of the callback only.
    std::vector<uint8_t> copy;
    if (args != nullptr && len > 0) copy.assign(args, args + len);
    const bool queued = worker->post([this, portId, methodId, portCallId, copy = std::move(copy)] {
      answer(portId, methodId, portCallId, copy);
    });
    return queued ? 1 : kUnavailable;
  } catch (...) {
    return kUnavailable;
  }
}

void NativeDefaults::answer(uint32_t portId, uint32_t methodId, uint32_t portCallId, const std::vector<uint8_t> &args) noexcept {
  std::vector<uint8_t> reply;
  try {
    if (portId == ports::kKv) {
      reply = kv(*kvStore_, portId, methodId, portCallId, args);
    } else if (portId == ports::kSecureStore) {
      reply = secure(methodId, portCallId, args);
    } else {
      reply = fs(methodId, portCallId, args);
    }
  } catch (const std::bad_alloc &) {
    reply = portReply(portCallId, kUnavailable);
    log_(4, std::string("the ") + portName(portId) + " port ran out of memory");
  } catch (...) {
    reply = portReply(portCallId, kUnavailable);
  }
  // Allowed from any thread (host contract 4). After `undra_shutdown` it reaches no runtime and is
  // ignored; `Host::shutdown` joins this thread before another core can exist (B3).
  api_.port_reply(reply.data(), static_cast<uint32_t>(reply.size()));
}

std::vector<uint8_t> NativeDefaults::kv(KvStore &store, uint32_t portId, uint32_t methodId, uint32_t portCallId, const std::vector<uint8_t> &args) {
  WireReader r(args.data(), args.size());
  const char *name = portName(portId);
  std::string error;
  if (methodId == ports::kKvGet) {
    const std::string key = r.str();
    if (!r.finish()) return portReply(portCallId, kUnavailable);
    std::optional<std::vector<uint8_t>> value;
    if (!store.get(key, value, error)) {
      log_(4, std::string(name) + ".get failed: " + error);
      return portReply(portCallId, kUnavailable);
    }
    return portReply(portCallId, kOk, optionBytes(value));
  }
  if (methodId == ports::kKvSet) {
    const std::string key = r.str();
    const std::vector<uint8_t> value = r.bytes();
    if (!r.finish()) return portReply(portCallId, kUnavailable);
    if (!store.set(key, value.data(), value.size(), error)) {
      log_(4, std::string(name) + ".set failed: " + error);
      return portReply(portCallId, kUnavailable);
    }
    return portReply(portCallId, kOk);
  }
  if (methodId == ports::kKvDelete) {
    const std::string key = r.str();
    if (!r.finish()) return portReply(portCallId, kUnavailable);
    if (!store.remove(key, error)) {
      log_(4, std::string(name) + ".delete failed: " + error);
      return portReply(portCallId, kUnavailable);
    }
    return portReply(portCallId, kOk);
  }
  if (methodId == ports::kKvList) {
    const std::string prefix = r.str();
    if (!r.finish()) return portReply(portCallId, kUnavailable);
    std::vector<std::string> keys;
    if (!store.list(prefix, keys, error)) {
      log_(4, std::string(name) + ".list failed: " + error);
      return portReply(portCallId, kUnavailable);
    }
    WireWriter w;
    w.strings(keys);
    return portReply(portCallId, kOk, w.out);
  }
  return portReply(portCallId, kUnavailable);
}

std::vector<uint8_t> NativeDefaults::secure(uint32_t methodId, uint32_t portCallId, const std::vector<uint8_t> &args) {
  WireReader r(args.data(), args.size());
  SecretStore &store = *secrets_;
  std::string error;
  if (methodId == ports::kSecureStoreGet) {
    const std::string key = r.str();
    if (!r.finish()) return portReply(portCallId, kUnavailable);
    std::optional<std::vector<uint8_t>> value;
    if (!store.get(key, value, error)) {
      log_(4, "SecureStore.get failed: " + error);
      return portReply(portCallId, kUnavailable);
    }
    return portReply(portCallId, kOk, optionBytes(value));
  }
  if (methodId == ports::kSecureStoreSet) {
    const std::string key = r.str();
    const std::vector<uint8_t> value = r.bytes();
    if (!r.finish()) return portReply(portCallId, kUnavailable);
    if (!store.set(key, value, error)) {
      log_(4, "SecureStore.set failed: " + error);
      return portReply(portCallId, kUnavailable);
    }
    return portReply(portCallId, kOk);
  }
  if (methodId == ports::kSecureStoreDelete) {
    const std::string key = r.str();
    if (!r.finish()) return portReply(portCallId, kUnavailable);
    if (!store.remove(key, error)) {
      log_(4, "SecureStore.delete failed: " + error);
      return portReply(portCallId, kUnavailable);
    }
    return portReply(portCallId, kOk);
  }
  if (methodId == ports::kSecureStoreList) {
    const std::string prefix = r.str();
    if (!r.finish()) return portReply(portCallId, kUnavailable);
    std::vector<std::string> keys;
    if (!store.list(prefix, keys, error)) {
      log_(4, "SecureStore.list failed: " + error);
      return portReply(portCallId, kUnavailable);
    }
    WireWriter w;
    w.strings(keys);
    return portReply(portCallId, kOk, w.out);
  }
  return portReply(portCallId, kUnavailable);
}

std::vector<uint8_t> NativeDefaults::fs(uint32_t methodId, uint32_t portCallId, const std::vector<uint8_t> &args) {
  WireReader r(args.data(), args.size());
  FsRoot &root = *fsRoot_;
  std::optional<FsFailure> failure;
  std::vector<uint8_t> body;
  if (methodId == ports::kFsRead) {
    const std::string path = r.str();
    if (!r.finish()) return portReply(portCallId, kUnavailable);
    std::vector<uint8_t> data;
    failure = root.read(path, data);
    if (!failure) {
      WireWriter w;
      w.out.reserve(4 + data.size());
      w.bytes(data);
      body = std::move(w.out);
    }
  } else if (methodId == ports::kFsWrite) {
    const std::string path = r.str();
    const std::vector<uint8_t> data = r.bytes();
    if (!r.finish()) return portReply(portCallId, kUnavailable);
    failure = root.write(path, data.data(), data.size());
  } else if (methodId == ports::kFsDelete) {
    const std::string path = r.str();
    if (!r.finish()) return portReply(portCallId, kUnavailable);
    failure = root.remove(path);
  } else if (methodId == ports::kFsList) {
    const std::string dir = r.str();
    if (!r.finish()) return portReply(portCallId, kUnavailable);
    std::vector<std::string> names;
    failure = root.list(dir, names);
    if (!failure) {
      WireWriter w;
      w.strings(names);
      body = std::move(w.out);
    }
  } else {
    return portReply(portCallId, kUnavailable);
  }
  if (failure) return portReply(portCallId, kTypedError, fsError(*failure));
  return portReply(portCallId, kOk, body);
}

void NativeDefaults::startConnectivity() noexcept {
  if (!connectivity_ || monitor_ == nullptr) return;
  try {
    const bool started = monitor_->start([this](bool online, NetKind kind) { report(online, kind); });
    if (!started) {
      log_(3, "the Connectivity source could not start (a missing permission?); the core assumes the network is up");
    }
  } catch (...) {
    log_(3, "the Connectivity source failed to start; the core assumes the network is up");
  }
}

void NativeDefaults::report(bool online, NetKind kind) noexcept {
  std::lock_guard<std::mutex> lock(reportMutex_);
  if (stopped_) return;
  if (hasLast_ && lastOnline_ == online && lastKind_ == kind) return;
  hasLast_ = true;
  lastOnline_ = online;
  lastKind_ = kind;
  uint8_t payload[3];
  payload[0] = online ? 1 : 0;
  payload[1] = static_cast<uint8_t>(static_cast<uint16_t>(kind));
  payload[2] = static_cast<uint8_t>(static_cast<uint16_t>(kind) >> 8);
  // From the source's own thread, never a callback: `undra_event` is allowed here (host contract 4).
  // Under `reportMutex_`, so `stop()` cannot return while an event is being delivered.
  api_.event(ports::kConnectivity, ports::kConnectivityChanged, payload, sizeof(payload));
}

void NativeDefaults::stop() noexcept {
  {
    std::lock_guard<std::mutex> lock(reportMutex_);
    stopped_ = true;
  }
  if (monitor_ != nullptr) {
    monitor_->stop();
  }
  kvWorker_.stop();
  secureWorker_.stop();
  fsWorker_.stop();
}

} // namespace undra::rn
