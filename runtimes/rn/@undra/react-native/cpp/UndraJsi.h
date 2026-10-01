// The JSI side of @undra/react-native (ADR-038, decisions 1, 4, 5 and 11).
//
// `Binding` belongs to one JS runtime and one core (ADR-044: one per namespace). `install` puts
// `globalThis.__undraNative[<namespace>]` into the runtime: plain JSI host functions over the core's
// table, which `NativeTransport` (src/transport.ts) calls. The binding never stores a JSI value: the
// inbox sink, the sync-port function and the frame callback are read from that object (`sink`,
// `portSync`, `frame`) when needed, so nothing outlives the runtime it came from, and a dev reload
// leaves no dangling JSI handle behind.
#pragma once

#include <jsi/jsi.h>

#include <atomic>
#include <memory>
#include <mutex>
#include <string>

#include <ReactCommon/CallInvoker.h>

#include "UndraApi.h"
#include "UndraDefaults.h"
#include "UndraFrameSource.h"
#include "UndraHost.h"

namespace undra::rn {

/// One JS runtime's view of one core of the process: its host object, its inbox, its frame source.
class Binding : public std::enable_shared_from_this<Binding> {
 public:
  Binding(const Api &api, std::shared_ptr<facebook::react::CallInvoker> invoker);
  ~Binding();
  Binding(const Binding &) = delete;
  Binding &operator=(const Binding &) = delete;

  /// Installs `globalThis.__undraNative[api.name_space]` in `rt` (once per runtime and core).
  void install(facebook::jsi::Runtime &rt);
  /// The JS runtime is going away (the TurboModule's destructor): shuts the core down if this
  /// binding started it, posts nothing more. Callable from any thread but a core callback.
  void detach() noexcept;

  /// Posts a drain of the inbox to the JS thread (the host's wake function). Safe from any thread,
  /// a core callback included: it holds no strong reference to the binding, so a callback thread is
  /// never the one that destroys it (whose destructor shuts the core down). Throws only what
  /// posting throws (out of memory), which `Host::requestDrain` catches.
  static void postDrain(
      const std::shared_ptr<facebook::react::CallInvoker> &invoker,
      const std::shared_ptr<std::atomic<bool>> &alive,
      const std::weak_ptr<Binding> &weak);
  /// Posts the frame callback to the JS thread (the frame source's callback); as `postDrain`.
  static void postFrame(
      const std::shared_ptr<facebook::react::CallInvoker> &invoker,
      const std::shared_ptr<std::atomic<bool>> &alive,
      const std::weak_ptr<Binding> &weak) noexcept;
  /// Delivers the inbox to `native.sink` until it is empty (at most 1000 rounds). Only the
  /// outermost drain on the thread delivers; one inside a sync port's JavaScript does nothing.
  void drain(facebook::jsi::Runtime &rt, const facebook::jsi::Object &native);

  /// The running host, if this binding started one.
  std::shared_ptr<Host> host() const;

  /// The core this binding reaches (its namespace is `api.name_space`).
  const Api &api;

 private:
  facebook::jsi::Value start(facebook::jsi::Runtime &rt, const facebook::jsi::Object &native, const facebook::jsi::Value *args, size_t count);
  /// The phone's platform of the default ports, made on first use on the JS thread (Android resolves
  /// its Java class there); null with `platformError_` set when there is none.
  std::shared_ptr<Platform> platform();
  void shutdownHost(facebook::jsi::Runtime &rt);

  std::shared_ptr<facebook::react::CallInvoker> invoker_;
  /// False once the runtime is going away: nothing is posted to it any more.
  std::shared_ptr<std::atomic<bool>> alive_ = std::make_shared<std::atomic<bool>>(true);
  mutable std::mutex mutex_;
  std::shared_ptr<Host> host_;
  std::unique_ptr<FrameSource> frames_;
  bool platformTried_ = false;
  std::shared_ptr<Platform> platform_;
  std::string platformError_;
};

} // namespace undra::rn
