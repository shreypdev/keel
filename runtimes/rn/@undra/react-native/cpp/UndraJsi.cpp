#include "UndraJsi.h"

#include <algorithm>
#include <cmath>
#include <cstring>
#include <map>
#include <utility>
#include <vector>

namespace jsi = facebook::jsi;

namespace undra::rn {

namespace {

/// Set on the JS thread while a drain delivers records: a drain started meanwhile (a host function
/// called from the sink's JavaScript) leaves its records to the running one, so order is kept.
thread_local bool t_draining = false;

/// The binding that started each core of the process (by namespace), so a new runtime (dev reload)
/// can stop a core whose runtime is gone before it starts its own. Never destroyed.
std::mutex &g_ownerMutex = *new std::mutex;
std::map<std::string, std::weak_ptr<Binding>> &g_owners = *new std::map<std::string, std::weak_ptr<Binding>>;

/// `globalThis.__undraNative[name_space]`: the object `install` put there, or `undefined`.
jsi::Value installedObject(jsi::Runtime &rt, const std::string &name_space) {
  jsi::Value all = rt.global().getProperty(rt, "__undraNative");
  if (!all.isObject()) {
    return jsi::Value::undefined();
  }
  return all.getObject(rt).getProperty(rt, name_space.c_str());
}

/// Bytes that JavaScript receives as an `ArrayBuffer` and owns from then on: an inbox batch, or
/// a copy of a sync port's arguments.
class VectorBuffer final : public jsi::MutableBuffer {
 public:
  explicit VectorBuffer(std::vector<uint8_t> bytes) : bytes_(std::move(bytes)) {}
  size_t size() const override {
    return bytes_.size();
  }
  uint8_t *data() override {
    return bytes_.empty() ? &empty_ : bytes_.data();
  }

 private:
  std::vector<uint8_t> bytes_;
  uint8_t empty_ = 0;
};

/// An `UndraBuf` the core returned, as an `ArrayBuffer`: no copy, and `undra_buf_free` exactly once,
/// when the garbage collector releases the buffer (ADR-038, decision 5).
class CoreBuffer final : public jsi::MutableBuffer {
 public:
  CoreBuffer(const Api &api, UndraBuf buf) : api_(api), buf_(buf) {}
  ~CoreBuffer() override {
    api_.buf_free(buf_);
  }
  CoreBuffer(const CoreBuffer &) = delete;
  CoreBuffer &operator=(const CoreBuffer &) = delete;
  size_t size() const override {
    return buf_.ptr != nullptr ? buf_.len : 0;
  }
  uint8_t *data() override {
    return buf_.ptr != nullptr ? buf_.ptr : &empty_;
  }

 private:
  const Api &api_;
  UndraBuf buf_;
  uint8_t empty_ = 0;
};

jsi::Value arrayBuffer(jsi::Runtime &rt, std::shared_ptr<jsi::MutableBuffer> buffer) {
  return jsi::Value(rt, jsi::ArrayBuffer(rt, std::move(buffer)));
}

/// `buf` as an `ArrayBuffer` that owns it. If that cannot be built (out of memory) `buf` is freed
/// here before the error reaches JavaScript, so it is freed exactly once either way.
jsi::Value coreArrayBuffer(jsi::Runtime &rt, const Api &api, UndraBuf buf) {
  std::shared_ptr<CoreBuffer> owned;
  try {
    owned = std::make_shared<CoreBuffer>(api, buf);
  } catch (...) {
    api.buf_free(buf);
    throw;
  }
  return arrayBuffer(rt, std::move(owned)); // a throw from here destroys `owned`, which frees `buf`
}

[[noreturn]] void raise(jsi::Runtime &rt, const std::string &message) {
  throw jsi::JSError(rt, "@undra/react-native: " + message);
}

uint32_t u32Arg(jsi::Runtime &rt, const jsi::Value *args, size_t count, size_t i, const char *what) {
  if (i >= count || !args[i].isNumber()) {
    raise(rt, std::string(what) + " must be a number");
  }
  const double value = args[i].asNumber();
  if (!(value >= 0.0 && value <= 4294967295.0) || std::floor(value) != value) {
    raise(rt, std::string(what) + " must be an unsigned 32-bit integer");
  }
  return static_cast<uint32_t>(value);
}

struct Bytes {
  const uint8_t *ptr;
  uint32_t len;
};

/// `args[i]` an `ArrayBuffer`, `args[i + 1]` a byte offset, `args[i + 2]` a byte length: a view the
/// core borrows for the duration of the call (no copy).
Bytes bytesArg(jsi::Runtime &rt, const jsi::Value *args, size_t count, size_t i) {
  if (i + 2 >= count || !args[i].isObject()) {
    raise(rt, "expected (ArrayBuffer, byteOffset, byteLength)");
  }
  jsi::Object object = args[i].getObject(rt);
  if (!object.isArrayBuffer(rt)) {
    raise(rt, "expected an ArrayBuffer");
  }
  jsi::ArrayBuffer buffer = object.getArrayBuffer(rt);
  const size_t size = buffer.size(rt);
  const uint32_t offset = u32Arg(rt, args, count, i + 1, "byteOffset");
  const uint32_t length = u32Arg(rt, args, count, i + 2, "byteLength");
  if (static_cast<size_t>(offset) + length > size) {
    raise(rt, "the view is outside its ArrayBuffer");
  }
  return Bytes{buffer.data(rt) + offset, length};
}

std::vector<uint32_t> u32Array(jsi::Runtime &rt, const jsi::Value &value, const char *what) {
  std::vector<uint32_t> out;
  if (value.isUndefined() || value.isNull()) {
    return out;
  }
  if (!value.isObject() || !value.getObject(rt).isArray(rt)) {
    raise(rt, std::string(what) + " must be an array of numbers");
  }
  jsi::Array array = value.getObject(rt).getArray(rt);
  const size_t length = array.size(rt);
  out.reserve(length);
  for (size_t i = 0; i < length; ++i) {
    jsi::Value item = array.getValueAtIndex(rt, i);
    out.push_back(u32Arg(rt, &item, 1, 0, what));
  }
  return out;
}

std::string takeString(const Api &api, UndraBuf buf) {
  std::string text;
  if (buf.ptr != nullptr && buf.len > 0) {
    text.assign(reinterpret_cast<const char *>(buf.ptr), buf.len);
  }
  api.buf_free(buf);
  return text;
}

/// `__undraNative[name_space]`: the receiver of a method call, or the installed object.
jsi::Object nativeOf(jsi::Runtime &rt, const jsi::Value &thisVal, const std::string &name_space) {
  if (thisVal.isObject()) {
    return thisVal.getObject(rt);
  }
  jsi::Value installed = installedObject(rt, name_space);
  if (!installed.isObject()) {
    raise(rt, "__undraNative." + name_space + " is not installed");
  }
  return installed.getObject(rt);
}

/// Runs a JavaScript sync port (`native.portSync`) on the JS thread, from inside a core callback
/// (ADR-038, decision 7). Everything is caught: nothing unwinds into the core.
class JsAnswerer final : public SyncPortAnswerer {
 public:
  JsAnswerer(jsi::Runtime &rt, const jsi::Object &native) : rt_(rt), native_(native) {}

  uint8_t answer(
      uint32_t portId,
      uint32_t methodId,
      uint32_t portCallId,
      const uint8_t *args,
      uint32_t len,
      std::vector<uint8_t> &reply) noexcept override {
    try {
      jsi::Value fn = native_.getProperty(rt_, "portSync");
      if (!fn.isObject() || !fn.getObject(rt_).isFunction(rt_)) {
        return 2;
      }
      // A copy: the core's bytes are valid only during this callback, and JavaScript may keep them.
      std::vector<uint8_t> copy;
      if (len > 0 && args != nullptr) {
        copy.assign(args, args + len);
      }
      jsi::Value result = fn.getObject(rt_).getFunction(rt_).call(
          rt_,
          jsi::Value(static_cast<double>(portId)),
          jsi::Value(static_cast<double>(methodId)),
          jsi::Value(static_cast<double>(portCallId)),
          arrayBuffer(rt_, std::make_shared<VectorBuffer>(std::move(copy))));
      if (!result.isObject()) {
        return 2;
      }
      jsi::Object object = result.getObject(rt_);
      if (!object.isArrayBuffer(rt_)) {
        return 2;
      }
      jsi::ArrayBuffer buffer = object.getArrayBuffer(rt_);
      const uint8_t *data = buffer.data(rt_);
      reply.assign(data, data + buffer.size(rt_));
      return 0;
    } catch (...) {
      return 2;
    }
  }

 private:
  jsi::Runtime &rt_;
  const jsi::Object &native_;
};

} // namespace

Binding::Binding(const Api &api, std::shared_ptr<facebook::react::CallInvoker> invoker)
    : api(api), invoker_(std::move(invoker)) {}

Binding::~Binding() {
  detach();
}

std::shared_ptr<Host> Binding::host() const {
  std::lock_guard<std::mutex> lock(mutex_);
  return host_;
}

void Binding::detach() noexcept {
  alive_->store(false);
  std::shared_ptr<Host> host;
  std::unique_ptr<FrameSource> frames;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    host = std::move(host_);
    frames = std::move(frames_);
  }
  if (host) {
    host->shutdown();
  }
}

void Binding::postDrain(
    const std::shared_ptr<facebook::react::CallInvoker> &invoker,
    const std::shared_ptr<std::atomic<bool>> &alive,
    const std::weak_ptr<Binding> &weak) {
  if (!alive->load()) {
    return;
  }
  // May throw (out of memory posting the task): `Host::requestDrain` catches it and re-arms.
  invoker->invokeAsync([weak, alive](jsi::Runtime &rt) {
    if (!alive->load()) {
      return;
    }
    std::shared_ptr<Binding> self = weak.lock(); // on the JS thread: never inside a core callback
    if (!self) {
      return;
    }
    try {
      jsi::Value native = installedObject(rt, self->api.name_space);
      if (native.isObject()) {
        self->drain(rt, native.getObject(rt));
      }
    } catch (...) {
      // Nothing may escape into React Native's scheduler (a fatal JS error in a release build); a
      // record that arrives later wakes the JS thread again.
    }
  });
}

void Binding::postFrame(
    const std::shared_ptr<facebook::react::CallInvoker> &invoker,
    const std::shared_ptr<std::atomic<bool>> &alive,
    const std::weak_ptr<Binding> &weak) noexcept {
  if (!alive->load()) {
    return;
  }
  try {
    invoker->invokeAsync([weak, alive](jsi::Runtime &rt) {
      if (!alive->load()) {
        return;
      }
      std::shared_ptr<Binding> self = weak.lock(); // on the JS thread, as in postDrain
      if (!self) {
        return;
      }
      try {
        jsi::Value native = installedObject(rt, self->api.name_space);
        if (!native.isObject()) {
          return;
        }
        jsi::Value frame = native.getObject(rt).getProperty(rt, "frame");
        if (frame.isObject() && frame.getObject(rt).isFunction(rt)) {
          frame.getObject(rt).getFunction(rt).call(rt);
        }
      } catch (...) {
        // The frame callback reports its own failures; nothing may escape into the scheduler.
      }
    });
  } catch (...) {
    // Out of memory posting the task (on the display link's or the choreographer's thread, where
    // nothing may unwind): the JavaScript scheduler's 100 ms backstop runs the drains instead.
  }
}

void Binding::drain(jsi::Runtime &rt, const jsi::Object &native) {
  std::shared_ptr<Host> host = this->host();
  if (!host || t_draining || CallScope::current() != nullptr) {
    return;
  }
  t_draining = true;
  struct Reset {
    ~Reset() {
      t_draining = false;
    }
  } reset;
  for (int round = 0; round < 1000; ++round) {
    std::vector<uint8_t> batch = host->takeInbox();
    if (batch.empty()) {
      return;
    }
    jsi::Value sink = native.getProperty(rt, "sink");
    if (!sink.isObject() || !sink.getObject(rt).isFunction(rt)) {
      return; // no transport attached: the records are dropped
    }
    jsi::Value buffer = arrayBuffer(rt, std::make_shared<VectorBuffer>(std::move(batch)));
    try {
      sink.getObject(rt).getFunction(rt).call(rt, buffer);
    } catch (...) {
      // The sink reports its own failures (NativeTransport catches everything); a throw here is a
      // bug there, and the next batch must still be delivered.
    }
  }
  // Still busy after 1000 rounds: the rest goes to a later drain, so the JS thread stays live.
  host->requestDrain();
}

jsi::Value Binding::start(jsi::Runtime &rt, const jsi::Object &native, const jsi::Value *args, size_t count) {
  const Bytes config = bytesArg(rt, args, count, 0);
  const jsi::Value none = jsi::Value::undefined();
  const std::vector<uint32_t> portIds = u32Array(rt, count > 3 ? args[3] : none, "ports");
  const std::vector<uint32_t> syncPairs = u32Array(rt, count > 4 ? args[4] : none, "syncMethods");
  if (syncPairs.size() % 2 != 0) {
    raise(rt, "syncMethods must hold (portId, methodId) pairs");
  }
  // The standard ports JavaScript asks the module to answer natively (ADR-038 amendment B, B7): kept
  // only when this platform has them.
  const std::vector<uint32_t> asked = u32Array(rt, count > 5 ? args[5] : none, "nativePorts");
  StartOptions options;
  if (!asked.empty()) {
    if (std::shared_ptr<Platform> platform = this->platform()) {
      const std::vector<uint32_t> offered = nativePortsOf(*platform);
      for (uint32_t id : asked) {
        if (std::find(offered.begin(), offered.end(), id) != offered.end()) options.nativePorts.push_back(id);
      }
      options.platform = std::move(platform);
    }
  }
  std::vector<PortSpec> specs;
  specs.reserve(portIds.size());
  for (uint32_t id : portIds) {
    PortSpec spec;
    spec.portId = id;
    for (size_t i = 0; i < syncPairs.size(); i += 2) {
      if (syncPairs[i] == id) {
        spec.syncMethods.push_back(syncPairs[i + 1]);
      }
    }
    specs.push_back(std::move(spec));
  }
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (host_ && host_->running()) {
      return jsi::Value(static_cast<double>(start_code::kAlreadyStarted));
    }
  }
  // This core started by another runtime of this process (a reload whose old module was not
  // destroyed yet) is stopped first: a core runs once per process, and that runtime is gone or
  // going. Other cores (other namespaces) are not touched.
  std::shared_ptr<Binding> owner;
  {
    std::lock_guard<std::mutex> lock(g_ownerMutex);
    auto found = g_owners.find(api.name_space);
    if (found != g_owners.end()) {
      owner = found->second.lock();
    }
  }
  if (owner && owner.get() != this) {
    owner->detach();
  }
  std::weak_ptr<Binding> weak = weak_from_this();
  auto host = std::make_shared<Host>(api, [invoker = invoker_, alive = alive_, weak] {
    postDrain(invoker, alive, weak); // runs on core threads: no strong reference to the binding
  });
  uint32_t code = 0;
  {
    JsAnswerer answerer(rt, native);
    CallScope scope(*host, &answerer);
    code = host->start(config.ptr, config.len, specs, options);
  }
  if (code != 0) {
    return jsi::Value(static_cast<double>(code));
  }
  {
    std::lock_guard<std::mutex> lock(mutex_);
    host_ = host;
    if (!frames_) {
      frames_ = makeFrameSource([invoker = invoker_, alive = alive_, weak] { postFrame(invoker, alive, weak); });
    }
  }
  {
    std::lock_guard<std::mutex> lock(g_ownerMutex);
    g_owners[api.name_space] = weak;
  }
  drain(rt, native);
  return jsi::Value(0);
}

std::shared_ptr<Platform> Binding::platform() {
  std::lock_guard<std::mutex> lock(mutex_);
  if (!platformTried_) {
    platformTried_ = true;
    try {
      std::unique_ptr<Platform> made = makePlatform(platformError_);
      platform_ = std::shared_ptr<Platform>(std::move(made));
    } catch (...) {
      platformError_ = "the platform could not be reached";
    }
  }
  return platform_;
}

void Binding::shutdownHost(jsi::Runtime &rt) {
  std::shared_ptr<Host> host = this->host();
  if (!host) {
    return;
  }
  if (CallScope::inCallback(*host) || CallScope::current() != nullptr) {
    raise(rt, "the core cannot be shut down from inside a port call");
  }
  host->shutdown();
  host->takeInbox(); // nobody listens any more
  std::lock_guard<std::mutex> lock(mutex_);
  if (host_ == host) {
    host_.reset();
  }
}

void Binding::install(jsi::Runtime &rt) {
  const std::string &name_space = api.name_space;
  jsi::Value existing = rt.global().getProperty(rt, "__undraNative");
  jsi::Object all = existing.isObject() ? existing.getObject(rt) : jsi::Object(rt);
  if (all.getProperty(rt, name_space.c_str()).isObject()) {
    return;
  }
  std::shared_ptr<Binding> self = shared_from_this();
  jsi::Object native(rt);
  auto define = [&](const char *name, unsigned argc, jsi::HostFunctionType fn) {
    native.setProperty(
        rt, name, jsi::Function::createFromHostFunction(rt, jsi::PropNameID::forAscii(rt, name), argc, std::move(fn)));
  };

  // Runs `body` (which enters the core) inside a CallScope, then drains what it queued on this
  // thread before returning to JavaScript (ADR-038, decision 4a). Without a running core of its
  // own (not started, closed, or stopped because a reloaded runtime of this process started this
  // core again) the binding never reaches the C ABI: the core may then belong to another runtime,
  // whose calls, call ids, port call ids and state this one must not touch. `refused`
  // is the answer then, the one the C ABI gives with no core (status 5, ignored, unavailable).
  auto enter = [self](jsi::Runtime &rt, const jsi::Value &thisVal, auto &&body, auto &&refused) -> jsi::Value {
    std::shared_ptr<Host> host = self->host();
    if (!host || !host->running()) {
      return refused();
    }
    jsi::Object native = nativeOf(rt, thisVal, self->api.name_space);
    jsi::Value result;
    try {
      JsAnswerer answerer(rt, native);
      CallScope scope(*host, &answerer);
      result = body();
    } catch (...) {
      // What the core queued on this thread meanwhile woke nobody (the drain below would have
      // delivered it): post a drain for it, then let the error reach JavaScript.
      host->requestDrain();
      throw;
    }
    self->drain(rt, native);
    return result;
  };
  const auto none = [] { return jsi::Value::undefined(); };

  define("abiVersion", 0, [self](jsi::Runtime &, const jsi::Value &, const jsi::Value *, size_t) {
    return jsi::Value(static_cast<double>(self->api.abi_version));
  });
  define("schemaHash", 0, [self](jsi::Runtime &rt, const jsi::Value &, const jsi::Value *, size_t) {
    return jsi::Value(rt, jsi::BigInt::fromUint64(rt, self->api.schema_hash));
  });
  define("schemaJson", 0, [self](jsi::Runtime &rt, const jsi::Value &, const jsi::Value *, size_t) {
    return jsi::Value(rt, jsi::String::createFromUtf8(rt, takeString(self->api, self->api.schema_json())));
  });
  define("start", 6, [self](jsi::Runtime &rt, const jsi::Value &thisVal, const jsi::Value *args, size_t count) {
    return self->start(rt, nativeOf(rt, thisVal, self->api.name_space), args, count);
  });
  define("shutdown", 0, [self](jsi::Runtime &rt, const jsi::Value &, const jsi::Value *, size_t) {
    self->shutdownHost(rt);
    return jsi::Value::undefined();
  });
  // `undra_call` with no core: 5, refused.
  define("call", 3, [self, enter](jsi::Runtime &rt, const jsi::Value &thisVal, const jsi::Value *args, size_t count) {
    const Bytes payload = bytesArg(rt, args, count, 0);
    return enter(
        rt,
        thisVal,
        [&] { return jsi::Value(static_cast<double>(self->api.call(payload.ptr, payload.len))); },
        [] { return jsi::Value(5); });
  });
  // `undefined` with no core of this runtime (NativeTransport throws UndraTransportError("closed")).
  define("callSync", 3, [self, enter, none](jsi::Runtime &rt, const jsi::Value &thisVal, const jsi::Value *args, size_t count) {
    const Bytes payload = bytesArg(rt, args, count, 0);
    return enter(rt, thisVal, [&] { return coreArrayBuffer(rt, self->api, self->api.call_sync(payload.ptr, payload.len)); }, none);
  });
  define("cancel", 1, [self, enter, none](jsi::Runtime &rt, const jsi::Value &thisVal, const jsi::Value *args, size_t count) {
    const uint32_t callId = u32Arg(rt, args, count, 0, "callId");
    return enter(
        rt,
        thisVal,
        [&] {
          self->api.cancel(callId);
          return jsi::Value::undefined();
        },
        none);
  });
  define("streamCredit", 2, [self, enter, none](jsi::Runtime &rt, const jsi::Value &thisVal, const jsi::Value *args, size_t count) {
    const uint32_t callId = u32Arg(rt, args, count, 0, "callId");
    const uint32_t credit = u32Arg(rt, args, count, 1, "credit");
    return enter(
        rt,
        thisVal,
        [&] {
          self->api.stream_credit(callId, credit);
          return jsi::Value::undefined();
        },
        none);
  });
  define("observe", 4, [self, enter, none](jsi::Runtime &rt, const jsi::Value &thisVal, const jsi::Value *args, size_t count) {
    const uint64_t handle = static_cast<uint64_t>(u32Arg(rt, args, count, 0, "handleLo")) |
        (static_cast<uint64_t>(u32Arg(rt, args, count, 1, "handleHi")) << 32);
    const uint32_t signalId = u32Arg(rt, args, count, 2, "signalId");
    const bool on = count > 3 && args[3].isBool() && args[3].getBool();
    return enter(
        rt,
        thisVal,
        [&] {
          self->api.observe(handle, signalId, on ? 1 : 0);
          return jsi::Value::undefined();
        },
        none);
  });
  define("release", 2, [self, enter, none](jsi::Runtime &rt, const jsi::Value &thisVal, const jsi::Value *args, size_t count) {
    const uint64_t handle = static_cast<uint64_t>(u32Arg(rt, args, count, 0, "handleLo")) |
        (static_cast<uint64_t>(u32Arg(rt, args, count, 1, "handleHi")) << 32);
    return enter(
        rt,
        thisVal,
        [&] {
          self->api.release(handle);
          return jsi::Value::undefined();
        },
        none);
  });
  define("portReply", 3, [self, enter, none](jsi::Runtime &rt, const jsi::Value &thisVal, const jsi::Value *args, size_t count) {
    const Bytes payload = bytesArg(rt, args, count, 0);
    return enter(
        rt,
        thisVal,
        [&] {
          self->api.port_reply(payload.ptr, payload.len);
          return jsi::Value::undefined();
        },
        none);
  });
  define("event", 5, [self, enter, none](jsi::Runtime &rt, const jsi::Value &thisVal, const jsi::Value *args, size_t count) {
    const uint32_t portId = u32Arg(rt, args, count, 0, "portId");
    const uint32_t methodId = u32Arg(rt, args, count, 1, "methodId");
    const Bytes payload = bytesArg(rt, args, count, 2);
    return enter(
        rt,
        thisVal,
        [&] {
          self->api.event(portId, methodId, payload.ptr, payload.len);
          return jsi::Value::undefined();
        },
        none);
  });
  define("timerFired", 1, [self, enter, none](jsi::Runtime &rt, const jsi::Value &thisVal, const jsi::Value *args, size_t count) {
    const uint32_t timerId = u32Arg(rt, args, count, 0, "timerId");
    return enter(
        rt,
        thisVal,
        [&] {
          self->api.timer_fired(timerId);
          return jsi::Value::undefined();
        },
        none);
  });
  // Every store of this runtime's core; `undefined` without one (the core may be another
  // runtime's).
  define("snapshot", 0, [self](jsi::Runtime &rt, const jsi::Value &, const jsi::Value *, size_t) {
    std::shared_ptr<Host> host = self->host();
    if (!host || !host->running()) {
      return jsi::Value::undefined();
    }
    return coreArrayBuffer(rt, self->api, self->api.snapshot());
  });
  // `undra_restore` with no core: 6, unavailable.
  define("restore", 3, [self, enter](jsi::Runtime &rt, const jsi::Value &thisVal, const jsi::Value *args, size_t count) {
    const Bytes payload = bytesArg(rt, args, count, 0);
    return enter(
        rt,
        thisVal,
        [&] { return jsi::Value(static_cast<double>(self->api.restore(payload.ptr, payload.len))); },
        [] { return jsi::Value(6); });
  });
  define("statsJson", 0, [self](jsi::Runtime &rt, const jsi::Value &, const jsi::Value *, size_t) {
    return jsi::Value(rt, jsi::String::createFromUtf8(rt, takeString(self->api, self->api.stats_json())));
  });
  define("hostCounters", 0, [self](jsi::Runtime &rt, const jsi::Value &, const jsi::Value *, size_t) {
    jsi::Object out(rt);
    std::shared_ptr<Host> host = self->host();
    const HostCounters c = host ? host->counters() : HostCounters{};
    out.setProperty(rt, "records", static_cast<double>(c.records));
    out.setProperty(rt, "bytes", static_cast<double>(c.bytes));
    out.setProperty(rt, "wakes", static_cast<double>(c.wakes));
    out.setProperty(rt, "dropped", static_cast<double>(c.dropped));
    out.setProperty(rt, "nativePortCalls", static_cast<double>(c.nativePortCalls));
    out.setProperty(rt, "jsSyncPortCalls", static_cast<double>(c.jsSyncPortCalls));
    out.setProperty(rt, "unavailableSyncPortCalls", static_cast<double>(c.unavailableSyncPortCalls));
    return jsi::Value(rt, out);
  });
  // The default ports this platform answers natively, and where they keep their data
  // (ADR-038 amendment B): `{ ports, kv, fs, secureStore }`, or `{ ports: [], error }`.
  define("platformDefaults", 0, [self](jsi::Runtime &rt, const jsi::Value &, const jsi::Value *, size_t) {
    jsi::Object out(rt);
    std::shared_ptr<Platform> platform = self->platform();
    std::vector<uint32_t> ids;
    if (platform) {
      ids = nativePortsOf(*platform);
      out.setProperty(rt, "kv", jsi::String::createFromUtf8(rt, platform->kvDirectory()));
      out.setProperty(rt, "fs", jsi::String::createFromUtf8(rt, platform->fsRoot()));
      if (std::unique_ptr<SecretStore> secrets = platform->makeSecretStore()) {
        out.setProperty(rt, "secureStore", jsi::String::createFromUtf8(rt, secrets->describe()));
      }
    } else {
      std::lock_guard<std::mutex> lock(self->mutex_);
      out.setProperty(rt, "error", jsi::String::createFromUtf8(rt, self->platformError_));
    }
    jsi::Array ports(rt, ids.size());
    for (size_t i = 0; i < ids.size(); ++i) ports.setValueAtIndex(rt, i, jsi::Value(static_cast<double>(ids[i])));
    out.setProperty(rt, "ports", ports);
    return jsi::Value(rt, out);
  });
  define("requestFrame", 0, [self](jsi::Runtime &, const jsi::Value &, const jsi::Value *, size_t) {
    std::lock_guard<std::mutex> lock(self->mutex_);
    return jsi::Value(self->frames_ != nullptr && self->frames_->request());
  });

  // The table's namespace, which `NativeTransport` checks against the one it was given.
  native.setProperty(rt, "namespace", jsi::String::createFromUtf8(rt, name_space));
  all.setProperty(rt, name_space.c_str(), native);
  if (!existing.isObject()) {
    rt.global().setProperty(rt, "__undraNative", all);
  }
}

} // namespace undra::rn
