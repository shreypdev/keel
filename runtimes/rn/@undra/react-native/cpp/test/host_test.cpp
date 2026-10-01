// The C++ half of @undra/react-native against the real native core, without React Native.
//
// `Host` (cpp/UndraHost.*) is the part of the module that owns the core: callbacks, the inbox, the
// ports, start and shutdown. This test links it with the playground core built for this machine
// (`undra build -C examples/playground --platform host`) and checks the rules of ADR-038 and the host
// contract of ADR-026 with the core's real threads: what a call made on "the JS thread" (this test's
// main thread, inside a `CallScope`) produces is queued without a wake; what the core produces on its
// own threads wakes; one inbox keeps commit order across threads; port replies are `malloc`ed and the
// core frees them; every `UndraBuf` is freed once; shutdown answers what is in flight and a new core
// can start afterwards. `run.sh` builds it with AddressSanitizer and UndefinedBehaviorSanitizer.
//
// Prints `ok - <name>` per check and exits non-zero on the first failure.

#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <utility>
#include <vector>

#include <sys/stat.h>
#include <unistd.h>

#include <atomic>
#include <fstream>
#include <iterator>
#include <optional>

#include "../UndraApi.h"
#include "../UndraDefaults.h"
#include "../UndraHost.h"

namespace undra::rn {

/// Reaches the port trampolines, which the core normally calls.
struct HostTestAccess {
  static uint8_t native(Host &h, uint32_t port, uint32_t method, uint32_t id, const std::vector<uint8_t> &args, UndraBuf *out) {
    return Host::nativePortTrampoline(&h, port, method, id, args.data(), static_cast<uint32_t>(args.size()), out);
  }
  static uint8_t js(Host &h, uint32_t port, uint32_t method, uint32_t id, const std::vector<uint8_t> &args, UndraBuf *out) {
    return Host::jsPortTrampoline(&h, port, method, id, args.data(), static_cast<uint32_t>(args.size()), out);
  }
};

} // namespace undra::rn

using namespace undra::rn;

namespace {

int g_checks = 0;

[[noreturn]] void fail(const std::string &what) {
  std::fprintf(stderr, "not ok - %s\n", what.c_str());
  std::exit(1);
}

void check(bool condition, const std::string &what) {
  if (!condition) {
    fail(what);
  }
}

void ok(const char *name) {
  ++g_checks;
  std::printf("ok - %s\n", name);
}

// ----- wire helpers (docs/SPEC.md section 3) -------------------------------------------------

struct Writer {
  std::vector<uint8_t> bytes;
  Writer &u8(uint8_t v) {
    bytes.push_back(v);
    return *this;
  }
  Writer &u32(uint32_t v) {
    for (int i = 0; i < 4; ++i) bytes.push_back(static_cast<uint8_t>(v >> (8 * i)));
    return *this;
  }
  Writer &i32(int32_t v) { return u32(static_cast<uint32_t>(v)); }
  Writer &u64(uint64_t v) {
    for (int i = 0; i < 8; ++i) bytes.push_back(static_cast<uint8_t>(v >> (8 * i)));
    return *this;
  }
  Writer &str(const std::string &s) {
    u32(static_cast<uint32_t>(s.size()));
    bytes.insert(bytes.end(), s.begin(), s.end());
    return *this;
  }
};

uint64_t getU64(const uint8_t *in) {
  uint64_t v = 0;
  for (int i = 0; i < 8; ++i) v |= static_cast<uint64_t>(in[i]) << (8 * i);
  return v;
}

std::vector<uint8_t> freeCall(uint32_t method, uint32_t callId, const std::vector<uint8_t> &args) {
  Writer w;
  w.u8(0).u64(0).u32(method).u32(callId);
  w.bytes.insert(w.bytes.end(), args.begin(), args.end());
  return w.bytes;
}

std::vector<uint8_t> methodCall(uint64_t handle, uint32_t method, uint32_t callId, const std::vector<uint8_t> &args) {
  Writer w;
  w.u8(1).u64(handle).u32(method).u32(callId);
  w.bytes.insert(w.bytes.end(), args.begin(), args.end());
  return w.bytes;
}

std::vector<uint8_t> constructorCall(uint32_t type, uint32_t method, uint32_t callId, const std::vector<uint8_t> &args) {
  Writer w;
  w.u8(2).u32(type).u32(method).u32(callId);
  w.bytes.insert(w.bytes.end(), args.begin(), args.end());
  return w.bytes;
}

struct Record {
  RecordKind kind;
  std::vector<uint8_t> payload;
};

std::vector<Record> parse(const std::vector<uint8_t> &inbox) {
  std::vector<Record> out;
  std::size_t at = 0;
  while (at < inbox.size()) {
    check(inbox.size() - at >= kRecordHeader, "a record header is complete");
    const auto kind = static_cast<RecordKind>(inbox[at]);
    const uint32_t len = getU32(&inbox[at + 1]);
    check(inbox.size() - at - kRecordHeader >= len, "a record body is complete");
    out.push_back({kind, std::vector<uint8_t>(inbox.begin() + at + kRecordHeader, inbox.begin() + at + kRecordHeader + len)});
    at += kRecordHeader + len;
  }
  return out;
}

std::vector<uint8_t> config() {
  Writer w;
  w.str("host-test").str("inproc").u8(1).u8(0).u8(2);
  return w.bytes;
}

// The playground's ids (examples/playground/generated/ts/src/ids.ts), recomputed from the names.
const uint32_t kAdd = fnv1a32("fn.add");
const uint32_t kAddLater = fnv1a32("fn.add_later");
const uint32_t kConfigureRemote = fnv1a32("fn.configure_remote");
const uint32_t kCounter = fnv1a32("Counter");
const uint32_t kCounterNew = fnv1a32("Counter.new");
const uint32_t kCounterIncrement = fnv1a32("Counter.increment");
const uint32_t kTodos = fnv1a32("Todos");
const uint32_t kTodosNew = fnv1a32("Todos.new");
const uint32_t kTodosAdd = fnv1a32("Todos.add");
const uint32_t kTodosSetFilter = fnv1a32("Todos.set_filter");
const uint32_t kProbe = fnv1a32("Probe");
const uint32_t kProbeNew = fnv1a32("Probe.new");
const uint32_t kProbeHang = fnv1a32("Probe.hang");
const uint32_t kRemoteQuery = fnv1a32("query.remote_todos");
const uint32_t kHttp = fnv1a32("port.Http");
const uint32_t kHttpRequest = fnv1a32("Http.request");
const uint32_t kKv = fnv1a32("port.Kv");
const uint32_t kAllSignals = 0xffffffffu;

/// A host whose wake function counts and signals.
struct Fixture {
  const Api *api = nullptr;
  std::mutex mutex;
  std::condition_variable cv;
  int wakes = 0;
  std::unique_ptr<Host> host;
  std::vector<Record> seen; // everything taken so far, in order
  uint32_t nextCall = 1;

  explicit Fixture(const Api *api) : api(api) {
    host = std::make_unique<Host>(*api, [this] {
      std::lock_guard<std::mutex> lock(mutex);
      ++wakes;
      cv.notify_all();
    });
  }

  int wakeCount() {
    std::lock_guard<std::mutex> lock(mutex);
    return wakes;
  }

  /// Takes the inbox (as the JS thread's drain does) and answers every async port call: `Kv` as
  /// an empty store (so query hydration finishes), everything else "unavailable", the way the
  /// TypeScript side answers a port it does not implement.
  std::vector<Record> drain() {
    std::vector<Record> records = parse(host->takeInbox());
    for (const Record &r : records) {
      if (r.kind == RecordKind::PortCall) {
        check(r.payload.size() >= 12, "a PortCall record has its ids");
        const uint32_t port = getU32(r.payload.data());
        const uint32_t method = getU32(&r.payload[4]);
        Writer reply;
        reply.u32(getU32(&r.payload[8]));
        if (port == fnv1a32("port.Kv") && method == fnv1a32("Kv.get")) {
          reply.u8(0).u8(0); // Option<Bytes>: None
        } else if (port == fnv1a32("port.Kv") && method == fnv1a32("Kv.list")) {
          reply.u8(0).u32(0); // Vec<String>: empty
        } else if (port == fnv1a32("port.Kv")) {
          reply.u8(0); // set, delete: ()
        } else {
          reply.u8(2);
        }
        api->port_reply(reply.bytes.data(), static_cast<uint32_t>(reply.bytes.size()));
      }
    }
    seen.insert(seen.end(), records.begin(), records.end());
    return records;
  }

  /// Drains until the core has been quiet (nothing queued, no wake) for 200 ms, at most 5 s.
  void settle() {
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    while (std::chrono::steady_clock::now() < deadline) {
      const int before = wakeCount();
      const std::size_t seenBefore = seen.size();
      drain();
      std::this_thread::sleep_for(std::chrono::milliseconds(200));
      if (wakeCount() == before && seen.size() == seenBefore && !host->hasRecords()) return;
    }
  }

  /// Waits up to 5 s until `pred` holds over everything taken so far, draining on every wake.
  template <typename P>
  bool waitFor(P pred) {
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    while (true) {
      drain();
      if (pred()) return true;
      std::unique_lock<std::mutex> lock(mutex);
      if (cv.wait_until(lock, deadline) == std::cv_status::timeout) {
        lock.unlock();
        drain();
        return pred();
      }
    }
  }

  /// `undra_call_sync` on "the JS thread"; returns the reply payload, freeing the buffer once.
  std::vector<uint8_t> callSync(const std::vector<uint8_t> &payload) {
    CallScope scope(*host, nullptr);
    UndraBuf buf = api->call_sync(payload.data(), static_cast<uint32_t>(payload.size()));
    std::vector<uint8_t> reply(buf.ptr, buf.ptr + buf.len);
    api->buf_free(buf);
    return reply;
  }

  uint32_t call(const std::vector<uint8_t> &payload) {
    CallScope scope(*host, nullptr);
    return api->call(payload.data(), static_cast<uint32_t>(payload.size()));
  }

  void observe(uint64_t handle) {
    CallScope scope(*host, nullptr);
    api->observe(handle, kAllSignals, 1);
  }

  bool replied(uint32_t callId) const {
    for (const Record &r : seen) {
      if (r.kind == RecordKind::Reply && r.payload.size() >= 5 && getU32(r.payload.data()) == callId) return true;
    }
    return false;
  }

  const Record *reply(uint32_t callId) const {
    for (const Record &r : seen) {
      if (r.kind == RecordKind::Reply && r.payload.size() >= 5 && getU32(r.payload.data()) == callId) return &r;
    }
    return nullptr;
  }
};

uint64_t constructStore(Fixture &f, uint32_t type, uint32_t ctor, const std::vector<uint8_t> &args = {}) {
  std::vector<uint8_t> reply = f.callSync(constructorCall(type, ctor, f.nextCall++, args));
  check(reply.size() == 13 && reply[4] == 0, "a constructor answers a handle");
  return getU64(&reply[5]);
}

/// The handle of each change-set entry with its txn id, in inbox order.
std::vector<std::pair<uint64_t, uint64_t>> changeSetTxns(const std::vector<Record> &records) {
  std::vector<std::pair<uint64_t, uint64_t>> out;
  for (const Record &r : records) {
    if (r.kind != RecordKind::ChangeSet) continue;
    check(r.payload.size() >= 12, "a change-set has a header");
    const uint64_t txn = getU64(r.payload.data());
    const uint32_t count = getU32(&r.payload[8]);
    std::size_t at = 12;
    for (uint32_t i = 0; i < count; ++i) {
      check(r.payload.size() - at >= 17, "a change-set entry is complete");
      const uint64_t handle = getU64(&r.payload[at]);
      const uint32_t len = getU32(&r.payload[at + 13]);
      out.emplace_back(handle, txn);
      at += 17 + len;
    }
  }
  return out;
}

void testsWithOneCore(const Api *api) {
  Fixture f(api);
  // Http and Kv answered by "JavaScript" (async), as the TypeScript side registers them.
  std::vector<PortSpec> ports{{kHttp, {}}, {kKv, {}}};
  const std::vector<uint8_t> cfg = config();
  {
    CallScope scope(*f.host, nullptr);
    const uint32_t code = f.host->start(cfg.data(), static_cast<uint32_t>(cfg.size()), ports);
    check(code == 0, "the core starts (undra_init 0), got " + std::to_string(code));
  }
  check(f.host->running(), "running after start");
  f.drain();
  ok("start registers the ports before undra_init and starts the core");

  // A second host in the same process is refused without touching the core.
  {
    Host other(*api, [] {});
    check(other.start(cfg.data(), static_cast<uint32_t>(cfg.size()), {}) == start_code::kBusy, "a second host is busy");
    check(f.host->running(), "the first host is untouched");
  }
  ok("one core per process: a second host is refused");

  // A synchronous call on the JS thread: the reply comes back directly, nothing wakes.
  {
    f.settle();
    const int before = f.wakeCount();
    Writer args;
    args.i32(2).i32(3);
    std::vector<uint8_t> reply = f.callSync(freeCall(kAdd, f.nextCall++, args.bytes));
    check(reply.size() == 9 && reply[4] == 0, "add answers ok");
    check(static_cast<int32_t>(getU32(&reply[5])) == 5, "2 + 3 == 5");
    check(f.wakeCount() == before, "a sync call wakes nobody");
  }
  ok("call_sync on the JS thread answers directly and wakes nobody");

  // An async call: accepted on the JS thread, answered from the core thread, which wakes.
  {
    const int before = f.wakeCount();
    Writer args;
    args.i32(20).i32(22).u32(5);
    const uint32_t id = f.nextCall++;
    check(f.call(freeCall(kAddLater, id, args.bytes)) == 0, "add_later is accepted");
    check(f.waitFor([&] { return f.replied(id); }), "add_later replies within 5 s");
    const Record *r = f.reply(id);
    check(r->payload[4] == 0 && static_cast<int32_t>(getU32(&r->payload[5])) == 42, "add_later answers 42");
    check(f.wakeCount() > before, "a reply from the core thread wakes the JS thread");
  }
  ok("an async reply arrives from the core thread through the inbox and wakes");

  // Observe and a sync method through undra_call: the change-sets are queued on the JS thread,
  // before the reply, without a wake.
  {
    const uint64_t counter = constructStore(f, kCounter, kCounterNew);
    f.settle();
    const int before = f.wakeCount();
    f.observe(counter);
    std::vector<Record> initial = f.drain();
    check(!initial.empty() && initial.front().kind == RecordKind::ChangeSet, "observe queues the initial change-set at once");
    const uint32_t id = f.nextCall++;
    check(f.call(methodCall(counter, kCounterIncrement, id, {})) == 0, "increment is accepted");
    std::vector<Record> after = f.drain();
    check(after.size() >= 2, "increment queued a change-set and a reply");
    check(after[0].kind == RecordKind::ChangeSet, "the change-set comes first");
    bool sawReply = false;
    for (const Record &r : after) sawReply = sawReply || (r.kind == RecordKind::Reply && getU32(r.payload.data()) == id);
    check(sawReply, "then the reply");
    check(f.wakeCount() == before, "nothing on the JS thread woke it");
  }
  ok("observe and a sync method queue change-sets before the reply, on the JS thread");

  // Commit order across threads: Todos.add commits on the core thread, set_filter on the JS
  // thread; one inbox keeps every store's txn ids increasing.
  {
    const uint64_t todos = constructStore(f, kTodos, kTodosNew);
    f.observe(todos);
    std::vector<uint32_t> adds;
    for (int i = 0; i < 40; ++i) {
      Writer title;
      title.str("item " + std::to_string(i));
      const uint32_t id = f.nextCall++;
      check(f.call(methodCall(todos, kTodosAdd, id, title.bytes)) == 0, "Todos.add is accepted");
      adds.push_back(id);
      Writer filter;
      filter.u8(0).u8(static_cast<uint8_t>(i % 3)); // u16 variant index
      check(f.call(methodCall(todos, kTodosSetFilter, f.nextCall++, filter.bytes)) == 0, "set_filter is accepted");
    }
    check(f.waitFor([&] {
      for (uint32_t id : adds)
        if (!f.replied(id)) return false;
      return true;
    }), "every add replies");
    std::map<uint64_t, uint64_t> last;
    std::size_t entries = 0;
    for (const auto &[handle, txn] : changeSetTxns(f.seen)) {
      if (handle != todos) continue;
      ++entries;
      auto it = last.find(handle);
      check(it == last.end() || txn >= it->second, "txn ids of one store never go back in the inbox");
      last[handle] = txn;
    }
    check(entries > 40, "the adds and filters committed change-sets");
  }
  ok("one inbox keeps a store's commit order across the core thread and the JS thread");

  // An async port call from the core thread: queued with its ids, answered with undra_port_reply.
  {
    Writer base;
    base.str("https://playground.test");
    check(f.callSync(freeCall(kConfigureRemote, f.nextCall++, base.bytes))[4] == 0, "configure_remote");
    Writer list;
    list.str("rn-host-test");
    const uint64_t query = constructStore(f, kRemoteQuery, kRemoteQuery, list.bytes);
    f.observe(query);
    check(f.waitFor([&] {
      for (const Record &r : f.seen)
        if (r.kind == RecordKind::PortCall && getU32(r.payload.data()) == kHttp && getU32(&r.payload[4]) == kHttpRequest) return true;
      return false;
    }), "the query's Http.request reaches the inbox");
  }
  ok("an async port call is queued (port, method, call id, args) and answered by undra_port_reply");

  // The native ports: answered at once, the reply malloc'ed (the test frees it, as the core does).
  {
    UndraBuf out{};
    check(HostTestAccess::native(*f.host, ports::kClock, ports::kClockNowMs, 7, {}, &out) == 0, "Clock.now_ms answers");
    check(out.len == 13 && getU32(out.ptr) == 7 && out.ptr[4] == 0 && out.cap == 0, "a PortReply with an i64");
    const auto now = std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::system_clock::now().time_since_epoch()).count();
    const auto told = static_cast<int64_t>(getU64(out.ptr + 5));
    check(told <= now && now - told < 5000, "the wall clock in ms");
    std::free(out.ptr);

    out = {};
    check(HostTestAccess::native(*f.host, ports::kClock, ports::kClockMonotonicNs, 8, {}, &out) == 0, "monotonic_ns answers");
    const uint64_t t1 = getU64(out.ptr + 5);
    std::free(out.ptr);
    out = {};
    HostTestAccess::native(*f.host, ports::kClock, ports::kClockMonotonicNs, 9, {}, &out);
    check(getU64(out.ptr + 5) >= t1, "monotonic never goes back");
    std::free(out.ptr);

    Writer len;
    len.u32(32);
    out = {};
    check(HostTestAccess::native(*f.host, ports::kRng, ports::kRngFill, 10, len.bytes, &out) == 0, "Rng.fill answers");
    check(out.len == 5 + 4 + 32 && getU32(out.ptr + 5) == 32, "a Bytes of 32");
    std::free(out.ptr);
    Writer tooMany;
    tooMany.u32((1u << 24) + 1);
    out = {};
    check(HostTestAccess::native(*f.host, ports::kRng, ports::kRngFill, 11, tooMany.bytes, &out) == 2 && out.ptr == nullptr, "too many bytes is unavailable");

    Writer record;
    record.u8(3).str("t").str("m");
    out = {};
    check(HostTestAccess::native(*f.host, ports::kLog, ports::kLogLog, 0, record.bytes, &out) == 1 && out.ptr == nullptr,
          "a fire-and-forget log record allocates nothing");
    std::vector<Record> logs = f.drain();
    check(!logs.empty() && logs.back().kind == RecordKind::Log && logs.back().payload == record.bytes, "the record is queued for JS");
  }
  ok("Clock, Rng and Log are answered natively with malloc'ed replies");

  // A JS port method the schema does not mark synchronous is queued (answer 1), from any thread.
  {
    UndraBuf out{};
    const uint32_t port = fnv1a32("port.TestAsync");
    const uint32_t method = fnv1a32("TestAsync.get");
    std::thread([&] { check(HostTestAccess::js(*f.host, port, method, 12, {}, &out) == 1, "queued as async"); }).join();
    check(out.ptr == nullptr, "nothing allocated for an async answer");
    std::vector<Record> queued = f.drain();
    check(!queued.empty() && queued.back().kind == RecordKind::PortCall && getU32(queued.back().payload.data()) == port &&
              getU32(&queued.back().payload[8]) == 12,
          "the PortCall record carries the port and the call id");
  }
  ok("a JS port method that is not synchronous is queued, never run on a core thread");

  // Shutdown with a call in flight: answered by the core, dropped here; no callback afterwards.
  {
    const uint64_t probe = constructStore(f, kProbe, kProbeNew);
    const uint32_t id = f.nextCall++;
    check(f.call(methodCall(probe, kProbeHang, id, {})) == 0, "Probe.hang is accepted");
    const auto started = std::chrono::steady_clock::now();
    f.host->shutdown();
    const auto took = std::chrono::steady_clock::now() - started;
    check(!f.host->running(), "not running after shutdown");
    check(took < std::chrono::seconds(5), "shutdown does not wait for the hanging call");
    check(Host::runningHost() == nullptr, "the process has no running host");
    f.host->shutdown(); // idempotent
  }
  ok("shutdown with a call in flight returns, is idempotent, and frees the process's slot");
}

void testsWithSyncPorts(const Api *api) {
  // A host whose plan marks a made-up port's method synchronous: JavaScript may answer it only on
  // the JS thread inside a host function.
  struct Answerer : SyncPortAnswerer {
    int calls = 0;
    uint8_t answer(uint32_t, uint32_t, uint32_t id, const uint8_t *, uint32_t, std::vector<uint8_t> &reply) noexcept override {
      ++calls;
      Writer w;
      w.u32(id).u8(0).u32(99);
      reply = w.bytes;
      return 0;
    }
  } answerer;
  const uint32_t port = fnv1a32("port.TestSync");
  const uint32_t method = fnv1a32("TestSync.get");
  Fixture f(api);
  const std::vector<uint8_t> cfg = config();
  {
    CallScope scope(*f.host, nullptr);
    check(f.host->start(cfg.data(), static_cast<uint32_t>(cfg.size()), {{port, {method}}}) == 0, "a new core starts after shutdown");
  }
  f.drain();
  ok("a core starts again after undra_shutdown (dev reload)");

  {
    Writer args;
    args.i32(1).i32(1);
    std::vector<uint8_t> reply = f.callSync(freeCall(kAdd, f.nextCall++, args.bytes));
    check(reply.size() == 9 && static_cast<int32_t>(getU32(&reply[5])) == 2, "the new core answers");
  }

  UndraBuf out{};
  // Off the JS thread (no scope): unavailable, and one warning record.
  std::thread([&] { check(HostTestAccess::js(*f.host, port, method, 21, {}, &out) == 2, "unavailable off the JS thread"); }).join();
  std::thread([&] { check(HostTestAccess::js(*f.host, port, method, 22, {}, &out) == 2, "still unavailable"); }).join();
  std::vector<Record> warnings = f.drain();
  int logs = 0;
  for (const Record &r : warnings) logs += r.kind == RecordKind::Log ? 1 : 0;
  check(logs == 1, "warned once per port, got " + std::to_string(logs));
  check(f.host->counters().unavailableSyncPortCalls == 2, "counted");

  // On the JS thread with an answerer: answered synchronously, the reply malloc'ed.
  {
    CallScope scope(*f.host, &answerer);
    check(HostTestAccess::js(*f.host, port, method, 23, {}, &out) == 0, "answered on the JS thread");
    check(!CallScope::inCallback(*f.host), "the callback flag is cleared afterwards");
  }
  check(answerer.calls == 1 && out.len == 9 && getU32(out.ptr) == 23 && getU32(out.ptr + 5) == 99 && out.cap == 0, "the JS reply");
  std::free(out.ptr);
  ok("a JS sync port runs only on the JS thread; elsewhere it is unavailable and warned once");

  f.host.reset(); // the destructor shuts the core down
  check(Host::runningHost() == nullptr, "the destructor shut the core down");
  ok("destroying a running host shuts its core down");
}

void testsWithAReloadRace(const Api *api) {
  // A dev reload on iOS: the old runtime's module shuts its core down on the old JS thread while
  // the new runtime, on a JS thread of its own, starts one. The new start waits for that shutdown
  // instead of failing "busy" (the old binding is already gone, so nothing can stop it first).
  const std::vector<uint8_t> cfg = config();
  for (int round = 0; round < 20; ++round) {
    Fixture old(api);
    {
      CallScope scope(*old.host, nullptr);
      check(old.host->start(cfg.data(), static_cast<uint32_t>(cfg.size()), {}) == 0, "the old core starts");
    }
    old.drain();
    Fixture fresh(api);
    std::thread oldJsThread([&] { old.host->shutdown(); });
    while (old.host->running()) {
      std::this_thread::yield();
    }
    uint32_t code = 0;
    {
      CallScope scope(*fresh.host, nullptr);
      code = fresh.host->start(cfg.data(), static_cast<uint32_t>(cfg.size()), {});
    }
    oldJsThread.join();
    check(code == 0, "a start during another host's shutdown waits for it (round " + std::to_string(round) + "), got " +
              std::to_string(code));
    check(Host::runningHost() == fresh.host.get(), "the new host holds the process's slot");
    {
      Writer args;
      args.i32(4).i32(5);
      std::vector<uint8_t> reply = fresh.callSync(freeCall(kAdd, fresh.nextCall++, args.bytes));
      check(reply.size() == 9 && static_cast<int32_t>(getU32(&reply[5])) == 9, "the new core answers");
    }
    fresh.host->shutdown();
  }
  check(Host::runningHost() == nullptr, "no host holds the slot afterwards");
  ok("a start while another host is shutting down waits for it (a reload on two JS threads)");
}


// ----- the native default ports (ADR-038 amendment B) -------------------------------------------

const uint32_t kSecureStorePort = fnv1a32("port.SecureStore");
const uint32_t kFsPort = fnv1a32("port.Fs");
const uint32_t kKvPut = fnv1a32("fn.kv_put");
const uint32_t kKvGet = fnv1a32("fn.kv_get");
const uint32_t kKvKeys = fnv1a32("fn.kv_keys");
const uint32_t kSecretPut = fnv1a32("fn.secret_put");
const uint32_t kSecretGet = fnv1a32("fn.secret_get");
const uint32_t kFileWrite = fnv1a32("fn.file_write");
const uint32_t kFileRead = fnv1a32("fn.file_read");
const uint32_t kFileList = fnv1a32("fn.file_list");
const uint32_t kFileDelete = fnv1a32("fn.file_delete");
const uint32_t kDevice = fnv1a32("Device");
const uint32_t kDeviceNew = fnv1a32("Device.new");

/// What the test platform's secret store and network monitor share with the test.
struct TestPlatformState {
  std::mutex mutex;
  std::condition_variable cv;
  std::map<std::string, std::vector<uint8_t>> secrets;
  bool fail = false;
  bool block = false;
  bool entered = false;
  ConnectivitySource::Report report;
  bool started = false;
  bool stopped = false;
  bool slotHeldAtStop = false;
  std::atomic<int> workersStarted{0};
  std::atomic<int> workersEnded{0};
};

/// An in-memory secret store that can be made to fail or to block (to hold a worker mid-job).
struct TestSecrets final : SecretStore {
  explicit TestSecrets(std::shared_ptr<TestPlatformState> state) : s(std::move(state)) {}
  bool get(const std::string &key, std::optional<std::vector<uint8_t>> &value, std::string &error) override {
    std::lock_guard<std::mutex> lock(s->mutex);
    if (s->fail) {
      error = "the test keychain is locked";
      return false;
    }
    auto it = s->secrets.find(key);
    if (it == s->secrets.end()) value.reset();
    else value = it->second;
    return true;
  }
  bool set(const std::string &key, const std::vector<uint8_t> &value, std::string &) override {
    std::unique_lock<std::mutex> lock(s->mutex);
    s->entered = true;
    s->cv.notify_all();
    s->cv.wait(lock, [&] { return !s->block; });
    s->secrets[key] = value;
    return true;
  }
  bool remove(const std::string &key, std::string &) override {
    std::lock_guard<std::mutex> lock(s->mutex);
    s->secrets.erase(key);
    return true;
  }
  bool list(const std::string &prefix, std::vector<std::string> &keys, std::string &) override {
    std::lock_guard<std::mutex> lock(s->mutex);
    keys.clear();
    for (const auto &[key, value] : s->secrets)
      if (key.compare(0, prefix.size(), prefix) == 0) keys.push_back(key);
    return true;
  }
  std::string describe() const override { return "test secrets"; }
  std::shared_ptr<TestPlatformState> s;
};

/// A network monitor the test drives: it reports from threads of its own, as the real ones do.
struct ScriptedSource final : ConnectivitySource {
  explicit ScriptedSource(std::shared_ptr<TestPlatformState> state) : s(std::move(state)) {}
  bool start(Report report) override {
    {
      std::lock_guard<std::mutex> lock(s->mutex);
      s->report = std::move(report);
      s->started = true;
    }
    emit(true, NetKind::Wifi); // the current state first
    return true;
  }
  void stop() noexcept override {
    std::lock_guard<std::mutex> lock(s->mutex);
    s->stopped = true;
    s->slotHeldAtStop = Host::runningHost() != nullptr;
    s->report = nullptr;
  }
  void emit(bool online, NetKind kind) {
    std::thread([this, online, kind] {
      Report report;
      {
        std::lock_guard<std::mutex> lock(s->mutex);
        report = s->report;
      }
      if (report) report(online, kind);
    }).join();
  }
  std::shared_ptr<TestPlatformState> s;
};

struct TestPlatform final : Platform {
  TestPlatform(std::string kv, std::string fs) : kvDir(std::move(kv)), fsDir(std::move(fs)) {}
  std::string kvDirectory() override { return kvDir; }
  KvNaming kvNaming() override { return KvNaming::Fnv; }
  std::string fsRoot() override { return fsDir; }
  std::unique_ptr<SecretStore> makeSecretStore() override { return std::make_unique<TestSecrets>(state); }
  std::unique_ptr<ConnectivitySource> makeConnectivity() override { return std::make_unique<ScriptedSource>(state); }
  void workerStarted() noexcept override { state->workersStarted++; }
  void workerEnded() noexcept override { state->workersEnded++; }
  std::string kvDir;
  std::string fsDir;
  std::shared_ptr<TestPlatformState> state = std::make_shared<TestPlatformState>();
};

/// The last value of every signal of `handle` in `records`' change-sets.
std::map<uint32_t, std::vector<uint8_t>> signalValues(const std::vector<Record> &records, uint64_t handle) {
  std::map<uint32_t, std::vector<uint8_t>> out;
  for (const Record &r : records) {
    if (r.kind != RecordKind::ChangeSet) continue;
    const uint32_t count = getU32(&r.payload[8]);
    std::size_t at = 12;
    for (uint32_t i = 0; i < count; ++i) {
      const uint64_t h = getU64(&r.payload[at]);
      const uint32_t signal = getU32(&r.payload[at + 8]);
      const uint32_t len = getU32(&r.payload[at + 13]);
      if (h == handle) out[signal] = std::vector<uint8_t>(r.payload.begin() + at + 17, r.payload.begin() + at + 17 + len);
      at += 17 + len;
    }
  }
  return out;
}

std::string fileText(const std::string &path) {
  std::ifstream in(path, std::ios::binary);
  return std::string(std::istreambuf_iterator<char>(in), std::istreambuf_iterator<char>());
}

void testsWithNativeDefaults(const Api *api) {
  char pattern[] = "/tmp/undra-rn-defaults.XXXXXX";
  const char *base = ::mkdtemp(pattern);
  check(base != nullptr, "a temporary directory");
  TestPlatform platform(std::string(base) + "/kv", std::string(base) + "/fs");
  const std::vector<uint32_t> native = nativePortsOf(platform);
  check(native.size() == 4, "the test platform offers Kv, SecureStore, Fs and Connectivity");

  Fixture f(api);
  StartOptions options;
  options.nativePorts = native;
  options.platform = &platform;
  const std::vector<uint8_t> cfg = config();
  {
    CallScope scope(*f.host, nullptr);
    // The schema's ports as the plan lists them; the native ones are taken by the module.
    const uint32_t code = f.host->start(cfg.data(), static_cast<uint32_t>(cfg.size()), {{kHttp, {}}, {kKv, {}}, {kSecureStorePort, {}}, {kFsPort, {}}}, options);
    check(code == 0, "the core starts with native defaults, got " + std::to_string(code));
  }
  f.settle();
  {
    std::lock_guard<std::mutex> lock(platform.state->mutex);
    check(platform.state->started, "the Connectivity source started after undra_init");
  }
  ok("start registers the native defaults and starts the Connectivity source after undra_init");

  auto await = [&](uint32_t function, const std::vector<uint8_t> &args) -> const Record * {
    const uint32_t id = f.nextCall++;
    check(f.call(freeCall(function, id, args)) == 0, "the call is accepted");
    check(f.waitFor([&] { return f.replied(id); }), "it replies within 5 s");
    return f.reply(id);
  };
  auto noJsPortCall = [&](uint32_t port) {
    for (const Record &r : f.seen)
      if (r.kind == RecordKind::PortCall && getU32(r.payload.data()) == port) return false;
    return true;
  };

  // Kv through the core: the file is the Swift layout's, and JavaScript never saw a port call.
  {
    Writer put;
    put.str("rn.check").u32(2).u8('v').u8('1');
    const Record *r = await(kKvPut, put.bytes);
    check(r->payload[4] == 0, "kv_put answers ok");
    const std::string file = platform.kvDir + "/" + kvFileName(KvNaming::Fnv, "rn.check");
    const std::string expected = std::string("\x08\x00\x00\x00", 4) + "rn.check" + "v1";
    check(fileText(file) == expected, "the entry is in the Kv directory, in the native layout");
    Writer get;
    get.str("rn.check");
    r = await(kKvGet, get.bytes);
    check(r->payload.size() == 5 + 1 + 4 + 2 && r->payload[5] == 1 && r->payload[10] == 'v', "kv_get answers Some(\"v1\")");
    Writer keys;
    keys.str("rn.");
    r = await(kKvKeys, keys.bytes);
    check(r->payload.size() >= 9 && getU32(&r->payload[5]) == 1, "kv_keys lists the one key");
    check(noJsPortCall(kKv), "no Kv port call reached JavaScript");
  }
  ok("Kv through the core is answered natively, in the native file layout");

  // SecureStore through the core, and a failing store answering "unavailable" with a log record.
  {
    Writer put;
    put.str("token").u32(3).u8('s').u8('3').u8('c');
    check(await(kSecretPut, put.bytes)->payload[4] == 0, "secret_put answers ok");
    {
      std::lock_guard<std::mutex> lock(platform.state->mutex);
      check(platform.state->secrets["token"] == std::vector<uint8_t>({'s', '3', 'c'}), "the platform's store has it");
    }
    Writer get;
    get.str("token");
    const Record *r = await(kSecretGet, get.bytes);
    check(r->payload[4] == 0 && r->payload[5] == 1, "secret_get answers Some");
    {
      std::lock_guard<std::mutex> lock(platform.state->mutex);
      platform.state->fail = true;
    }
    r = await(kSecretGet, get.bytes);
    check(r->payload[4] == 2, "a failing store is unavailable: the core's call fails (status 2), never 'missing'");
    bool logged = false;
    for (const Record &rec : f.seen) {
      const std::string text(rec.payload.begin(), rec.payload.end());
      logged = logged || (rec.kind == RecordKind::Log && text.find("SecureStore.get failed: the test keychain is locked") != std::string::npos);
    }
    check(logged, "and the failure is logged");
    {
      std::lock_guard<std::mutex> lock(platform.state->mutex);
      platform.state->fail = false;
    }
    check(noJsPortCall(kSecureStorePort), "no SecureStore port call reached JavaScript");
  }
  ok("SecureStore through the core; a failing store answers unavailable and logs");

  // Fs through the core, with its typed errors.
  {
    Writer write;
    write.str("notes/a.txt").u32(2).u8('h').u8('i');
    check(await(kFileWrite, write.bytes)->payload[4] == 0, "file_write answers Ok(())");
    check(fileText(platform.fsDir + "/notes/a.txt") == "hi", "the file is under the Fs root");
    Writer read;
    read.str("notes/a.txt");
    const Record *r = await(kFileRead, read.bytes);
    check(r->payload[4] == 0 && getU32(&r->payload[5]) == 2, "file_read answers the bytes");
    Writer list;
    list.str("notes");
    r = await(kFileList, list.bytes);
    check(r->payload[4] == 0 && getU32(&r->payload[5]) == 1, "file_list answers one name");
    Writer escape;
    escape.str("../escape.txt");
    r = await(kFileRead, escape.bytes);
    check(r->payload[4] == 1 && r->payload.size() == 7 && r->payload[5] == 1 && r->payload[6] == 0, "'../' is the typed FsError::Denied (status 1, tag 1)");
    r = await(kFileWrite, [&] {
      Writer w;
      w.str("../escape.txt").u32(0);
      return w.bytes;
    }());
    check(r->payload[4] == 1 && r->payload[5] == 1, "writing outside the root: Denied");
    struct stat st{};
    check(::stat((std::string(base) + "/escape.txt").c_str(), &st) != 0, "nothing was written outside the root");
    Writer del;
    del.str("notes");
    check(await(kFileDelete, del.bytes)->payload[4] == 0, "file_delete of a directory answers Ok(())");
    r = await(kFileRead, read.bytes);
    check(r->payload[4] == 1 && r->payload[5] == 0, "then reading it is FsError::NotFound (tag 0)");
    check(noJsPortCall(kFsPort), "no Fs port call reached JavaScript");
  }
  ok("Fs through the core, with its typed errors; '../' is Denied");

  // Connectivity: the first report reached the core before any store existed; identical
  // consecutive reports are sent once.
  {
    const uint64_t device = constructStore(f, kDevice, kDeviceNew);
    f.observe(device);
    f.drain();
    auto values = signalValues(f.seen, device);
    check(values[0] == std::vector<uint8_t>{1}, "online");
    check(values[1] == std::vector<uint8_t>({0, 0}), "on Wi-Fi");
    check(values[3] == std::vector<uint8_t>({1, 0, 0, 0}), "one Connectivity report so far");
    ScriptedSource source(platform.state);
    source.emit(false, NetKind::None);
    source.emit(false, NetKind::None);
    source.emit(true, NetKind::Cellular);
    check(f.waitFor([&] {
      auto v = signalValues(f.seen, device);
      return v[3] == std::vector<uint8_t>({3, 0, 0, 0});
    }), "three reports reached the core");
    values = signalValues(f.seen, device);
    check(values[0] == std::vector<uint8_t>{1} && values[1] == std::vector<uint8_t>({1, 0}), "online on cellular now");
    f.settle();
    check(signalValues(f.seen, device)[3] == std::vector<uint8_t>({3, 0, 0, 0}), "the repeated offline report was sent once");
  }
  ok("Connectivity reports reach the core from the source's thread, the first one at start, repeats once");

  check(f.host->counters().nativePortCalls > 0, "native port calls are counted");

  // Shutdown while a SecureStore job is running: the event source stops, the worker finishes its
  // job and is joined, and only then is the process's slot released.
  {
    {
      std::lock_guard<std::mutex> lock(platform.state->mutex);
      platform.state->block = true;
      platform.state->entered = false;
    }
    Writer put;
    put.str("late").u32(0);
    check(f.call(freeCall(kSecretPut, f.nextCall++, put.bytes)) == 0, "secret_put is accepted");
    {
      std::unique_lock<std::mutex> lock(platform.state->mutex);
      check(platform.state->cv.wait_for(lock, std::chrono::seconds(5), [&] { return platform.state->entered; }), "the worker is inside the store");
    }
    std::thread jsThread([&] { f.host->shutdown(); });
    std::this_thread::sleep_for(std::chrono::milliseconds(200));
    check(Host::runningHost() == f.host.get(), "the slot is held while a worker is still running a job");
    {
      std::lock_guard<std::mutex> lock(platform.state->mutex);
      check(platform.state->stopped && platform.state->slotHeldAtStop, "the Connectivity source stopped before the slot was released");
      platform.state->block = false;
    }
    platform.state->cv.notify_all();
    jsThread.join();
    check(Host::runningHost() == nullptr, "then the slot is released");
    check(platform.state->workersStarted.load() == 3 && platform.state->workersEnded.load() == 3, "three workers ran, and all three ended");
    ScriptedSource(platform.state).emit(false, NetKind::None); // nothing to report to: a no-op
  }
  ok("shutdown stops the source, joins the workers mid-job, then releases the slot");

  std::string cmd = std::string("rm -rf '") + base + "'";
  if (std::system(cmd.c_str()) != 0) std::printf("# could not remove %s\n", base);
}

} // namespace

int main() {
  std::string error;
  const Api *api = loadApi(error);
  if (api == nullptr) {
    fail("load the core: " + error);
  }
  check(api->abi_version == kAbiVersion, "the core speaks C ABI 1");
  testsWithOneCore(api);
  testsWithSyncPorts(api);
  testsWithAReloadRace(api);
  testsWithNativeDefaults(api);
  std::printf("# %d checks passed\n", g_checks);
  return 0;
}
