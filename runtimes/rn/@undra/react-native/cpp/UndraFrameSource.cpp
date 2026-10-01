// The Android frame source (`AChoreographer`), and the "no frame source" fallback of platforms
// other than Android and Apple. Apple's (`CADisplayLink`) is `ios/UndraFrameSource.mm`.
#include "UndraFrameSource.h"

#if defined(__ANDROID__)

#include <android/choreographer.h>

#include <atomic>
#include <mutex>

namespace undra::rn {

namespace {

/// What a posted frame callback reaches; shared so a callback that fires after the source is gone
/// finds it expired.
struct State {
  std::function<void()> onFrame;
  std::atomic<bool> armed{false};
};

#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"

void onVsync(long /*frameTimeNanos*/, void *data) {
  auto *weak = static_cast<std::weak_ptr<State> *>(data);
  if (std::shared_ptr<State> state = weak->lock()) {
    state->armed.store(false);
    if (state->onFrame) {
      state->onFrame();
    }
  }
  delete weak;
}

/// `AChoreographer` of the thread that first armed it: the JS thread, a looper thread in React
/// Native, so the callback runs between two JS tasks and only posts the drain.
class ChoreographerSource final : public FrameSource {
 public:
  explicit ChoreographerSource(std::function<void()> onFrame) : state_(std::make_shared<State>()) {
    state_->onFrame = std::move(onFrame);
  }

  bool request() override {
    if (state_->armed.exchange(true)) {
      return true;
    }
    if (choreographer_ == nullptr) {
      choreographer_ = AChoreographer_getInstance();
    }
    if (choreographer_ == nullptr) {
      state_->armed.store(false);
      return false; // not a looper thread: the JavaScript side uses a timer
    }
    // `postFrameCallback` (API 24) rather than `postFrameCallback64` (API 29): React Native's
    // minimum SDK is 24, and the frame time is not used.
    AChoreographer_postFrameCallback(choreographer_, &onVsync, new std::weak_ptr<State>(state_));
    return true;
  }

 private:
  std::shared_ptr<State> state_;
  AChoreographer *choreographer_ = nullptr;
};

#pragma clang diagnostic pop

} // namespace

std::unique_ptr<FrameSource> makeFrameSource(std::function<void()> onFrame) {
  return std::make_unique<ChoreographerSource>(std::move(onFrame));
}

} // namespace undra::rn

#elif !defined(__APPLE__)

namespace undra::rn {

namespace {

class NoFrameSource final : public FrameSource {
 public:
  bool request() override {
    return false;
  }
};

} // namespace

std::unique_ptr<FrameSource> makeFrameSource(std::function<void()> /*onFrame*/) {
  return std::make_unique<NoFrameSource>();
}

} // namespace undra::rn

#endif
