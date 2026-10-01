// The Apple frame source: a `CADisplayLink` on the main run loop, paused while nothing waits
// (ADR-038, decision 6). Each armed tick posts one drain to the JS thread and pauses the link again.
#import <Foundation/Foundation.h>
#import <QuartzCore/QuartzCore.h>

#include <atomic>
#include <functional>
#include <memory>

#include "UndraFrameSource.h"

/// The display link's target. Main thread only, except `wanted`, which the JS thread sets.
@interface UndraDisplayLinkTarget : NSObject
- (instancetype)initWithCallback:(std::function<void()>)callback wanted:(std::shared_ptr<std::atomic<bool>>)wanted;
- (void)resume;
- (void)invalidate;
@end

@implementation UndraDisplayLinkTarget {
  std::function<void()> _callback;
  std::shared_ptr<std::atomic<bool>> _wanted;
  CADisplayLink *_link;
}

- (instancetype)initWithCallback:(std::function<void()>)callback wanted:(std::shared_ptr<std::atomic<bool>>)wanted
{
  if ((self = [super init])) {
    _callback = std::move(callback);
    _wanted = std::move(wanted);
  }
  return self;
}

- (void)resume
{
  if (_link == nil) {
    _link = [CADisplayLink displayLinkWithTarget:self selector:@selector(tick:)];
    [_link addToRunLoop:[NSRunLoop mainRunLoop] forMode:NSRunLoopCommonModes];
  }
  _link.paused = NO;
}

- (void)tick:(CADisplayLink *)link
{
  if (_wanted->exchange(false) && _callback) {
    _callback();
  }
  // A request made after the exchange dispatched a `resume` that runs after this tick.
  if (!_wanted->load()) {
    link.paused = YES;
  }
}

- (void)invalidate
{
  [_link invalidate];
  _link = nil;
  _callback = nullptr;
}

@end

namespace undra::rn {

namespace {

class DisplayLinkSource final : public FrameSource {
 public:
  explicit DisplayLinkSource(std::function<void()> onFrame)
      : wanted_(std::make_shared<std::atomic<bool>>(false)),
        target_([[UndraDisplayLinkTarget alloc] initWithCallback:std::move(onFrame) wanted:wanted_]) {}

  ~DisplayLinkSource() override {
    UndraDisplayLinkTarget *target = target_;
    dispatch_async(dispatch_get_main_queue(), ^{
      [target invalidate];
    });
  }

  bool request() override {
    if (wanted_->exchange(true)) {
      return true;
    }
    UndraDisplayLinkTarget *target = target_;
    dispatch_async(dispatch_get_main_queue(), ^{
      [target resume];
    });
    return true;
  }

 private:
  std::shared_ptr<std::atomic<bool>> wanted_;
  UndraDisplayLinkTarget *target_;
};

} // namespace

std::unique_ptr<FrameSource> makeFrameSource(std::function<void()> onFrame) {
  return std::make_unique<DisplayLinkSource>(std::move(onFrame));
}

} // namespace undra::rn
