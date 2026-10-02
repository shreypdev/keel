#include "UndraHost.h"

#include "UndraDefaults.h"

#include <algorithm>
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <cstdlib>
#include <stdlib.h>
#include <cstring>
#include <map>
#include <new>
#include <string>

namespace undra::rn {

static_assert(ports::kLog == 0x575ff24au, "Log port id (docs/SPEC.md section 8)");
static_assert(ports::kLogLog == 0xd49d5649u, "Log.log method id");
static_assert(ports::kClock == 0xcd99c48eu, "Clock port id");
static_assert(ports::kClockNowMs == 0xccc94d90u, "Clock.now_ms method id");
static_assert(ports::kClockMonotonicNs == 0x2cb2b4bfu, "Clock.monotonic_ns method id");
static_assert(ports::kRng == 0x25135bf5u, "Rng port id");
static_assert(ports::kRngFill == 0x2832b8edu, "Rng.fill method id");

namespace {

/// The JS thread's innermost scope.
thread_local CallScope *t_scope = nullptr;

/// A core's slot (one running host per core, ADR-044): the host that holds it, and whether that host
/// is inside `undra_shutdown` right now.
struct Slot {
  Host *host = nullptr;
  bool stopping = false;
};

/// The process's slots, one per namespace that ever had a host, under `g_slotMutex`; `g_slotFreed`
/// is notified when a slot is released. Never destroyed (a host may shut down while the process
/// exits).
std::mutex &g_slotMutex = *new std::mutex;
std::condition_variable &g_slotFreed = *new std::condition_variable;
std::map<std::string, Slot> &g_slots = *new std::map<std::string, Slot>;

/// How long `start` waits for a host of the same core that is shutting down on another thread (a
/// reloaded runtime's old module going away on the old JS thread) to release the slot.
constexpr auto kSlotWait = std::chrono::seconds(5);

/// Releases `host`'s core's slot if `host` holds it.
void releaseSlot(const Host *host) noexcept {
  {
    std::lock_guard<std::mutex> lock(g_slotMutex);
    auto slot = g_slots.find(host->api().name_space);
    if (slot == g_slots.end() || slot->second.host != host) {
      return;
    }
    slot->second = Slot{};
  }
  g_slotFreed.notify_all();
}

/// The most bytes one `Rng.fill` may ask for (the limit of the other runtimes).
constexpr uint32_t kMaxRngBytes = 1u << 24;

/// The target of the log records this host writes itself.
constexpr char kLogTarget[] = "undra::react-native";

void putU64(uint8_t *out, uint64_t value) noexcept {
  for (int i = 0; i < 8; ++i) {
    out[i] = static_cast<uint8_t>(value >> (8 * i));
  }
}

/// Hands `body` to the core as the synchronous answer of `portCallId` (status 0): a `malloc`ed
/// `PortReply` payload with `cap = 0`, which the core copies and `free`s (docs/SPEC.md section 6.3).
uint8_t replyOk(uint32_t portCallId, const uint8_t *body, std::size_t len, UndraBuf *out) noexcept {
  if (out == nullptr || len > UINT32_MAX - 5) {
    return 2;
  }
  auto *block = static_cast<uint8_t *>(std::malloc(5 + len));
  if (block == nullptr) {
    return 2;
  }
  putU32(block, portCallId);
  block[4] = 0;
  if (len > 0) {
    std::memcpy(block + 5, body, len);
  }
  out->ptr = block;
  out->len = static_cast<uint32_t>(5 + len);
  out->cap = 0;
  return 0;
}

/// Copies a complete `PortReply` payload the JavaScript side built into `malloc`ed memory.
uint8_t replyRaw(const std::vector<uint8_t> &reply, UndraBuf *out) noexcept {
  if (out == nullptr || reply.size() < 5 || reply.size() > UINT32_MAX) {
    return 2;
  }
  auto *block = static_cast<uint8_t *>(std::malloc(reply.size()));
  if (block == nullptr) {
    return 2;
  }
  std::memcpy(block, reply.data(), reply.size());
  out->ptr = block;
  out->len = static_cast<uint32_t>(reply.size());
  out->cap = 0;
  return 0;
}

} // namespace

// ----- CallScope ------------------------------------------------------------------------------

CallScope::CallScope(Host &host, SyncPortAnswerer *answerer) noexcept
    : host(host), answerer(answerer), previous_(t_scope) {
  t_scope = this;
}

CallScope::~CallScope() {
  t_scope = previous_;
}

CallScope *CallScope::current() noexcept {
  return t_scope;
}

bool CallScope::inCallback(const Host &host) noexcept {
  for (const CallScope *scope = t_scope; scope != nullptr; scope = scope->previous_) {
    if (&scope->host == &host && scope->inCallback_) {
      return true;
    }
  }
  return false;
}

// ----- Host -----------------------------------------------------------------------------------

Host::Host(const Api &api, std::function<void()> wake) : api_(api), wake_(std::move(wake)) {
  inbox_.reserve(4096);
}

Host::~Host() {
  shutdown();
}

Host *Host::runningHost(const std::string &name_space) noexcept {
  std::lock_guard<std::mutex> lock(g_slotMutex);
  auto slot = g_slots.find(name_space);
  return slot != g_slots.end() ? slot->second.host : nullptr;
}

uint32_t Host::start(const uint8_t *config, uint32_t len, const std::vector<PortSpec> &specs, const StartOptions &options) {
  if (started_.exchange(true)) {
    return start_code::kAlreadyStarted;
  }
  try {
    std::unique_lock<std::mutex> lock(g_slotMutex);
    Slot &slot = g_slots[api_.name_space]; // a reference into a map stays valid while others are added
    // A host of this core that is already inside `undra_shutdown` on another thread frees the slot
    // when that returns (a dev reload: the old runtime's module goes away on the old JS thread while
    // the new runtime starts): wait for it, bounded, rather than refusing. A running host of this
    // core is refused; a host of another core holds another slot.
    if (slot.host != nullptr && slot.stopping) {
      g_slotFreed.wait_for(lock, kSlotWait, [&slot] { return slot.host == nullptr; });
    }
    if (slot.host != nullptr) {
      started_.store(false);
      return start_code::kBusy;
    }
    slot.host = this;
  } catch (...) {
    started_.store(false); // out of memory adding the slot
    throw;
  }
  if (api_.abi_version != kAbiVersion) {
    releaseSlot(this);
    started_.store(false);
    return start_code::kAbiMismatch;
  }
  for (const PortSpec &spec : specs) {
    for (uint32_t method : spec.syncMethods) {
      syncMethods_.insert(key(spec.portId, method));
    }
  }
  if (options.platform != nullptr && !options.nativePorts.empty()) {
    try {
      defaults_ = std::make_unique<NativeDefaults>(api_, options.platform, options.nativePorts, [this](uint8_t level, const std::string &message) {
        log(level, message.c_str());
      });
    } catch (...) {
      defaults_.reset(); // out of memory: every port stays JavaScript's
    }
  }
  // Registered before `undra_init`, so the start-up hooks (query hydration reads Kv) never race a
  // late registration (docs/SPEC.md section 6). Clock, Rng and Log are answered here, on whatever
  // thread asks; Timer keeps the core's own timer thread.
  for (uint32_t id : {ports::kClock, ports::kRng, ports::kLog}) {
    api_.port_register(id, &Host::nativePortTrampoline, this);
    registered_.push_back(id);
  }
  for (const PortSpec &spec : specs) {
    const uint32_t id = spec.portId;
    if (id == ports::kClock || id == ports::kRng || id == ports::kLog || id == ports::kTimer) {
      continue;
    }
    const bool native = defaults_ != nullptr && defaults_->answers(id);
    api_.port_register(id, native ? &Host::defaultPortTrampoline : &Host::jsPortTrampoline, this);
    registered_.push_back(id);
  }
  // Accept records from here on: the core may call back from inside `undra_init`.
  running_.store(true, std::memory_order_release);
  const uint32_t code = api_.init(
      config, len, &Host::replyTrampoline, &Host::changeSetTrampoline, &Host::streamTrampoline, this);
  if (code != 0) {
    running_.store(false, std::memory_order_release);
    for (uint32_t id : registered_) {
      api_.port_register(id, nullptr, nullptr);
    }
    registered_.clear();
    syncMethods_.clear();
    if (defaults_ != nullptr) {
      // A start-up hook may have queued a native call before `undra_init` failed: the worker is joined
      // here, its reply reaching no core, before the slot is released.
      defaults_->stop();
      defaults_.reset();
    }
    releaseSlot(this);
    started_.store(false);
    return code;
  }
  // After `undra_init`: an event before it would reach no runtime. The first report follows at once.
  if (defaults_ != nullptr && defaults_->reportsConnectivity()) {
    defaults_->startConnectivity();
  }
  return code;
}

void Host::shutdown() noexcept {
  {
    // Under the slot's lock, so a `start` on another thread sees "running" or "stopping", never a
    // held slot that is neither.
    std::lock_guard<std::mutex> lock(g_slotMutex);
    if (!running_.exchange(false, std::memory_order_acq_rel)) {
      return;
    }
    auto slot = g_slots.find(api_.name_space);
    if (slot != g_slots.end() && slot->second.host == this) {
      slot->second.stopping = true;
    }
  }
  // Waits for the callbacks still running (host contract 5): when it returns, `this` is never
  // read by the core again. What the core says meanwhile (status 3 for calls in flight) is
  // dropped: whoever closed the core has failed those calls already.
  api_.shutdown();
  // B3 of ADR-038 amendment B: the event source and the workers end before another core may start.
  if (defaults_ != nullptr) {
    defaults_->stop();
  }
  registered_.clear();
  releaseSlot(this);
}

std::vector<uint8_t> Host::takeInbox() {
  drainPending_.store(false, std::memory_order_release);
  std::vector<uint8_t> taken;
  std::lock_guard<std::mutex> lock(inboxMutex_);
  if (!inbox_.empty()) {
    // The taken buffer goes to JavaScript as it is (it becomes the batch's ArrayBuffer); the next
    // record starts a new one.
    taken.swap(inbox_);
  }
  return taken;
}

bool Host::hasRecords() const {
  std::lock_guard<std::mutex> lock(inboxMutex_);
  return !inbox_.empty();
}

void Host::requestDrain() noexcept {
  if (drainPending_.exchange(true, std::memory_order_acq_rel)) {
    return;
  }
  {
    std::lock_guard<std::mutex> lock(inboxMutex_);
    counters_.wakes++;
  }
  try {
    if (wake_) {
      wake_();
    }
  } catch (...) {
    drainPending_.store(false, std::memory_order_release);
  }
}

HostCounters Host::counters() const {
  std::lock_guard<std::mutex> lock(inboxMutex_);
  return counters_;
}

bool Host::append(RecordKind kind, const uint8_t *head, std::size_t headLen, const uint8_t *body, std::size_t bodyLen) noexcept {
  if (!running_.load(std::memory_order_acquire)) {
    return false;
  }
  const std::size_t payload = headLen + bodyLen;
  const CallScope *scope = t_scope;
  const bool onJsThread = scope != nullptr && &scope->host == this;
  {
    std::lock_guard<std::mutex> lock(inboxMutex_);
    if (payload > UINT32_MAX) {
      counters_.dropped++;
      return false;
    }
    try {
      if (inbox_.capacity() == 0) {
        inbox_.reserve(std::max<std::size_t>(4096, kRecordHeader + payload));
      }
      uint8_t header[kRecordHeader];
      header[0] = static_cast<uint8_t>(kind);
      putU32(header + 1, static_cast<uint32_t>(payload));
      inbox_.insert(inbox_.end(), header, header + kRecordHeader);
      if (headLen > 0) {
        inbox_.insert(inbox_.end(), head, head + headLen);
      }
      if (bodyLen > 0) {
        inbox_.insert(inbox_.end(), body, body + bodyLen);
      }
    } catch (...) {
      // Out of memory: the record is lost (and counted); the core is not told, it must not unwind.
      counters_.dropped++;
      return false;
    }
    counters_.records++;
    counters_.bytes += payload;
  }
  // On the JS thread inside a host function the host function drains before it returns.
  if (!onJsThread) {
    requestDrain();
  }
  return true;
}

void Host::log(uint8_t level, const char *message) noexcept {
  try {
    const std::size_t targetLen = sizeof(kLogTarget) - 1;
    const std::size_t messageLen = std::strlen(message);
    std::vector<uint8_t> record(1 + 4 + targetLen + 4 + messageLen);
    record[0] = level;
    putU32(record.data() + 1, static_cast<uint32_t>(targetLen));
    std::memcpy(record.data() + 5, kLogTarget, targetLen);
    putU32(record.data() + 5 + targetLen, static_cast<uint32_t>(messageLen));
    std::memcpy(record.data() + 9 + targetLen, message, messageLen);
    append(RecordKind::Log, nullptr, 0, record.data(), record.size());
  } catch (...) {
    // Nothing to report with.
  }
}

bool Host::isSyncMethod(uint32_t portId, uint32_t methodId) const noexcept {
  return syncMethods_.count(key(portId, methodId)) != 0;
}

// ----- callbacks ------------------------------------------------------------------------------

void Host::replyTrampoline(void *user, uint32_t /*callId*/, const uint8_t *ptr, uint32_t len) noexcept {
  if (auto *host = static_cast<Host *>(user)) {
    host->append(RecordKind::Reply, nullptr, 0, ptr, len);
  }
}

void Host::changeSetTrampoline(void *user, const uint8_t *ptr, uint32_t len) noexcept {
  if (auto *host = static_cast<Host *>(user)) {
    host->append(RecordKind::ChangeSet, nullptr, 0, ptr, len);
  }
}

void Host::streamTrampoline(void *user, uint32_t /*callId*/, const uint8_t *ptr, uint32_t len) noexcept {
  if (auto *host = static_cast<Host *>(user)) {
    host->append(RecordKind::StreamItem, nullptr, 0, ptr, len);
  }
}

uint8_t Host::nativePortTrampoline(
    void *user,
    uint32_t portId,
    uint32_t methodId,
    uint32_t portCallId,
    const uint8_t *ptr,
    uint32_t len,
    UndraBuf *outReply) noexcept {
  auto *host = static_cast<Host *>(user);
  if (host == nullptr) {
    return 2;
  }
  return host->answerNative(portId, methodId, portCallId, ptr, len, outReply);
}

uint8_t Host::defaultPortTrampoline(
    void *user,
    uint32_t portId,
    uint32_t methodId,
    uint32_t portCallId,
    const uint8_t *ptr,
    uint32_t len,
    UndraBuf *outReply) noexcept {
  auto *host = static_cast<Host *>(user);
  if (host == nullptr || host->defaults_ == nullptr || !host->running()) {
    return 2;
  }
  {
    std::lock_guard<std::mutex> lock(host->inboxMutex_);
    host->counters_.nativePortCalls++;
  }
  // Queued on the port's worker; the answer comes later through `undra_port_reply` (1). `Db` answers
  // a call that names nothing open at once, into `outReply` (0).
  return host->defaults_->post(portId, methodId, portCallId, ptr, len, outReply);
}

uint8_t Host::jsPortTrampoline(
    void *user,
    uint32_t portId,
    uint32_t methodId,
    uint32_t portCallId,
    const uint8_t *ptr,
    uint32_t len,
    UndraBuf *outReply) noexcept {
  auto *host = static_cast<Host *>(user);
  if (host == nullptr) {
    return 2;
  }
  return host->answerJs(portId, methodId, portCallId, ptr, len, outReply);
}

uint8_t Host::answerNative(
    uint32_t portId,
    uint32_t methodId,
    uint32_t portCallId,
    const uint8_t *args,
    uint32_t len,
    UndraBuf *out) noexcept {
  {
    std::lock_guard<std::mutex> lock(inboxMutex_);
    counters_.nativePortCalls++;
  }
  uint8_t word[8];
  switch (portId) {
    case ports::kClock:
      if (methodId == ports::kClockNowMs) {
        const auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(
                            std::chrono::system_clock::now().time_since_epoch())
                            .count();
        putU64(word, static_cast<uint64_t>(static_cast<int64_t>(ms)));
        return replyOk(portCallId, word, sizeof(word), out);
      }
      if (methodId == ports::kClockMonotonicNs) {
        const auto ns = std::chrono::duration_cast<std::chrono::nanoseconds>(
                            std::chrono::steady_clock::now().time_since_epoch())
                            .count();
        putU64(word, static_cast<uint64_t>(ns));
        return replyOk(portCallId, word, sizeof(word), out);
      }
      return 2;
    case ports::kRng: {
      if (methodId != ports::kRngFill || args == nullptr || len != 4) {
        return 2;
      }
      const uint32_t n = getU32(args);
      if (n > kMaxRngBytes) {
        return 2;
      }
      auto *block = static_cast<uint8_t *>(std::malloc(5 + 4 + static_cast<std::size_t>(n)));
      if (block == nullptr || out == nullptr) {
        std::free(block);
        return 2;
      }
      putU32(block, portCallId);
      block[4] = 0;
      putU32(block + 5, n); // Bytes: u32 length, then the bytes
      if (n > 0) {
        arc4random_buf(block + 9, n);
      }
      out->ptr = block;
      out->len = 9 + n;
      out->cap = 0;
      return 0;
    }
    case ports::kLog:
      if (methodId != ports::kLogLog || args == nullptr || len < 1) {
        return 2;
      }
      append(RecordKind::Log, nullptr, 0, args, len);
      // Port call id 0 is the core's own fire-and-forget record: nothing waits for an answer and
      // "async" leaves nothing to allocate or free (host contract 6).
      if (portCallId == 0) {
        return 1;
      }
      return replyOk(portCallId, nullptr, 0, out);
    default:
      return 2;
  }
}

uint8_t Host::answerJs(
    uint32_t portId,
    uint32_t methodId,
    uint32_t portCallId,
    const uint8_t *args,
    uint32_t len,
    UndraBuf *out) noexcept {
  if (isSyncMethod(portId, methodId)) {
    CallScope *scope = t_scope;
    if (scope != nullptr && &scope->host == this && scope->answerer != nullptr) {
      {
        std::lock_guard<std::mutex> lock(inboxMutex_);
        counters_.jsSyncPortCalls++;
      }
      std::vector<uint8_t> reply;
      const bool was = scope->inCallback_;
      scope->inCallback_ = true;
      const uint8_t answer = scope->answerer->answer(portId, methodId, portCallId, args, len, reply);
      scope->inCallback_ = was;
      if (answer != 0) {
        return 2;
      }
      return replyRaw(reply, out);
    }
    // A JavaScript implementation cannot run on this thread, and the core must not wait for the JS
    // thread (it may be waiting for the core lock): unavailable (ADR-038, decision 7).
    {
      std::lock_guard<std::mutex> lock(inboxMutex_);
      counters_.unavailableSyncPortCalls++;
    }
    bool first = false;
    try {
      std::lock_guard<std::mutex> lock(warnedMutex_);
      first = warned_.insert(portId).second;
    } catch (...) {
      first = false;
    }
    if (first) {
      char message[320];
      std::snprintf(
          message,
          sizeof(message),
          "port 0x%08x method 0x%08x is synchronous and implemented in JavaScript, but the core called it "
          "from one of its own threads, where JavaScript cannot run: answered unavailable. Implement it "
          "natively or make the method async (docs/REACT_NATIVE.md, limits). Reported once per port.",
          portId,
          methodId);
      log(3, message);
    }
    return 2;
  }
  uint8_t head[12];
  putU32(head, portId);
  putU32(head + 4, methodId);
  putU32(head + 8, portCallId);
  if (!append(RecordKind::PortCall, head, sizeof(head), args, len)) {
    return 2; // not queued (shutting down, or out of memory): nobody would answer
  }
  return 1;
}

} // namespace undra::rn
