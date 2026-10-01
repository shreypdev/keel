// The JSI side of @undra/react-native (ADR-038, decisions 1, 4, 5 and 11).
//
// `Binding` belongs to one JS runtime. `install` puts `globalThis.__undraNative` into it: plain JSI
// host functions over the C ABI, which `NativeTransport` (src/transport.ts) calls. The binding never
// stores a JSI value: the inbox sink, the sync-port function and the frame callback are read from
// `__undraNative` (`sink`, `portSync`, `frame`) when needed, so nothing outlives the runtime it came
// from, and a dev reload leaves no dangling JSI handle behind.
#pragma once

#include <jsi/jsi.h>

#include <atomic>
#include <memory>
#include <mutex>
#include <string>

#include <ReactCommon/CallInvoker.h>

#include "UndraApi.h"
#include "UndraFrameSource.h"
#include "UndraHost.h"

namespace undra::rn {

/// One JS runtime's view of the process's core.
class Binding : public std::enable_shared_from_this<Binding> {
 public:
  Binding(const Api &api, std::shared_ptr<facebook::react::CallInvoker> invoker);
  ~Binding();
  Binding(const Binding &) = delete;
  Binding &operator=(const Binding &) = delete;

  /// Installs `globalThis.__undraNative` in `rt` (once per runtime).
  void install(facebook::jsi::Runtime &rt);
  /// The JS runtime is going away (the TurboModule's destructor): shuts the core down if this
  /// binding started it, posts nothing more. Callable from any thread but a core callback.
  void detach() noexcept;

  /// Posts a drain of the inbox to the JS thread (the host's wake function).
  void postDrain() noexcept;
  /// Posts the frame callback to the JS thread (the frame source's callback).
  void postFrame() noexcept;
  /// Delivers the inbox to `native.sink` until it is empty (at most 1000 rounds). Only the
  /// outermost drain on the thread delivers; one inside a sync port's JavaScript does nothing.
  void drain(facebook::jsi::Runtime &rt, const facebook::jsi::Object &native);

  /// The running host, if this binding started one.
  std::shared_ptr<Host> host() const;

  const Api &api;

 private:
  facebook::jsi::Value start(facebook::jsi::Runtime &rt, const facebook::jsi::Object &native, const facebook::jsi::Value *args, size_t count);
  void shutdownHost(facebook::jsi::Runtime &rt);

  std::shared_ptr<facebook::react::CallInvoker> invoker_;
  std::atomic<bool> detached_{false};
  mutable std::mutex mutex_;
  std::shared_ptr<Host> host_;
  std::unique_ptr<FrameSource> frames_;
};

} // namespace undra::rn
