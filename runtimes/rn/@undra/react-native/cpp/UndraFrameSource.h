// A vsync source for the mirror's drains (ADR-038, decision 6).
//
// React Native 0.87's `requestAnimationFrame` is a zero-delay timer in bridgeless mode, so the
// module supplies the frame itself: `CADisplayLink` on iOS (`ios/UndraFrameSource.mm`) and
// `AChoreographer` on Android (`UndraFrameSource.cpp`). The source is paused while nothing waits.
#pragma once

#include <functional>
#include <memory>

namespace undra::rn {

/// Runs a callback once at the next display frame, each time it is armed.
class FrameSource {
 public:
  virtual ~FrameSource() = default;
  /// Arms the source: `onFrame` runs once at the next vsync, on a platform thread (it must only post
  /// work). Arming an armed source does nothing. `false` when this platform has no frame source
  /// (the JavaScript side then falls back to a timer).
  virtual bool request() = 0;
};

/// The frame source of this platform. Called on the JS thread.
std::unique_ptr<FrameSource> makeFrameSource(std::function<void()> onFrame);

} // namespace undra::rn
